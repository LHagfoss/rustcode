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
}

// The digest is not a credential, but it has no business in a log either.
impl fmt::Debug for DeviceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
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
    pub fn enroll(&mut self, name: &str) -> Result<(DeviceRecord, Secret), DeviceError> {
        if self.devices.len() >= MAX_DEVICES {
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
            paired_at: Utc::now(),
        };
        self.devices.push(record.clone());
        Ok((record, Secret::new(token)))
    }

    /// The device `token` belongs to, if `device_id` is paired and the token
    /// is its own. An unknown device costs the same comparison as a known one.
    pub fn verify(&self, device_id: &str, token: &str) -> Option<&DeviceRecord> {
        let presented = digest(token.as_bytes());
        let record = self.devices.iter().find(|record| record.id == device_id);
        let mut expected = [0u8; 32];
        let known = record.is_some_and(|record| {
            hex::decode_to_slice(&record.token_sha256, &mut expected).is_ok()
        });
        let matches = constant_time_eq(&presented, &expected);
        record.filter(|_| known && matches)
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
            loaded.verify(&record.id, token.expose()).map(|r| &r.id),
            Some(&record.id)
        );
    }

    #[test]
    fn verification_rejects_wrong_tokens_unknown_devices_and_swapped_tokens() {
        let mut devices = Devices::default();
        let (first, first_token) = devices.enroll("first").unwrap();
        let (second, second_token) = devices.enroll("second").unwrap();
        assert!(devices.verify(&first.id, "wrong").is_none());
        assert!(devices.verify(&first.id, "").is_none());
        assert!(
            devices
                .verify("ffffffffffffffff", first_token.expose())
                .is_none()
        );
        // A valid token is bound to its own device.
        assert!(devices.verify(&first.id, second_token.expose()).is_none());
        assert!(devices.verify(&second.id, first_token.expose()).is_none());
        // The stored digest is not itself a credential.
        assert!(devices.verify(&first.id, &first.token_sha256).is_none());
        assert!(devices.verify(&second.id, second_token.expose()).is_some());
    }

    #[test]
    fn a_corrupt_digest_never_matches_the_zero_digest_fallback() {
        let mut devices = Devices::default();
        let (record, _token) = devices.enroll("phone").unwrap();
        devices.devices[0].token_sha256 = "not-hex".into();
        assert!(devices.verify(&record.id, "anything").is_none());
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
        assert!(loaded.verify(&record.id, token.expose()).is_none());
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
    fn debug_output_omits_the_token_digest() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("phone").unwrap();
        let debug = format!("{devices:?} {record:?}");
        assert!(!debug.contains(&record.token_sha256));
        assert!(!debug.contains(token.expose()));
    }
}
