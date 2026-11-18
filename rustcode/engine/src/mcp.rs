use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{Mutex, Notify, mpsc};

pub struct McpClient {
    pub name: String,
    transport: Transport,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    next_id: Arc<Mutex<i64>>,
    tools: Arc<StdMutex<Vec<Value>>>,
    child: Arc<Mutex<Option<Child>>>,
    stderr_diagnostics: Arc<StdMutex<Vec<String>>>,
    stderr_finished: Arc<Notify>,
    /// Progress listeners for in-flight stdio requests, keyed by the request
    /// id sent as `progressToken`. A `notifications/progress` for the token
    /// restarts that request's timeout.
    progress: ProgressListeners,
    /// How long a request may go without a response or progress, in seconds.
    request_timeout_secs: AtomicU64,
}

type ProgressListeners = Arc<Mutex<HashMap<i64, mpsc::UnboundedSender<()>>>>;

/// How long an MCP request may wait without a response or progress when the
/// server's config sets no `timeout_secs`.
pub(crate) const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// A request that keeps reporting progress may run at most this many timeouts
/// in total, so a server that only ever reports progress cannot hold a turn
/// forever. Counted in time, not notifications: a server that reports often
/// must not use the allowance up sooner than one that reports rarely.
const MAX_TOTAL_TIMEOUTS: u32 = 61;

/// Bound one server response before parsing or retaining its contents.
const MAX_MCP_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Sends `notifications/cancelled` for a stdio request when dropped while
/// still armed.
struct CancelOnDrop {
    tx: mpsc::Sender<Value>,
    id: i64,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.tx.try_send(json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {"requestId": self.id, "reason": "The client stopped waiting"},
            }));
        }
    }
}

enum Transport {
    Stdio { tx: mpsc::Sender<Value> },
    // Boxed: `reqwest::Client` alone is ~280 bytes and would bloat every
    // `McpClient` (and every match on it) for the stdio-only case.
    Remote { state: Mutex<Box<RemoteState>> },
}

struct RemoteState {
    url: String,
    headers: HashMap<String, String>,
    http: reqwest::Client,
    session_id: Option<String>,
    legacy_endpoint: Option<String>,
    auth: Option<OAuthToken>,
    client_id: Option<String>,
}

/// Cloned-out remote connection state so a request can be sent without
/// holding the state lock across network I/O.
struct RemoteSnapshot {
    url: String,
    target: String,
    headers: HashMap<String, String>,
    session_id: Option<String>,
    bearer: Option<String>,
    http: reqwest::Client,
}

/// Startup warnings surfaced to the model up front so a failed server does not
/// burn agent rounds silently (#1633). Populated by
/// `start_enabled_servers_with_timeout`, read by tool-notice composition.
static MCP_STARTUP_WARNINGS: OnceLock<StdMutex<Vec<String>>> = OnceLock::new();
/// Startup failure per server name, so the MCP catalogue can say why a
/// configured server has no tools.
static MCP_STARTUP_FAILURES: OnceLock<StdMutex<HashMap<String, String>>> = OnceLock::new();
/// Count of filtered non-JSON stdout lines per server (noisy servers pollute
/// stdout without losing the diagnostic via dbg_log).
static MCP_NON_JSON_COUNTS: OnceLock<StdMutex<HashMap<String, u64>>> = OnceLock::new();

/// Record startup warnings for in-turn surfacing (in addition to the exit
/// summary / stderr paths).
pub fn record_mcp_startup_warnings(warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    let slot = MCP_STARTUP_WARNINGS.get_or_init(|| StdMutex::new(Vec::new()));
    if let Ok(mut guard) = slot.lock() {
        for w in warnings {
            if !guard.contains(w) {
                guard.push(w.clone());
            }
        }
    }
}

/// Failed-server notices for prompt composition. Deduplicated, bounded.
pub fn mcp_startup_warnings() -> Vec<String> {
    MCP_STARTUP_WARNINGS
        .get()
        .and_then(|slot| slot.lock().ok())
        .map(|guard| guard.iter().take(8).cloned().collect())
        .unwrap_or_default()
}

pub(crate) fn record_mcp_startup_failure(name: &str, reason: &str) {
    let slot = MCP_STARTUP_FAILURES.get_or_init(|| StdMutex::new(HashMap::new()));
    if let Ok(mut guard) = slot.lock() {
        guard.insert(name.to_owned(), reason.to_owned());
    }
}

/// Forget a server's last failure once it has connected.
fn clear_mcp_startup_failure(name: &str) {
    if let Some(Ok(mut guard)) = MCP_STARTUP_FAILURES.get().map(StdMutex::lock) {
        guard.remove(name);
    }
}

/// Startup failures by server name, recorded when a configured server could
/// not be started at session launch.
pub fn mcp_startup_failures() -> HashMap<String, String> {
    MCP_STARTUP_FAILURES
        .get()
        .and_then(|slot| slot.lock().ok())
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

fn note_mcp_non_json_line(server: &str) {
    let slot = MCP_NON_JSON_COUNTS.get_or_init(|| StdMutex::new(HashMap::new()));
    if let Ok(mut guard) = slot.lock() {
        *guard.entry(server.to_owned()).or_insert(0) += 1;
    }
}

/// Per-server filtered stdout noise, for diagnostics.
pub fn mcp_non_json_counts() -> HashMap<String, u64> {
    MCP_NON_JSON_COUNTS
        .get()
        .and_then(|slot| slot.lock().ok())
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// OAuth tokens for one remote MCP server. Persisted under
/// `~/.config/rustcode/mcp-oauth/<server>.json` — never in `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct OAuthToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
}

impl OAuthToken {
    fn expired(&self) -> bool {
        match self.expires_at {
            None => false,
            Some(deadline) => now_unix_secs().saturating_add(30) >= deadline,
        }
    }

    fn usable(&self) -> bool {
        !self.access_token.is_empty() && !self.expired()
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) type McpRegistry = Arc<StdMutex<HashMap<String, Arc<McpClient>>>>;

tokio::task_local! {
    pub(crate) static DIRECT_MCP_REGISTRY: McpRegistry;
}

pub fn get_mcp_registry() -> McpRegistry {
    static REGISTRY: OnceLock<McpRegistry> = OnceLock::new();
    DIRECT_MCP_REGISTRY
        .try_with(Clone::clone)
        .unwrap_or_else(|_| {
            REGISTRY
                .get_or_init(|| Arc::new(StdMutex::new(HashMap::new())))
                .clone()
        })
}

/// Monotonic counter bumped whenever the MCP tool set changes (a server is
/// connected or disconnected). The prompt cache stores the generation it was
/// built against and rebuilds the static system prompt + tool schema only when
/// this value moves — the "dirty flag" for MCP-driven schema changes.
static MCP_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Current MCP tool-set generation. See [`MCP_GENERATION`].
pub fn mcp_generation() -> u64 {
    MCP_GENERATION.load(Ordering::Relaxed)
}

/// Signal that the MCP tool set changed, invalidating any cached prompt/schema.
pub fn bump_mcp_generation() {
    MCP_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Enabled servers queued for the ACP engine's background startup.
static DEFERRED_SERVERS: StdMutex<Option<Vec<crate::config::McpServerConfig>>> =
    StdMutex::new(None);
static DEFERRED_START: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CONNECTING_SERVERS: StdMutex<Vec<String>> = StdMutex::new(Vec::new());

pub(crate) fn connecting_servers() -> Vec<String> {
    CONNECTING_SERVERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Queue enabled servers before starting the background task.
pub fn defer_enabled_servers(servers: &[crate::config::McpServerConfig]) {
    let enabled = servers
        .iter()
        .filter(|server| server.enabled)
        .cloned()
        .collect::<Vec<_>>();
    *CONNECTING_SERVERS.lock().unwrap_or_else(|e| e.into_inner()) =
        enabled.iter().map(|s| s.name.clone()).collect();
    *DEFERRED_SERVERS
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = (!enabled.is_empty()).then_some(enabled);
}

/// Start the deferred servers, once, concurrently, and log how long it
/// took. Later and concurrent callers return when the first start is done;
/// with nothing deferred this is a no-op.
pub async fn start_deferred_servers() {
    let _starting = DEFERRED_START.lock().await;
    let Some(servers) = DEFERRED_SERVERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let started = std::time::Instant::now();
    let warnings = futures_util::future::join_all(servers.iter().map(|server| async move {
        let warnings = start_enabled_servers(std::slice::from_ref(server), |name| async move {
            start_server_by_name(&name).await
        })
        .await;
        CONNECTING_SERVERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|n| n != &server.name);
        warnings
    }))
    .await
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    crate::logger::operational_event(
        "mcp.background_start",
        serde_json::json!({
            "servers": servers.len(),
            "failed": warnings.len(),
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    );
}

#[cfg(test)]
pub(crate) fn deferred_server_names() -> Vec<String> {
    DEFERRED_SERVERS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .flatten()
        .map(|server| server.name.clone())
        .collect()
}

/// Start each enabled MCP server, keeping one server's failure from blocking
/// the remaining configured servers. Returns any warning messages collected.
pub async fn start_enabled_servers<F, Fut>(
    servers: &[crate::config::McpServerConfig],
    launcher: F,
) -> Vec<String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    start_enabled_servers_with_timeout(servers, Duration::from_secs(10), launcher).await
}

async fn start_enabled_servers_with_timeout<F, Fut>(
    servers: &[crate::config::McpServerConfig],
    startup_timeout: Duration,
    mut launcher: F,
) -> Vec<String>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut warnings = Vec::new();
    for server in servers.iter().filter(|server| server.enabled) {
        let name = server.name.clone();
        match tokio::time::timeout(startup_timeout, launcher(name.clone())).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                record_mcp_startup_failure(&name, &error);
                let msg = format!("[mcp] failed to start server {name}: {error}");
                crate::dbg_log!("{msg}");
                warnings.push(msg);
            }
            Err(_) => {
                record_mcp_startup_failure(
                    &name,
                    &format!("timed out after {:.1}s", startup_timeout.as_secs_f64()),
                );
                let msg = format!(
                    "[mcp] timed out starting server {name} after {:.1}s; continuing",
                    startup_timeout.as_secs_f64()
                );
                crate::dbg_log!("{msg}");
                warnings.push(msg);
            }
        }
    }
    record_mcp_startup_warnings(&warnings);
    warnings
}

/// Translate a transport failure into an actionable message that names the
/// server, so the TUI warning path can show it instead of silently omitting
/// the server's tools.
fn remote_connect_error(name: &str, url: &str, error: &reqwest::Error) -> String {
    if error.is_connect() {
        format!(
            "Failed to connect to MCP server '{name}' at {url}: {error} \
             (is the server running and reachable?)"
        )
    } else if error.is_timeout() {
        format!("MCP server '{name}' at {url} timed out: {error}")
    } else {
        format!("MCP request to server '{name}' at {url} failed: {error}")
    }
}

fn auth_required_error(name: &str, url: &str, detail: &str) -> String {
    format!(
        "MCP server '{name}' at {url} requires authentication: {detail}. \
         Complete the OAuth browser login and retry \
         (tokens persist under ~/.config/rustcode/mcp-oauth/)"
    )
}

fn truncate_snippet(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out.replace(char::is_control, " ")
}

/// `scheme://authority` of an HTTP(S) URL, without any path or query.
fn http_origin(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{authority}")
        }
        None => url.to_string(),
    }
}

/// Resolve a legacy-SSE `endpoint` event value (absolute URL, absolute path,
/// or relative path) against the MCP base URL.
fn join_endpoint(base: &str, endpoint: &str) -> String {
    let endpoint = endpoint.trim();
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    if let Some(path) = endpoint.strip_prefix('/') {
        return format!("{}/{}", http_origin(base), path);
    }
    format!("{}/{}", base.trim_end_matches('/'), endpoint)
}

/// Extract the first `data:` payload of an `event: endpoint` SSE block (the
/// legacy MCP SSE transport's session handshake).
fn parse_sse_endpoint_event(text: &str) -> Option<String> {
    let mut event: Option<&str> = None;
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            event = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            event = Some(name.trim());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:")
            && event == Some("endpoint")
        {
            let data = data.trim();
            if !data.is_empty() {
                return Some(data.to_string());
            }
        }
    }
    None
}

/// Extract a `param="value"` pair from a `WWW-Authenticate: Bearer ...`
/// challenge header.
fn parse_challenge_param(header: &str, param: &str) -> Option<String> {
    let needle = format!("{param}=\"");
    let start = header.find(&needle)? + needle.len();
    let end = header[start..].find('"')?;
    Some(header[start..start + end].to_string())
}

/// Authorization-server metadata URLs to probe, in order: the metadata host
/// itself, the RFC 8414 path-inserted variant on the MCP origin, then the
/// OpenID discovery fallback.
fn oauth_server_metadata_urls(auth_base: &str, mcp_url: &str) -> Vec<String> {
    let mut urls = vec![format!(
        "{}/.well-known/oauth-authorization-server",
        auth_base.trim_end_matches('/')
    )];
    let path = mcp_url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path.trim_matches('/'))
        .unwrap_or("");
    if !path.is_empty() {
        urls.push(format!(
            "{}/.well-known/oauth-authorization-server/{path}",
            http_origin(mcp_url)
        ));
    }
    urls.push(format!(
        "{}/.well-known/openid-configuration",
        auth_base.trim_end_matches('/')
    ));
    urls
}

/// RFC 8628 device authorization grant identifier.
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// String entries of a metadata array field, e.g. `scopes_supported`.
fn metadata_strings(meta: &Value, key: &str) -> Vec<String> {
    meta.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Request the protected resource's scopes, without filtering by the AS's
/// potentially incomplete discovery list. Add `offline_access` when the AS
/// advertises it to request a refresh token. Omit an empty scope parameter.
fn requested_scopes(resource_meta: Option<&Value>, server_meta: &Value) -> Option<String> {
    let server_scopes = metadata_strings(server_meta, "scopes_supported");
    let resource_scopes = resource_meta
        .map(|meta| metadata_strings(meta, "scopes_supported"))
        .unwrap_or_default();
    // Resource metadata describes the permissions this MCP server needs. AS
    // discovery may omit supported scopes (RFC 8414); it is not an allowlist.
    let mut scopes = resource_scopes;
    if server_scopes.iter().any(|scope| scope == "offline_access")
        && !scopes.iter().any(|scope| scope == "offline_access")
    {
        scopes.push("offline_access".to_string());
    }
    if scopes.is_empty() {
        return None;
    }
    Some(scopes.join(" "))
}

/// The response mode to use for a loopback redirect, or `None` when the
/// server advertises response modes but none of them can deliver a code to a
/// loopback listener. RFC 6749 makes `query` the default, so an absent (or
/// empty) `response_modes_supported` means the loopback flow is fine.
fn loopback_response_mode(server_meta: &Value) -> Option<&'static str> {
    let modes = metadata_strings(server_meta, "response_modes_supported");
    if modes.is_empty() {
        return Some("query");
    }
    if modes.iter().any(|mode| mode == "query") {
        Some("query")
    } else {
        None
    }
}

/// Whether the server can complete an RFC 8628 device login, which needs
/// neither a redirect URI nor a browser callback reaching a local listener.
fn device_grant_supported(server_meta: &Value) -> bool {
    if !server_meta
        .get("device_authorization_endpoint")
        .and_then(Value::as_str)
        .is_some_and(|endpoint| !endpoint.is_empty())
    {
        return false;
    }
    let grants = metadata_strings(server_meta, "grant_types_supported");
    grants.is_empty() || grants.iter().any(|grant| grant == DEVICE_CODE_GRANT)
}

/// How the authorization code (or its device equivalent) will be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OAuthFlow {
    /// Loopback redirect, optionally naming an advertised query mode.
    Loopback(Option<&'static str>),
    /// RFC 8628 device authorization grant.
    Device,
}

/// Prefer a query redirect or device grant; allow a plain code redirect
/// without an explicit response mode when those are unavailable.
fn select_oauth_flow(server_meta: &Value) -> Result<OAuthFlow, String> {
    select_registered_oauth_flow(server_meta, None)
}

fn select_registered_oauth_flow(
    server_meta: &Value,
    registration: Option<&Value>,
) -> Result<OAuthFlow, String> {
    // A registration response is authoritative for this client's grants. A
    // supported server-wide grant is not necessarily enabled for the client.
    let granted = registration.filter(|meta| meta.get("grant_types").is_some());
    let grants = if let Some(registered) = granted {
        metadata_strings(registered, "grant_types")
    } else {
        metadata_strings(server_meta, "grant_types_supported")
    };
    let code = grants.iter().any(|grant| grant == "authorization_code")
        || (granted.is_none() && grants.is_empty());
    if code && let Some(mode) = loopback_response_mode(server_meta) {
        return Ok(OAuthFlow::Loopback(Some(mode)));
    }
    if device_grant_supported(server_meta)
        && (granted.is_none() || grants.iter().any(|grant| grant == DEVICE_CODE_GRANT))
    {
        return Ok(OAuthFlow::Device);
    }
    // Some providers advertise only web_message but accept a plain code
    // redirect when response_mode is omitted. Never send an unsupported mode.
    if code {
        return Ok(OAuthFlow::Loopback(None));
    }
    Err(
        "the registered client has no usable authorization-code or device authorization grant"
            .into(),
    )
}

/// The login this client will actually run, derived entirely from the
/// discovered metadata: which grant can complete, which scopes to ask for,
/// and which grant types the dynamic client registration should request.
#[derive(Debug)]
struct OAuthPlan {
    flow: OAuthFlow,
    scope: Option<String>,
    registration_grants: Vec<&'static str>,
}

/// Decide the grant and the scope from the metadata, before any network
/// request is sent.
fn plan_oauth_login(
    resource_meta: Option<&Value>,
    server_meta: &Value,
) -> Result<OAuthPlan, String> {
    let flow = select_oauth_flow(server_meta)?;
    // Register both usable flows so rejection of the device grant can fall
    // back to a code redirect without using an unregistered grant.
    let advertised = metadata_strings(server_meta, "grant_types_supported");
    let mut registration_grants = Vec::new();
    if advertised.is_empty() || advertised.iter().any(|grant| grant == "authorization_code") {
        registration_grants.push("authorization_code");
    }
    if device_grant_supported(server_meta) {
        registration_grants.push(DEVICE_CODE_GRANT);
    }
    if advertised.is_empty() || advertised.iter().any(|grant| grant == "refresh_token") {
        registration_grants.push("refresh_token");
    }
    Ok(OAuthPlan {
        flow,
        scope: requested_scopes(resource_meta, server_meta),
        registration_grants,
    })
}

/// Parameters of an authorization request. Omit `response_mode` for the
/// authorization-code fallback and `scope` when discovery yields no scopes.
struct AuthorizationRequest<'a> {
    authorization_endpoint: &'a str,
    client_id: Option<&'a str>,
    redirect_uri: &'a str,
    code_challenge: &'a str,
    state: &'a str,
    response_mode: Option<&'a str>,
    scope: Option<&'a str>,
    resource: &'a str,
}

fn build_authorization_url(request: &AuthorizationRequest<'_>) -> String {
    let mut url = format!(
        "{}?response_type=code&code_challenge={}&code_challenge_method=S256",
        request.authorization_endpoint,
        urlencoding::encode(request.code_challenge),
    );
    if let Some(client_id) = request.client_id {
        url.push_str(&format!("&client_id={}", urlencoding::encode(client_id)));
    }
    url.push_str(&format!(
        "&redirect_uri={}",
        urlencoding::encode(request.redirect_uri)
    ));
    if let Some(scope) = request.scope {
        url.push_str(&format!("&scope={}", urlencoding::encode(scope)));
    }
    if let Some(mode) = request.response_mode {
        url.push_str(&format!("&response_mode={}", urlencoding::encode(mode)));
    }
    url.push_str(&format!("&state={}", urlencoding::encode(request.state)));
    url.push_str(&format!(
        "&resource={}",
        urlencoding::encode(request.resource)
    ));
    url
}

/// An RFC 8628 device authorization response.
#[derive(Debug)]
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval: Duration,
    expires_in: Duration,
}

fn parse_device_authorization(body: &Value, name: &str) -> Result<DeviceAuthorization, String> {
    let field = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.is_empty())
    };
    let device_code = field("device_code")
        .ok_or_else(|| format!("MCP OAuth for '{name}': device response had no device_code"))?;
    let user_code = field("user_code")
        .ok_or_else(|| format!("MCP OAuth for '{name}': device response had no user_code"))?;
    let verification_uri = field("verification_uri")
        .or_else(|| field("verification_url"))
        .ok_or_else(|| {
            format!("MCP OAuth for '{name}': device response had no verification_uri")
        })?;
    // RFC 8628: the client starts polling at `interval` (default 5s) and
    // backs off by 5s on every `slow_down`. `expires_in` bounds the login.
    let interval = body
        .get("interval")
        .and_then(Value::as_u64)
        .filter(|secs| *secs > 0)
        .unwrap_or(5);
    let expires_in = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(900);
    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete: field("verification_uri_complete"),
        interval: Duration::from_secs(interval),
        expires_in: Duration::from_secs(expires_in),
    })
}

/// Whether the interactive browser launch is suppressed (`RUSTCODE_MCP_NO_BROWSER`).
fn browser_launch_suppressed() -> bool {
    std::env::var("RUSTCODE_MCP_NO_BROWSER").is_ok_and(|value| !value.trim().is_empty())
}

fn pkce_pair() -> (String, String) {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let verifier = format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = crate::platform::command("open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = crate::platform::command("cmd")
            .args(["/c", "start", "", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = crate::platform::command("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn oauth_token_path_in_dir(dir: &std::path::Path, server: &str) -> std::path::PathBuf {
    let safe: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join("mcp-oauth").join(format!("{safe}.json"))
}

#[cfg(test)]
fn load_oauth_token_from_dir(dir: &std::path::Path, server: &str) -> Option<OAuthToken> {
    let path = oauth_token_path_in_dir(dir, server);
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

#[cfg(test)]
fn save_oauth_token_to_dir(
    dir: &std::path::Path,
    server: &str,
    token: &OAuthToken,
) -> Result<(), String> {
    write_oauth_token_file(&oauth_token_path_in_dir(dir, server), token)
}

fn write_oauth_token_file(path: &std::path::Path, token: &OAuthToken) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(token)
        .map_err(|e| format!("failed to serialize OAuth token: {e}"))?;
    std::fs::write(path, contents)
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_oauth_token(server: &str) -> Option<OAuthToken> {
    let path = oauth_token_path(server)?;
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Whether RustCode holds a sign-in for `server` (`rustcode mcp login`), read from disk without
/// refreshing it, so a harness thread can tell cheaply that a sign-in arrived or went.
pub(crate) fn has_saved_login(server: &str) -> bool {
    load_oauth_token(server).is_some()
}

/// Remove RustCode's saved sign-in for `server`. `Ok(false)` when there was none.
pub(crate) fn forget_saved_login(server: &str) -> Result<bool, String> {
    let Some(path) = oauth_token_path(server) else {
        return Ok(false);
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "could not remove the sign-in for '{server}': {error}"
        )),
    }
}

fn save_oauth_token(server: &str, token: &OAuthToken) -> Result<(), String> {
    let path = oauth_token_path(server)
        .ok_or_else(|| "no config directory is available for OAuth token storage".to_string())?;
    write_oauth_token_file(&path, token)
}

fn oauth_token_path(server: &str) -> Option<std::path::PathBuf> {
    let dir = crate::config::get_config_dir()?;
    Some(oauth_token_path_in_dir(&dir, server))
}

async fn post_oauth_form_raw(
    http: &reqwest::Client,
    endpoint: &str,
    params: &[(&str, String)],
) -> Result<(reqwest::StatusCode, Value, String), String> {
    let body = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let resp = http
        .post(endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("OAuth token request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let json = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
    Ok((status, json, text))
}

async fn post_oauth_form(
    http: &reqwest::Client,
    endpoint: &str,
    params: &[(&str, String)],
) -> Result<Value, String> {
    let (status, json, text) = post_oauth_form_raw(http, endpoint, params).await?;
    if !status.is_success() {
        let snippet = truncate_snippet(&text, 300);
        return Err(format!(
            "OAuth token request failed (HTTP {status}): {snippet}"
        ));
    }
    if json.is_null() {
        return Err("OAuth token response was not JSON".to_string());
    }
    Ok(json)
}

fn oauth_token_from_response(
    body: &Value,
    client_id: Option<&str>,
    token_endpoint: &str,
) -> Result<OAuthToken, String> {
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "OAuth token response had no access_token".to_string())?
        .to_string();
    if access_token.is_empty() {
        return Err("OAuth token response had an empty access_token".to_string());
    }
    let expires_at = body
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|secs| now_unix_secs().saturating_add(secs.saturating_sub(30)));
    Ok(OAuthToken {
        access_token,
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        client_id: client_id.map(str::to_string),
        token_endpoint: Some(token_endpoint.to_string()),
        expires_at,
    })
}

/// Shared JSON-RPC error tail so stdio and remote responses surface server
/// errors identically.
fn check_jsonrpc_response(resp: Value) -> Result<Value, String> {
    if let Some(err) = resp.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown server error");
        return Err(msg.to_string());
    }
    Ok(resp)
}

/// MCP Apps visibility defaults to both audiences when absent; malformed values fail closed.
pub(crate) fn tool_visible_to(tool: &Value, audience: &str) -> bool {
    match tool.pointer("/_meta/ui/visibility") {
        None => true,
        Some(Value::Array(visibility)) => visibility
            .iter()
            .any(|entry| entry.as_str() == Some(audience)),
        Some(_) => false,
    }
}

async fn bounded_mcp_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let count = reader
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if count > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "MCP response exceeds size limit",
        ));
    }
    Ok((count != 0).then_some(bytes))
}

async fn bounded_mcp_body(mut response: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("response read failed: {e}"))?
    {
        if chunk.len() > MAX_MCP_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err("response exceeds size limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl McpClient {
    #[cfg(test)]
    pub async fn start(
        name: String,
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Result<Arc<Self>, String> {
        Self::start_in_workspace(name, command, args, env, None).await
    }

    pub async fn start_in_workspace(
        name: String,
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
        workspace: Option<&std::path::Path>,
    ) -> Result<Arc<Self>, String> {
        let mut process = crate::platform::tokio_command(&command);
        if let Some(workspace) = workspace {
            process.current_dir(workspace);
        }
        // An engine started by a GUI app inherits a minimal PATH, and a
        // server that shells out (`glab`, `gh`, `bunx`, …) would not find
        // its tools. The server's own `env` still overrides it.
        process.env("PATH", crate::platform::augmented_path());
        let mut child = process
            .args(&args)
            .envs(&env)
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server {name}: {e}"))?;

        let stdin = child.stdin.take().ok_or("Failed to open stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
        let stderr = child.stderr.take().ok_or("Failed to open stderr")?;

        let (tx, mut rx) = mpsc::channel::<Value>(32);
        let pending = Arc::new(Mutex::new(HashMap::<
            i64,
            tokio::sync::oneshot::Sender<Value>,
        >::new()));
        let pending_clone = Arc::clone(&pending);
        let progress: ProgressListeners = Arc::new(Mutex::new(HashMap::new()));
        let progress_clone = Arc::clone(&progress);

        // Stdin writer task
        tokio::spawn(async move {
            let mut writer = stdin;
            while let Some(msg) = rx.recv().await {
                if let Ok(mut line) = serde_json::to_string(&msg) {
                    line.push('\n');
                    if writer.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    if writer.flush().await.is_err() {
                        break;
                    }
                }
            }
        });

        // Stdout reader task
        let stdout_server_name = name.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let bytes = match bounded_mcp_line(&mut reader, MAX_MCP_RESPONSE_BYTES).await {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => break,
                    Err(error) => {
                        crate::dbg_log!("[mcp:{stdout_server_name}] stdout read failed: {error}");
                        if error.kind() == std::io::ErrorKind::InvalidData {
                            for (id, sender) in pending_clone.lock().await.drain() {
                                let _ = sender.send(json!({"jsonrpc":"2.0", "id":id,
                                    "error":{"code":-32000, "message":"MCP response exceeds size limit"}}));
                            }
                        }
                        break;
                    }
                };
                let Ok(line) = String::from_utf8(bytes) else {
                    crate::dbg_log!("[mcp:{stdout_server_name}] invalid UTF-8 on stdout");
                    break;
                };
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if msg.get("method").and_then(Value::as_str) == Some("notifications/progress") {
                        let token = msg["params"]["progressToken"].as_i64();
                        if let Some(listener) = match token {
                            Some(token) => progress_clone.lock().await.get(&token).cloned(),
                            None => None,
                        } {
                            let _ = listener.send(());
                        }
                    } else if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        let mut pend = pending_clone.lock().await;
                        if let Some(sender) = pend.remove(&id) {
                            let _ = sender.send(msg);
                        }
                    }
                } else {
                    // Filter noisy stdout without losing the diagnostic:
                    // count per server and keep the truncated line in debug.log
                    // with the server name attached.
                    note_mcp_non_json_line(&stdout_server_name);
                    crate::dbg_log!(
                        "[mcp:{stdout_server_name}] ignoring non-JSON stdout line: {}",
                        line.chars().take(100).collect::<String>()
                    );
                }
            }
            // Closing stdout means the server can no longer answer pending
            // requests. Drop their senders so callers fail immediately
            // instead of waiting for the request timeout.
            pending_clone.lock().await.clear();
        });

        let stderr_name = name.clone();
        let stderr_diagnostics = Arc::new(StdMutex::new(Vec::new()));
        let stderr_diagnostics_clone = Arc::clone(&stderr_diagnostics);
        let stderr_finished = Arc::new(Notify::new());
        let stderr_finished_clone = Arc::clone(&stderr_finished);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                crate::dbg_log!("[mcp:{stderr_name}] {line}");
                if let Ok(mut diagnostics) = stderr_diagnostics_clone.lock() {
                    const MAX_DIAGNOSTIC_LINES: usize = 16;
                    diagnostics.push(line);
                    if diagnostics.len() > MAX_DIAGNOSTIC_LINES {
                        diagnostics.remove(0);
                    }
                }
            }
            stderr_finished_clone.notify_one();
        });

        let next_id = Arc::new(Mutex::new(1));
        let tools = Arc::new(StdMutex::new(Vec::new()));

        let client = Arc::new(Self {
            name: name.clone(),
            transport: Transport::Stdio { tx },
            pending,
            next_id,
            tools: Arc::clone(&tools),
            child: Arc::new(Mutex::new(Some(child))),
            stderr_diagnostics,
            stderr_finished,
            progress,
            request_timeout_secs: AtomicU64::new(DEFAULT_REQUEST_TIMEOUT_SECS),
        });

        client.handshake().await?;
        Ok(client)
    }

    /// Start a remote (Streamable HTTP, with legacy SSE fallback) MCP server.
    /// The handshake, tool listing, schema-budget participation, and warning
    /// path are identical to stdio once the client exists.
    pub async fn start_remote(
        name: String,
        url: String,
        headers: HashMap<String, String>,
    ) -> Result<Arc<Self>, String> {
        Self::start_remote_configured(name, url, headers, None).await
    }

    async fn start_remote_configured(
        name: String,
        url: String,
        headers: HashMap<String, String>,
        client_id: Option<String>,
    ) -> Result<Arc<Self>, String> {
        let url = url.trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!(
                "MCP server '{name}' has an invalid `url` {url:?}: \
                 expected an http:// or https:// endpoint"
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("MCP server '{name}': failed to build HTTP client: {e}"))?;
        let auth = load_oauth_token(&name);
        let client = Arc::new(Self {
            name: name.clone(),
            transport: Transport::Remote {
                state: Mutex::new(Box::new(RemoteState {
                    url: url.clone(),
                    headers,
                    http,
                    session_id: None,
                    legacy_endpoint: None,
                    auth,
                    client_id,
                })),
            },
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            tools: Arc::new(StdMutex::new(Vec::new())),
            child: Arc::new(Mutex::new(None)),
            stderr_diagnostics: Arc::new(StdMutex::new(Vec::new())),
            stderr_finished: Arc::new(Notify::new()),
            progress: Arc::new(Mutex::new(HashMap::new())),
            request_timeout_secs: AtomicU64::new(DEFAULT_REQUEST_TIMEOUT_SECS),
        });

        client.handshake().await.map_err(|error| {
            format!("Failed to connect to MCP server '{name}' at {url}: {error}")
        })?;
        Ok(client)
    }

    /// Shared `initialize` → `notifications/initialized` → `tools/list`
    /// handshake for both transports.
    async fn handshake(self: &Arc<Self>) -> Result<(), String> {
        // Handshake: initialize
        let _init_res = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "rustcode-client",
                        "version": "1.0.0"
                    }
                }),
            )
            .await?;

        // Send initialized notification
        let _ = self.notify("notifications/initialized", json!({}));

        // Fetch tools list
        let mut tools_list = Vec::new();
        if let Ok(tools_res) = self.call("tools/list", json!({})).await
            && let Some(tools_arr) = tools_res
                .get("result")
                .and_then(|r| r.get("tools"))
                .and_then(|t| t.as_array())
        {
            tools_list = tools_arr.clone();
        }

        // Store tools list
        {
            let mut t = self.tools.lock().map_err(|e| e.to_string())?;
            *t = tools_list;
        }

        Ok(())
    }

    /// Wait this long for a request's response, and for a stdio tool call,
    /// this long since its last progress notification.
    pub(crate) fn set_request_timeout(&self, timeout_secs: Option<u64>) {
        let secs = timeout_secs
            .filter(|secs| *secs > 0)
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        self.request_timeout_secs.store(secs, Ordering::Relaxed);
    }

    fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs.load(Ordering::Relaxed))
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        match &self.transport {
            Transport::Stdio { .. } => self.call_stdio(method, params).await,
            Transport::Remote { .. } => self.call_remote(method, params).await,
        }
    }

    async fn call_stdio(&self, method: &str, params: Value) -> Result<Value, String> {
        let Transport::Stdio { tx } = &self.transport else {
            return Err("MCP client is not a stdio client".to_string());
        };
        let id = {
            let mut nid = self.next_id.lock().await;
            let current = *nid;
            *nid += 1;
            current
        };

        let (tx_one, rx) = tokio::sync::oneshot::channel();
        {
            let mut pend = self.pending.lock().await;
            pend.insert(id, tx_one);
        }

        // Ask for progress on tool calls, so a long one that reports it can
        // outlive the timeout instead of failing while its work goes on.
        let mut params = params;
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        if method == "tools/call"
            && let Some(object) = params.as_object_mut()
        {
            object.insert("_meta".to_string(), json!({"progressToken": id}));
            self.progress.lock().await.insert(id, progress_tx);
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        if let Err(error) = tx.send(req).await {
            self.pending.lock().await.remove(&id);
            self.progress.lock().await.remove(&id);
            return Err(format!("Failed to send request: {error}"));
        }

        // Until it is answered, a request given up on (timed out, or its
        // turn stopped and this future dropped) is cancelled on the server,
        // which would otherwise keep working on a call nobody waits for.
        let mut cancel = CancelOnDrop {
            tx: tx.clone(),
            id,
            armed: method != "initialize",
        };
        let timeout = self.request_timeout();
        let mut rx = rx;
        // Each progress notification restarts the wait for the next sign of
        // life; the total stays bounded by `MAX_TOTAL_TIMEOUTS`.
        let started = tokio::time::Instant::now();
        let total_deadline = started + timeout.saturating_mul(MAX_TOTAL_TIMEOUTS);
        let mut idle_deadline = started + timeout;
        let outcome = loop {
            let wake = idle_deadline.min(total_deadline);
            tokio::select! {
                response = &mut rx => break Ok(response),
                Some(()) = progress_rx.recv() => {
                    idle_deadline = tokio::time::Instant::now() + timeout;
                }
                () = tokio::time::sleep_until(wake) => break Err(wake >= total_deadline),
            }
        };
        self.progress.lock().await.remove(&id);
        // Answered, or the server is gone: nothing left to cancel.
        if outcome.is_ok() {
            cancel.armed = false;
        }
        let resp = match outcome {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                // The server often writes an actionable configuration error to
                // stderr immediately before closing stdout (for example, the
                // mail MCP reports missing IMAP credentials). Give that
                // reader a short chance to finish before falling back to the
                // generic connection error.
                let _ = tokio::time::timeout(
                    Duration::from_millis(100),
                    self.stderr_finished.notified(),
                )
                .await;
                let diagnostics = self
                    .stderr_diagnostics
                    .lock()
                    .map(|lines| lines.join(" | "))
                    .unwrap_or_default();
                if diagnostics.is_empty() {
                    return Err("Server closed connection before responding".to_string());
                }
                return Err(format!(
                    "Server closed connection before responding; server reported: {diagnostics}"
                ));
            }
            Err(true) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "MCP request still running after {}s, the most a call may take with \
                     progress (raise `timeout_secs` for server '{}' to allow longer)",
                    timeout.saturating_mul(MAX_TOTAL_TIMEOUTS).as_secs(),
                    self.name
                ));
            }
            Err(false) => {
                self.pending.lock().await.remove(&id);
                return Err(format!(
                    "MCP request timed out after {}s without a response or progress \
                     (raise `timeout_secs` for server '{}' if its tools run longer)",
                    timeout.as_secs(),
                    self.name
                ));
            }
        };
        check_jsonrpc_response(resp)
    }

    async fn call_remote(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = {
            let mut nid = self.next_id.lock().await;
            let current = *nid;
            *nid += 1;
            current
        };
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let resp = self.remote_roundtrip(id, &body).await?;
        check_jsonrpc_response(resp)
    }

    async fn remote_snapshot(&self) -> Option<RemoteSnapshot> {
        let Transport::Remote { state } = &self.transport else {
            return None;
        };
        let state = state.lock().await;
        let bearer = state
            .auth
            .as_ref()
            .filter(|token| token.usable())
            .map(|token| token.access_token.clone());
        Some(RemoteSnapshot {
            target: state
                .legacy_endpoint
                .clone()
                .unwrap_or_else(|| state.url.clone()),
            url: state.url.clone(),
            headers: state.headers.clone(),
            session_id: state.session_id.clone(),
            bearer,
            http: state.http.clone(),
        })
    }

    async fn store_session_id(&self, session_id: &str) {
        if let Transport::Remote { state } = &self.transport {
            state.lock().await.session_id = Some(session_id.to_string());
        }
    }

    async fn store_legacy_endpoint(&self, endpoint: &str) {
        if let Transport::Remote { state } = &self.transport {
            state.lock().await.legacy_endpoint = Some(endpoint.to_string());
        }
    }

    fn build_remote_post(snapshot: &RemoteSnapshot, body: &Value) -> reqwest::RequestBuilder {
        let mut req = snapshot
            .http
            .post(&snapshot.target)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        {
            use reqwest::header::{HeaderName, HeaderValue};
            for (key, value) in &snapshot.headers {
                if let (Ok(name), Ok(val)) = (
                    HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    req = req.header(name, val);
                } else {
                    crate::dbg_log!("[mcp] ignoring invalid configured header {key:?}");
                }
            }
        }
        if let Some(session) = snapshot.session_id.as_deref() {
            req = req.header("mcp-session-id", session);
        }
        if let Some(bearer) = snapshot.bearer.as_deref()
            && let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {bearer}"))
        {
            req = req.header("Authorization", value);
        }
        req.json(body)
    }

    /// POST one JSON-RPC message, following Streamable HTTP semantics:
    /// single JSON or SSE-wrapped responses, session tracking, one OAuth
    /// retry on 401/403, and one legacy-SSE fallback on 404/405.
    async fn remote_roundtrip(&self, id: i64, body: &Value) -> Result<Value, String> {
        let mut auth_retried = false;
        let mut legacy_retried = false;
        loop {
            let Some(snapshot) = self.remote_snapshot().await else {
                return Err("MCP client is not a remote client".to_string());
            };
            let name = self.name.clone();
            let request = Self::build_remote_post(&snapshot, body).timeout(self.request_timeout());
            let resp = request
                .send()
                .await
                .map_err(|e| remote_connect_error(&name, &snapshot.url, &e))?;
            if let Some(session) = resp
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                self.store_session_id(session).await;
            }
            let status = resp.status();
            if status.is_success() {
                return Self::response_to_jsonrpc(resp, status, id, &name).await;
            }
            if (status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN)
                && !auth_retried
            {
                auth_retried = true;
                let challenge = resp
                    .headers()
                    .get("www-authenticate")
                    .and_then(|value| value.to_str().ok());
                match self.ensure_remote_auth(challenge).await {
                    Ok(true) => continue,
                    Ok(false) => {
                        return Err(auth_required_error(
                            &name,
                            &snapshot.url,
                            "no OAuth token is stored and the interactive browser flow is disabled",
                        ));
                    }
                    Err(flow_error) => return Err(flow_error),
                }
            }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                return Err(auth_required_error(
                    &name,
                    &snapshot.url,
                    "the stored token was rejected",
                ));
            }
            if (status == reqwest::StatusCode::NOT_FOUND
                || status == reqwest::StatusCode::METHOD_NOT_ALLOWED)
                && !legacy_retried
            {
                legacy_retried = true;
                match self.discover_legacy_endpoint().await {
                    Ok(endpoint) => {
                        crate::dbg_log!("[mcp:{name}] falling back to legacy SSE endpoint");
                        self.store_legacy_endpoint(&endpoint).await;
                        continue;
                    }
                    Err(fallback_error) => {
                        crate::dbg_log!("[mcp:{name}] SSE fallback failed: {fallback_error}");
                    }
                }
            }
            let snippet = match bounded_mcp_body(resp).await {
                Ok(body) => truncate_snippet(&String::from_utf8_lossy(&body), 300),
                Err(error) => error,
            };
            return Err(format!(
                "MCP server '{name}' request failed (HTTP {status}): {snippet}"
            ));
        }
    }

    /// Interpret a successful Streamable HTTP response: plain JSON (object or
    /// batch array) or an SSE stream of `data:` JSON payloads.
    async fn response_to_jsonrpc(
        resp: reqwest::Response,
        status: reqwest::StatusCode,
        id: i64,
        name: &str,
    ) -> Result<Value, String> {
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = bounded_mcp_body(resp)
            .await
            .map_err(|e| format!("MCP server '{name}' {e}"))?;
        let text = String::from_utf8(body)
            .map_err(|e| format!("MCP server '{name}' returned invalid UTF-8: {e}"))?;
        if content_type.contains("text/event-stream") {
            for line in text.lines() {
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(payload)
                    && (value.get("id") == Some(&json!(id))
                        || (value.get("id").is_none() && value.get("result").is_some()))
                {
                    return Ok(value);
                }
            }
            return Err(format!(
                "MCP server '{name}' sent an SSE stream with no response for request {id}"
            ));
        }
        if text.trim().is_empty() {
            return Err(format!(
                "MCP server '{name}' accepted the request but returned no result (HTTP {status})"
            ));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| {
            format!(
                "MCP server '{name}' returned invalid JSON: {e} ({})",
                truncate_snippet(text.trim(), 200)
            )
        })?;
        if let Some(array) = value.as_array() {
            return array
                .iter()
                .find(|item| item.get("id") == Some(&json!(id)))
                .cloned()
                .ok_or_else(|| {
                    format!("MCP server '{name}' batch response had no entry for request {id}")
                });
        }
        Ok(value)
    }

    /// Legacy SSE transport fallback: `GET` the endpoint as an event stream,
    /// wait for the `event: endpoint` session handshake, and return the
    /// message POST target it advertises.
    async fn discover_legacy_endpoint(&self) -> Result<String, String> {
        let Some(snapshot) = self.remote_snapshot().await else {
            return Err("MCP client is not a remote client".to_string());
        };
        let name = self.name.clone();
        let resp = snapshot
            .http
            .get(&snapshot.url)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| remote_connect_error(&name, &snapshot.url, &e))?;
        if !resp.status().is_success() {
            return Err(format!(
                "legacy SSE handshake failed (HTTP {})",
                resp.status()
            ));
        }
        use futures_util::StreamExt as _;
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        let found = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|e| format!("SSE read failed: {e}"))?;
                buf.extend_from_slice(&bytes);
                if buf.len() > 65_536 {
                    return Err("SSE stream exceeded 64KiB without an endpoint event".to_string());
                }
                if let Ok(text) = std::str::from_utf8(&buf)
                    && let Some(endpoint) = parse_sse_endpoint_event(text)
                {
                    return Ok(endpoint);
                }
            }
            Err("SSE stream ended without an endpoint event".to_string())
        })
        .await
        .map_err(|_| format!("MCP server '{name}' legacy SSE handshake timed out after 10s"))??;
        Ok(join_endpoint(&snapshot.url, &found))
    }

    /// Ensure a usable OAuth token is stored, running the refresh or browser
    /// flow when needed. Returns `true` when the caller should retry the
    /// request, `false` when no token can be obtained non-interactively.
    async fn ensure_remote_auth(&self, challenge: Option<&str>) -> Result<bool, String> {
        self.ensure_remote_auth_with(&launch_oauth_browser, challenge)
            .await
    }

    async fn ensure_remote_auth_with(
        &self,
        on_login_url: &(dyn Fn(&str) + Send + Sync),
        challenge: Option<&str>,
    ) -> Result<bool, String> {
        let name = self.name.clone();
        let (http, stored) = match &self.transport {
            Transport::Remote { state } => {
                let state = state.lock().await;
                (state.http.clone(), state.auth.clone())
            }
            Transport::Stdio { .. } => return Err("MCP client is not a remote client".to_string()),
        };
        if let Some(token) = stored {
            if token.usable() {
                return Ok(true);
            }
            if let (Some(refresh), Some(endpoint)) =
                (token.refresh_token.clone(), token.token_endpoint.clone())
                && let Ok(next) =
                    refresh_access_token(&http, &endpoint, token.client_id.as_deref(), &refresh)
                        .await
            {
                self.store_auth_token(&next).await;
                if let Err(error) = save_oauth_token(&name, &next) {
                    crate::dbg_log!("[mcp:{name}] failed to persist refreshed token: {error}");
                }
                return Ok(true);
            }
        }
        match self
            .run_oauth_flow_with(&http, on_login_url, challenge)
            .await
        {
            Ok(token) => {
                self.store_auth_token(&token).await;
                if let Err(error) = save_oauth_token(&name, &token) {
                    eprintln!(
                        "[mcp] WARNING: OAuth login succeeded but the token could not be persisted: {error}"
                    );
                }
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    async fn store_auth_token(&self, token: &OAuthToken) {
        if let Transport::Remote { state } = &self.transport {
            state.lock().await.auth = Some(token.clone());
        }
    }

    /// Full OAuth 2.1 login: protected-resource discovery, server metadata,
    /// dynamic client registration, then whichever grant the server can
    /// actually complete — a loopback authorization code, or the RFC 8628
    /// device grant when no query redirect
    /// response mode is advertised. Tokens are returned to the caller for
    /// durable storage outside `config.toml`.
    #[cfg(test)]
    async fn run_oauth_flow(&self, http: &reqwest::Client) -> Result<OAuthToken, String> {
        self.run_oauth_flow_with(http, &launch_oauth_browser, None)
            .await
    }

    async fn run_oauth_flow_with(
        &self,
        http: &reqwest::Client,
        on_login_url: &(dyn Fn(&str) + Send + Sync),
        challenge: Option<&str>,
    ) -> Result<OAuthToken, String> {
        let name = self.name.clone();
        let mcp_url = match &self.transport {
            Transport::Remote { state } => state.lock().await.url.clone(),
            Transport::Stdio { .. } => return Err("MCP client is not a remote client".to_string()),
        };
        let www_auth = if let Some(challenge) = challenge {
            challenge.to_string()
        } else {
            let probe = http
                .get(&mcp_url)
                .header("Accept", "application/json, text/event-stream")
                .send()
                .await
                .map_err(|error| remote_connect_error(&name, &mcp_url, &error))?;
            probe
                .headers()
                .get("www-authenticate")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };

        let mut auth_server_bases: Vec<String> = Vec::new();
        let mut resource_meta: Option<Value> = None;
        if let Some(meta_url) = parse_challenge_param(&www_auth, "resource_metadata")
            && let Ok(meta_resp) = http.get(&meta_url).send().await
            && meta_resp.status().is_success()
            && let Ok(meta) = meta_resp.json::<Value>().await
        {
            if let Some(servers) = meta.get("authorization_servers").and_then(Value::as_array) {
                auth_server_bases
                    .extend(servers.iter().filter_map(Value::as_str).map(str::to_string));
            }
            resource_meta = Some(meta);
        }
        if auth_server_bases.is_empty() {
            auth_server_bases.push(http_origin(&mcp_url));
        }

        let mut server_meta: Option<Value> = None;
        for base in &auth_server_bases {
            for candidate in oauth_server_metadata_urls(base, &mcp_url) {
                let Ok(resp) = http.get(&candidate).send().await else {
                    continue;
                };
                if !resp.status().is_success() {
                    continue;
                }
                let Ok(meta) = resp.json::<Value>().await else {
                    continue;
                };
                if meta
                    .get("authorization_endpoint")
                    .and_then(Value::as_str)
                    .is_some()
                    && meta.get("token_endpoint").and_then(Value::as_str).is_some()
                {
                    server_meta = Some(meta);
                    break;
                }
            }
            if server_meta.is_some() {
                break;
            }
        }
        let meta = server_meta.ok_or_else(|| {
            auth_required_error(
                &name,
                &mcp_url,
                "the server did not advertise OAuth authorization endpoints",
            )
        })?;
        let authorization_endpoint = meta["authorization_endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let token_endpoint = meta["token_endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let registration_endpoint = meta
            .get("registration_endpoint")
            .and_then(Value::as_str)
            .map(str::to_string);

        // The grant and the scope come from the metadata, never a hardcoded
        // list: the request asks for scopes this server supports, and for the
        // refresh token that makes the login reusable across restarts.
        let plan = plan_oauth_login(resource_meta.as_ref(), &meta)
            .map_err(|detail| auth_required_error(&name, &mcp_url, &detail))?;
        let mut flow = plan.flow;
        let challenged_scopes = parse_challenge_param(&www_auth, "scope");
        let scope = if let Some(challenged) = challenged_scopes {
            requested_scopes(
                Some(
                    &json!({"scopes_supported": challenged.split_whitespace().collect::<Vec<_>>() }),
                ),
                &meta,
            )
        } else {
            plan.scope
        };

        // The listener is bound for both flows: the device grant does not use
        // it, but registering a client with a loopback redirect URI keeps
        // dynamic registration compatible with servers that require one.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| {
                format!("MCP OAuth for '{name}': failed to bind a local redirect port: {e}")
            })?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("MCP OAuth for '{name}': failed to read local port: {e}"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");

        let mut client_id = match &self.transport {
            Transport::Remote { state } => state.lock().await.client_id.clone(),
            Transport::Stdio { .. } => None,
        }
        .filter(|id| !id.trim().is_empty());
        let mut code_granted = plan.registration_grants.contains(&"authorization_code");
        if client_id.is_none() {
            let endpoint = registration_endpoint.ok_or_else(|| auth_required_error(
                &name, &mcp_url, "configure a pre-registered `client_id`; the server offers no dynamic registration endpoint",
            ))?;
            let mut registration = json!({
                "redirect_uris": [redirect_uri],
                "client_name": "rustcode",
                "application_type": "native",
                "grant_types": plan.registration_grants,
                "response_types": if plan.registration_grants.contains(&"authorization_code") { vec!["code"] } else { vec![] },
                "token_endpoint_auth_method": "none",
            });
            if let Some(scope) = &scope {
                registration["scope"] = json!(scope);
            }
            let resp = http
                .post(&endpoint)
                .json(&registration)
                .send()
                .await
                .map_err(|error| {
                    format!("MCP OAuth for '{name}': client registration failed: {error}")
                })?;
            let status = resp.status();
            let text = resp.text().await.map_err(|error| {
                format!("MCP OAuth for '{name}': client registration response failed: {error}")
            })?;
            if !status.is_success() {
                return Err(format!(
                    "MCP OAuth for '{name}': client registration failed (HTTP {status}): {}",
                    truncate_snippet(&text, 300)
                ));
            }
            let registered: Value = serde_json::from_str(&text).map_err(|error| {
                format!("MCP OAuth for '{name}': invalid client registration response: {error}")
            })?;
            client_id = Some(
                registered
                    .get("client_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        format!("MCP OAuth for '{name}': client registration returned no client_id")
                    })?
                    .to_string(),
            );
            flow = select_registered_oauth_flow(&meta, Some(&registered))
                .map_err(|detail| auth_required_error(&name, &mcp_url, &detail))?;
            code_granted = registered.get("grant_types").is_none()
                || metadata_strings(&registered, "grant_types")
                    .iter()
                    .any(|grant| grant == "authorization_code");
        }
        if flow == OAuthFlow::Device {
            match run_device_grant(
                http,
                &name,
                meta["device_authorization_endpoint"]
                    .as_str()
                    .unwrap_or_default(),
                &token_endpoint,
                client_id.as_deref(),
                scope.as_deref(),
                on_login_url,
            )
            .await
            {
                Ok(token) => return Ok(token),
                Err(DeviceGrantError::Unavailable(error)) if code_granted => {
                    crate::dbg_log!(
                        "[mcp:{name}] device grant unavailable, falling back to authorization code: {error}"
                    );
                    flow = OAuthFlow::Loopback(None);
                }
                Err(error) => return Err(error.into_string()),
            }
        }
        let OAuthFlow::Loopback(response_mode) = flow else {
            unreachable!("device flow returned above")
        };
        let (verifier, challenge) = pkce_pair();
        let flow_state = format!("{:032x}", rand::random::<u128>());
        let auth_url = build_authorization_url(&AuthorizationRequest {
            authorization_endpoint: &authorization_endpoint,
            client_id: client_id.as_deref(),
            redirect_uri: &redirect_uri,
            code_challenge: &challenge,
            state: &flow_state,
            response_mode,
            scope: scope.as_deref(),
            resource: &mcp_url,
        });

        eprintln!(
            "[mcp] OAuth login required for '{name}': open this URL in your browser:\n{auth_url}"
        );
        on_login_url(&auth_url);

        let (code, returned_state) = wait_for_oauth_code(listener, &name).await?;
        if returned_state != flow_state {
            return Err(format!(
                "MCP OAuth for '{name}': state mismatch; the login response was not for this attempt"
            ));
        }
        let form_client_id = client_id.clone();
        let body = post_oauth_form(
            http,
            &token_endpoint,
            &[
                ("grant_type", "authorization_code".to_string()),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
                ("resource", mcp_url),
            ]
            .into_iter()
            .chain(client_id.map(|id| ("client_id", id)))
            .collect::<Vec<_>>(),
        )
        .await?;
        oauth_token_from_response(&body, form_client_id.as_deref(), &token_endpoint)
    }

    /// Return only the tool result; JSON-RPC request IDs must not affect polling hashes.
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let response = self
            .call("tools/call", json!({"name": tool, "arguments": arguments}))
            .await?;
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "MCP response missing result".into())
    }

    /// Call an app-visible tool on this exact server only after the host approves it.
    /// The approval callback receives the server, tool and arguments for an origin-labelled prompt.
    pub async fn call_app_tool<F, Fut>(
        &self,
        tool: &str,
        arguments: Value,
        approve: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(&str, &str, &Value) -> Fut,
        Fut: Future<Output = bool>,
    {
        if !arguments.is_object() {
            return Err("MCP app tool arguments must be an object".into());
        }
        if serde_json::to_vec(&arguments)
            .map_err(|e| e.to_string())?
            .len()
            > 512 * 1024
        {
            return Err("MCP app tool arguments exceed size limit".into());
        }
        let allowed = self.get_tools()?.iter().any(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(tool) && tool_visible_to(entry, "app")
        });
        if !allowed {
            return Err("MCP app tool is unavailable or not visible to apps".into());
        }
        if !approve(&self.name, tool, &arguments).await {
            return Err("MCP app tool call was not approved".into());
        }
        self.call_tool(tool, arguments).await
    }

    /// Fetch only the UI resource declared by this server's tool definition.
    /// A caller cannot use this method to read an arbitrary server resource.
    pub async fn read_tool_app(
        &self,
        tool: &str,
    ) -> Result<Option<rustcode_core::mcp_app::McpAppResource>, String> {
        let uri = self
            .get_tools()?
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(tool))
            .and_then(|entry| entry.pointer("/_meta/ui/resourceUri"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(uri) = uri else {
            return Ok(None);
        };
        if !uri.starts_with("ui://") {
            return Err("MCP app resource must use ui://".into());
        }
        let response = self.call("resources/read", json!({"uri": uri})).await?;
        let contents = response
            .pointer("/result/contents")
            .and_then(Value::as_array)
            .ok_or("MCP app resource is unavailable")?;
        let resource = contents
            .iter()
            .find(|entry| entry.get("uri").and_then(Value::as_str) == Some(uri.as_str()))
            .ok_or("MCP app resource URI did not match")?;
        let text = resource
            .get("text")
            .and_then(Value::as_str)
            .ok_or("MCP app resource is not text")?;
        let mime = resource.get("mimeType").and_then(Value::as_str);
        rustcode_core::mcp_app::mcp_app_resource(&uri, mime, text)
            .map(Some)
            .ok_or_else(|| "MCP app resource has an unsupported MIME type".into())
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        match &self.transport {
            Transport::Stdio { tx } => {
                let req = json!({
                    "jsonrpc": "2.0",
                    "method": method,
                    "params": params
                });
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(req).await;
                });
                Ok(())
            }
            Transport::Remote { state } => {
                let Ok(state) = state.try_lock() else {
                    return Ok(());
                };
                let snapshot = RemoteSnapshot {
                    target: state
                        .legacy_endpoint
                        .clone()
                        .unwrap_or_else(|| state.url.clone()),
                    url: state.url.clone(),
                    headers: state.headers.clone(),
                    session_id: state.session_id.clone(),
                    bearer: state
                        .auth
                        .as_ref()
                        .filter(|token| token.usable())
                        .map(|token| token.access_token.clone()),
                    http: state.http.clone(),
                };
                drop(state);
                let body = json!({
                    "jsonrpc": "2.0",
                    "method": method,
                    "params": params
                });
                tokio::spawn(async move {
                    let _ = Self::build_remote_post(&snapshot, &body).send().await;
                });
                Ok(())
            }
        }
    }

    pub fn get_tools(&self) -> Result<Vec<Value>, String> {
        let t = self.tools.lock().map_err(|e| e.to_string())?;
        Ok(t.clone())
    }

    pub async fn shutdown(&self) {
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
        }
        // Best-effort Streamable HTTP session close; never blocks shutdown.
        if let Transport::Remote { state } = &self.transport
            && let Ok(state) = state.try_lock()
            && let Some(session) = state.session_id.clone()
        {
            let http = state.http.clone();
            let url = state.url.clone();
            drop(state);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = http
                        .delete(&url)
                        .header("mcp-session-id", session)
                        .send()
                        .await;
                });
            }
        }
    }
}

/// The access token RustCode holds for a remote server, for a harness that
/// connects to it on its own (Claude Code, Codex): refreshed and saved first
/// when it expired and can be. None when there is no usable sign-in.
pub(crate) async fn bearer_for_harness(server: &str) -> Option<String> {
    let token = load_oauth_token(server)?;
    if token.usable() {
        return Some(token.access_token);
    }
    let (Some(refresh), Some(endpoint)) = (
        token.refresh_token.as_deref(),
        token.token_endpoint.as_deref(),
    ) else {
        return None;
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .ok()?;
    match refresh_access_token(&http, endpoint, token.client_id.as_deref(), refresh).await {
        Ok(fresh) => {
            if let Err(error) = save_oauth_token(server, &fresh) {
                crate::dbg_log!("harness MCP: '{server}' token not saved: {error}");
            }
            Some(fresh.access_token)
        }
        Err(error) => {
            crate::dbg_log!("harness MCP: '{server}' token refresh failed: {error}");
            None
        }
    }
}

/// The servers with RustCode's sign-in added: a remote server without its own
/// `Authorization` header gets the saved OAuth token as one, so a harness uses
/// the login made in RustCode instead of failing on its own.
pub(crate) async fn with_saved_logins(
    mut servers: Vec<crate::config::McpServerConfig>,
) -> Vec<crate::config::McpServerConfig> {
    for server in &mut servers {
        if server.url.is_none()
            || server
                .headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("authorization"))
        {
            continue;
        }
        if let Some(token) = bearer_for_harness(&server.name).await {
            server
                .headers
                .insert("Authorization".to_owned(), format!("Bearer {token}"));
        }
    }
    servers
}

async fn refresh_access_token(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: Option<&str>,
    refresh_token: &str,
) -> Result<OAuthToken, String> {
    let body = post_oauth_form(
        http,
        token_endpoint,
        &[
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh_token.to_string()),
        ]
        .into_iter()
        .chain(client_id.map(|id| ("client_id", id.to_string())))
        .collect::<Vec<_>>(),
    )
    .await?;
    let mut token = oauth_token_from_response(&body, client_id, token_endpoint)?;
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh_token.to_string());
    }
    Ok(token)
}

enum DeviceGrantError {
    Unavailable(String),
    Failed(String),
}
impl DeviceGrantError {
    fn into_string(self) -> String {
        match self {
            Self::Unavailable(error) | Self::Failed(error) => error,
        }
    }
}
impl From<String> for DeviceGrantError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
fn launch_oauth_browser(url: &str) {
    if !browser_launch_suppressed() {
        open_browser(url);
    }
}

/// RFC 8628 device authorization grant: no redirect URI and no browser
/// callback reaching a local listener, so it works against a server whose
/// only advertised response mode is `web_message.opener`.
async fn run_device_grant(
    http: &reqwest::Client,
    name: &str,
    device_endpoint: &str,
    token_endpoint: &str,
    client_id: Option<&str>,
    scope: Option<&str>,
    on_login_url: &(dyn Fn(&str) + Send + Sync),
) -> Result<OAuthToken, DeviceGrantError> {
    let device_params = device_authorization_params(client_id, scope);
    let (status, body, text) = post_oauth_form_raw(http, device_endpoint, &device_params).await?;
    if !status.is_success() {
        let snippet = truncate_snippet(&text, 300);
        let error = format!(
            "MCP OAuth for '{name}': device authorization request failed (HTTP {status}): {snippet}"
        );
        return Err(
            if status.as_u16() == 404
                || status.as_u16() == 405
                || matches!(
                    body.get("error").and_then(Value::as_str),
                    Some("unauthorized_client" | "unsupported_grant_type" | "invalid_client")
                )
            {
                DeviceGrantError::Unavailable(error)
            } else {
                DeviceGrantError::Failed(error)
            },
        );
    }
    let device = parse_device_authorization(&body, name)?;

    eprintln!(
        "[mcp] OAuth login required for '{name}': the authorization server does not support \
         browser redirects, so use the device code flow.\n\
         Visit {} and enter code: {}",
        device.verification_uri, device.user_code
    );
    {
        let openable = device
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| device.verification_uri.clone());
        on_login_url(&openable);
    }

    let token = poll_device_token(
        http,
        name,
        token_endpoint,
        client_id,
        &device.device_code,
        device.interval,
        device.expires_in,
    )
    .await?;
    let token = oauth_token_from_response(&token, client_id, token_endpoint)?;
    if token.refresh_token.is_none() {
        crate::dbg_log!("[mcp:{name}] device login returned no refresh token");
    }
    Ok(token)
}

/// Form parameters for the RFC 8628 device authorization request.
fn device_authorization_params(
    client_id: Option<&str>,
    scope: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut params = Vec::new();
    if let Some(client_id) = client_id {
        params.push(("client_id", client_id.to_string()));
    }
    if let Some(scope) = scope {
        params.push(("scope", scope.to_string()));
    }
    params
}

/// Poll the token endpoint until the user finishes (or abandons) the device
/// login. RFC 8628 §3.5: `authorization_pending` means keep waiting,
/// `slow_down` means back off by 5s, `expired_token` and `access_denied`
/// are terminal.
async fn poll_device_token(
    http: &reqwest::Client,
    name: &str,
    token_endpoint: &str,
    client_id: Option<&str>,
    device_code: &str,
    mut interval: Duration,
    expires_in: Duration,
) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + expires_in;
    loop {
        tokio::time::sleep(interval).await;
        let params = device_token_params(client_id, device_code);
        let (status, body, text) = post_oauth_form_raw(http, token_endpoint, &params).await?;
        if status.is_success() {
            return Ok(body);
        }
        let error = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval = interval.saturating_add(Duration::from_secs(5)),
            "access_denied" => {
                return Err(format!(
                    "MCP OAuth for '{name}': the device login was denied before it was approved"
                ));
            }
            "expired_token" => {
                return Err(format!(
                    "MCP OAuth for '{name}': the device code expired before the login was approved"
                ));
            }
            _ => {
                let snippet = truncate_snippet(&text, 300);
                return Err(format!(
                    "MCP OAuth for '{name}': device token request failed (HTTP {status}): {snippet}"
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "MCP OAuth for '{name}': timed out waiting for the device login"
            ));
        }
    }
}

fn device_token_params(client_id: Option<&str>, device_code: &str) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("grant_type", DEVICE_CODE_GRANT.to_string()),
        ("device_code", device_code.to_string()),
    ];
    if let Some(client_id) = client_id {
        params.push(("client_id", client_id.to_string()));
    }
    params
}

/// Wait for the OAuth authorization server to redirect back to the loopback
/// listener, and return the `(code, state)` pair from the query string.
async fn wait_for_oauth_code(
    listener: tokio::net::TcpListener,
    name: &str,
) -> Result<(String, String), String> {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(300), listener.accept())
        .await
        .map_err(|_| format!("MCP OAuth for '{name}': timed out waiting for the browser login"))
        .and_then(|result| {
            result.map_err(|e| format!("MCP OAuth for '{name}': local redirect failed: {e}"))
        })?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        use tokio::io::AsyncReadExt as _;
        match socket.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 16_384 {
                    break;
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or_default();
    let target = request_line.split_whitespace().nth(1).unwrap_or_default();
    // Browsers do not transmit URL fragments to an HTTP listener.
    let params = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    for pair in params.split('&') {
        if let Some((key, value)) = pair.split_once('=')
            && let Ok(decoded) = urlencoding::decode(value)
        {
            match key {
                "code" => code = Some(decoded.into_owned()),
                "state" => state = Some(decoded.into_owned()),
                _ => {}
            }
        }
    }
    let page = "<html><body><h1>Login complete</h1>\
        <p>You can close this tab and return to RustCode.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => Err(format!(
            "MCP OAuth for '{name}': the browser redirect had no authorization code"
        )),
    }
}

pub async fn start_server_by_name(name: &str) -> Result<(), String> {
    let workspace = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    start_server_by_name_in_workspace(name, &workspace).await
}

/// A job owns this client; it is never shared across workspace configurations.
pub(crate) async fn start_owned_server(
    server: &crate::config::McpServerConfig,
    workspace: &std::path::Path,
) -> Result<Arc<McpClient>, String> {
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err("MCP workspace must be an existing absolute directory".into());
    }
    let name = &server.name;
    if !server.enabled {
        return Err(format!("MCP server '{name}' is disabled"));
    }
    // A missing transport is a configuration error, surfaced through the same
    // warning path as a spawn or connection failure — never a silent skip.
    server.validate()?;
    let client = if server.is_remote() {
        let url = server.url.clone().unwrap_or_default();
        let headers = server.headers.clone();
        let name = name.clone();
        tokio::time::timeout(
            Duration::from_secs(10),
            McpClient::start_remote_configured(
                name.clone(),
                url,
                headers,
                server.client_id.clone(),
            ),
        )
        .await
        .map_err(|_| format!("MCP server '{name}' startup timed out"))??
    } else {
        tokio::time::timeout(
            Duration::from_secs(10),
            McpClient::start_in_workspace(
                server.name.clone(),
                server.command.clone(),
                server.args.clone(),
                server.env.clone(),
                Some(workspace),
            ),
        )
        .await
        .map_err(|_| format!("MCP server '{name}' startup timed out"))??
    };
    client.set_request_timeout(server.timeout_secs);
    Ok(client)
}

/// Owns an isolated registry and reaps clients even when a turn is dropped.
pub(crate) struct ScheduledServers(pub(crate) McpRegistry);

impl ScheduledServers {
    pub fn new() -> Self {
        Self(Arc::new(StdMutex::new(HashMap::new())))
    }

    pub(crate) fn insert(&mut self, client: Arc<McpClient>) -> Result<(), String> {
        let mut registry = self.0.lock().map_err(|e| e.to_string())?;
        if registry.contains_key(&client.name) {
            return Err(format!(
                "MCP server '{}' is already owned by another turn",
                client.name
            ));
        }
        registry.insert(client.name.clone(), client.clone());
        Ok(())
    }
}

impl Drop for ScheduledServers {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.0.lock() {
            for (_, client) in registry.drain() {
                // The client may also be retained by a cancelled blocking tool.
                // Explicitly terminate it instead of relying on the last Arc.
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        client.shutdown().await;
                    });
                }
            }
        }
    }
}

pub async fn start_server_by_name_in_workspace(
    name: &str,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let config = {
        let cfg = crate::config::load_config_for_workspace(workspace).2;
        cfg.mcp_servers.iter().find(|s| s.name == name).cloned()
    };

    match config {
        Some(srv_config) if srv_config.enabled => {
            start_configured_server(srv_config, workspace).await
        }
        _ => Ok(()),
    }
}

/// Start `srv_config` in this session, replacing a running server of the same
/// name. The registry lock is only taken once the client is up, never across
/// the spawn or handshake.
pub(crate) async fn start_configured_server(
    srv_config: crate::config::McpServerConfig,
    workspace: &std::path::Path,
) -> Result<(), String> {
    srv_config.validate()?;
    let name = srv_config.name.clone();
    let timeout_secs = srv_config.timeout_secs;
    shutdown_server(&name).await;

    let client = if srv_config.is_remote() {
        McpClient::start_remote_configured(
            srv_config.name.clone(),
            srv_config.url.clone().unwrap_or_default(),
            srv_config.headers.clone(),
            srv_config.client_id.clone(),
        )
        .await?
    } else {
        McpClient::start_in_workspace(
            srv_config.name.clone(),
            srv_config.command,
            srv_config.args,
            srv_config.env,
            Some(workspace),
        )
        .await?
    };
    client.set_request_timeout(timeout_secs);
    clear_mcp_startup_failure(&name);
    if let Ok(mut reg) = get_mcp_registry().lock() {
        reg.insert(name, client);
    }
    bump_mcp_generation();
    Ok(())
}

/// The config files and modification times the last reconcile saw, and the
/// server list it applied. A turn reconciles only when a file changed.
type ConfigStamp = Vec<(std::path::PathBuf, Option<std::time::SystemTime>)>;
static APPLIED_CONFIG: StdMutex<Option<(ConfigStamp, Vec<crate::config::McpServerConfig>)>> =
    StdMutex::new(None);

fn config_stamp(workspace: &std::path::Path) -> ConfigStamp {
    crate::config::config_files_for_workspace(workspace)
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            (path, modified)
        })
        .collect()
}

/// What bringing the running servers in line with the config takes: the
/// servers to (re)start and the names to stop. Only differences from the
/// last applied list act: a server that failed to start is not retried every
/// turn, and a new one the app already started is not started again.
pub(crate) fn plan_reconcile(
    previous: &[crate::config::McpServerConfig],
    current: &[crate::config::McpServerConfig],
    running: &HashSet<String>,
) -> (Vec<crate::config::McpServerConfig>, Vec<String>) {
    let start = current
        .iter()
        .filter(|server| server.enabled)
        .filter(
            |server| match previous.iter().find(|p| p.name == server.name) {
                Some(before) => before != *server,
                None => !running.contains(&server.name),
            },
        )
        .cloned()
        .collect();
    let stop = running
        .iter()
        .filter(|name| {
            !current
                .iter()
                .any(|server| server.enabled && &server.name == *name)
        })
        .filter(|name| previous.iter().any(|server| &server.name == *name))
        .cloned()
        .collect();
    (start, stop)
}

/// Start servers added to the config since the session began and stop ones
/// removed or turned off, wherever the edit came from: `rustcode mcp add`,
/// `/mcp`, the app, or a hand-edited file. Cheap when nothing changed: one
/// `stat` per config file. Turns with their own registry (scheduled jobs)
/// keep theirs.
pub(crate) async fn reconcile_configured_servers(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
) {
    if DIRECT_MCP_REGISTRY.try_with(|_| ()).is_ok() {
        return;
    }
    let (workspace, session_servers) = {
        let s = state.lock().await;
        let workspace = s
            .effective_workspace_root()
            .or_else(|| std::env::current_dir().ok());
        (workspace, s.config.mcp_servers.clone())
    };
    let Some(workspace) = workspace else {
        return;
    };
    let stamp = config_stamp(&workspace);
    let previous = {
        let applied = APPLIED_CONFIG
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match applied.as_ref() {
            Some((last, _)) if *last == stamp => return,
            Some((_, servers)) => servers.clone(),
            None => session_servers.clone(),
        }
    };
    let current = crate::config::load_config_for_workspace(&workspace)
        .2
        .mcp_servers;
    *APPLIED_CONFIG
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((stamp, current.clone()));
    if session_servers != current {
        let mut s = state.lock().await;
        s.config.mcp_servers = current.clone();
        s.request_redraw();
    }
    let running = get_mcp_registry()
        .lock()
        .map(|registry| registry.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    let (start, stop) = plan_reconcile(&previous, &current, &running);
    if start.is_empty() && stop.is_empty() {
        return;
    }
    for name in &stop {
        shutdown_server(name).await;
    }
    let started_at = std::time::Instant::now();
    let results = futures_util::future::join_all(start.iter().map(|server| {
        let workspace = workspace.clone();
        async move {
            let name = server.name.clone();
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                start_configured_server(server.clone(), &workspace),
            )
            .await
            .unwrap_or_else(|_| Err(format!("MCP server '{name}' startup timed out")));
            if let Err(error) = &result {
                record_mcp_startup_failure(&name, error);
            }
            (name, result.is_ok())
        }
    }))
    .await;
    crate::logger::operational_event(
        "mcp.config_reconcile",
        serde_json::json!({
            "started": results.iter().filter(|(_, ok)| *ok).map(|(name, _)| name).collect::<Vec<_>>(),
            "failed": results.iter().filter(|(_, ok)| !*ok).map(|(name, _)| name).collect::<Vec<_>>(),
            "stopped": stop,
            "elapsed_ms": started_at.elapsed().as_millis() as u64,
        }),
    );
}

pub async fn shutdown_server(name: &str) {
    let client = {
        if let Ok(mut reg) = get_mcp_registry().lock() {
            reg.remove(name)
        } else {
            None
        }
    };
    if let Some(c) = client {
        c.shutdown().await;
        bump_mcp_generation();
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mcp/oauth_tests.rs"]
mod oauth_tests;

#[cfg(test)]
mod deferred_start_tests {
    fn server(name: &str, enabled: bool) -> crate::config::McpServerConfig {
        serde_json::from_value(serde_json::json!({"name": name, "enabled": enabled}))
            .expect("server config")
    }

    /// Startup queues enabled servers once and clears their connecting state.
    #[tokio::test]
    async fn queued_servers_start_once() {
        super::defer_enabled_servers(&[server("lazy-a", true), server("lazy-off", false)]);
        assert_eq!(super::deferred_server_names(), ["lazy-a"]);
        assert_eq!(super::connecting_servers(), ["lazy-a"]);
        assert!(super::get_mcp_registry().lock().unwrap().is_empty());

        // `lazy-a` is in no config file, so the start finds nothing to run.
        let background = tokio::spawn(super::start_deferred_servers());
        background.await.unwrap();
        assert!(super::deferred_server_names().is_empty());
        assert!(super::connecting_servers().is_empty());
        super::start_deferred_servers().await;
        assert!(super::deferred_server_names().is_empty());
    }
}

#[cfg(test)]
mod harness_login_tests {
    use super::{OAuthToken, save_oauth_token, with_saved_logins};
    use crate::config::McpServerConfig;

    fn server(name: &str, url: Option<&str>) -> McpServerConfig {
        McpServerConfig {
            name: name.to_owned(),
            command: if url.is_some() {
                String::new()
            } else {
                "srv".to_owned()
            },
            args: vec![],
            env: Default::default(),
            url: url.map(str::to_owned),
            headers: Default::default(),
            client_id: None,
            enabled: true,
            always_include: false,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn a_harness_gets_the_login_made_in_rustcode() {
        for name in ["login-remote", "login-own-header", "login-stdio"] {
            save_oauth_token(
                name,
                &OAuthToken {
                    access_token: "tok".into(),
                    refresh_token: None,
                    client_id: None,
                    token_endpoint: None,
                    expires_at: None,
                },
            )
            .unwrap();
        }
        let mut own = server("login-own-header", Some("https://b.example/mcp"));
        own.headers
            .insert("authorization".into(), "Bearer mine".into());
        let servers = with_saved_logins(vec![
            server("login-remote", Some("https://a.example/mcp")),
            own,
            server("login-stdio", None),
            server("login-none", Some("https://c.example/mcp")),
        ])
        .await;
        assert_eq!(servers[0].headers["Authorization"], "Bearer tok");
        assert_eq!(servers[1].headers.len(), 1, "its own header is kept");
        assert!(
            servers[2].headers.is_empty(),
            "stdio servers take no header"
        );
        assert!(servers[3].headers.is_empty(), "no sign-in, no header");
    }
}
