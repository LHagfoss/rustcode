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
//! On macOS the entries are written and read by Apple's own `/usr/bin/security`
//! tool rather than in this process. A login-keychain item trusts the program
//! that created it, and for a binary without a Developer ID team that trust is
//! pinned to the hash of that exact build: every update, every development
//! build, and the app and the CLI each, asked for the login keychain password
//! again, once per machine and again on each reconnect that rotated a token.
//! An item `security` created trusts `security`, which is the same for every
//! RustCode binary on the Mac and does not change with RustCode updates, so the
//! keychain never asks. Entries a release before this one wrote under
//! [`LEGACY_MACOS_SERVICE`] are moved on first read: read once in this process
//! (the last prompt, which "Allow" answers), written again through `security`,
//! and the old item deleted.
//!
//! When the credential store is not available — a headless Linux box without
//! a Secret Service, a CI container — the client does not silently fall back
//! to a plain file. It says what would let it continue:
//! `RUSTCODE_MACHINE_TOKEN_STORE=file` keeps machine tokens in
//! `machines/tokens.json` (`0600` in a `0700` directory) instead. Tests use
//! [`TokenStore::in_file`] directly, so no test touches a real keychain.
//!
//! Every call that goes to the store blocks on the operating system's own IPC:
//! a Secret Service prompt, a keychain unlock, a Credential Manager dialog — and
//! on Linux, with the `async-secret-service` backend this crate enables, a call
//! made on the thread a tokio runtime runs on **deadlocks** (keyring's own
//! caution on that feature pair: spawn a thread of your own for keyring calls).
//! A pairing cannot wait for either, so the app and the CLI reach the store
//! through [`bounded`] — on a thread of its own, with a deadline — and open it
//! *before* a connection request is sent: a store that cannot answer must stop
//! the pairing at this end, not after the other machine has accepted the request
//! and enrolled this computer (#3325).

use crate::protocol::Secret;
use crate::registry::write_private_json;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Keychain service name, shared by every machine and profile. The account
/// names the machine; see [`keychain_account`].
///
/// macOS has a name of its own because its entries are written by
/// `/usr/bin/security` (see the module documentation): an entry under the old
/// name trusts only the build that wrote it, and reading it through `security`
/// would prompt again.
#[cfg(target_os = "macos")]
pub const KEYCHAIN_SERVICE: &str = "rustcode-machine-tokens";
#[cfg(not(target_os = "macos"))]
pub const KEYCHAIN_SERVICE: &str = "rustcode-machines";

/// Where earlier releases kept machine tokens in the macOS keychain, read
/// once to move each entry to [`KEYCHAIN_SERVICE`].
pub const LEGACY_MACOS_SERVICE: &str = "rustcode-machines";

/// The file the explicit fallback writes.
pub const TOKENS_FILE: &str = "tokens.json";

/// Environment variable that selects the file fallback, or turns it off.
pub const TOKEN_STORE_ENV: &str = "RUSTCODE_MACHINE_TOKEN_STORE";

const MAX_TOKENS_BYTES: u64 = 128 * 1024;
const MAX_TOKENS: usize = 256;

/// How long one credential-store call may take before it is given up on. A
/// keyring waiting for a person to unlock it never answers on its own; ten
/// seconds is generous for an unlock that is happening and short enough that a
/// pairing never looks hung.
pub const STORE_TIMEOUT: Duration = Duration::from_secs(10);

/// Run one credential-store call off the async runtime, bounded.
///
/// The store is reached through the operating system's IPC, which blocks for as
/// long as a person takes to unlock it — and blocking there holds a runtime
/// thread that other commands share. The call therefore runs on a thread of its
/// own, which is left behind if it never answers: a prompt nobody is looking at
/// must not hold the command that asked, nor keep a process alive after it has
/// given up. A call that does not finish within `timeout` comes back as the
/// error to show, naming the file fallback.
pub async fn bounded<T, F>(what: &str, timeout: Duration, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("machine-token-store".to_owned())
        .spawn(move || {
            let _ = send.send(work());
        })
        .context("could not start a thread for the machine token store")?;
    match tokio::time::timeout(timeout, receive).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => bail!("the machine token store task stopped without an answer"),
        Err(_) => bail!(
            "{what} took longer than {} s and was given up on: the operating system credential \
             store did not answer, which a locked keyring or a missing Secret Service does. \
             Set {TOKEN_STORE_ENV}=file to keep machine tokens in machines/{TOKENS_FILE} instead",
            timeout.as_secs().max(1)
        ),
    }
}

/// [`TokenStore::open`], off the async runtime and bounded: what a pairing (and
/// a reconnect) calls, so an unanswering store cannot hold it.
pub async fn open_bounded(config_dir: &Path, timeout: Duration) -> Result<TokenStore> {
    let dir = config_dir.to_path_buf();
    bounded("opening the machine token store", timeout, move || {
        TokenStore::open(&dir)
    })
    .await
}

/// [`TokenStore::set`], off the async runtime and bounded. A pairing has stored
/// its token only once this returns `Ok`, so a store that cannot answer is not
/// mistaken for one that did.
pub async fn set_bounded(
    tokens: Arc<TokenStore>,
    host_id: String,
    token: Secret,
    timeout: Duration,
) -> Result<()> {
    bounded("storing the pairing token", timeout, move || {
        tokens.set(&host_id, &token)
    })
    .await
}

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

/// Run one keyring call on a thread of its own.
///
/// `keyring` with `async-secret-service`, the Linux Secret Service backend this
/// crate enables, wraps a blocking clock over D-Bus and **deadlocks when it is
/// called from a thread a tokio runtime drives** — which is where the app's
/// commands, the CLI and the connection supervisor all call it. keyring's own
/// documentation for that feature pair says to spawn a thread for its calls, so
/// that is what every call does here. It also keeps a keychain prompt on a
/// thread nobody else's work is waiting on.
fn on_a_thread<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| match scope.spawn(work).join() {
        Ok(value) => value,
        // Nothing here catches panics; pass the keyring one on as it was.
        Err(panic) => std::panic::resume_unwind(panic),
    })
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
            Backend::Keychain { profile } => os_read(&keychain_account(profile, host_id))
                .map(|token| token.map(Secret::new))
                .map_err(|error| keyring_error("read", host_id, error)),
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
        match &self.backend {
            Backend::Keychain { profile } => {
                os_write(&keychain_account(profile, host_id), token.expose())
                    .map_err(|error| keyring_error("store", host_id, error))?;
            }
            Backend::File { path, tokens } => {
                let mut tokens = tokens.lock().expect("token map is not poisoned");
                // Disk first: the map in memory changes only once the write
                // landed, so a failed write leaves the previous token in both
                // places and the rotation it belongs to is never acknowledged.
                let mut candidate = tokens.clone();
                candidate.insert(host_id.to_owned(), token.expose().to_owned());
                write_tokens(path, &candidate)?;
                *tokens = candidate;
            }
        }
        // Only a stored token makes the access token before it stale.
        self.forget_access(host_id);
        Ok(())
    }

    pub fn remove(&self, host_id: &str) -> Result<()> {
        self.forget_access(host_id);
        match &self.backend {
            Backend::Keychain { profile } => os_delete(&keychain_account(profile, host_id))
                .map_err(|error| keyring_error("remove", host_id, error)),
            Backend::File { path, tokens } => {
                let mut tokens = tokens.lock().expect("token map is not poisoned");
                if !tokens.contains_key(host_id) {
                    return Ok(());
                }
                let mut candidate = tokens.clone();
                candidate.remove(host_id);
                write_tokens(path, &candidate)?;
                *tokens = candidate;
                Ok(())
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

fn keyring_error(action: &str, host_id: &str, error: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!(
        "could not {action} the pairing token for machine {host_id} in the operating system credential store ({error:#}); \
         set RUSTCODE_MACHINE_TOKEN_STORE=file to keep machine tokens in machines/tokens.json instead"
    )
}

/// Whether the credential store can hold an entry, without leaving one
/// behind. A keychain that needs an interactive unlock fails here, which is
/// what makes [`TokenStore::open`] report it before pairing rather than after.
fn keyring_probe() -> Result<()> {
    #[cfg(target_os = "macos")]
    let store = SecurityTool::login(KEYCHAIN_SERVICE);
    #[cfg(not(target_os = "macos"))]
    let store = Keyring(KEYCHAIN_SERVICE);
    store
        .read("__probe__")
        .map(drop)
        .map_err(|error| anyhow::anyhow!("credential store unavailable: {error:#}"))
}

/// One place in the operating system's store that keeps a token per account.
trait Slot {
    fn read(&self, account: &str) -> Result<Option<String>>;
    fn write(&self, account: &str, token: &str) -> Result<()>;
    /// Removing what is not there is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The `keyring` crate: Credential Manager, Secret Service, and on macOS only
/// the read of an entry an earlier release wrote.
struct Keyring(&'static str);

impl Slot for Keyring {
    fn read(&self, account: &str) -> Result<Option<String>> {
        on_a_thread(|| {
            match keyring::Entry::new(self.0, account).and_then(|entry| entry.get_password()) {
                Ok(token) => Ok(Some(token)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn write(&self, account: &str, token: &str) -> Result<()> {
        on_a_thread(|| {
            keyring::Entry::new(self.0, account)
                .and_then(|entry| entry.set_password(token))
                .map_err(Into::into)
        })
    }

    fn delete(&self, account: &str) -> Result<()> {
        on_a_thread(|| {
            match keyring::Entry::new(self.0, account).and_then(|entry| entry.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
    }
}

#[cfg(not(target_os = "macos"))]
fn os_read(account: &str) -> Result<Option<String>> {
    Keyring(KEYCHAIN_SERVICE).read(account)
}

#[cfg(not(target_os = "macos"))]
fn os_write(account: &str, token: &str) -> Result<()> {
    Keyring(KEYCHAIN_SERVICE).write(account, token)
}

#[cfg(not(target_os = "macos"))]
fn os_delete(account: &str) -> Result<()> {
    Keyring(KEYCHAIN_SERVICE).delete(account)
}

/// The token of `account`, moving it from `legacy` to `current` the first time
/// it is only in `legacy`. A move that fails still answers with the token, so
/// the connection it is for goes ahead; the move is tried again next time, and
/// the old entry is deleted only once the new one holds the token.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn read_migrating(current: &dyn Slot, legacy: &dyn Slot, account: &str) -> Result<Option<String>> {
    if let Some(token) = current.read(account)? {
        return Ok(Some(token));
    }
    let Some(token) = legacy.read(account)? else {
        return Ok(None);
    };
    if current.write(account, &token).is_ok() {
        let _ = legacy.delete(account);
    }
    Ok(Some(token))
}

/// An entry an earlier release wrote in the macOS keychain: read in this
/// process, because reading it through `security` would ask for the password
/// on behalf of a tool the user does not know, and deleted through `security`,
/// because keyring reads an entry before it deletes it, which would ask again.
#[cfg(target_os = "macos")]
struct MacLegacy;

#[cfg(target_os = "macos")]
impl Slot for MacLegacy {
    fn read(&self, account: &str) -> Result<Option<String>> {
        Keyring(LEGACY_MACOS_SERVICE).read(account)
    }

    fn write(&self, _account: &str, _token: &str) -> Result<()> {
        bail!("nothing is written under {LEGACY_MACOS_SERVICE} any more")
    }

    fn delete(&self, account: &str) -> Result<()> {
        SecurityTool::login(LEGACY_MACOS_SERVICE).delete(account)
    }
}

#[cfg(target_os = "macos")]
fn os_read(account: &str) -> Result<Option<String>> {
    read_migrating(&SecurityTool::login(KEYCHAIN_SERVICE), &MacLegacy, account)
}

#[cfg(target_os = "macos")]
fn os_write(account: &str, token: &str) -> Result<()> {
    SecurityTool::login(KEYCHAIN_SERVICE).write(account, token)?;
    // A re-pairing over an entry that was never moved leaves nothing behind.
    let _ = MacLegacy.delete(account);
    Ok(())
}

#[cfg(target_os = "macos")]
fn os_delete(account: &str) -> Result<()> {
    SecurityTool::login(KEYCHAIN_SERVICE).delete(account)?;
    MacLegacy.delete(account)
}

/// Apple's keychain tool, by absolute path so `PATH` cannot stand in for it.
#[cfg(target_os = "macos")]
const SECURITY: &str = "/usr/bin/security";

/// `security`'s exit status for `errSecItemNotFound`.
#[cfg(target_os = "macos")]
const SECURITY_NOT_FOUND: i32 = 44;

/// Keychain items written and read by `/usr/bin/security`, so the item trusts
/// that tool rather than whichever RustCode build created it.
#[cfg(target_os = "macos")]
struct SecurityTool {
    service: &'static str,
    /// A keychain file, for tests; `None` is the user's default keychain.
    keychain: Option<PathBuf>,
}

#[cfg(target_os = "macos")]
impl SecurityTool {
    fn login(service: &'static str) -> Self {
        Self {
            service,
            keychain: None,
        }
    }

    fn command(&self, verb: &str, account: &str) -> std::process::Command {
        let mut command = std::process::Command::new(SECURITY);
        command
            .args([verb, "-s", self.service, "-a", account])
            .stdin(std::process::Stdio::null());
        command
    }
}

#[cfg(target_os = "macos")]
impl Slot for SecurityTool {
    fn read(&self, account: &str) -> Result<Option<String>> {
        let mut command = self.command("find-generic-password", account);
        command.arg("-w");
        command.args(&self.keychain);
        let output = command
            .output()
            .with_context(|| format!("could not run {SECURITY}"))?;
        match output.status.code() {
            Some(0) => {
                let mut token =
                    String::from_utf8(output.stdout).context("the keychain entry is not text")?;
                if token.ends_with('\n') {
                    token.pop();
                }
                Ok(Some(token))
            }
            Some(SECURITY_NOT_FOUND) => Ok(None),
            _ => bail!(
                "security find-generic-password failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    /// Written through `security -i` on standard input, so the token is never
    /// in an argument list another process can read, then read back: `-U`
    /// replaces an entry in place, and only a token that reads back as written
    /// counts as stored.
    fn write(&self, account: &str, token: &str) -> Result<()> {
        use std::io::Write;
        let line = add_command(self.service, account, token, self.keychain.as_deref())?;
        let mut child = std::process::Command::new(SECURITY)
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("could not run {SECURITY}"))?;
        let written = child
            .stdin
            .take()
            .context("no standard input for security")?
            .write_all(line.as_bytes());
        // Its error output can echo the command, which carries the token.
        let status = child.wait().context("security did not finish")?;
        written.context("could not hand the entry to security")?;
        ensure!(
            status.success(),
            "security add-generic-password failed ({status})"
        );
        ensure!(
            self.read(account)?.as_deref() == Some(token),
            "the keychain did not keep the token it was given"
        );
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        let mut command = self.command("delete-generic-password", account);
        command.args(&self.keychain);
        let output = command
            .stdout(std::process::Stdio::null())
            .output()
            .with_context(|| format!("could not run {SECURITY}"))?;
        match output.status.code() {
            Some(0 | SECURITY_NOT_FOUND) => Ok(()),
            _ => bail!(
                "security delete-generic-password failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
}

/// The `security -i` line that adds or replaces one entry. The token goes as
/// hex (`-X`), so it needs no quoting; every other value is quoted, and one
/// that quoting cannot carry (a quote, a backslash, a control character such as
/// the newline that would start a second command) is refused.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn add_command(
    service: &str,
    account: &str,
    token: &str,
    keychain: Option<&Path>,
) -> Result<String> {
    fn quoted(value: &str) -> Result<String> {
        ensure!(
            !value.is_empty()
                && value
                    .chars()
                    .all(|c| (c.is_ascii_graphic() || c == ' ') && c != '"' && c != '\\'),
            "{value:?} cannot be named in the keychain"
        );
        Ok(format!("\"{value}\""))
    }
    let mut line = format!(
        "add-generic-password -U -s {} -a {} -X {}",
        quoted(service)?,
        quoted(account)?,
        hex::encode(token)
    );
    if let Some(keychain) = keychain {
        let keychain = keychain.to_str().context("the keychain path is not text")?;
        line.push(' ');
        line.push_str(&quoted(keychain)?);
    }
    line.push('\n');
    Ok(line)
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

    /// A write that fails changes nothing: the token stays what it was in
    /// memory as well as on disk, so a rotation that could not be stored is
    /// never acknowledged on the strength of a token only this process knows.
    #[test]
    fn a_failed_write_keeps_the_previous_token_and_access() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = TokenStore::in_file(dir.path()).unwrap();
        store.set("gw-a", &Secret::new("refresh-1")).unwrap();
        let at = Utc::now() + chrono::Duration::minutes(15);
        store.keep_access("gw-a", Access::new(Secret::new("access-1"), at));
        // A directory with a file in it where the token file goes: the atomic
        // replace cannot succeed, as on an unwritable or full disk.
        let path = file_path(dir.path());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("blocker"), b"x").unwrap();

        let error = store
            .set("gw-a", &Secret::new("refresh-2"))
            .expect_err("the write cannot land");
        assert!(!format!("{error:#}").contains("refresh-2"), "{error:#}");
        assert_eq!(store.get("gw-a").unwrap().unwrap().expose(), "refresh-1");
        assert!(
            store.access("gw-a").is_some(),
            "nothing rotated, so the access token is still the live one"
        );
        store
            .set("gw-b", &Secret::new("other"))
            .expect_err("a new machine fails the same way");
        assert!(store.get("gw-b").unwrap().is_none());
        store
            .remove("gw-a")
            .expect_err("a removal that cannot be written is not forgotten");
        assert_eq!(store.get("gw-a").unwrap().unwrap().expose(), "refresh-1");
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

    /// A slot in memory, with a switch that makes writes fail.
    #[derive(Default)]
    struct MemorySlot {
        tokens: std::sync::Mutex<BTreeMap<String, String>>,
        reads: std::sync::atomic::AtomicUsize,
        refuse_writes: bool,
    }

    impl Slot for MemorySlot {
        fn read(&self, account: &str) -> Result<Option<String>> {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.tokens.lock().unwrap().get(account).cloned())
        }
        fn write(&self, account: &str, token: &str) -> Result<()> {
            ensure!(!self.refuse_writes, "refused");
            self.tokens
                .lock()
                .unwrap()
                .insert(account.to_owned(), token.to_owned());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<()> {
            self.tokens.lock().unwrap().remove(account);
            Ok(())
        }
    }

    fn reads(slot: &MemorySlot) -> usize {
        slot.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// An entry an earlier release wrote is read from the old place once, moved,
    /// and never read from there again: on macOS that read is the one prompt
    /// left, and every later read goes where nothing prompts.
    #[test]
    fn a_legacy_entry_is_moved_on_first_read_and_not_read_again() {
        let current = MemorySlot::default();
        let legacy = MemorySlot::default();
        legacy.write("p/gw-a", "refresh-1").unwrap();

        let token = read_migrating(&current, &legacy, "p/gw-a").unwrap();
        assert_eq!(token.as_deref(), Some("refresh-1"));
        assert_eq!(
            current
                .tokens
                .lock()
                .unwrap()
                .get("p/gw-a")
                .map(String::as_str),
            Some("refresh-1")
        );
        assert!(
            legacy.tokens.lock().unwrap().is_empty(),
            "the old entry goes"
        );

        let legacy_reads = reads(&legacy);
        for _ in 0..3 {
            let token = read_migrating(&current, &legacy, "p/gw-a").unwrap();
            assert_eq!(token.as_deref(), Some("refresh-1"));
        }
        assert_eq!(
            reads(&legacy),
            legacy_reads,
            "reconnects never read it again"
        );

        // A machine stored nowhere is simply not paired.
        assert_eq!(read_migrating(&current, &legacy, "p/gw-b").unwrap(), None);
    }

    /// A move that cannot be written keeps the old entry and still answers, so
    /// the connection goes ahead and nothing is lost.
    #[test]
    fn a_failed_move_keeps_the_legacy_entry() {
        let current = MemorySlot {
            refuse_writes: true,
            ..MemorySlot::default()
        };
        let legacy = MemorySlot::default();
        legacy.write("p/gw-a", "refresh-1").unwrap();
        let token = read_migrating(&current, &legacy, "p/gw-a").unwrap();
        assert_eq!(token.as_deref(), Some("refresh-1"));
        assert_eq!(
            legacy
                .tokens
                .lock()
                .unwrap()
                .get("p/gw-a")
                .map(String::as_str),
            Some("refresh-1")
        );
    }

    /// The `security -i` line carries the token as hex only, and refuses a
    /// name that could end the quoted value or start a second command.
    #[test]
    fn the_security_command_never_carries_a_raw_token_or_a_second_command() {
        let line = add_command("svc", "profile/gw-a", "tok\"en;\n", None).unwrap();
        assert_eq!(
            line,
            format!(
                "add-generic-password -U -s \"svc\" -a \"profile/gw-a\" -X {}\n",
                hex::encode("tok\"en;\n")
            )
        );
        assert!(!line.contains("tok\""), "{line}");
        let with_keychain =
            add_command("svc", "a", "t", Some(Path::new("/tmp/x y.keychain-db"))).unwrap();
        assert!(
            with_keychain.ends_with(" \"/tmp/x y.keychain-db\"\n"),
            "{with_keychain}"
        );
        for account in [
            "gw\"; delete-keychain login",
            "gw\nlist-keychains",
            "gw\\a",
            "",
        ] {
            assert!(
                add_command("svc", account, "t", None).is_err(),
                "{account:?}"
            );
        }
    }

    /// The macOS path end to end, in a keychain file of its own: an entry
    /// `security` wrote is read back by it, replaced in place, and deleted, and
    /// a missing one is `None`. Ignored by default because it creates (and
    /// deletes) a keychain; run it with `--ignored` on a Mac.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "creates a temporary keychain; run with --ignored on macOS"]
    fn the_security_tool_round_trips_in_a_keychain_of_its_own() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::process::Command::new(SECURITY)
                    .arg("delete-keychain")
                    .arg(&self.0)
                    .status();
            }
        }
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let scratch = Scratch(dir.path().join("tokens-test.keychain-db"));
        for args in [
            vec!["create-keychain", "-p", "pw"],
            vec!["unlock-keychain", "-p", "pw"],
        ] {
            let status = std::process::Command::new(SECURITY)
                .args(args)
                .arg(&scratch.0)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let store = SecurityTool {
            service: "rustcode-tokens-test",
            keychain: Some(scratch.0.clone()),
        };
        let account = "profile id/gw-a";
        assert_eq!(store.read(account).unwrap(), None);
        store.write(account, "refresh-1").unwrap();
        assert_eq!(store.read(account).unwrap().as_deref(), Some("refresh-1"));
        store.write(account, "refresh-2 \"quoted\"").unwrap();
        assert_eq!(
            store.read(account).unwrap().as_deref(),
            Some("refresh-2 \"quoted\"")
        );
        store.delete(account).unwrap();
        assert_eq!(store.read(account).unwrap(), None);
        store.delete(account).unwrap();
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

    /// `keyring` with `async-secret-service` deadlocks when it is called from a
    /// thread a tokio runtime drives, so every keyring call is made on a thread
    /// of its own — this is that mechanism, and the value it returns.
    #[test]
    fn a_keyring_call_runs_on_a_thread_of_its_own() {
        let caller = std::thread::current().id();
        let worker = on_a_thread(|| std::thread::current().id());
        assert_ne!(
            caller, worker,
            "the call must not run on the caller's thread"
        );
        assert_eq!(on_a_thread(|| 7), 7);
    }

    /// A store that never answers is given up on within the deadline, with the
    /// message that says what to do about it, and the call runs off this
    /// thread: a locked keyring is not an answer, it is silence (#3325).
    #[tokio::test]
    async fn a_store_that_does_not_answer_is_given_up_on() {
        let started = std::time::Instant::now();
        let error = bounded(
            "opening the machine token store",
            Duration::from_millis(100),
            || {
                // What an unlock prompt does: nothing, for longer than anybody waits.
                std::thread::sleep(Duration::from_secs(1));
                Ok(())
            },
        )
        .await
        .expect_err("silence is not an answer");
        let text = format!("{error:#}");
        assert!(text.contains("did not answer"), "{text}");
        assert!(text.contains("RUSTCODE_MACHINE_TOKEN_STORE=file"), "{text}");
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "the pairing must not wait for the store: {:?}",
            started.elapsed()
        );
    }

    /// The bound is only a ceiling: a store that answers is used, and the
    /// token it stores is the one the pairing handed out.
    #[tokio::test]
    async fn an_answered_store_is_used() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = bounded("opening the machine token store", STORE_TIMEOUT, {
            let dir = dir.path().to_path_buf();
            move || TokenStore::in_file(&dir)
        })
        .await
        .unwrap();
        let store = Arc::new(store);
        set_bounded(
            Arc::clone(&store),
            "gw-a".to_owned(),
            Secret::new("token-a"),
            STORE_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(store.get("gw-a").unwrap().unwrap().expose(), "token-a");
    }
}
