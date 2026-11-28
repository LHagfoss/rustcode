//! What a relay keeps between runs: its identifier, the secret that admits
//! members and the key each machine id first registered with.

use crate::daemon::private_files::{publish_private_json, read_private_json};
use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `relay/relay.json` in the configuration directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayState {
    pub relay_id: String,
    pub secret: String,
    /// Machine id to the public key it registered with first. A later
    /// registration under the same id with another key is refused, so a
    /// member that learnt the secret cannot take over another machine's id.
    #[serde(default)]
    pub members: BTreeMap<String, String>,
}

/// The largest `relay.json` read back.
const MAX_STATE_BYTES: u64 = 1024 * 1024;

impl RelayState {
    pub fn new() -> Self {
        Self {
            relay_id: random_token(16),
            secret: random_token(32),
            members: BTreeMap::new(),
        }
    }

    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("relay").join("relay.json")
    }

    /// The state at `path`, made and saved when there is none. A file that
    /// exists but cannot be read stops the relay rather than replacing the
    /// secret every member holds.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match read_private_json::<Self>(path, MAX_STATE_BYTES) {
            Ok(state) => Ok(state),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                let state = Self::new();
                state.save(path)?;
                Ok(state)
            }
            Err(error) => Err(error).with_context(|| {
                format!(
                    "failed to read the relay state {}; remove it to start over \
                     (every machine must register again)",
                    path.display()
                )
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        publish_private_json(path, self)
    }
}

impl Default for RelayState {
    fn default() -> Self {
        Self::new()
    }
}

/// `bytes` random bytes as unpadded base64url.
pub fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rng().fill(&mut buffer[..]);
    URL_SAFE_NO_PAD.encode(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = RelayState::path(dir.path());
        let first = RelayState::load_or_create(&path).unwrap();
        let again = RelayState::load_or_create(&path).unwrap();
        assert_eq!(first.relay_id, again.relay_id);
        assert_eq!(first.secret, again.secret);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn an_unreadable_state_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(RelayState::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
    }
}
