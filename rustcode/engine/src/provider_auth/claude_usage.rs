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
//!
//! Claude Code keeps a login in the Keychain item for its config directory and
//! may also keep `<config dir>/.credentials.json`; both are read, and the copy
//! with the freshest token is used, since one can lag the other.
//!
//! Its access token lives for hours and is renewed only when Claude Code runs:
//! the refresh token beside it stays valid for weeks. An account nobody ran a
//! thread on since the access token lapsed therefore still reads as signed in
//! while its stored token has expired. Nothing here refreshes it, so instead of
//! only saying so the read answers with the last windows RustCode recorded for
//! that login (`claude-usage.json`), which the app shows "as of" when they were
//! read; the plain expiry is the answer only when there is nothing recorded yet.
//!
//! The endpoint rate-limits (#3300): it answered HTTP 429 while the app polled
//! it every minute, so numbers are kept for `WINDOWS_TTL` and every login has
//! one read in flight at most, however often a view is opened or a window comes
//! back into focus. A 429 is honoured rather than retried: the wait it names is
//! kept, else the wait doubles per refusal up to `BACKOFF_MAX`, and the cached
//! numbers are served with their age until it ends, so a rate limit is never a
//! banner the user could have done nothing about.

use super::rate_limits::RateLimitWindow;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The Keychain item Claude Code keeps the default login in.
const DEFAULT_KEYCHAIN_ITEM: &str = "Claude Code-credentials";

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// How long a failed read is served from the cache: a retry is what stops it
/// failing, and the next poll is a minute away.
const FRESH: Duration = Duration::from_secs(60);
/// How long the numbers are served from the cache. They move slowly — a 5-hour
/// and a weekly window — and the endpoint rate-limits, so one request per login
/// per interval is enough for the composer's meter and Settings › Usage.
const WINDOWS_TTL: Duration = Duration::from_secs(300);
/// The wait after a 429 that names no `Retry-After`, doubled for each further
/// refusal, and the ceiling both kinds of wait are kept under.
const BACKOFF_FIRST: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);
/// A `Retry-After` shorter than this is rounded up to it: even a refusal that
/// names a moment away is worth half a minute's quiet.
const BACKOFF_MIN: Duration = Duration::from_secs(30);
const TIMEOUT: Duration = Duration::from_secs(10);

/// What a 429 says when there are no cached numbers to show instead. The reason
/// is `transient`, so the app tries again — which the backoff paces.
const RATE_LIMITED: &str =
    "Anthropic is limiting how often plan usage is read. RustCode will wait before asking again.";

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
        /// Unix seconds of the read that recorded these numbers, when they are
        /// the last ones RustCode read for this login rather than a read made
        /// now: the stored access token has lapsed and Claude Code has not run
        /// since. The app shows them "as of" then.
        as_of: Option<i64>,
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
    /// Unix seconds when the numbers were read, for serving them later marked
    /// with their age.
    read_at: i64,
    usage: ClaudeUsage,
}

/// One login's wait after a 429, and how many refusals in a row it has taken:
/// the next refusal waits twice as long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Backoff {
    until: Instant,
    attempt: u32,
}

/// The logins that are waiting out a 429, in memory like the cache: a restart
/// asks once at the most, and one login's wait never holds another's numbers
/// back.
static BACKOFF: Mutex<Option<HashMap<Option<PathBuf>, Backoff>>> = Mutex::new(None);

/// The plan windows of the login in `config_dir` (the default login when
/// `None`), from the cache while fresh unless `freshness` forces a read. Two
/// accounts never share numbers, and one account is read once at a time. A login
/// Anthropic rate-limited is not asked again until its wait ends: its cached
/// numbers are served with their age instead, and an explicit look waits too.
pub(crate) async fn usage(config_dir: Option<&Path>, freshness: Freshness) -> ClaudeUsage {
    let key = config_dir.map(Path::to_path_buf);
    if let Some(cached) = cached(&key, freshness) {
        return cached;
    }
    // Anthropic refused for now and named when it would answer again: asking
    // before then would only be refused again, so the last numbers are served
    // with their age. An explicit look waits too — the interval is Anthropic's
    // to set, and a press of Refresh does not shorten it.
    if backing_off(&key) {
        return stale_windows(config_dir)
            .unwrap_or_else(|| ClaudeUsage::unavailable(NoUsage::Transient, RATE_LIMITED));
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
    fresh_enough(&entry.usage, entry.started.elapsed()).then(|| entry.usage.clone())
}

/// How long one cached answer is served: the numbers for `WINDOWS_TTL`, since
/// this endpoint rate-limits, and a failure for `FRESH`, since a retry is how it
/// stops failing.
fn ttl(answer: &ClaudeUsage) -> Duration {
    match answer {
        ClaudeUsage::Windows { .. } => WINDOWS_TTL,
        _ => FRESH,
    }
}

/// Whether an answer read `age` ago is still the one to serve.
fn fresh_enough(answer: &ClaudeUsage, age: Duration) -> bool {
    age < ttl(answer)
}

/// The last numbers this login's cache holds, marked with when they were read,
/// so the app shows their age instead of drawing them as this minute's. `None`
/// when the cache holds no numbers for it — a failure is no stand-in.
fn stale_windows(config_dir: Option<&Path>) -> Option<ClaudeUsage> {
    let key = config_dir.map(Path::to_path_buf);
    let cache = CACHE.lock().ok()?;
    let entry = cache.as_ref()?.get(&key)?;
    let ClaudeUsage::Windows { plan, windows, .. } = &entry.usage else {
        return None;
    };
    (!windows.is_empty()).then(|| ClaudeUsage::Windows {
        plan: plan.clone(),
        windows: windows.clone(),
        as_of: Some(entry.read_at),
    })
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
                read_at: read_at(usage),
                usage: usage.clone(),
            },
        );
    }
}

/// When the numbers in `usage` were read: their own `as_of` when they are ones
/// already marked with it, else now.
fn read_at(usage: &ClaudeUsage) -> i64 {
    match usage {
        ClaudeUsage::Windows {
            as_of: Some(at), ..
        } => *at,
        _ => chrono::Utc::now().timestamp(),
    }
}

/// Whether this login is waiting out a 429, clearing a wait that has ended.
fn backing_off(key: &Option<PathBuf>) -> bool {
    let Ok(mut state) = BACKOFF.lock() else {
        return false;
    };
    let Some(state) = state.as_mut() else {
        return false;
    };
    match state.get(key) {
        Some(entry) if entry.until > Instant::now() => true,
        Some(_) => {
            state.remove(key);
            false
        }
        None => false,
    }
}

/// Starts, or lengthens, this login's wait after a 429.
fn remember_backoff(config_dir: Option<&Path>, retry_after: Option<Duration>) {
    let key = config_dir.map(Path::to_path_buf);
    let Ok(mut state) = BACKOFF.lock() else {
        return;
    };
    let state = state.get_or_insert_with(HashMap::new);
    let attempt = state.get(&key).map_or(0, |entry| entry.attempt);
    let wait = backoff_wait(retry_after, attempt);
    state.insert(
        key,
        Backoff {
            until: Instant::now() + wait,
            attempt: attempt.saturating_add(1),
        },
    );
}

/// Ends this login's wait: an answer means Anthropic is talking to us again.
fn forget_backoff(config_dir: Option<&Path>) {
    let key = config_dir.map(Path::to_path_buf);
    let Ok(mut state) = BACKOFF.lock() else {
        return;
    };
    if let Some(state) = state.as_mut() {
        state.remove(&key);
    }
}

/// How long to wait after a 429: what its `Retry-After` asked for, else a
/// doubling wait — a minute, two, four — that stops at `BACKOFF_MAX`. Either is
/// kept under the ceiling, so a header that names an hour or a day is a long
/// pause, not a dead login.
fn backoff_wait(retry_after: Option<Duration>, attempt: u32) -> Duration {
    match retry_after {
        Some(after) => after.clamp(BACKOFF_MIN, BACKOFF_MAX),
        None => BACKOFF_FIRST
            .saturating_mul(1 << attempt.min(24))
            .min(BACKOFF_MAX),
    }
}

/// The wait a `Retry-After` header names: seconds, or an HTTP date. Anything
/// else is no answer, and the wait doubles on its own schedule instead.
fn retry_after(header: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    let header = header?.trim();
    if let Ok(seconds) = header.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(header).ok()?;
    // A date that has passed names no wait; the ceiling's floor covers it.
    Some(
        (at.with_timezone(&chrono::Utc) - now)
            .to_std()
            .unwrap_or(Duration::ZERO),
    )
}

/// A login's stored sign-ins, and how they were reached.
struct Stored {
    /// Every copy of the sign-in found for this login, in the order they were
    /// read: the Keychain item first, then `<config dir>/.credentials.json`.
    /// Claude Code writes back whichever store it could open, so one copy can
    /// lag the other; the copy with the freshest token is the one used.
    copies: Vec<Value>,
    /// macOS refused the Keychain item, so what there is came from the
    /// credentials file. If no copy is usable either, the refusal is what the
    /// user must act on: reported as an expiry, the app would retry on every
    /// poll, and every retry would ask macOS for the item again.
    keychain_denied: bool,
}

/// When a copy's access token expires, when it says (unix milliseconds).
fn expires_at(credentials: &Value) -> Option<i64> {
    credentials.get("claudeAiOauth")?.get("expiresAt")?.as_i64()
}

/// The freshest usable sign-in among the copies Claude Code keeps, and its
/// token: the copy whose token expires last, never mind which store it came
/// from. `Err` is why there is none, in words the user can act on; a Keychain
/// refusal that no copy made up for stays a refusal, never an expiry.
fn usable_sign_in(
    stored: &Stored,
    now_ms: i64,
    named: bool,
) -> Result<(&Value, &str), ClaudeUsage> {
    let mut best: Option<(&Value, &str, i64)> = None;
    let mut expired = false;
    for copy in &stored.copies {
        match sign_in(copy, now_ms) {
            SignIn::Fresh(token) => {
                // A copy that does not say when it expires counts as the
                // oldest: one that does is the better bet when picking.
                let at = expires_at(copy).unwrap_or(0);
                if best.is_none_or(|(_, _, known)| at > known) {
                    best = Some((copy, token, at));
                }
            }
            SignIn::Expired => expired = true,
            SignIn::Unreadable => {}
        }
    }
    match best {
        Some((copy, token, _)) => Ok((copy, token)),
        None => Err(failure_without_sign_in(
            stored.keychain_denied,
            expired,
            named,
        )),
    }
}

/// Why no stored copy could be used. A refusal macOS gave outranks the copies'
/// own state, and a lapsed access token is not the user signing out: Claude
/// Code renews it the next time it runs, so the message asks for that instead
/// of asking them to sign in again.
fn failure_without_sign_in(keychain_denied: bool, expired: bool, named: bool) -> ClaudeUsage {
    if keychain_denied {
        return ClaudeUsage::unavailable(NoUsage::Denied, KEYCHAIN_REFUSED);
    }
    if expired {
        return ClaudeUsage::unavailable(
            NoUsage::Expired,
            if named {
                EXPIRED_ACCOUNT
            } else {
                EXPIRED_DEFAULT
            },
        );
    }
    ClaudeUsage::unavailable(
        NoUsage::Transient,
        "Claude Code's stored sign-in could not be read. Ask Claude Code to sign in again.",
    )
}

async fn read(config_dir: Option<&Path>, freshness: Freshness) -> ClaudeUsage {
    let stored = match credentials(config_dir, freshness).await {
        Ok(stored) => stored,
        Err(NoCredentials::None) => return ClaudeUsage::NotSignedIn,
        Err(NoCredentials::Unavailable(reason, message)) => {
            return ClaudeUsage::unavailable(reason, message);
        }
    };
    match usable_sign_in(
        &stored,
        chrono::Utc::now().timestamp_millis(),
        config_dir.is_some(),
    ) {
        Ok((copy, token)) => match fetch(copy, token).await {
            Answer::Usage(answered) => {
                // Anthropic answered, so whatever it said, the wait is over.
                forget_backoff(config_dir);
                let answered = request_failure(&stored, answered);
                record_read(config_dir, &answered);
                answered
            }
            Answer::RateLimited(after) => {
                remember_backoff(config_dir, after);
                // A 429 is not the user's to act on, and they can do nothing
                // about it: the last numbers stand, marked with when they were
                // read. The refusal is heard only when there are none.
                stale_windows(config_dir)
                    .unwrap_or_else(|| request_failure(&stored, http_failure(429)))
            }
        },
        Err(failure) => recorded_instead(config_dir, failure),
    }
}

/// What asking Anthropic left: the numbers, or a refusal to read them yet.
enum Answer {
    Usage(ClaudeUsage),
    /// HTTP 429, with the wait its `Retry-After` named, if it named one.
    RateLimited(Option<Duration>),
}

/// Asks Anthropic for the plan windows with `token`: the windows, or why the
/// request did not produce them.
async fn fetch(sign_in: &Value, token: &str) -> Answer {
    let plan = sign_in
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("rustcode/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Answer::Usage(ClaudeUsage::unavailable(
                NoUsage::Transient,
                error.to_string(),
            ));
        }
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
            return Answer::Usage(ClaudeUsage::unavailable(
                NoUsage::Transient,
                "Anthropic did not answer.",
            ));
        }
    };
    let status = response.status();
    if status.as_u16() == 429 {
        // Read before the body: the wait is in the headers, and there is no
        // answer to parse.
        let header = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok());
        return Answer::RateLimited(retry_after(header, chrono::Utc::now()));
    }
    if !status.is_success() {
        return Answer::Usage(http_failure(status.as_u16()));
    }
    Answer::Usage(match response.json::<Value>().await {
        Ok(body) => ClaudeUsage::Windows {
            plan,
            windows: parse(&body),
            as_of: None,
        },
        Err(_) => {
            ClaudeUsage::unavailable(NoUsage::Transient, "Anthropic's answer could not be read.")
        }
    })
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
/// other status is worth one more try; a 429 is paced by the caller's backoff.
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
        429 => ClaudeUsage::unavailable(NoUsage::Transient, RATE_LIMITED),
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
        return crate::platform::command("/usr/bin/security")
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

/// Why a login has no numbers when its stored access token has lapsed: Claude
/// Code renews that token the next time it runs, and nothing here refreshes it
/// (a refresh would rotate it under Claude Code). The refresh token beside the
/// access token is still good, so the sign-in did not expire.
const EXPIRED_ACCOUNT: &str = "Run a Claude Code thread on this account to refresh its usage.";
const EXPIRED_DEFAULT: &str = "Run a Claude Code thread to refresh its usage.";

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
/// when `None`): every copy of it, since Claude Code keeps one in the Keychain
/// item for the directory and may keep `<config dir>/.credentials.json` too.
async fn credentials(
    config_dir: Option<&Path>,
    freshness: Freshness,
) -> Result<Stored, NoCredentials> {
    let file = || config_dir.map(Path::to_path_buf).or_else(default_dir);
    #[cfg(target_os = "macos")]
    {
        let key = config_dir.map(Path::to_path_buf);
        let mut copies = Vec::new();
        // Why the Keychain gave no copy, when it gave none. A refusal macOS
        // already gave is not repeated on its own: the file is read instead,
        // and only an explicit look asks the Keychain again.
        let mut miss = skip_keychain(freshness, keychain_blocked(&key)).then_some(
            NoCredentials::Unavailable(NoUsage::Denied, KEYCHAIN_REFUSED),
        );
        if miss.is_none() {
            let item = config_dir.map_or_else(
                || DEFAULT_KEYCHAIN_ITEM.to_owned(),
                |dir| keychain_item_name(dir),
            );
            let output = crate::platform::tokio_command("/usr/bin/security")
                .args(["find-generic-password", "-s", &item, "-w"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await;
            let failure = match output {
                Ok(output) => match keychain_outcome(
                    output.status.success(),
                    output.status.code(),
                    &output.stdout,
                ) {
                    Ok(value) => {
                        // macOS handed it over: nothing is blocked any more.
                        remember_keychain_denial(&key, false);
                        copies.push(value);
                        None
                    }
                    Err(miss) => Some(miss),
                },
                // `security` could not be run at all: not a sign-out, and not
                // a read worth repeating without the user asking.
                Err(_) => Some(NoCredentials::Unavailable(
                    NoUsage::Denied,
                    KEYCHAIN_REFUSED,
                )),
            };
            if let Some(refused) = failure {
                remember_keychain_denial(&key, is_denial(&refused));
                miss = Some(refused);
            }
        }
        // Both stores can hold a sign-in, and Claude Code writes back to
        // whichever it could open: the file is read even when the Keychain
        // answered, and the freshest token among them is the one used. A
        // refusal macOS gave is remembered: if no copy is usable, the user must
        // allow the Keychain, not read an expiry that retries it.
        let denied = miss.as_ref().is_some_and(is_denial);
        if let Some(dir) = file()
            && let Some(value) = read_credentials_file(dir).await
        {
            copies.push(value);
        }
        if copies.is_empty() {
            return Err(miss.unwrap_or(NoCredentials::None));
        }
        Ok(Stored {
            copies,
            keychain_denied: denied,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = freshness;
        match file() {
            Some(dir) => read_credentials_file(dir)
                .await
                .map(|value| Stored {
                    copies: vec![value],
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
        .or_else(|| crate::home::home_dir().map(|home| home.join(".claude")))
}

async fn read_credentials_file(dir: PathBuf) -> Option<Value> {
    let text = tokio::fs::read_to_string(dir.join(".credentials.json"))
        .await
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// The last windows RustCode read for one login, kept across engine restarts in
/// `claude-usage.json` in the config directory.
///
/// Claude Code renews an access token only when it runs, so a signed-in account
/// that has been idle for longer than the token's lifetime reads as expired.
/// Rather than leaving the user with only that, a read that finds no usable
/// token answers with what was recorded here, which the app shows "as of" when
/// it was read. Written only when a read answered, and never mistaken for a
/// fresh read (it carries `as_of`).
const HISTORY_FILE: &str = "claude-usage.json";

/// One login's last numbers, as written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecordedPlan {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    /// Unix seconds of the read that recorded them.
    #[serde(default)]
    read_at: i64,
    windows: Vec<RecordedWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecordedWindow {
    label: String,
    used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HistoryFile {
    /// One entry per login: the config directory, or `""` for the default one.
    #[serde(default)]
    logins: BTreeMap<String, RecordedPlan>,
}

/// A login's key in the history file: its config directory, and `""` for the
/// login that runs with no `CLAUDE_CONFIG_DIR`.
fn history_key(config_dir: Option<&Path>) -> String {
    config_dir
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn history_path(dir: &Path) -> PathBuf {
    dir.join(HISTORY_FILE)
}

fn read_history(dir: &Path) -> HistoryFile {
    std::fs::read_to_string(history_path(dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// What was recorded for the login in `config_dir` (the default login for
/// `None`), as the windows to answer with, `as_of` included. `None` when this
/// login was never read, or what was recorded holds no window of ours.
fn recorded_in(dir: &Path, config_dir: Option<&Path>) -> Option<ClaudeUsage> {
    let record = read_history(dir).logins.remove(&history_key(config_dir))?;
    let windows: Vec<LabelledWindow> = record
        .windows
        .iter()
        .filter_map(|window| {
            Some(LabelledWindow {
                label: label_for(&window.label)?,
                window: RateLimitWindow {
                    used_percent: window.used_percent,
                    window_minutes: window.window_minutes,
                    resets_at: window.resets_at,
                },
            })
        })
        .collect();
    (!windows.is_empty()).then_some(ClaudeUsage::Windows {
        plan: record.plan,
        windows,
        as_of: Some(record.read_at),
    })
}

/// Keeps the numbers of a read that answered, for the next time this login's
/// stored access token has lapsed. A read that is itself recorded numbers is
/// not written again.
fn record_in(dir: &Path, config_dir: Option<&Path>, usage: &ClaudeUsage) {
    let ClaudeUsage::Windows {
        plan,
        windows,
        as_of: None,
    } = usage
    else {
        return;
    };
    if windows.is_empty() {
        return;
    }
    let record = RecordedPlan {
        plan: plan.clone(),
        read_at: chrono::Utc::now().timestamp(),
        windows: windows
            .iter()
            .map(|window| RecordedWindow {
                label: window.label.to_owned(),
                used_percent: window.window.used_percent,
                window_minutes: window.window.window_minutes,
                resets_at: window.window.resets_at,
            })
            .collect(),
    };
    let mut history = read_history(dir);
    let key = history_key(config_dir);
    if history.logins.get(&key) == Some(&record) {
        return;
    }
    history.logins.insert(key, record);
    if let Ok(text) = serde_json::to_string(&history) {
        let _ = std::fs::create_dir_all(dir);
        let _ = crate::worktrees::write_atomic(&history_path(dir), text.as_bytes());
    }
}

/// The numbers recorded for the login in `config_dir`, if any.
fn last_recorded(config_dir: Option<&Path>) -> Option<ClaudeUsage> {
    recorded_in(&crate::harness::config::config_dir().ok()?, config_dir)
}

/// Keeps a read's numbers, when there is a config directory to keep them in.
fn record_read(config_dir: Option<&Path>, usage: &ClaudeUsage) {
    if let Ok(dir) = crate::harness::config::config_dir() {
        record_in(&dir, config_dir, usage);
    }
}

/// What to answer when no stored sign-in is usable. Only a lapsed access token
/// is answered from the history: it is the account being idle, not the user
/// signing out, and Claude Code renews the token the next time it runs. Every
/// other failure keeps its own reason, which the app acts on.
fn recorded_instead(config_dir: Option<&Path>, failure: ClaudeUsage) -> ClaudeUsage {
    with_recorded(last_recorded(config_dir), failure)
}

/// [`recorded_instead`] with the recorded numbers passed in, so the choice is
/// tested without a config directory.
fn with_recorded(last: Option<ClaudeUsage>, failure: ClaudeUsage) -> ClaudeUsage {
    let lapsed = matches!(
        failure,
        ClaudeUsage::Unavailable {
            reason: NoUsage::Expired,
            ..
        }
    );
    if !lapsed {
        return failure;
    }
    last.unwrap_or(failure)
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

/// The plan windows Anthropic reports: the JSON key, the label the app shows,
/// and the window's length in minutes.
const WINDOWS: [(&str, &str, u64); 4] = [
    ("five_hour", "5-hour", 300),
    ("seven_day", "Weekly", 10_080),
    ("seven_day_opus", "Weekly · Opus", 10_080),
    ("seven_day_sonnet", "Weekly · Sonnet", 10_080),
];

/// The label of a window whose name was recorded, if it is one of ours.
fn label_for(name: &str) -> Option<&'static str> {
    WINDOWS
        .iter()
        .find(|(_, label, _)| *label == name)
        .map(|(_, label, _)| *label)
}

/// `{five_hour, seven_day, seven_day_opus, seven_day_sonnet}` windows of
/// `{utilization (percent), resets_at (RFC 3339)}`.
fn parse(body: &Value) -> Vec<LabelledWindow> {
    WINDOWS
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
                as_of: None,
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
            as_of: None,
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

    /// The reason `usable_sign_in` refuses the copies it was given with, for
    /// the tests below.
    fn refusal(stored: &Stored, now: i64) -> NoUsage {
        match usable_sign_in(stored, now, true) {
            Err(ClaudeUsage::Unavailable { reason, .. }) => reason,
            other => panic!("{other:?}"),
        }
    }

    /// One login's stored copies, the Keychain item first, as `credentials`
    /// hands them over.
    fn stored(copies: Vec<Value>) -> Stored {
        Stored {
            copies,
            keychain_denied: false,
        }
    }

    /// A sign-in with `expiresAt`, as Claude Code writes it.
    fn sign_in_at(token: &str, expires: i64) -> Value {
        json!({"claudeAiOauth": {"accessToken": token, "expiresAt": expires, "subscriptionType": "max"}})
    }

    /// A denied Keychain whose credentials-file fallback holds an expired
    /// sign-in must stay a denial. Reported as an expiry, the app would retry
    /// every poll, and every retry would ask macOS to hand the item over again.
    #[test]
    fn a_denied_keychain_stays_denied_when_the_file_is_no_use() {
        let now = 1_000_000;
        let expired = Stored {
            copies: vec![sign_in_at("t", now - 1)],
            keychain_denied: true,
        };
        assert_eq!(refusal(&expired, now), NoUsage::Denied);
        // The copy is the Keychain one, which says how to allow the read.
        assert!(matches!(
            usable_sign_in(&expired, now, true),
            Err(ClaudeUsage::Unavailable { message, .. }) if message == KEYCHAIN_REFUSED
        ));
        // A file with no usable token at all is no stand-in either.
        let unreadable = Stored {
            copies: vec![json!({})],
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
            copies: vec![sign_in_at("t", now + 3_600_000)],
            keychain_denied: true,
        };
        assert!(matches!(usable_sign_in(&fresh, now, true), Ok((_, "t"))));
        let expired = stored(vec![sign_in_at("t", now - 1)]);
        assert_eq!(refusal(&expired, now), NoUsage::Expired);
        let unreadable = stored(vec![json!({})]);
        assert_eq!(refusal(&unreadable, now), NoUsage::Transient);
    }

    /// Claude Code writes back to whichever store it could open, so one copy
    /// can lag the other. A lapsed Keychain item must not hide the renewed
    /// token in the config directory's credentials file (#2845).
    #[test]
    fn the_freshest_stored_copy_is_the_one_used() {
        let now = 1_000_000;
        // The Keychain item was last written before this account ran again; the
        // file has the token Claude Code renewed since.
        let account = stored(vec![
            sign_in_at("stale", now - 1),
            sign_in_at("renewed", now + 3_600_000),
        ]);
        assert!(matches!(
            usable_sign_in(&account, now, true),
            Ok((_, "renewed"))
        ));
        // Two usable copies: the one that lives longer is used, so a copy
        // written a moment ago does not beat the account's own renewed one.
        let both = stored(vec![
            sign_in_at("short", now + 60_000),
            sign_in_at("long", now + 7_200_000),
        ]);
        assert!(matches!(usable_sign_in(&both, now, true), Ok((_, "long"))));
        // A copy that does not say when it expires counts as the oldest.
        let unknown = stored(vec![
            json!({"claudeAiOauth": {"accessToken": "unknown"}}),
            sign_in_at("dated", now + 3_600_000),
        ]);
        assert!(matches!(
            usable_sign_in(&unknown, now, true),
            Ok((_, "dated"))
        ));
        // Neither copy usable is still the expiry, not a sign-out.
        assert_eq!(
            refusal(&stored(vec![sign_in_at("a", now - 1), json!({})]), now),
            NoUsage::Expired
        );
    }

    /// A lapsed access token is the account being idle, not the user signing
    /// out: its message asks for a thread on that account, which is what
    /// renews the token, instead of claiming the sign-in has expired.
    #[test]
    fn a_lapsed_token_asks_for_a_thread_on_that_account() {
        let account = failure_without_sign_in(false, true, true);
        assert!(matches!(
            &account,
            ClaudeUsage::Unavailable { reason: NoUsage::Expired, message }
                if message == EXPIRED_ACCOUNT && message.contains("thread on this account")
        ));
        let default = failure_without_sign_in(false, true, false);
        assert!(matches!(
            &default,
            ClaudeUsage::Unavailable { message, .. } if message == EXPIRED_DEFAULT
        ));
        // A refusal macOS gave still outranks it, and nothing usable at all is
        // still the retryable unreadable sign-in.
        assert!(matches!(
            failure_without_sign_in(true, true, true),
            ClaudeUsage::Unavailable {
                reason: NoUsage::Denied,
                ..
            }
        ));
        assert!(matches!(
            failure_without_sign_in(false, false, true),
            ClaudeUsage::Unavailable {
                reason: NoUsage::Transient,
                ..
            }
        ));
    }

    /// The numbers a read recorded are what a lapsed token answers with, marked
    /// with when they were read; every other failure keeps its reason.
    #[test]
    fn a_lapsed_token_answers_with_the_recorded_numbers() {
        let recorded = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: vec![labelled("5-hour", 12.0, Some(300), Some(1_793_660_400))],
            as_of: Some(1_762_500_000),
        };
        let expired = ClaudeUsage::unavailable(NoUsage::Expired, EXPIRED_ACCOUNT);
        assert_eq!(
            with_recorded(Some(recorded.clone()), expired.clone()),
            recorded
        );
        // Nothing recorded yet: the expiry stands, and the app can act on it.
        assert_eq!(with_recorded(None, expired.clone()), expired);
        // A refusal, a 403 or a denied Keychain is not answered with old
        // numbers: their reason is what the app has to act on.
        for failure in [
            ClaudeUsage::unavailable(NoUsage::Refused, "refused"),
            ClaudeUsage::unavailable(NoUsage::Forbidden, "forbidden"),
            ClaudeUsage::unavailable(NoUsage::Denied, KEYCHAIN_REFUSED),
            ClaudeUsage::unavailable(NoUsage::Transient, "transient"),
            ClaudeUsage::NotSignedIn,
        ] {
            assert_eq!(
                with_recorded(Some(recorded.clone()), failure.clone()),
                failure
            );
        }
    }

    fn labelled(
        label: &'static str,
        used_percent: f64,
        window_minutes: Option<u64>,
        resets_at: Option<i64>,
    ) -> LabelledWindow {
        LabelledWindow {
            label,
            window: RateLimitWindow {
                used_percent,
                window_minutes,
                resets_at,
            },
        }
    }

    /// A read that answered is kept per login, and read back as numbers marked
    /// with when they were read — never as a fresh read.
    #[test]
    fn a_read_that_answered_is_kept_for_its_own_login() {
        let dir = tempfile::tempdir().unwrap();
        let work = Some(Path::new("/tmp/claude-usage-work"));
        let home = Some(Path::new("/tmp/claude-usage-home"));
        // Nothing recorded yet: no answer to fall back on.
        assert_eq!(recorded_in(dir.path(), work), None);
        let answered = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: vec![labelled("5-hour", 12.0, Some(300), Some(1_793_660_400))],
            as_of: None,
        };
        record_in(dir.path(), work, &answered);
        let before = chrono::Utc::now().timestamp();
        let recorded = recorded_in(dir.path(), work).expect("the read was kept");
        let after = chrono::Utc::now().timestamp();
        let ClaudeUsage::Windows {
            plan,
            windows,
            as_of,
        } = recorded
        else {
            panic!("{recorded:?}");
        };
        assert_eq!(plan.as_deref(), Some("max"));
        assert_eq!(
            windows,
            vec![labelled("5-hour", 12.0, Some(300), Some(1_793_660_400))]
        );
        let as_of = as_of.expect("recorded numbers say when they were read");
        assert!((before..=after).contains(&as_of), "{as_of}");
        // Another login has its own entry, and the default login its own.
        assert_eq!(recorded_in(dir.path(), home), None);
        assert_eq!(recorded_in(dir.path(), None), None);
        record_in(
            dir.path(),
            None,
            &ClaudeUsage::Windows {
                plan: None,
                windows: vec![labelled("Weekly", 41.5, Some(10_080), None)],
                as_of: None,
            },
        );
        assert!(matches!(
            recorded_in(dir.path(), None),
            Some(ClaudeUsage::Windows { windows, .. }) if windows[0].window.used_percent == 41.5
        ));
        // Recording numbers that are themselves recorded ones changes nothing.
        let stored_before = std::fs::read_to_string(history_path(dir.path())).unwrap();
        record_in(dir.path(), work, &recorded_in(dir.path(), work).unwrap());
        assert_eq!(
            std::fs::read_to_string(history_path(dir.path())).unwrap(),
            stored_before
        );
    }

    /// A record whose labels this build no longer knows, or whose windows are
    /// all unknown, is no answer at all — never an empty row pretending to be
    /// numbers.
    #[test]
    fn a_record_without_a_known_window_is_no_answer() {
        let dir = tempfile::tempdir().unwrap();
        let key = Some(Path::new("/tmp/claude-usage-odd"));
        std::fs::write(
            history_path(dir.path()),
            json!({
                "logins": {
                    "/tmp/claude-usage-odd": {
                        "plan": "max",
                        "read_at": 1_762_500_000,
                        "windows": [
                            {"label": "Weekly · Haiku", "used_percent": 3.0},
                            {"label": "Weekly", "used_percent": 41.5, "window_minutes": 10_080},
                        ],
                    },
                },
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            recorded_in(dir.path(), key),
            Some(ClaudeUsage::Windows {
                plan: Some("max".into()),
                windows: vec![labelled("Weekly", 41.5, Some(10_080), None)],
                as_of: Some(1_762_500_000),
            })
        );
        std::fs::write(
            history_path(dir.path()),
            json!({"logins": {"/tmp/claude-usage-odd": {"read_at": 1, "windows": [{"label": "nope", "used_percent": 1.0}]}}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(recorded_in(dir.path(), key), None);
        // A file that is not ours at all is no answer either.
        std::fs::write(history_path(dir.path()), "not json").unwrap();
        assert_eq!(recorded_in(dir.path(), key), None);
    }

    /// A fallback whose request failed — a 401, a 500, no answer at all — must
    /// still report the Keychain refusal: `refused` or `transient` would send
    /// the app back to the Keychain on every poll, and each read would prompt
    /// the user again.
    #[test]
    fn a_failed_request_on_a_denied_fallback_stays_denied() {
        let stored = Stored {
            copies: vec![json!({})],
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
            copies: vec![json!({})],
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
            copies: vec![json!({})],
            keychain_denied: true,
        };
        let answered = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: Vec::new(),
            as_of: None,
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
                copies: vec![json!({})],
                keychain_denied: true,
            };
            let answered = ClaudeUsage::Windows {
                plan: Some("max".into()),
                windows: Vec::new(),
                as_of: None,
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

    /// The numbers move slowly and Anthropic rate-limits them, so they are kept
    /// for five minutes: repeated looks at a view — and the composer's meter
    /// asking for the same account — make one request per interval. A failure is
    /// kept for a minute instead, since a retry is what stops it failing.
    #[test]
    fn numbers_are_cached_for_five_minutes_and_a_failure_for_one() {
        let windows = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: vec![labelled("5-hour", 12.0, Some(300), None)],
            as_of: None,
        };
        assert_eq!(WINDOWS_TTL, Duration::from_secs(300));
        assert_eq!(ttl(&windows), WINDOWS_TTL);
        assert!(fresh_enough(&windows, WINDOWS_TTL - Duration::from_secs(1)));
        assert!(!fresh_enough(&windows, WINDOWS_TTL));
        let failed = ClaudeUsage::unavailable(NoUsage::Transient, "transient");
        assert_eq!(ttl(&failed), FRESH);
        assert!(fresh_enough(&failed, Duration::from_secs(59)));
        assert!(!fresh_enough(&failed, FRESH));
        // A login that answered is served from the cache, not read again.
        let key = Some(PathBuf::from("/tmp/claude-ttl-windows"));
        remember(key.clone(), Instant::now(), &windows);
        assert_eq!(cached(&key, Freshness::Cached), Some(windows));
        // An explicit look still reads again, however fresh the cache is.
        assert_eq!(cached(&key, Freshness::Now), None);
    }

    /// A `Retry-After` names the wait in seconds or as an HTTP date. Anything
    /// else is no answer, and the wait doubles on its own instead.
    #[test]
    fn a_429_says_how_long_to_wait() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-02T23:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            retry_after(Some("120"), now),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            retry_after(Some("Mon, 02 Nov 2026 23:01:30 GMT"), now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            retry_after(Some("Mon, 02 Nov 2026 23:01:30 +0000"), now),
            Some(Duration::from_secs(90))
        );
        // A date that has already passed names no wait, which the ceiling's
        // floor then covers.
        assert_eq!(
            retry_after(Some("Mon, 02 Nov 2026 22:59:00 GMT"), now),
            Some(Duration::ZERO)
        );
        for header in [Some("soon"), Some(""), None] {
            assert_eq!(retry_after(header, now), None, "{header:?}");
        }
    }

    /// Without a `Retry-After` each refusal doubles the wait — a minute, two,
    /// four — up to half an hour, and a named wait is honoured within the same
    /// ceiling.
    #[test]
    fn a_429_backs_off_further_each_time() {
        assert_eq!(backoff_wait(None, 0), Duration::from_secs(60));
        assert_eq!(backoff_wait(None, 1), Duration::from_secs(120));
        assert_eq!(backoff_wait(None, 2), Duration::from_secs(240));
        assert_eq!(backoff_wait(None, 3), Duration::from_secs(480));
        assert_eq!(backoff_wait(None, 4), Duration::from_secs(960));
        assert_eq!(backoff_wait(None, 5), BACKOFF_MAX);
        assert_eq!(backoff_wait(None, 40), BACKOFF_MAX);
        assert_eq!(BACKOFF_MAX, Duration::from_secs(30 * 60));
        assert_eq!(
            backoff_wait(Some(Duration::from_secs(600)), 5),
            Duration::from_secs(600)
        );
        // A header that names hours is a long pause, not a dead login.
        assert_eq!(
            backoff_wait(Some(Duration::from_secs(6 * 3600)), 0),
            BACKOFF_MAX
        );
        assert_eq!(backoff_wait(Some(Duration::from_secs(1)), 0), BACKOFF_MIN);
    }

    /// The wait belongs to the login that was refused: another account is read
    /// as usual, a second refusal lengthens the wait instead of restarting it,
    /// and an answer — or the wait running out — lets the login ask again.
    #[test]
    fn a_429_quiets_one_login_and_ends_with_its_wait() {
        let key = Path::new("/tmp/claude-429-quiet");
        let quiet = Some(key.to_path_buf());
        let other = Some(PathBuf::from("/tmp/claude-429-other"));
        forget_backoff(Some(key));
        assert!(!backing_off(&quiet));
        remember_backoff(Some(key), Some(Duration::from_secs(600)));
        assert!(backing_off(&quiet));
        assert!(!backing_off(&other), "one login's wait is not another's");
        // A second refusal doubles the wait and keeps the count.
        remember_backoff(Some(key), None);
        let entry = BACKOFF
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .get(&quiet)
            .copied()
            .unwrap();
        assert_eq!(entry.attempt, 2);
        assert!(
            entry.until > Instant::now() + Duration::from_secs(100),
            "{entry:?}"
        );
        // A wait that has run out asks again, and is not kept.
        BACKOFF.lock().unwrap().as_mut().unwrap().insert(
            quiet.clone(),
            Backoff {
                until: Instant::now() - Duration::from_secs(1),
                attempt: 3,
            },
        );
        assert!(!backing_off(&quiet));
        assert!(!backing_off(&quiet), "an ended wait is dropped");
        // An answer ends it too, so the next refusal starts a minute out.
        remember_backoff(Some(key), Some(Duration::from_secs(600)));
        forget_backoff(Some(key));
        assert!(!backing_off(&quiet));
        remember_backoff(Some(key), None);
        let entry = BACKOFF
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .get(&quiet)
            .copied()
            .unwrap();
        assert_eq!(entry.attempt, 1);
        forget_backoff(Some(key));
    }

    /// A 429 is answered with the last numbers, marked with when they were read,
    /// so the app says how old they are instead of showing an error the user
    /// could do nothing about. Only a login with no numbers hears the refusal.
    #[test]
    fn a_429_keeps_the_numbers_and_their_age() {
        let key = Path::new("/tmp/claude-429-cached");
        let windows = ClaudeUsage::Windows {
            plan: Some("max".into()),
            windows: vec![labelled("5-hour", 12.0, Some(300), Some(1_793_660_400))],
            as_of: None,
        };
        let before = chrono::Utc::now().timestamp();
        remember(Some(key.to_path_buf()), Instant::now(), &windows);
        let kept = stale_windows(Some(key)).expect("the numbers stay");
        let ClaudeUsage::Windows {
            plan,
            windows,
            as_of,
        } = kept.clone()
        else {
            panic!("{kept:?}");
        };
        assert_eq!(plan.as_deref(), Some("max"));
        assert_eq!(
            windows,
            vec![labelled("5-hour", 12.0, Some(300), Some(1_793_660_400))]
        );
        let as_of = as_of.expect("served numbers say when they were read");
        assert!((before..=chrono::Utc::now().timestamp()).contains(&as_of));
        // Keeping them again must not make them look newer than they are: a
        // second 429 shows the same age, not this minute's.
        remember(Some(key.to_path_buf()), Instant::now(), &kept);
        assert_eq!(stale_windows(Some(key)), Some(kept));
        // A login with nothing cached hears the refusal, since there is nothing
        // to show instead. A cached failure, or a row with no window in it, is
        // no stand-in for numbers either.
        assert_eq!(
            stale_windows(Some(Path::new("/tmp/claude-429-empty"))),
            None
        );
        let failed = ClaudeUsage::unavailable(NoUsage::Transient, RATE_LIMITED);
        remember(
            Some(PathBuf::from("/tmp/claude-429-failed")),
            Instant::now(),
            &failed,
        );
        assert_eq!(
            stale_windows(Some(Path::new("/tmp/claude-429-failed"))),
            None
        );
        let blank = ClaudeUsage::Windows {
            plan: None,
            windows: Vec::new(),
            as_of: None,
        };
        remember(
            Some(PathBuf::from("/tmp/claude-429-blank")),
            Instant::now(),
            &blank,
        );
        assert_eq!(
            stale_windows(Some(Path::new("/tmp/claude-429-blank"))),
            None
        );
        // The refusal it falls back to says what RustCode is doing about it, and
        // keeps the reason the app retries on.
        match http_failure(429) {
            ClaudeUsage::Unavailable { reason, message } => {
                assert_eq!(reason, NoUsage::Transient);
                assert_eq!(message, RATE_LIMITED);
            }
            other => panic!("{other:?}"),
        }
    }
}
