//! One WebSocket connection, from TCP accept to close.
//!
//! Each connection runs on its own task with its own bounded queue, so a
//! slow or stuck device can only hurt itself. Until the handshake succeeds
//! the socket holds an unauthenticated slot, works against one deadline and
//! receives nothing but a handshake response or an error.

use super::gateway::{Admitted, LiveConnection, MAX_FRAME_BYTES, Shared, UnauthenticatedSlot};
use super::handshake::{ErrorCode, HandshakeRequest, HandshakeResponse, MAX_HANDSHAKE_FRAME_BYTES};
use super::router::{FrameQueue, FrameSink};
use super::wire_names;
use futures_util::{SinkExt, StreamExt, stream::SplitSink, stream::SplitStream};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::{
    WebSocketStream, accept_hdr_async_with_config,
    tungstenite::{
        Error as WsError, Message,
        handshake::server::{ErrorResponse, Request, Response},
        http::StatusCode,
        protocol::WebSocketConfig,
    },
};

type Socket = WebSocketStream<TcpStream>;

/// How long a closing connection may spend delivering its last error frame.
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) async fn serve_connection(
    shared: Arc<Shared>,
    socket: TcpStream,
    slot: UnauthenticatedSlot,
) {
    let deadline = Instant::now() + shared.limits.handshake_timeout;
    // tungstenite bounds the HTTP upgrade itself (64 KiB of headers); these
    // bound every message after it.
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let upgraded = tokio::select! {
        _ = shared.shutdown.cancelled() => return,
        upgraded = tokio::time::timeout_at(
            deadline,
            accept_hdr_async_with_config(socket, refuse_browsers, Some(config)),
        ) => upgraded,
    };
    let Ok(Ok(mut socket)) = upgraded else {
        return;
    };
    let first = tokio::select! {
        _ = shared.shutdown.cancelled() => return,
        first = tokio::time::timeout_at(deadline, first_frame(&mut socket)) => first,
    };
    let mut thread_names = false;
    let admission = match first {
        Err(_) => Err(HandshakeResponse::error(
            ErrorCode::HandshakeTimeout,
            "no handshake frame arrived in time",
        )),
        Ok(Err(None)) => return,
        Ok(Err(Some(rejection))) => Err(rejection),
        Ok(Ok(text)) => {
            thread_names =
                HandshakeRequest::parse(&text).is_ok_and(|request| request.wants_thread_names());
            shared.admit_device(&text)
        }
    };
    let Admitted {
        connection,
        mut welcome,
        sink,
        queue,
    } = match admission {
        Ok(admitted) => admitted,
        Err(rejection) => {
            // One attempt per socket: the peer must reconnect to try again,
            // and every limit that matters is host-wide.
            goodbye(&mut socket, &rejection).await;
            return;
        }
    };
    drop(slot);
    if thread_names {
        welcome.confirm_thread_names();
    }

    let delivered = tokio::time::timeout(
        shared.limits.write_timeout,
        socket.send(Message::text(welcome.to_text())),
    )
    .await;
    if !matches!(delivered, Ok(Ok(()))) {
        if matches!(welcome, HandshakeResponse::Paired { .. }) {
            shared.forget_undelivered(&connection.context.device_id);
        }
        return;
    }
    session(&shared, socket, connection, sink, queue, thread_names).await;
}

/// The one `Origin` a native app may send. React Native always adds one to a
/// WebSocket upgrade and lets the caller replace it, so the phone app sends
/// this custom scheme instead. A browser can never send a non-`http(s)`
/// origin, and a page cannot set a header on a WebSocket it opens, so
/// accepting it verbatim does not open the gateway to a web page.
pub const NATIVE_APP_ORIGIN: &str = "rustcode-mobile://app";

/// Refuse the upgrade when the request carries an `Origin` header other than
/// [`NATIVE_APP_ORIGIN`]. Browsers always send one and native clients do not,
/// so a web page open on the host or the LAN cannot be used to reach the
/// gateway and spend pairing attempts.
///
/// The value is compared verbatim: `null`, a near miss such as a trailing
/// slash, and an `http(s)` origin that merely matches the request's `Host`
/// all stay refused, because DNS rebinding can forge the last of those.
#[allow(clippy::result_large_err)]
fn refuse_browsers(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let native_app = request
        .headers()
        .get("origin")
        .is_none_or(|origin| origin.as_bytes() == NATIVE_APP_ORIGIN.as_bytes());
    if !native_app {
        let mut refusal = ErrorResponse::new(None);
        *refusal.status_mut() = StatusCode::FORBIDDEN;
        return Err(refusal);
    }
    Ok(response)
}

/// The first text frame, or why there is none: `None` when the peer went
/// away, a rejection when it sent something a handshake cannot be.
async fn first_frame(socket: &mut Socket) -> Result<String, Option<HandshakeResponse>> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                if text.len() > MAX_HANDSHAKE_FRAME_BYTES {
                    return Err(Some(HandshakeResponse::error(
                        ErrorCode::FrameTooLarge,
                        "handshake frame is too large",
                    )));
                }
                return Ok(text.as_str().to_owned());
            }
            // Control frames are answered by tungstenite; the deadline still runs.
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Binary(_))) => {
                return Err(Some(HandshakeResponse::error(
                    ErrorCode::InvalidFrame,
                    "frames must be JSON text",
                )));
            }
            Some(Err(WsError::Capacity(_))) => {
                return Err(Some(HandshakeResponse::error(
                    ErrorCode::FrameTooLarge,
                    "handshake frame is too large",
                )));
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Err(None),
        }
    }
}

async fn goodbye(socket: &mut Socket, error: &HandshakeResponse) {
    let _ = tokio::time::timeout(GOODBYE_TIMEOUT, async {
        socket.send(Message::text(error.to_text())).await?;
        socket.close(None).await
    })
    .await;
}

async fn session(
    shared: &Arc<Shared>,
    socket: Socket,
    connection: LiveConnection,
    sink: FrameSink,
    mut queue: FrameQueue,
    thread_names: bool,
) {
    let (mut writer, mut reader) = socket.split();
    shared.router.connected(&connection.context, &sink);
    // Cancellation (revocation, overflow, shutdown) wins over whatever the
    // pump is doing, including a write stuck on a peer that stopped reading.
    let ended = tokio::select! {
        biased;
        _ = connection.control.cancel.cancelled() => Some(connection.control.reason()),
        ended = pump(shared, &connection, &sink, &mut writer, &mut reader, &mut queue, thread_names) => ended,
    };
    shared.router.disconnected(&connection.context);
    // Leave the live registry before the goodbye so status never lists a
    // connection that is only being told why it was closed.
    drop(connection);
    if let Some(code) = ended {
        let error = HandshakeResponse::error(code, close_message(code));
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, async {
            writer.send(Message::text(error.to_text())).await?;
            writer.close().await
        })
        .await;
    }
}

/// Move frames in both directions until the connection ends. Returns the
/// error to report to the device, or `None` when it is already gone.
async fn pump(
    shared: &Arc<Shared>,
    connection: &LiveConnection,
    sink: &FrameSink,
    writer: &mut SplitSink<Socket, Message>,
    reader: &mut SplitStream<Socket>,
    queue: &mut FrameQueue,
    thread_names: bool,
) -> Option<ErrorCode> {
    let limits = shared.limits;
    let mut ping =
        tokio::time::interval_at(Instant::now() + limits.ping_interval, limits.ping_interval);
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            outbound = queue.recv() => {
                let frame = outbound?;
                let frame = if thread_names { wire_names::to_thread_names(&frame) } else { frame };
                write(writer, Message::text(frame), limits.write_timeout).await?;
            }
            _ = ping.tick() => {
                if last_heard.elapsed() >= limits.idle_timeout {
                    return Some(ErrorCode::IdleTimeout);
                }
                write(writer, Message::Ping(Default::default()), limits.write_timeout).await?;
            }
            inbound = reader.next() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    last_heard = Instant::now();
                    if thread_names {
                        let text = wire_names::to_session_names(text.as_str());
                        shared.router.frame(&connection.context, &text, sink);
                    } else {
                        shared.router.frame(&connection.context, text.as_str(), sink);
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {
                    last_heard = Instant::now();
                }
                Some(Ok(Message::Binary(_))) => return Some(ErrorCode::InvalidFrame),
                Some(Err(WsError::Capacity(_))) => return Some(ErrorCode::FrameTooLarge),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            },
        }
    }
}

/// Write one frame within the deadline; `None` means the connection is dead.
async fn write(
    writer: &mut SplitSink<Socket, Message>,
    message: Message,
    deadline: Duration,
) -> Option<()> {
    match tokio::time::timeout(deadline, writer.send(message)).await {
        Ok(Ok(())) => Some(()),
        _ => None,
    }
}

fn close_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Revoked => "this device was revoked on the host",
        ErrorCode::SlowConsumer => "the device fell too far behind",
        ErrorCode::IdleTimeout => "the connection was idle for too long",
        ErrorCode::FrameTooLarge => "frame is too large",
        ErrorCode::InvalidFrame => "frames must be JSON text",
        ErrorCode::ShuttingDown => "the gateway is shutting down",
        _ => "connection closed",
    }
}

#[cfg(test)]
mod tests {
    use super::{NATIVE_APP_ORIGIN, refuse_browsers};
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    /// `None` accepts the upgrade; `Some(403)` refuses it.
    fn upgrade_status(origin: Option<&str>) -> Option<u16> {
        let mut request = Request::builder().uri("/").body(()).unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("origin", origin.parse().unwrap());
        }
        let response = Response::new(());
        match refuse_browsers(&request, response) {
            Ok(_) => None,
            Err(refusal) => Some(refusal.status().as_u16()),
        }
    }

    #[test]
    fn a_request_without_an_origin_is_accepted() {
        assert_eq!(upgrade_status(None), None);
    }

    #[test]
    fn the_native_app_origin_is_accepted() {
        assert_eq!(NATIVE_APP_ORIGIN, "rustcode-mobile://app");
        assert_eq!(upgrade_status(Some(NATIVE_APP_ORIGIN)), None);
    }

    #[test]
    fn browser_origins_and_near_misses_are_refused() {
        for origin in [
            "http://192.168.1.5:7878",
            "http://127.0.0.1:7878",
            "https://evil.com",
            "null",
            "chrome-extension://x",
            "file://",
            "rustcode-mobile://app/",
            "rustcode-mobile://app?x",
            "rustcode-mobile://appx",
            "rustcode-mobile://app ",
            "RUSTCODE-MOBILE://APP",
        ] {
            assert_eq!(upgrade_status(Some(origin)), Some(403), "{origin}");
        }
    }
}
