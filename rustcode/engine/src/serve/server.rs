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
use crate::daemon::protocol::{
    ProtocolError, read_async_frame, read_async_frame_with_buffer, write_async_frame,
};

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

/// Compare every byte without returning early for a matching prefix. Runtime
/// may still reveal the presented token's length, which is client controlled.
fn token_matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    let mut difference = expected.len() ^ presented.len();
    for index in 0..expected.len().max(presented.len()) {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ presented.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
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
    if !token_matches(token, &presented) {
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
    let mut feed = shared.updates.subscribe();
    let snapshot = shared.latest_snapshot.lock().await.clone();
    let mut current_generation = snapshot.as_ref().map(|event| event.generation);
    if let Some(snapshot) = snapshot {
        write_async_frame(&mut writer, &ServeResponse::Event(snapshot)).await?;
    }

    let mut pending_request = Vec::new();
    loop {
        tokio::select! {
            request = read_async_frame_with_buffer(&mut reader, &mut pending_request) => {
                apply_request(&shared.handle, request?);
            }
            response = feed.recv() => {
                let response = response.map_err(|_| ProtocolError::UnexpectedEof)?;
                if let ServeResponse::Event(event) = &response {
                    if current_generation.is_some_and(|generation| event.generation < generation) {
                        continue;
                    }
                    current_generation = Some(event.generation);
                }
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
    use crate::controller::{ControllerSnapshot, ControllerUpdate, TurnUpdate};
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

    #[test]
    fn token_match_requires_equal_length_and_all_bytes() {
        assert!(token_matches("secret-token", "secret-token"));
        assert!(!token_matches("secret-token", "secret-tokem"));
        assert!(!token_matches("secret-token", "secret-token-extra"));
        assert!(!token_matches("secret-token", "secret-toke"));
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
            let read = tokio::time::timeout(
                Duration::from_secs(15),
                self.reader.read_until(b'\n', &mut line),
            )
            .await
            .expect("response timeout")
            .expect("read response");
            assert!(read > 0, "serve connection closed before a response frame");
            serde_json::from_slice(&line).expect("response JSON")
        }

        /// Next snapshot frame, whatever it carries.
        async fn await_snapshot(&mut self) -> serde_json::Value {
            let frame = self.next_value().await;
            assert!(
                frame["type"] == "event" && frame["update"]["type"] == "snapshot",
                "expected snapshot stream, got {frame}"
            );
            frame
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
    async fn reconnect_keeps_updates_arriving_during_snapshot_replay() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let workspace = tempfile::tempdir().expect("workspace");
        let (handle, _updates) = InteractiveController::spawn(
            &tokio::runtime::Handle::current(),
            workspace.path().to_path_buf(),
        );
        let snapshot = ControllerEvent {
            generation: 1,
            update: ControllerUpdate::Snapshot(ControllerSnapshot::from_state(
                1,
                &crate::app::AppState::new(),
            )),
        };
        let (fanout, _) = broadcast::channel(BROADCAST_CAPACITY);
        let shared = Arc::new(Shared {
            handle,
            updates: fanout,
            latest_snapshot: Mutex::new(Some(snapshot)),
        });
        let snapshot_guard = shared.latest_snapshot.lock().await;
        let connection_shared = Arc::clone(&shared);
        let connection = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            serve_connection(socket, connection_shared, "test-token").await
        });

        let mut client = ScriptClient::connect(addr).await;
        client
            .send(&ServeRequest::Auth {
                token: "test-token".into(),
            })
            .await;
        assert_eq!(client.next_value().await["type"], "ready");
        tokio::time::timeout(Duration::from_secs(2), async {
            while shared.updates.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("live feed subscribed before snapshot replay");
        let stale = ControllerEvent {
            generation: 0,
            update: ControllerUpdate::Turn(TurnUpdate::TextDelta("stale".into())),
        };
        let _ = shared.updates.send(ServeResponse::Event(stale));
        let live = ControllerEvent {
            generation: 1,
            update: ControllerUpdate::Turn(TurnUpdate::TextDelta("during replay".into())),
        };
        let _ = shared.updates.send(ServeResponse::Event(live));
        drop(snapshot_guard);
        assert_eq!(client.next_value().await["update"]["type"], "snapshot");
        let update = client.next_value().await;
        assert_eq!(update["update"]["type"], "text_delta");
        assert_eq!(update["update"]["text"], "during replay");
        connection.abort();
    }

    #[tokio::test]
    async fn partial_request_survives_an_interleaved_update() {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let (commands, mut received_commands) = tokio::sync::mpsc::unbounded_channel();
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        let shared = Arc::new(Shared {
            handle: ControllerHandle::new(commands),
            updates,
            latest_snapshot: Mutex::new(None),
        });
        let connection_shared = Arc::clone(&shared);
        let connection = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept");
            serve_connection(socket, connection_shared, "test-token").await
        });

        let mut client = ScriptClient::connect(addr).await;
        client
            .send(&ServeRequest::Auth {
                token: "test-token".into(),
            })
            .await;
        assert_eq!(client.next_value().await["type"], "ready");
        tokio::time::timeout(Duration::from_secs(2), async {
            while shared.updates.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("feed subscription");

        client.writer.write_all(b"{\"type\":\"list").await.unwrap();
        // Allow the server to consume the first bytes while waiting for the newline.
        tokio::time::sleep(Duration::from_millis(20)).await;
        shared
            .updates
            .send(ServeResponse::Event(ControllerEvent {
                generation: 1,
                update: ControllerUpdate::Turn(TurnUpdate::TextDelta("interleaved".into())),
            }))
            .unwrap();
        assert_eq!(client.next_value().await["update"]["text"], "interleaved");
        client.writer.write_all(b"_sessions\"}\n").await.unwrap();
        let request = tokio::time::timeout(Duration::from_secs(2), received_commands.recv())
            .await
            .expect("server should retain the partial request");
        assert!(matches!(request, Some(Command::ListSessions)));
        connection.abort();
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
