//! Paired devices and their revocable tokens.
//!
//! A device token is 256 random bits handed to the device once. The host
//! keeps only its SHA-256 digest, in an owner-only file replaced atomically.
//! The token is high-entropy, so a fast digest is enough: there is nothing to
//! brute-force that a slow hash would protect.
//!
//! A device that asks for `token_rotation` never gets a static token: it holds
//! a refresh token, which every successful `authenticate` replaces, and a
//! short-lived access token that reconnects without rotating. Only digests are
//! stored, and the access token's digest lives in memory alone, so a restart
//! needs a refresh rather than an access token.

use super::handshake::Secret;
use super::pairing::{constant_time_eq, digest};
use crate::daemon::private_files::{
    ensure_private_directory, owner_only, publish_private_json, read_private_json, remove_if_exists,
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// Upper bound on paired devices; pairing fails with `busy` beyond it.
pub const MAX_DEVICES: usize = 32;

/// Device names are labels typed on a phone; keep them short and printable.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// Install identifiers are opaque strings chosen by the app; anything outside
/// these bounds is ignored as if the device sent none.
pub const MAX_INSTALL_ID_CHARS: usize = 128;
const MIN_INSTALL_ID_CHARS: usize = 16;

/// A connection refreshes the recorded last-seen time at most this often, so
/// a device that reconnects in a loop does not rewrite the store each time.
pub const LAST_SEEN_RESOLUTION: chrono::Duration = chrono::Duration::minutes(5);

/// How long a static pairing lasts. After it the token is refused like an
/// unknown one and the device pairs again, which bounds what a copied token is
/// worth. A device that rotates its tokens has no expiry: it lasts until it is
/// revoked or a retired token is replayed.
pub const DEVICE_LIFETIME: chrono::Duration = chrono::Duration::days(30);

/// How long an access token is accepted after it was minted. It lets a device
/// reconnect on a flapping network without rotating its refresh token.
pub const ACCESS_TOKEN_LIFETIME: chrono::Duration = chrono::Duration::minutes(15);

/// Retired refresh-token fingerprints kept for one device, so any token this
/// host ever issued is recognized as a replay instead of being answered as a
/// wrong token. Each is 64 bits of the token's SHA-256, which keeps 1024 of
/// them to a few dozen KiB of the owner-only store. A family that rotates past
/// the cap is revoked conservatively rather than forgotten, so no issued
/// token can fall out of the history.
const MAX_RETIRED_TOKENS: usize = 1024;

/// Bytes of a digest kept for a retired refresh token. 64 bits is far beyond
/// any forged match at this scale (a wrong token collides with 2^-64), while
/// keeping the whole history small enough for the store.
const RETIRED_FINGERPRINT_BYTES: usize = 8;

/// Longest platform label kept.
pub const MAX_PLATFORM_CHARS: usize = 32;

/// Platform of a record stored before devices said theirs, or of a device
/// that sent none.
pub const UNKNOWN_PLATFORM: &str = "unknown";

const STORE_VERSION: u32 = 1;
/// Room for [`MAX_DEVICES`] families, each with [`MAX_RETIRED_TOKENS`] retired
/// fingerprint, inside one owner-only JSON file.
const MAX_STORE_BYTES: u64 = 1024 * 1024;

/// What a paired device is, as it says. A label for the owner, never a
/// permission: ACP access requires a separate host-controlled grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// The phone app. Records stored before devices said their kind are phones.
    #[default]
    Phone,
    /// The desktop app on another machine.
    Desktop,
    /// The `rustcode` command line on another machine.
    Cli,
    /// A kind written by a newer gateway. Kept loadable and display-only.
    Unknown,
}

impl<'de> Deserialize<'de> for DeviceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "phone" => Self::Phone,
            "desktop" => Self::Desktop,
            "cli" => Self::Cli,
            _ => Self::Unknown,
        })
    }
}

impl DeviceKind {
    /// The kind a device names on the wire. Anything unknown, including a
    /// kind a newer client invents, is a phone, as before devices said it.
    pub fn parse(kind: Option<&str>) -> Self {
        match kind.map(str::trim) {
            Some(kind) if kind.eq_ignore_ascii_case("desktop") => Self::Desktop,
            Some(kind) if kind.eq_ignore_ascii_case("cli") => Self::Cli,
            _ => Self::Phone,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

/// What a device says about itself when it asks to connect or pairs: its
/// kind, its platform and the stable identifier of the machine it runs on.
/// Labels for the owner, sanitized; none of it is an identity or a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProfile {
    pub kind: DeviceKind,
    /// `ios`, `android`, `macos`, `windows`, `linux`, … lowercase, bounded.
    pub platform: String,
    /// The connecting machine's own stable identifier, for a desktop or a
    /// command line. `None` when the device sent none or an unusable one.
    pub host_id: Option<String>,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        Self {
            kind: DeviceKind::Phone,
            platform: UNKNOWN_PLATFORM.to_owned(),
            host_id: None,
        }
    }
}

impl DeviceProfile {
    /// Build a profile from what a device sent, keeping what is usable.
    pub fn from_wire(kind: Option<&str>, platform: Option<&str>, host_id: Option<&str>) -> Self {
        Self {
            kind: DeviceKind::parse(kind),
            platform: sanitize_platform(platform.unwrap_or_default()),
            host_id: host_id.and_then(sanitize_host_id),
        }
    }
}

fn unknown_platform() -> String {
    UNKNOWN_PLATFORM.to_owned()
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// Full ACP access explicitly granted by the host owner.
    #[serde(default)]
    pub acp: bool,
    /// Random identifier assigned by the gateway. The only identity.
    pub id: String,
    /// Label supplied by the device. Not unique, not trusted.
    pub name: String,
    /// Hex SHA-256 of the static device token, for a record that pairs
    /// without `token_rotation`. The token itself is never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_sha256: Option<String>,
    pub paired_at: DateTime<Utc>,
    /// Hex SHA-256 of the app install's identifier, when the device sent one.
    /// Pairing again from the same install replaces this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_sha256: Option<String>,
    /// When the device last paired or authenticated, to within
    /// [`LAST_SEEN_RESOLUTION`]. `None` for records stored before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
    /// What the device is. Records stored before it was kept are phones.
    #[serde(default)]
    pub kind: DeviceKind,
    /// The platform the device said it runs on, as in [`DeviceProfile`].
    #[serde(default = "unknown_platform")]
    pub platform: String,
    /// The connecting machine's stable identifier, when it sent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// The rotating credentials of a device that paired with
    /// `token_rotation`. `None` for the static token of an older client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rotation: Option<TokenFamily>,
}

/// One device's rotating credentials: digests only.
///
/// `current` is the newest refresh token; `previous` is the one it replaced and
/// is still accepted until the device acknowledges `current` or uses it, so a
/// dropped `authenticated` frame never locks the device out. Anything in
/// `retired` was replaced for good: presenting one is a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TokenFamily {
    /// Groups the tokens of one pairing, for the owner's log.
    family_id: String,
    /// Hex SHA-256 of the newest refresh token.
    current: String,
    /// Hex SHA-256 of the token `current` replaced, still accepted until the
    /// device acknowledges `current` or uses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<String>,
    /// Fingerprints ([`RETIRED_FINGERPRINT_BYTES`] of the SHA-256, hex) of
    /// every refresh token replaced for good. Presenting one is a replay.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired: Vec<String>,
    /// The rotation whose answer the device has not acknowledged yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rotation_id: Option<String>,
    /// The access token minted with `current` and when it stops being
    /// accepted. Memory only: an access token never touches disk, so after a
    /// restart a device authenticates with its refresh token.
    #[serde(skip)]
    access: Option<AccessToken>,
    /// The answer the last rotation handed out, while it awaits
    /// acknowledgement. A retry with the previous refresh token gets exactly
    /// this answer again, so a second holder of the previous token cannot make
    /// the host rotate the stored token away. Memory only: after a restart
    /// there is nothing to re-issue, and a retry is a replay.
    #[serde(skip)]
    pending: Option<PendingRotation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessToken {
    sha256: String,
    expires_at: DateTime<Utc>,
}

/// The tokens of a rotation whose acknowledgement has not arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRotation {
    refresh_token: Secret,
    access_token: Secret,
    access_expires_at: DateTime<Utc>,
    rotation_id: String,
}

impl TokenFamily {
    /// Whether `stored` (hex) is the digest of `presented`. A damaged digest
    /// never matches the zero digest a failed decode would leave behind.
    fn digest_matches(stored: &str, presented: &[u8; 32]) -> bool {
        let mut expected = [0u8; 32];
        if hex::decode_to_slice(stored, &mut expected).is_err() {
            return false;
        }
        constant_time_eq(presented, &expected)
    }

    /// Whether `stored` is the fingerprint of `presented`.
    fn fingerprint_matches(stored: &str, presented: &[u8; 32]) -> bool {
        let mut expected = [0u8; RETIRED_FINGERPRINT_BYTES];
        if hex::decode_to_slice(stored, &mut expected).is_err() {
            return false;
        }
        let mut difference = 0u8;
        for (a, b) in presented[..RETIRED_FINGERPRINT_BYTES].iter().zip(&expected) {
            difference |= a ^ b;
        }
        std::hint::black_box(difference) == 0
    }

    /// A fresh family: its first refresh token and the access token that goes
    /// with it. Nothing is retired yet, so there is nothing to acknowledge.
    fn new(now: DateTime<Utc>) -> (Self, Issued) {
        let (refresh, access, access_expires_at) = mint(now);
        let family = Self {
            family_id: random_id(),
            current: hex::encode(digest(refresh.expose().as_bytes())),
            previous: None,
            retired: Vec::new(),
            rotation_id: None,
            access: Some(AccessToken {
                sha256: hex::encode(digest(access.expose().as_bytes())),
                expires_at: access_expires_at,
            }),
            pending: None,
        };
        let issued = Issued {
            refresh_token: refresh,
            access_token: access,
            access_expires_at,
            rotation_id: None,
        };
        (family, issued)
    }

    /// Replace the tokens after the newest refresh token was presented. The
    /// token before it is retired for good. `None` when the family already
    /// keeps as many retired tokens as it may; the caller revokes it then.
    fn rotate(&mut self, now: DateTime<Utc>) -> Option<Issued> {
        if !self.has_room() {
            return None;
        }
        if let Some(previous) = self.previous.take() {
            self.retire(previous);
        }
        let (refresh, access, access_expires_at) = mint(now);
        let current = hex::encode(digest(refresh.expose().as_bytes()));
        self.previous = Some(std::mem::replace(&mut self.current, current));
        self.access = Some(AccessToken {
            sha256: hex::encode(digest(access.expose().as_bytes())),
            expires_at: access_expires_at,
        });
        let rotation_id = random_id();
        self.rotation_id = Some(rotation_id.clone());
        self.pending = Some(PendingRotation {
            refresh_token: refresh.clone(),
            access_token: access.clone(),
            access_expires_at,
            rotation_id: rotation_id.clone(),
        });
        Some(Issued {
            refresh_token: refresh,
            access_token: access,
            access_expires_at,
            rotation_id: Some(rotation_id),
        })
    }

    /// The answer still awaiting acknowledgement, for a retry with the
    /// previous refresh token. `None` once it was acknowledged, or after a
    /// restart: a retry then is a replay.
    fn pending_answer(&self) -> Option<Issued> {
        let pending = self.pending.as_ref()?;
        Some(Issued {
            refresh_token: pending.refresh_token.clone(),
            access_token: pending.access_token.clone(),
            access_expires_at: pending.access_expires_at,
            rotation_id: Some(pending.rotation_id.clone()),
        })
    }

    /// Whether one more retired token fits in the history.
    fn has_room(&self) -> bool {
        self.retired.len() < MAX_RETIRED_TOKENS
    }

    /// Whether `presented` is a refresh token this family retired.
    fn is_retired(&self, presented: &[u8; 32]) -> bool {
        self.retired
            .iter()
            .any(|stored| Self::fingerprint_matches(stored, presented))
    }

    /// Forget a token for good, keeping the fingerprint its digest starts
    /// with. Only called with room ([`Self::has_room`]).
    fn retire(&mut self, digest: String) {
        self.retired
            .push(digest.chars().take(RETIRED_FINGERPRINT_BYTES * 2).collect());
    }
}

/// A new refresh token, access token and the access token's expiry.
fn mint(now: DateTime<Utc>) -> (Secret, Secret, DateTime<Utc>) {
    (random_token(), random_token(), now + ACCESS_TOKEN_LIFETIME)
}

/// 256 random bits, base64url. The only copy of a token.
fn random_token() -> Secret {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    Secret::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Sixteen hex characters naming a device, a token family or a rotation.
fn random_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

impl DeviceRecord {
    /// Whether the device exchanges a rotating refresh token for a new one.
    pub fn rotates(&self) -> bool {
        self.rotation.is_some()
    }

    /// The identifier grouping one pairing's tokens, for the owner's log.
    pub fn family_id(&self) -> Option<&str> {
        self.rotation
            .as_ref()
            .map(|family| family.family_id.as_str())
    }

    /// When the pairing stops being accepted: [`DEVICE_LIFETIME`] after
    /// pairing, or never for a device that rotates its tokens.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.rotation
            .is_none()
            .then(|| self.paired_at + DEVICE_LIFETIME)
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at().is_some_and(|expires| now >= expires)
    }
}

// The digest is not a credential, but it has no business in a log either.
impl fmt::Debug for DeviceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("kind", &self.kind)
            .field("platform", &self.platform)
            .field("host_id", &self.host_id)
            .field("rotates", &self.rotates())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceError {
    /// `MAX_DEVICES` are already paired.
    Full,
    NotFound(String),
    /// A name or identifier prefix that matches several devices.
    Ambiguous {
        selector: String,
        ids: Vec<String>,
    },
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(
                formatter,
                "{MAX_DEVICES} devices are already paired; revoke one first"
            ),
            Self::NotFound(selector) => write!(formatter, "no paired device matches '{selector}'"),
            Self::Ambiguous { selector, ids } => write!(
                formatter,
                "'{selector}' matches several devices; revoke by identifier: {}",
                ids.join(", ")
            ),
        }
    }
}

impl std::error::Error for DeviceError {}

/// A device just paired, and the record it replaced.
#[derive(Debug)]
pub struct Enrollment {
    pub record: DeviceRecord,
    /// The only copy of the new static token; `None` for a rotating device.
    pub token: Option<Secret>,
    /// The first tokens of a rotating device: a refresh token and its access
    /// token. `None` for a static device.
    pub issued: Option<Issued>,
    /// Identifier of the earlier record from the same install, now gone.
    pub replaced: Option<String>,
}

/// Tokens handed to a device: the first pair of a pairing, or a rotation.
#[derive(Debug)]
pub struct Issued {
    /// The only copy of the new refresh token.
    pub refresh_token: Secret,
    /// The only copy of the new access token.
    pub access_token: Secret,
    pub access_expires_at: DateTime<Utc>,
    /// The rotation the device acknowledges with `token_ack`; `None` for the
    /// first tokens of a pairing, which retire nothing.
    pub rotation_id: Option<String>,
}

/// How a device presented a credential to `authenticate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credential<'a> {
    /// A long-lived token: the static token of a device that cannot rotate, or
    /// the refresh token of one that can.
    Refresh(&'a str),
    /// A short-lived access token, for reconnecting without rotating.
    Access(&'a str),
}

/// An accepted `authenticate`.
#[derive(Debug)]
pub struct Accepted {
    pub record: DeviceRecord,
    /// New tokens to hand back, when the record rotated.
    pub issued: Option<Issued>,
    /// When the access token this connection holds stops being accepted.
    pub access_expires_at: Option<DateTime<Utc>>,
}

/// Why an `authenticate` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticateError {
    /// Unknown device, wrong token, expired static pairing, or an access
    /// token this host never issued. All indistinguishable, as before.
    Unauthorized,
    /// The access token this record issued has expired or was replaced:
    /// authenticate with the refresh token instead of pairing again.
    AccessExpired,
    /// A refresh token this family retired for good: the whole family was
    /// revoked by this call.
    Replayed,
    /// The family retired as many tokens as the host keeps history for, so it
    /// was revoked rather than let an issued token be answered as a wrong one.
    /// The device pairs again.
    Exhausted,
}

/// What `Devices::acknowledge` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acknowledgement {
    /// Nothing to retire: unknown device, a static one, or a stale rotation.
    Ignored,
    /// The previous refresh token is retired for good.
    Retired,
    /// The history was full, so the family was removed instead.
    Exhausted,
}

/// The set of paired devices, as held in memory and written to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Devices {
    version: u32,
    devices: Vec<DeviceRecord>,
}

impl Default for Devices {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            devices: Vec::new(),
        }
    }
}

impl Devices {
    pub fn list(&self) -> &[DeviceRecord] {
        &self.devices
    }

    /// Add a device and mint its token. The returned token is the only copy.
    /// Expired devices are forgotten first, so they never hold a place.
    pub fn enroll(&mut self, name: &str) -> Result<(DeviceRecord, Secret), DeviceError> {
        let Enrollment { record, token, .. } = self.enroll_install(name, None)?;
        Ok((
            record,
            token.expect("a pairing without rotation mints a static token"),
        ))
    }

    /// [`Self::enroll`] for a device that names its app install. A record
    /// already paired from that install is replaced: it is dropped with its
    /// token, and the new record takes its place under a fresh identifier,
    /// so one install never holds more than one record.
    pub fn enroll_install(
        &mut self,
        name: &str,
        install_id: Option<&str>,
    ) -> Result<Enrollment, DeviceError> {
        self.enroll_as(name, install_id, &DeviceProfile::default())
    }

    /// [`Self::enroll_install`] for a device that said what it is.
    pub fn enroll_as(
        &mut self,
        name: &str,
        install_id: Option<&str>,
        profile: &DeviceProfile,
    ) -> Result<Enrollment, DeviceError> {
        self.enroll_device(name, install_id, profile, false)
    }

    /// [`Self::enroll_install`] for a device that asked for `token_rotation`:
    /// it gets a refresh token and an access token, and no expiry.
    pub fn enroll_rotating(
        &mut self,
        name: &str,
        install_id: Option<&str>,
        profile: &DeviceProfile,
    ) -> Result<Enrollment, DeviceError> {
        self.enroll_device(name, install_id, profile, true)
    }

    fn enroll_device(
        &mut self,
        name: &str,
        install_id: Option<&str>,
        profile: &DeviceProfile,
        rotate: bool,
    ) -> Result<Enrollment, DeviceError> {
        let now = Utc::now();
        self.devices.retain(|record| !record.is_expired(now));
        let install_sha256 = install_id.and_then(install_digest);
        let replaced = install_sha256.as_ref().and_then(|install| {
            self.devices
                .iter()
                .position(|record| record.install_sha256.as_ref() == Some(install))
        });
        if replaced.is_none() && self.devices.len() >= MAX_DEVICES {
            return Err(DeviceError::Full);
        }
        let (token_sha256, token, rotation, issued) = if rotate {
            let (family, issued) = TokenFamily::new(now);
            (None, None, Some(family), Some(issued))
        } else {
            let token = random_token();
            let digest_hex = hex::encode(digest(token.expose().as_bytes()));
            (Some(digest_hex), Some(token), None, None)
        };
        let record = DeviceRecord {
            acp: false,
            id: random_id(),
            name: sanitize_name(name),
            token_sha256,
            paired_at: now,
            install_sha256,
            last_seen_at: Some(now),
            kind: profile.kind,
            platform: profile.platform.clone(),
            host_id: profile.host_id.clone(),
            rotation,
        };
        let replaced = replaced.map(|index| self.devices.remove(index).id);
        self.devices.push(record.clone());
        Ok(Enrollment {
            record,
            token,
            issued,
            replaced,
        })
    }

    /// Record that `device_id` connected at `now`. True when the record
    /// changed and is worth writing back; within [`LAST_SEEN_RESOLUTION`] of
    /// the last record nothing changes.
    pub fn mark_seen(&mut self, device_id: &str, now: DateTime<Utc>) -> bool {
        let Some(record) = self.devices.iter_mut().find(|r| r.id == device_id) else {
            return false;
        };
        if record
            .last_seen_at
            .is_some_and(|seen| now - seen < LAST_SEEN_RESOLUTION)
        {
            return false;
        }
        record.last_seen_at = Some(now);
        true
    }

    /// The device `token` belongs to, if `device_id` is paired with a static
    /// token, the token is its own and the pairing has not expired at `now`.
    /// An unknown device costs the same comparison as a known one.
    pub fn verify(
        &self,
        device_id: &str,
        token: &str,
        now: DateTime<Utc>,
    ) -> Option<&DeviceRecord> {
        let presented = digest(token.as_bytes());
        let record = self.devices.iter().find(|record| record.id == device_id);
        let mut expected = [0u8; 32];
        let known = record.is_some_and(|record| {
            record
                .token_sha256
                .as_deref()
                .is_some_and(|stored| hex::decode_to_slice(stored, &mut expected).is_ok())
        });
        let matches = constant_time_eq(&presented, &expected);
        record.filter(|record| known && matches && !record.is_expired(now))
    }

    /// Accept an `authenticate`. An access token confirms the record as it is;
    /// a refresh token (or the static token of a device that cannot rotate)
    /// rotates the record, and this set must then be saved.
    ///
    /// The rotation rules a client relies on:
    /// - The newest refresh token and, until it is acknowledged or used, the
    ///   one it replaced are both accepted.
    /// - Using the newest token retires the one before it, so a copy of it
    ///   presented later is a replay; acknowledging the newest retires it too.
    /// - A retry with the previous token inside that window is idempotent: it
    ///   gets the very same answer again and changes nothing. A second holder
    ///   of the previous token therefore gains nothing the device did not
    ///   already receive, and cannot rotate the stored token away. Without the
    ///   answer in memory (a restart since the rotation) the retry is a replay.
    /// - Any token this family retired is a replay, and removes the whole
    ///   record. A token this host never issued is only refused, so a guessed
    ///   device identifier cannot revoke a device.
    /// - An access token that is not the live one is answered
    ///   [`AuthenticateError::AccessExpired`], so a route holding the one a
    ///   rotation replaced refreshes instead of pairing again.
    /// - A family that has retired as many tokens as it may keep is removed
    ///   ([`AuthenticateError::Exhausted`]): forgetting a retired token would
    ///   answer it `unauthorized` instead of a replay.
    pub fn accept(
        &mut self,
        device_id: &str,
        credential: Credential<'_>,
        now: DateTime<Utc>,
    ) -> Result<Accepted, AuthenticateError> {
        let Some(index) = self
            .devices
            .iter()
            .position(|record| record.id == device_id)
        else {
            return Err(AuthenticateError::Unauthorized);
        };
        let record = &self.devices[index];
        if record.is_expired(now) {
            return Err(AuthenticateError::Unauthorized);
        }
        let presented = match credential {
            Credential::Refresh(token) => digest(token.as_bytes()),
            Credential::Access(token) => digest(token.as_bytes()),
        };
        let Some(family) = record.rotation.clone() else {
            // A record that cannot rotate takes only its static token.
            if matches!(credential, Credential::Access(_)) {
                return Err(AuthenticateError::Unauthorized);
            }
            let matches = record
                .token_sha256
                .as_deref()
                .is_some_and(|stored| TokenFamily::digest_matches(stored, &presented));
            return if matches {
                Ok(Accepted {
                    record: record.clone(),
                    issued: None,
                    access_expires_at: None,
                })
            } else {
                Err(AuthenticateError::Unauthorized)
            };
        };
        if matches!(credential, Credential::Access(_)) {
            // The live access token reconnects without rotating.
            if let Some(access) = family.access.as_ref()
                && TokenFamily::digest_matches(&access.sha256, &presented)
            {
                return if now >= access.expires_at {
                    Err(AuthenticateError::AccessExpired)
                } else {
                    Ok(Accepted {
                        record: record.clone(),
                        issued: None,
                        access_expires_at: Some(access.expires_at),
                    })
                };
            }
            // A rotating record never answers an access token `unauthorized`:
            // it cannot tell the token a rotation replaced, one a restart
            // forgot, and a wrong one apart, and pairing again is never the
            // right answer to an access token. `access_expired` sends the
            // device to its refresh token, which is always valid.
            return Err(AuthenticateError::AccessExpired);
        }
        let from_current = TokenFamily::digest_matches(&family.current, &presented);
        let from_previous = family
            .previous
            .as_deref()
            .is_some_and(|previous| TokenFamily::digest_matches(previous, &presented));
        if !from_current && !from_previous {
            if family.is_retired(&presented) {
                // A copy of a retired token: the family is worth nothing now.
                self.devices.remove(index);
                return Err(AuthenticateError::Replayed);
            }
            return Err(AuthenticateError::Unauthorized);
        }
        if from_previous {
            // The two-phase window: hand back the same answer, unchanged.
            return match family.pending_answer() {
                Some(issued) => {
                    let access_expires_at = issued.access_expires_at;
                    Ok(Accepted {
                        record: record.clone(),
                        issued: Some(issued),
                        access_expires_at: Some(access_expires_at),
                    })
                }
                // Acknowledged already, or a restart lost the answer: a copy
                // of the previous token is in use.
                None => {
                    self.devices.remove(index);
                    Err(AuthenticateError::Replayed)
                }
            };
        }
        let mut family = family;
        let Some(issued) = family.rotate(now) else {
            // No room left to remember this one: revoke rather than forget.
            self.devices.remove(index);
            return Err(AuthenticateError::Exhausted);
        };
        let access_expires_at = issued.access_expires_at;
        let mut record = record.clone();
        record.rotation = Some(family);
        self.devices[index] = record.clone();
        Ok(Accepted {
            record,
            access_expires_at: Some(access_expires_at),
            issued: Some(issued),
        })
    }

    /// Retire the previous refresh token after the device acknowledged the
    /// rotation that issued the current one. [`Acknowledgement::Ignored`] when
    /// nothing changed: the record is gone, does not rotate, or the
    /// acknowledgement names a rotation another connection replaced, which is
    /// not an error.
    pub fn acknowledge(&mut self, device_id: &str, rotation_id: &str) -> Acknowledgement {
        let Some(index) = self.devices.iter().position(|r| r.id == device_id) else {
            return Acknowledgement::Ignored;
        };
        let exhausted = {
            let Some(family) = self.devices[index].rotation.as_mut() else {
                return Acknowledgement::Ignored;
            };
            if family.rotation_id.as_deref() != Some(rotation_id) {
                return Acknowledgement::Ignored;
            }
            family.rotation_id = None;
            family.pending = None;
            match family.previous.take() {
                Some(previous) if family.has_room() => {
                    family.retire(previous);
                    false
                }
                Some(_) => true,
                None => false,
            }
        };
        if exhausted {
            self.devices.remove(index);
            return Acknowledgement::Exhausted;
        }
        Acknowledgement::Retired
    }

    /// Resolve what a user typed: a full identifier, a unique identifier
    /// prefix of at least four characters, or a unique exact name.
    pub fn select(&self, selector: &str) -> Result<&DeviceRecord, DeviceError> {
        if let Some(record) = self.devices.iter().find(|record| record.id == selector) {
            return Ok(record);
        }
        let matches: Vec<&DeviceRecord> = self
            .devices
            .iter()
            .filter(|record| {
                record.name == selector || (selector.len() >= 4 && record.id.starts_with(selector))
            })
            .collect();
        match matches.as_slice() {
            [] => Err(DeviceError::NotFound(selector.to_string())),
            [only] => Ok(only),
            several => Err(DeviceError::Ambiguous {
                selector: selector.to_string(),
                ids: several.iter().map(|record| record.id.clone()).collect(),
            }),
        }
    }

    /// Change only the host-controlled grant, never client-supplied metadata.
    pub fn set_acp(&mut self, selector: &str, allow: bool) -> Result<DeviceRecord, DeviceError> {
        let id = self.select(selector)?.id.clone();
        let record = self
            .devices
            .iter_mut()
            .find(|record| record.id == id)
            .unwrap();
        record.acp = allow;
        Ok(record.clone())
    }

    pub fn remove(&mut self, device_id: &str) -> Option<DeviceRecord> {
        let index = self.devices.iter().position(|r| r.id == device_id)?;
        Some(self.devices.remove(index))
    }
}

/// Keep a device-supplied label printable and bounded. It is shown in a
/// terminal, so control characters (including escape sequences) are dropped.
pub(super) fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_DEVICE_NAME_CHARS)
        .collect();
    if cleaned.is_empty() {
        "unnamed device".to_string()
    } else {
        cleaned
    }
}

/// Keep a device-supplied platform a short lowercase token: it is shown in
/// a terminal and matched by the apps. Empty becomes [`UNKNOWN_PLATFORM`].
pub(super) fn sanitize_platform(platform: &str) -> String {
    let cleaned: String = platform
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(MAX_PLATFORM_CHARS)
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        UNKNOWN_PLATFORM.to_owned()
    } else {
        cleaned
    }
}

/// A host identifier the device sent, or `None` when it is out of the same
/// bounds as an install identifier. Kept as sent: it names a machine the
/// owner may pair with in turn, not the user.
fn sanitize_host_id(host_id: &str) -> Option<String> {
    let length = host_id.chars().count();
    let valid = (MIN_INSTALL_ID_CHARS..=MAX_INSTALL_ID_CHARS).contains(&length)
        && host_id.chars().all(|c| c.is_ascii_graphic());
    valid.then(|| host_id.to_owned())
}

/// Digest of an install identifier the device sent, or `None` when it is out
/// of bounds. Only the digest is stored: the identifier says nothing about
/// the user, but the host has no reason to keep it.
fn install_digest(install_id: &str) -> Option<String> {
    let length = install_id.chars().count();
    let valid = (MIN_INSTALL_ID_CHARS..=MAX_INSTALL_ID_CHARS).contains(&length)
        && install_id.chars().all(|c| c.is_ascii_graphic());
    valid.then(|| hex::encode(digest(install_id.as_bytes())))
}

/// A note that one family must never authenticate again, kept in its own file
/// next to the store. It exists for the one case where the store write that
/// removed a family fails: the note is written instead, before the family's
/// connections are closed, and applied at startup before anything is served, so
/// a restart cannot bring a revoked family back.
#[derive(Debug, Clone)]
pub struct RevocationNotes {
    path: PathBuf,
}

/// Largest number of pending revocations kept; there cannot be more devices.
const MAX_REVOCATION_NOTES: usize = MAX_DEVICES;
const NOTES_VERSION: u32 = 1;
const MAX_NOTES_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Notes {
    version: u32,
    families: Vec<String>,
}

impl Default for Notes {
    fn default() -> Self {
        Self {
            version: NOTES_VERSION,
            families: Vec::new(),
        }
    }
}

impl RevocationNotes {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The families noted so far; a missing file is none.
    pub fn load(&self) -> Result<Vec<String>> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => ensure!(
                metadata.is_file() && owner_only(&metadata),
                "revocation notes {} must be a regular owner-only file (0600)",
                self.path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("failed to inspect the revocation notes"),
        }
        let notes: Notes = read_private_json(&self.path, MAX_NOTES_BYTES)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        ensure!(notes.version == NOTES_VERSION, "unsupported notes version");
        Ok(notes.families)
    }

    /// Note `device_id`, replacing the file atomically. A note already there is
    /// left as it is.
    pub fn add(&self, device_id: &str) -> Result<()> {
        let mut notes = Notes {
            version: NOTES_VERSION,
            families: self.load().unwrap_or_default(),
        };
        if notes.families.iter().any(|known| known == device_id) {
            return Ok(());
        }
        notes.families.push(device_id.to_owned());
        if notes.families.len() > MAX_REVOCATION_NOTES {
            notes.families.remove(0);
        }
        let parent = self
            .path
            .parent()
            .context("revocation notes have no parent")?;
        ensure_private_directory(parent, "remote gateway")?;
        publish_private_json(&self.path, &notes)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }

    /// Drop the notes: the store on disk is authoritative again. A missing file
    /// is fine.
    pub fn clear(&self) -> Result<()> {
        match remove_if_exists(&self.path) {
            Ok(()) => Ok(()),
            Err(error) => Err(error).context("failed to remove the revocation notes"),
        }
    }
}

/// Apply the notes left by a store write that failed: every noted family is
/// gone from `devices` before the gateway serves anyone, and the store is
/// rewritten. A store that cannot be rewritten keeps the notes for the next
/// start, and the families stay refused in memory.
pub fn apply_revocation_notes(
    store: &DeviceStore,
    notes: &RevocationNotes,
    mut devices: Devices,
) -> Devices {
    let noted = match notes.load() {
        Ok(noted) => noted,
        Err(error) => {
            crate::dbg_log!("remote gateway: ERROR: cannot read revocation notes: {error:#}");
            return devices;
        }
    };
    if noted.is_empty() {
        return devices;
    }
    let mut removed = false;
    for device_id in &noted {
        removed |= devices.remove(device_id).is_some();
    }
    if let Err(error) = store.save(&devices) {
        crate::dbg_log!(
            "remote gateway: ERROR: could not write the store while applying {} revocation note(s); they stay refused and the notes are kept: {error:#}",
            noted.len()
        );
        return devices;
    }
    if removed {
        crate::dbg_log!(
            "remote gateway: applied {} revocation note(s) left by a failed store write",
            noted.len()
        );
    }
    if let Err(error) = notes.clear() {
        crate::dbg_log!("remote gateway: could not clear the revocation notes: {error:#}");
    }
    devices
}

/// The owner-only file behind [`Devices`].
#[derive(Debug, Clone)]
pub struct DeviceStore {
    path: PathBuf,
}

impl DeviceStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the paired devices; a missing file is an empty set. A file that
    /// anyone but its owner can read is refused rather than trusted.
    pub fn load(&self) -> Result<Devices> {
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Devices::default());
            }
            Err(error) => return Err(error).context("failed to inspect the device store"),
        };
        ensure!(
            metadata.is_file() && owner_only(&metadata),
            "device store {} must be a regular owner-only file (0600)",
            self.path.display()
        );
        let devices: Devices = read_private_json(&self.path, MAX_STORE_BYTES)
            .with_context(|| format!("failed to read device store {}", self.path.display()))?;
        if devices.version != STORE_VERSION {
            bail!("unsupported device store version {}", devices.version);
        }
        ensure!(
            devices.devices.len() <= MAX_DEVICES,
            "device store holds too many devices"
        );
        Ok(devices)
    }

    /// Replace the store atomically. The file is created 0600 under a
    /// temporary name inside the private directory and renamed into place,
    /// so it is never visible half-written or with wider permissions.
    pub fn save(&self, devices: &Devices) -> Result<()> {
        let parent = self.path.parent().context("device store has no parent")?;
        ensure_private_directory(parent, "remote gateway")?;
        publish_private_json(&self.path, devices)
            .with_context(|| format!("failed to write device store {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn store() -> (tempfile::TempDir, DeviceStore) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = DeviceStore::new(dir.path().join("remote").join("devices.json"));
        (dir, store)
    }

    #[test]
    fn enrolled_token_verifies_and_only_its_digest_is_stored() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let (record, token) = devices.enroll("Lars's iPhone").unwrap();
        store.save(&devices).unwrap();

        assert_eq!(record.id.len(), 16);
        assert_eq!(token.expose().len(), 43);
        let on_disk = std::fs::read_to_string(store.path()).unwrap();
        assert!(!on_disk.contains(token.expose()));
        assert!(on_disk.contains(&hex::encode(digest(token.expose().as_bytes()))));

        let loaded = store.load().unwrap();
        assert_eq!(loaded, devices);
        assert_eq!(
            loaded
                .verify(&record.id, token.expose(), Utc::now())
                .map(|r| &r.id),
            Some(&record.id)
        );
    }

    #[test]
    fn verification_rejects_wrong_tokens_unknown_devices_and_swapped_tokens() {
        let mut devices = Devices::default();
        let (first, first_token) = devices.enroll("first").unwrap();
        let (second, second_token) = devices.enroll("second").unwrap();
        assert!(devices.verify(&first.id, "wrong", Utc::now()).is_none());
        assert!(devices.verify(&first.id, "", Utc::now()).is_none());
        assert!(
            devices
                .verify("ffffffffffffffff", first_token.expose(), Utc::now())
                .is_none()
        );
        // A valid token is bound to its own device.
        assert!(
            devices
                .verify(&first.id, second_token.expose(), Utc::now())
                .is_none()
        );
        assert!(
            devices
                .verify(&second.id, first_token.expose(), Utc::now())
                .is_none()
        );
        // The stored digest is not itself a credential.
        assert!(
            devices
                .verify(
                    &first.id,
                    first.token_sha256.as_deref().expect("a static token"),
                    Utc::now()
                )
                .is_none()
        );
        assert!(
            devices
                .verify(&second.id, second_token.expose(), Utc::now())
                .is_some()
        );
    }

    #[test]
    fn a_corrupt_digest_never_matches_the_zero_digest_fallback() {
        let mut devices = Devices::default();
        let (record, _token) = devices.enroll("phone").unwrap();
        devices.devices[0].token_sha256 = Some("not-hex".into());
        assert!(devices.verify(&record.id, "anything", Utc::now()).is_none());
    }

    #[test]
    fn a_pairing_expires_after_its_lifetime_and_frees_its_place() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        let expires_at = record.expires_at().expect("a static pairing expires");
        assert_eq!(expires_at, record.paired_at + DEVICE_LIFETIME);
        let just_before = expires_at - chrono::Duration::seconds(1);
        assert!(
            devices
                .verify(&record.id, token.expose(), just_before)
                .is_some()
        );
        assert!(
            devices
                .verify(&record.id, token.expose(), expires_at)
                .is_none()
        );

        // A store full of expired devices still takes a new one.
        for _ in 1..MAX_DEVICES {
            devices.enroll("other").unwrap();
        }
        assert_eq!(devices.enroll("late").unwrap_err(), DeviceError::Full);
        for record in &mut devices.devices {
            record.paired_at -= DEVICE_LIFETIME;
        }
        let (fresh, _) = devices.enroll("late").unwrap();
        assert_eq!(devices.list(), [fresh]);
    }

    #[test]
    fn revoked_device_no_longer_verifies_after_reload() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let (record, token) = devices.enroll("phone").unwrap();
        store.save(&devices).unwrap();
        assert_eq!(devices.remove(&record.id).unwrap().id, record.id);
        store.save(&devices).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.list().is_empty());
        assert!(
            loaded
                .verify(&record.id, token.expose(), Utc::now())
                .is_none()
        );
        assert!(devices.remove(&record.id).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn store_and_directory_are_owner_only_from_the_first_write() {
        let (dir, store) = store();
        let mut devices = store.load().unwrap();
        devices.enroll("phone").unwrap();
        store.save(&devices).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.path()), 0o600);
        assert_eq!(mode(&dir.path().join("remote")), 0o700);
        // Atomic replacement leaves no temporary file behind.
        store.save(&devices).unwrap();
        assert_eq!(
            std::fs::read_dir(dir.path().join("remote"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(mode(store.path()), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn widened_permissions_are_refused_not_repaired() {
        let (dir, store) = store();
        let mut devices = store.load().unwrap();
        devices.enroll("phone").unwrap();
        store.save(&devices).unwrap();

        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("owner-only"), "{error}");
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.load().is_ok());

        // A world-accessible directory is refused before anything is written.
        let remote = dir.path().join("remote");
        std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = store.save(&devices).unwrap_err().to_string();
        assert!(error.contains("private"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_store_is_refused() {
        let (dir, store) = store();
        let target = dir.path().join("elsewhere.json");
        std::fs::write(&target, r#"{"version":1,"devices":[]}"#).unwrap();
        std::fs::create_dir(dir.path().join("remote")).unwrap();
        std::os::unix::fs::symlink(&target, store.path()).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn selection_accepts_ids_prefixes_and_unique_names_only() {
        let mut devices = Devices::default();
        let (first, _) = devices.enroll("phone").unwrap();
        let (second, _) = devices.enroll("phone").unwrap();
        let (third, _) = devices.enroll("tablet").unwrap();
        assert_eq!(devices.select(&first.id).unwrap().id, first.id);
        assert_eq!(devices.select("tablet").unwrap().id, third.id);
        assert_eq!(devices.select(&third.id[..8]).unwrap().id, third.id);
        assert_eq!(
            devices.select("phone"),
            Err(DeviceError::Ambiguous {
                selector: "phone".into(),
                ids: vec![first.id.clone(), second.id.clone()],
            })
        );
        assert_eq!(
            devices.select("laptop"),
            Err(DeviceError::NotFound("laptop".into()))
        );
        // Too short to be a prefix, and never an empty-string wildcard.
        assert!(devices.select(&third.id[..3]).is_err());
        assert!(devices.select("").is_err());
    }

    #[test]
    fn names_are_labels_sanitized_for_a_terminal() {
        let mut devices = Devices::default();
        let (record, _) = devices.enroll("  my\u{1b}[31m  phone\n").unwrap();
        assert_eq!(record.name, "my[31m phone");
        let (record, _) = devices.enroll("\u{7}\n").unwrap();
        assert_eq!(record.name, "unnamed device");
        let (record, _) = devices.enroll(&"x".repeat(500)).unwrap();
        assert_eq!(record.name.chars().count(), MAX_DEVICE_NAME_CHARS);
    }

    #[test]
    fn enrollment_stops_at_the_device_limit() {
        let mut devices = Devices::default();
        for index in 0..MAX_DEVICES {
            devices.enroll(&format!("device {index}")).unwrap();
        }
        assert_eq!(devices.enroll("one more"), Err(DeviceError::Full));
        assert_eq!(devices.list().len(), MAX_DEVICES);
    }

    #[test]
    fn the_same_install_pairs_into_one_record_and_revokes_its_old_token() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let install = "A1B2C3D4-0000-4000-8000-1234567890AB";
        let first = devices.enroll_install("iPhone", Some(install)).unwrap();
        let other = devices
            .enroll_install("iPad", Some(&install.replace('A', "B")))
            .unwrap();
        let again = devices
            .enroll_install("Lars's iPhone", Some(install))
            .unwrap();
        store.save(&devices).unwrap();

        assert_eq!(again.replaced.as_deref(), Some(first.record.id.as_str()));
        assert_eq!(other.replaced, None);
        let loaded = store.load().unwrap();
        let ids: Vec<&str> = loaded.list().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, [other.record.id.as_str(), again.record.id.as_str()]);
        assert_eq!(loaded.list()[1].name, "Lars's iPhone");
        let now = Utc::now();
        assert!(
            loaded
                .verify(
                    &first.record.id,
                    first.token.as_ref().expect("a static token").expose(),
                    now
                )
                .is_none()
        );
        assert!(
            loaded
                .verify(
                    &again.record.id,
                    again.token.as_ref().expect("a static token").expose(),
                    now
                )
                .is_some()
        );
        // Only a digest of the install identifier is stored.
        assert!(
            !std::fs::read_to_string(store.path())
                .unwrap()
                .contains(install)
        );
    }

    #[test]
    fn a_full_store_still_takes_an_install_that_pairs_again() {
        let mut devices = Devices::default();
        let install = "install-identifier-0001";
        devices.enroll_install("phone", Some(install)).unwrap();
        for index in 1..MAX_DEVICES {
            devices.enroll(&format!("device {index}")).unwrap();
        }
        assert_eq!(devices.enroll("one more").unwrap_err(), DeviceError::Full);
        let again = devices.enroll_install("phone", Some(install)).unwrap();
        assert!(again.replaced.is_some());
        assert_eq!(devices.list().len(), MAX_DEVICES);
    }

    #[test]
    fn install_ids_out_of_bounds_are_ignored_and_never_match_each_other() {
        let mut devices = Devices::default();
        for unusable in ["", "short", "has spaces in it, too", &"x".repeat(200)] {
            let first = devices.enroll_install("phone", Some(unusable)).unwrap();
            let second = devices.enroll_install("phone", Some(unusable)).unwrap();
            assert_eq!(
                (first.replaced, second.replaced),
                (None, None),
                "{unusable}"
            );
        }
        assert_eq!(devices.list().len(), 8);
    }

    #[test]
    fn last_seen_is_kept_coarsely_and_older_records_load_without_it() {
        let mut devices = Devices::default();
        let (record, _) = devices.enroll("phone").unwrap();
        let paired = record.last_seen_at.unwrap();
        assert!(!devices.mark_seen(&record.id, paired + chrono::Duration::minutes(1)));
        let later = paired + LAST_SEEN_RESOLUTION;
        assert!(devices.mark_seen(&record.id, later));
        assert_eq!(devices.list()[0].last_seen_at, Some(later));
        assert!(!devices.mark_seen("ffffffffffffffff", later));

        let stored = r#"{"version":1,"devices":[{"id":"0011223344556677","name":"iPhone","token_sha256":"00","paired_at":"2026-10-01T10:00:00Z"}]}"#;
        let old: Devices = serde_json::from_str(stored).unwrap();
        assert_eq!(old.list()[0].last_seen_at, None);
    }

    #[test]
    fn records_stored_before_devices_said_their_kind_load_as_phones() {
        let stored = r#"{"version":1,"devices":[{"id":"0011223344556677","name":"iPhone","token_sha256":"00","paired_at":"2026-10-01T10:00:00Z","last_seen_at":"2026-10-02T08:00:00Z"}]}"#;
        let old: Devices = serde_json::from_str(stored).unwrap();
        let record = &old.list()[0];
        assert_eq!(record.kind, DeviceKind::Phone);
        assert_eq!(record.platform, UNKNOWN_PLATFORM);
        assert_eq!(record.host_id, None);
        assert_eq!(
            record.last_seen_at,
            Some("2026-10-02T08:00:00Z".parse().unwrap())
        );
        // Written back, the defaults are explicit and nothing else changes.
        let written = serde_json::to_value(&old).unwrap();
        assert_eq!(written["devices"][0]["kind"], "phone");
        assert_eq!(written["devices"][0]["platform"], "unknown");
        assert!(written["devices"][0].get("host_id").is_none());
        let again: Devices = serde_json::from_value(written).unwrap();
        assert_eq!(again, old);
    }

    #[test]
    fn a_kind_written_by_a_newer_gateway_does_not_break_store_loading() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        devices.enroll("Lars's desktop").unwrap();
        store.save(&devices).unwrap();

        let stored = std::fs::read_to_string(store.path()).unwrap();
        let future_kind = stored.replace(r#""kind":"phone""#, r#""kind":"tablet""#);
        assert_ne!(stored, future_kind, "test fixture must replace the kind");
        std::fs::write(store.path(), future_kind).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.list()[0].kind, DeviceKind::Unknown);
        assert_eq!(loaded.list()[0].kind.to_string(), "unknown");
        // Rewriting the store gives the catch-all a stable, known spelling.
        store.save(&loaded).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(store.path()).unwrap()).unwrap();
        assert_eq!(written["devices"][0]["kind"], "unknown");
    }

    #[test]
    fn a_desktop_pairs_with_its_kind_platform_and_host_and_keeps_them() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let profile = DeviceProfile::from_wire(
            Some("desktop"),
            Some("Windows"),
            Some("host-0123456789abcdef"),
        );
        let enrolled = devices
            .enroll_as("Lars's PC", Some("install-identifier-0002"), &profile)
            .unwrap();
        store.save(&devices).unwrap();
        let loaded = store.load().unwrap();
        let record = &loaded.list()[0];
        assert_eq!(record.id, enrolled.record.id);
        assert_eq!(record.kind, DeviceKind::Desktop);
        assert_eq!(record.platform, "windows");
        assert_eq!(record.host_id.as_deref(), Some("host-0123456789abcdef"));
        // Pairing again from the same install takes the new profile.
        let cli = DeviceProfile::from_wire(Some("cli"), Some("linux"), None);
        let again = devices
            .enroll_as("Lars's PC", Some("install-identifier-0002"), &cli)
            .unwrap();
        assert_eq!(again.replaced.as_deref(), Some(record.id.as_str()));
        assert_eq!(devices.list().len(), 1);
        assert_eq!(devices.list()[0].kind, DeviceKind::Cli);
        assert_eq!(devices.list()[0].host_id, None);
    }

    #[test]
    fn device_profiles_keep_only_usable_values() {
        assert_eq!(
            DeviceProfile::from_wire(None, None, None),
            DeviceProfile::default()
        );
        assert_eq!(DeviceKind::parse(Some(" Desktop ")), DeviceKind::Desktop);
        assert_eq!(DeviceKind::parse(Some("CLI")), DeviceKind::Cli);
        // A kind from a newer client is a phone, as before devices said it.
        assert_eq!(DeviceKind::parse(Some("toaster")), DeviceKind::Phone);
        let profile =
            DeviceProfile::from_wire(Some("desktop"), Some("mac\u{1b}[31mOS!"), Some("short"));
        assert_eq!(profile.platform, "mac31mos");
        assert_eq!(profile.host_id, None);
        let long = DeviceProfile::from_wire(None, Some(&"x".repeat(100)), Some(&"h".repeat(200)));
        assert_eq!(long.platform.len(), MAX_PLATFORM_CHARS);
        assert_eq!(long.host_id, None);
        assert_eq!(
            DeviceProfile::from_wire(None, Some(""), None).platform,
            "unknown"
        );
        assert_eq!(
            DeviceProfile::from_wire(None, None, Some("has spaces in the host id")).host_id,
            None
        );
    }

    #[test]
    fn last_seen_moves_with_every_connection_past_the_resolution() {
        let mut devices = Devices::default();
        let (record, _) = devices.enroll("phone").unwrap();
        let mut at = record.last_seen_at.unwrap();
        for _ in 0..3 {
            at += LAST_SEEN_RESOLUTION + chrono::Duration::seconds(1);
            assert!(devices.mark_seen(&record.id, at));
            assert_eq!(devices.list()[0].last_seen_at, Some(at));
        }
        // A record stored without it is filled on the first connection.
        devices.devices[0].last_seen_at = None;
        assert!(devices.mark_seen(&record.id, at));
        assert_eq!(devices.list()[0].last_seen_at, Some(at));
    }

    #[test]
    fn debug_output_omits_the_token_digest() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        let debug = format!("{devices:?} {record:?}");
        assert!(!debug.contains(record.token_sha256.as_deref().expect("a static token")));
        assert!(!debug.contains(token.expose()));
    }
}

#[cfg(test)]
mod acp_grant_tests {
    use super::*;
    #[test]
    fn grants_default_off_and_persist_without_client_control() {
        let mut devices = Devices::default();
        let enrolled = devices
            .enroll_as(
                "desktop",
                None,
                &DeviceProfile::from_wire(Some("desktop"), None, None),
            )
            .unwrap();
        assert!(!enrolled.record.acp);
        let mut old = serde_json::to_value(&devices).unwrap();
        old["devices"][0].as_object_mut().unwrap().remove("acp");
        let old: Devices = serde_json::from_value(old).unwrap();
        assert!(!old.list()[0].acp);
        devices.set_acp(&enrolled.record.id, true).unwrap();
        let loaded: Devices =
            serde_json::from_slice(&serde_json::to_vec(&devices).unwrap()).unwrap();
        assert!(
            loaded
                .verify(
                    &enrolled.record.id,
                    enrolled.token.as_ref().expect("a static token").expose(),
                    Utc::now()
                )
                .unwrap()
                .acp
        );
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    fn store() -> (tempfile::TempDir, DeviceStore) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = DeviceStore::new(dir.path().join("remote").join("devices.json"));
        (dir, store)
    }

    /// A rotating device, the tokens it paired with, and the set holding it.
    fn rotating() -> (Devices, DeviceRecord, Secret, Secret) {
        let mut devices = Devices::default();
        let enrolled = devices
            .enroll_rotating("desktop", None, &DeviceProfile::default())
            .unwrap();
        let issued = enrolled.issued.expect("a rotating pairing issues tokens");
        (
            devices,
            enrolled.record,
            issued.refresh_token,
            issued.access_token,
        )
    }

    #[test]
    fn a_rotating_pairing_has_no_static_token_and_never_expires() {
        let mut devices = Devices::default();
        let enrolled = devices
            .enroll_rotating("Lars's PC", None, &DeviceProfile::default())
            .unwrap();
        assert!(enrolled.token.is_none());
        let issued = enrolled.issued.expect("a rotating pairing issues tokens");
        assert!(issued.rotation_id.is_none());
        assert_eq!(issued.refresh_token.expose().len(), 43);
        assert_eq!(issued.access_token.expose().len(), 43);
        assert_eq!(
            issued.access_expires_at,
            enrolled.record.paired_at + ACCESS_TOKEN_LIFETIME
        );
        assert!(enrolled.record.rotates());
        assert_eq!(enrolled.record.expires_at(), None);
        // Long past the 30 days a static pairing would have lasted.
        let later = enrolled.record.paired_at + chrono::Duration::days(400);
        assert!(
            devices
                .accept(
                    &enrolled.record.id,
                    Credential::Refresh(issued.refresh_token.expose()),
                    later
                )
                .is_ok()
        );
    }

    #[test]
    fn the_access_token_reconnects_without_rotating_until_it_expires() {
        let (mut devices, record, _refresh, access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(14);
        let accepted = devices
            .accept(&record.id, Credential::Access(access.expose()), at)
            .unwrap();
        assert!(accepted.issued.is_none());
        assert_eq!(
            accepted.record.id, record.id,
            "an access token leaves the record as it is"
        );

        let at = record.paired_at + ACCESS_TOKEN_LIFETIME;
        assert_eq!(
            devices
                .accept(&record.id, Credential::Access(access.expose()), at)
                .unwrap_err(),
            AuthenticateError::AccessExpired
        );
        // Any other access token asks the device to refresh, and revokes
        // nothing.
        assert_eq!(
            devices
                .accept(&record.id, Credential::Access("not-a-token"), at)
                .unwrap_err(),
            AuthenticateError::AccessExpired
        );
        assert_eq!(devices.list().len(), 1);
    }

    #[test]
    fn a_refresh_token_rotates_and_the_acknowledgement_retires_the_old_one() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(16);
        let rotated = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap();
        let next = rotated.issued.expect("a refresh token rotates");
        let rotation_id = next.rotation_id.expect("a rotation has an id");
        assert_eq!(next.access_expires_at, at + ACCESS_TOKEN_LIFETIME);
        // The acknowledgement retires the token before the new one; from then
        // on presenting it is a replay.
        assert_eq!(
            devices.acknowledge(&record.id, &rotation_id),
            Acknowledgement::Retired
        );
        assert_eq!(
            devices.acknowledge(&record.id, &rotation_id),
            Acknowledgement::Ignored
        );
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap_err(),
            AuthenticateError::Replayed
        );
        assert!(devices.list().is_empty(), "the whole family is gone");
    }

    #[test]
    fn a_dropped_authenticated_frame_never_locks_a_device_out() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(20);
        // The answer was dropped, so the device still holds R1.
        let first = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        let retry = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        // The retry hands back the very same answer: nothing rotated away.
        assert_eq!(first.refresh_token.expose(), retry.refresh_token.expose());
        assert_eq!(first.access_token.expose(), retry.access_token.expose());
        assert_eq!(first.rotation_id, retry.rotation_id);
        assert_eq!(devices.list().len(), 1, "no revocation on a retry");
        // The stored answer keeps working, and using it retires R1 for good.
        assert!(
            devices
                .accept(
                    &record.id,
                    Credential::Refresh(first.refresh_token.expose()),
                    at
                )
                .is_ok()
        );
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap_err(),
            AuthenticateError::Replayed
        );
    }

    #[test]
    fn a_retry_after_a_restart_is_a_replay_not_a_fresh_answer() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let enrolled = devices
            .enroll_rotating("desktop", None, &DeviceProfile::default())
            .unwrap();
        let refresh = enrolled
            .issued
            .as_ref()
            .unwrap()
            .refresh_token
            .expose()
            .to_owned();
        let at = enrolled.record.paired_at + chrono::Duration::minutes(20);
        devices
            .accept(&enrolled.record.id, Credential::Refresh(&refresh), at)
            .unwrap();
        store.save(&devices).unwrap();

        // The restart forgot the answer it handed out, so presenting the
        // previous token again is a copy of it, not a retry.
        let mut reloaded = store.load().unwrap();
        assert_eq!(
            reloaded
                .accept(&enrolled.record.id, Credential::Refresh(&refresh), at)
                .unwrap_err(),
            AuthenticateError::Replayed
        );
        assert!(reloaded.list().is_empty());
    }

    #[test]
    fn using_the_new_token_retires_the_one_before_it() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(20);
        let second = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        // The first use of R2 retires R1, without any acknowledgement.
        assert!(
            devices
                .accept(
                    &record.id,
                    Credential::Refresh(second.refresh_token.expose()),
                    at
                )
                .is_ok()
        );
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap_err(),
            AuthenticateError::Replayed
        );
    }

    #[test]
    fn two_connections_racing_on_one_refresh_token_are_not_a_replay() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(20);
        // Two routes race with the same R1: the second is an idempotent retry,
        // so both are answered with the same tokens and neither is a replay.
        let first = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        let second = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        assert_eq!(first.refresh_token.expose(), second.refresh_token.expose());
        assert_eq!(devices.list().len(), 1);
        // The answer both hold is the current token, and it works.
        assert!(
            devices
                .accept(
                    &record.id,
                    Credential::Refresh(second.refresh_token.expose()),
                    at
                )
                .is_ok()
        );
    }

    #[test]
    fn a_thief_with_the_previous_token_cannot_supersede_the_answer() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(20);
        // The real device rotates R1 into R2.
        let real = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        // A thief presenting the same R1 gets R2 again, not a new token.
        let thief = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        assert_eq!(real.refresh_token.expose(), thief.refresh_token.expose());
        assert_eq!(real.rotation_id, thief.rotation_id);
        // The real device acknowledges R2: it is the current token, and R1 is
        // retired for good.
        assert_eq!(
            devices.acknowledge(&record.id, real.rotation_id.as_deref().unwrap()),
            Acknowledgement::Retired
        );
        assert_eq!(devices.list().len(), 1);
        assert!(
            devices
                .accept(
                    &record.id,
                    Credential::Refresh(real.refresh_token.expose()),
                    at
                )
                .is_ok()
        );
        // The real device was never locked out; a copy of R1 is a replay.
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap_err(),
            AuthenticateError::Replayed
        );
    }

    #[test]
    fn an_access_token_a_rotation_replaced_asks_for_a_refresh() {
        let (mut devices, record, refresh, access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(5);
        let rotated = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .unwrap();
        // Both the replaced token and one this host never issued send the
        // device to its refresh token instead of pairing again.
        for stale in [access.expose(), "not-an-access-token"] {
            assert_eq!(
                devices
                    .accept(&record.id, Credential::Access(stale), at)
                    .unwrap_err(),
                AuthenticateError::AccessExpired,
                "{stale}"
            );
        }
        // The live one reconnects, and the refresh token is untouched.
        assert!(
            devices
                .accept(
                    &record.id,
                    Credential::Access(rotated.access_token.expose()),
                    at
                )
                .is_ok()
        );
    }

    #[test]
    fn a_wrong_token_is_refused_without_revoking_the_device() {
        let (mut devices, record, refresh, _access) = rotating();
        let at = record.paired_at + chrono::Duration::minutes(20);
        for wrong in ["", "not-a-token", &"a".repeat(43)] {
            assert_eq!(
                devices
                    .accept(&record.id, Credential::Refresh(wrong), at)
                    .unwrap_err(),
                AuthenticateError::Unauthorized,
                "{wrong}"
            );
        }
        // A guessed device identifier cannot revoke a device either.
        assert_eq!(
            devices
                .accept(
                    "ffffffffffffffff",
                    Credential::Refresh(refresh.expose()),
                    at
                )
                .unwrap_err(),
            AuthenticateError::Unauthorized
        );
        assert_eq!(devices.list().len(), 1);
        assert!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .is_ok()
        );
    }

    #[test]
    fn only_digests_are_stored_and_the_access_token_never_reaches_disk() {
        let (_dir, store) = store();
        let mut devices = store.load().unwrap();
        let enrolled = devices
            .enroll_rotating("desktop", None, &DeviceProfile::default())
            .unwrap();
        let issued = enrolled.issued.expect("a rotating pairing issues tokens");
        store.save(&devices).unwrap();
        let on_disk = std::fs::read_to_string(store.path()).unwrap();
        assert!(!on_disk.contains(issued.refresh_token.expose()));
        assert!(!on_disk.contains(issued.access_token.expose()));
        assert!(!on_disk.contains("access"));
        assert!(!on_disk.contains("token_sha256"));

        // A restart keeps the rotated state and forgets the access token.
        let mut loaded = store.load().unwrap();
        assert!(loaded.list()[0].rotates());
        assert!(loaded.list()[0].rotation.as_ref().unwrap().access.is_none());
        let at = enrolled.record.paired_at + chrono::Duration::minutes(1);
        assert_eq!(
            loaded
                .accept(
                    &enrolled.record.id,
                    Credential::Access(issued.access_token.expose()),
                    at
                )
                .unwrap_err(),
            AuthenticateError::AccessExpired
        );
        let rotated = loaded
            .accept(
                &enrolled.record.id,
                Credential::Refresh(issued.refresh_token.expose()),
                at,
            )
            .unwrap()
            .issued
            .expect("a refresh token rotates");
        store.save(&loaded).unwrap();

        // Reloaded, the token it stored is current and the one before it is
        // still the previous token, so the pairing survived the restart.
        let mut reloaded = store.load().unwrap();
        assert!(
            reloaded
                .accept(
                    &enrolled.record.id,
                    Credential::Refresh(rotated.refresh_token.expose()),
                    at
                )
                .is_ok()
        );
        // The answer that replaced R1 only lived in memory, so after the
        // restart a copy of R1 is a replay rather than a retry.
        let mut reloaded = store.load().unwrap();
        assert_eq!(
            reloaded
                .accept(
                    &enrolled.record.id,
                    Credential::Refresh(issued.refresh_token.expose()),
                    at
                )
                .unwrap_err(),
            AuthenticateError::Replayed
        );
    }

    #[test]
    fn a_static_device_is_unaffected_by_the_rotating_path() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        assert!(!record.rotates());
        assert_eq!(
            record.expires_at(),
            Some(record.paired_at + DEVICE_LIFETIME)
        );
        let at = record.paired_at + chrono::Duration::days(29);
        assert!(
            devices
                .accept(&record.id, Credential::Refresh(token.expose()), at)
                .is_ok()
        );
        assert_eq!(
            devices
                .accept(&record.id, Credential::Access(token.expose()), at)
                .unwrap_err(),
            AuthenticateError::Unauthorized
        );
        let after = record.paired_at + DEVICE_LIFETIME;
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(token.expose()), after)
                .unwrap_err(),
            AuthenticateError::Unauthorized
        );
        assert_eq!(devices.list().len(), 1);
    }

    #[test]
    fn a_record_stored_without_rotation_fields_loads_as_a_static_device() {
        let stored = r#"{"version":1,"devices":[{"id":"0011223344556677","name":"iPhone","token_sha256":"00","paired_at":"2026-10-01T10:00:00Z"}]}"#;
        let old: Devices = serde_json::from_str(stored).unwrap();
        assert!(!old.list()[0].rotates());
        assert!(old.list()[0].expires_at().is_some());
    }

    #[test]
    fn a_family_that_retires_too_many_tokens_is_revoked_not_forgotten() {
        let (mut devices, record, mut refresh, _access) = rotating();
        let mut at = record.paired_at;
        for _ in 0..MAX_RETIRED_TOKENS {
            at += chrono::Duration::minutes(1);
            let rotated = devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap()
                .issued
                .expect("a refresh token rotates");
            let rotation_id = rotated.rotation_id.expect("a rotation has an id");
            assert_eq!(
                devices.acknowledge(&record.id, &rotation_id),
                Acknowledgement::Retired
            );
            refresh = rotated.refresh_token;
        }
        let family = devices.list()[0].rotation.as_ref().unwrap();
        assert_eq!(family.retired.len(), MAX_RETIRED_TOKENS);
        // One more rotation would have to forget the oldest fingerprint, which
        // would answer an issued token as a wrong one: revoke instead.
        at += chrono::Duration::minutes(1);
        assert_eq!(
            devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap_err(),
            AuthenticateError::Exhausted
        );
        assert!(devices.list().is_empty());
    }

    #[test]
    fn an_acknowledgement_that_would_overfill_the_history_revokes_the_family() {
        let (mut devices, record, mut refresh, _access) = rotating();
        let mut at = record.paired_at;
        for _ in 0..MAX_RETIRED_TOKENS - 1 {
            at += chrono::Duration::minutes(1);
            let rotated = devices
                .accept(&record.id, Credential::Refresh(refresh.expose()), at)
                .unwrap()
                .issued
                .expect("a refresh token rotates");
            let rotation_id = rotated.rotation_id.expect("a rotation has an id");
            assert_eq!(
                devices.acknowledge(&record.id, &rotation_id),
                Acknowledgement::Retired
            );
            refresh = rotated.refresh_token;
        }
        // One rotation leaves the history full with a token still pending, so
        // the next rotation retires the one before it and fills the history.
        let first = devices
            .accept(&record.id, Credential::Refresh(refresh.expose()), at)
            .unwrap()
            .issued
            .expect("a refresh token rotates");
        let second = devices
            .accept(
                &record.id,
                Credential::Refresh(first.refresh_token.expose()),
                at,
            )
            .unwrap()
            .issued
            .expect("a refresh token rotates");
        assert_eq!(
            devices.acknowledge(
                &record.id,
                second.rotation_id.as_deref().expect("a rotation has an id")
            ),
            Acknowledgement::Exhausted
        );
        assert!(devices.list().is_empty());
    }
}

#[cfg(test)]
mod revocation_note_tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, DeviceStore, RevocationNotes) {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let remote = dir.path().join("remote");
        (
            dir,
            DeviceStore::new(remote.join("devices.json")),
            RevocationNotes::new(remote.join("revocations.json")),
        )
    }

    #[test]
    fn notes_are_written_read_back_and_cleared() {
        let (_dir, store, notes) = paths();
        assert!(notes.load().unwrap().is_empty());
        notes.add("0011223344556677").unwrap();
        notes.add("0011223344556677").unwrap();
        notes.add("8899aabbccddeeff").unwrap();
        assert_eq!(
            notes.load().unwrap(),
            ["0011223344556677", "8899aabbccddeeff"]
        );
        notes.clear().unwrap();
        assert!(notes.load().unwrap().is_empty());
        // Clearing a note that is already gone is fine.
        notes.clear().unwrap();
        // A note never carries a token or a name.
        notes.add("0011223344556677").unwrap();
        let on_disk = std::fs::read_to_string(notes.path()).unwrap();
        assert!(!on_disk.contains("token"));
        let loaded = store.load().unwrap();
        assert!(loaded.list().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_note_is_owner_only_from_the_first_write() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, _store, notes) = paths();
        notes.add("0011223344556677").unwrap();
        let mode = std::fs::metadata(notes.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let directory = std::fs::metadata(notes.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory, 0o700);
    }

    #[test]
    fn notes_apply_before_serving_and_survive_a_store_that_cannot_be_written() {
        let (dir, store, notes) = paths();
        let mut devices = store.load().unwrap();
        let enrolled = devices
            .enroll_rotating("desktop", None, &DeviceProfile::default())
            .unwrap();
        let device_id = enrolled.record.id.clone();
        store.save(&devices).unwrap();
        notes.add(&device_id).unwrap();

        // The store is there and the note is there: the family is withdrawn
        // before anything is served, and the store is rewritten.
        let applied = apply_revocation_notes(&store, &notes, store.load().unwrap());
        assert!(applied.list().is_empty());
        assert!(store.load().unwrap().list().is_empty());
        assert!(notes.load().unwrap().is_empty());

        // Now with a directory that cannot be written: the family is still
        // refused in memory, and the note stays for the next start.
        let mut devices = store.load().unwrap();
        let second = devices
            .enroll_rotating("desktop", None, &DeviceProfile::default())
            .unwrap();
        let second_id = second.record.id.clone();
        store.save(&devices).unwrap();
        notes.add(&second_id).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let remote = store.path().parent().unwrap().to_path_buf();
            std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o500)).unwrap();
            let applied = apply_revocation_notes(&store, &notes, store.load().unwrap());
            assert!(applied.list().is_empty(), "refused in memory");
            assert!(
                !store.load().unwrap().list().is_empty(),
                "the store could not be written"
            );
            assert_eq!(notes.load().unwrap(), [second_id.clone()]);
            std::fs::set_permissions(&remote, std::fs::Permissions::from_mode(0o700)).unwrap();
            // Once the directory is writable again, the next start finishes.
            let applied = apply_revocation_notes(&store, &notes, store.load().unwrap());
            assert!(applied.list().is_empty());
            assert!(store.load().unwrap().list().is_empty());
            assert!(notes.load().unwrap().is_empty());
        }
        let _ = dir;
    }
}
