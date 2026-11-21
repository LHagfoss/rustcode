//! The WebSocket transport: one connection to a machine's remote gateway.
//!
//! The gateway refuses browser `Origin`s and expects the handshake as the
//! first text frame, so this dials with `tokio-tungstenite` from Rust, sends
//! at most one handshake frame per step, and then either hands back a
//! [`Connection`] for plain remote-protocol frames or switches the socket to
//! ACP ([`AcpChannel`]).
//!
//! The gateway port answers plain `ws://` and its own TLS with a self-signed
//! certificate (`docs/remote-security.md` › Direct TLS). A machine whose
//! certificate fingerprint is known is dialled with TLS pinned to it
//! ([`Connection::open_pinned`]); one whose fingerprint is not known yet is
//! dialled plain, and the overlay, if any, is what encrypts it. A `wss://`
//! address is a machine's public address, a tunnel (cloudflared) to its
//! engine's HTTP listener, which relays `/ws` to the gateway: TLS runs to the
//! tunnel's edge, verified against the operating system's trust store.

use crate::protocol::{
    AcpClientFrame, AcpServerFrame, HandshakeRequest, HandshakeResponse, MAX_HANDSHAKE_FRAME_BYTES,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{WebSocketStream, client_async_with_config, tungstenite::Message};

/// Default TCP port of a machine's gateway.
pub const DEFAULT_PORT: u16 = 17879;

/// Port of a `wss://` address that names none.
pub const TLS_PORT: u16 = 443;

/// Where a machine's public address serves the gateway's WebSocket.
pub const PUBLIC_WS_PATH: &str = "/ws";

/// Frames larger than this are refused by the gateway; refuse them here too,
/// so a broken peer cannot make the client allocate without bound.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long a TCP connect and the WebSocket upgrade together may take. The
/// route racing in step 1.8 keeps its own, much shorter, per-route deadline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The byte stream under the WebSocket: plain TCP, or TLS to a tunnel's edge.
trait Stream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Stream for T {}

type Socket = WebSocketStream<Box<dyn Stream>>;

/// One TLS configuration for the process, built on first use.
fn tls_connector() -> Result<tokio_rustls::TlsConnector> {
    static CONFIG: OnceLock<Result<Arc<tokio_rustls::rustls::ClientConfig>, String>> =
        OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        use rustls_platform_verifier::BuilderVerifierExt;
        use tokio_rustls::rustls;
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .and_then(|builder| builder.with_platform_verifier())
            .map(|builder| Arc::new(builder.with_no_client_auth()))
            .map_err(|error| error.to_string())
    });
    match config {
        Ok(config) => Ok(tokio_rustls::TlsConnector::from(Arc::clone(config))),
        Err(error) => bail!("TLS is unavailable on this computer: {error}"),
    }
}

/// TLS pinned to one certificate: the SHA-256 of its DER must be `fingerprint`
/// (lowercase hex). Names, dates and issuers are not checked, because the
/// gateway's certificate is self-signed and only its fingerprint vouches for
/// it; the handshake signature still proves the peer holds the key.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<tokio_rustls::rustls::crypto::CryptoProvider>,
}

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        if certificate_fingerprint(end_entity) == self.fingerprint {
            Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The SHA-256 of a certificate's DER, lowercase hex: what a machine's
/// `tls_fingerprint` names.
pub fn certificate_fingerprint(der: &[u8]) -> String {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, der);
    hex::encode(digest.as_ref())
}

/// Whether `fingerprint` reads as one: 64 lowercase hex digits.
pub fn is_fingerprint(fingerprint: &str) -> bool {
    fingerprint.len() == 64
        && fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn pinned_connector(fingerprint: &str) -> Result<tokio_rustls::TlsConnector> {
    use tokio_rustls::rustls;
    ensure!(
        is_fingerprint(fingerprint),
        "the saved certificate fingerprint is not one; pair again"
    );
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = Arc::new(Pinned {
        fingerprint: fingerprint.to_owned(),
        provider: Arc::clone(&provider),
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| anyhow::anyhow!("TLS is unavailable on this computer: {error}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(config)))
}

/// The name a pinned dial presents. The gateway serves one certificate
/// whatever the name, and the pin, not the name, is what is checked.
const PINNED_SERVER_NAME: &str = "rustcode-gateway";

/// How the socket under the WebSocket is secured.
#[derive(Clone, Copy)]
enum Security<'a> {
    Plain,
    /// `wss://`: the operating system's trust store, for a public address.
    Platform,
    /// The gateway's own certificate, by fingerprint.
    Pinned(&'a str),
}

/// Whether `address` is a TLS (`wss://` or `https://`) address: a machine's
/// public address behind a tunnel.
pub fn is_tls(address: &str) -> bool {
    address
        .trim()
        .split_once("://")
        .is_some_and(|(scheme, _)| matches!(scheme.to_ascii_lowercase().as_str(), "wss" | "https"))
}

/// An open WebSocket to a machine's gateway, before or after authentication.
pub struct Connection {
    socket: Socket,
    /// The challenge this socket's `described` frame carried, for a machine
    /// that takes device keys; spent by the next `authenticate`.
    challenge: Option<String>,
    /// The certificate fingerprint this socket's TLS was pinned to, when it
    /// was dialled with [`Connection::open_pinned`].
    pinned: Option<String>,
}

impl Connection {
    /// The certificate fingerprint this socket is pinned to: `None` on plain
    /// `ws://` and on a `wss://` public address.
    pub fn pinned(&self) -> Option<&str> {
        self.pinned.as_deref()
    }

    /// Keep the challenge the machine's `described` frame carried.
    pub fn set_challenge(&mut self, challenge: Option<String>) {
        self.challenge = challenge;
    }

    /// The challenge from this socket's `described` frame, if the machine
    /// takes device keys.
    pub fn challenge(&self) -> Option<&str> {
        self.challenge.as_deref()
    }

    /// Dial `address` and complete the WebSocket upgrade.
    ///
    /// `address` is `host:port`, a bare host (the default port), `[v6]:port`
    /// or an explicit `ws://` URL.
    pub async fn open(address: &str) -> Result<Self> {
        Self::open_with_timeout(address, CONNECT_TIMEOUT).await
    }

    pub async fn open_with_timeout(address: &str, timeout: Duration) -> Result<Self> {
        let security = if is_tls(address) {
            Security::Platform
        } else {
            Security::Plain
        };
        Self::open_secured(address, timeout, security).await
    }

    /// Dial `address` with TLS pinned to `fingerprint`, the gateway's own
    /// certificate. Nothing is retried in the clear: a machine that pinned
    /// TLS and now answers plain, or with another certificate, is refused.
    /// A `wss://` address keeps the trust store; the pin is the gateway's
    /// and the tunnel's edge has its own certificate.
    ///
    /// `expected` names the certificate in the refusal, as in "did not answer
    /// with {expected}", and `hint` (empty for none) follows it: where the pin
    /// came from decides what the owner can do.
    pub async fn open_pinned(
        address: &str,
        fingerprint: &str,
        expected: &str,
        hint: &str,
        timeout: Duration,
    ) -> Result<Self> {
        if is_tls(address) {
            return Self::open_with_timeout(address, timeout).await;
        }
        Self::open_secured(address, timeout, Security::Pinned(fingerprint))
            .await
            .with_context(|| {
                format!("{address} did not answer with {expected}, so nothing was sent{hint}")
            })
    }

    async fn open_secured(
        address: &str,
        timeout: Duration,
        security: Security<'_>,
    ) -> Result<Self> {
        let url = websocket_url(address)?;
        let target = dial_target(address)?;
        let socket = tokio::time::timeout(timeout, TcpStream::connect(target))
            .await
            .with_context(|| format!("connecting to {address} took longer than {timeout:?}"))?
            .with_context(|| format!("could not connect to {address}"))?;
        // A prompt's worth of output is small; do not wait on Nagle for it.
        let _ = socket.set_nodelay(true);
        let (connector, name) = match security {
            Security::Plain => (None, None),
            Security::Platform => {
                let host = host_part(address).to_owned();
                let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host)
                    .with_context(|| format!("{address} is not a host name TLS can verify"))?;
                (Some(tls_connector()?), Some(name))
            }
            Security::Pinned(fingerprint) => (
                Some(pinned_connector(fingerprint)?),
                Some(tokio_rustls::rustls::pki_types::ServerName::try_from(
                    PINNED_SERVER_NAME,
                )?),
            ),
        };
        let pinned = match security {
            Security::Pinned(fingerprint) => Some(fingerprint.to_owned()),
            _ => None,
        };
        let socket: Box<dyn Stream> = match connector.zip(name) {
            Some((connector, name)) => {
                let tls = tokio::time::timeout(timeout, connector.connect(name, socket))
                    .await
                    .with_context(|| format!("{address} did not finish the TLS handshake in time"))?
                    .with_context(|| format!("the TLS handshake with {address} failed"))?;
                Box::new(tls)
            }
            None => Box::new(socket),
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (socket, _response) =
            tokio::time::timeout(timeout, client_async_with_config(url, socket, Some(config)))
                .await
                .with_context(|| format!("{address} did not answer the WebSocket upgrade in time"))?
                .with_context(|| format!("{address} refused the WebSocket upgrade"))?;
        Ok(Self {
            socket,
            challenge: None,
            pinned,
        })
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
    /// notifications and server-to-client requests that arrive first. The
    /// whole wait runs against one deadline.
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        let frame =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_json(&frame).await?;
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            let frame = self
                .next_json_by(deadline)
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
                last_heard: tokio::time::Instant::now(),
            }),
            Err(_) => {
                // The gateway refuses with an `error` frame: ACP access is a
                // grant the machine's owner gives, and a relay needs an engine
                // to relay to, so name which one is missing.
                match serde_json::from_str::<HandshakeResponse>(&answer) {
                    Ok(response) => Err(crate::machine::handshake_failure("acp_open", response)),
                    Err(_) => bail!("the machine did not confirm acp_open"),
                }
            }
        }
    }

    /// Switch the authenticated socket to the hub link (#3957): send
    /// `hub_join` with the access this machine grants the hub (an
    /// outside-agent level such as `delegate`), wait for `hub_joined`, and
    /// return the channel the hub's calls arrive on. The hub refuses with an
    /// `error` frame when it is not the fleet's hub or this device is not
    /// another computer.
    pub async fn hub_open(self, access: &str, ask: bool) -> Result<AcpChannel> {
        let mut connection = self;
        let join = serde_json::json!({"type": "hub_join", "access": access, "ask": ask});
        connection.send_json(&join).await.context("send hub_join")?;
        let answer = connection.next_text(Some(CONNECT_TIMEOUT)).await?;
        let joined = serde_json::from_str::<Value>(&answer)
            .ok()
            .is_some_and(|frame| frame.get("type").and_then(Value::as_str) == Some("hub_joined"));
        if joined {
            return Ok(AcpChannel {
                socket: connection.socket,
                last_heard: tokio::time::Instant::now(),
            });
        }
        match serde_json::from_str::<HandshakeResponse>(&answer) {
            Ok(response) => Err(crate::machine::handshake_failure("hub_join", response)),
            Err(_) => bail!("the machine did not confirm hub_join"),
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
        self.next_json_by(tokio::time::Instant::now() + timeout)
            .await
    }

    async fn next_json_by(&mut self, deadline: tokio::time::Instant) -> Result<Option<Value>> {
        match self
            .next_text_or_close(Some((deadline, CONNECT_TIMEOUT)))
            .await?
        {
            Some(text) => Ok(Some(
                serde_json::from_str(&text).context("the machine sent a frame that is not JSON")?,
            )),
            None => Ok(None),
        }
    }

    async fn next_text(&mut self, timeout: Option<Duration>) -> Result<String> {
        let deadline = timeout.map(|timeout| (tokio::time::Instant::now() + timeout, timeout));
        self.next_text_or_close(deadline)
            .await?
            .context("the machine closed the connection")
    }

    /// The next text frame; `None` when the socket closed. Pings are answered
    /// by tungstenite, and the gateway's pongs are read here.
    ///
    /// `deadline` is absolute for the whole read: a ping every 30 s must not
    /// restart the wait, or a peer that only pings is never timed out.
    async fn next_text_or_close(
        &mut self,
        deadline: Option<(tokio::time::Instant, Duration)>,
    ) -> Result<Option<String>> {
        loop {
            let next = match deadline {
                Some((deadline, timeout)) => tokio::time::timeout_at(deadline, self.socket.next())
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

const NOT_JSON: &str = "the machine sent a frame that is not JSON";

/// A JSON-RPC channel over a socket that is now carrying ACP.
///
/// One object per text frame. The machine sends notifications
/// (`session/update`) and server-to-client requests (approvals, questions)
/// on the same channel as answers, so [`Self::recv`] returns every frame and
/// [`Self::request`] filters for the one it asked for.
pub struct AcpChannel {
    socket: Socket,
    /// When anything last arrived from the machine: a frame, a ping, a pong.
    /// The gateway pings every 30 s, so a long silence is a dead link.
    last_heard: tokio::time::Instant,
}

impl AcpChannel {
    /// When anything last arrived from the machine, pings included. A relay
    /// that hears nothing for well past the gateway's ping interval holds a
    /// connection the network dropped without a close.
    pub fn last_heard(&self) -> tokio::time::Instant {
        self.last_heard
    }

    /// Ask the machine to answer now: its pong moves [`Self::last_heard`]. A
    /// relay sends one after a wake from sleep or a network change, when the
    /// socket may be dead without having said so.
    pub async fn ping(&mut self) -> Result<()> {
        self.socket
            .send(Message::Ping(Default::default()))
            .await
            .context("ping the machine")
    }

    /// Send one JSON-RPC frame, request or notification.
    pub async fn send(&mut self, frame: Value) -> Result<()> {
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .context("send")
    }

    /// The next ACP frame, or `None` when the machine closed the channel.
    ///
    /// This waits at most [`CONNECT_TIMEOUT`]: it is for a round trip. A
    /// connection that stays open and relays frames uses [`Self::next`].
    pub async fn recv(&mut self) -> Result<Option<Value>> {
        self.recv_by(Some(tokio::time::Instant::now() + CONNECT_TIMEOUT))
            .await
    }

    /// The next ACP frame, waiting as long as the machine keeps the channel
    /// open; `None` when it closed it.
    ///
    /// This is what a relay reads: a quiet thread sends nothing for minutes at
    /// a time, and the gateway's pings must not end the wait.
    pub async fn next(&mut self) -> Result<Option<Value>> {
        self.recv_by(None).await
    }

    /// The next frame before `deadline`; pings do not restart the wait.
    async fn recv_by(&mut self, deadline: Option<tokio::time::Instant>) -> Result<Option<Value>> {
        let Some(text) = self.text_by(deadline).await? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&text).context(NOT_JSON)?))
    }

    /// The next frame's text before `deadline`, as the machine wrote it.
    async fn text_by(&mut self, deadline: Option<tokio::time::Instant>) -> Result<Option<String>> {
        loop {
            let next = match deadline {
                Some(deadline) => tokio::time::timeout_at(deadline, self.socket.next())
                    .await
                    .with_context(|| {
                        format!("the machine stopped sending ACP frames for {CONNECT_TIMEOUT:?}")
                    })?,
                None => self.socket.next().await,
            };
            if matches!(next, Some(Ok(_))) {
                self.last_heard = tokio::time::Instant::now();
            }
            match next {
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(text.as_str().to_owned()));
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

    /// The next ACP frame as the machine wrote it, for a relay that passes
    /// frames on without reading them: checked to be JSON, never built into a
    /// tree and written out again. Waits like [`Self::next`].
    pub async fn next_text(&mut self) -> Result<Option<String>> {
        let Some(text) = self.text_by(None).await? else {
            return Ok(None);
        };
        serde_json::from_str::<serde::de::IgnoredAny>(&text).context(NOT_JSON)?;
        Ok(Some(text))
    }

    /// Send one JSON-RPC frame that is already text. Text that is not JSON is
    /// refused here, since the machine closes the connection on it.
    pub async fn send_text(&mut self, frame: String) -> Result<()> {
        serde_json::from_str::<serde::de::IgnoredAny>(&frame)
            .context("the frame to send is not JSON")?;
        self.socket.send(Message::text(frame)).await.context("send")
    }

    /// Send a request and return its answer, skipping notifications and
    /// server-to-client requests that arrive first. The whole wait runs
    /// against one deadline, so a stream of notifications cannot keep a
    /// missing answer alive.
    pub async fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .await?;
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            let frame = self
                .recv_by(Some(deadline))
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

/// The WebSocket URL for `address`, refusing schemes this transport cannot
/// dial. A `wss://` (or `https://`) address without a path dials
/// [`PUBLIC_WS_PATH`], where a machine's public address serves the gateway.
pub fn websocket_url(address: &str) -> Result<String> {
    let address = address.trim();
    ensure!(!address.is_empty(), "enter a machine address");
    let (scheme, rest) = match address.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => (String::new(), address),
    };
    match scheme.as_str() {
        "ws" => Ok(format!("ws://{rest}")),
        "wss" | "https" => {
            let has_path = rest.find('/').is_some_and(|slash| slash + 1 < rest.len());
            let host = rest.trim_end_matches('/');
            Ok(if has_path {
                format!("wss://{rest}")
            } else {
                format!("wss://{host}{PUBLIC_WS_PATH}")
            })
        }
        "http" => Ok(format!("ws://{rest}")),
        "" => Ok(format!("ws://{address}")),
        other => bail!("{address}: `{other}://` is not a machine address"),
    }
}

/// `address` with its port spelled out, `host:port` or `[v6]:port`, the way the
/// gateway reports its own routes. An address that already names a port, or
/// carries a scheme or a path, is returned as it was.
pub fn with_explicit_port(address: &str) -> String {
    let trimmed = address.trim();
    let names_port = if let Some(rest) = trimmed.strip_prefix('[') {
        rest.contains("]:")
    } else {
        trimmed.rsplit_once(':').is_some_and(|(host, port)| {
            !host.contains(':') && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
        })
    };
    let host = host_part(trimmed);
    if names_port || trimmed.contains("://") || trimmed.contains('/') || host.is_empty() {
        return address.to_owned();
    }
    let port = port_or_default(trimmed);
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// What `TcpStream::connect` is given: the address with the default gateway
/// port filled in, and no scheme or path.
pub fn dial_target(address: &str) -> Result<String> {
    let address = address.trim();
    // The scheme picks the default port (`wss://` is 443), so read it first.
    let port = port_or_default(address);
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
    Ok(if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    })
}

/// The port in `address`, or [`TLS_PORT`] for a `wss://` address and
/// [`DEFAULT_PORT`] for any other.
pub fn port_or_default(address: &str) -> u16 {
    let original = address;
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
        .unwrap_or(if is_tls(original) {
            TLS_PORT
        } else {
            DEFAULT_PORT
        })
}

/// The host of `[v6]:port`, `host:port`, `[v6]` or `host`, without brackets,
/// scheme or path (`ws://studio.local:17879/` is `studio.local`).
pub fn host_part(address: &str) -> &str {
    let address = address.trim();
    let address = address.split_once("://").map_or(address, |(_, rest)| rest);
    let address = address.split('/').next().unwrap_or(address);
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
    fn a_tls_address_dials_the_public_listener() {
        for (address, url, target) in [
            (
                "wss://rustcode.example.com",
                "wss://rustcode.example.com/ws",
                "rustcode.example.com:443",
            ),
            (
                "https://rustcode.example.com/",
                "wss://rustcode.example.com/ws",
                "rustcode.example.com:443",
            ),
            (
                "wss://rustcode.example.com:8443/ws",
                "wss://rustcode.example.com:8443/ws",
                "rustcode.example.com:8443",
            ),
        ] {
            assert!(is_tls(address), "{address}");
            assert_eq!(websocket_url(address).unwrap(), url, "{address}");
            assert_eq!(dial_target(address).unwrap(), target, "{address}");
        }
        assert!(!is_tls("ws://studio.local"));
        assert!(!is_tls("studio.local:17879"));
        assert!(websocket_url("ftp://studio").is_err());
        assert!(websocket_url("  ").is_err());
    }

    /// A certificate the operating system does not trust ends the dial: a
    /// public route never falls back to a connection it cannot verify.
    #[tokio::test]
    async fn an_untrusted_certificate_is_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            // Not a TLS server at all: whatever it answers cannot verify.
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncWriteExt;
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        });
        let error = Connection::open_with_timeout(
            &format!("wss://localhost:{port}"),
            Duration::from_secs(5),
        )
        .await
        .err()
        .expect("an unverifiable server is refused");
        assert!(format!("{error:#}").contains("TLS handshake"), "{error:#}");
        server.abort();
    }

    /// The wait for one frame is one deadline, not one per frame: a peer that
    /// sends only pings must still be timed out.
    #[tokio::test]
    async fn a_pinging_peer_does_not_extend_a_read_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let until = tokio::time::Instant::now() + Duration::from_secs(4);
            while tokio::time::Instant::now() < until {
                if socket
                    .send(Message::Ping(Default::default()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let mut connection = Connection::open(&addr.to_string()).await.unwrap();
        let started = std::time::Instant::now();
        let error = connection
            .next_handshake(Duration::from_millis(150))
            .await
            .unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            format!("{error:#}").contains("no frame from the machine"),
            "{error:#}"
        );
        assert!(
            elapsed < Duration::from_millis(900),
            "the pings restarted the deadline: waited {elapsed:?}"
        );
        server.abort();
    }

    /// A relay waits as long as the machine is quiet; pings alone do not
    /// answer it, and a closed channel ends it.
    #[tokio::test]
    async fn next_waits_through_a_quiet_channel() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            // The switch to ACP, then a quiet stretch with only pings.
            assert!(matches!(socket.next().await, Some(Ok(Message::Text(_)))));
            socket
                .send(Message::text(r#"{"type":"acp_opened"}"#))
                .await
                .unwrap();
            for _ in 0..3 {
                socket
                    .send(Message::Ping(Default::default()))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            socket
                .send(Message::text(
                    r#"{"jsonrpc":"2.0","method":"session/update"}"#,
                ))
                .await
                .unwrap();
            // The close frame, and a moment for it to reach the other side:
            // dropping the socket right after it would reset the connection.
            socket.close(None).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let mut channel = Connection::open(&addr.to_string())
            .await
            .unwrap()
            .acp_open()
            .await
            .expect("the stub opens ACP");
        let frame = channel.next().await.unwrap().expect("a frame arrives");
        assert_eq!(frame["method"], "session/update");
        assert!(channel.next().await.unwrap().is_none(), "then it closes");
        server.await.unwrap();
    }

    /// A relay passes a frame on byte for byte, in both directions, and
    /// still refuses text that is not JSON.
    #[tokio::test]
    async fn a_relayed_frame_keeps_its_text() {
        // Key order and spacing a rebuilt tree would not keep.
        const FRAME: &str =
            r#"{"method":"session/update",  "jsonrpc":"2.0","params":{"b":1,"a":2}}"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert!(matches!(socket.next().await, Some(Ok(Message::Text(_)))));
            socket
                .send(Message::text(r#"{"type":"acp_opened"}"#))
                .await
                .unwrap();
            socket.send(Message::text(FRAME)).await.unwrap();
            let Some(Ok(Message::Text(echo))) = socket.next().await else {
                panic!("the relay sends the frame back");
            };
            assert_eq!(echo.as_str(), FRAME);
            socket.send(Message::text("not json")).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let mut channel = Connection::open(&addr.to_string())
            .await
            .unwrap()
            .acp_open()
            .await
            .expect("the stub opens ACP");
        let frame = channel.next_text().await.unwrap().expect("a frame arrives");
        assert_eq!(frame, FRAME);
        assert!(channel.send_text("{oops".into()).await.is_err());
        channel.send_text(frame).await.unwrap();
        let error = channel.next_text().await.unwrap_err();
        assert!(error.to_string().contains("not JSON"), "{error}");
        server.await.unwrap();
    }

    /// A relay with no engine behind it says which stage failed: a machine
    /// with nothing to relay to is not the same as one that refused access,
    /// and neither reads as "not connected" (#3341).
    #[tokio::test]
    async fn acp_open_names_the_stage_that_failed() {
        for (code, expected) in [
            (
                crate::protocol::ErrorCode::Internal,
                "no RustCode engine is running",
            ),
            (
                crate::protocol::ErrorCode::UnauthorizedAccess,
                "not allowed to use that machine's threads",
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let answer = HandshakeResponse::Error {
                code,
                message: "connection closed".into(),
                retry_after_secs: None,
            }
            .to_text();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                assert!(matches!(socket.next().await, Some(Ok(Message::Text(_)))));
                socket.send(Message::text(answer)).await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            });
            let error = match Connection::open(&addr.to_string())
                .await
                .unwrap()
                .acp_open()
                .await
            {
                Ok(_) => panic!("a machine with no engine cannot open ACP"),
                Err(error) => error,
            };
            let refusal = error
                .downcast_ref::<crate::machine::Refusal>()
                .expect("the refusal keeps its code");
            assert_eq!(refusal.code, code);
            assert_eq!(refusal.step, "acp_open");
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
            server.await.unwrap();
        }
    }

    #[test]
    fn a_host_is_read_without_its_scheme_or_path() {
        for (address, host) in [
            ("studio.local", "studio.local"),
            ("studio.local:17879", "studio.local"),
            ("ws://studio.local:17879/", "studio.local"),
            ("http://studio.local", "studio.local"),
            ("[fd00::1]:17879", "fd00::1"),
            ("ws://[fd00::1]:17879", "fd00::1"),
        ] {
            assert_eq!(host_part(address), host, "{address}");
        }
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

    #[test]
    fn a_route_without_a_port_gets_the_default_one() {
        assert_eq!(with_explicit_port("100.90.16.237"), "100.90.16.237:17879");
        assert_eq!(
            with_explicit_port("100.90.16.237:9000"),
            "100.90.16.237:9000"
        );
        assert_eq!(with_explicit_port("studio.local"), "studio.local:17879");
        assert_eq!(with_explicit_port("fdfd::2"), "[fdfd::2]:17879");
        assert_eq!(with_explicit_port("[fdfd::2]"), "[fdfd::2]:17879");
        assert_eq!(with_explicit_port("[fdfd::2]:17879"), "[fdfd::2]:17879");
        assert_eq!(
            with_explicit_port("ws://studio.local/"),
            "ws://studio.local/"
        );
    }
}
