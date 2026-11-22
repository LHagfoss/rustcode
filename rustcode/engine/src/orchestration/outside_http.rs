//! Stateless Streamable HTTP MCP with public-client OAuth/PKCE.
//!
//! It always answers on 127.0.0.1. With `[outside] public_url` set it also
//! answers requests for that host, which a tunnel or proxy (cloudflared)
//! forwards to the same loopback port: hosted agents register themselves,
//! sign in on a page this server shows, and the owner approves with a
//! one-time code from `rc outside code`. Secrets are never logged;
//! restarting discards unexchanged authorization codes.
use super::outside_signin::{self, Consent, Outcome, PAGE_HEADERS};
use super::{mcp_names, outside, tool_socket::CallRequest};
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
    /// Where `rc outside code` leaves its one-time sign-in codes.
    sign_in_codes: PathBuf,
    /// The owner's passkeys, and the links that add one.
    passkeys: PathBuf,
    passkey_enrollments: PathBuf,
    /// The browsers signed in to the web app (`outside_web.rs`).
    web_sessions: PathBuf,
    /// The production web build, when this machine has one.
    web: Option<PathBuf>,
    /// This computer's name, as the sign-in page shows it.
    machine: String,
    /// The configuration root whose remote gateway `/ws` is relayed to.
    config: PathBuf,
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

/// How long a restarting engine waits for the one before it to let go of the
/// saved port before it serves on another.
const PORT_HANDOVER: Duration = Duration::from_secs(15);

/// Bind the saved port, waiting up to `handover` for an engine that is still
/// shutting down to release it. A tunnel or reverse proxy (`[outside]
/// public_url`) points at that port, so it is never replaced: when it stays
/// busy this run serves on the next free one and the next start tries the
/// saved one again. Only a first start saves the port it found.
async fn bind_configured(
    path: &std::path::Path,
    handover: Duration,
) -> Result<TcpListener, String> {
    let saved = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|port| *port != 0);
    let preferred = saved.unwrap_or(DEFAULT_PORT);
    let deadline = Instant::now() + handover;
    let listener = loop {
        match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, preferred)).await {
            Ok(listener) => break listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                if saved.is_none() || Instant::now() >= deadline {
                    break bind_port(preferred).await.map_err(|e| e.to_string())?;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    match saved {
        None => crate::service::write_private(path, port.to_string().as_bytes())
            .map_err(|e| format!("could not save outside-agent HTTP port: {e}"))?,
        Some(saved) if saved != port => crate::dbg_log!(
            "outside-agent HTTP port {saved} is busy; serving on {port} until the next start, so a tunnel to {saved} cannot reach it"
        ),
        Some(_) => {}
    }
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

/// What `rc outside code` prints.
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
        let listener = bind_configured(&dir.join(PORT_FILE), PORT_HANDOVER).await?;
        let base = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().map_err(|e| e.to_string())?.port()
        );
        let public = match configured_public_base() {
            Some(Ok(public)) => Some(public),
            Some(Err(error)) => {
                // Local agents keep working; `rc outside code` says what is wrong.
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
            passkeys: super::outside_passkey::passkey_path().ok_or("configuration unavailable")?,
            passkey_enrollments: super::outside_passkey::enrollment_path()
                .ok_or("configuration unavailable")?,
            web_sessions: super::outside_web::session_path().ok_or("configuration unavailable")?,
            web: super::outside_web::web_dir(),
            config: dir.clone(),
            machine: crate::fleet::own_name_override()
                .or_else(crate::remote_gateway::gateway::os_host_name)
                .unwrap_or_else(|| "this computer".to_owned()),
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
                    tokio::spawn(handle(stream, server, Some(permit)));
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
    /// Every byte read, head included, for a `/ws` upgrade relayed as is.
    raw: Vec<u8>,
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
        raw: bytes,
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
    body: impl AsRef<[u8]>,
    extra: &str,
) {
    let body = body.as_ref();
    let cache = if extra.contains("Cache-Control:") {
        ""
    } else {
        "Cache-Control: no-store\r\n"
    };
    let head = format!(
        "HTTP/1.1 {status} Response\r\nContent-Type: {content_type}\r\n{cache}Connection: close\r\nContent-Length: {}\r\n{extra}\r\n",
        body.len()
    );
    let mut response = head.into_bytes();
    response.extend_from_slice(body);
    let _ = stream.write_all(&response).await;
}

async fn html(stream: &mut TcpStream, status: u16, page: &str) {
    write_response(
        stream,
        status,
        "text/html; charset=utf-8",
        page,
        PAGE_HEADERS,
    )
    .await;
}

/// A page served with headers of its own (a consent page with a script).
async fn html_with(stream: &mut TcpStream, status: u16, (page, headers): (String, String)) {
    write_response(stream, status, "text/html; charset=utf-8", page, &headers).await;
}

async fn see_other(stream: &mut TcpStream, location: &str) {
    write_response(
        stream,
        303,
        "text/plain",
        "",
        &format!("Location: {location}\r\nReferrer-Policy: no-referrer\r\n"),
    )
    .await;
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

/// The `/authorize` parameters the consent page carries in its form.
const AUTHORIZE_FIELDS: [&str; 8] = [
    "response_type",
    "client_id",
    "redirect_uri",
    "scope",
    "resource",
    "state",
    "code_challenge",
    "code_challenge_method",
];

impl Server {
    /// The consent page for `a`, with the level `selected` and an `error`,
    /// and the headers it is served with: a page that offers the owner's
    /// passkeys runs one script, allowed by its nonce.
    fn consent(
        &self,
        a: &Authorization,
        q: &HashMap<String, String>,
        error: Option<&str>,
        selected: Option<outside::Scope>,
    ) -> (String, String) {
        let prompt = self.public.as_deref().and_then(|public| {
            super::outside_passkey::PasskeyPrompt::for_origin(&self.passkeys, public)
        });
        let headers = prompt.as_ref().map_or_else(
            || PAGE_HEADERS.to_owned(),
            |prompt| super::outside_passkey::script_headers(&prompt.nonce),
        );
        let hidden: Vec<(&str, &str)> = AUTHORIZE_FIELDS
            .iter()
            .filter_map(|k| q.get(*k).map(|v| (*k, v.as_str())))
            .collect();
        let host = self
            .public
            .as_deref()
            .unwrap_or(&self.base)
            .split("://")
            .nth(1)
            .unwrap_or("");
        let page = outside_signin::consent_page(
            &Consent {
                client_name: &a.client.name,
                destination: (!loopback(&a.redirect)).then(|| a.redirect.host_str().unwrap_or("")),
                machine: &self.machine,
                host,
                requested: a.scope,
                hidden: &hidden,
                passkey: prompt.as_ref(),
            },
            error,
            selected,
        );
        (page, headers)
    }
}

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

/// Where the remote gateway's WebSocket is served on this listener.
pub const WS_PATH: &str = "/ws";
/// Where Loop webhooks answer: `POST /hooks/<loop-id>/<secret>`, or
/// `/hooks/<loop-id>` with `Authorization: Bearer <secret>` (`acp::loops`).
pub const HOOKS_PATH: &str = "/hooks/";

/// Serve one accepted connection. A `/ws` connection lives as long as the
/// device stays connected, so it holds neither the request slot nor the
/// request deadline once it is relayed; the gateway bounds its own.
async fn handle(
    stream: TcpStream,
    server: Arc<Server>,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) {
    let config = server.config.clone();
    let sessions = server.web_sessions.clone();
    let upgrade = {
        let _permit = permit;
        tokio::time::timeout(Duration::from_secs(600), serve(stream, server))
            .await
            .ok()
            .flatten()
    };
    match upgrade {
        Some(Upgrade::Gateway(stream, head)) => relay_to_gateway(stream, head, &config).await,
        Some(Upgrade::App { stream, session }) => {
            crate::remote_gateway::serve_browser(stream, &config, &sessions, &session).await
        }
        None => {}
    }
}

/// A socket that outlives its request: a device's `/ws`, relayed to the
/// gateway as is, or the web app's ACP socket, already answered with `101`.
enum Upgrade {
    Gateway(TcpStream, Vec<u8>),
    App { stream: TcpStream, session: String },
}

fn websocket_upgrade(req: &Request) -> bool {
    req.method == "GET"
        && req
            .headers
            .get("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// The refusal for a credential presented where it does not belong: an
/// outside agent's token on the app's routes, or the app's cookie on `/mcp`.
fn wrong_audience(what: &str) -> Value {
    json!({"error":"wrong_audience","error_description":what})
}

/// Hand a `/ws` upgrade to the remote gateway's loopback ingress and copy
/// bytes both ways until either side closes. The gateway answers the upgrade
/// itself, so the bytes already read go first. Without a running gateway the
/// device is told so instead of left waiting.
async fn relay_to_gateway(mut stream: TcpStream, head: Vec<u8>, config: &Path) {
    let ingress = crate::remote_gateway::lifecycle::RemoteLifecycle::new(config)
        .running_registration()
        .and_then(|registration| registration.ingress_address);
    let gateway = match ingress {
        Some(address) => TcpStream::connect(address).await.ok(),
        None => None,
    };
    let Some(mut gateway) = gateway else {
        respond(
            &mut stream,
            503,
            json!({"error":"remote_access_off","error_description":"remote access is not running on this machine"}),
            "",
        )
        .await;
        return;
    };
    let _ = stream.set_nodelay(true);
    let _ = gateway.set_nodelay(true);
    if gateway.write_all(&head).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut gateway).await;
}

/// Answer one request. A WebSocket upgrade at `/ws` is not answered here:
/// the socket and the bytes read from it come back for [`relay_to_gateway`].
async fn serve(mut stream: TcpStream, server: Arc<Server>) -> Option<Upgrade> {
    let Some(req) = tokio::time::timeout(Duration::from_secs(10), read(&mut stream))
        .await
        .ok()
        .flatten()
    else {
        return None;
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
        return None;
    };
    // The remote protocol for paired devices and machines, so one tunnel
    // serves both. Its `Origin` rule is the gateway's (only the native app's
    // or none), applied to the relayed upgrade itself.
    if req.target == WS_PATH || req.target.starts_with(&format!("{WS_PATH}?")) {
        if !websocket_upgrade(&req) {
            respond(
                &mut stream,
                426,
                json!({"error":"upgrade_required"}),
                "Upgrade: websocket\r\n",
            )
            .await;
            return None;
        }
        return Some(Upgrade::Gateway(stream, req.raw));
    }
    if req.headers.get("origin").is_some_and(|o| o != base) {
        respond(&mut stream, 403, json!({"error":"forbidden"}), "").await;
        return None;
    }
    let (path, query) = req.target.split_once('?').unwrap_or((&req.target, ""));
    if public
        && (path == SOCKET_PATH || path == TICKET_PATH || path == LOGIN_PATH || path == LOGOUT_PATH)
    {
        return serve_app(stream, &server, &req, base, path, query).await;
    }
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
                    html_with(&mut stream, 200, server.consent(&a, &q, None, None)).await;
                    return None;
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
        ("GET", "/logo.png") => {
            write_response(
                &mut stream,
                200,
                "image/png",
                outside_signin::LOGO,
                "Cache-Control: public, max-age=86400\r\n",
            )
            .await;
            return None;
        }
        // A link from `rc outside owner add-passkey`: the page that
        // makes the owner's passkey for this public address.
        ("GET", "/owner/passkey") if public => {
            let q = fields(query.as_bytes());
            let ticket = q.get("ticket").map(String::as_str).unwrap_or("");
            let Some(offer) =
                super::outside_passkey::enrollment_at(&server.passkey_enrollments, ticket)
            else {
                html(
                    &mut stream,
                    410,
                    &outside_signin::message_page(
                        "This link no longer works",
                        "A passkey link works once, for 10 minutes. Make a new one with `rc outside owner add-passkey` on this machine.",
                    ),
                )
                .await;
                return None;
            };
            let rp = super::outside_passkey::rp_id(base);
            let existing: Vec<String> = super::outside_passkey::list_at(&server.passkeys)
                .into_iter()
                .filter(|passkey| passkey.rp_id == rp)
                .map(|passkey| passkey.id)
                .collect();
            let nonce = super::outside_passkey::nonce();
            let page = super::outside_passkey::enrollment_page(
                &server.machine,
                ticket,
                &offer,
                &rp,
                &existing,
                &nonce,
            );
            html_with(
                &mut stream,
                200,
                (page, super::outside_passkey::script_headers(&nonce)),
            )
            .await;
            return None;
        }
        ("POST", "/owner/passkey") if public => {
            let q = fields(&req.body);
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let page = match super::outside_passkey::register_at(
                &server.passkeys,
                &server.passkey_enrollments,
                base,
                &super::outside_passkey::Registration {
                    ticket: get("ticket"),
                    client_data: get("pk_client"),
                    attestation: get("pk_attestation"),
                },
            ) {
                Ok(passkey) => {
                    crate::dbg_log!("owner passkey {} added", passkey.name);
                    outside::changed();
                    outside_signin::message_page(
                        "Passkey added",
                        &format!(
                            "“{}” now approves hosted agents that sign in to RustCode on {}. The sign-in page offers it next to the code.",
                            passkey.name, server.machine
                        ),
                    )
                }
                Err(why) => outside_signin::message_page("No passkey was added", &why),
            };
            html(&mut stream, 200, &page).await;
            return None;
        }
        // The consent page's form; only the public address shows that page.
        ("POST", "/authorize") if public => {
            let q = fields(&req.body);
            let Some(a) = Authorization::parse(&server, &q, base, public) else {
                respond(&mut stream, 400, json!({"error":"invalid_request"}), "").await;
                return None;
            };
            let selected = q
                .get("access")
                .and_then(|text| outside::Scope::parse(text))
                .or(a.scope)
                .unwrap_or(outside::Scope::ReadOnly);
            let error = match q.get("decision").map(String::as_str) {
                Some("app") => {
                    let destination = if loopback(&a.redirect) {
                        "an app on the asking computer".to_owned()
                    } else {
                        a.redirect.host_str().unwrap_or("").to_owned()
                    };
                    let kept: HashMap<String, String> = AUTHORIZE_FIELDS
                        .iter()
                        .filter_map(|k| q.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                        .collect();
                    match outside_signin::start(&a.client.name, &destination, selected, kept) {
                        Ok((id, key, _)) => {
                            see_other(&mut stream, &format!("/authorize/wait?id={id}&key={key}"))
                                .await;
                            return None;
                        }
                        Err(error) => error.to_string(),
                    }
                }
                Some("approve" | "passkey") if server.locked() => {
                    "Too many failed approvals. Wait a few minutes, then try again.".to_owned()
                }
                // The owner's passkey, checked here against this public
                // address (`outside_passkey.rs`). It approves the level chosen
                // on the page, at most the one the app asked for.
                Some("passkey") => {
                    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
                    let assertion = super::outside_passkey::Assertion {
                        id: get("pk_id"),
                        client_data: get("pk_client"),
                        authenticator_data: get("pk_auth"),
                        signature: get("pk_sig"),
                    };
                    match super::outside_passkey::verify_assertion_at(
                        &server.passkeys,
                        base,
                        &assertion,
                    ) {
                        Ok(name) => {
                            crate::dbg_log!(
                                "outside-agent sign-in for {} approved with the passkey {name}",
                                a.client.name
                            );
                            let scope = selected.min(a.scope.unwrap_or(outside::Scope::Admin));
                            let code = server.issue(&a, scope, None);
                            see_other(&mut stream, &a.answer(base, &[("code", &code)])).await;
                            return None;
                        }
                        Err(why) => {
                            server.failed();
                            format!("The passkey did not approve this sign-in: {why}.")
                        }
                    }
                }
                Some("approve") => match outside::redeem_sign_in_code_at(
                    &server.sign_in_codes,
                    q.get("code").map(String::as_str).unwrap_or(""),
                ) {
                    Some((ceiling, ask)) => {
                        let code = server.issue(&a, selected.min(ceiling), ask);
                        see_other(&mut stream, &a.answer(base, &[("code", &code)])).await;
                        return None;
                    }
                    None => {
                        server.failed();
                        "That sign-in code is not valid. A code works once, for 10 minutes."
                            .to_owned()
                    }
                },
                _ => {
                    see_other(&mut stream, &a.answer(base, &[("error", "access_denied")])).await;
                    return None;
                }
            };
            html_with(
                &mut stream,
                200,
                server.consent(&a, &q, Some(&error), Some(selected)),
            )
            .await;
            return None;
        }
        // The page that waits for the owner's answer in the app; it reloads
        // itself, and moves on to the agent once answered.
        ("GET", "/authorize/wait") if public => {
            let q = fields(query.as_bytes());
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let gone = || {
                outside_signin::message_page(
                    "This sign-in is no longer waiting",
                    "It was answered, cancelled or took longer than five minutes. Start again from the app you were connecting.",
                )
            };
            let Some((outcome, words, kept, name)) = outside_signin::poll(get("id"), get("key"))
            else {
                html(&mut stream, 410, &gone()).await;
                return None;
            };
            let Some(a) = Authorization::parse(&server, &kept, base, public) else {
                outside_signin::cancel(get("id"), get("key"));
                html(&mut stream, 410, &gone()).await;
                return None;
            };
            if get("cancel") == "1" {
                outside_signin::cancel(get("id"), get("key"));
                see_other(&mut stream, &a.answer(base, &[("error", "access_denied")])).await;
                return None;
            }
            match outcome {
                Outcome::Waiting => {
                    let cancel = format!(
                        "/authorize/wait?id={}&key={}&cancel=1",
                        get("id"),
                        get("key")
                    );
                    let page =
                        outside_signin::waiting_page(&name, &server.machine, &words, &cancel);
                    html(&mut stream, 200, &page).await;
                }
                Outcome::Approved { scope, ask } => {
                    let code = server.issue(&a, scope, ask);
                    see_other(&mut stream, &a.answer(base, &[("code", &code)])).await;
                }
                Outcome::Declined => {
                    see_other(&mut stream, &a.answer(base, &[("error", "access_denied")])).await;
                }
            }
            return None;
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
        (_, "/mcp") if super::outside_web::cookie(&req.headers).is_some() => {
            status = 401;
            wrong_audience("a browser signed in to the RustCode app cannot use /mcp")
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
                                let mut args = v["params"]["arguments"].clone();
                                let answer = match mcp_names::call_args(tool, &mut args) {
                                    Ok(names) => {
                                        let tool = mcp_names::internal_tool(tool);
                                        let request = CallRequest {
                                            session_id: String::new(),
                                            token: token.into(),
                                            tool: tool.into(),
                                            args,
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
                                        names.answer(tool, answer)
                                    }
                                    Err(answer) => answer,
                                };
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
        (method, path) if path.starts_with(HOOKS_PATH) => {
            if method != "POST" {
                status = 405;
                json!({"error":"method_not_allowed"})
            } else {
                let rest = &path[HOOKS_PATH.len()..];
                let bearer = req
                    .headers
                    .get("authorization")
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .unwrap_or("");
                let (id, secret) = rest.split_once('/').unwrap_or((rest, bearer));
                let ping = req.headers.get("x-github-event").map(String::as_str) == Some("ping");
                let (code, answer) =
                    crate::acp::loops::trigger(id, secret.trim(), &req.body, ping).await;
                status = code;
                answer
            }
        }
        // The web app's own files, for a browser signed in with the
        // owner's passkey; anything else is sent to the login page.
        ("GET", path) if public && !path.starts_with("/.well-known/") => {
            serve_app_file(&mut stream, &server, &req, path).await;
            return None;
        }
        _ => {
            status = 404;
            json!({"error":"not_found"})
        }
    };
    respond(&mut stream, status, result, &extra).await;
    None
}

use super::outside_web::{LOGIN_PATH, LOGOUT_PATH, SOCKET_PATH, TICKET_PATH};

/// The signed-in browser's session id, from its cookie.
fn web_session(server: &Server, req: &Request) -> Option<String> {
    super::outside_web::check_at(
        &server.web_sessions,
        super::outside_web::cookie(&req.headers)?,
    )
}

async fn serve_app_file(stream: &mut TcpStream, server: &Server, req: &Request, path: &str) {
    let page = path
        .rsplit('/')
        .next()
        .is_none_or(|last| !last.contains('.'))
        || path.ends_with(".html");
    if web_session(server, req).is_none() {
        if page {
            see_other(stream, LOGIN_PATH).await;
        } else {
            respond(stream, 401, json!({"error":"login_required"}), "").await;
        }
        return;
    }
    let Some(dir) = server.web.as_deref() else {
        html(
            stream,
            404,
            &outside_signin::message_page(
                "The web app is not installed here",
                "You are signed in, but this machine has no web build of RustCode. Build it with `bun run build:hub` in apps/desktop and copy dist-hub to ~/.config/rustcode/web (or set RUSTCODE_WEB_DIR).",
            ),
        )
        .await;
        return;
    };
    match super::outside_web::asset(dir, path) {
        Some((bytes, kind, immutable)) => {
            write_response(
                stream,
                200,
                kind,
                bytes,
                &super::outside_web::app_headers(immutable),
            )
            .await
        }
        None => respond(stream, 404, json!({"error":"not_found"}), "").await,
    }
}

/// The web app's login, logout, socket ticket and socket, on the public
/// address only. A bearer token is never accepted here: those belong to
/// `/mcp`.
async fn serve_app(
    mut stream: TcpStream,
    server: &Server,
    req: &Request,
    base: &str,
    path: &str,
    query: &str,
) -> Option<Upgrade> {
    if req.headers.contains_key("authorization") {
        respond(
            &mut stream,
            401,
            wrong_audience("an outside agent's sign-in does not open the RustCode app"),
            "",
        )
        .await;
        return None;
    }
    // A form post or a socket comes from the app's own pages: the browser
    // names their origin, which must be this one.
    let same_origin = req.headers.get("origin").map(String::as_str) == Some(base);
    match (req.method.as_str(), path) {
        ("GET", LOGIN_PATH) => {
            if web_session(server, req).is_some() {
                see_other(&mut stream, "/").await;
                return None;
            }
            let page = super::outside_web::login_page(&server.machine, &server.passkeys, base);
            html_with(&mut stream, 200, page).await;
        }
        ("POST", LOGIN_PATH) if same_origin => {
            if server.locked() {
                html(
                    &mut stream,
                    429,
                    &outside_signin::message_page(
                        "Too many failed sign-ins",
                        "Wait a few minutes, then try again.",
                    ),
                )
                .await;
                return None;
            }
            let q = fields(&req.body);
            let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
            let assertion = super::outside_passkey::Assertion {
                id: get("pk_id"),
                client_data: get("pk_client"),
                authenticator_data: get("pk_auth"),
                signature: get("pk_sig"),
            };
            let signed_in =
                super::outside_passkey::verify_assertion_at(&server.passkeys, base, &assertion)
                    .and_then(|name| {
                        let browser = super::outside_web::browser_name(
                            req.headers
                                .get("user-agent")
                                .map(String::as_str)
                                .unwrap_or(""),
                        );
                        let (_, secret) = super::outside_web::create_at(
                            &server.web_sessions,
                            &browser,
                            assertion.id.trim(),
                        )?;
                        crate::dbg_log!("web app: {browser} signed in with passkey {name}");
                        Ok(secret)
                    });
            match signed_in {
                Ok(secret) => {
                    outside::changed();
                    write_response(
                        &mut stream,
                        303,
                        "text/plain",
                        "",
                        &format!(
                            "Location: /\r\nReferrer-Policy: no-referrer\r\n{}",
                            super::outside_web::set_cookie(&secret)
                        ),
                    )
                    .await;
                }
                Err(why) => {
                    server.failed();
                    html(
                        &mut stream,
                        401,
                        &outside_signin::message_page("You are not signed in", &why),
                    )
                    .await;
                }
            }
        }
        ("POST", LOGOUT_PATH) if same_origin => {
            if let Some(secret) = super::outside_web::cookie(&req.headers) {
                super::outside_web::sign_out_secret_at(&server.web_sessions, secret);
                outside::changed();
            }
            write_response(
                &mut stream,
                303,
                "text/plain",
                "",
                &format!(
                    "Location: {LOGIN_PATH}\r\n{}",
                    super::outside_web::clear_cookie()
                ),
            )
            .await;
        }
        ("POST", TICKET_PATH) if same_origin => match web_session(server, req) {
            Some(session) => {
                let ticket = super::outside_web::new_ticket(&session);
                respond(
                    &mut stream,
                    200,
                    json!({"ticket": ticket, "session": session}),
                    "",
                )
                .await;
            }
            None => respond(&mut stream, 401, json!({"error":"login_required"}), "").await,
        },
        // The ticket, not the cookie, opens the socket: it is the page's
        // own `fetch` that made it, moments ago.
        ("GET", SOCKET_PATH) if same_origin && websocket_upgrade(req) => {
            let ticket = fields(query.as_bytes())
                .remove("ticket")
                .unwrap_or_default();
            let key = req
                .headers
                .get("sec-websocket-key")
                .cloned()
                .unwrap_or_default();
            let session = super::outside_web::spend_ticket(&ticket)
                .filter(|session| super::outside_web::active_at(&server.web_sessions, session));
            let (Some(session), false) = (session, key.is_empty()) else {
                respond(&mut stream, 401, json!({"error":"login_required"}), "").await;
                return None;
            };
            let head = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                super::outside_web::accept_key(&key)
            );
            if stream.write_all(head.as_bytes()).await.is_err() {
                return None;
            }
            return Some(Upgrade::App { stream, session });
        }
        _ => respond(&mut stream, 403, json!({"error":"forbidden"}), "").await,
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_busy_saved_port_falls_forward_for_one_run_and_stays_saved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PORT_FILE);
        let occupied = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = occupied.local_addr().unwrap().port();
        std::fs::write(&path, port.to_string()).unwrap();
        let listener = bind_configured(&path, Duration::ZERO).await.unwrap();
        let chosen = listener.local_addr().unwrap();
        assert!(chosen.ip().is_loopback());
        assert!(chosen.port() > port);
        // A tunnel points at the saved port: it is not replaced.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), port.to_string());
        drop(listener);
        drop(occupied);
        let restarted = bind_configured(&path, Duration::ZERO).await.unwrap();
        assert_eq!(restarted.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn a_restart_waits_for_the_old_engine_to_release_the_saved_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PORT_FILE);
        let old = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = old.local_addr().unwrap().port();
        std::fs::write(&path, port.to_string()).unwrap();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            drop(old);
        });
        let listener = bind_configured(&path, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(listener.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn missing_port_uses_default_or_next_free_port() {
        let dir = tempfile::tempdir().unwrap();
        let listener = bind_configured(&dir.path().join(PORT_FILE), Duration::ZERO)
            .await
            .unwrap();
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
