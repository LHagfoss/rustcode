//! Claude plan usage for the Claude Code harness (#2845).
//!
//! A signed-in Claude Code keeps its OAuth sign-in in the macOS Keychain
//! (`Claude Code-credentials`) or, elsewhere, `~/.claude/.credentials.json`.
//! Its access token can read the account's plan windows from
//! `api.anthropic.com/api/oauth/usage` (5-hour, weekly, weekly per model).
//! This only reads: it never refreshes the token (a refresh would rotate it
//! under Claude Code, as #2765 did for OpenAI), so an expired sign-in is
//! reported and left to Claude Code, and the next read picks up whatever Claude
//! Code renewed. Asked at most once a minute, unless the caller forces a read:
//! Settings › Usage's Refresh does, so a sign-in renewed in Claude Code is seen
//! at once instead of up to a minute (or a paused background read) later.
//!
//! A failed read says why in a machine-readable reason, since a retry fixes
//! some failures and not others: an expired or refused sign-in is renewed in
//! Claude Code and must be read again, while a Keychain prompt the user denied
//! must not be raised again on its own.

use super::rate_limits::RateLimitWindow;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The Keychain item Claude Code keeps the default login in.
const DEFAULT_KEYCHAIN_ITEM: &str = "Claude Code-credentials";

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const FRESH: Duration = Duration::from_secs(60);
const TIMEOUT: Duration = Duration::from_secs(10);

/// One labelled window: `5-hour`, `Weekly`, `Weekly · Opus`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LabelledWindow {
    pub label: &'static str,
    pub window: RateLimitWindow,
}

/// What a read found: the windows, or why there are none.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClaudeUsage {
    Windows {
        plan: Option<String>,
        windows: Vec<LabelledWindow>,
    },
    /// No Claude Code sign-in on this machine.
    NotSignedIn,
    /// Signed in, but the numbers could not be read; `reason` says whether a
    /// retry can help, `message` says what the user can do.
    Unavailable { reason: NoUsage, message: String },
}

impl ClaudeUsage {
    fn unavailable(reason: NoUsage, message: impl Into<String>) -> Self {
        ClaudeUsage::Unavailable {
            reason,
            message: message.into(),
        }
    }
}

/// Why a run found no numbers. The reason travels to the app
/// (`_rustcode/usage/plans`'s `reason`), which retries what a retry can fix
/// and leaves alone what would only ask the user again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoUsage {
    /// Claude Code's stored access token has passed `expiresAt`. Claude Code
    /// renews it the next time it runs; RustCode only reads the new one.
    Expired,
    /// Anthropic refused the stored token (HTTP 401): sign in again in Claude
    /// Code, and the next read uses the new token.
    Refused,
    /// Anthropic does not allow this sign-in to read plan usage (HTTP 403).
    /// Renewing the same sign-in does not change that, so it is not retried on
    /// its own.
    Forbidden,
    /// macOS would not hand Claude Code's Keychain item over (a denied prompt,
    /// a locked Keychain, no UI to ask in). Asking again on the next poll would
    /// ask the user again, so only an explicit look retries it.
    Denied,
    /// No answer, an unexpected status, or an unreadable sign-in: worth trying
    /// again at the next poll.
    Transient,
}

impl NoUsage {
    /// How the app hears it, in `_rustcode/usage/plans`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            NoUsage::Expired => "expired",
            NoUsage::Refused => "refused",
            NoUsage::Forbidden => "forbidden",
            NoUsage::Denied => "denied",
            NoUsage::Transient => "transient",
        }
    }
}

/// Whether a read may answer from the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Freshness {
    /// The background poll: the cached answer while it is under a minute old.
    Cached,
    /// An explicit look (Settings › Usage's Refresh): always read again, so a
    /// sign-in renewed in Claude Code is seen at once.
    Now,
}

/// One entry per config directory: `None` is the default login, `Some` a
/// named account's own `CLAUDE_CONFIG_DIR`, whose numbers are its own.
static CACHE: Mutex<Option<HashMap<Option<PathBuf>, Entry>>> = Mutex::new(None);

/// One read at a time per config directory: a forced read for Settings › Usage
/// and the background poll for the same login share it, so they never read the
/// same Keychain item at once (macOS could ask twice).
static READS: Mutex<Option<HashMap<Option<PathBuf>, Arc<tokio::sync::Mutex<()>>>>> =
    Mutex::new(None);

/// The config directories whose Keychain item macOS refused. A background read
/// does not ask again — that is what would prompt the user again — and uses the
/// credentials file alone until an explicit look asks macOS once more. In
/// memory only, like the cache: a restart asks once at the most.
static DENIED: Mutex<Option<HashSet<Option<PathBuf>>>> = Mutex::new(None);

/// A remembered answer and when the read that gave it started.
struct Entry {
    started: Instant,
    usage: ClaudeUsage,
}

/// The plan windows of the login in `config_dir` (the default login when
/// `None`), from the cache while fresh unless `freshness` forces a read. Two
/// accounts never share numbers, and one account is read once at a time.
pub(crate) async fn usage(config_dir: Option<&Path>, freshness: Freshness) -> ClaudeUsage {
    let key = config_dir.map(Path::to_path_buf);
    if let Some(cached) = cached(&key, freshness) {
        return cached;
    }
    // When this ask was made: a read that *began* after this point saw the same
    // Keychain as one started now would, so its answer is shared instead of
    // asking the Keychain a second time (and possibly prompting twice).
    let requested = Instant::now();
    let read_lock = read_lock(&key);
    let _reading = read_lock.lock().await;
    if let Some(cached) = settled_while_waiting(&key, freshness, requested) {
        return cached;
    }
    let started = Instant::now();
    let read = read(config_dir, freshness).await;
    remember(key, started, &read);
    read
}

/// Whether a background read may skip the Keychain because macOS already
/// refused it. An explicit read always asks, so the user can allow it again.
fn skip_keychain(freshness: Freshness, blocked: bool) -> bool {
    freshness == Freshness::Cached && blocked
}

/// Whether macOS refused this directory's Keychain item on an earlier read.
fn keychain_blocked(key: &Option<PathBuf>) -> bool {
    DENIED
        .lock()
        .map(|denied| denied.as_ref().is_some_and(|denied| denied.contains(key)))
        .unwrap_or(false)
}

/// Records, or clears, that macOS refused this directory's Keychain item.
fn remember_keychain_denial(key: &Option<PathBuf>, denied: bool) {
    let Ok(mut blocked) = DENIED.lock() else {
        return;
    };
    let blocked = blocked.get_or_insert_with(HashSet::new);
    if denied {
        blocked.insert(key.clone());
    } else {
        blocked.remove(key);
    }
}

/// Whether a failed lookup was macOS refusing the item, as opposed to there
/// being none or the item being unusable.
fn is_denial(miss: &NoCredentials) -> bool {
    matches!(miss, NoCredentials::Unavailable(NoUsage::Denied, _))
}

/// The lock that makes reads of one login take turns.
fn read_lock(key: &Option<PathBuf>) -> Arc<tokio::sync::Mutex<()>> {
    let mut reads = READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reads
        .get_or_insert_with(HashMap::new)
        .entry(key.clone())
        .or_default()
        .clone()
}

/// What a read learned while this call waited for its turn, if it can serve
/// this call. A cached answer serves the background poll while it is fresh. A
/// forced read is served only by a read that began after this ask was made:
/// that one saw the renewed sign-in this ask is asking about.
fn settled_while_waiting(
    key: &Option<PathBuf>,
    freshness: Freshness,
    requested: Instant,
) -> Option<ClaudeUsage> {
    if freshness == Freshness::Cached {
        return cached(key, freshness);
    }
    let cache = CACHE.lock().ok()?;
    let entry = cache.as_ref()?.get(key)?;
    (entry.started >= requested).then(|| entry.usage.clone())
}

/// The fresh cached answer for one config directory, if there is one and the
/// caller did not ask for a forced read.
fn cached(key: &Option<PathBuf>, freshness: Freshness) -> Option<ClaudeUsage> {
    if freshness == Freshness::Now {
        return None;
    }
    let cache = CACHE.lock().ok()?;
    let entry = cache.as_ref()?.get(key)?;
    (entry.started.elapsed() < FRESH).then(|| entry.usage.clone())
}

/// Keeps an answer, unless a read that started later has already answered: a
/// forced read must not be undone by a background one that began before it.
fn remember(key: Option<PathBuf>, started: Instant, usage: &ClaudeUsage) {
    if let Ok(mut cache) = CACHE.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.get(&key).is_some_and(|known| known.started > started) {
            return;
        }
        cache.insert(
            key,
            Entry {
                started,
                usage: usage.clone(),
            },
        );
    }
}

/// A login's stored sign-in, and how it was reached.
struct Stored {
    /// The credentials as stored (`claudeAiOauth` and the rest).
    value: Value,
    /// macOS refused the Keychain item, so this came from the credentials
    /// file instead. If the file's sign-in is no use either, the refusal is
    /// what the user must act on: reported as an expiry, the app would retry
    /// on every poll, and every retry would ask macOS for the item again.
    keychain_denied: bool,
}

/// The token to use, or why the stored sign-in cannot be used. A Keychain
/// refusal that the credentials file did not make up for stays a refusal,
/// never an expiry.
fn usable_token(stored: &Stored, now_ms: i64) -> Result<&str, ClaudeUsage> {
    match sign_in(&stored.value, now_ms) {
        SignIn::Fresh(token) => Ok(token),
        SignIn::Expired | SignIn::Unreadable if stored.keychain_denied => {
            Err(ClaudeUsage::unavailable(NoUsage::Denied, KEYCHAIN_REFUSED))
        }
        SignIn::Expired => Err(ClaudeUsage::unavailable(
            NoUsage::Expired,
            "Claude Code's sign-in has expired. Open Claude Code to renew it, then Refresh.",
        )),
        SignIn::Unreadable => Err(ClaudeUsage::unavailable(
            NoUsage::Transient,
            "Claude Code's stored sign-in could not be read. Ask Claude Code to sign in again.",
        )),
    }
}

async fn read(config_dir: Option<&Path>, freshness: Freshness) -> ClaudeUsage {
    let stored = match credentials(config_dir, freshness).await {
        Ok(stored) => stored,
        Err(NoCredentials::None) => return ClaudeUsage::NotSignedIn,
        Err(NoCredentials::Unavailable(reason, message)) => {
            return ClaudeUsage::unavailable(reason, message);
        }
    };
    match usable_token(&stored, chrono::Utc::now().timestamp_millis()) {
        Ok(token) => request_failure(&stored, fetch(&stored, token).await),
        Err(failure) => failure,
    }
}

/// Asks Anthropic for the plan windows with `token`: the windows, or why the
/// request did not produce them.
async fn fetch(stored: &Stored, token: &str) -> ClaudeUsage {
    let plan = stored
        .value
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("rustcode/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ClaudeUsage::unavailable(NoUsage::Transient, error.to_string()),
    };
    let response = match client
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return ClaudeUsage::unavailable(NoUsage::Transient, "Anthropic did not answer.");
        }
    };
    let status = response.status();
    if !status.is_success() {
        return http_failure(status.as_u16());
    }
    match response.json::<Value>().await {
        Ok(body) => ClaudeUsage::Windows {
            plan,
            windows: parse(&body),
        },
        Err(_) => {
            ClaudeUsage::unavailable(NoUsage::Transient, "Anthropic's answer could not be read.")
        }
    }
}

/// What the app hears when the plan request did not succeed. A refusal macOS
/// gave for the Keychain item outranks the request's own failure while the
/// sign-in came from the credentials file: a retryable reason there would send
/// the app back to the Keychain on every poll, and each of those reads would
/// ask the user again. A request that answered keeps its windows.
fn request_failure(stored: &Stored, answered: ClaudeUsage) -> ClaudeUsage {
    match answered {
        ClaudeUsage::Unavailable { .. } if stored.keychain_denied => {
            ClaudeUsage::unavailable(NoUsage::Denied, KEYCHAIN_REFUSED)
        }
        other => other,
    }
}

/// What a non-success answer from Anthropic means for the sign-in. A 401 is
/// the token to renew in Claude Code; a 403 is a refusal of access that
/// renewing the same sign-in does not fix, so the app must not keep asking. Any
/// other status is worth one more try.
fn http_failure(status: u16) -> ClaudeUsage {
    match status {
        401 => ClaudeUsage::unavailable(
            NoUsage::Refused,
            "Claude Code's sign-in was refused. Sign in again in Claude Code, then Refresh.",
        ),
        403 => ClaudeUsage::unavailable(
            NoUsage::Forbidden,
            "Anthropic does not allow this sign-in to read plan usage. Press Refresh to try once more.",
        ),
        code => ClaudeUsage::unavailable(
            NoUsage::Transient,
            format!("Anthropic answered HTTP {code}."),
        ),
    }
}

/// The Keychain item a config directory's sign-in is kept in: the default
/// item for the default login, else `Claude Code-credentials-` and the first
/// 8 hex digits of the directory path's SHA-256. Claude Code does not document
/// the name; a missing item is reported as no sign-in, never guessed at.
pub(crate) fn keychain_item_name(config_dir: &Path) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(config_dir.to_string_lossy().as_bytes());
    let prefix = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{DEFAULT_KEYCHAIN_ITEM}-{prefix}")
}

/// Whether the Keychain holds a sign-in for `config_dir`. Reads no secret:
/// only asks whether the item exists.
pub(crate) fn keychain_item_exists(config_dir: &Path) -> bool {
    keychain_item_named(&keychain_item_name(config_dir))
}

/// Whether the Keychain holds the default login's sign-in, the item Claude Code
/// uses with no `CLAUDE_CONFIG_DIR`. Reads no secret.
pub(crate) fn default_keychain_item_exists() -> bool {
    keychain_item_named(DEFAULT_KEYCHAIN_ITEM)
}

fn keychain_item_named(item: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", item])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = item;
        false
    }
}

/// Why a login's stored sign-in was not handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NoCredentials {
    /// There is none: no Keychain item and no credentials file either.
    None,
    /// There may be one, but it could not be read; the reason says whether an
    /// explicit retry can help, and the text says what happened. Never
    /// reported as signed out: a denied Keychain prompt or a locked Keychain is
    /// not the user signing out.
    Unavailable(NoUsage, &'static str),
}

/// macOS would not hand the item over: a denied prompt, a locked Keychain, no
/// UI to ask in.
const KEYCHAIN_REFUSED: &str =
    "macOS did not allow reading Claude Code's sign-in from the Keychain.";

/// The item was printed but held no sign-in.
const KEYCHAIN_UNREADABLE: &str = "Claude Code's sign-in in the Keychain could not be read.";

/// What a failed `security find-generic-password -w` means for a login.
///
/// `security` exits with the low byte of the Security framework's `OSStatus`, and
/// only `errSecItemNotFound` (`-25300` → 44) means there is no such item. A
/// denied prompt exits 128 (`errSecUserCanceled`), a locked or UI-less Keychain
/// 36 (`errSecInteractionNotAllowed`), a refused one 51 (`errSecAuthFailed`);
/// none of them says the user is signed out. Neither does `security` failing to
/// run at all (no status).
fn keychain_miss(code: Option<i32>) -> NoCredentials {
    match code {
        Some(44) => NoCredentials::None,
        _ => NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED),
    }
}

/// What one `security find-generic-password -w` run left: the stored sign-in, or
/// why there is none. A run that succeeded but printed something else reads as
/// signed in but unreadable, never as signed out.
fn keychain_outcome(
    success: bool,
    code: Option<i32>,
    stdout: &[u8],
) -> Result<Value, NoCredentials> {
    if success {
        // Printed, but no sign-in in it: the item is there, so an explicit
        // retry (or Claude Code rewriting it) can fix this; a background
        // retry costs another read, not another prompt.
        serde_json::from_slice(stdout)
            .map_err(|_| NoCredentials::Unavailable(NoUsage::Transient, KEYCHAIN_UNREADABLE))
    } else {
        Err(keychain_miss(code))
    }
}

/// Claude Code's stored sign-in, as JSON, for `config_dir` (the default login
/// when `None`).
async fn credentials(
    config_dir: Option<&Path>,
    freshness: Freshness,
) -> Result<Stored, NoCredentials> {
    let file = || config_dir.map(Path::to_path_buf).or_else(default_dir);
    #[cfg(target_os = "macos")]
    {
        let key = config_dir.map(Path::to_path_buf);
        // A refusal macOS already gave is not repeated on its own: the file is
        // read instead. Only an explicit look asks the Keychain again, which is
        // the user saying they want to allow it.
        let skipped = skip_keychain(freshness, keychain_blocked(&key));
        let miss = if skipped {
            NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED)
        } else {
            let item = config_dir.map_or_else(
                || DEFAULT_KEYCHAIN_ITEM.to_owned(),
                |dir| keychain_item_name(dir),
            );
            let output = tokio::process::Command::new("/usr/bin/security")
                .args(["find-generic-password", "-s", &item, "-w"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await
                .map_err(|_| NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED));
            let miss = match output {
                Ok(output) => match keychain_outcome(
                    output.status.success(),
                    output.status.code(),
                    &output.stdout,
                ) {
                    Ok(value) => {
                        // macOS handed it over: nothing is blocked any more.
                        remember_keychain_denial(&key, false);
                        return Ok(Stored {
                            value,
                            keychain_denied: false,
                        });
                    }
                    Err(miss) => miss,
                },
                Err(miss) => miss,
            };
            remember_keychain_denial(&key, is_denial(&miss));
            miss
        };
        // The credentials file may still hold the sign-in (and does elsewhere),
        // so it has its chance before a failed lookup is called a sign-out. A
        // refusal macOS gave is remembered: if the file cannot stand in, the
        // user must allow the Keychain, not read an expiry that retries it.
        let denied = is_denial(&miss);
        if let Some(dir) = file()
            && let Some(value) = read_credentials_file(dir).await
        {
            return Ok(Stored {
                value,
                keychain_denied: denied,
            });
        }
        Err(miss)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = freshness;
        match file() {
            Some(dir) => read_credentials_file(dir)
                .await
                .map(|value| Stored {
                    value,
                    keychain_denied: false,
                })
                .ok_or(NoCredentials::None),
            None => Err(NoCredentials::None),
        }
    }
}

fn default_dir() -> Option<PathBuf> {
    crate::shell_env::env_var("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))
}

async fn read_credentials_file(dir: PathBuf) -> Option<Value> {
    let text = tokio::fs::read_to_string(dir.join(".credentials.json"))
        .await
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// What the credentials file's sign-in is good for.
enum SignIn<'a> {
    /// A token that is valid for at least another minute.
    Fresh(&'a str),
    /// The stored token has passed `expiresAt` (or is about to): Claude Code
    /// renews it the next time it runs, and RustCode never does.
    Expired,
    /// There is a sign-in, but it holds no usable token.
    Unreadable,
}

fn sign_in(credentials: &Value, now_ms: i64) -> SignIn<'_> {
    let Some(oauth) = credentials.get("claudeAiOauth") else {
        return SignIn::Unreadable;
    };
    let Some(token) = oauth.get("accessToken").and_then(Value::as_str) else {
        return SignIn::Unreadable;
    };
    if token.is_empty() {
        return SignIn::Unreadable;
    }
    match oauth.get("expiresAt").and_then(Value::as_i64) {
        Some(at) if at <= now_ms + 60_000 => SignIn::Expired,
        _ => SignIn::Fresh(token),
    }
}

/// `{five_hour, seven_day, seven_day_opus, seven_day_sonnet}` windows of
/// `{utilization (percent), resets_at (RFC 3339)}`.
fn parse(body: &Value) -> Vec<LabelledWindow> {
    [
        ("five_hour", "5-hour", 300),
        ("seven_day", "Weekly", 10_080),
        ("seven_day_opus", "Weekly · Opus", 10_080),
        ("seven_day_sonnet", "Weekly · Sonnet", 10_080),
    ]
    .into_iter()
    .filter_map(|(key, label, minutes)| {
        let window = body.get(key)?;
        let used_percent = window.get("utilization")?.as_f64()?.clamp(0.0, 100.0);
        let resets_at = window
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp());
        Some(LabelledWindow {
            label,
            window: RateLimitWindow {
                used_percent,
                window_minutes: Some(minutes),
                resets_at,
            },
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_plan_windows() {
        let windows = parse(&json!({
            "five_hour": {"utilization": 12.0, "resets_at": "2026-11-02T23:00:00.000000+00:00"},
            "seven_day": {"utilization": 41.5, "resets_at": "2026-11-06T10:00:00+00:00"},
            "seven_day_opus": null,
            "seven_day_sonnet": {"utilization": 3, "resets_at": null}
        }));
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].label, "5-hour");
        assert_eq!(windows[0].window.used_percent, 12.0);
        assert_eq!(windows[0].window.window_minutes, Some(300));
        assert_eq!(windows[0].window.resets_at, Some(1_793_660_400));
        assert_eq!(windows[1].label, "Weekly");
        assert_eq!(windows[2].label, "Weekly · Sonnet");
        assert_eq!(windows[2].window.resets_at, None);
    }

    #[test]
    fn each_config_directory_has_its_own_answer() {
        let first = Some(PathBuf::from("/tmp/claude-account-a"));
        let second = Some(PathBuf::from("/tmp/claude-account-b"));
        let started = Instant::now();
        remember(
            first.clone(),
            started,
            &ClaudeUsage::Windows {
                plan: Some("max".into()),
                windows: Vec::new(),
            },
        );
        remember(second.clone(), started, &ClaudeUsage::NotSignedIn);
        assert!(matches!(
            cached(&first, Freshness::Cached),
            Some(ClaudeUsage::Windows { .. })
        ));
        assert_eq!(
            cached(&second, Freshness::Cached),
            Some(ClaudeUsage::NotSignedIn)
        );
        // The default login is its own entry too.
        assert_eq!(cached(&None, Freshness::Cached), None);
        let expired = ClaudeUsage::unavailable(NoUsage::Expired, "expired");
        remember(None, started, &expired);
        assert_eq!(cached(&None, Freshness::Cached), Some(expired));
        assert!(
            matches!(
                cached(&first, Freshness::Cached),
                Some(ClaudeUsage::Windows { .. })
            ),
            "one account's answer never replaces another's"
        );
    }

    /// Settings › Usage's Refresh must not be answered with the minute-old
    /// "expired" a background read left: the user has just renewed the sign-in.
    #[test]
    fn a_forced_read_ignores_the_cache() {
        let key = Some(PathBuf::from("/tmp/claude-forced"));
        remember(
            key.clone(),
            Instant::now(),
            &ClaudeUsage::unavailable(NoUsage::Expired, "expired"),
        );
        assert!(matches!(
            cached(&key, Freshness::Cached),
            Some(ClaudeUsage::Unavailable { .. })
        ));
        assert_eq!(cached(&key, Freshness::Now), None);
    }

    /// A background read that began before a forced one must not put its older
    /// answer back after the forced read has answered.
    #[test]
    fn a_read_that_started_later_wins() {
        let key = Some(PathBuf::from("/tmp/claude-race"));
        let early = Instant::now();
        let late = early + Duration::from_millis(50);
        let fresh = ClaudeUsage::Windows {
            plan: None,
            windows: Vec::new(),
        };
        remember(key.clone(), late, &fresh);
        remember(
            key.clone(),
            early,
            &ClaudeUsage::unavailable(NoUsage::Expired, "expired"),
        );
        assert_eq!(cached(&key, Freshness::Cached), Some(fresh));
    }

    #[test]
    fn only_a_missing_item_is_not_signed_in() {
        // `security` exits with the low byte of the OSStatus: errSecItemNotFound
        // (-25300) is 44, the one code that means the item does not exist.
        assert_eq!(keychain_miss(Some(44)), NoCredentials::None);
        // A denied prompt (errSecUserCanceled, -128), a locked or UI-less
        // Keychain (errSecInteractionNotAllowed, -25308 -> 36), a refused one
        // (errSecAuthFailed, -25293 -> 51), an I/O error (-36 -> 220) and no
        // status at all are all "could not read", never "signed out".
        for code in [Some(128), Some(36), Some(51), Some(220), Some(1), None] {
            assert_eq!(
                keychain_miss(code),
                NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED),
                "{code:?}"
            );
        }
    }

    #[test]
    fn a_keychain_answer_is_a_sign_in_or_a_reason() {
        let stored = json!({"claudeAiOauth": {"accessToken": "t"}});
        assert_eq!(
            keychain_outcome(true, Some(0), stored.to_string().as_bytes()),
            Ok(stored.clone())
        );
        // Printed, but no sign-in in it: signed in and unreadable, not signed
        // out. Its reason is the retryable one, not a denial.
        assert_eq!(
            keychain_outcome(true, Some(0), b"not json"),
            Err(NoCredentials::Unavailable(
                NoUsage::Transient,
                KEYCHAIN_UNREADABLE
            ))
        );
        assert_eq!(
            keychain_outcome(false, Some(44), b""),
            Err(NoCredentials::None)
        );
        assert_eq!(
            keychain_outcome(false, Some(128), b""),
            Err(NoCredentials::Unavailable(
                NoUsage::Denied,
                KEYCHAIN_REFUSED
            ))
        );
    }

    #[test]
    fn the_keychain_item_is_named_after_the_directory() {
        let account = std::path::Path::new("/Users/me/.claude-paral-2");
        let name = keychain_item_name(account);
        assert!(name.starts_with("Claude Code-credentials-"), "{name}");
        let suffix = name.trim_start_matches("Claude Code-credentials-");
        assert_eq!(suffix.len(), 8, "{name}");
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        assert_eq!(
            name,
            keychain_item_name(account),
            "the same directory always names the same item"
        );
        assert_ne!(
            name,
            keychain_item_name(std::path::Path::new("/Users/me/.claude-paral-3")),
            "another directory is another login"
        );
        // Pinned: Claude Code derives it from the directory path's SHA-256.
        use sha2::Digest;
        let digest = sha2::Sha256::digest(account.to_string_lossy().as_bytes());
        assert_eq!(suffix, &format!("{digest:x}")[..8]);
    }

    #[test]
    fn an_expired_token_is_reported_as_expired_not_used() {
        let now = 1_000_000;
        let stored = |expires: i64| json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": expires, "subscriptionType": "max"}});
        assert!(matches!(
            sign_in(&stored(now + 3_600_000), now),
            SignIn::Fresh("t")
        ));
        // About to expire counts as expired: the request would arrive too late.
        assert!(matches!(
            sign_in(&stored(now + 30_000), now),
            SignIn::Expired
        ));
        assert!(matches!(sign_in(&stored(now - 1), now), SignIn::Expired));
        // A refreshed sign-in is used at once, without RustCode renewing anything.
        assert!(matches!(
            sign_in(&stored(now + 3_600_000), now + 60_000),
            SignIn::Fresh("t")
        ));
        // No sign-in at all is not an expiry: nothing to renew.
        assert!(matches!(sign_in(&json!({}), now), SignIn::Unreadable));
        assert!(matches!(
            sign_in(&json!({"claudeAiOauth": {"accessToken": ""}}), now),
            SignIn::Unreadable
        ));
    }

    /// The reason `usable_token` refuses a stored sign-in with, for the tests
    /// below.
    fn refusal(stored: &Stored, now: i64) -> NoUsage {
        match usable_token(stored, now) {
            Err(ClaudeUsage::Unavailable { reason, .. }) => reason,
            other => panic!("{other:?}"),
        }
    }

    /// A denied Keychain whose credentials-file fallback holds an expired
    /// sign-in must stay a denial. Reported as an expiry, the app would retry
    /// every poll, and every retry would ask macOS to hand the item over again.
    #[test]
    fn a_denied_keychain_stays_denied_when_the_file_is_no_use() {
        let now = 1_000_000;
        let expired = Stored {
            value: json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": now - 1}}),
            keychain_denied: true,
        };
        assert_eq!(refusal(&expired, now), NoUsage::Denied);
        // The copy is the Keychain one, which says how to allow the read.
        assert!(matches!(
            usable_token(&expired, now),
            Err(ClaudeUsage::Unavailable { message, .. }) if message == KEYCHAIN_REFUSED
        ));
        // A file with no usable token at all is no stand-in either.
        let unreadable = Stored {
            value: json!({}),
            keychain_denied: true,
        };
        assert_eq!(refusal(&unreadable, now), NoUsage::Denied);
    }

    /// The fallback itself still works: a refusal for the item does not throw
    /// away a fresh sign-in the credentials file holds, and without a refusal
    /// an unusable file keeps its ordinary retryable reason.
    #[test]
    fn a_denied_keychain_still_uses_a_fresh_file_sign_in() {
        let now = 1_000_000;
        let fresh = Stored {
            value: json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": now + 3_600_000}}),
            keychain_denied: true,
        };
        assert_eq!(usable_token(&fresh, now), Ok("t"));
        let expired = Stored {
            value: json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": now - 1}}),
            keychain_denied: false,
        };
        assert_eq!(refusal(&expired, now), NoUsage::Expired);
        let unreadable = Stored {
            value: json!({}),
            keychain_denied: false,
        };
        assert_eq!(refusal(&unreadable, now), NoUsage::Transient);
    }

    /// A fallback whose request failed — a 401, a 500, no answer at all — must
    /// still report the Keychain refusal: `refused` or `transient` would send
    /// the app back to the Keychain on every poll, and each read would prompt
    /// the user again.
    #[test]
    fn a_failed_request_on_a_denied_fallback_stays_denied() {
        let stored = Stored {
            value: json!({}),
            keychain_denied: true,
        };
        for failure in [
            http_failure(401),
            http_failure(403),
            http_failure(500),
            ClaudeUsage::unavailable(NoUsage::Transient, "Anthropic did not answer."),
        ] {
            match request_failure(&stored, failure) {
                ClaudeUsage::Unavailable { reason, message } => {
                    assert_eq!(reason, NoUsage::Denied);
                    assert_eq!(message, KEYCHAIN_REFUSED);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// Without that refusal the request's own reason stands, so a token renewal
    /// and a transient failure are retried as before.
    #[test]
    fn a_failed_request_without_a_denial_keeps_its_reason() {
        let stored = Stored {
            value: json!({}),
            keychain_denied: false,
        };
        let reason = |failure| match request_failure(&stored, failure) {
            ClaudeUsage::Unavailable { reason, .. } => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(reason(http_failure(401)), NoUsage::Refused);
        assert_eq!(reason(http_failure(403)), NoUsage::Forbidden);
        assert_eq!(reason(http_failure(503)), NoUsage::Transient);
    }

    /// A fallback that answered keeps its numbers: the refusal only outranks a
    /// failure, never a successful read.
    #[test]
    fn a_denied_fallback_that_answers_keeps_its_windows() {
        let stored = Stored {
            value: json!({}),
            keychain_denied: true,
        };
        let answered = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: Vec::new(),
        };
        assert_eq!(request_failure(&stored, answered.clone()), answered);
    }

    /// A refusal macOS gave is remembered. When the file then *works*, the next
    /// polls must not ask the Keychain again (asking is what prompts): they read
    /// the file only, and still draw its numbers. Only an explicit look asks
    /// macOS again, and a handover clears the state.
    #[test]
    fn a_denied_keychain_is_not_asked_again_after_a_successful_fallback() {
        let key = Some(PathBuf::from("/tmp/claude-denied-state"));
        let miss = NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED);
        remember_keychain_denial(&key, is_denial(&miss));
        for _ in 0..3 {
            assert!(skip_keychain(Freshness::Cached, keychain_blocked(&key)));
            // The file's numbers still come through: a successful fallback is a
            // read, not a failure.
            let stored = Stored {
                value: json!({}),
                keychain_denied: true,
            };
            let answered = ClaudeUsage::Windows {
                plan: Some("max".into()),
                windows: Vec::new(),
            };
            assert_eq!(request_failure(&stored, answered.clone()), answered);
        }
        // An explicit look asks macOS once more; a handover clears the state, and
        // the background reads the Keychain again.
        assert!(!skip_keychain(Freshness::Now, keychain_blocked(&key)));
        remember_keychain_denial(&key, false);
        assert!(!keychain_blocked(&key));
        assert!(!skip_keychain(Freshness::Cached, keychain_blocked(&key)));
    }

    /// Only a refusal blocks later reads: an item that is missing or unreadable
    /// is not a Deny, so it must not stop the Keychain from being read.
    #[test]
    fn only_a_refusal_blocks_later_reads() {
        assert!(is_denial(&NoCredentials::Unavailable(
            NoUsage::Denied,
            KEYCHAIN_REFUSED
        )));
        assert!(!is_denial(&NoCredentials::None));
        assert!(!is_denial(&NoCredentials::Unavailable(
            NoUsage::Transient,
            KEYCHAIN_UNREADABLE
        )));
        let key = Some(PathBuf::from("/tmp/claude-not-blocked"));
        remember_keychain_denial(&key, is_denial(&NoCredentials::None));
        assert!(!keychain_blocked(&key));
    }

    #[test]
    fn every_failure_names_a_reason_the_app_can_act_on() {
        // These five are the whole vocabulary the app knows; see
        // `apps/desktop/src/acp/rustcode.ts`.
        assert_eq!(NoUsage::Expired.as_str(), "expired");
        assert_eq!(NoUsage::Refused.as_str(), "refused");
        assert_eq!(NoUsage::Forbidden.as_str(), "forbidden");
        assert_eq!(NoUsage::Denied.as_str(), "denied");
        assert_eq!(NoUsage::Transient.as_str(), "transient");
    }

    #[test]
    fn a_401_is_renewable_and_a_403_is_not() {
        let reason = |status| match http_failure(status) {
            ClaudeUsage::Unavailable { reason, .. } => reason,
            other => panic!("{other:?}"),
        };
        let message = |status| match http_failure(status) {
            ClaudeUsage::Unavailable { message, .. } => message,
            other => panic!("{other:?}"),
        };
        // Signing in again in Claude Code fixes a refused token.
        assert_eq!(reason(401), NoUsage::Refused);
        assert!(message(401).contains("Sign in again"));
        // A forbidden sign-in is not fixed by renewing it, and its copy must not
        // claim that it is.
        assert_eq!(reason(403), NoUsage::Forbidden);
        assert!(message(403).contains("does not allow"), "{}", message(403));
        assert!(!message(403).contains("Sign in again"));
        // Anything else is worth another try.
        for status in [429, 500, 503] {
            assert_eq!(reason(status), NoUsage::Transient, "{status}");
        }
    }

    /// The forced read a page makes and the background poll for the same login
    /// take turns: the one that waited shares an answer that began after its
    /// ask, and re-reads otherwise. Neither can ask macOS twice at once.
    #[test]
    fn a_read_that_began_after_the_ask_serves_it() {
        let key = Some(PathBuf::from("/tmp/claude-coalesce"));
        let requested = Instant::now();
        let answer = ClaudeUsage::unavailable(NoUsage::Expired, "expired");
        // A read that began before the ask may have seen the old sign-in: a
        // forced read needs its own.
        remember(key.clone(), requested - Duration::from_millis(50), &answer);
        assert_eq!(settled_while_waiting(&key, Freshness::Now, requested), None);
        // A read that began after it saw the renewed sign-in already.
        remember(key.clone(), requested, &answer);
        assert_eq!(
            settled_while_waiting(&key, Freshness::Now, requested),
            Some(answer.clone())
        );
        // The background poll shares any answer under a minute old.
        assert_eq!(
            settled_while_waiting(&key, Freshness::Cached, requested),
            Some(answer)
        );
    }
}
