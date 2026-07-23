use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc};

#[allow(dead_code)]
pub struct McpClient {
    pub name: String,
    tx: mpsc::Sender<Value>,
    pending: Arc<Mutex<HashMap<i64, tokio::sync::oneshot::Sender<Value>>>>,
    next_id: Arc<Mutex<i64>>,
    tools: Arc<StdMutex<Vec<Value>>>,
    child: Arc<Mutex<Option<Child>>>,
}

pub fn get_mcp_registry() -> &'static StdMutex<HashMap<String, Arc<McpClient>>> {
    static REGISTRY: OnceLock<StdMutex<HashMap<String, Arc<McpClient>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(HashMap::new()))
}

impl McpClient {
    pub async fn start(
        name: String,
        command: String,
        args: Vec<String>,
    ) -> Result<Arc<Self>, String> {
        let mut child = Command::new(&command)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server {name}: {e}"))?;

        let stdin = child.stdin.take().ok_or("Failed to open stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to open stdout")?;

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
                if let Ok(msg) = serde_json::from_str::<Value>(&line)
                    && let Some(id) = msg.get("id").and_then(|i| i.as_i64()) {
                        let mut pend = pending_clone.lock().await;
                        if let Some(sender) = pend.remove(&id) {
                            let _ = sender.send(msg);
                        }
                    }
            }
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

        self.tx
            .send(req)
            .await
            .map_err(|e| format!("Failed to send request: {e}"))?;

        let resp = rx
            .await
            .map_err(|_| "Server closed connection before responding".to_string())?;
        if let Some(err) = resp.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown server error");
            return Err(msg.to_string());
        }

        Ok(resp)
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
    let config = {
        let cfg = crate::config::load_config().2;
        cfg.mcp_servers.iter().find(|s| s.name == name).cloned()
    };

    if let Some(srv_config) = config {
        if !srv_config.enabled {
            return Ok(());
        }
        shutdown_server(name).await;

        let client =
            McpClient::start(srv_config.name.clone(), srv_config.command, srv_config.args).await?;
        if let Ok(mut reg) = get_mcp_registry().lock() {
            reg.insert(name.to_string(), client);
        }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_client_handshake() {
        // A simple mock process that responds to 'initialize' request and 'tools/list' request
        let script = "read line; echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1.0.0\"}}}'; read line; read line; echo '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"test_tool\",\"description\":\"a test tool\",\"inputSchema\":{}}]}}'";
        let client = McpClient::start(
            "mock_server".to_string(),
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
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
}
