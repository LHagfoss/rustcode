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
/// How long an answer, or the lack of one, is reused.
const FRESH: Duration = Duration::from_secs(60);
/// With no `codex`, or one signed out, ask again only this rarely.
const ABSENT: Duration = Duration::from_secs(600);
const TIMEOUT: Duration = Duration::from_secs(10);
const READ_ID: u64 = 2;

static CACHE: Mutex<Option<(Instant, Option<ProviderRateLimits>)>> = Mutex::new(None);

fn program() -> std::ffi::OsString {
    crate::shell_env::env_var(PROGRAM_ENV)
        .filter(|value| !value.trim().is_empty())
        .map(Into::into)
        .unwrap_or_else(|| "codex".into())
}

/// The plan windows the Codex CLI reports, from the cache while fresh.
pub(crate) async fn rate_limits() -> Option<ProviderRateLimits> {
    if let Some((at, cached)) = CACHE.lock().ok().and_then(|cache| cache.clone()) {
        let ttl = if cached.is_some() { FRESH } else { ABSENT };
        if at.elapsed() < ttl {
            return cached;
        }
    }
    let read = tokio::time::timeout(TIMEOUT, read()).await.ok().flatten();
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), read.clone()));
    }
    read
}

async fn read() -> Option<ProviderRateLimits> {
    let mut child = tokio::process::Command::new(program())
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
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
            .ok()?;
    }
    stdin.flush().await.ok()?;
    let mut lines = BufReader::new(child.stdout.take()?).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(READ_ID) {
            return message
                .get("result")
                .and_then(|result| result.get("rateLimits"))
                .and_then(parse);
        }
    }
    None
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
}
