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
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

static CACHE: Mutex<Option<(Instant, ClaudeUsage)>> = Mutex::new(None);

/// The account's plan windows, from the cache while fresh.
pub(crate) async fn usage() -> ClaudeUsage {
    if let Some((at, cached)) = CACHE.lock().ok().and_then(|cache| cache.clone())
        && at.elapsed() < FRESH
    {
        return cached;
    }
    let read = read().await;
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), read.clone()));
    }
    read
}

async fn read() -> ClaudeUsage {
    let Some(credentials) = credentials().await else {
        return ClaudeUsage::NotSignedIn;
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

/// Claude Code's stored sign-in, as JSON.
async fn credentials() -> Option<Value> {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .await
            .ok()?;
        if output.status.success()
            && let Ok(value) = serde_json::from_slice::<Value>(&output.stdout)
        {
            return Some(value);
        }
    }
    let dir = crate::shell_env::env_var("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".claude"))
        })?;
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
    fn an_expired_token_is_not_used() {
        let now = 1_000_000;
        let stored = |expires: i64| json!({"claudeAiOauth": {"accessToken": "t", "expiresAt": expires, "subscriptionType": "max"}});
        assert_eq!(access_token(&stored(now + 3_600_000), now), Some("t"));
        assert_eq!(access_token(&stored(now + 30_000), now), None);
        assert_eq!(access_token(&json!({}), now), None);
    }
}
