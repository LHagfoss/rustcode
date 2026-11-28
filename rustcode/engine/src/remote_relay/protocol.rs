//! The relay's wire format: one JSON object per line, before the splice.
//!
//! A member's control connection speaks it over TLS pinned to the relay's
//! certificate for as long as the member stays registered. A dial and a pipe
//! speak it in the clear for one or two lines, after which the connection is
//! a raw byte pipe carrying the gateway's own pinned TLS end to end.

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The relay protocol version a member or client names in its first frame.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest frame either side accepts, newline included.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Separates relay signatures from anything else the same key signs.
pub const SIGNATURE_DOMAIN: &str = "rustcode-relay-v1";

/// What a member or client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// A machine registers to be reachable through the relay.
    Register {
        version: u32,
        secret: String,
        machine_id: String,
        public_key: String,
    },
    /// A paired device asks for a pipe to a registered machine.
    Dial {
        version: u32,
        machine_id: String,
        public_key: String,
    },
    /// A member answers an [`Reply::Open`] with a fresh connection.
    Pipe { pipe: String },
    /// The signature of [`signed_message`] for the challenge just issued.
    Prove { signature: String },
    /// The device keys that may dial this member, replacing earlier ones.
    AllowedKeys { keys: Vec<String> },
    /// Answers [`Reply::Ping`].
    Pong,
}

/// What the relay sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Challenge {
        challenge: String,
    },
    Registered {
        relay_id: String,
    },
    /// The pipe is spliced; every byte after this line is the machine's.
    Connected,
    /// A client is waiting: open a pipe connection naming `pipe`.
    Open {
        pipe: String,
    },
    Ping,
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// Why the relay refused a frame. The connection closes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnsupportedVersion,
    BadRequest,
    Unauthorized,
    KeyMismatch,
    MachineOffline,
    NotAllowed,
    Busy,
    Timeout,
}

impl Reply {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Error {
            code,
            message: message.into(),
        }
    }
}

/// Which side of the relay a signature is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Member,
    Client,
}

impl Role {
    fn wire(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Client => "client",
        }
    }
}

/// The bytes a member or client signs to answer `challenge` from the relay
/// `relay_id` for `machine_id`: the domain, the role and each value, each
/// followed by a zero byte, so no value can shift into another.
pub fn signed_message(role: Role, relay_id: &str, challenge: &str, machine_id: &str) -> Vec<u8> {
    let mut message = Vec::new();
    for part in [
        SIGNATURE_DOMAIN,
        role.wire(),
        relay_id,
        challenge,
        machine_id,
    ] {
        message.extend_from_slice(part.as_bytes());
        message.push(0);
    }
    message
}

/// The next frame, or `None` at a clean end of stream. Reads no byte past the
/// frame's newline, so whatever follows stays in `reader` for the splice.
pub async fn read_frame<T, R>(reader: &mut R) -> Result<Option<T>>
where
    T: DeserializeOwned,
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let read = (&mut *reader)
        .take(MAX_FRAME_BYTES as u64)
        .read_until(b'\n', &mut line)
        .await
        .context("could not read a relay frame")?;
    if read == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        bail!("relay frame is too long or cut off");
    }
    Ok(Some(
        serde_json::from_slice(&line).context("relay frame is not valid")?,
    ))
}

/// Write `frame` as one line and flush it.
pub async fn write_frame<T, W>(writer: &mut W, frame: &T) -> Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

/// What a machine needs to register with a relay: where it is, the
/// fingerprint of its certificate and the secret that admits members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinLink {
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub secret: String,
}

/// The scheme of a [`JoinLink`].
pub const JOIN_SCHEME: &str = "rustcode-relay";

impl JoinLink {
    /// `host:port`, with an IPv6 host in brackets.
    pub fn address(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let rest = text
            .trim()
            .strip_prefix(JOIN_SCHEME)
            .and_then(|rest| rest.strip_prefix("://"))
            .with_context(|| format!("a relay link starts with {JOIN_SCHEME}://"))?;
        let (authority, query) = rest
            .split_once('?')
            .context("the relay link has no fingerprint or secret")?;
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, port) = bracketed
                .split_once("]:")
                .context("the relay link has no port")?;
            (host, port)
        } else {
            authority
                .rsplit_once(':')
                .context("the relay link has no port")?
        };
        let port: u16 = port.parse().context("the relay link's port is not one")?;
        let mut fingerprint = None;
        let mut secret = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("fp", value)) => fingerprint = Some(value.to_ascii_lowercase()),
                Some(("secret", value)) => secret = Some(value.to_owned()),
                _ => {}
            }
        }
        let fingerprint = fingerprint.context("the relay link has no fingerprint")?;
        if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("the relay link's fingerprint is not one");
        }
        let secret = secret
            .filter(|secret| !secret.is_empty())
            .context("the relay link has no secret")?;
        if host.is_empty() {
            bail!("the relay link has no host");
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            fingerprint,
            secret,
        })
    }
}

impl std::fmt::Display for JoinLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{JOIN_SCHEME}://{}?fp={}&secret={}",
            self.address(),
            self.fingerprint,
            self.secret
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_link_reads_back_as_written() {
        for host in ["relay.example.com", "203.0.113.4", "2001:db8::1"] {
            let link = JoinLink {
                host: host.to_owned(),
                port: 17880,
                fingerprint: "ab".repeat(32),
                secret: "s3cret".to_owned(),
            };
            assert_eq!(JoinLink::parse(&link.to_string()).unwrap(), link);
        }
    }

    #[test]
    fn a_join_link_without_a_fingerprint_or_secret_is_refused() {
        assert!(JoinLink::parse("rustcode-relay://h:1?secret=x").is_err());
        assert!(JoinLink::parse(&format!("rustcode-relay://h:1?fp={}", "ab".repeat(32))).is_err());
        assert!(JoinLink::parse("https://h:1?fp=ab&secret=x").is_err());
    }

    #[test]
    fn the_signed_parts_cannot_shift_into_each_other() {
        assert_ne!(
            signed_message(Role::Client, "ab", "c", "d"),
            signed_message(Role::Client, "a", "bc", "d")
        );
        assert_ne!(
            signed_message(Role::Client, "r", "c", "m"),
            signed_message(Role::Member, "r", "c", "m")
        );
    }

    #[tokio::test]
    async fn a_frame_leaves_the_bytes_after_it_unread() {
        let mut bytes = serde_json::to_vec(&Request::Pong).unwrap();
        bytes.extend_from_slice(b"\n\x16tls");
        let mut reader = tokio::io::BufReader::new(&bytes[..]);
        let frame: Request = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(frame, Request::Pong);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"\x16tls");
    }

    #[tokio::test]
    async fn an_overlong_frame_is_refused() {
        let bytes = vec![b'x'; MAX_FRAME_BYTES + 10];
        let mut reader = tokio::io::BufReader::new(&bytes[..]);
        assert!(read_frame::<Request, _>(&mut reader).await.is_err());
    }
}
