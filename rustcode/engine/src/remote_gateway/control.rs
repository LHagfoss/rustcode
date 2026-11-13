//! Private control socket of the gateway: local CLI only.
//!
//! One newline-delimited JSON request per connection, reusing the daemon's
//! bounded framing. The socket is 0600 inside the 0700 gateway directory, so
//! only the owning user can ask for a pairing challenge, revoke a device or
//! stop the gateway. Session owners do not use it: they register on the
//! owner socket next to it ([`super::owner_ipc`]).

use super::address::AdvertisedAddress;
use super::connect::{RequestEnd, RequestState};
use super::devices::{DeviceKind, DeviceRecord, UNKNOWN_PLATFORM};
use super::handshake::Secret;
use super::hub::SessionSummary;
use super::pairing::{self, ChallengeEnd, PairingOffer};
use crate::daemon::protocol::{read_async_frame, write_async_frame};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::BufReader;

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;

/// Deadline for one control exchange, on both ends.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    /// Host owner grants or revokes full ACP control. Never a device frame.
    SetAcp {
        device: String,
        allow: bool,
    },
    /// Issue a pairing challenge, replacing any outstanding one.
    Pair,
    /// Create a connect code for another computer, replacing an open one.
    /// The pairing challenge is left as it is.
    MachineOffer,
    /// Withdraw the open connect code.
    CancelMachineOffer,
    /// Revoke a device by identifier, identifier prefix or unique name.
    Revoke {
        device: String,
    },
    /// Accept or deny a device's connection request. Accepting issues a
    /// pairing challenge bound to it, replacing any outstanding one, and
    /// answers with its code.
    AnswerConnectRequest {
        id: String,
        accept: bool,
    },
    /// Stop the gateway. The instance must match so a stale caller cannot
    /// stop a successor.
    Shutdown {
        instance_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlResponse {
    Status {
        status: GatewayStatus,
    },
    Offer {
        offer: OfferDetails,
    },
    MachineOffer {
        offer: Box<MachineOfferDetails>,
    },
    MachineOfferCancelled {
        /// Whether a connect code was still open.
        was_open: bool,
    },
    Revoked {
        device_id: String,
        device_name: String,
        closed_connections: usize,
    },
    ConnectAnswered {
        answer: ConnectAnswer,
    },
    Ack,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayStatus {
    #[serde(default)]
    pub version: String,
    pub pid: u32,
    pub process_start_time: u64,
    /// Changes on every gateway start.
    pub instance_id: String,
    /// Stable for this host's configuration directory.
    pub gateway_id: String,
    pub uptime_seconds: u64,
    pub listen_address: String,
    /// Every address a WebSocket listener is bound to, primary first, each
    /// with the interface it is on. `listen_address` is the first of them.
    /// Empty from a gateway that does not report them.
    #[serde(default)]
    pub listening: Vec<crate::remote_gateway::address::ListenAddress>,
    pub advertised_address: String,
    /// Who started this gateway: the RustCode server, the app, or a terminal.
    /// Absent from a gateway that predates it.
    #[serde(default)]
    pub started_by: super::lifecycle::GatewayOwner,
    pub paired_devices: usize,
    pub connections: Vec<ConnectionSummary>,
    /// Whether a pairing challenge is currently redeemable.
    pub pairing_open: bool,
    /// The advertised address is loopback: no other machine can pair.
    #[serde(default)]
    pub loopback_only: bool,
    /// Shared sessions; `None` from a gateway without session routing.
    #[serde(default)]
    pub sessions: Option<Vec<SessionSummary>>,
    /// Addresses this machine has right now, as `address  (interface)`. A
    /// listen address that is not among them has gone stale.
    #[serde(default)]
    pub candidates: Vec<String>,
    /// The addresses a device can dial this gateway at, best first, each with
    /// the kind of network it is on. The handshake reports the same list.
    #[serde(default)]
    pub routes: Vec<crate::remote_gateway::address::HostRoute>,
    /// Paired devices, connected or not. Never a token.
    #[serde(default)]
    pub devices: Vec<DeviceSummary>,
    /// `None` from a gateway that does not report it.
    #[serde(default)]
    pub pairing: Option<PairingStatus>,
    /// The connect code, apart from the challenge. `None` from a gateway
    /// without connect codes.
    #[serde(default)]
    pub machine_offer: Option<MachineOfferStatus>,
    /// Sessions whose owner process is alive but not registered here.
    #[serde(default)]
    pub unregistered_owners: Vec<String>,
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    /// Holds one `<session id>.log` per session started from a device.
    #[serde(default)]
    pub owner_log_directory: Option<PathBuf>,
    /// Devices asking to connect, oldest first. Empty from a gateway that
    /// does not take connection requests.
    #[serde(default)]
    pub connect_requests: Vec<ConnectRequestSummary>,
    /// Connection requests that ended recently, oldest first.
    #[serde(default)]
    pub ended_connect_requests: Vec<EndedConnectRequest>,
    /// Connection attempts the gateway turned away, newest first, so the owner
    /// can say why a device does not get in. Empty from a gateway that does
    /// not keep them. Never a secret.
    #[serde(default)]
    pub refused_connections: Vec<RefusedConnection>,
    /// The bind is `auto` and follows the machine's addresses. `None` from a
    /// gateway that does not report it.
    #[serde(default)]
    pub automatic_bind: Option<bool>,
    /// The advertised host was given explicitly and never moves by itself.
    #[serde(default)]
    pub fixed_advertise: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    /// When the device last paired or connected; `None` when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Phone, desktop or command line. A phone from an older gateway.
    #[serde(default)]
    pub kind: DeviceKind,
    /// `ios`, `windows`, …; `unknown` from an older gateway.
    #[serde(default = "unknown_platform")]
    pub platform: String,
    /// The paired machine's own identifier, for a desktop or command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
}

fn unknown_platform() -> String {
    UNKNOWN_PLATFORM.to_owned()
}

impl From<&DeviceRecord> for DeviceSummary {
    fn from(device: &DeviceRecord) -> Self {
        Self {
            id: device.id.clone(),
            name: device.name.clone(),
            last_seen_at: device.last_seen_at,
            kind: device.kind,
            platform: device.platform.clone(),
            host_id: device.host_id.clone(),
        }
    }
}

/// Where pairing stands, without any secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingStatus {
    /// Lifetime left of the open challenge.
    pub expires_in_secs: Option<u64>,
    /// Wrong codes the open challenge still tolerates.
    pub attempts_left: Option<u8>,
    /// Pairing is locked after too many failures on this host.
    pub locked_for_secs: Option<u64>,
    /// Serial of the open challenge. `None` without one, or from a gateway
    /// that does not number its challenges.
    #[serde(default)]
    pub challenge: Option<u64>,
    /// Serial of the most recently issued challenge, open or not.
    #[serde(default)]
    pub last_challenge: Option<u64>,
    /// The last challenge that ended before it expired.
    #[serde(default)]
    pub ended: Option<EndedChallenge>,
    /// Whether any socket reached this gateway since the most recent challenge
    /// was issued. `Some(false)` on a challenge that is gone means no device
    /// even got as far as connecting, which is a network problem, not a wrong
    /// code. `None` before any challenge, or from a gateway that does not track
    /// it.
    #[serde(default)]
    pub last_challenge_reached: Option<bool>,
}

/// Where the connect code stands, without any secret.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineOfferStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<OpenMachineOffer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ended: Option<EndedMachineOffer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenMachineOffer {
    pub serial: u64,
    pub expires_in_secs: u64,
    /// Wrong tries the code still tolerates.
    pub attempts_left: u8,
}

/// A connect code that ended before it expired: a computer paired with it,
/// or its wrong tries ran out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedMachineOffer {
    pub serial: u64,
    pub end: ChallengeEnd,
    /// The device record the code created, when it paired one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// A connect code, shown once by whoever asked for it. Secrets stay
/// redacted in `Debug`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineOfferDetails {
    /// The short form, `K7MQ-29XH-TP4W-NRD6`.
    pub code: Secret,
    /// The long form, `rustcode://connect/…`: every route and the gateway's id.
    pub link: Secret,
    /// The address the short code carries, or the one to type next to it
    /// when it carries none (`address_in_code` is false).
    pub address: String,
    pub address_in_code: bool,
    pub gateway_id: String,
    pub instance_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// Every address in the long form, best first.
    #[serde(default)]
    pub routes: Vec<crate::remote_gateway::address::HostRoute>,
    /// No other computer can reach this gateway: the code is useless.
    #[serde(default)]
    pub loopback_only: bool,
    pub expires_in_secs: u64,
    /// Numbers connect codes, to follow this one in [`MachineOfferStatus`].
    pub serial: u64,
    /// Pairing is locked on this host right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_for_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedChallenge {
    pub challenge: u64,
    pub end: ChallengeEnd,
}

/// A device asking to connect. Never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectRequestSummary {
    pub id: String,
    /// The label the device sent, trimmed and bounded. Not an identity.
    pub device_name: String,
    /// What the device says it is. A phone from an older gateway.
    #[serde(default)]
    pub device_kind: DeviceKind,
    pub platform: String,
    /// The network address the request came from.
    pub remote_addr: String,
    pub age_secs: u64,
    /// Time left to answer, or once accepted to present the code.
    pub expires_in_secs: u64,
    pub state: RequestState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedConnectRequest {
    pub id: String,
    pub device_name: String,
    pub end: RequestEnd,
    /// The device it paired, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub ended_secs_ago: u64,
}

/// A connection attempt the gateway turned away, for its owner to see why a
/// device did not get in: a phone whose app speaks another protocol version, a
/// code that did not work, or a pairing this host no longer accepts. Never a
/// secret: only what the device's frame said about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedConnection {
    pub reason: RefusalReason,
    /// How many times the same attempt was refused since the gateway started.
    #[serde(default = "one_attempt")]
    pub attempts: u32,
    /// The label the device sent, when its frame named one. Not an identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// What the device said it is. A phone from an older gateway.
    #[serde(default)]
    pub device_kind: DeviceKind,
    /// `ios`, `android`, `windows`, …; `unknown` when it said nothing usable.
    #[serde(default = "unknown_platform")]
    pub platform: String,
    /// The remote protocol version the device sent, when its frame carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u32>,
    /// Set with [`RefusalReason::UnsupportedVersion`]: the versions this host
    /// speaks, so the owner is told which side is behind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_versions: Vec<u32>,
    /// The network address the attempt came from.
    pub remote_addr: String,
    pub refused_secs_ago: u64,
}

fn one_attempt() -> u32 {
    1
}

/// Why the gateway refused a connection. Each one names something its owner can
/// act on, so the answer is never only "the phone could not connect".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The app on the device speaks a remote protocol version this host does
    /// not: one of the two must be updated.
    UnsupportedVersion,
    /// The pairing code or credential was wrong, already used or expired.
    PairingFailed,
    /// Too many failed pairing attempts on this host, so even the right code is
    /// refused until the lockout ends.
    RateLimited,
    /// Unknown, revoked or expired device, or a token that does not match it.
    Unauthorized,
    /// A retired refresh token was presented; the gateway therefore removed the
    /// whole pairing. The device has to pair again.
    TokenReplayed,
    /// The WebSocket upgrade carried an `Origin` no native app sends, so it was
    /// refused before a single frame arrived: a browser, or an app build older
    /// than the one that sends the native app origin.
    UnexpectedOrigin,
    /// The device opened a TLS connection (`wss://`, `https://`) to a gateway
    /// that speaks plain WebSocket, so nothing it sent could be read.
    SecureConnection,
    /// The first frame was JSON text the gateway could not read as a pair,
    /// authenticate or connect request: an app build whose handshake differs
    /// from this host's.
    UnrecognizedRequest,
}

impl RefusalReason {
    /// A short name for a terminal line, in the words the owner acts on.
    pub fn label(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "app version this host does not speak",
            Self::PairingFailed => "pairing code refused (wrong, used or expired)",
            Self::RateLimited => "pairing locked after too many wrong codes",
            Self::Unauthorized => "unknown or revoked device",
            Self::TokenReplayed => "pairing revoked (a retired token was used)",
            Self::UnexpectedOrigin => "unexpected app origin, refused before any frame",
            Self::SecureConnection => "TLS (wss://) dial to a plain WebSocket gateway",
            Self::UnrecognizedRequest => "first frame not understood (different app version?)",
        }
    }

    /// The handshake error code this reason was decided from, for a gateway
    /// that records a refusal where the frame was refused.
    pub fn from_error_code(code: super::handshake::ErrorCode) -> Option<Self> {
        use super::handshake::ErrorCode;
        Some(match code {
            ErrorCode::UnsupportedVersion => Self::UnsupportedVersion,
            ErrorCode::PairingFailed => Self::PairingFailed,
            ErrorCode::RateLimited => Self::RateLimited,
            ErrorCode::Unauthorized => Self::Unauthorized,
            ErrorCode::TokenReplayed => Self::TokenReplayed,
            // A first frame that is not the handshake is what an app build with
            // a different handshake sends, and its owner can update it. Binary,
            // oversized and doubled frames are refused before they get here.
            ErrorCode::InvalidFrame => Self::UnrecognizedRequest,
            // A full gateway or a slow one is not about the device. Recording
            // those would crowd out the ones that help.
            _ => return None,
        })
    }
}

/// The owner's answer to a connection request. On acceptance `code` is the
/// only copy of the challenge's code, for the host to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectAnswer {
    pub id: String,
    pub device_name: String,
    /// Gateway process that issued this answer and its code.
    pub instance_id: String,
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    /// Pairing is locked on this host right now: the code cannot be
    /// redeemed until the lockout ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_for_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSummary {
    pub device_id: String,
    pub device_name: String,
}

/// Everything needed to show pairing details. Secrets stay redacted in
/// `Debug`; they are shown once by the command that asked for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferDetails {
    pub advertised_address: String,
    /// The address is loopback: only this machine can use the offer.
    #[serde(default)]
    pub loopback_only: bool,
    pub gateway_id: String,
    /// Gateway instance that issued this offer.
    #[serde(default)]
    pub instance_id: Option<String>,
    pub credential: Secret,
    pub code: Secret,
    pub expires_in_secs: u64,
    /// Serial of this challenge, to follow it in [`PairingStatus`]. `None`
    /// from a gateway that does not number its challenges.
    #[serde(default)]
    pub challenge: Option<u64>,
    /// Pairing is locked on this host right now: the challenge cannot be
    /// redeemed, not even with the right code, until the lockout ends.
    #[serde(default)]
    pub locked_for_secs: Option<u64>,
    /// Other `host:port` addresses the listener answers on, also in the QR
    /// payload; see [`pairing::QrPayload::alternates`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<String>,
    /// The exact string a pairing QR code encodes.
    pub qr_payload: Secret,
}

impl OfferDetails {
    pub fn new(
        address: &AdvertisedAddress,
        gateway_id: &str,
        host_name: Option<&str>,
        offer: PairingOffer,
    ) -> Self {
        Self::with_alternates(address, gateway_id, host_name, Vec::new(), offer)
    }

    /// [`Self::new`] for a listener that also answers on `alternates`
    /// ([`super::address::alternate_addresses`]).
    pub fn with_alternates(
        address: &AdvertisedAddress,
        gateway_id: &str,
        host_name: Option<&str>,
        alternates: Vec<String>,
        offer: PairingOffer,
    ) -> Self {
        Self {
            advertised_address: address.to_string(),
            gateway_id: gateway_id.to_string(),
            instance_id: None,
            loopback_only: address.host().is_loopback(),
            qr_payload: Secret::new(pairing::qr_payload(
                address,
                gateway_id,
                &offer.credential,
                host_name,
                &alternates,
            )),
            alternates,
            credential: offer.credential,
            code: offer.code,
            expires_in_secs: offer.lifetime.as_secs(),
            challenge: None,
            locked_for_secs: None,
        }
    }
}

/// Send one request to a running gateway and wait for its answer.
pub async fn request(socket_path: &Path, request: &ControlRequest) -> Result<ControlResponse> {
    tokio::time::timeout(CONTROL_TIMEOUT, async {
        let mut stream = super::local_socket::connect(socket_path).await?;
        write_async_frame(&mut stream, request).await?;
        Ok(read_async_frame(&mut BufReader::new(stream)).await?)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_details_embed_the_advertised_address_and_hide_secrets_in_debug() {
        let address = AdvertisedAddress::new("192.168.1.20", 17879).unwrap();
        let offer = PairingOffer {
            credential: Secret::new("credential-value"),
            code: Secret::new("1234-5678"),
            lifetime: Duration::from_secs(120),
        };
        let details = OfferDetails::new(&address, "gw", Some("studio"), offer);
        assert_eq!(details.advertised_address, "192.168.1.20:17879");
        assert_eq!(details.expires_in_secs, 120);
        assert!(details.qr_payload.expose().contains("credential-value"));
        assert!(details.qr_payload.expose().contains("192.168.1.20:17879"));
        let debug = format!("{details:?}");
        assert!(!debug.contains("credential-value"), "{debug}");
        assert!(!debug.contains("1234-5678"), "{debug}");
    }
}
