//! One WebSocket connection, from TCP accept to close.
//!
//! Each connection runs on its own task with its own bounded queue, so a
//! slow or stuck device can only hurt itself. Until the handshake succeeds
//! the socket holds an unauthenticated slot, works against one deadline and
//! receives nothing but a handshake response or an error.
//!
//! The one exception is a socket whose first frame is `connect_request`: it
//! stays open, past the handshake deadline, while the host's owner answers
//! (see [`super::connect`]). It is told `connect_pending`, then
//! `connect_approved`, `connect_denied` or `connect_expired`; after
//! `connect_approved` it may send exactly one more frame, a `pair` request
//! with the code the owner reads off the host.

use super::connect::Outcome;
use super::control::RefusalReason;
use super::devices::DeviceProfile;
use super::gateway::{
    Admitted, ConnectTicket, LiveConnection, MAX_FRAME_BYTES, Shared, UnauthenticatedSlot,
};
use super::handshake::{
    DeviceHint, ErrorCode, HandshakeRequest, HandshakeResponse, MAX_HANDSHAKE_FRAME_BYTES,
    SessionControl, session_control,
};
use super::pairing::CODE_DIGITS;
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
    let mut socket = match upgraded {
        Ok(Ok(socket)) => socket,
        Ok(Err(WsError::Http(response))) => {
            // The upgrade is refused before any frame, so the device is told
            // nothing. A native app built before it sent the native app origin
            // is refused exactly like a browser page, and its owner has no way
            // to learn that from the connection itself.
            if response.status() == StatusCode::FORBIDDEN {
                shared.record_refusal(
                    slot.ip(),
                    RefusalReason::UnexpectedOrigin,
                    &DeviceHint::default(),
                );
            }
            return;
        }
        _ => return,
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
        Ok(Ok(text)) => match HandshakeRequest::parse(&text) {
            Ok(HandshakeRequest::ConnectRequest {
                device_name,
                platform,
                install_id,
                device_kind,
                host_id,
                ..
            }) => {
                let ticket = shared.open_connect_request(
                    slot.ip(),
                    &device_name,
                    DeviceProfile::from_wire(
                        device_kind.as_deref(),
                        Some(&platform),
                        host_id.as_deref(),
                    ),
                    install_id.as_deref(),
                );
                match ticket {
                    Err(rejection) => Err(rejection),
                    Ok(ticket) => match connect_request(&shared, &mut socket, ticket).await {
                        Connected::Admitted(admitted, names) => {
                            thread_names = names;
                            Ok(admitted)
                        }
                        Connected::Closing(frame) => Err(frame),
                        Connected::Gone => return,
                    },
                }
            }
            parsed => {
                thread_names = parsed.is_ok_and(|request| request.wants_thread_names());
                shared.admit_device(slot.ip(), &text)
            }
        },
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

/// How a connection request ended for its socket.
enum Connected {
    /// The device presented the code and is paired; with whether it asked
    /// for the thread names.
    Admitted(Admitted, bool),
    /// Send this frame, then close.
    Closing(HandshakeResponse),
    /// The device went away.
    Gone,
}

/// What happened while a requesting socket waited.
enum Waited {
    Outcome(Outcome),
    Deadline,
    Text(String),
    Closing(HandshakeResponse),
    Gone,
}

/// Follow a connection request from `connect_pending` to its end. Dropping
/// the ticket on the way out ends the request if nothing else did.
async fn connect_request(
    shared: &Arc<Shared>,
    socket: &mut Socket,
    ticket: ConnectTicket,
) -> Connected {
    let mut outcome = ticket.outcome.clone();
    let pending = HandshakeResponse::ConnectPending {
        request_id: ticket.id.clone(),
        // Rounded up: the request was opened a moment ago.
        expires_in_secs: ticket
            .expires_at
            .saturating_duration_since(std::time::Instant::now())
            .as_secs_f64()
            .ceil() as u64,
    };
    if send_frame(shared, socket, &pending).await.is_none() {
        return Connected::Gone;
    }
    let deadline = Instant::from_std(ticket.expires_at);
    // Until the owner answers, the socket only listens.
    let expires_in = loop {
        let decided = match wait(shared, socket, &mut outcome, deadline, false).await {
            Waited::Deadline if !shared.expire_connect_request(&ticket.id) => {
                // Accepted at the last moment: its challenge runs on.
                *outcome.borrow_and_update()
            }
            Waited::Deadline => return Connected::Closing(HandshakeResponse::ConnectExpired),
            Waited::Outcome(decided) => decided,
            Waited::Text(_) => unreachable!("text is refused while waiting"),
            Waited::Closing(frame) => return Connected::Closing(frame),
            Waited::Gone => return Connected::Gone,
        };
        match decided {
            Outcome::Pending => {}
            Outcome::Approved { expires_in } => break expires_in,
            Outcome::Denied => return Connected::Closing(HandshakeResponse::ConnectDenied),
            Outcome::Expired => return Connected::Closing(HandshakeResponse::ConnectExpired),
        }
    };
    let approved = HandshakeResponse::ConnectApproved {
        request_id: ticket.id.clone(),
        code_length: CODE_DIGITS as u32,
        expires_in_secs: expires_in.as_secs(),
    };
    if send_frame(shared, socket, &approved).await.is_none() {
        return Connected::Gone;
    }
    // One frame: the pair request with the code.
    let deadline = Instant::now() + expires_in;
    loop {
        match wait(shared, socket, &mut outcome, deadline, true).await {
            Waited::Text(text) => {
                let names = HandshakeRequest::parse(&text)
                    .is_ok_and(|request| request.wants_thread_names());
                return match shared.admit_connected(ticket.ip(), &ticket.id, &text) {
                    Ok(admitted) => Connected::Admitted(admitted, names),
                    Err(rejection) => Connected::Closing(rejection),
                };
            }
            Waited::Outcome(Outcome::Pending | Outcome::Approved { .. }) => {}
            Waited::Outcome(Outcome::Denied | Outcome::Expired) => {
                return Connected::Closing(HandshakeResponse::ConnectExpired);
            }
            Waited::Deadline => {
                shared.expire_connect_request(&ticket.id);
                return Connected::Closing(HandshakeResponse::ConnectExpired);
            }
            Waited::Closing(frame) => return Connected::Closing(frame),
            Waited::Gone => return Connected::Gone,
        }
    }
}

/// Wait on a requesting socket for the owner's answer, the deadline, or
/// (with `accept_text`) the device's next frame. Pings keep the socket
/// alive meanwhile; any other frame the device sends early is an error.
async fn wait(
    shared: &Arc<Shared>,
    socket: &mut Socket,
    outcome: &mut tokio::sync::watch::Receiver<Outcome>,
    deadline: Instant,
    accept_text: bool,
) -> Waited {
    let limits = shared.limits;
    let mut ping =
        tokio::time::interval_at(Instant::now() + limits.ping_interval, limits.ping_interval);
    enum Event {
        Shutdown,
        Changed(bool),
        Deadline,
        Ping,
        Inbound(Option<Result<Message, WsError>>),
    }
    loop {
        let event = tokio::select! {
            biased;
            _ = shared.shutdown.cancelled() => Event::Shutdown,
            changed = outcome.changed() => Event::Changed(changed.is_ok()),
            _ = tokio::time::sleep_until(deadline) => Event::Deadline,
            _ = ping.tick() => Event::Ping,
            inbound = socket.next() => Event::Inbound(inbound),
        };
        match event {
            Event::Shutdown => {
                return Waited::Closing(HandshakeResponse::error(
                    ErrorCode::ShuttingDown,
                    close_message(ErrorCode::ShuttingDown),
                ));
            }
            // A request that ends without a word to its socket is over.
            Event::Changed(false) => return Waited::Outcome(Outcome::Expired),
            Event::Changed(true) => return Waited::Outcome(*outcome.borrow_and_update()),
            Event::Deadline => return Waited::Deadline,
            Event::Ping => {
                let sent = tokio::time::timeout(
                    limits.write_timeout,
                    socket.send(Message::Ping(Default::default())),
                )
                .await;
                if !matches!(sent, Ok(Ok(()))) {
                    return Waited::Gone;
                }
            }
            Event::Inbound(inbound) => match inbound {
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Text(text))) if accept_text => {
                    if text.len() > MAX_HANDSHAKE_FRAME_BYTES {
                        return Waited::Closing(HandshakeResponse::error(
                            ErrorCode::FrameTooLarge,
                            "handshake frame is too large",
                        ));
                    }
                    return Waited::Text(text.as_str().to_owned());
                }
                Some(Ok(Message::Text(_))) => {
                    return Waited::Closing(HandshakeResponse::error(
                        ErrorCode::InvalidFrame,
                        "wait for connect_approved before sending the code",
                    ));
                }
                Some(Ok(Message::Binary(_))) => {
                    return Waited::Closing(HandshakeResponse::error(
                        ErrorCode::InvalidFrame,
                        "frames must be JSON text",
                    ));
                }
                Some(Err(WsError::Capacity(_))) => {
                    return Waited::Closing(HandshakeResponse::error(
                        ErrorCode::FrameTooLarge,
                        "handshake frame is too large",
                    ));
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Waited::Gone,
            },
        }
    }
}

/// Send one handshake frame within the write deadline; `None` when the
/// socket is dead.
async fn send_frame(shared: &Shared, socket: &mut Socket, frame: &HandshakeResponse) -> Option<()> {
    match tokio::time::timeout(
        shared.limits.write_timeout,
        socket.send(Message::text(frame.to_text())),
    )
    .await
    {
        Ok(Ok(())) => Some(()),
        _ => None,
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
    if ended.is_none() {
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, writer.close()).await;
    }
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
                    // A token acknowledgement is the gateway's own frame: it
                    // retires the previous refresh token and is never routed.
                    match session_control(text.as_str()) {
                        SessionControl::NotAControl => {}
                        SessionControl::TokenAck(ack) => {
                            shared.acknowledge_rotation(
                                &connection.context.device_id,
                                &ack.rotation_id,
                            );
                            continue;
                        }
                        SessionControl::Malformed => return Some(ErrorCode::InvalidFrame),
                    }
                    if serde_json::from_str::<crate::remote::protocol::AcpClientFrame>(&text).is_ok() {
                        if !shared.acp_allowed(&connection.context.device_id) {
                            let error = HandshakeResponse::error(ErrorCode::UnauthorizedAccess, "ACP access requires approval from the host owner");
                            write(writer, Message::text(error.to_text()), limits.write_timeout).await?;
                            continue;
                        }
                        // Stop hub pushes before switching to raw ACP frames.
                        shared.router.disconnected(&connection.context);
                        // An app-spawned engine publishes its own endpoint; a
                        // shared server publishes `run/server.json`. Either
                        // description can be a dead process's, so relaying
                        // tries each in turn (#3341).
                        let endpoints = crate::server::relay_endpoints(
                            &shared.lifecycle.config_directory(),
                        );
                        if endpoints.is_empty() {
                            eprintln!(
                                "remote ACP: no RustCode engine is running to relay to; \
                                 keep the app open on this machine, or run `rustcode server run`"
                            );
                            crate::dbg_log!(
                                "remote ACP: no engine described itself in run/ (no engine.json, no server.json)"
                            );
                            return Some(ErrorCode::Internal);
                        }
                        return super::acp_relay::relay(&endpoints, writer, reader, Arc::clone(&connection.control), limits.write_timeout, limits.ping_interval, limits.idle_timeout).await;
                    }
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
        ErrorCode::Internal => "the relay to this machine's engine could not be opened",
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
