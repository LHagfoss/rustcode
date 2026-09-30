use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify, mpsc};

#[allow(dead_code)]
pub struct McpClient {
    pub name: String,
    transport: Transport,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    next_id: Arc<Mutex<i64>>,
    tools: Arc<StdMutex<Vec<Value>>>,
    child: Arc<Mutex<Option<Child>>>,
    stderr_diagnostics: Arc<StdMutex<Vec<String>>>,
    stderr_finished: Arc<Notify>,
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
                let msg = format!("[mcp] failed to start server {name}: {error}");
                crate::dbg_log!("{msg}");
                warnings.push(msg);
            }
            Err(_) => {
                let msg = format!(
                    "[mcp] timed out starting server {name} after {:.1}s; continuing",
                    startup_timeout.as_secs_f64()
                );
                crate::dbg_log!("{msg}");
                warnings.push(msg);
            }
        }
    }
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
        let _ = std::process::Command::new("open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = std::process::Command::new("xdg-open")
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

fn save_oauth_token(server: &str, token: &OAuthToken) -> Result<(), String> {
    let path = oauth_token_path(server)
        .ok_or_else(|| "no config directory is available for OAuth token storage".to_string())?;
    write_oauth_token_file(&path, token)
}

fn oauth_token_path(server: &str) -> Option<std::path::PathBuf> {
    let dir = crate::config::get_config_dir()?;
    Some(oauth_token_path_in_dir(&dir, server))
}

async fn post_oauth_form(
    http: &reqwest::Client,
    endpoint: &str,
    params: &[(&str, String)],
) -> Result<Value, String> {
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
    if !resp.status().is_success() {
        let status = resp.status();
        let snippet = truncate_snippet(&resp.text().await.unwrap_or_default(), 300);
        return Err(format!(
            "OAuth token request failed (HTTP {status}): {snippet}"
        ));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| format!("OAuth token response was not JSON: {e}"))
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
        let mut process = Command::new(&command);
        if let Some(workspace) = workspace {
            process.current_dir(workspace);
        }
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
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Ok(msg) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        let mut pend = pending_clone.lock().await;
                        if let Some(sender) = pend.remove(&id) {
                            let _ = sender.send(msg);
                        }
                    }
                } else {
                    crate::dbg_log!(
                        "[mcp] ignoring non-JSON line from server: {}",
                        if line.len() > 100 {
                            &line[..100]
                        } else {
                            &line
                        }
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
                })),
            },
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
            tools: Arc::new(StdMutex::new(Vec::new())),
            child: Arc::new(Mutex::new(None)),
            stderr_diagnostics: Arc::new(StdMutex::new(Vec::new())),
            stderr_finished: Arc::new(Notify::new()),
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

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        if let Err(error) = tx.send(req).await {
            self.pending.lock().await.remove(&id);
            return Err(format!("Failed to send request: {error}"));
        }

        let resp = match tokio::time::timeout(Duration::from_secs(30), rx).await {
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
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err("MCP request timed out".to_string());
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
            let request = Self::build_remote_post(&snapshot, body);
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
                match self.ensure_remote_auth().await {
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
            let snippet = truncate_snippet(&resp.text().await.unwrap_or_default(), 300);
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
        if content_type.contains("text/event-stream") {
            let text = resp
                .text()
                .await
                .map_err(|e| format!("MCP server '{name}' SSE read failed: {e}"))?;
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
        let text = resp
            .text()
            .await
            .map_err(|e| format!("MCP server '{name}' response read failed: {e}"))?;
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
    async fn ensure_remote_auth(&self) -> Result<bool, String> {
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
        if std::env::var("RUSTCODE_MCP_NO_BROWSER").is_ok_and(|v| !v.trim().is_empty()) {
            return Ok(false);
        }
        match self.run_oauth_flow(&http).await {
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

    /// Full OAuth 2.1 browser flow: protected-resource discovery, server
    /// metadata, dynamic client registration, loopback authorization code,
    /// and code exchange. Tokens are returned to the caller for durable
    /// storage outside `config.toml`.
    async fn run_oauth_flow(&self, http: &reqwest::Client) -> Result<OAuthToken, String> {
        let name = self.name.clone();
        let mcp_url = match &self.transport {
            Transport::Remote { state } => state.lock().await.url.clone(),
            Transport::Stdio { .. } => return Err("MCP client is not a remote client".to_string()),
        };
        let probe = http
            .get(&mcp_url)
            .header("Accept", "application/json, text/event-stream")
            .send()
            .await
            .map_err(|e| remote_connect_error(&name, &mcp_url, &e))?;
        let www_auth = probe
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        let mut auth_server_bases: Vec<String> = Vec::new();
        if let Some(meta_url) = parse_challenge_param(&www_auth, "resource_metadata")
            && let Ok(meta_resp) = http.get(&meta_url).send().await
            && meta_resp.status().is_success()
            && let Ok(meta) = meta_resp.json::<Value>().await
            && let Some(servers) = meta.get("authorization_servers").and_then(Value::as_array)
        {
            auth_server_bases.extend(servers.iter().filter_map(Value::as_str).map(str::to_string));
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

        let mut client_id: Option<String> = None;
        if let Some(registration_endpoint) = registration_endpoint {
            let registration = json!({
                "redirect_uris": [redirect_uri],
                "client_name": "rustcode",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            });
            if let Ok(resp) = http
                .post(&registration_endpoint)
                .json(&registration)
                .send()
                .await
                && resp.status().is_success()
                && let Ok(body) = resp.json::<Value>().await
                && let Some(id) = body.get("client_id").and_then(Value::as_str)
            {
                client_id = Some(id.to_string());
            }
        }

        let (verifier, challenge) = pkce_pair();
        let flow_state = format!("{:032x}", rand::random::<u128>());
        let mut auth_url = format!(
            "{authorization_endpoint}?response_type=code&code_challenge={challenge}\
             &code_challenge_method=S256&redirect_uri={}&state={flow_state}",
            urlencoding::encode(&redirect_uri),
        );
        if let Some(id) = client_id.as_deref() {
            auth_url.push_str(&format!("&client_id={}", urlencoding::encode(id)));
        }
        auth_url.push_str(&format!("&resource={}", urlencoding::encode(&mcp_url)));

        eprintln!(
            "[mcp] OAuth login required for '{name}': open this URL in your browser:\n{auth_url}"
        );
        open_browser(&auth_url);

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

/// Wait for the OAuth authorization server to redirect back to the loopback
/// listener, and return the `(code, state)` query pair.
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
    let query = request_line
        .split_whitespace()
        .nth(1)
        .and_then(|target| target.split_once('?'))
        .map(|(_, query)| query)
        .unwrap_or_default();
    let mut code: Option<String> = None;
    let mut state: Option<String> = None;
    for pair in query.split('&') {
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
    if server.is_remote() {
        let url = server.url.clone().unwrap_or_default();
        let headers = server.headers.clone();
        let name = name.clone();
        return tokio::time::timeout(
            Duration::from_secs(10),
            McpClient::start_remote(name.clone(), url, headers),
        )
        .await
        .map_err(|_| format!("MCP server '{name}' startup timed out"))?;
    }
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
    .map_err(|_| format!("MCP server '{name}' startup timed out"))?
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

    if let Some(srv_config) = config {
        if !srv_config.enabled {
            return Ok(());
        }
        srv_config.validate()?;
        shutdown_server(name).await;

        let client = if srv_config.is_remote() {
            McpClient::start_remote(
                srv_config.name.clone(),
                srv_config.url.clone().unwrap_or_default(),
                srv_config.headers.clone(),
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
        if let Ok(mut reg) = get_mcp_registry().lock() {
            reg.insert(name.to_string(), client);
        }
        bump_mcp_generation();
    }
    Ok(())
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
mod tests {
    use super::*;

    fn stdio_server(name: &str) -> crate::config::McpServerConfig {
        crate::config::McpServerConfig {
            name: name.to_string(),
            command: "not-used".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            enabled: true,
            always_include: false,
        }
    }

    #[tokio::test]
    async fn startup_helper_visits_enabled_servers_and_continues_after_failure() {
        let servers = vec![
            crate::config::McpServerConfig {
                name: "enabled-one".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                url: None,
                headers: HashMap::new(),
                enabled: true,
                always_include: false,
            },
            crate::config::McpServerConfig {
                name: "disabled".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                url: None,
                headers: HashMap::new(),
                enabled: false,
                always_include: false,
            },
            crate::config::McpServerConfig {
                name: "enabled-two".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                url: None,
                headers: HashMap::new(),
                enabled: true,
                always_include: false,
            },
        ];
        let started = Arc::new(StdMutex::new(Vec::new()));
        let observed = Arc::clone(&started);

        let warnings = start_enabled_servers(&servers, move |name| {
            let observed = Arc::clone(&observed);
            async move {
                observed.lock().unwrap().push(name.clone());
                if name == "enabled-one" {
                    Err("injected startup failure".to_string())
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert_eq!(
            started.lock().unwrap().as_slice(),
            ["enabled-one", "enabled-two"]
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("enabled-one"));
    }

    #[tokio::test]
    async fn startup_helper_does_not_wait_forever_for_a_hanging_server() {
        let servers = vec![
            crate::config::McpServerConfig {
                name: "hanging".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                url: None,
                headers: HashMap::new(),
                enabled: true,
                always_include: false,
            },
            crate::config::McpServerConfig {
                name: "reachable".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                url: None,
                headers: HashMap::new(),
                enabled: true,
                always_include: false,
            },
        ];
        let started = Arc::new(StdMutex::new(Vec::new()));
        let observed = Arc::clone(&started);

        let completed = tokio::time::timeout(
            Duration::from_millis(100),
            start_enabled_servers_with_timeout(&servers, Duration::from_millis(10), move |name| {
                let observed = Arc::clone(&observed);
                async move {
                    observed.lock().unwrap().push(name.clone());
                    if name == "hanging" {
                        std::future::pending().await
                    } else {
                        Ok(())
                    }
                }
            }),
        )
        .await;

        assert!(completed.is_ok(), "startup must be bounded per server");
        let warnings = completed.unwrap();
        assert_eq!(started.lock().unwrap().as_slice(), ["hanging", "reachable"]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hanging"));
    }

    #[tokio::test]
    async fn test_mcp_client_handshake() {
        // A simple mock process that responds to 'initialize' request and 'tools/list' request
        let script = "read line; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1.0.0\"}}}'; read line; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"test_tool\",\"description\":\"a test tool\",\"inputSchema\":{}}]}}'";
        let client = McpClient::start(
            "mock_server".to_string(),
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
            HashMap::new(),
        )
        .await;

        assert!(client.is_ok());
        let client = client.unwrap();

        assert_eq!(client.name, "mock_server");
        let tools = client.get_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].get("name").unwrap().as_str().unwrap(), "test_tool");

        client.shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_client_passes_configured_environment() {
        let script = "test \"$MCP_TEST_ENV\" = forwarded || exit 3; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1.0.0\"}}}'; read line; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[]}}'";
        let mut env = HashMap::new();
        env.insert("MCP_TEST_ENV".to_string(), "forwarded".to_string());

        let client = McpClient::start(
            "env_server".to_string(),
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
            env,
        )
        .await;

        assert!(client.is_ok());
        client.unwrap().shutdown().await;
    }

    #[tokio::test]
    async fn test_mcp_client_reports_server_exit_without_waiting_for_request_timeout() {
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            McpClient::start(
                "exited_server".to_string(),
                "sh".to_string(),
                vec!["-c".to_string(), "exit 0".to_string()],
                HashMap::new(),
            ),
        )
        .await
        .expect("server exit should be observed promptly");

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("an exited MCP server cannot complete startup"),
        };
        assert!(error.contains("Server closed connection"));
    }

    #[tokio::test]
    async fn test_mcp_client_includes_server_stderr_when_startup_fails() {
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            McpClient::start(
                "diagnostic_server".to_string(),
                "sh".to_string(),
                vec![
                    "-c".to_string(),
                    "echo 'IMAP_USER not set' >&2; exit 1".to_string(),
                ],
                HashMap::new(),
            ),
        )
        .await
        .expect("server startup failure should be observed promptly");

        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("a server that exits cannot complete startup"),
        };
        assert!(error.contains("Server closed connection"));
        assert!(error.contains("IMAP_USER not set"));
    }

    #[test]
    fn owned_server_rejects_entries_without_a_transport() {
        let mut server = stdio_server("half-configured");
        server.command = String::new();
        let error = server
            .validate()
            .expect_err("neither command nor url must fail");
        assert!(error.contains("half-configured"));
        assert!(error.contains("command"));
        assert!(error.contains("url"));
    }

    #[test]
    fn remote_entries_validate_and_win_over_command() {
        let mut server = stdio_server("vercel");
        server.url = Some("https://mcp.vercel.com".to_string());
        assert!(server.is_remote());
        assert!(server.validate().is_ok());
        assert!(!stdio_server("plain").is_remote());
    }

    #[test]
    fn oauth_tokens_round_trip_through_durable_storage() {
        let dir = tempfile::tempdir().expect("oauth token dir");
        let token = OAuthToken {
            access_token: "access-123".to_string(),
            refresh_token: Some("refresh-456".to_string()),
            client_id: Some("client-789".to_string()),
            token_endpoint: Some("https://auth.example.com/token".to_string()),
            expires_at: Some(now_unix_secs() + 3600),
        };
        save_oauth_token_to_dir(dir.path(), "vercel", &token).expect("save token");
        let path = oauth_token_path_in_dir(dir.path(), "vercel");
        assert!(path.ends_with("mcp-oauth/vercel.json"));
        let loaded = load_oauth_token_from_dir(dir.path(), "vercel").expect("load token");
        assert_eq!(loaded, token);
        assert!(loaded.usable());

        let expired = OAuthToken {
            expires_at: Some(now_unix_secs().saturating_sub(10)),
            ..token
        };
        assert!(!expired.usable());
    }

    #[derive(Clone, Copy)]
    enum MockHttpMode {
        Json,
        Sse,
        Unauthorized,
        Oversized,
        /// Legacy SSE server: the Streamable HTTP endpoint 404s, `GET /mcp`
        /// yields an `event: endpoint` handshake, and JSON-RPC flows over
        /// the advertised `/rpc` path.
        LegacySse,
        /// Minimal OAuth token endpoint for refresh/exchange round-trips.
        TokenEndpoint,
    }

    fn find_headers_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|pos| pos + 4)
    }

    /// Minimal single-request-per-connection HTTP mock speaking just enough
    /// JSON-RPC to exercise the remote handshake, tool listing, and calls.
    async fn spawn_mock_http_mcp(mode: MockHttpMode) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock MCP server");
        let url = format!("http://{}/mcp", listener.local_addr().expect("mock addr"));
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match socket.read(&mut chunk).await {
                            Ok(0) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if find_headers_end(&buf).is_some() || buf.len() > 65_536 {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let head_end = find_headers_end(&buf).unwrap_or(buf.len());
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let content_length = head
                        .lines()
                        .skip(1)
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut body = buf[head_end..].to_vec();
                    while body.len() < content_length {
                        match socket.read(&mut chunk).await {
                            Ok(0) => break,
                            Ok(n) => body.extend_from_slice(&chunk[..n]),
                            Err(_) => break,
                        }
                    }
                    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    let method = request
                        .get("method")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let mut request_line =
                        head.lines().next().unwrap_or_default().split_whitespace();
                    let http_method = request_line.next().unwrap_or_default();
                    let request_path = request_line.next().unwrap_or("/");

                    let (status, content_type, extra, payload) = match mode {
                        MockHttpMode::Unauthorized => (
                            401,
                            "application/json",
                            "WWW-Authenticate: Bearer error=\"invalid_token\"\r\n".to_string(),
                            json!({"error": "unauthorized"}).to_string(),
                        ),
                        MockHttpMode::TokenEndpoint => (
                            200,
                            "application/json",
                            String::new(),
                            json!({
                                "access_token": "refreshed-access",
                                "token_type": "Bearer",
                                "expires_in": 3600,
                            })
                            .to_string(),
                        ),
                        MockHttpMode::LegacySse if http_method == "GET" => (
                            200,
                            "text/event-stream",
                            String::new(),
                            "event: endpoint\ndata: /rpc\n\n".to_string(),
                        ),
                        MockHttpMode::LegacySse if request_path != "/rpc" => (
                            404,
                            "application/json",
                            String::new(),
                            json!({"error": "not found"}).to_string(),
                        ),
                        _ if id.is_null() => {
                            // JSON-RPC notifications get 202 with no body.
                            (202, "application/json", String::new(), String::new())
                        }
                        _ => {
                            let response = match method.as_str() {
                                "initialize" => json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "result": {
                                        "protocolVersion": "2024-11-05",
                                        "capabilities": {},
                                        "serverInfo": {"name": "mock-http", "version": "1.0.0"},
                                    },
                                }),
                                "tools/list" => json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "result": { "tools": mock_http_tools(mode) },
                                }),
                                "tools/call" => {
                                    let tool = request
                                        .get("params")
                                        .and_then(|p| p.get("name"))
                                        .and_then(Value::as_str)
                                        .unwrap_or("unknown");
                                    json!({
                                        "jsonrpc": "2.0", "id": id,
                                        "result": {
                                            "content": [
                                                {"type": "text", "text": format!("called {tool}")},
                                            ],
                                        },
                                    })
                                }
                                _ => json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "error": {"code": -32601, "message": "Method not found"},
                                }),
                            };
                            let body = response.to_string();
                            let (content_type, body) = match mode {
                                MockHttpMode::Sse => (
                                    "text/event-stream",
                                    format!("event: message\ndata: {body}\n\n"),
                                ),
                                _ => ("application/json", body),
                            };
                            (
                                200,
                                content_type,
                                "mcp-session-id: mock-session\r\n".to_string(),
                                body,
                            )
                        }
                    };
                    let reason = match status {
                        200 => "OK",
                        202 => "Accepted",
                        401 => "Unauthorized",
                        404 => "Not Found",
                        _ => "Error",
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
                         Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (url, handle)
    }

    fn mock_http_tools(mode: MockHttpMode) -> Vec<Value> {
        match mode {
            MockHttpMode::Oversized => {
                // Two tools whose combined schema bytes exceed the native
                // budget, so an always_include reservation must be rejected.
                let half = "x".repeat(crate::tools::schema::MAX_MCP_NATIVE_SCHEMA_BYTES / 2);
                vec![
                    json!({
                        "name": "big_first",
                        "description": format!("Remote tool {half}"),
                        "inputSchema": {"type": "object", "properties": {}},
                    }),
                    json!({
                        "name": "big_second",
                        "description": format!("Remote tool {half}"),
                        "inputSchema": {"type": "object", "properties": {}},
                    }),
                ]
            }
            _ => vec![json!({
                "name": "http_tool",
                "description": "a remote test tool",
                "inputSchema": {"type": "object", "properties": {}},
            })],
        }
    }

    #[tokio::test]
    async fn remote_http_happy_path_lists_and_calls_tools() {
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Json).await;
        let client = McpClient::start_remote("http-happy".to_string(), url, HashMap::new())
            .await
            .expect("remote server should start");
        let tools = client.get_tools().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(Value::as_str),
            Some("http_tool")
        );
        let result = client
            .call_tool("http_tool", json!({}))
            .await
            .expect("tool call");
        let text = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(text.contains("http_tool"), "unexpected result: {result}");
        client.shutdown().await;
    }

    #[tokio::test]
    async fn remote_http_sse_wrapped_responses_parse() {
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Sse).await;
        let client = McpClient::start_remote("http-sse".to_string(), url, HashMap::new())
            .await
            .expect("SSE-wrapped server should start");
        let tools = client.get_tools().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(Value::as_str),
            Some("http_tool")
        );
        client.shutdown().await;
    }

    #[tokio::test]
    async fn remote_http_legacy_sse_fallback_lists_and_calls_tools() {
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::LegacySse).await;
        let client = McpClient::start_remote("http-legacy".to_string(), url, HashMap::new())
            .await
            .expect("legacy SSE server should start via fallback");
        let tools = client.get_tools().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("name").and_then(Value::as_str),
            Some("http_tool")
        );
        let result = client
            .call_tool("http_tool", json!({}))
            .await
            .expect("tool call over legacy endpoint");
        let text = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(text.contains("http_tool"), "unexpected result: {result}");
        client.shutdown().await;
    }

    #[tokio::test]
    async fn oauth_refresh_exchanges_a_new_access_token() {
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::TokenEndpoint).await;
        let http = reqwest::Client::new();
        let token = refresh_access_token(&http, &url, Some("client-1"), "old-refresh")
            .await
            .expect("refresh should succeed");
        assert_eq!(token.access_token, "refreshed-access");
        // The mock omits `refresh_token`, so the previous one is preserved
        // for the next refresh.
        assert_eq!(token.refresh_token.as_deref(), Some("old-refresh"));
        assert_eq!(token.client_id.as_deref(), Some("client-1"));
        assert_eq!(token.token_endpoint.as_deref(), Some(url.as_str()));
        assert!(token.usable());
    }

    #[test]
    fn sse_endpoint_helpers_parse_and_join() {
        let stream = ": keep-alive\n\nevent: message\ndata: {\"id\":1}\n\nevent: endpoint\ndata: /rpc?session=abc\n\n";
        assert_eq!(
            parse_sse_endpoint_event(stream).as_deref(),
            Some("/rpc?session=abc")
        );
        assert_eq!(
            parse_sse_endpoint_event("event: message\ndata: {}\n\n"),
            None
        );
        assert_eq!(
            join_endpoint("http://127.0.0.1:9/mcp", "/rpc?session=abc"),
            "http://127.0.0.1:9/rpc?session=abc"
        );
        assert_eq!(
            join_endpoint("http://127.0.0.1:9/mcp", "https://other.example/rpc"),
            "https://other.example/rpc"
        );
        assert_eq!(
            join_endpoint("http://127.0.0.1:9/mcp/", "rpc"),
            "http://127.0.0.1:9/mcp/rpc"
        );
        assert_eq!(
            http_origin("https://mcp.sentry.dev/mcp?x=1"),
            "https://mcp.sentry.dev"
        );
        assert_eq!(
            parse_challenge_param(
                "Bearer error=\"invalid_token\", resource_metadata=\"https://example.com/.well-known/oauth-protected-resource\"",
                "resource_metadata"
            )
            .as_deref(),
            Some("https://example.com/.well-known/oauth-protected-resource")
        );
        let candidates =
            oauth_server_metadata_urls("https://auth.example.com", "https://mcp.example.com/mcp");
        assert!(candidates[0].contains("oauth-authorization-server"));
        assert!(
            candidates
                .iter()
                .any(|u| u.contains("openid-configuration"))
        );
    }

    #[tokio::test]
    async fn remote_http_connection_failure_names_the_server() {
        let result = McpClient::start_remote(
            "http-down".to_string(),
            "http://127.0.0.1:9/mcp".to_string(),
            HashMap::new(),
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("unreachable server must fail"),
        };
        assert!(error.contains("http-down"), "unexpected error: {error}");
        assert!(
            error.to_lowercase().contains("connect"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn remote_http_auth_required_without_a_browser_flow() {
        // SAFETY: only the OAuth path in this module reads this variable, and
        // no other test in this module triggers an OAuth flow concurrently
        // with this test (all other remote tests use non-401 mocks).
        unsafe {
            std::env::set_var("RUSTCODE_MCP_NO_BROWSER", "1");
        }
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Unauthorized).await;
        let result = McpClient::start_remote("http-auth".to_string(), url, HashMap::new()).await;
        unsafe {
            std::env::remove_var("RUSTCODE_MCP_NO_BROWSER");
        }
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("401-only server must fail without a browser flow"),
        };
        assert!(error.contains("http-auth"), "unexpected error: {error}");
        assert!(
            error.to_lowercase().contains("authentication"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn remote_http_schema_budget_overflow_rejects_the_reservation() {
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Oversized).await;
        let client = McpClient::start_remote("http-big".to_string(), url, HashMap::new())
            .await
            .expect("oversized remote server should still start");
        let tools = client.get_tools().expect("tools");
        assert_eq!(tools.len(), 2);
        // Flatten exactly the way the native schema builder consumes live
        // registry tools, then run the real reservation/budget selection.
        let flattened: Vec<(String, String, Value)> = tools
            .iter()
            .map(|tool| {
                (
                    tool.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    tool.get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    tool.get("inputSchema").cloned().unwrap_or(json!({})),
                )
            })
            .collect();
        let owners = vec![client.name.clone(), client.name.clone()];
        let messages = vec![json!({"role": "user", "content": "use the big tools"})];
        let (selected, stats) =
            crate::tools::schema::select_mcp_tools_for_context_with_sticky_and_reservations_in_phase(
                &flattened,
                &owners,
                std::slice::from_ref(&client.name),
                &messages,
                &[],
                crate::tools::schema::ToolSchemaPhase::Established,
            );
        assert!(selected.is_empty());
        assert_eq!(stats.rejected_reservations.len(), 1);
        assert_eq!(stats.rejected_reservations[0], client.name);
        assert!(stats.schema_budget_exhausted);
        assert!(stats.mcp_schema_bytes <= stats.mcp_schema_budget_bytes);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn owned_server_refuses_disabled_remote_servers_without_network_use() {
        let workspace = tempfile::tempdir().expect("workspace");
        let server = crate::config::McpServerConfig {
            name: "http-off".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            headers: HashMap::new(),
            enabled: false,
            always_include: false,
        };
        let result = start_owned_server(&server, workspace.path()).await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("disabled server must not start"),
        };
        assert!(error.contains("disabled"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn owned_server_starts_remote_servers_within_startup_timeout() {
        let workspace = tempfile::tempdir().expect("workspace");
        let (url, _server) = spawn_mock_http_mcp(MockHttpMode::Json).await;
        let server = crate::config::McpServerConfig {
            name: "http-owned".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url),
            headers: HashMap::new(),
            enabled: true,
            always_include: false,
        };
        let result = start_owned_server(&server, workspace.path()).await;
        let client = match result {
            Ok(client) => client,
            Err(error) => panic!("owned remote server should start: {error}"),
        };
        assert_eq!(client.get_tools().expect("tools").len(), 1);
        client.shutdown().await;
    }
}
