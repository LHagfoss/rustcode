//! `rustcode mcp-orchestrator --session <id> --socket <path>` (token in
//! `RUSTCODE_ORCHESTRATOR_TOKEN`):
//! a stdio MCP server that offers an external harness the orchestration
//! tools (`docs/orchestrator.md`). It holds no state of its own: every
//! `tools/call` goes to the parent engine's [`super::hub`], which checks the
//! session and token and forwards it to the client like a built-in call.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::hub::{CallAnswer, CallRequest};

/// The MCP revisions this server speaks; it answers with the client's when
/// it is one of these.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Where calls go: the parent engine's socket, or a stand-in in tests.
pub trait Forward: Send + Sync + 'static {
    fn call(
        &self,
        request: CallRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>>;
}

/// The parent engine at `socket`.
pub struct SocketForward {
    pub socket: PathBuf,
}

impl Forward for SocketForward {
    fn call(
        &self,
        request: CallRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CallAnswer> + Send + '_>> {
        Box::pin(async move {
            #[cfg(unix)]
            {
                super::hub::call(&self.socket, &request).await
            }
            #[cfg(not(unix))]
            {
                let _ = request;
                CallAnswer {
                    success: false,
                    content: "error: the orchestrator needs a unix socket".to_owned(),
                }
            }
        })
    }
}

/// The session and token every call carries.
#[derive(Debug, Clone)]
pub struct Identity {
    pub session_id: String,
    pub token: String,
}

/// The tools the server lists, as MCP tool entries.
pub fn tools() -> Vec<Value> {
    super::tool_names(super::VERSION)
        .into_iter()
        .filter_map(|name| crate::tools::TOOLS.iter().find(|tool| tool.name == name))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
            })
        })
        .collect()
}

/// Run on stdin/stdout until the harness closes stdin.
pub async fn run(session_id: String, socket: PathBuf, token: String) -> Result<(), String> {
    serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        Identity { session_id, token },
        Arc::new(SocketForward { socket }),
    )
    .await
}

/// Serve newline-delimited JSON-RPC from `input`, answering on `output`.
pub async fn serve<R, W>(
    input: R,
    mut output: W,
    identity: Identity,
    forward: Arc<dyn Forward>,
) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sender, mut answers) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(message) = answers.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();
            line.push(b'\n');
            if output.write_all(&line).await.is_err() || output.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(input).lines();
    let mut calls = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            let _ = sender.send(error(Value::Null, -32700, "parse error"));
            continue;
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        // Notifications (no id) and answers to requests we never send.
        let Some(id) = id else {
            continue;
        };
        if method.is_empty() {
            continue;
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str);
                let version = asked
                    .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                let _ = sender.send(result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "rustcode-orchestrator",
                            "title": "RustCode threads",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": super::PROMPT_HINT_V3.trim(),
                    }),
                ));
            }
            "ping" => {
                let _ = sender.send(result(id, json!({})));
            }
            "tools/list" => {
                let _ = sender.send(result(id, json!({ "tools": tools() })));
            }
            "tools/call" => {
                let sender = sender.clone();
                let forward = Arc::clone(&forward);
                let identity = identity.clone();
                calls.spawn(async move {
                    let answer = call(forward.as_ref(), &identity, &params).await;
                    let _ = sender.send(result(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": answer.content }],
                            "isError": !answer.success,
                        }),
                    ));
                });
            }
            _ => {
                let _ = sender.send(error(id, -32601, "method not found"));
            }
        }
    }
    // stdin closed: the harness is done with us. Dropping the calls in
    // flight closes their sockets, which cancels them in the engine.
    calls.shutdown().await;
    drop(sender);
    let _ = writer.await;
    Ok(())
}

async fn call(forward: &dyn Forward, identity: &Identity, params: &Value) -> CallAnswer {
    let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
    if !super::tool_names(super::VERSION).contains(&tool) {
        return CallAnswer {
            success: false,
            content: format!("error: unknown tool '{tool}'"),
        };
    }
    let args = params
        .get("arguments")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    // Checked here too, so a bad call never leaves the harness's machine
    // without a clear answer.
    if let Err(reason) = super::validate(tool, &args) {
        return CallAnswer {
            success: false,
            content: format!("error: invalid {tool} call: {reason}. Nothing was sent."),
        };
    }
    forward
        .call(CallRequest {
            session_id: identity.session_id.clone(),
            token: identity.token.clone(),
            tool: tool.to_owned(),
            args,
        })
        .await
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::hub::Hub;
    use super::*;
    use crate::preview::{PreviewClient, PreviewFuture};
    use tokio::io::AsyncBufReadExt;

    /// The app: answers every call with what it was asked.
    struct EchoApp;

    impl PreviewClient for EchoApp {
        fn request(&self, params: Value) -> PreviewFuture {
            Box::pin(async move { Ok(json!({ "asked": params })) })
        }
    }

    async fn hub() -> (tempfile::TempDir, Arc<Hub>) {
        let dir = tempfile::tempdir().unwrap();
        let hub = Hub::bind(dir.path().join("run").join("orchestrator-test.sock"))
            .await
            .unwrap();
        (dir, hub)
    }

    /// Drive the server with `messages` and collect its answers by id; one
    /// answer is expected for each message that has an id.
    async fn exchange(identity: Identity, socket: PathBuf, messages: &[Value]) -> Vec<Value> {
        let expected = messages
            .iter()
            .filter(|message| message.get("id").is_some())
            .count();
        let (mut client, server_side) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_side);
        let server = tokio::spawn(serve(
            server_read,
            server_write,
            identity,
            Arc::new(SocketForward { socket }),
        ));
        let mut input = String::new();
        for message in messages {
            input.push_str(&message.to_string());
            input.push('\n');
        }
        client.write_all(input.as_bytes()).await.unwrap();
        let (read, mut write) = tokio::io::split(client);
        let mut lines = BufReader::new(read).lines();
        let mut answers = Vec::new();
        while answers.len() < expected {
            let line = lines.next_line().await.unwrap().expect("an answer");
            answers.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        write.shutdown().await.unwrap();
        server.await.unwrap().unwrap();
        answers.sort_by_key(|answer| answer["id"].as_i64());
        answers
    }

    #[tokio::test]
    async fn tools_list_and_call_round_trip_to_the_app() {
        let (_dir, hub) = hub().await;
        let token = hub.register("thread-1", Arc::new(EchoApp));
        let mode = std::fs::metadata(hub.path()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.permissions().mode() & 0o777, 0o600);

        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token,
            },
            hub.path().to_owned(),
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                       "params": {"protocolVersion": "2025-03-26", "capabilities": {}}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                       "params": {"name": "start_session", "arguments": {"prompt": "fix the login"}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                       "params": {"name": "fan_out", "arguments": {"prompt": "x", "harnesses": []}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}),
            ],
        )
        .await;
        assert_eq!(answers.len(), 5, "{answers:?}");
        assert_eq!(answers[0]["result"]["protocolVersion"], "2025-03-26");
        assert!(answers[0]["result"]["capabilities"]["tools"].is_object());
        let listed = answers[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            super::super::tool_names(super::super::VERSION)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
        assert!(answers[1]["result"]["tools"][0]["inputSchema"]["type"] == "object");

        let called = &answers[2]["result"];
        assert_eq!(called["isError"], false, "{called}");
        let text = called["content"][0]["text"].as_str().unwrap();
        let echoed: Value = serde_json::from_str(text).unwrap();
        // The same `_rustcode/orchestration` params a built-in call sends.
        assert_eq!(
            echoed["asked"],
            json!({"tool": "start_session", "args": {"prompt": "fix the login"}})
        );

        assert_eq!(answers[3]["result"]["isError"], true);
        assert!(
            answers[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Nothing was sent")
        );
        assert_eq!(answers[4]["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn a_wrong_token_or_session_is_refused() {
        let (_dir, hub) = hub().await;
        let token = hub.register("thread-1", Arc::new(EchoApp));
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                          "params": {"name": "list_sessions", "arguments": {}}});
        for identity in [
            Identity {
                session_id: "thread-1".into(),
                token: "not-the-token".into(),
            },
            Identity {
                session_id: "thread-2".into(),
                token: token.clone(),
            },
        ] {
            let answers = exchange(identity, hub.path().to_owned(), &[call.clone()]).await;
            assert_eq!(answers[0]["result"]["isError"], true);
            assert!(
                answers[0]["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("did not accept")
            );
        }
        // A released thread's token stops working.
        hub.unregister("thread-1");
        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token,
            },
            hub.path().to_owned(),
            &[call],
        )
        .await;
        assert_eq!(answers[0]["result"]["isError"], true);
    }

    #[tokio::test]
    async fn an_unreachable_engine_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let answers = exchange(
            Identity {
                session_id: "thread-1".into(),
                token: "t".into(),
            },
            dir.path().join("missing.sock"),
            &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": "list_sessions", "arguments": {}}})],
        )
        .await;
        assert!(
            answers[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not reachable")
        );
    }
}
