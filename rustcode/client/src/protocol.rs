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

/// The capability this client names in `pair` and `authenticate` to exchange a
/// rotating refresh token for a new one. A gateway that predates it answers the
/// pairing with the static `token` and its 30-day expiry, which this client
/// still presents and stores.
pub const CAPABILITY_TOKEN_ROTATION: &str = "token_rotation";

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
        /// Optional features the device understands. Naming
        /// [`CAPABILITY_TOKEN_ROTATION`] asks for rotating tokens instead of
        /// the static device token and its expiry.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
    /// Present a device token obtained from an earlier `paired` frame.
    Authenticate {
        protocol_version: u32,
        device_id: String,
        /// The static device token, or the refresh token of a device that
        /// rotates its tokens. Absent when `refresh_token` or `access_token`
        /// is the credential.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Secret>,
        /// The rotating device's long-lived token: every accepted one is
        /// replaced by the answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<Secret>,
        /// The short-lived token from an earlier `paired` or `authenticated`
        /// frame: reconnects without rotating anything.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_token: Option<Secret>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
        /// Optional features the device understands, as in `pair`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Vec<String>>,
    },
    /// The device stored the refresh token from the `authenticated` frame that
    /// named `rotation_id`, so the machine may retire the token before it. A
    /// session-path frame after authentication, never a first frame.
    TokenAck { rotation_id: String },
}

/// An address the host answers on, as a handshake reports it. The kind is the
/// host's own word (`lan`, `overlay`, `hostname`, `public`; a gateway before
/// the overlay kind says `vpn`), and `provider` names the mesh product
/// (`netbird`, `tailscale`, `zerotier`) when the host can tell. A kind this
/// client does not know is still a route it may try.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRoute {
    pub address: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
    /// Pairing succeeded. `token` is shown exactly once, or, for a device that
    /// named [`CAPABILITY_TOKEN_ROTATION`], `refresh_token` and `access_token`
    /// are and the pairing does not expire.
    Paired {
        protocol_version: u32,
        gateway_id: String,
        instance_id: String,
        device_id: String,
        device_name: String,
        /// The static token of a device that does not rotate; shown once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Secret>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_name: Option<String>,
        /// When this pairing stops being accepted; absent for a device that
        /// rotates its tokens, whose pairing lasts until it is revoked.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        /// The rotating device's first refresh token; shown once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<Secret>,
        /// The access token to reconnect with, until `access_expires_at`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_token: Option<Secret>,
        /// When the machine stops accepting `access_token`, RFC 3339 in UTC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_expires_at: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
        /// The RustCode version running on that machine, for the row that
        /// says it needs updating. A label, never a gate; absent from an
        /// older gateway.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// What that machine runs: `macos`, `linux` or `windows`. Absent from
        /// an older gateway, whose machine stays `unknown` until it updates.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        /// The addresses this machine answers on now, best first. A newer
        /// gateway sends them; an older one leaves the field out.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        routes: Vec<HostRoute>,
    },
    /// The presented credential was accepted.
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
        /// The new refresh token when a refresh token was presented: the
        /// device stores it and answers with `token_ack`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<Secret>,
        /// The access token for this connection, until `access_expires_at`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_token: Option<Secret>,
        /// When the machine stops accepting the access token, RFC 3339 in UTC.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_expires_at: Option<String>,
        /// The rotation to acknowledge; absent when nothing rotated.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire_names: Option<String>,
        /// That machine's RustCode version, as in `paired`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// That machine's platform, as in `paired`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        /// The addresses this machine answers on now, best first, as in
        /// `paired`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        routes: Vec<HostRoute>,
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
    /// A retired refresh token was presented: the machine revoked the whole
    /// pairing. The device drops its tokens and pairs again.
    TokenReplayed,
    /// The access token expired or was replaced. The pairing is still valid:
    /// authenticate with the refresh token.
    AccessExpired,
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
    /// "pair again" instead of "offline". `AccessExpired` is not one of them:
    /// the refresh token still authenticates.
    pub fn needs_new_pairing(self) -> bool {
        matches!(
            self,
            Self::Unauthorized | Self::Revoked | Self::TokenReplayed
        )
    }

    /// Whether the machine revoked the pairing rather than refusing a stale
    /// credential. The saved token is worthless after one of these, so the
    /// client drops it instead of presenting it again.
    pub fn revoked_pairing(self) -> bool {
        matches!(self, Self::Revoked | Self::TokenReplayed)
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
                "authenticate_access.json".to_owned(),
                "authenticate_refresh.json".to_owned(),
                "authenticate_thread_names.json".to_owned(),
                "connect_request.json".to_owned(),
                "connect_request_desktop.json".to_owned(),
                "pair_code.json".to_owned(),
                "pair_credential.json".to_owned(),
                "token_ack.json".to_owned(),
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

    /// The two ways a rotating device presents its tokens, and the
    /// acknowledgement that retires the one a rotation replaced, decode to the
    /// shapes the client acts on.
    #[test]
    fn the_rotating_handshake_frames_carry_what_the_client_needs() {
        let directory = contract_dir().join("golden/handshake");
        let HandshakeRequest::Authenticate {
            token,
            refresh_token,
            access_token,
            capabilities,
            ..
        } = serde_json::from_value(json(&directory.join("requests/authenticate_refresh.json")))
            .unwrap()
        else {
            panic!("authenticate_refresh is an authenticate frame");
        };
        assert!(token.is_none(), "a refresh token is not the static slot");
        assert!(refresh_token.is_some());
        assert!(access_token.is_none());
        assert_eq!(
            capabilities.as_deref(),
            Some([CAPABILITY_TOKEN_ROTATION.to_owned()].as_slice()),
            "a rotating device names the capability on authenticate"
        );

        let HandshakeRequest::Authenticate {
            token,
            refresh_token,
            access_token,
            capabilities,
            ..
        } = serde_json::from_value(json(&directory.join("requests/authenticate_access.json")))
            .unwrap()
        else {
            panic!("authenticate_access is an authenticate frame");
        };
        assert!(token.is_none() && refresh_token.is_none());
        assert!(access_token.is_some());
        assert!(
            capabilities.is_none(),
            "an access reconnect asks for nothing"
        );

        let ack: HandshakeRequest =
            serde_json::from_value(json(&directory.join("requests/token_ack.json"))).unwrap();
        assert_eq!(
            ack,
            HandshakeRequest::TokenAck {
                rotation_id: "5be1c4a07d2e9f63".to_owned()
            }
        );

        let HandshakeResponse::Paired {
            token,
            expires_at,
            refresh_token,
            access_token,
            access_expires_at,
            ..
        } = serde_json::from_value(json(&directory.join("responses/paired_rotating.json")))
            .unwrap()
        else {
            panic!("paired_rotating is a paired frame");
        };
        assert!(
            token.is_none() && expires_at.is_none(),
            "a rotating pairing never expires"
        );
        assert!(refresh_token.is_some() && access_token.is_some());
        assert_eq!(access_expires_at.as_deref(), Some("2026-08-22T09:15:00Z"));

        let HandshakeResponse::Authenticated {
            refresh_token,
            access_token,
            rotation_id,
            ..
        } = serde_json::from_value(json(
            &directory.join("responses/authenticated_rotating.json"),
        ))
        .unwrap()
        else {
            panic!("authenticated_rotating is an authenticated frame");
        };
        assert!(refresh_token.is_some() && access_token.is_some());
        assert_eq!(rotation_id.as_deref(), Some("5be1c4a07d2e9f63"));

        let HandshakeResponse::Authenticated {
            refresh_token,
            access_token,
            rotation_id,
            access_expires_at,
            ..
        } = serde_json::from_value(json(&directory.join("responses/authenticated_access.json")))
            .unwrap()
        else {
            panic!("authenticated_access is an authenticated frame");
        };
        assert!(
            refresh_token.is_none() && access_token.is_none() && rotation_id.is_none(),
            "an access reconnect rotates nothing"
        );
        assert!(access_expires_at.is_some());
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
            ErrorCode::TokenReplayed,
            ErrorCode::AccessExpired,
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
            token: Some(Secret::new(needle)),
            refresh_token: Some(Secret::new(needle)),
            access_token: Some(Secret::new(needle)),
            wire_names: None,
            capabilities: None,
        };
        assert!(!format!("{frame:?}").contains(needle));
        assert!(!format!("{:?}", Secret::new(needle)).contains(needle));
    }

    /// The two error codes a rotating device acts on are the ones the contract
    /// spells, and only a revoked pairing drops the saved token.
    #[test]
    fn the_rotation_error_codes_are_read_the_way_the_client_acts() {
        assert!(ErrorCode::TokenReplayed.revoked_pairing());
        assert!(ErrorCode::Revoked.revoked_pairing());
        assert!(!ErrorCode::Unauthorized.revoked_pairing());
        assert!(!ErrorCode::AccessExpired.revoked_pairing());
        assert!(!ErrorCode::AccessExpired.needs_new_pairing());
        assert!(ErrorCode::TokenReplayed.needs_new_pairing());
        assert!(ErrorCode::Unauthorized.needs_new_pairing());
    }
}
