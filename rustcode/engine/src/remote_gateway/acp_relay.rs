//! A desktop's restricted ACP connection to the per-user server.
use super::gateway::MAX_FRAME_BYTES;
use super::handshake::ErrorCode;
use super::router::ConnectionControl;
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

type Socket = WebSocketStream<TcpStream>;
const OUTBOUND_FRAMES: usize = 1024;
const OUTBOUND_BYTES: usize = 16 * 1024 * 1024;

// The permit travels with each frame, covering queued and in-flight writes.
struct OutboundFrame {
    text: String,
    _bytes: tokio::sync::OwnedSemaphorePermit,
}

struct OutboundQueue {
    sender: tokio::sync::mpsc::Sender<OutboundFrame>,
    bytes: Arc<tokio::sync::Semaphore>,
    control: Arc<ConnectionControl>,
}

impl OutboundQueue {
    fn new(control: Arc<ConnectionControl>) -> (Self, tokio::sync::mpsc::Receiver<OutboundFrame>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(OUTBOUND_FRAMES);
        (
            Self {
                sender,
                bytes: Arc::new(tokio::sync::Semaphore::new(OUTBOUND_BYTES)),
                control,
            },
            receiver,
        )
    }

    fn send(&self, text: String) -> Result<(), ErrorCode> {
        let permit = u32::try_from(text.len())
            .ok()
            .and_then(|length| Arc::clone(&self.bytes).try_acquire_many_owned(length).ok());
        if let Some(permit) = permit {
            if self
                .sender
                .try_send(OutboundFrame {
                    text,
                    _bytes: permit,
                })
                .is_ok()
            {
                return Ok(());
            }
        }
        self.control.close(ErrorCode::SlowConsumer);
        Err(ErrorCode::SlowConsumer)
    }
}

const MAX_PENDING_REQUESTS: usize = 1024;

#[derive(Default)]
struct PendingRequests(Mutex<HashSet<String>>);

impl PendingRequests {
    fn remember(&self, frame: &str) -> Result<(), ErrorCode> {
        let Ok(value) = serde_json::from_str::<Value>(frame) else {
            return Ok(());
        };
        if value.get("method").and_then(Value::as_str).is_none() {
            return Ok(());
        }
        let Some(id) = value
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
        else {
            return Ok(());
        };
        let mut pending = self.0.lock().unwrap();
        if pending.len() >= MAX_PENDING_REQUESTS {
            return Err(ErrorCode::SlowConsumer);
        }
        pending.insert(id.to_string());
        Ok(())
    }

    fn answer(&self, id: &Value) -> bool {
        self.0.lock().unwrap().remove(&id.to_string())
    }
}

const EXACT_METHODS: &[&str] = &[
    "initialize",
    "_rustcode/session/status",
    "_rustcode/session/snooze",
    "_rustcode/session/repair",
    "_rustcode/harnesses",
    "_rustcode/harnesses/default",
    "_rustcode/harnesses/set",
    "_rustcode/settings/get",
    "_rustcode/settings/set",
    "_rustcode/skills/add",
    "_rustcode/skills/get",
];
const METHOD_PREFIXES: &[&str] = &["session/", "_rustcode/pulls", "_rustcode/capabilities"];

fn allowed(method: &str) -> bool {
    EXACT_METHODS.contains(&method)
        || METHOD_PREFIXES
            .iter()
            .any(|prefix| method.starts_with(prefix))
}

/// Connect to the first engine that answers, and relay between it and the
/// device. A description left behind by an engine that has since ended is
/// logged and skipped, then the next endpoint is tried. The caller keeps
/// revocation and gateway shutdown cancellation active for the lifetime of
/// this future.
pub(super) async fn relay(
    endpoints: &[String],
    writer: &mut SplitSink<Socket, Message>,
    reader: &mut SplitStream<Socket>,
    control: Arc<ConnectionControl>,
    write_timeout: Duration,
    ping_interval: Duration,
    idle_timeout: Duration,
) -> Option<ErrorCode> {
    let Some(stream) = connect_first(endpoints, write_timeout).await else {
        eprintln!(
            "remote ACP: no local RustCode engine accepted a connection (tried {})",
            endpoints.join(", ")
        );
        return Some(ErrorCode::Internal);
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
    let (sink, mut queue) = OutboundQueue::new(control);
    let pending = PendingRequests::default();
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
            if let Err(code) = pending.remember(&text) {
                return Some(code);
            }
            if sink
                .send(text.trim_end_matches(['\r', '\n']).to_owned())
                .is_err()
            {
                return Some(ErrorCode::SlowConsumer);
            }
        }
    };
    let consume = async {
        let mut ping =
            tokio::time::interval_at(tokio::time::Instant::now() + ping_interval, ping_interval);
        let mut last_heard = tokio::time::Instant::now();
        loop {
            tokio::select! {
                frame = queue.recv() => {
                    let OutboundFrame { text, _bytes } = frame?;
                    if !matches!(tokio::time::timeout(write_timeout, writer.send(Message::text(text))).await, Ok(Ok(()))) { return None; }
                }
                _ = ping.tick() => {
                    if last_heard.elapsed() >= idle_timeout { return Some(ErrorCode::IdleTimeout); }
                    if !matches!(tokio::time::timeout(write_timeout, writer.send(Message::Ping(Default::default()))).await, Ok(Ok(()))) { return None; }
                }
                message = reader.next() => {
                    last_heard = tokio::time::Instant::now();
                    match message {
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
                        if value.get("method").is_none() && !pending.answer(&value["id"]) {
                            continue;
                        }
                        // Forward the same parsed object that was authorized, avoiding
                        // duplicate-key parser differences. Resume metadata is opaque.
                        let line = value.to_string() + "\n";
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
        }
    };
    tokio::select! { result = produce => result, result = consume => result }
}

/// The first endpoint that accepts a connection. Which endpoints exist is
/// only a description (`server.json`, `engine.json`); connecting tells which
/// engine is really running.
async fn connect_first(
    endpoints: &[String],
    write_timeout: Duration,
) -> Option<crate::server::listen::ClientStream> {
    for endpoint in endpoints {
        match tokio::time::timeout(write_timeout, crate::server::listen::connect(endpoint)).await {
            Ok(Ok(stream)) => {
                crate::dbg_log!("remote ACP: relaying to {endpoint}");
                return Some(stream);
            }
            Ok(Err(error)) => {
                crate::dbg_log!("remote ACP: {endpoint} did not accept a connection: {error}")
            }
            Err(_) => crate::dbg_log!(
                "remote ACP: {endpoint} did not accept a connection within {write_timeout:?}"
            ),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn outbound_queue_caps_bytes_and_releases_written_frames() {
        let control = Arc::new(ConnectionControl::new(
            tokio_util::sync::CancellationToken::new(),
        ));
        let (sink, mut queue) = OutboundQueue::new(Arc::clone(&control));
        for _ in 0..16 {
            sink.send("x".repeat(1024 * 1024)).unwrap();
        }
        assert_eq!(sink.bytes.available_permits(), 0);
        let frame = queue.recv().await.unwrap();
        assert_eq!(
            sink.bytes.available_permits(),
            0,
            "in-flight frame still counts"
        );
        drop(frame);
        sink.send("x".repeat(1024 * 1024)).unwrap();
        assert_eq!(sink.send("x".into()), Err(ErrorCode::SlowConsumer));
        assert!(control.cancel.is_cancelled());
        assert_eq!(control.reason(), ErrorCode::SlowConsumer);
    }

    #[test]
    fn server_request_ids_are_bounded_and_answers_are_single_use() {
        let pending = PendingRequests::default();
        pending
            .remember(r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission"}"#)
            .unwrap();
        assert!(!pending.answer(&json!("7")));
        assert!(!pending.answer(&json!(99)));
        assert!(pending.answer(&json!(7)));
        assert!(!pending.answer(&json!(7)));
        for id in 0..MAX_PENDING_REQUESTS {
            pending
                .remember(&json!({"id":id,"method":"session/request_permission"}).to_string())
                .unwrap();
        }
        assert_eq!(
            pending.remember(r#"{"id":"overflow","method":"session/request_permission"}"#),
            Err(ErrorCode::SlowConsumer)
        );
    }

    #[test]
    fn desktop_allowlist_contract() {
        assert_eq!(
            EXACT_METHODS,
            &[
                "initialize",
                "_rustcode/session/status",
                "_rustcode/session/snooze",
                "_rustcode/session/repair",
                "_rustcode/harnesses",
                "_rustcode/harnesses/default",
                "_rustcode/harnesses/set",
                "_rustcode/settings/get",
                "_rustcode/settings/set",
                "_rustcode/skills/add",
                "_rustcode/skills/get",
            ]
        );
        assert_eq!(
            METHOD_PREFIXES,
            &["session/", "_rustcode/pulls", "_rustcode/capabilities"]
        );
        for method in [
            "initialize",
            "_rustcode/session/status",
            "_rustcode/session/snooze",
            "_rustcode/session/repair",
            "session/load",
            "session/new",
            "session/prompt",
            "session/cancel",
            "session/future",
            "_rustcode/pulls",
            "_rustcode/pulls/list",
            "_rustcode/pullsFuture",
            "_rustcode/harnesses",
            "_rustcode/harnesses/default",
            "_rustcode/harnesses/set",
            "_rustcode/settings/get",
            "_rustcode/settings/set",
            "_rustcode/skills/add",
            "_rustcode/skills/get",
            "_rustcode/capabilities",
            "_rustcode/capabilities/set",
            "_rustcode/capabilities/mcp/add",
            "_rustcode/capabilities/mcp/remove",
            "_rustcode/capabilities/mcp/restart",
        ] {
            assert!(allowed(method), "{method}");
        }
        for method in [
            "_rustcode/settings/changed",
            "_rustcode/settings/reset",
            "_rustcode/threads/changes",
            "_rustcode/createOptions",
            "_rustcode/pending",
            "_rustcode/thread/",
            "_rustcode/queue/",
            "_rustcode/thread/get",
            "_rustcode/queue/add",
            "",
            "session",
            "initialize/x",
            "_rustcode/thread",
            "_rustcode/queue",
            "_rustcode/settings",
            "_rustcode/threads/list",
            "_rustcode/harnesses/accounts/add",
            "_rustcode/harnesses/accounts/remove",
            "_rustcode/harnesses/remove",
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
