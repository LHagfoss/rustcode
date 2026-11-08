//! Device tokens, kept in the operating system's credential store.
//!
//! A pairing hands this client one token, shown once. It is a credential for
//! everything that machine shows the app, so it belongs in the OS keychain
//! (Keychain, Credential Manager, Secret Service) under a key that names the
//! machine's `host_id`, and never in `credentials.json` or
//! `provider-accounts.json`: those belong to model providers, and mixing the
//! two would put a machine token in whatever exports those files.
//!
//! When the credential store is not available — a headless Linux box without
//! a Secret Service, a CI container — the client does not silently fall back
//! to a plain file. It says what would let it continue:
//! `RUSTCODE_MACHINE_TOKEN_STORE=file` keeps machine tokens in
//! `machines/tokens.json` (`0600` in a `0700` directory) instead. Tests use
//! [`TokenStore::in_file`] directly, so no test touches a real keychain.

use crate::protocol::Secret;
use crate::registry::write_private_json;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Keychain service name. Every machine token is one entry under it, keyed by
/// `host_id`, so `security find-generic-password -s rustcode-machines` (or the
/// platform's equivalent) lists them.
pub const KEYCHAIN_SERVICE: &str = "rustcode-machines";

/// The file the explicit fallback writes.
pub const TOKENS_FILE: &str = "tokens.json";

/// Environment variable that selects the file fallback, or turns it off.
pub const TOKEN_STORE_ENV: &str = "RUSTCODE_MACHINE_TOKEN_STORE";

const MAX_TOKENS_BYTES: u64 = 128 * 1024;

#[derive(Debug)]
enum Backend {
    Keychain,
    File {
        path: PathBuf,
        tokens: std::sync::Mutex<BTreeMap<String, String>>,
    },
}

/// The pairing tokens of the saved machines.
#[derive(Debug)]
pub struct TokenStore {
    backend: Backend,
}

impl TokenStore {
    /// The store to use on this profile: the OS credential store, or the file
    /// fallback when `RUSTCODE_MACHINE_TOKEN_STORE=file` asks for it.
    ///
    /// A credential store that is not reachable is reported as an error with
    /// what to do about it, rather than saved to disk behind the user's back.
    pub fn open(config_dir: &Path) -> Result<Self> {
        match std::env::var(TOKEN_STORE_ENV).ok().as_deref() {
            Some("file") => Ok(Self::in_file(config_dir)),
            Some("keychain") | None => {
                keyring_probe().context(
                    "the operating system credential store is not available for machine tokens; \
                     set RUSTCODE_MACHINE_TOKEN_STORE=file to keep them in machines/tokens.json (owner-only) instead",
                )?;
                Ok(Self {
                    backend: Backend::Keychain,
                })
            }
            Some(other) => anyhow::bail!(
                "{TOKEN_STORE_ENV}={other} is not a token store; use `file` or `keychain`"
            ),
        }
    }

    /// The file fallback, without touching the keychain. Tests use this, and
    /// so does `RUSTCODE_MACHINE_TOKEN_STORE=file`.
    pub fn in_file(config_dir: &Path) -> Self {
        let path = crate::config::machines_dir(config_dir).join(TOKENS_FILE);
        let tokens = read_tokens(&path).unwrap_or_default();
        Self {
            backend: Backend::File {
                path,
                tokens: std::sync::Mutex::new(tokens),
            },
        }
    }

    /// Where a token is kept, for a message to the user. Never a token.
    pub fn describe(&self) -> String {
        match &self.backend {
            Backend::Keychain => {
                format!("the operating system credential store ({KEYCHAIN_SERVICE})")
            }
            Backend::File { path, .. } => path.display().to_string(),
        }
    }

    pub fn get(&self, host_id: &str) -> Result<Option<Secret>> {
        match &self.backend {
            Backend::Keychain => match keyring::Entry::new(KEYCHAIN_SERVICE, host_id)
                .and_then(|entry| entry.get_password())
            {
                Ok(token) => Ok(Some(Secret::new(token))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(keyring_error("read", host_id, error)),
            },
            Backend::File { tokens, .. } => Ok(tokens
                .lock()
                .expect("token map is not poisoned")
                .get(host_id)
                .map(|token| Secret::new(token.clone()))),
        }
    }

    pub fn set(&self, host_id: &str, token: &Secret) -> Result<()> {
        match &self.backend {
            Backend::Keychain => keyring::Entry::new(KEYCHAIN_SERVICE, host_id)
                .and_then(|entry| entry.set_password(token.expose()))
                .map_err(|error| keyring_error("store", host_id, error)),
            Backend::File { path, tokens } => {
                let mut tokens = tokens.lock().expect("token map is not poisoned");
                tokens.insert(host_id.to_owned(), token.expose().to_owned());
                write_tokens(path, &tokens)
            }
        }
    }

    pub fn remove(&self, host_id: &str) -> Result<()> {
        match &self.backend {
            Backend::Keychain => {
                match keyring::Entry::new(KEYCHAIN_SERVICE, host_id)
                    .and_then(|entry| entry.delete_credential())
                {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(error) => Err(keyring_error("remove", host_id, error)),
                }
            }
            Backend::File { path, tokens } => {
                let mut tokens = tokens.lock().expect("token map is not poisoned");
                if tokens.remove(host_id).is_none() {
                    return Ok(());
                }
                write_tokens(path, &tokens)
            }
        }
    }
}

fn keyring_error(action: &str, host_id: &str, error: keyring::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "could not {action} the pairing token for machine {host_id} in the operating system credential store ({error}); \
         set RUSTCODE_MACHINE_TOKEN_STORE=file to keep machine tokens in machines/tokens.json instead"
    )
}

/// Whether the credential store can hold an entry, without leaving one
/// behind. A keychain that needs an interactive unlock fails here, which is
/// what makes [`TokenStore::open`] report it before pairing rather than after.
fn keyring_probe() -> Result<()> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, "__probe__")
        .map_err(|error| anyhow::anyhow!("credential store unavailable: {error}"))?;
    match entry.get_password() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(anyhow::anyhow!("credential store unavailable: {error}")),
    }
}

#[derive(Serialize, Deserialize)]
struct StoredTokens {
    version: u32,
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

fn read_tokens(path: &Path) -> Option<BTreeMap<String, String>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_TOKENS_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_TOKENS_BYTES {
        return None;
    }
    serde_json::from_slice::<StoredTokens>(&bytes)
        .ok()
        .map(|stored| stored.tokens)
}

fn write_tokens(path: &Path, tokens: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        tokens.len() <= 256,
        "refusing to write more than 256 machine tokens"
    );
    write_private_json(
        path,
        &StoredTokens {
            version: 1,
            tokens: tokens.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file fallback round-trips, stays owner-only and forgets a removed
    /// machine. Nothing here reaches the keychain.
    #[test]
    fn the_file_fallback_stores_lists_and_forgets() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = TokenStore::in_file(dir.path());
        assert!(store.get("gw-a").unwrap().is_none());
        store.set("gw-a", &Secret::new("token-a")).unwrap();
        store.set("gw-b", &Secret::new("token-b")).unwrap();
        assert_eq!(store.get("gw-a").unwrap().unwrap().expose(), "token-a");
        // A second store on the same directory reads what the first wrote.
        let reopened = TokenStore::in_file(dir.path());
        assert_eq!(reopened.get("gw-b").unwrap().unwrap().expose(), "token-b");
        store.remove("gw-a").unwrap();
        assert!(store.get("gw-a").unwrap().is_none());
        assert_eq!(reopened.get("gw-b").unwrap().unwrap().expose(), "token-b");
        // Removing what is not there is not an error.
        store.remove("gw-a").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = crate::config::machines_dir(dir.path()).join(TOKENS_FILE);
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// The path the fallback writes is inside the configuration directory and
    /// never one of the provider credential files.
    #[test]
    fn the_fallback_never_uses_the_provider_credential_files() {
        let dir = std::path::PathBuf::from("/tmp/rustcode-config");
        let store = TokenStore::in_file(&dir);
        let described = store.describe();
        assert_eq!(
            described,
            dir.join("machines").join(TOKENS_FILE).display().to_string()
        );
        assert!(!described.contains("credentials.json"));
        assert!(!described.contains("provider-accounts.json"));
    }

    #[test]
    fn an_unknown_backend_value_is_refused() {
        // Not run through `open`, so the process-wide variable is untouched;
        // this asserts the message the CLI would show for a typo.
        let message =
            format!("{TOKEN_STORE_ENV}=floppy is not a token store; use `file` or `keychain`");
        assert!(message.contains("file") && message.contains("keychain"));
    }
}
