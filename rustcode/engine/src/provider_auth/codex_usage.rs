//! ChatGPT plan usage from the user's own Codex CLI (#2545).
//!
//! A ChatGPT sign-in token is minted for `api.openai.com/v1`, which reports
//! no plan windows: no limit headers, no stream event, and no usage endpoint
//! it may read. A signed-in `codex` CLI can: `codex app-server` answers
//! `account/rateLimits/read` with the 5-hour and weekly windows of that
//! ChatGPT account. This asks it, at most once a minute, and only reads.

use super::rate_limits::{ProviderRateLimits, RateLimitWindow};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROGRAM_ENV: &str = "RUSTCODE_CODEX_CLI";
/// How long an answer is reused.
const FRESH: Duration = Duration::from_secs(60);
/// How long a failure is reused: short, so signing in or installing the CLI
/// shows up on the next refresh instead of ten minutes later (#2875).
const RETRY: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(10);
const READ_ID: u64 = 2;

/// Why the Codex CLI gave no plan windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexUsageError {
    /// No `codex` on the login shell's PATH (or at `RUSTCODE_CODEX_CLI`).
    NotInstalled,
    /// The CLI answered but is not signed in to ChatGPT.
    SignedOut,
    /// Anything else: the error, without tokens.
    Unavailable(String),
}

type Read = Result<ProviderRateLimits, CodexUsageError>;

static CACHE: Mutex<Option<(Instant, Read)>> = Mutex::new(None);

/// The `codex` to run: `RUSTCODE_CODEX_CLI`, else `codex` resolved on the
/// login shell's PATH like harness detection does. An app opened from the
/// Dock has only `/usr/bin:/bin` and would not find a Homebrew `codex`.
fn program() -> Option<std::path::PathBuf> {
    let program = crate::shell_env::env_var(PROGRAM_ENV)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "codex".into());
    crate::harness::detect::resolve_program(&program)
}

/// The plan windows the Codex CLI reports, from the cache while fresh.
pub(crate) async fn rate_limits() -> Option<ProviderRateLimits> {
    read_cached().await.ok()
}

/// Like [`rate_limits`], with the reason when there are none.
pub(crate) async fn read_cached() -> Read {
    if let Some((at, cached)) = CACHE.lock().ok().and_then(|cache| cache.clone()) {
        let ttl = if cached.is_ok() { FRESH } else { RETRY };
        if at.elapsed() < ttl {
            return cached;
        }
    }
    let read = match tokio::time::timeout(TIMEOUT, read()).await {
        Ok(read) => read,
        Err(_) => Err(CodexUsageError::Unavailable(
            "Codex CLI did not answer within 10 seconds".into(),
        )),
    };
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), read.clone()));
    }
    read
}

async fn read() -> Read {
    let unavailable = |what: &str| CodexUsageError::Unavailable(what.to_string());
    let program = program().ok_or(CodexUsageError::NotInstalled)?;
    let mut child = tokio::process::Command::new(&program)
        .arg("app-server")
        .env("PATH", crate::harness::detect::search_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            CodexUsageError::Unavailable(format!("could not start Codex CLI: {error}"))
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| unavailable("no stdin"))?;
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"clientInfo": {"name": "rustcode", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"jsonrpc": "2.0", "method": "initialized"}),
        json!({"jsonrpc": "2.0", "id": READ_ID, "method": "account/rateLimits/read", "params": {}}),
    ];
    for request in requests {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|_| unavailable("Codex CLI closed its input"))?;
    }
    stdin
        .flush()
        .await
        .map_err(|_| unavailable("Codex CLI closed its input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(READ_ID) {
            return answer(&message);
        }
    }
    Err(unavailable("Codex CLI exited without an answer"))
}

/// The `account/rateLimits/read` reply: windows, a sign-in error, or another.
fn answer(message: &Value) -> Read {
    if let Some(error) = message.get("error") {
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let lower = text.to_ascii_lowercase();
        return Err(
            if [
                "auth",
                "login",
                "log in",
                "sign in",
                "signed in",
                "not logged",
            ]
            .iter()
            .any(|needle| lower.contains(needle))
            {
                CodexUsageError::SignedOut
            } else {
                CodexUsageError::Unavailable(text.chars().take(200).collect())
            },
        );
    }
    message
        .get("result")
        .and_then(|result| result.get("rateLimits"))
        .and_then(parse)
        .ok_or(CodexUsageError::SignedOut)
}

/// `{primary, secondary}` windows of `{usedPercent, windowDurationMins,
/// resetsAt}` (Unix seconds).
fn parse(limits: &Value) -> Option<ProviderRateLimits> {
    let window = |key: &str| {
        let window = limits.get(key)?;
        Some(RateLimitWindow {
            used_percent: window.get("usedPercent")?.as_f64()?.clamp(0.0, 100.0),
            window_minutes: window.get("windowDurationMins").and_then(Value::as_u64),
            resets_at: window.get("resetsAt").and_then(Value::as_i64),
        })
    };
    let limits = ProviderRateLimits {
        primary: window("primary"),
        secondary: window("secondary"),
    };
    (limits.primary.is_some() || limits.secondary.is_some()).then_some(limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_app_servers_windows() {
        let limits = parse(&json!({
            "limitId": "codex",
            "primary": {"usedPercent": 5, "windowDurationMins": 300, "resetsAt": 1_793_188_931},
            "secondary": {"usedPercent": 1, "windowDurationMins": 10080, "resetsAt": 1_793_775_731},
            "planType": "plus"
        }))
        .unwrap();
        let primary = limits.primary.unwrap();
        assert_eq!(primary.used_percent, 5.0);
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.resets_at, Some(1_793_188_931));
        assert_eq!(limits.secondary.unwrap().window_minutes, Some(10080));
        assert!(parse(&json!({"primary": null})).is_none());
    }

    #[test]
    fn tells_a_sign_in_error_from_another() {
        let ok = answer(&json!({"id": 2, "result": {"rateLimits": {
            "primary": {"usedPercent": 51, "windowDurationMins": 10080, "resetsAt": 1}
        }}}));
        assert_eq!(ok.unwrap().primary.unwrap().used_percent, 51.0);
        assert_eq!(
            answer(&json!({"id": 2, "error": {"code": -32600, "message": "Not logged in"}})),
            Err(CodexUsageError::SignedOut)
        );
        assert_eq!(
            answer(&json!({"id": 2, "result": {"rateLimits": null}})),
            Err(CodexUsageError::SignedOut)
        );
        assert_eq!(
            answer(&json!({"id": 2, "error": {"code": -32601, "message": "method not found"}})),
            Err(CodexUsageError::Unavailable("method not found".into()))
        );
    }
}
