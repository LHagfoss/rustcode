//! A desktop's restricted ACP connection to the per-user server.
use super::gateway::MAX_FRAME_BYTES;
use super::handshake::ErrorCode;
use super::router::{ConnectionControl, FrameSink};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<TcpStream>;
const OUTBOUND_FRAMES: usize = 1024;

const EXACT_METHODS: &[&str] = &[
    "initialize", "_rustcode/settings/get", "_rustcode/settings/set", "_rustcode/settings/changed",
    "_rustcode/threads/changes", "_rustcode/harnesses", "_rustcode/createOptions", "_rustcode/pending",
];
const METHOD_PREFIXES: &[&str] = &["session/", "_rustcode/thread/", "_rustcode/queue/", "_rustcode/pulls"];

fn allowed(method: &str) -> bool {
    EXACT_METHODS.contains(&method) || METHOD_PREFIXES.iter().any(|prefix| method.starts_with(prefix))
}

/// Connect only to an already running server. The caller keeps revocation and
/// gateway shutdown cancellation active for the lifetime of this future.
pub(super) async fn relay(
    endpoint: &str,
    writer: &mut SplitSink<Socket, Message>,
    reader: &mut SplitStream<Socket>,
    control: Arc<ConnectionControl>,
    write_timeout: Duration,
) -> Option<ErrorCode> {
    let stream =
        match tokio::time::timeout(write_timeout, crate::server::listen::connect(endpoint)).await {
            Ok(Ok(stream)) => stream,
            _ => return Some(ErrorCode::Internal),
        };
    if !matches!(
        tokio::time::timeout(
            write_timeout,
            writer.send(Message::text(
                serde_json::to_string(&crate::remote::protocol::AcpServerFrame::AcpOpened).unwrap()
            ))
        )
        .await,
        Ok(Ok(()))
    ) {
        return None;
    }
    let (server_read, mut server_write) = tokio::io::split(stream);
    let (sink, mut queue) = FrameSink::new(OUTBOUND_FRAMES, control);
    let produce = async {
        let mut server = BufReader::new(server_read);
        loop {
            let mut bytes = Vec::new();
            let read = (&mut server)
                .take((MAX_FRAME_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)
                .await;
            match read {
                Ok(0) => return None,
                Ok(_) if bytes.len() > MAX_FRAME_BYTES => return Some(ErrorCode::FrameTooLarge),
                Err(_) => return None,
                _ => {}
            }
            let Ok(text) = String::from_utf8(bytes) else {
                return Some(ErrorCode::InvalidFrame);
            };
            if sink
                .send(text.trim_end_matches(['\r', '\n']).to_owned())
                .is_err()
            {
                return Some(ErrorCode::SlowConsumer);
            }
        }
    };
    let consume = async {
        loop {
            tokio::select! {
                frame = queue.recv() => {
                    let frame = frame?;
                    if !matches!(tokio::time::timeout(write_timeout, writer.send(Message::text(frame))).await, Ok(Ok(()))) { return None; }
                }
                message = reader.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(value) = serde_json::from_str::<Value>(&text) else { return Some(ErrorCode::InvalidFrame); };
                        if !value.is_object() || value["jsonrpc"] != "2.0" { return Some(ErrorCode::InvalidFrame); }
                        if let Some(method) = value.get("method") {
                            if !method.as_str().is_some_and(allowed) {
                                if let Some(id) = value.get("id") {
                                    let error = json!({"jsonrpc":"2.0", "id":id, "error":{
                                        "code":-32601,"message":"method is not permitted over remote ACP",
                                        "data":{"code":"unauthorized_access"}}});
                                    if !matches!(tokio::time::timeout(write_timeout, writer.send(Message::text(error.to_string()))).await, Ok(Ok(()))) { return None; }
                                }
                                continue;
                            }
                        } else if value.get("id").is_none() || (value.get("result").is_none() && value.get("error").is_none()) {
                            return Some(ErrorCode::InvalidFrame);
                        }
                        // Remove only JSON whitespace line breaks; escaped newlines in
                        // strings and the resume cursor payload remain untouched.
                        let line = text.replace(['\r', '\n'], "") + "\n";
                        if !matches!(tokio::time::timeout(write_timeout, server_write.write_all(line.as_bytes())).await, Ok(Ok(()))) { return None; }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if !matches!(tokio::time::timeout(write_timeout, writer.send(Message::Pong(payload))).await, Ok(Ok(()))) { return None; }
                    }
                    Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Binary(_))) => return Some(ErrorCode::InvalidFrame),
                    _ => return None,
                }
            }
        }
    };
    tokio::select! { result = produce => result, result = consume => result }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_allowlist_contract() {
        assert_eq!(EXACT_METHODS, &["initialize", "_rustcode/settings/get", "_rustcode/settings/set",
            "_rustcode/settings/changed", "_rustcode/threads/changes", "_rustcode/harnesses",
            "_rustcode/createOptions", "_rustcode/pending"]);
        assert_eq!(METHOD_PREFIXES, &["session/", "_rustcode/thread/", "_rustcode/queue/", "_rustcode/pulls"]);
        for method in [
            "initialize",
            "session/load",
            "session/new",
            "session/prompt",
            "session/cancel",
            "session/future",
            "_rustcode/thread/get",
            "_rustcode/queue/add",
            "_rustcode/pulls",
            "_rustcode/pulls/list",
            "_rustcode/pullsFuture",
            "_rustcode/settings/get",
            "_rustcode/settings/set",
            "_rustcode/settings/changed",
            "_rustcode/threads/changes",
            "_rustcode/harnesses",
            "_rustcode/createOptions",
            "_rustcode/pending",
        ] {
            assert!(allowed(method), "{method}");
        }
        for method in [
            "",
            "session",
            "initialize/x",
            "_rustcode/thread",
            "_rustcode/queue",
            "_rustcode/settings",
            "_rustcode/settings/reset",
            "_rustcode/threads/list",
            "_rustcode/harnesses/set",
            "_rustcode/provider/credentials",
            "_rustcode/login",
            "_rustcode/remote/pair",
            "_rustcode/server/status",
            "_rustcode/server/stop",
            "_rustcode/mcp/set",
            "_rustcode/data/query",
            "_rustcode/unknown",
            "unknown",
        ] {
            assert!(!allowed(method), "{method}");
        }
    }
}
