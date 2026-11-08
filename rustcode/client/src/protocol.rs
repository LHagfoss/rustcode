//! Remote protocol v1 as a *client* speaks it: the handshake frames and the
//! two frames that switch an authenticated socket to ACP.
//!
//! These types mirror the gateway's own
//! (`rustcode/engine/src/remote_gateway/handshake.rs` and
//! `rustcode/engine/src/remote/protocol.rs`) but live here so the client
//! crate never depends on the whole engine. The committed contract under
//! `docs/remote-protocol/v1` is what keeps the two in step: the tests in this
//! module decode every golden frame and the schema documents, so a wire change
//! on the gateway side fails here until this mirror follows it.
//!
//! Everything after [`AcpServerFrame::AcpOpened`] is ACP JSON-RPC, one object
//! per WebSocket text frame, carried opaquely as [`serde_json::Value`].

use serde::{Deserialize, Serialize};
use std::fmt;

/// Version of the handshake frames below.
pub const HANDSHAKE_PROTOCOL_VERSION: u32 = 1;

/// Largest handshake frame either side sends, matching the gateway's limit.
pub const MAX_HANDSHAKE_FRAME_BYTES: usize = 8 * 1024;

/// Digits in the pairing code a machine shows and the owner types, as the
/// gateway issues it. A `connect_approved` frame reports the same number.
pub const PAIRING_CODE_DIGITS: u32 = 8;

/// `device_kind` of the desktop app's machine client.
pub const DEVICE_KIND_DESKTOP: &str = "desktop";
/// `device_kind` of the command line's machine client.
pub const DEVICE_KIND_CLI: &str = "cli";

/// A pairing credential, manual code or device token. It serializes as a plain
/// string; `Debug` never prints the value, so it cannot reach a log line or an
/// error string through a derived impl.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value, for the `pair` frame or the keychain.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

/// Which representation of a pairing challenge the device presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingMethod {
    /// The high-entropy credential carried by the QR payload.
    Credential,
    /// The eight-digit code typed by hand next to the address.
    Code,
}

/// First frame from a device, and the two that follow it during a pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HandshakeRequest {
    /// Ask the machine's owner to let this client pair. Carries no secret. The
    /// socket stays open for the answer, then takes one `pair` frame with the
    /// code the owner reads off the machine.
    ConnectRequest {
        protocol_version: u32,
        /// A label chosen by the client, shown to the owner. Never an identity.
        device_name: String,
        /// `macos`, `windows`, `linux`, ….
        platform: String,
        /// A random identifier generated once per install: pairing again with
        /// it replaces this install's earlier device record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        install_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_kind: Option<String>,
        /// This machine's stable identifier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
    },
    /// Exchange a pairing challenge for a device token.
    Pair {
        protocol_version: u32,
        method: PairingMethod,
        secret: Secret,
        device_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        install_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_id: Option<String>,
    },
    /// Present a device token obtained from an earlier `paired` frame.
    Authenticate {
        protocol_version: u32,
        device_id: String,
        token: Secret,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
    },
}

/// Frames the gateway sends during the handshake, and the error frame that
/// precedes a close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HandshakeResponse {
    /// The request is in front of the machine's owner. Wait on the same socket.
    ConnectPending {
        request_id: String,
        expires_in_secs: u64,
    },
    /// The owner accepted and the machine shows a code. Send one `pair` frame
    /// with `method: "code"` and that code within `expires_in_secs`.
    ConnectApproved {
        request_id: String,
        code_length: u32,
        expires_in_secs: u64,
    },
    /// The owner denied the request; the connection closes.
    ConnectDenied,
    /// Nobody answered in time, or the code was not sent in time.
    ConnectExpired,
    /// Pairing succeeded. `token` is shown exactly once.
    Paired {
        protocol_version: u32,
        gateway_id: String,
        instance_id: String,
        device_id: String,
        device_name: String,
        token: Secret,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
    },
    /// The device token was accepted.
    Authenticated {
        protocol_version: u32,
        gateway_id: String,
        instance_id: String,
        device_id: String,
        device_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_secs: Option<u64>,
    },
}

impl HandshakeResponse {
    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("handshake frames always serialize")
    }

    /// The `(code, message)` of an error frame, for a message to the user.
    pub fn error(&self) -> Option<(ErrorCode, &str)> {
        match self {
            Self::Error { code, message, .. } => Some((*code, message.as_str())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidFrame,
    FrameTooLarge,
    UnsupportedVersion,
    HandshakeTimeout,
    /// Wrong, expired, exhausted or already used pairing secret.
    PairingFailed,
    RateLimited,
    /// Unknown device, revoked device or wrong token.
    Unauthorized,
    /// The authenticated device cannot perform this operation.
    UnauthorizedAccess,
    Busy,
    ConnectBusy,
    ConnectRateLimited,
    /// The device was revoked while connected. Do not reconnect.
    Revoked,
    SlowConsumer,
    IdleTimeout,
    ShuttingDown,
    NotImplemented,
    Internal,
}

impl ErrorCode {
    /// Whether pairing again could fix this. `Unauthorized` means the token is
    /// gone (revoked, expired or replaced), which is the client's cue to say
    /// "pair again" instead of "offline".
    pub fn needs_new_pairing(self) -> bool {
        matches!(self, Self::Unauthorized | Self::Revoked)
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The wire name, so a message matches what the gateway logged.
        let name = serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "internal".to_owned());
        formatter.pad(&name)
    }
}

/// Switch an authenticated desktop WebSocket to raw ACP JSON-RPC frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AcpClientFrame {
    AcpOpen,
}

/// Confirmation that subsequent text frames carry ACP JSON-RPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpServerFrame {
    AcpOpened,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// The committed contract, as the repository ships it.
    fn contract_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/remote-protocol/v1")
    }

    fn json(path: &Path) -> serde_json::Value {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn files(directory: &Path) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = std::fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.retain(|name| !name.is_empty());
        names
    }

    /// Every golden handshake request decodes, and re-encoding it reproduces
    /// the committed JSON exactly: no field we drop, invent or rename.
    #[test]
    fn golden_handshake_requests_decode_and_re_encode() {
        let directory = contract_dir().join("golden/handshake/requests");
        let names = files(&directory);
        assert_eq!(
            names,
            BTreeSet::from([
                "authenticate.json".to_owned(),
                "authenticate_thread_names.json".to_owned(),
                "connect_request.json".to_owned(),
                "connect_request_desktop.json".to_owned(),
                "pair_code.json".to_owned(),
                "pair_credential.json".to_owned(),
            ]),
            "a golden request was added or removed: update this mirror and this list"
        );
        for name in names {
            let golden = json(&directory.join(&name));
            let frame: HandshakeRequest = serde_json::from_value(golden.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                serde_json::to_value(&frame).unwrap(),
                golden,
                "{name} does not re-encode identically"
            );
        }
    }

    /// The desktop request that wins ACP access carries the kind, the machine
    /// identifier and the install identifier, and they survive the round trip.
    #[test]
    fn the_desktop_connect_request_names_the_machine() {
        let golden =
            json(&contract_dir().join("golden/handshake/requests/connect_request_desktop.json"));
        let HandshakeRequest::ConnectRequest {
            device_kind,
            host_id,
            install_id,
            platform,
            ..
        } = serde_json::from_value(golden).unwrap()
        else {
            panic!("the desktop golden frame is a connect_request");
        };
        assert_eq!(device_kind.as_deref(), Some(DEVICE_KIND_DESKTOP));
        assert_eq!(host_id.as_deref(), Some("3f9a1c7e5b2d4086"));
        assert!(install_id.is_some());
        assert_eq!(platform, "windows");
    }

    /// Every golden handshake response decodes, including each error code the
    /// gateway can answer this client with, and re-encodes identically.
    #[test]
    fn golden_handshake_responses_decode_and_re_encode() {
        let directory = contract_dir().join("golden/handshake/responses");
        let names = files(&directory);
        assert!(names.len() >= 20, "the response contract shrank: {names:?}");
        let mut codes: BTreeSet<String> = BTreeSet::new();
        for name in names {
            let golden = json(&directory.join(&name));
            let frame: HandshakeResponse = serde_json::from_value(golden.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            if let HandshakeResponse::Error { code, .. } = &frame {
                codes.insert(code.to_string());
            }
            assert_eq!(
                serde_json::to_value(&frame).unwrap(),
                golden,
                "{name} does not re-encode identically"
            );
        }
        // The codes this client acts on must all be spelled the way the
        // gateway sends them.
        for code in [
            ErrorCode::Unauthorized,
            ErrorCode::UnauthorizedAccess,
            ErrorCode::PairingFailed,
            ErrorCode::Revoked,
            ErrorCode::ConnectBusy,
            ErrorCode::ConnectRateLimited,
        ] {
            let name = code.to_string();
            assert!(codes.contains(&name), "{name} has no golden frame");
        }
    }

    /// The two frames that switch to ACP, and the schema documents they are
    /// generated from, are what this client sends and expects.
    #[test]
    fn golden_acp_frames_decode() {
        let directory = contract_dir();
        assert_eq!(
            serde_json::from_value::<AcpClientFrame>(json(&directory.join("acp_open.json")))
                .unwrap(),
            AcpClientFrame::AcpOpen
        );
        assert_eq!(
            serde_json::to_value(AcpClientFrame::AcpOpen).unwrap(),
            json(&directory.join("acp_open.json"))
        );
        assert_eq!(
            serde_json::from_value::<AcpServerFrame>(json(&directory.join("acp_opened.json")))
                .unwrap(),
            AcpServerFrame::AcpOpened
        );
        assert_eq!(
            serde_json::to_value(AcpServerFrame::AcpOpened).unwrap(),
            json(&directory.join("acp_opened.json"))
        );
        for schema in ["acp-client.schema.json", "acp-server.schema.json"] {
            let document = json(&directory.join(schema));
            assert_eq!(
                document["title"].as_str(),
                Some(match schema {
                    "acp-client.schema.json" => "AcpClientFrame",
                    _ => "AcpServerFrame",
                }),
                "{schema} is not the frame this client speaks"
            );
        }
    }

    /// Secrets stay out of `Debug`, so they cannot reach a log line.
    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let needle = "s3cr3t-value-zzz";
        let frame = HandshakeRequest::Authenticate {
            protocol_version: 1,
            device_id: "0011".into(),
            token: Secret::new(needle),
            wire_names: None,
        };
        assert!(!format!("{frame:?}").contains(needle));
        assert!(!format!("{:?}", Secret::new(needle)).contains(needle));
    }
}
