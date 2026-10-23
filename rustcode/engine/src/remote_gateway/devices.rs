//! Paired devices and their revocable tokens.
//!
//! A device token is 256 random bits handed to the device once. The host
//! keeps only its SHA-256 digest, in an owner-only file replaced atomically.
//! The token is high-entropy, so a fast digest is enough: there is nothing to
//! brute-force that a slow hash would protect.

use super::handshake::Secret;
use super::pairing::{constant_time_eq, digest};
use crate::daemon::lifecycle::{ensure_private_directory, publish_private_json, read_private_json};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::os::unix::fs::PermissionsExt;
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

/// How long a pairing lasts. After it the token is refused like an unknown
/// one and the device pairs again, which bounds what a copied token is worth.
pub const DEVICE_LIFETIME: chrono::Duration = chrono::Duration::days(30);

const STORE_VERSION: u32 = 1;
const MAX_STORE_BYTES: u64 = 256 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// Random identifier assigned by the gateway. The only identity.
    pub id: String,
    /// Label supplied by the device. Not unique, not trusted.
    pub name: String,
    /// Hex SHA-256 of the device token. The token itself is never stored.
    token_sha256: String,
    pub paired_at: DateTime<Utc>,
    /// Hex SHA-256 of the app install's identifier, when the device sent one.
    /// Pairing again from the same install replaces this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_sha256: Option<String>,
    /// When the device last paired or authenticated, to within
    /// [`LAST_SEEN_RESOLUTION`]. `None` for records stored before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl DeviceRecord {
    /// When the token stops being accepted: [`DEVICE_LIFETIME`] after pairing.
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.paired_at + DEVICE_LIFETIME
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at()
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
    /// The only copy of the new token.
    pub token: Secret,
    /// Identifier of the earlier record from the same install, now gone.
    pub replaced: Option<String>,
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
        self.enroll_install(name, None)
            .map(|enrolled| (enrolled.record, enrolled.token))
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
        let mut id_bytes = [0u8; 8];
        let mut token_bytes = [0u8; 32];
        rand::rng().fill(&mut id_bytes);
        rand::rng().fill(&mut token_bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes);
        let record = DeviceRecord {
            id: hex::encode(id_bytes),
            name: sanitize_name(name),
            token_sha256: hex::encode(digest(token.as_bytes())),
            paired_at: now,
            install_sha256,
            last_seen_at: Some(now),
        };
        let replaced = replaced.map(|index| self.devices.remove(index).id);
        self.devices.push(record.clone());
        Ok(Enrollment {
            record,
            token: Secret::new(token),
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

    /// The device `token` belongs to, if `device_id` is paired, the token is
    /// its own and the pairing has not expired at `now`. An unknown device
    /// costs the same comparison as a known one.
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
            hex::decode_to_slice(&record.token_sha256, &mut expected).is_ok()
        });
        let matches = constant_time_eq(&presented, &expected);
        record.filter(|record| known && matches && !record.is_expired(now))
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

    pub fn remove(&mut self, device_id: &str) -> Option<DeviceRecord> {
        let index = self.devices.iter().position(|r| r.id == device_id)?;
        Some(self.devices.remove(index))
    }
}

/// Keep a device-supplied label printable and bounded. It is shown in a
/// terminal, so control characters (including escape sequences) are dropped.
fn sanitize_name(name: &str) -> String {
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

/// Digest of an install identifier the device sent, or `None` when it is out
/// of bounds. Only the digest is stored: the identifier says nothing about
/// the user, but the host has no reason to keep it.
fn install_digest(install_id: &str) -> Option<String> {
    let length = install_id.chars().count();
    let valid = (MIN_INSTALL_ID_CHARS..=MAX_INSTALL_ID_CHARS).contains(&length)
        && install_id.chars().all(|c| c.is_ascii_graphic());
    valid.then(|| hex::encode(digest(install_id.as_bytes())))
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
            metadata.is_file() && metadata.permissions().mode() & 0o077 == 0,
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
                .verify(&first.id, &first.token_sha256, Utc::now())
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
        devices.devices[0].token_sha256 = "not-hex".into();
        assert!(devices.verify(&record.id, "anything", Utc::now()).is_none());
    }

    #[test]
    fn a_pairing_expires_after_its_lifetime_and_frees_its_place() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        assert_eq!(record.expires_at(), record.paired_at + DEVICE_LIFETIME);
        let just_before = record.expires_at() - chrono::Duration::seconds(1);
        assert!(
            devices
                .verify(&record.id, token.expose(), just_before)
                .is_some()
        );
        assert!(
            devices
                .verify(&record.id, token.expose(), record.expires_at())
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
                .verify(&first.record.id, first.token.expose(), now)
                .is_none()
        );
        assert!(
            loaded
                .verify(&again.record.id, again.token.expose(), now)
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
        assert_eq!(serde_json::to_string(&old).unwrap(), stored);
    }

    #[test]
    fn debug_output_omits_the_token_digest() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        let debug = format!("{devices:?} {record:?}");
        assert!(!debug.contains(&record.token_sha256));
        assert!(!debug.contains(token.expose()));
    }
}
