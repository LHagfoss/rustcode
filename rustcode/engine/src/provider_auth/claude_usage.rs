//! Claude plan usage for the Claude Code harness (#2845).
//!
//! A signed-in Claude Code keeps its OAuth sign-in in the macOS Keychain
//! (`Claude Code-credentials`) or, elsewhere, `~/.claude/.credentials.json`.
//! Its access token can read the account's plan windows from
//! `api.anthropic.com/api/oauth/usage` (5-hour, weekly, weekly per model).
//! This only reads: it never refreshes the token (a refresh would rotate it
//! under Claude Code, as #2765 did for OpenAI), so an expired sign-in is
//! reported and left to Claude Code. Asked at most once a minute.

use super::rate_limits::RateLimitWindow;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
    /// Signed in, but the numbers could not be read; the text says why.
    Unavailable(String),
}

/// One entry per config directory: `None` is the default login, `Some` a
/// named account's own `CLAUDE_CONFIG_DIR`, whose numbers are its own.
static CACHE: Mutex<Option<HashMap<Option<PathBuf>, (Instant, ClaudeUsage)>>> = Mutex::new(None);

/// The plan windows of the login in `config_dir` (the default login when
/// `None`), from the cache while fresh. Two accounts never share numbers.
pub(crate) async fn usage(config_dir: Option<&Path>) -> ClaudeUsage {
    let key = config_dir.map(Path::to_path_buf);
    if let Some(cached) = cached(&key) {
        return cached;
    }
    let read = read(config_dir).await;
    remember(key, &read);
    read
}

/// The fresh cached answer for one config directory, if there is one.
fn cached(key: &Option<PathBuf>) -> Option<ClaudeUsage> {
    let cache = CACHE.lock().ok()?;
    let (at, usage) = cache.as_ref()?.get(key)?;
    (at.elapsed() < FRESH).then(|| usage.clone())
}

fn remember(key: Option<PathBuf>, usage: &ClaudeUsage) {
    if let Ok(mut cache) = CACHE.lock() {
        cache
            .get_or_insert_with(HashMap::new)
            .insert(key, (Instant::now(), usage.clone()));
    }
}

async fn read(config_dir: Option<&Path>) -> ClaudeUsage {
    let credentials = match credentials(config_dir).await {
        Ok(credentials) => credentials,
        Err(NoCredentials::None) => return ClaudeUsage::NotSignedIn,
        Err(NoCredentials::Unavailable(error)) => {
            return ClaudeUsage::Unavailable(error.to_owned());
        }
    };
    let Some(token) = access_token(&credentials, chrono::Utc::now().timestamp_millis()) else {
        return ClaudeUsage::Unavailable(
            "Claude Code's sign-in has expired. Open Claude Code to renew it.".into(),
        );
    };
    let plan = credentials
        .pointer("/claudeAiOauth/subscriptionType")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let client = match reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("rustcode/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(client) => client,
        Err(error) => return ClaudeUsage::Unavailable(error.to_string()),
    };
    let response = match client
        .get(USAGE_URL)
        .bearer_auth(token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return ClaudeUsage::Unavailable("Anthropic did not answer.".into()),
    };
    let status = response.status();
    if !status.is_success() {
        return ClaudeUsage::Unavailable(if status.as_u16() == 401 {
            "Claude Code's sign-in was refused. Open Claude Code to sign in again.".into()
        } else {
            format!("Anthropic answered HTTP {}.", status.as_u16())
        });
    }
    match response.json::<Value>().await {
        Ok(body) => ClaudeUsage::Windows {
            plan,
            windows: parse(&body),
        },
        Err(_) => ClaudeUsage::Unavailable("Anthropic's answer could not be read.".into()),
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
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                &keychain_item_name(config_dir),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = config_dir;
        false
    }
}

/// Why a login's stored sign-in was not handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NoCredentials {
    /// There is none: no Keychain item and no credentials file either.
    None,
    /// There may be one, but it could not be read; the text says why. Never
    /// reported as signed out: a denied Keychain prompt or a locked Keychain is
    /// not the user signing out.
    Unavailable(&'static str),
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
        _ => NoCredentials::Unavailable(KEYCHAIN_REFUSED),
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
        serde_json::from_slice(stdout).map_err(|_| NoCredentials::Unavailable(KEYCHAIN_UNREADABLE))
    } else {
        Err(keychain_miss(code))
    }
}

/// Claude Code's stored sign-in, as JSON, for `config_dir` (the default login
/// when `None`).
async fn credentials(config_dir: Option<&Path>) -> Result<Value, NoCredentials> {
    #[cfg(target_os = "macos")]
    {
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
            .map_err(|_| NoCredentials::Unavailable(KEYCHAIN_REFUSED))?;
        let miss = match keychain_outcome(
            output.status.success(),
            output.status.code(),
            &output.stdout,
        ) {
            Ok(value) => return Ok(value),
            Err(miss) => miss,
        };
        // The credentials file may still hold the sign-in (and does elsewhere),
        // so it has its chance before a failed lookup is called a sign-out.
        let file = config_dir.map(Path::to_path_buf).or_else(default_dir);
        if let Some(dir) = file
            && let Some(value) = read_credentials_file(dir).await
        {
            return Ok(value);
        }
        Err(miss)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let file = config_dir.map(Path::to_path_buf).or_else(default_dir);
        match file {
            Some(dir) => read_credentials_file(dir).await.ok_or(NoCredentials::None),
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

/// The access token while it is valid for at least another minute.
fn access_token(credentials: &Value, now_ms: i64) -> Option<&str> {
    let oauth = credentials.get("claudeAiOauth")?;
    let token = oauth.get("accessToken")?.as_str()?;
    let expires = oauth.get("expiresAt").and_then(Value::as_i64);
    (!token.is_empty() && expires.is_none_or(|at| at > now_ms + 60_000)).then_some(token)
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
        remember(
            first.clone(),
            &ClaudeUsage::Windows {
                plan: Some("max".into()),
                windows: Vec::new(),
            },
        );
        remember(second.clone(), &ClaudeUsage::NotSignedIn);
        assert!(matches!(cached(&first), Some(ClaudeUsage::Windows { .. })));
        assert_eq!(cached(&second), Some(ClaudeUsage::NotSignedIn));
        // The default login is its own entry too.
        assert_eq!(cached(&None), None);
        remember(None, &ClaudeUsage::Unavailable("no".into()));
        assert_eq!(cached(&None), Some(ClaudeUsage::Unavailable("no".into())));
        assert!(
            matches!(cached(&first), Some(ClaudeUsage::Windows { .. })),
            "one account's answer never replaces another's"
        );
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
                NoCredentials::Unavailable(KEYCHAIN_REFUSED),
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
        // Printed, but no sign-in in it: signed in and unreadable, not signed out.
        assert_eq!(
            keychain_outcome(true, Some(0), b"not json"),
            Err(NoCredentials::Unavailable(KEYCHAIN_UNREADABLE))
        );
        assert_eq!(
            keychain_outcome(false, Some(44), b""),
            Err(NoCredentials::None)
        );
        assert_eq!(
            keychain_outcome(false, Some(128), b""),
            Err(NoCredentials::Unavailable(KEYCHAIN_REFUSED))
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
    fn an_expired_token_is_not_used() {
        let now = 1_000_000;
        let stored = |expires: i64| json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": expires, "subscriptionType": "max"}});
        assert_eq!(access_token(&stored(now + 3_600_000), now), Some("t"));
        assert_eq!(access_token(&stored(now + 30_000), now), None);
        assert_eq!(access_token(&json!({}), now), None);
    }
}
