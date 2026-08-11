use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};

async fn model_response(mut stream: TcpStream, request_number: usize, cwd: &Path) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        if read == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let headers_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap()
        + 4;
    let headers = String::from_utf8_lossy(&request[..headers_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while request.len() < headers_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        if read == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..read]);
    }

    if request_number == 1 {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let body = if request_number == 0 {
        format!(
            "data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{{\"name\":\"list_directory\",\"arguments\":{}}}}}]}}}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\ndata: [DONE]\n\n",
            serde_json::to_string(&serde_json::json!({"path": cwd})).unwrap()
        )
    } else {
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

async fn start_model_server(listener: TcpListener, cwd: std::path::PathBuf) {
    for request_number in 0..2 {
        let (stream, _) = listener.accept().await.unwrap();
        model_response(stream, request_number, &cwd).await;
    }
}

async fn next_json_line(
    reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> serde_json::Value {
    let line = tokio::time::timeout(Duration::from_secs(5), reader.next_line())
        .await
        .expect("ACP response timed out")
        .expect("ACP stdout read failed")
        .expect("ACP exited before responding");
    serde_json::from_str(&line).expect("ACP emitted invalid JSON")
}

async fn send_json(child: &mut Child, value: serde_json::Value) {
    let stdin = child.stdin.as_mut().expect("ACP stdin missing");
    stdin
        .write_all(format!("{}\n", value).as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();
}

#[tokio::test]
async fn prompt_work_does_not_block_acp_event_loop_after_a_tool_call() {
    let config_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let model_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(start_model_server(listener, cwd.path().to_path_buf()));

    std::fs::write(
        config_dir.path().join("models.json"),
        serde_json::json!({
            "default": {"big": "test", "small": "test"},
            "models": [{
                "name": "test",
                "url": model_url,
                "model": "test",
                "tool_protocol": "apinative",
                "context_window": 8192
            }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        config_dir.path().join("config.json"),
        serde_json::json!({
            "tool_protocol": "apinative",
            "max_tool_rounds": 4,
            "mcp_servers": []
        })
        .to_string(),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_rustcode"))
        .arg("--acp")
        .env("RUSTCODE_CONFIG_DIR", config_dir.path())
        .current_dir(cwd.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap()).lines();

    send_json(
        &mut child,
        serde_json::json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"test","version":"0"}}
        }),
    )
    .await;
    assert_eq!(next_json_line(&mut reader).await["id"], 1);

    send_json(
        &mut child,
        serde_json::json!({
            "jsonrpc":"2.0", "id":2, "method":"session/new",
            "params":{"cwd":cwd.path(),"mcpServers":[]}
        }),
    )
    .await;
    let session = next_json_line(&mut reader).await;
    let session_id = session["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new failed: {session}"));

    send_json(
        &mut child,
        serde_json::json!({
            "jsonrpc":"2.0", "id":3, "method":"session/prompt",
            "params":{"sessionId":session_id,"prompt":[{"type":"text","text":"inspect the directory"}]}
        }),
    )
    .await;
    send_json(
        &mut child,
        serde_json::json!({
            "jsonrpc":"2.0", "id":4, "method":"initialize",
            "params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"test","version":"0"}}
        }),
    )
    .await;

    let line = tokio::time::timeout(Duration::from_millis(150), reader.next_line())
        .await
        .expect("ACP event loop was blocked by prompt work")
        .unwrap()
        .unwrap();
    let message: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        message["id"], 4,
        "the concurrent request must be serviced promptly"
    );

    child.kill().await.unwrap();
    let _ = child.wait().await;
    server.abort();
}
