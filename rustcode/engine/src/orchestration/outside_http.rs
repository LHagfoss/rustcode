//! Local-only, stateless Streamable HTTP MCP with public-client OAuth/PKCE.
//! Secrets are never logged; restarting discards unexchanged authorization codes.
use super::{hub::CallRequest, outside};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::OnceCell,
};
use tokio_util::sync::CancellationToken;

static URL: OnceCell<String> = OnceCell::const_new();
struct Code {
    client: String,
    redirect: String,
    challenge: String,
    scope: outside::Scope,
    expires: Instant,
}
struct Server {
    base: String,
    settings: outside::Settings,
    runner: Arc<dyn outside::Runner>,
    codes: Mutex<HashMap<String, Code>>,
    slots: Arc<tokio::sync::Semaphore>,
}
fn secret() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}
fn pkce(verifier: &str, challenge: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
}
// High port, away from common development server defaults.
const DEFAULT_PORT: u16 = 43871;
const PORT_FILE: &str = "outside-http-port";

async fn bind_port(preferred: u16) -> std::io::Result<TcpListener> {
    for port in preferred..=u16::MAX {
        match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AddrInUse,
        "no free outside-agent HTTP port",
    ))
}

async fn bind_configured(path: &std::path::Path) -> Result<TcpListener, String> {
    let preferred = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT);
    let listener = bind_port(preferred).await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    crate::service::write_private(path, port.to_string().as_bytes())
        .map_err(|e| format!("could not save outside-agent HTTP port: {e}"))?;
    Ok(listener)
}

/// Start once per engine, reusing the saved loopback port when available.
pub async fn ensure() -> Result<String, String> {
    URL.get_or_try_init(|| async {
        let path = crate::config::get_config_dir()
            .ok_or("could not determine the config directory")?
            .join(PORT_FILE);
        let listener = bind_configured(&path).await?;
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().map_err(|e| e.to_string())?.port()
        );
        let server = Arc::new(Server {
            base: base.clone(),
            settings: outside::Settings::machine().ok_or("configuration unavailable")?,
            runner: Arc::new(outside::EngineRunner),
            codes: Mutex::new(HashMap::new()),
            slots: Arc::new(tokio::sync::Semaphore::new(32)),
        });
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let server = server.clone();
                if let Ok(permit) = server.slots.clone().try_acquire_owned() {
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ =
                            tokio::time::timeout(Duration::from_secs(600), serve(stream, server))
                                .await;
                    });
                }
            }
        });
        Ok(format!("{base}/mcp"))
    })
    .await
    .cloned()
}
struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}
async fn read(stream: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let end = loop {
        if bytes.len() >= 65536 {
            return None;
        }
        let mut buf = [0; 1024];
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
        if let Some(i) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    if first.next()? != "HTTP/1.1" {
        return None;
    }
    let mut headers = HashMap::new();
    for line in lines.filter(|s| !s.is_empty()) {
        let (k, v) = line.split_once(':')?;
        if headers
            .insert(k.to_ascii_lowercase(), v.trim().to_owned())
            .is_some()
        {
            return None;
        }
    }
    if headers.contains_key("transfer-encoding") {
        return None;
    }
    let length = headers
        .get("content-length")
        .map(|v| v.parse::<usize>())
        .transpose()
        .ok()?
        .unwrap_or(0);
    if length > 65536 {
        return None;
    }
    while bytes.len() < end + length {
        let mut buf = [0; 1024];
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    Some(Request {
        method,
        target,
        headers,
        body: bytes[end..end + length].to_vec(),
    })
}
async fn respond(stream: &mut TcpStream, status: u16, value: Value, extra: &str) {
    let body = value.to_string();
    let response = format!(
        "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}
fn fields(text: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(text).into_owned().collect()
}
async fn serve(mut stream: TcpStream, server: Arc<Server>) {
    let Some(req) = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let authority = server.base.trim_start_matches("http://");
    // Host validation prevents DNS rebinding. Browser requests from other origins are refused.
    if req.headers.get("host").map(String::as_str) != Some(authority)
        || req.headers.get("origin").is_some_and(|o| o != &server.base)
    {
        respond(&mut stream, 403, json!({"error":"forbidden"}), "").await;
        return;
    }
    let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
    let resource = format!("{}/mcp", server.base);
    let mut status = 200;
    let mut extra = String::new();
    let result = match (req.method.as_str(), path) {
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            json!({"resource":resource,"authorization_servers":[server.base],"scopes_supported":["read_only","delegate","manage"],"bearer_methods_supported":["header"]})
        }
        ("GET", "/.well-known/oauth-authorization-server") => {
            json!({"issuer":server.base,"authorization_endpoint":format!("{}/authorize",server.base),"token_endpoint":format!("{}/token",server.base),"response_types_supported":["code"],"grant_types_supported":["authorization_code"],"token_endpoint_auth_methods_supported":["none"],"code_challenge_methods_supported":["S256"],"scopes_supported":["read_only","delegate","manage"]})
        }
        ("GET", "/authorize") => {
            let q = fields(query.as_bytes());
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let redirect = url::Url::parse(get("redirect_uri")).ok().filter(|u| {
                u.scheme() == "http"
                    && u.host_str() == Some("127.0.0.1")
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.fragment().is_none()
            });
            let scope = match get("scope") {
                "read_only" => Some(outside::Scope::ReadOnly),
                "delegate" => Some(outside::Scope::Delegate),
                "manage" => Some(outside::Scope::Manage),
                _ => None,
            };
            if get("response_type") != "code"
                || get("code_challenge_method") != "S256"
                || get("code_challenge").len() != 43
                || URL_SAFE_NO_PAD.decode(get("code_challenge")).is_err()
                || get("client_id").is_empty()
                || get("client_id").len() > 200
                || get("resource") != resource
                || redirect.is_none()
                || scope.is_none()
            {
                status = 400;
                json!({"error":"invalid_request"})
            } else {
                let scope = scope.unwrap();
                let mut destination = redirect.unwrap();
                if outside::approve_connection(get("client_id"), destination.as_str(), scope).await
                {
                    let code = secret();
                    {
                        let mut codes = server.codes.lock().unwrap();
                        codes.retain(|_, c| c.expires > Instant::now());
                        codes.insert(
                            code.clone(),
                            Code {
                                client: get("client_id").into(),
                                redirect: get("redirect_uri").into(),
                                challenge: get("code_challenge").into(),
                                scope,
                                expires: Instant::now() + Duration::from_secs(60),
                            },
                        );
                    }
                    destination
                        .query_pairs_mut()
                        .append_pair("code", &code)
                        .append_pair("state", get("state"));
                } else {
                    destination
                        .query_pairs_mut()
                        .append_pair("error", "access_denied")
                        .append_pair("state", get("state"));
                }
                status = 302;
                extra = format!("Location: {}\r\n", destination);
                json!({})
            }
        }
        ("POST", "/token") => {
            let q = fields(&req.body);
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let code = server.codes.lock().unwrap().remove(get("code"));
            if let Some(code) = code.filter(|c| {
                c.expires > Instant::now()
                    && c.client == get("client_id")
                    && c.redirect == get("redirect_uri")
                    && get("resource") == resource
                    && get("grant_type") == "authorization_code"
                    && pkce(get("code_verifier"), &c.challenge)
            }) {
                match outside::create_http(&server.settings.grants, &code.client, code.scope) {
                    Ok((_, token)) => {
                        json!({"access_token":token,"token_type":"Bearer","expires_in":3600,"scope":code.scope.as_str()})
                    }
                    Err(_) => {
                        status = 503;
                        json!({"error":"temporarily_unavailable"})
                    }
                }
            } else {
                status = 400;
                json!({"error":"invalid_grant"})
            }
        }
        (_, "/mcp") => {
            let token = req
                .headers
                .get("authorization")
                .and_then(|v| v.strip_prefix("Bearer "))
                .unwrap_or("");
            if outside::authenticate_transport_at(&server.settings.grants, token, true).is_none() {
                status = 401;
                extra = format!(
                    "WWW-Authenticate: Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"\r\n",
                    server.base
                );
                json!({"error":"invalid_token"})
            } else if req.method != "POST" {
                status = 405;
                json!({"error":"method_not_allowed"})
            } else {
                match serde_json::from_slice::<Value>(&req.body) {
                    Ok(v) if v["jsonrpc"] == "2.0" && v["method"].is_string() => {
                        let id = v.get("id").cloned();
                        let method = v["method"].as_str().unwrap();
                        if id.is_none() {
                            status = 202;
                            json!({})
                        } else {
                            let tool = match method {
                                "initialize" => outside::CONTROL_INSTRUCTIONS,
                                "tools/list" => outside::CONTROL_TOOLS,
                                "tools/call" => v["params"]["name"].as_str().unwrap_or(""),
                                "ping" => "",
                                _ => "",
                            };
                            if method == "ping" {
                                json!({"jsonrpc":"2.0","id":id,"result":{}})
                            } else if tool.is_empty() {
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                            } else {
                                let request = CallRequest {
                                    session_id: String::new(),
                                    token: token.into(),
                                    tool: tool.into(),
                                    args: v["params"]["arguments"].clone(),
                                    outside: true,
                                };
                                let answer = outside::answer_transport_with(
                                    &server.settings,
                                    server.runner.as_ref(),
                                    request,
                                    &CancellationToken::new(),
                                    true,
                                )
                                .await;
                                let result = match method {
                                    "initialize" => {
                                        json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"rustcode","version":env!("CARGO_PKG_VERSION")},"instructions":answer.content})
                                    }
                                    "tools/list" => {
                                        json!({"tools":serde_json::from_str::<Value>(&answer.content).unwrap_or(json!([]))})
                                    }
                                    _ => {
                                        json!({"isError":!answer.success,"content":[{"type":"text","text":answer.content}]})
                                    }
                                };
                                json!({"jsonrpc":"2.0","id":id,"result":result})
                            }
                        }
                    }
                    _ => {
                        status = 400;
                        json!({"error":"invalid_request"})
                    }
                }
            }
        }
        _ => {
            status = 404;
            json!({"error":"not_found"})
        }
    };
    respond(&mut stream, status, result, &extra).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn port_falls_forward_and_persists_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PORT_FILE);
        let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = occupied.local_addr().unwrap().port();
        std::fs::write(&path, port.to_string()).unwrap();
        let listener = bind_configured(&path).await.unwrap();
        let chosen = listener.local_addr().unwrap();
        assert!(chosen.ip().is_loopback());
        assert!(chosen.port() > port);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            chosen.port().to_string()
        );
        drop(listener);
        let restarted = bind_configured(&path).await.unwrap();
        assert_eq!(restarted.local_addr().unwrap(), chosen);
    }

    #[tokio::test]
    async fn missing_port_uses_default_or_next_free_port() {
        let dir = tempfile::tempdir().unwrap();
        let listener = bind_configured(&dir.path().join(PORT_FILE)).await.unwrap();
        assert!(listener.local_addr().unwrap().port() >= DEFAULT_PORT);
    }

    #[test]
    fn s256_only_verifies_the_original_verifier() {
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(pkce(v, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(!pkce("wrong", v));
        assert!(!pkce(v, v));
    }
    #[test]
    fn http_tokens_expire_revoke_and_do_not_authenticate_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grants.json");
        let (grant, token) = outside::create_http(&path, "app", outside::Scope::ReadOnly).unwrap();
        assert!(outside::authenticate_at(&path, &token).is_none());
        assert!(outside::authenticate_transport_at(&path, &token, true).is_some());
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data["clients"][0]["expires_at"] = json!(0);
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(outside::authenticate_transport_at(&path, &token, true).is_none());
        outside::revoke_at(&path, &grant.id).unwrap();
        let (grant, token) =
            outside::create_http(&path, "revoked", outside::Scope::ReadOnly).unwrap();
        assert!(outside::authenticate_transport_at(&path, &token, true).is_some());
        outside::revoke_at(&path, &grant.id).unwrap();
        assert!(outside::authenticate_transport_at(&path, &token, true).is_none());
    }
}

#[cfg(test)]
#[path = "outside_http_interop.rs"]
mod interop;
