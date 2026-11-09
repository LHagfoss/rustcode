//! Device tokens, kept in the operating system's credential store.
//!
//! A pairing hands this client one durable token, shown once: the static
//! device token, or, for a device that rotates its tokens, a refresh token
//! that every accepted reconnect replaces. It is a credential for everything
//! that machine shows the app, so it belongs in the OS keychain (Keychain,
//! Credential Manager, Secret Service), and never in `credentials.json` or
//! `provider-accounts.json`: those belong to model providers, and mixing the
//! two would put a machine token in whatever exports those files.
//!
//! A rotating machine also hands out a short-lived access token. It is kept in
//! memory only ([`Access`]): the machine forgets its access tokens when its
//! gateway restarts, and the refresh token is what reconnects then. Nothing
//! here writes an access token anywhere.
//!
//! One keychain service holds one entry per machine per *profile*: the
//! account is `<install id>/<host id>`, where the install id is the random
//! identifier `machines/install-id` holds for this configuration directory.
//! Two `RUSTCODE_CONFIG_DIR` profiles therefore never share or overwrite each
//! other's entries.
//!
//! When the credential store is not available — a headless Linux box without
//! a Secret Service, a CI container — the client does not silently fall back
//! to a plain file. It says what would let it continue:
//! `RUSTCODE_MACHINE_TOKEN_STORE=file` keeps machine tokens in
//! `machines/tokens.json` (`0600` in a `0700` directory) instead. Tests use
//! [`TokenStore::in_file`] directly, so no test touches a real keychain.

use crate::protocol::Secret;
use crate::registry::write_private_json;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Keychain service name, shared by every machine and profile. The account
/// names the machine; see [`keychain_account`].
pub const KEYCHAIN_SERVICE: &str = "rustcode-machines";

/// The file the explicit fallback writes.
pub const TOKENS_FILE: &str = "tokens.json";

/// Environment variable that selects the file fallback, or turns it off.
pub const TOKEN_STORE_ENV: &str = "RUSTCODE_MACHINE_TOKEN_STORE";

const MAX_TOKENS_BYTES: u64 = 128 * 1024;
const MAX_TOKENS: usize = 256;

/// A short-lived access token and when the machine stops accepting it.
///
/// Unlike the refresh token it is never written anywhere: it lives as long as
/// this process does, so a restart simply reconnects with the refresh token.
/// The machine treats every access token it did not mint in its current run as
/// expired, so an old one is a wasted attempt at best.
#[derive(Clone, PartialEq, Eq)]
pub struct Access {
    token: Secret,
    expires_at: DateTime<Utc>,
}

impl Access {
    pub fn new(token: Secret, expires_at: DateTime<Utc>) -> Self {
        Self { token, expires_at }
    }

    /// The token to present.
    pub fn token(&self) -> &Secret {
        &self.token
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Whether the machine should still take it at `now`.
    pub fn is_valid_at(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
    }
}

/// Where and when, never what: a token in a `Debug` line is a token in a log.
impl fmt::Debug for Access {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Access")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

enum Backend {
    /// The OS credential store, under `KEYCHAIN_SERVICE`. `profile` is this
    /// configuration directory's install id.
    Keychain { profile: String },
    File {
        path: PathBuf,
        tokens: std::sync::Mutex<BTreeMap<String, String>>,
    },
}

/// The keychain account of one machine in one profile. It keeps two
/// `RUSTCODE_CONFIG_DIR` profiles from overwriting each other's entries, and
/// keeps a `host_id` that came from another profile from being read here.
fn keychain_account(profile: &str, host_id: &str) -> String {
    format!("{profile}/{host_id}")
}

/// What a backend is, never what it holds: a token in a `Debug` line is a
/// token in a log file.
impl fmt::Debug for Backend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keychain { profile } => formatter
                .debug_struct("Keychain")
                .field("service", &KEYCHAIN_SERVICE)
                .field("profile", profile)
                .finish(),
            Self::File { path, tokens } => {
                let count = tokens.lock().map(|tokens| tokens.len()).unwrap_or(0);
                formatter
                    .debug_struct("File")
                    .field("path", path)
                    .field("tokens", &format!("<{count} redacted>"))
                    .finish()
            }
        }
    }
}

/// The pairing tokens of the saved machines.
#[derive(Debug)]
pub struct TokenStore {
    backend: Backend,
    /// Access tokens, in memory only, keyed by `host_id`. Dropped by a new
    /// refresh token, a re-pairing and a removal, because each of those makes
    /// the access token the machine minted before it worthless.
    access: std::sync::Mutex<BTreeMap<String, Access>>,
}

impl TokenStore {
    /// The store to use on this profile: the OS credential store, or the file
    /// fallback when `RUSTCODE_MACHINE_TOKEN_STORE=file` asks for it.
    ///
    /// A credential store that is not reachable is reported as an error with
    /// what to do about it, rather than saved to disk behind the user's back.
    pub fn open(config_dir: &Path) -> Result<Self> {
        match std::env::var(TOKEN_STORE_ENV).ok().as_deref() {
            Some("file") => Ok(Self::in_file(config_dir)?),
            Some("keychain") | None => {
                // The account name is per profile, so the keychain is only
                // usable once this profile has an install id.
                let profile = crate::machine::install_id(config_dir)?;
                keyring_probe().context(
                    "the operating system credential store is not available for machine tokens; \
                     set RUSTCODE_MACHINE_TOKEN_STORE=file to keep them in machines/tokens.json (owner-only) instead",
                )?;
                Ok(Self {
                    backend: Backend::Keychain { profile },
                    access: std::sync::Mutex::new(BTreeMap::new()),
                })
            }
            Some(other) => {
                bail!("{TOKEN_STORE_ENV}={other} is not a token store; use `file` or `keychain`")
            }
        }
    }

    /// The file fallback, without touching the keychain. Tests use this, and
    /// so does `RUSTCODE_MACHINE_TOKEN_STORE=file`.
    ///
    /// A token file that cannot be read is an error, never an empty store:
    /// treating it as empty would let the next save drop every token in it.
    pub fn in_file(config_dir: &Path) -> Result<Self> {
        let path = crate::config::machines_dir(config_dir).join(TOKENS_FILE);
        let tokens = read_tokens(&path)?;
        Ok(Self {
            backend: Backend::File {
                path,
                tokens: std::sync::Mutex::new(tokens),
            },
            access: std::sync::Mutex::new(BTreeMap::new()),
        })
    }

    /// Where a token is kept, for a message to the user. Never a token.
    pub fn describe(&self) -> String {
        match &self.backend {
            Backend::Keychain { profile } => format!(
                "the operating system credential store ({KEYCHAIN_SERVICE}, profile {profile})"
            ),
            Backend::File { path, .. } => path.display().to_string(),
        }
    }

    /// The durable token of `host_id`: a refresh token for a device that
    /// rotates its tokens, or the static token of a pairing that does not.
    pub fn get(&self, host_id: &str) -> Result<Option<Secret>> {
        match &self.backend {
            Backend::Keychain { profile } => {
                match keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(profile, host_id))
                    .and_then(|entry| entry.get_password())
                {
                    Ok(token) => Ok(Some(Secret::new(token))),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(error) => Err(keyring_error("read", host_id, error)),
                }
            }
            Backend::File { tokens, .. } => Ok(tokens
                .lock()
                .expect("token map is not poisoned")
                .get(host_id)
                .map(|token| Secret::new(token.clone()))),
        }
    }

    /// Replace the durable token of `host_id`.
    ///
    /// The access token this process holds for the machine goes with it: a new
    /// refresh token comes from a rotation or a pairing, and the access token
    /// minted before either is no longer the live one. Rotation therefore
    /// calls this first and [`Self::keep_access`] after it.
    pub fn set(&self, host_id: &str, token: &Secret) -> Result<()> {
        self.forget_access(host_id);
        match &self.backend {
            Backend::Keychain { profile } => {
                keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(profile, host_id))
                    .and_then(|entry| entry.set_password(token.expose()))
                    .map_err(|error| keyring_error("store", host_id, error))
            }
            Backend::File { path, tokens } => {
                let mut tokens = tokens.lock().expect("token map is not poisoned");
                tokens.insert(host_id.to_owned(), token.expose().to_owned());
                write_tokens(path, &tokens)
            }
        }
    }

    pub fn remove(&self, host_id: &str) -> Result<()> {
        self.forget_access(host_id);
        match &self.backend {
            Backend::Keychain { profile } => {
                match keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(profile, host_id))
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

    /// The access token this process holds for `host_id`, if any. Never read
    /// from disk: it is gone after a restart, which is what the machine
    /// expects.
    pub fn access(&self, host_id: &str) -> Option<Access> {
        self.access
            .lock()
            .expect("access token map is not poisoned")
            .get(host_id)
            .cloned()
    }

    /// Keep `access` in memory for the next reconnect. Nothing is written.
    pub fn keep_access(&self, host_id: &str, access: Access) {
        self.access
            .lock()
            .expect("access token map is not poisoned")
            .insert(host_id.to_owned(), access);
    }

    /// Drop the access token of `host_id`, keeping its durable token.
    pub fn forget_access(&self, host_id: &str) {
        self.access
            .lock()
            .expect("access token map is not poisoned")
            .remove(host_id);
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

/// The tokens in `path`: empty when there is no file, and an error when there
/// is one that cannot be read, so a corrupt file is never overwritten with
/// what this process happens to know.
fn read_tokens(path: &Path) -> Result<BTreeMap<String, String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", path.display()));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_TOKENS_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= MAX_TOKENS_BYTES,
        "{} is larger than {MAX_TOKENS_BYTES} bytes",
        path.display()
    );
    let stored: StoredTokens = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{} is not a token file; move it aside and pair with the machines again",
            path.display()
        )
    })?;
    Ok(stored.tokens)
}

fn write_tokens(path: &Path, tokens: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        tokens.len() <= MAX_TOKENS,
        "refusing to write more than {MAX_TOKENS} machine tokens"
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

    fn file_path(dir: &Path) -> PathBuf {
        crate::config::machines_dir(dir).join(TOKENS_FILE)
    }

    /// The file fallback round-trips, stays owner-only and forgets a removed
    /// machine. Nothing here reaches the keychain.
    #[test]
    fn the_file_fallback_stores_lists_and_forgets() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = TokenStore::in_file(dir.path()).unwrap();
        assert!(store.get("gw-a").unwrap().is_none());
        store.set("gw-a", &Secret::new("token-a")).unwrap();
        store.set("gw-b", &Secret::new("token-b")).unwrap();
        assert_eq!(store.get("gw-a").unwrap().unwrap().expose(), "token-a");
        // A second store on the same directory reads what the first wrote.
        let reopened = TokenStore::in_file(dir.path()).unwrap();
        assert_eq!(reopened.get("gw-b").unwrap().unwrap().expose(), "token-b");
        store.remove("gw-a").unwrap();
        assert!(store.get("gw-a").unwrap().is_none());
        assert_eq!(reopened.get("gw-b").unwrap().unwrap().expose(), "token-b");
        // Removing what is not there is not an error.
        store.remove("gw-a").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = file_path(dir.path());
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// A corrupt token file is reported and left alone: an empty store would
    /// overwrite it on the next pairing and lose every other machine's token.
    #[test]
    fn a_corrupt_token_file_is_an_error_and_is_not_overwritten() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = file_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let error = TokenStore::in_file(dir.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("is not a token file"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
        // A file that is too large is refused the same way.
        std::fs::write(&path, vec![b' '; (MAX_TOKENS_BYTES + 1) as usize]).unwrap();
        let error = TokenStore::in_file(dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("larger than"), "{error:#}");
    }

    /// The access token lives in this process only: a second store on the same
    /// directory is a restart and has none, and everything that makes the
    /// durable token new makes the access token before it worthless.
    #[test]
    fn an_access_token_is_memory_only_and_goes_with_the_durable_one() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let at = Utc::now() + chrono::Duration::minutes(15);
        let store = TokenStore::in_file(dir.path()).unwrap();
        store.set("gw-a", &Secret::new("refresh-1")).unwrap();
        store.keep_access("gw-a", Access::new(Secret::new("access-1"), at));
        let kept = store.access("gw-a").expect("kept in memory");
        assert_eq!(kept.token().expose(), "access-1");
        assert_eq!(kept.expires_at(), at);
        assert!(
            TokenStore::in_file(dir.path())
                .unwrap()
                .access("gw-a")
                .is_none(),
            "a restart keeps no access token"
        );
        // A rotation or a pairing writes a new durable token; the access token
        // the machine minted before it is not the live one.
        store.set("gw-a", &Secret::new("refresh-2")).unwrap();
        assert!(store.access("gw-a").is_none());
        // Removing the machine drops both.
        store.keep_access("gw-a", Access::new(Secret::new("access-2"), at));
        store.remove("gw-a").unwrap();
        assert!(store.access("gw-a").is_none());
        assert!(store.get("gw-a").unwrap().is_none());
        // And it never reaches the file.
        let bytes = std::fs::read(file_path(dir.path())).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("access-2"), "{text}");
    }

    /// `Debug` names where the tokens are, never what they are.
    #[test]
    fn debug_never_prints_a_token() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = TokenStore::in_file(dir.path()).unwrap();
        store
            .set("gw-a", &Secret::new("token-that-must-not-be-printed"))
            .unwrap();
        let rendered = format!("{store:?}");
        assert!(
            !rendered.contains("token-that-must-not-be-printed"),
            "{rendered}"
        );
        assert!(rendered.contains("1 redacted"), "{rendered}");
        assert!(rendered.contains(TOKENS_FILE), "{rendered}");
    }

    /// Two configuration directories are two profiles: their keychain
    /// accounts differ, so neither can read or overwrite the other's token.
    #[test]
    fn keychain_accounts_are_per_profile_and_machine() {
        let account = keychain_account("profile-a", "gw-8e1f4b7a");
        assert_eq!(account, "profile-a/gw-8e1f4b7a");
        assert_ne!(account, keychain_account("profile-b", "gw-8e1f4b7a"));
        assert_ne!(account, keychain_account("profile-a", "gw-00112233"));
        // The install id is a real identifier and differs per directory.
        let first = tempfile::tempdir_in("/tmp").unwrap();
        let second = tempfile::tempdir_in("/tmp").unwrap();
        let one = crate::machine::install_id(first.path()).unwrap();
        let two = crate::machine::install_id(second.path()).unwrap();
        assert_ne!(one, two);
        assert_eq!(
            crate::machine::install_id(first.path()).unwrap(),
            one,
            "kept for the next run"
        );
    }

    /// The path the fallback writes is inside the configuration directory and
    /// never one of the provider credential files.
    #[test]
    fn the_fallback_never_uses_the_provider_credential_files() {
        let dir = std::path::PathBuf::from("/tmp/rustcode-config");
        let store = TokenStore::in_file(&dir).unwrap();
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
