//! One WebSocket connection, from TCP accept to close.
//!
//! Each connection runs on its own task with its own bounded queue, so a
//! slow or stuck device can only hurt itself. Until the handshake succeeds
//! the socket holds an unauthenticated slot, works against one deadline and
//! receives nothing but a handshake response or an error.

use super::gateway::{Admitted, LiveConnection, MAX_FRAME_BYTES, Shared, UnauthenticatedSlot};
use super::handshake::{ErrorCode, HandshakeResponse, MAX_HANDSHAKE_FRAME_BYTES};
use super::router::{FrameQueue, FrameSink};
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
    let admission = match first {
        Err(_) => Err(HandshakeResponse::error(
            ErrorCode::HandshakeTimeout,
            "no handshake frame arrived in time",
        )),
        Ok(Err(None)) => return,
        Ok(Err(Some(rejection))) => Err(rejection),
        Ok(Ok(text)) => shared.admit_device(&text),
    };
    let Admitted {
        connection,
        welcome,
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
    session(&shared, socket, connection, sink, queue).await;
}

/// Refuse the upgrade when the request carries an `Origin` header. Browsers
/// always send one and native clients do not, so a web page open on the host
/// or the LAN cannot be used to reach the gateway and spend pairing attempts.
#[allow(clippy::result_large_err)]
fn refuse_browsers(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.headers().contains_key("origin") {
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
) {
    let (mut writer, mut reader) = socket.split();
    shared.router.connected(&connection.context, &sink);
    // Cancellation (revocation, overflow, shutdown) wins over whatever the
    // pump is doing, including a write stuck on a peer that stopped reading.
    let ended = tokio::select! {
        biased;
        _ = connection.control.cancel.cancelled() => Some(connection.control.reason()),
        ended = pump(shared, &connection, &sink, &mut writer, &mut reader, &mut queue) => ended,
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
) -> Option<ErrorCode> {
    let limits = shared.limits;
    let mut ping =
        tokio::time::interval_at(Instant::now() + limits.ping_interval, limits.ping_interval);
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            outbound = queue.recv() => {
                let frame = outbound?;
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
                    shared.router.frame(&connection.context, text.as_str(), sink);
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
