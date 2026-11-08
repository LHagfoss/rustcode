//! The WebSocket transport: one connection to a machine's remote gateway.
//!
//! The gateway refuses browser `Origin`s and expects the handshake as the
//! first text frame, so this dials with `tokio-tungstenite` from Rust, sends
//! at most one handshake frame per step, and then either hands back a
//! [`Connection`] for plain remote-protocol frames or switches the socket to
//! ACP ([`AcpChannel`]).
//!
//! TLS is not spoken here: the gateway has no TLS listener
//! (`docs/remote.md` › Security posture), and a private overlay such as
//! Tailscale or NetBird is what encrypts the traffic. A `wss://` address is
//! therefore refused with a message saying so rather than silently dialled.

use crate::protocol::{
    AcpClientFrame, AcpServerFrame, HandshakeRequest, HandshakeResponse, MAX_HANDSHAKE_FRAME_BYTES,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{WebSocketStream, client_async_with_config, tungstenite::Message};

/// Default TCP port of a machine's gateway.
pub const DEFAULT_PORT: u16 = 17879;

/// Frames larger than this are refused by the gateway; refuse them here too,
/// so a broken peer cannot make the client allocate without bound.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long a TCP connect and the WebSocket upgrade together may take. The
/// route racing in step 1.8 keeps its own, much shorter, per-route deadline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// Plain `ws` only: the gateway has no TLS listener, so there is no TLS
// stream type to allow for here.
type Socket = WebSocketStream<TcpStream>;

/// An open WebSocket to a machine's gateway, before or after authentication.
pub struct Connection {
    socket: Socket,
}

impl Connection {
    /// Dial `address` and complete the WebSocket upgrade.
    ///
    /// `address` is `host:port`, a bare host (the default port), `[v6]:port`
    /// or an explicit `ws://` URL.
    pub async fn open(address: &str) -> Result<Self> {
        Self::open_with_timeout(address, CONNECT_TIMEOUT).await
    }

    pub async fn open_with_timeout(address: &str, timeout: Duration) -> Result<Self> {
        let url = websocket_url(address)?;
        let target = dial_target(address)?;
        let socket = tokio::time::timeout(timeout, TcpStream::connect(target))
            .await
            .with_context(|| format!("connecting to {address} took longer than {timeout:?}"))?
            .with_context(|| format!("could not connect to {address}"))?;
        // A prompt's worth of output is small; do not wait on Nagle for it.
        let _ = socket.set_nodelay(true);
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (socket, _response) =
            tokio::time::timeout(timeout, client_async_with_config(url, socket, Some(config)))
                .await
                .with_context(|| format!("{address} did not answer the WebSocket upgrade in time"))?
                .with_context(|| format!("{address} refused the WebSocket upgrade"))?;
        Ok(Self { socket })
    }

    /// Send one handshake frame.
    pub async fn send_handshake(&mut self, frame: &HandshakeRequest) -> Result<()> {
        let text = serde_json::to_string(frame)?;
        ensure!(
            text.len() <= MAX_HANDSHAKE_FRAME_BYTES,
            "handshake frame would be {} bytes; the limit is {MAX_HANDSHAKE_FRAME_BYTES}",
            text.len()
        );
        self.socket.send(Message::text(text)).await.context("send")
    }

    /// The next handshake frame, waiting at most `timeout`. A closed socket
    /// is an error: the gateway answers every step with a frame first.
    pub async fn next_handshake(&mut self, timeout: Duration) -> Result<HandshakeResponse> {
        let text = self.next_text(Some(timeout)).await?;
        serde_json::from_str(&text).with_context(|| {
            // The text can carry a token, so only its shape is reported.
            format!(
                "the machine answered with something other than a handshake frame ({} bytes)",
                text.len()
            )
        })
    }

    /// Send one JSON-RPC request and wait for its answer, skipping
    /// notifications and server-to-client requests that arrive first.
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        let frame =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_json(&frame).await?;
        loop {
            let frame = self
                .next_json(CONNECT_TIMEOUT)
                .await?
                .context("the machine closed the connection before answering")?;
            if frame.get("id") == Some(&Value::from(id)) && frame.get("method").is_none() {
                return Ok(frame);
            }
            // A notification (`session/update`) or the machine asking us
            // something: there is nothing to answer here yet, so it is
            // skipped. Step 1.9 drives approvals and questions.
        }
    }

    /// Switch the authenticated socket to ACP: send `acp_open`, wait for
    /// `acp_opened`, and return the raw JSON-RPC channel.
    pub async fn acp_open(self) -> Result<AcpChannel> {
        let mut connection = self;
        let text = serde_json::to_string(&AcpClientFrame::AcpOpen)?;
        connection
            .socket
            .send(Message::text(text))
            .await
            .context("send acp_open")?;
        let answer = connection.next_text(Some(CONNECT_TIMEOUT)).await?;
        match serde_json::from_str::<AcpServerFrame>(&answer) {
            Ok(AcpServerFrame::AcpOpened) => Ok(AcpChannel {
                socket: connection.socket,
            }),
            Err(_) => {
                // The gateway refuses with an `error` frame. ACP access is a
                // grant the machine's owner gives, so say how to give it.
                match serde_json::from_str::<HandshakeResponse>(&answer) {
                    Ok(HandshakeResponse::Error {
                        code,
                        message,
                        retry_after_secs,
                    }) => Err(anyhow::anyhow!(
                        "the machine refused ACP access ({code}): {message}{}; on that machine, \
                         run `rustcode remote devices allow <device>`",
                        retry_after_secs
                            .map(|seconds| format!(" (retry in {seconds}s)"))
                            .unwrap_or_default(),
                    )),
                    _ => bail!("the machine did not confirm acp_open"),
                }
            }
        }
    }

    async fn send_json(&mut self, frame: &Value) -> Result<()> {
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .context("send")
    }

    /// The next JSON frame, or `None` when the machine closed the socket.
    pub async fn next_json(&mut self, timeout: Duration) -> Result<Option<Value>> {
        match self.next_text_or_close(Some(timeout)).await? {
            Some(text) => Ok(Some(
                serde_json::from_str(&text).context("the machine sent a frame that is not JSON")?,
            )),
            None => Ok(None),
        }
    }

    async fn next_text(&mut self, timeout: Option<Duration>) -> Result<String> {
        self.next_text_or_close(timeout)
            .await?
            .context("the machine closed the connection")
    }

    /// The next text frame; `None` when the socket closed. Pings are answered
    /// by tungstenite, and the gateway's pongs are read here.
    async fn next_text_or_close(&mut self, timeout: Option<Duration>) -> Result<Option<String>> {
        loop {
            let next = match timeout {
                Some(timeout) => tokio::time::timeout(timeout, self.socket.next())
                    .await
                    .with_context(|| format!("no frame from the machine within {timeout:?}"))?,
                None => self.socket.next().await,
            };
            match next {
                Some(Ok(Message::Text(text))) => return Ok(Some(text.as_str().to_owned())),
                Some(Ok(Message::Binary(_))) => {
                    bail!("the machine sent a binary frame; remote protocol v1 is text")
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    if let Some(frame) = frame {
                        bail!(
                            "the machine closed the connection: {} (code {})",
                            frame.reason,
                            u16::from(frame.code)
                        );
                    }
                    return Ok(None);
                }
                Some(Err(error)) => {
                    return Err(error).context("the connection to the machine failed");
                }
                None => return Ok(None),
            }
        }
    }
}

/// A JSON-RPC channel over a socket that is now carrying ACP.
///
/// One object per text frame. The machine sends notifications
/// (`session/update`) and server-to-client requests (approvals, questions)
/// on the same channel as answers, so [`Self::recv`] returns every frame and
/// [`Self::request`] filters for the one it asked for.
pub struct AcpChannel {
    socket: Socket,
}

impl AcpChannel {
    /// Send one JSON-RPC frame, request or notification.
    pub async fn send(&mut self, frame: Value) -> Result<()> {
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .context("send")
    }

    /// The next ACP frame, or `None` when the machine closed the channel.
    pub async fn recv(&mut self) -> Result<Option<Value>> {
        loop {
            match tokio::time::timeout(CONNECT_TIMEOUT, self.socket.next())
                .await
                .context("the machine stopped sending ACP frames")?
            {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(
                        serde_json::from_str(&text)
                            .context("the machine sent a frame that is not JSON")?,
                    ));
                }
                Some(Ok(Message::Binary(_))) => bail!("the machine sent a binary ACP frame"),
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(_))) | None => return Ok(None),
                Some(Err(error)) => {
                    return Err(error).context("the ACP connection to the machine failed");
                }
            }
        }
    }

    /// Send a request and return its answer, skipping notifications and
    /// server-to-client requests that arrive first.
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .await?;
        loop {
            let frame = self
                .recv()
                .await?
                .context("the machine closed the ACP channel before answering")?;
            if frame.get("id") == Some(&Value::from(id)) && frame.get("method").is_none() {
                ensure!(
                    frame.get("error").is_none(),
                    "the machine refused {method}: {}",
                    Describe::of(&frame)
                );
                return Ok(frame);
            }
        }
    }
}

/// A bounded description of an ACP error frame, for a message to the user.
struct Describe;

impl Describe {
    fn of(frame: &Value) -> String {
        let error = &frame["error"];
        let message = error["message"].as_str().unwrap_or("no message");
        match error["code"].as_i64() {
            Some(code) => format!("{message} (code {code})"),
            None => message.to_owned(),
        }
    }
}

/// The `ws://` URL for `address`, refusing schemes this transport cannot dial.
pub fn websocket_url(address: &str) -> Result<String> {
    let address = address.trim();
    ensure!(!address.is_empty(), "enter a machine address");
    let (scheme, rest) = match address.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => (String::new(), address),
    };
    match scheme.as_str() {
        "ws" => Ok(format!("ws://{rest}")),
        "wss" | "https" => bail!(
            "{address}: the gateway has no TLS listener; dial its plain `ws://` address over a private network instead"
        ),
        "http" => Ok(format!("ws://{rest}")),
        "" => Ok(format!("ws://{address}")),
        other => bail!("{address}: `{other}://` is not a machine address"),
    }
}

/// What `TcpStream::connect` is given: the address with the default gateway
/// port filled in, and no scheme or path.
pub fn dial_target(address: &str) -> Result<String> {
    let address = address.trim();
    let address = address.split_once("://").map_or(address, |(_, rest)| rest);
    // `rest` may still carry a path (`ws://host:1/`); the gateway serves `/`.
    let address = address.split('/').next().unwrap_or(address);
    let host = host_part(address);
    ensure!(!host.is_empty(), "enter a machine address, not a port");
    ensure!(
        host.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')),
        "{address} is not a machine address"
    );
    let port = port_or_default(address);
    Ok(if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    })
}

/// The port in `address`, or [`DEFAULT_PORT`].
pub fn port_or_default(address: &str) -> u16 {
    let address = address.trim();
    let address = address.split_once("://").map_or(address, |(_, rest)| rest);
    let address = address.split('/').next().unwrap_or(address);
    let port = if let Some(rest) = address.strip_prefix('[') {
        rest.split_once("]:").map(|(_, port)| port)
    } else {
        address
            .rsplit_once(':')
            .filter(|(host, _)| !host.contains(':'))
            .map(|(_, port)| port)
    };
    port.and_then(|port| port.parse().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PORT)
}

/// The host of `[v6]:port`, `host:port`, `[v6]` or `host`, without brackets.
pub fn host_part(address: &str) -> &str {
    let address = address.trim();
    if let Some(rest) = address.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            host
        }
        _ => address,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_websocket_urls_and_dial_targets() {
        for (address, url, target) in [
            (
                "192.168.1.20:17879",
                "ws://192.168.1.20:17879",
                "192.168.1.20:17879",
            ),
            ("192.168.1.20", "ws://192.168.1.20", "192.168.1.20:17879"),
            ("studio.local", "ws://studio.local", "studio.local:17879"),
            ("[fd00::1]:9000", "ws://[fd00::1]:9000", "[fd00::1]:9000"),
            ("[fd00::1]", "ws://[fd00::1]", "[fd00::1]:17879"),
            (
                "ws://studio.local:17879/",
                "ws://studio.local:17879/",
                "studio.local:17879",
            ),
            (
                "http://studio.local",
                "ws://studio.local",
                "studio.local:17879",
            ),
        ] {
            assert_eq!(websocket_url(address).unwrap(), url, "{address}");
            assert_eq!(dial_target(address).unwrap(), target, "{address}");
        }
    }

    #[test]
    fn a_tls_address_is_refused_with_a_reason() {
        for address in ["wss://studio.local:17879", "https://studio.local"] {
            let error = websocket_url(address).unwrap_err();
            assert!(
                format!("{error:#}").contains("no TLS listener"),
                "{error:#}"
            );
        }
        assert!(websocket_url("ftp://studio").is_err());
        assert!(websocket_url("  ").is_err());
    }

    #[test]
    fn ports_default_and_parse() {
        assert_eq!(port_or_default("studio"), DEFAULT_PORT);
        assert_eq!(port_or_default("studio:9000"), 9000);
        assert_eq!(port_or_default("[fd00::1]:9000"), 9000);
        assert_eq!(port_or_default("[fd00::1]"), DEFAULT_PORT);
        assert_eq!(port_or_default("fd00::1"), DEFAULT_PORT);
        // A port that is not a number is not a port; the default is used.
        assert_eq!(port_or_default("studio:http"), DEFAULT_PORT);
        assert_eq!(port_or_default("studio:0"), DEFAULT_PORT);
    }
}
