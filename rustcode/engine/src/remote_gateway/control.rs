//! Private control socket of the gateway: local CLI only.
//!
//! One newline-delimited JSON request per connection, reusing the daemon's
//! bounded framing. The socket is 0600 inside the 0700 gateway directory, so
//! only the owning user can ask for a pairing challenge, revoke a device or
//! stop the gateway. Session owners do not use it: they register on the
//! owner socket next to it ([`super::owner_ipc`]).

use super::address::AdvertisedAddress;
use super::handshake::Secret;
use super::hub::SessionSummary;
use super::pairing::{self, ChallengeEnd, PairingOffer};
use crate::daemon::protocol::{read_async_frame, write_async_frame};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::{io::BufReader, net::UnixStream};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;

/// Deadline for one control exchange, on both ends.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Status,
    /// Issue a pairing challenge, replacing any outstanding one.
    Pair,
    /// Revoke a device by identifier, identifier prefix or unique name.
    Revoke {
        device: String,
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
    Revoked {
        device_id: String,
        device_name: String,
        closed_connections: usize,
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
    pub advertised_address: String,
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
    /// Paired devices, connected or not. Never a token.
    #[serde(default)]
    pub devices: Vec<DeviceSummary>,
    /// `None` from a gateway that does not report it.
    #[serde(default)]
    pub pairing: Option<PairingStatus>,
    /// Sessions whose owner process is alive but not registered here.
    #[serde(default)]
    pub unregistered_owners: Vec<String>,
    #[serde(default)]
    pub log_path: Option<PathBuf>,
    /// Holds one `<session id>.log` per session started from a device.
    #[serde(default)]
    pub owner_log_directory: Option<PathBuf>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedChallenge {
    pub challenge: u64,
    pub end: ChallengeEnd,
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
        Self {
            advertised_address: address.to_string(),
            gateway_id: gateway_id.to_string(),
            loopback_only: address.host().is_loopback(),
            qr_payload: Secret::new(pairing::qr_payload(
                address,
                gateway_id,
                &offer.credential,
                host_name,
            )),
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
        let mut stream = UnixStream::connect(socket_path).await?;
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
