//! Stateless Streamable HTTP MCP with public-client OAuth/PKCE.
//!
//! It always answers on 127.0.0.1. With `[outside] public_url` set it also
//! answers requests for that host, which a tunnel or proxy (cloudflared)
//! forwards to the same loopback port: hosted agents register themselves,
//! sign in on a page this server shows, and the owner approves with a
//! one-time code from `rustcode outside code`. Secrets are never logged;
//! restarting discards unexchanged authorization codes.
use super::{hub::CallRequest, outside};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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
    name: String,
    redirect: String,
    challenge: String,
    scope: outside::Scope,
    ask: Option<bool>,
    expires: Instant,
}
struct Server {
    /// `http://127.0.0.1:<port>`, what this computer's own agents use.
    base: String,
    /// `https://<host>` from `[outside] public_url`, what a tunnel serves.
    public: Option<String>,
    /// Signs registered client ids, so registration stores nothing.
    key: [u8; 32],
    /// Where `rustcode outside code` leaves its one-time sign-in codes.
    sign_in_codes: PathBuf,
    settings: outside::Settings,
    runner: Arc<dyn outside::Runner>,
    codes: Mutex<HashMap<String, Code>>,
    /// Wrong sign-in codes typed recently; too many lock the page for a while.
    failures: Mutex<Vec<Instant>>,
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
const KEY_FILE: &str = "outside-oauth-key";
/// Registered client ids start with this; any other id is a plain app name.
const CLIENT_PREFIX: &str = "rcc.";
const MAX_REDIRECTS: usize = 5;
const MAX_REDIRECT_CHARS: usize = 512;
/// Wrong codes allowed in [`FAILURE_WINDOW`] before the page stops checking.
const MAX_FAILURES: usize = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(600);

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

/// The origin `[outside] public_url` names, or why it cannot be used. Only
/// `https://host[:port]`: hosted agents send credentials over HTTPS only, and
/// the endpoint's paths are fixed.
pub fn public_base(text: &str) -> Result<String, String> {
    let wrong = || {
        format!(
            "[outside] public_url must be an https:// address with no path, such as \
             https://rustcode.example.com (got {text:?})"
        )
    };
    let url = url::Url::parse(text.trim()).map_err(|_| wrong())?;
    if url.scheme() != "https"
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(wrong());
    }
    Ok(url.origin().ascii_serialization())
}

/// The configured public origin, if one is set and valid.
pub fn configured_public_base() -> Option<Result<String, String>> {
    let (_, _, config) = crate::config::load_config();
    config.outside.public_url.as_deref().map(public_base)
}

/// What `rustcode outside code` prints.
pub struct SignInCode {
    /// `XXXX-XXXX`, shown once.
    pub code: String,
    pub scope: &'static str,
    pub minutes: i64,
    /// The sign-in page's address, or why `[outside] public_url` is unusable.
    pub public_url: Option<Result<String, String>>,
}

/// A one-time code for the public sign-in page, at most at `access`
/// (`read_only` when not given). `ask` overrides whether the app's changes
/// wait for approval in the app.
pub fn sign_in_code(access: Option<&str>, ask: Option<bool>) -> Result<SignInCode, String> {
    let scope = match access {
        None => outside::Scope::ReadOnly,
        Some(text) => outside::Scope::parse(text).ok_or_else(|| {
            format!("unknown access {text:?}: read_only, delegate, manage or admin")
        })?,
    };
    let path = outside::sign_in_code_path().ok_or("could not determine the config directory")?;
    Ok(SignInCode {
        code: outside::create_sign_in_code_at(&path, scope, ask)?,
        scope: scope.as_str(),
        minutes: outside::SIGN_IN_CODE_SECS / 60,
        public_url: configured_public_base(),
    })
}

fn load_key(path: &Path) -> Result<[u8; 32], String> {
    if let Some(key) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| hex::decode(text.trim()).ok())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
    {
        return Ok(key);
    }
    let key: [u8; 32] = rand::random();
    crate::service::write_private(path, hex::encode(key).as_bytes())
        .map_err(|e| format!("could not save the outside-agent signing key: {e}"))?;
    Ok(key)
}

/// Start once per engine, reusing the saved loopback port when available.
pub async fn ensure() -> Result<String, String> {
    URL.get_or_try_init(|| async {
        let dir =
            crate::config::get_config_dir().ok_or("could not determine the config directory")?;
        let listener = bind_configured(&dir.join(PORT_FILE)).await?;
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().map_err(|e| e.to_string())?.port()
        );
        let public = match configured_public_base() {
            Some(Ok(public)) => Some(public),
            Some(Err(error)) => {
                // Local agents keep working; `rustcode outside code` says what is wrong.
                crate::dbg_log!("outside-agent public URL ignored: {error}");
                None
            }
            None => None,
        };
        let server = Arc::new(Server {
            base: base.clone(),
            public,
            key: load_key(&dir.join(KEY_FILE))?,
            sign_in_codes: outside::sign_in_code_path().ok_or("configuration unavailable")?,
            settings: outside::Settings::machine().ok_or("configuration unavailable")?,
            runner: Arc::new(outside::EngineRunner),
            codes: Mutex::new(HashMap::new()),
            failures: Mutex::new(Vec::new()),
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
    write_response(
        stream,
        status,
        "application/json",
        &value.to_string(),
        extra,
    )
    .await;
}
async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
    extra: &str,
) {
    let response = format!(
        "HTTP/1.1 {status} Response\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}
fn fields(text: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(text).into_owned().collect()
}

// ------------------------------------------------------------- clients

/// Who is signing in: the name it gave and, for a registered client, the
/// callbacks it registered.
struct Client {
    name: String,
    redirects: Option<Vec<String>>,
}

fn mac(key: &[u8; 32], body: &str) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(b"mcp-client-id.");
    mac.update(body.as_bytes());
    mac
}

fn loopback(url: &url::Url) -> bool {
    url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

fn plain(url: &url::Url) -> bool {
    url.username().is_empty() && url.password().is_none() && url.fragment().is_none()
}

/// Loopback callbacks match on any port (RFC 8252 §7.3); others exactly.
fn redirect_matches(registered: &str, presented: &url::Url) -> bool {
    let Ok(mut registered) = url::Url::parse(registered) else {
        return false;
    };
    if loopback(&registered) && loopback(presented) {
        let mut presented = presented.clone();
        let _ = registered.set_port(None);
        let _ = presented.set_port(None);
        registered == presented
    } else {
        registered.as_str() == presented.as_str()
    }
}

/// The highest access a space-separated `scope` names; words that are not a
/// RustCode level are ignored. `None` when it names none.
fn requested_scope(text: &str) -> Option<outside::Scope> {
    text.split_whitespace()
        .filter_map(outside::Scope::parse)
        .max()
}

impl Server {
    /// A client id that carries its own registration, signed so nobody can
    /// add a callback to it.
    fn register_client(&self, name: &str, redirects: &[String]) -> String {
        let body = URL_SAFE_NO_PAD.encode(json!({"v":1,"n":name,"r":redirects}).to_string());
        let signature = URL_SAFE_NO_PAD.encode(mac(&self.key, &body).finalize().into_bytes());
        format!("{CLIENT_PREFIX}{body}.{signature}")
    }

    fn client(&self, id: &str) -> Option<Client> {
        let Some(rest) = id.strip_prefix(CLIENT_PREFIX) else {
            // An app that names itself and signs in from this computer.
            return (!id.is_empty() && id.len() <= 200).then(|| Client {
                name: id.to_owned(),
                redirects: None,
            });
        };
        let (body, signature) = rest.split_once('.')?;
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        mac(&self.key, body).verify_slice(&signature).ok()?;
        let value: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
        Some(Client {
            name: value["n"].as_str()?.to_owned(),
            redirects: Some(
                value["r"]
                    .as_array()?
                    .iter()
                    .filter_map(|r| r.as_str().map(str::to_owned))
                    .collect(),
            ),
        })
    }

    /// The callback to send the answer to, when this client may use it here.
    /// An `https` callback (a hosted agent) needs a registered client and a
    /// request through the public address; this computer's own apps use loopback.
    fn redirect(&self, client: &Client, presented: &str, public: bool) -> Option<url::Url> {
        let url = url::Url::parse(presented).ok().filter(plain)?;
        let allowed = match &client.redirects {
            None => loopback(&url),
            Some(list) => {
                list.iter().any(|r| redirect_matches(r, &url))
                    && (loopback(&url) || (public && url.scheme() == "https"))
            }
        };
        allowed.then_some(url)
    }

    /// `/register` (RFC 7591): nothing is stored, the id is the registration.
    fn registration(&self, body: &[u8]) -> (u16, Value) {
        let invalid = |description: &str| {
            (
                400,
                json!({"error":"invalid_client_metadata","error_description":description}),
            )
        };
        let Ok(metadata) = serde_json::from_slice::<Value>(body) else {
            return invalid("send the client metadata as JSON");
        };
        let Some(redirects) = metadata["redirect_uris"].as_array() else {
            return invalid("register at least one redirect URI");
        };
        let redirects: Vec<String> = redirects
            .iter()
            .filter_map(|r| r.as_str().map(str::to_owned))
            .collect();
        if redirects.is_empty() || redirects.len() > MAX_REDIRECTS {
            return (
                400,
                json!({"error":"invalid_redirect_uri","error_description":format!("register between 1 and {MAX_REDIRECTS} redirect URIs")}),
            );
        }
        let parsed: Vec<url::Url> = redirects
            .iter()
            .filter(|r| r.len() <= MAX_REDIRECT_CHARS)
            .filter_map(|r| url::Url::parse(r).ok())
            .filter(|u| plain(u) && (loopback(u) || u.scheme() == "https"))
            .collect();
        if parsed.len() != redirects.len() {
            return (
                400,
                json!({"error":"invalid_redirect_uri","error_description":"redirect URIs must be https:// or http://127.0.0.1 / localhost"}),
            );
        }
        let given: String = metadata["client_name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        let name = match given.trim() {
            "" => parsed[0].host_str().unwrap_or("MCP client").to_owned(),
            name => name.to_owned(),
        };
        let client_id = self.register_client(&name, &redirects);
        (
            201,
            json!({
                "client_id": client_id,
                "client_id_issued_at": chrono::Utc::now().timestamp(),
                "client_name": name,
                "redirect_uris": redirects,
                "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
            }),
        )
    }

    fn locked(&self) -> bool {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.retain(|at| at.elapsed() < FAILURE_WINDOW);
        failures.len() >= MAX_FAILURES
    }

    fn failed(&self) {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Instant::now());
    }

    fn issue(
        &self,
        authorization: &Authorization,
        scope: outside::Scope,
        ask: Option<bool>,
    ) -> String {
        let code = secret();
        let mut codes = self.codes.lock().unwrap();
        codes.retain(|_, c| c.expires > Instant::now());
        codes.insert(
            code.clone(),
            Code {
                client: authorization.client_id.clone(),
                name: authorization.client.name.clone(),
                redirect: authorization.redirect_raw.clone(),
                challenge: authorization.challenge.clone(),
                scope,
                ask,
                expires: Instant::now() + Duration::from_secs(60),
            },
        );
        code
    }
}

/// A checked `/authorize` request.
struct Authorization {
    client_id: String,
    client: Client,
    redirect: url::Url,
    redirect_raw: String,
    scope: Option<outside::Scope>,
    state: String,
    challenge: String,
}

impl Authorization {
    fn parse(
        server: &Server,
        q: &HashMap<String, String>,
        base: &str,
        public: bool,
    ) -> Option<Self> {
        let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
        let resource = format!("{base}/mcp");
        if get("response_type") != "code"
            || get("code_challenge_method") != "S256"
            || get("code_challenge").len() != 43
            || URL_SAFE_NO_PAD.decode(get("code_challenge")).is_err()
            || !(get("resource").is_empty() || get("resource") == resource)
        {
            return None;
        }
        let client = server.client(get("client_id"))?;
        let redirect = server.redirect(&client, get("redirect_uri"), public)?;
        Some(Self {
            client_id: get("client_id").to_owned(),
            client,
            redirect,
            redirect_raw: get("redirect_uri").to_owned(),
            scope: requested_scope(get("scope")),
            state: get("state").to_owned(),
            challenge: get("code_challenge").to_owned(),
        })
    }

    fn answer(&self, base: &str, pairs: &[(&str, &str)]) -> String {
        let mut destination = self.redirect.clone();
        {
            let mut query = destination.query_pairs_mut();
            for (k, v) in pairs {
                query.append_pair(k, v);
            }
            query
                .append_pair("state", &self.state)
                .append_pair("iss", base);
        }
        destination.to_string()
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The page a sign-in through the public address opens on. It names the app
/// and where access goes, and takes a code from `rustcode outside code`.
fn sign_in_page(a: &Authorization, q: &HashMap<String, String>, error: Option<&str>) -> String {
    let destination = a.redirect.host_str().unwrap_or("this computer");
    let goes_to = if loopback(&a.redirect) {
        "an app on the computer that opened this page".to_owned()
    } else {
        format!(
            "<b>{}</b>. Whoever runs {} gets this access.",
            escape(destination),
            escape(destination)
        )
    };
    let asks = a
        .scope
        .map(|s| s.label().to_owned())
        .unwrap_or_else(|| "the access your code allows".to_owned());
    let hidden: String = [
        "response_type",
        "client_id",
        "redirect_uri",
        "scope",
        "resource",
        "state",
        "code_challenge",
        "code_challenge_method",
    ]
    .iter()
    .filter_map(|k| q.get(*k).map(|v| (k, v)))
    .map(|(k, v)| format!(r#"<input type="hidden" name="{k}" value="{}">"#, escape(v)))
    .collect();
    let error = error
        .map(|e| format!(r#"<p class="error">{}</p>"#, escape(e)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in to RustCode</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:32rem;margin:3rem auto;padding:0 1rem;color:#222}}input[type=text]{{font:inherit;font-size:1.4rem;letter-spacing:.15em;width:100%;padding:.4rem;box-sizing:border-box}}button{{font:inherit;padding:.4rem 1rem;margin:.8rem .4rem 0 0}}.error{{color:#b00020}}code{{background:#eee;padding:0 .2rem}}</style></head><body><h1>Sign in to RustCode</h1><p><b>{name}</b> (the name it gave itself) wants to use RustCode on this machine.</p><p>Access goes to {goes_to}</p><p>It asks for: <b>{asks}</b>.</p><p>Approve only a sign-in you just started. On this machine, run <code>rustcode outside code --access delegate</code> (or another level) and enter the code it prints. The code sets the most this app may do.</p>{error}<form method="post" action="/authorize">{hidden}<label>Code<br><input type="text" name="code" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" autofocus placeholder="XXXX-XXXX"></label><br><button name="decision" value="approve">Approve</button><button name="decision" value="deny">Decline</button></form></body></html>"#,
        name = escape(&a.client.name),
    )
}

const PAGE_HEADERS: &str = "Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'\r\nX-Frame-Options: DENY\r\nReferrer-Policy: same-origin\r\n";

fn token_answer(tokens: outside::HttpTokens) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "access_token": tokens.access,
        "token_type": "Bearer",
        "expires_in": tokens.grant.expires_at.map_or(0, |at| (at - now).max(0)),
        "refresh_token": tokens.refresh,
        "scope": tokens.grant.scope.as_str(),
    })
}

async fn serve(mut stream: TcpStream, server: Arc<Server>) {
    let Some(req) = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    // Host validation prevents DNS rebinding: only this computer's loopback
    // address and the configured public host are answered. Browser requests
    // from other origins are refused.
    let host = req.headers.get("host").map(String::as_str);
    let (base, public) = if host == Some(server.base.trim_start_matches("http://")) {
        (server.base.as_str(), false)
    } else if let Some(public) = server
        .public
        .as_deref()
        .filter(|p| host == Some(p.trim_start_matches("https://")))
    {
        (public, true)
    } else {
        respond(&mut stream, 403, json!({"error":"forbidden"}), "").await;
        return;
    };
    if req.headers.get("origin").is_some_and(|o| o != base) {
        respond(&mut stream, 403, json!({"error":"forbidden"}), "").await;
        return;
    }
    let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
    let resource = format!("{base}/mcp");
    let mut status = 200;
    let mut extra = String::new();
    let result = match (req.method.as_str(), path) {
        ("GET", "/.well-known/oauth-protected-resource")
        | ("GET", "/.well-known/oauth-protected-resource/mcp") => {
            json!({"resource":resource,"authorization_servers":[base],"scopes_supported":["read_only","delegate","manage","admin"],"bearer_methods_supported":["header"]})
        }
        ("GET", "/.well-known/oauth-authorization-server") => {
            json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"registration_endpoint":format!("{base}/register"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"token_endpoint_auth_methods_supported":["none"],"code_challenge_methods_supported":["S256"],"scopes_supported":["read_only","delegate","manage","admin"],"authorization_response_iss_parameter_supported":true})
        }
        ("POST", "/register") => {
            let (code, answer) = server.registration(&req.body);
            status = code;
            answer
        }
        ("GET", "/authorize") => {
            let q = fields(query.as_bytes());
            match Authorization::parse(&server, &q, base, public) {
                None => {
                    status = 400;
                    json!({"error":"invalid_request"})
                }
                Some(a) if public => {
                    let page = sign_in_page(&a, &q, None);
                    write_response(
                        &mut stream,
                        200,
                        "text/html; charset=utf-8",
                        &page,
                        PAGE_HEADERS,
                    )
                    .await;
                    return;
                }
                Some(a) => {
                    let scope = a.scope.unwrap_or(outside::Scope::ReadOnly);
                    let location =
                        if outside::approve_connection(&a.client.name, a.redirect.as_str(), scope)
                            .await
                        {
                            let code = server.issue(&a, scope, None);
                            a.answer(base, &[("code", &code)])
                        } else {
                            a.answer(base, &[("error", "access_denied")])
                        };
                    status = 302;
                    extra = format!("Location: {location}\r\n");
                    json!({})
                }
            }
        }
        // The sign-in page's form; only the public address shows that page.
        ("POST", "/authorize") if public => {
            let q = fields(&req.body);
            let Some(a) = Authorization::parse(&server, &q, base, public) else {
                respond(&mut stream, 400, json!({"error":"invalid_request"}), "").await;
                return;
            };
            let location = if q.get("decision").map(String::as_str) != Some("approve") {
                a.answer(base, &[("error", "access_denied")])
            } else {
                let error = if server.locked() {
                    Some(
                        "Too many wrong codes. Wait a few minutes, then make a new code and try again.",
                    )
                } else {
                    match outside::redeem_sign_in_code_at(
                        &server.sign_in_codes,
                        q.get("code").map(String::as_str).unwrap_or(""),
                    ) {
                        Some((ceiling, ask)) => {
                            let scope = a.scope.map_or(ceiling, |s| s.min(ceiling));
                            let code = server.issue(&a, scope, ask);
                            let location = a.answer(base, &[("code", &code)]);
                            write_response(
                                &mut stream,
                                303,
                                "text/plain",
                                "",
                                &format!("Location: {location}\r\n"),
                            )
                            .await;
                            return;
                        }
                        None => {
                            server.failed();
                            Some(
                                "That code is not valid. It works once, for 10 minutes; make a new one with rustcode outside code.",
                            )
                        }
                    }
                };
                let page = sign_in_page(&a, &q, error);
                write_response(
                    &mut stream,
                    200,
                    "text/html; charset=utf-8",
                    &page,
                    PAGE_HEADERS,
                )
                .await;
                return;
            };
            write_response(
                &mut stream,
                303,
                "text/plain",
                "",
                &format!("Location: {location}\r\n"),
            )
            .await;
            return;
        }
        ("POST", "/token") => {
            let q = fields(&req.body);
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let tokens = match get("grant_type") {
                "authorization_code" => {
                    let code = server.codes.lock().unwrap().remove(get("code"));
                    code.filter(|c| {
                        c.expires > Instant::now()
                            && c.client == get("client_id")
                            && c.redirect == get("redirect_uri")
                            && (get("resource").is_empty() || get("resource") == resource)
                            && pkce(get("code_verifier"), &c.challenge)
                    })
                    .map(|code| {
                        outside::create_http(
                            &server.settings.grants,
                            &code.name,
                            &code.client,
                            code.scope,
                            code.ask,
                        )
                    })
                }
                "refresh_token" => outside::refresh_http(
                    &server.settings.grants,
                    get("refresh_token"),
                    get("client_id"),
                )
                .map(Ok),
                _ => None,
            };
            match tokens {
                Some(Ok(tokens)) => token_answer(tokens),
                Some(Err(_)) => {
                    status = 503;
                    json!({"error":"temporarily_unavailable"})
                }
                None => {
                    status = 400;
                    json!({"error":"invalid_grant"})
                }
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
                    "WWW-Authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\"\r\n"
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
        let tokens =
            outside::create_http(&path, "app", "app", outside::Scope::ReadOnly, None).unwrap();
        let (grant, token) = (tokens.grant, tokens.access);
        assert!(outside::authenticate_at(&path, &token).is_none());
        assert!(outside::authenticate_transport_at(&path, &token, true).is_some());
        let mut data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        data["clients"][0]["expires_at"] = json!(0);
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(outside::authenticate_transport_at(&path, &token, true).is_none());
        outside::revoke_at(&path, &grant.id).unwrap();
        let tokens =
            outside::create_http(&path, "revoked", "revoked", outside::Scope::ReadOnly, None)
                .unwrap();
        let (grant, token) = (tokens.grant, tokens.access);
        assert!(outside::authenticate_transport_at(&path, &token, true).is_some());
        outside::revoke_at(&path, &grant.id).unwrap();
        assert!(outside::authenticate_transport_at(&path, &token, true).is_none());
    }
}

#[cfg(test)]
#[path = "outside_http_interop.rs"]
mod interop;
