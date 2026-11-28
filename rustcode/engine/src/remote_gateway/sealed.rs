//! Sealed connections: every frame encrypted end to end with a Noise NK
//! session keyed to the gateway's static X25519 key ([`super::noise`]).
//!
//! A device that has the gateway's `sealed_key` (from the pairing QR code or,
//! trusted on first use, a `described` frame) sends `seal` as its first frame:
//!
//! ```json
//! {"type":"seal","protocol":"Noise_NK_25519_ChaChaPoly_SHA256","message":"<base64url>"}
//! ```
//!
//! and the gateway answers `{"type":"sealed","message":"<base64url>"}`. From
//! then on every text frame in either direction is the unpadded base64url of
//! one Noise transport message whose plaintext is the frame the plain
//! protocol would have sent. Pings, pongs and close frames are not sealed.
//! The handshake payloads are empty.
//!
//! The key lives in `remote/sealed.json` (owner-only). A new key breaks every
//! sealed pairing until the device pairs again, as a new TLS certificate does.

use super::noise::{self, KeyPair, PROLOGUE, PROTOCOL_NAME, Transport};
use crate::daemon::private_files::{publish_private_json, read_private_json};
use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{Sink, Stream};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll, ready};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// The gateway's static key, as `remote/sealed.json` holds it.
#[derive(Serialize, Deserialize)]
struct Stored {
    private_key: String,
}

/// The gateway's static X25519 key.
pub struct SealedIdentity {
    key: KeyPair,
    public: String,
}

impl SealedIdentity {
    /// Read the key at `path`, creating it when there is none. A file that
    /// exists but cannot be read stops the gateway rather than replacing the
    /// key every sealed pairing knows.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let stored = match read_private_json::<Stored>(path, 4 * 1024) {
            Ok(stored) => stored,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                let key = KeyPair::generate();
                let stored = Stored {
                    private_key: URL_SAFE_NO_PAD.encode(key.private()),
                };
                publish_private_json(path, &stored)?;
                stored
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read the gateway's sealed key {}; remove it to make a new one \
                         (devices that sealed with the old one must pair again)",
                        path.display()
                    )
                });
            }
        };
        let private: [u8; 32] = URL_SAFE_NO_PAD
            .decode(stored.private_key.trim())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .with_context(|| format!("the sealed key {} is not one", path.display()))?;
        Ok(Self::from_key(KeyPair::from_private(private)?))
    }

    /// A fresh key that is never written anywhere, for tests.
    #[cfg(test)]
    pub fn ephemeral() -> Self {
        Self::from_key(KeyPair::generate())
    }

    fn from_key(key: KeyPair) -> Self {
        let public = URL_SAFE_NO_PAD.encode(key.public());
        Self { key, public }
    }

    /// The public key as devices hold it: unpadded base64url of 32 bytes.
    pub fn public_key(&self) -> &str {
        &self.public
    }

    /// Answer a `seal` frame's handshake message: the `sealed` frame to send
    /// and the session's transport.
    pub fn accept(&self, hello: &SealHello) -> Result<(String, Transport)> {
        anyhow::ensure!(
            hello.protocol == PROTOCOL_NAME,
            "the sealed protocol {} is not {PROTOCOL_NAME}",
            hello.protocol
        );
        let message = URL_SAFE_NO_PAD
            .decode(hello.message.trim())
            .context("the sealed handshake message is not base64url")?;
        let responded = noise::respond(&self.key, PROLOGUE, &message, &[])?;
        let answer = serde_json::to_string(&SealAnswer {
            kind: SealAnswerKind::Sealed,
            message: URL_SAFE_NO_PAD.encode(&responded.message),
        })?;
        Ok((answer, responded.transport))
    }
}

/// A device's `seal` frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealHello {
    #[serde(rename = "type")]
    kind: SealHelloKind,
    pub protocol: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SealHelloKind {
    Seal,
}

impl SealHello {
    /// `text` as a `seal` frame, when it is one.
    pub fn parse(text: &str) -> Option<Self> {
        if !text.contains("\"seal\"") {
            return None;
        }
        serde_json::from_str(text).ok()
    }

    #[cfg(test)]
    pub fn new(message: &[u8]) -> Self {
        Self {
            kind: SealHelloKind::Seal,
            protocol: PROTOCOL_NAME.to_owned(),
            message: URL_SAFE_NO_PAD.encode(message),
        }
    }
}

/// The gateway's `sealed` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealAnswer {
    #[serde(rename = "type")]
    kind: SealAnswerKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SealAnswerKind {
    Sealed,
}

/// A WebSocket that seals its text frames once [`Self::seal`] is called and
/// passes them through before.
pub struct Sealed<S> {
    inner: S,
    transport: Option<Transport>,
}

impl<S> Sealed<S> {
    pub fn plain(inner: S) -> Self {
        Self {
            inner,
            transport: None,
        }
    }

    /// Seal every text frame from here on.
    pub fn seal(&mut self, transport: Transport) {
        self.transport = Some(transport);
    }

    pub fn is_sealed(&self) -> bool {
        self.transport.is_some()
    }
}

fn broken(why: &'static str) -> WsError {
    WsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, why))
}

fn open(transport: &mut Transport, text: &str) -> Result<Message, WsError> {
    let sealed = URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|_| broken("a sealed frame is not base64url"))?;
    let plain = transport
        .receive
        .decrypt(&sealed)
        .map_err(|_| broken("a sealed frame did not authenticate"))?;
    let text = String::from_utf8(plain).map_err(|_| broken("a sealed frame is not UTF-8"))?;
    Ok(Message::text(text))
}

impl<S> Stream for Sealed<S>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin,
{
    type Item = Result<Message, WsError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let next = ready!(Pin::new(&mut this.inner).poll_next(cx));
        Poll::Ready(match (this.transport.as_mut(), next) {
            (Some(transport), Some(Ok(Message::Text(text)))) => {
                Some(open(transport, text.as_str()))
            }
            (Some(_), Some(Ok(Message::Binary(_)))) => {
                Some(Err(broken("a sealed connection carries no binary frames")))
            }
            (_, next) => next,
        })
    }
}

impl<S> Sink<Message> for Sealed<S>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    type Error = WsError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.get_mut().inner).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), WsError> {
        let this = self.get_mut();
        let item = match (this.transport.as_mut(), item) {
            (Some(transport), Message::Text(text)) => {
                let sealed = transport
                    .send
                    .encrypt(text.as_str().as_bytes())
                    .map_err(|_| broken("could not seal a frame"))?;
                Message::text(URL_SAFE_NO_PAD.encode(sealed))
            }
            (Some(_), Message::Binary(_)) => {
                return Err(broken("a sealed connection carries no binary frames"));
            }
            (_, item) => item,
        };
        Pin::new(&mut this.inner).start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Result<(), WsError>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

/// `payload` with `sealed_key` added: the QR code a device seals with.
pub fn qr_with_sealed_key(payload: &str, sealed_key: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(serde_json::Value::Object(mut fields)) => {
            fields.insert(
                "sealed_key".to_owned(),
                serde_json::Value::String(sealed_key.to_owned()),
            );
            serde_json::Value::Object(fields).to_string()
        }
        _ => payload.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    #[test]
    fn the_key_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote").join("sealed.json");
        let first = SealedIdentity::load_or_create(&path).unwrap();
        let again = SealedIdentity::load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), again.public_key());
        assert_eq!(
            URL_SAFE_NO_PAD.decode(first.public_key()).unwrap().len(),
            32
        );
    }

    #[test]
    fn an_unreadable_key_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sealed.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(SealedIdentity::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
    }

    #[test]
    fn a_seal_frame_parses_and_other_frames_do_not() {
        let hello = SealHello::new(b"m");
        let text = serde_json::to_string(&hello).unwrap();
        assert_eq!(SealHello::parse(&text), Some(hello));
        assert_eq!(SealHello::parse(r#"{"type":"describe"}"#), None);
        assert_eq!(SealHello::parse(r#"{"type":"sealed","message":"x"}"#), None);
    }

    #[test]
    fn the_qr_code_gains_the_sealed_key() {
        let payload = r#"{"protocol_version":1,"address":"a:1","gateway_id":"g","credential":"c"}"#;
        let sealed: serde_json::Value =
            serde_json::from_str(&qr_with_sealed_key(payload, "KEY")).unwrap();
        assert_eq!(sealed["sealed_key"], "KEY");
        assert_eq!(sealed["gateway_id"], "g");
    }

    #[tokio::test]
    async fn frames_round_trip_sealed_and_cannot_be_read_on_the_wire() {
        let identity = SealedIdentity::ephemeral();
        let public = URL_SAFE_NO_PAD.decode(identity.public_key()).unwrap();
        let (initiator, first) = noise::Initiator::start(&public, PROLOGUE, &[]).unwrap();
        let (answer, host_transport) = identity.accept(&SealHello::new(&first)).unwrap();
        let answer: SealAnswer = serde_json::from_str(&answer).unwrap();
        let (_, device_transport) = initiator
            .finish(&URL_SAFE_NO_PAD.decode(&answer.message).unwrap())
            .unwrap();

        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = futures_util::sink::unfold(
            std::sync::Arc::clone(&captured),
            |captured, message: Message| async move {
                captured.lock().unwrap().push(message);
                Ok::<_, WsError>(captured)
            },
        );
        let mut device = Sealed::plain(Box::pin(sink));
        device.seal(device_transport);
        device
            .send(Message::text(r#"{"type":"authenticate"}"#))
            .await
            .unwrap();

        let on_the_wire = captured.lock().unwrap().remove(0);
        let Message::Text(on_the_wire) = on_the_wire else {
            panic!("a sealed frame is text");
        };
        assert!(!on_the_wire.as_str().contains("authenticate"));

        let mut host = Sealed::plain(futures_util::stream::iter(vec![Ok::<_, WsError>(
            Message::Text(on_the_wire),
        )]));
        host.seal(host_transport);
        let opened = host.next().await.unwrap().unwrap();
        assert_eq!(opened, Message::text(r#"{"type":"authenticate"}"#));
    }
}
