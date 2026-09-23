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
    tx: mpsc::Sender<Value>,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    next_id: Arc<Mutex<i64>>,
    tools: Arc<StdMutex<Vec<Value>>>,
    child: Arc<Mutex<Option<Child>>>,
    stderr_diagnostics: Arc<StdMutex<Vec<String>>>,
    stderr_finished: Arc<Notify>,
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
            tx,
            pending,
            next_id,
            tools: Arc::clone(&tools),
            child: Arc::new(Mutex::new(Some(child))),
            stderr_diagnostics,
            stderr_finished,
        });

        // Handshake: initialize
        let _init_res = client
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
        let _ = client.notify("notifications/initialized", json!({}));

        // Fetch tools list
        let mut tools_list = Vec::new();
        if let Ok(tools_res) = client.call("tools/list", json!({})).await
            && let Some(tools_arr) = tools_res
                .get("result")
                .and_then(|r| r.get("tools"))
                .and_then(|t| t.as_array())
        {
            tools_list = tools_arr.clone();
        }

        // Store tools list
        {
            let mut t = tools.lock().map_err(|e| e.to_string())?;
            *t = tools_list;
        }

        Ok(client)
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = {
            let mut nid = self.next_id.lock().await;
            let current = *nid;
            *nid += 1;
            current
        };

        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pend = self.pending.lock().await;
            pend.insert(id, tx);
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        if let Err(error) = self.tx.send(req).await {
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
        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown server error");
            return Err(msg.to_string());
        }

        Ok(resp)
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
        let req = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(req).await;
        });
        Ok(())
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
    pub(crate) fn new() -> Self {
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
        let cfg = crate::config::load_config_for_workspace(&workspace).2;
        cfg.mcp_servers.iter().find(|s| s.name == name).cloned()
    };

    if let Some(srv_config) = config {
        if !srv_config.enabled {
            return Ok(());
        }
        shutdown_server(name).await;

        let client = McpClient::start_in_workspace(
            srv_config.name.clone(),
            srv_config.command,
            srv_config.args,
            srv_config.env,
            Some(workspace),
        )
        .await?;
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

    #[tokio::test]
    async fn startup_helper_visits_enabled_servers_and_continues_after_failure() {
        let servers = vec![
            crate::config::McpServerConfig {
                name: "enabled-one".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                enabled: true,
            },
            crate::config::McpServerConfig {
                name: "disabled".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                enabled: false,
            },
            crate::config::McpServerConfig {
                name: "enabled-two".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                enabled: true,
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
                enabled: true,
            },
            crate::config::McpServerConfig {
                name: "reachable".to_string(),
                command: "not-used".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                enabled: true,
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
}
