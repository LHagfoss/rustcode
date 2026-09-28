//! TCP server driving one controller session worker (issue #1443).
//!
//! One worker per server process (MVP scope). Each authed connection gets
//! the latest snapshot replayed, then a live feed of worker updates while
//! it sends requests. Connections are independent: a slow client never
//! blocks the worker or other clients (per-connection tasks + broadcast
//! fanout with a bounded buffer).

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast};

use crate::controller::{
    Command, ControllerEvent, ControllerHandle, ControllerUpdate, InteractiveController,
};
use crate::daemon::protocol::{ProtocolError, read_async_frame, write_async_frame};

use super::protocol::{SERVE_PROTOCOL_VERSION, ServeRequest, ServeResponse};

/// How long a connection may take to complete the auth handshake.
const AUTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Worker-event fanout buffer per server. Snapshots are the largest frames;
/// a lagging client drops the connection rather than stalling the worker.
const BROADCAST_CAPACITY: usize = 64;

pub struct ServeOptions {
    pub token: String,
    pub workspace: PathBuf,
}

/// Only loopback binds pass without explicit opt-in. Unspecified (`0.0.0.0`,
/// `[::]`) is not loopback, so it is refused too.
pub fn resolve_bind(bind: &str, allow_remote: bool) -> Result<IpAddr, String> {
    let ip: IpAddr = bind
        .parse()
        .map_err(|_| format!("invalid bind address: {bind}"))?;
    if !allow_remote && !ip.is_loopback() {
        return Err(format!(
            "refusing non-loopback bind {bind} without --allow-remote"
        ));
    }
    Ok(ip)
}

struct Shared {
    handle: ControllerHandle,
    updates: broadcast::Sender<ServeResponse>,
    latest_snapshot: Mutex<Option<ControllerEvent>>,
}

/// Serve on an already-bound listener until it errors. Starts the session
/// worker for `options.workspace` with one active session.
pub async fn serve(
    listener: TcpListener,
    options: ServeOptions,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (handle, mut updates) = InteractiveController::spawn(
        &tokio::runtime::Handle::current(),
        options.workspace.clone(),
    );
    handle
        .send(Command::StartNew(options.workspace.clone()))
        .map_err(|_| "session worker unavailable".to_string())?;
    let (fanout, _) = broadcast::channel(BROADCAST_CAPACITY);
    let shared = Arc::new(Shared {
        handle,
        updates: fanout,
        latest_snapshot: Mutex::new(None),
    });

    let pump = Arc::clone(&shared);
    tokio::spawn(async move {
        while let Some(event) = updates.recv().await {
            if matches!(event.update, ControllerUpdate::Snapshot(_)) {
                *pump.latest_snapshot.lock().await = Some(event.clone());
            }
            // A missing receiver just means no clients are attached yet.
            let _ = pump.updates.send(ServeResponse::from_event(event));
        }
    });

    let token: Arc<str> = options.token.into();
    loop {
        let (socket, _) = listener.accept().await?;
        let shared = Arc::clone(&shared);
        let token = Arc::clone(&token);
        tokio::spawn(async move {
            if let Err(error) = serve_connection(socket, shared, &token).await {
                crate::dbg_log!("serve connection closed: {error}");
            }
        });
    }
}

async fn serve_connection(
    socket: tokio::net::TcpStream,
    shared: Arc<Shared>,
    token: &str,
) -> Result<(), ProtocolError> {
    let (reader, mut writer) = socket.into_split();
    let mut reader = BufReader::new(reader);

    let hello: ServeRequest = tokio::time::timeout(AUTH_TIMEOUT, read_async_frame(&mut reader))
        .await
        .map_err(|_| ProtocolError::UnexpectedEof)??;
    let ServeRequest::Auth { token: presented } = hello else {
        write_async_frame(
            &mut writer,
            &ServeResponse::error("unauthorized", "first frame must authenticate"),
        )
        .await?;
        return Ok(());
    };
    if presented != token {
        write_async_frame(
            &mut writer,
            &ServeResponse::error("unauthorized", "bad token"),
        )
        .await?;
        return Ok(());
    }
    write_async_frame(
        &mut writer,
        &ServeResponse::Ready {
            version: SERVE_PROTOCOL_VERSION,
        },
    )
    .await?;
    if let Some(snapshot) = shared.latest_snapshot.lock().await.clone() {
        write_async_frame(&mut writer, &ServeResponse::Event(snapshot)).await?;
    }

    let mut feed = shared.updates.subscribe();
    loop {
        tokio::select! {
            request = read_async_frame(&mut reader) => {
                apply_request(&shared.handle, request?);
            }
            response = feed.recv() => {
                let response = response.map_err(|_| ProtocolError::UnexpectedEof)?;
                write_async_frame(&mut writer, &response).await?;
            }
        }
    }
}

/// Forward a client request to the worker. Malformed frames close the
/// connection at the read site; everything else is a worker command.
fn apply_request(handle: &ControllerHandle, request: ServeRequest) {
    let command = match request {
        ServeRequest::Auth { .. } => return,
        ServeRequest::ListSessions => Command::ListSessions,
        ServeRequest::Submit { prompt } => Command::Submit(prompt),
        ServeRequest::AnswerQuestion { answer } => Command::AnswerQuestion(answer),
        ServeRequest::Approve { batch_id, choice } => Command::ApprovalBatch { batch_id, choice },
        ServeRequest::Cancel => Command::Cancel,
    };
    let _ = handle.send(command);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::ControllerUpdate;
    use std::time::Duration;
    use tokio::net::TcpStream;

    #[test]
    fn loopback_binds_pass_and_remote_binds_need_opt_in() {
        assert!(resolve_bind("127.0.0.1", false).is_ok());
        assert!(resolve_bind("::1", false).is_ok());
        assert!(resolve_bind("0.0.0.0", false).is_err());
        assert!(resolve_bind("192.168.1.10", false).is_err());
        assert!(resolve_bind("0.0.0.0", true).is_ok());
        assert!(resolve_bind("not-an-addr", false).is_err());
    }

    struct ScriptClient {
        reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
        writer: tokio::net::tcp::OwnedWriteHalf,
    }

    impl ScriptClient {
        async fn connect(addr: std::net::SocketAddr) -> Self {
            let socket = TcpStream::connect(addr).await.expect("serve connect");
            let (reader, writer) = socket.into_split();
            Self {
                reader: BufReader::new(reader),
                writer,
            }
        }

        async fn send(&mut self, request: &ServeRequest) {
            write_async_frame(&mut self.writer, request)
                .await
                .expect("send request");
        }

        /// Next raw frame as JSON. The scripted client asserts on wire
        /// shape rather than deserializing contract types.
        async fn next_value(&mut self) -> serde_json::Value {
            use tokio::io::AsyncBufReadExt;
            let mut line = Vec::new();
            tokio::time::timeout(
                Duration::from_secs(15),
                self.reader.read_until(b'\n', &mut line),
            )
            .await
            .expect("response timeout")
            .expect("read response");
            serde_json::from_slice(&line).expect("response JSON")
        }

        /// Next snapshot frame, whatever it carries.
        async fn await_snapshot(&mut self) -> serde_json::Value {
            loop {
                let frame = self.next_value().await;
                if frame["type"] == "event" && frame["update"]["type"] == "snapshot" {
                    return frame;
                }
                panic!("expected snapshot stream, got {frame}");
            }
        }

        /// Next snapshot whose transcript contains `needle`.
        async fn await_snapshot_with(&mut self, needle: &str) -> serde_json::Value {
            loop {
                let frame = self.next_value().await;
                if frame["type"] == "event" && frame["update"]["type"] == "snapshot" {
                    let found = frame["update"]["transcript"]
                        .as_array()
                        .map(|transcript| {
                            transcript.iter().any(|item| {
                                item["content"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .contains(needle)
                            })
                        })
                        .unwrap_or(false);
                    if found {
                        return frame;
                    }
                    continue;
                }
                panic!("expected snapshot stream, got {frame}");
            }
        }

        /// Next worker error frame (e.g. rejected approval batch). Worker
        /// errors are lifted to top-level frames, not nested updates.
        async fn await_error(&mut self) -> serde_json::Value {
            loop {
                let frame = self.next_value().await;
                if frame["type"] == "error" {
                    return frame;
                }
                // Snapshots and turn updates interleave freely; keep waiting.
                assert_eq!(frame["type"], "event", "unexpected frame {frame}");
            }
        }
    }

    #[tokio::test]
    async fn wrong_token_is_rejected_before_any_session_data() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let workspace = tempfile::tempdir().expect("workspace");
        tokio::spawn(serve(
            listener,
            ServeOptions {
                token: "correct-token".into(),
                workspace: workspace.path().to_path_buf(),
            },
        ));

        let mut client = ScriptClient::connect(addr).await;
        client
            .send(&ServeRequest::Auth {
                token: "wrong-token".into(),
            })
            .await;
        let rejection = client.next_value().await;
        assert_eq!(rejection["type"], "error");
        assert_eq!(rejection["code"], "unauthorized");
        // Connection closes after the rejection.
        let mut buf = Vec::new();
        use tokio::io::AsyncReadExt;
        assert_eq!(
            client
                .reader
                .read_to_end(&mut buf)
                .await
                .expect("read close"),
            0
        );
    }

    #[tokio::test]
    async fn session_round_trip_list_submit_stream_approve_cancel() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let workspace = tempfile::tempdir().expect("workspace");
        tokio::spawn(serve(
            listener,
            ServeOptions {
                token: "test-token".into(),
                workspace: workspace.path().to_path_buf(),
            },
        ));

        let mut client = ScriptClient::connect(addr).await;
        client
            .send(&ServeRequest::Auth {
                token: "test-token".into(),
            })
            .await;
        let ready = client.next_value().await;
        assert_eq!(ready["type"], "ready");
        assert_eq!(ready["version"], SERVE_PROTOCOL_VERSION);

        // list → the freshly started session is advertised.
        client.send(&ServeRequest::ListSessions).await;
        let listed = client.await_snapshot().await;
        assert!(listed["update"]["session_id"].is_string());

        // submit → a native command resolves without any provider round-trip.
        client
            .send(&ServeRequest::Submit {
                prompt: "/help".into(),
            })
            .await;
        client.await_snapshot_with("Native commands:").await;

        // approve → an unknown batch is rejected as a top-level error frame.
        client
            .send(&ServeRequest::Approve {
                batch_id: "no-such-batch".into(),
                choice: crate::controller::ApprovalChoice::Approve,
            })
            .await;
        let rejection = client.await_error().await;
        assert_eq!(rejection["code"], "session");
        assert!(rejection["message"].as_str().is_some_and(|m| !m.is_empty()));

        // cancel → accepted on an idle session (snapshot churn, no error).
        client.send(&ServeRequest::Cancel).await;
        client.send(&ServeRequest::ListSessions).await;
        client.await_snapshot().await;
    }
}
