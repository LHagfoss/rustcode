//! Quota for the machine's Antigravity CLI login, using its read-only JSON report.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Mutex;

const TIMEOUT: Duration = Duration::from_secs(10);
const FRESH: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(10);

type Read = Result<Vec<Value>, String>;
static CACHE: Mutex<Option<(Instant, Read)>> = Mutex::const_new(None);

/// The quota groups, read at most once a minute (ten seconds after a failed
/// read); `force` reads again now. None when the CLI is not installed.
pub(crate) async fn read_cached(force: bool) -> Option<Read> {
    let mut cached = CACHE.lock().await;
    if let Some((at, read)) = cached.as_ref() {
        let fresh = if read.is_ok() { FRESH } else { RETRY };
        if !force && at.elapsed() < fresh {
            return Some(read.clone());
        }
    }
    let (program, env) = configured()?;
    let read = tokio::time::timeout(TIMEOUT, read(&program, &env))
        .await
        .unwrap_or_else(|_| {
            Err(
                "Antigravity CLI did not answer within 10 seconds. It may be waiting for a \
                 sign-in: press Sign in under Settings › Harnesses › Antigravity, then Refresh."
                    .to_owned(),
            )
        });
    if let Err(error) = &read {
        crate::dbg_log!("antigravity quota: {} failed: {error}", program.display());
    }
    *cached = Some((Instant::now(), read.clone()));
    Some(read)
}

/// Settings › Usage's row. An absent CLI has no row. Refresh can bypass the
/// cache after sign-in.
pub(crate) async fn plan(force: bool) -> Option<Value> {
    let (status, windows, error) = match read_cached(force).await? {
        Ok(windows) => ("ok", windows, None),
        Err(error) => ("unavailable", Vec::new(), Some(error)),
    };
    Some(json!({
        "id": "antigravity", "harness": "antigravity", "name": "Model quotas",
        "via": "Antigravity", "status": status, "windows": windows, "error": error,
    }))
}

/// The program and environment Settings › Harnesses runs Antigravity with,
/// so the quota is read from the same install and login as its threads.
fn configured() -> Option<(PathBuf, Vec<(String, String)>)> {
    let harness = crate::harness::config::config_dir().ok().and_then(|dir| {
        let (_, entries) = crate::harness::config::load_in(&dir);
        crate::harness::resolve(&entries, "antigravity")
    });
    let (program, env) = match harness {
        Some(harness) => (
            harness
                .command
                .first()
                .cloned()
                .unwrap_or_else(|| "agy".to_owned()),
            harness.env,
        ),
        None => ("agy".to_owned(), Vec::new()),
    };
    Some((crate::harness::detect::resolve_program(&program)?, env))
}

async fn read(program: &Path, env: &[(String, String)]) -> Read {
    let start = || format!("Could not start {}.", program.display());
    let mut command = crate::platform::tokio_command(program);
    command
        .env("PATH", crate::harness::detect::search_path())
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let version = command
        .arg("--version")
        .output()
        .await
        .map_err(|_| start())?;
    // Earlier versions interpret this slash command as a model prompt.
    if !version.status.success() || !supports_usage(&String::from_utf8_lossy(&version.stdout)) {
        return Err("Update Antigravity CLI to 1.1.11 or later to see quotas.".to_owned());
    }

    // An expired login can send the CLI down its OAuth flow. Suppress its
    // browser launchers and keep the command out of the user's project.
    let prepare = |_| "Could not prepare the Antigravity quota read.".to_owned();
    let directory = tempfile::tempdir().map_err(prepare)?;
    for name in ["open", "xdg-open", "gio"] {
        let launcher = directory.path().join(name);
        std::fs::write(&launcher, "#!/bin/sh\nexit 1\n").map_err(prepare)?;
        std::fs::set_permissions(launcher, std::fs::Permissions::from_mode(0o700))
            .map_err(prepare)?;
    }
    let path = crate::harness::detect::search_path();
    let path = std::env::join_paths(
        std::iter::once(directory.path().to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .map_err(|_| "Could not prepare the Antigravity quota read.".to_owned())?;
    let report = crate::platform::tokio_command(program)
        .args(["--print", "/usage", "--output-format", "json"])
        .current_dir(directory.path())
        .envs(env.iter().map(|(name, value)| (name, value)))
        .env("PATH", path)
        .env("BROWSER", directory.path().join("open"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| start())?;
    let said = said(&report.stdout, &report.stderr);
    if !report.status.success() {
        return Err(with_said(
            "Antigravity CLI could not read the quota. Check its sign-in, then press Refresh.",
            said.as_deref(),
        ));
    }
    let value = serde_json::from_slice(&report.stdout).map_err(|_| {
        with_said(
            "Antigravity CLI did not return a quota report.",
            said.as_deref(),
        )
    })?;
    parse_report(&value).ok_or_else(|| {
        with_said(
            "Antigravity CLI could not report quota. Check its sign-in in agy.",
            said.as_deref(),
        )
    })
}

/// Longest stretch of the CLI's own words an error carries.
const SAID_LIMIT: usize = 300;

/// What the CLI said about a failed read: the `error` of its JSON answer,
/// else its last stderr lines, trimmed to [`SAID_LIMIT`] characters. None
/// when it said nothing.
fn said(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let from_json = serde_json::from_slice::<Value>(stdout)
        .ok()
        .and_then(|value| {
            ["error", "message", "response"]
                .into_iter()
                .find_map(|key| match &value[key] {
                    Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
                    Value::Object(object) => object
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    _ => None,
                })
        });
    let text = from_json.unwrap_or_else(|| {
        let stderr = String::from_utf8_lossy(stderr);
        let lines: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        lines[lines.len().saturating_sub(3)..].join(" ")
    });
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(if text.chars().count() > SAID_LIMIT {
        let cut: String = text.chars().take(SAID_LIMIT).collect();
        format!("{cut}…")
    } else {
        text
    })
}

fn with_said(message: &str, said: Option<&str>) -> String {
    match said {
        Some(said) => format!("{message} agy said: {said}"),
        None => message.to_owned(),
    }
}

fn supports_usage(output: &str) -> bool {
    let Some(version) = crate::harness::detect::parse_version(output) else {
        return false;
    };
    let release = version.split_once('+').map_or(version.as_str(), |(v, _)| v);
    let parts: Vec<_> = release.split('.').collect();
    let [major, minor, patch] = parts.as_slice() else {
        return false;
    };
    match (
        major.parse::<u64>(),
        minor.parse::<u64>(),
        patch.parse::<u64>(),
    ) {
        (Ok(major), Ok(minor), Ok(patch)) => (major, minor, patch) >= (1, 1, 11),
        _ => false,
    }
}

fn parse_report(value: &Value) -> Option<Vec<Value>> {
    if value["status"] != "SUCCESS" || value["command"]["name"] != "usage" {
        return None;
    }
    let groups = value["command"]["data"]["groups"].as_array()?;
    Some(
        groups
            .iter()
            .filter_map(|group| Some((group["name"].as_str()?, group["buckets"].as_array()?)))
            .flat_map(|(group, buckets)| {
                buckets.iter().filter_map(move |bucket| {
                    if bucket["disabled"] == true {
                        return None;
                    }
                    let remaining = bucket["remaining_fraction"]
                        .as_f64()
                        .filter(|value| (0.0..=1.0).contains(value))?;
                    let (name, minutes) = match bucket["window"].as_str() {
                        Some("5h") => ("5-hour", Some(300)),
                        Some("weekly") => ("Weekly", Some(10_080)),
                        _ => (bucket["name"].as_str()?, None),
                    };
                    let resets_at = bucket["reset_time"]
                        .as_str()
                        .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                        .map(|time| time.timestamp());
                    Some(json!({
                        "label": format!("{group} · {name}"),
                        "usedPercent": (1.0 - remaining) * 100.0,
                        "windowMinutes": minutes,
                        "resetsAt": resets_at,
                    }))
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_versions_where_usage_might_start_a_model_turn() {
        assert!(!supports_usage("1.1.10"));
        assert!(!supports_usage("1.1.11-rc.1"));
        assert!(!supports_usage("unknown"));
        assert!(supports_usage("1.1.11"));
        assert!(supports_usage("agy v1.5.11"));
    }

    #[test]
    fn quota_report_keeps_model_groups_and_skips_unreported_windows() {
        let report = json!({
            "status": "SUCCESS",
            "command": {"name": "usage", "data": {"groups": [
                {"name": "Gemini Models", "buckets": [
                    {"window": "weekly", "remaining_fraction": 0.75, "reset_time": "2026-11-25T12:09:15Z"},
                    {"window": "5h", "reset_time": "2026-11-25T12:09:15Z"},
                ]},
                {"name": "Claude and GPT models", "buckets": [
                    {"window": "weekly", "remaining_fraction": 0.0},
                    {"window": "5h", "remaining_fraction": 1.0, "disabled": true},
                ]},
            ]}},
        });
        let rows = parse_report(&report).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["label"], "Gemini Models · Weekly");
        assert_eq!(rows[0]["usedPercent"], 25.0);
        assert_eq!(rows[0]["windowMinutes"], 10_080);
        assert_eq!(rows[0]["resetsAt"], 1_795_608_555_i64);
        assert_eq!(rows[1]["label"], "Claude and GPT models · Weekly");
        assert_eq!(rows[1]["usedPercent"], 100.0);
        assert!(rows[1]["resetsAt"].is_null());
        assert!(parse_report(&json!({"status": "SUCCESS", "num_turns": 1})).is_none());
    }

    #[test]
    fn a_failed_read_carries_what_the_cli_said() {
        let json = br#"{"status":"ERROR","error":"Authentication required"}"#;
        assert_eq!(said(json, b"").as_deref(), Some("Authentication required"));
        let stderr = b"starting\n\nfirst\nsecond\nthird\n";
        assert_eq!(said(b"", stderr).as_deref(), Some("first second third"));
        assert_eq!(said(b"", b"  \n"), None);
        let long = "x".repeat(SAID_LIMIT + 10);
        assert_eq!(
            said(b"", long.as_bytes()).unwrap().chars().count(),
            SAID_LIMIT + 1
        );
        assert_eq!(
            with_said("Could not read.", Some("Authentication required")),
            "Could not read. agy said: Authentication required"
        );
        assert_eq!(with_said("Could not read.", None), "Could not read.");
    }
}
