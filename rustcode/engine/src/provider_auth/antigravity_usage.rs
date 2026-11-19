//! Quota for the machine's Antigravity CLI login, using its read-only JSON report.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Mutex;

const TIMEOUT: Duration = Duration::from_secs(10);
const FRESH: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(10);

type Read = Result<Vec<Value>, &'static str>;
static CACHE: Mutex<Option<(Instant, Read)>> = Mutex::const_new(None);

/// An absent CLI has no row. Refresh can bypass the cache after sign-in.
pub(crate) async fn plan(force: bool) -> Option<Value> {
    let mut cached = CACHE.lock().await;
    let read = match cached.as_ref() {
        Some((at, read)) if !force && at.elapsed() < if read.is_ok() { FRESH } else { RETRY } => {
            read.clone()
        }
        _ => {
            let program = crate::harness::detect::resolve_program("agy")?;
            let read = tokio::time::timeout(TIMEOUT, read(&program))
                .await
                .unwrap_or(Err("Antigravity CLI did not answer within 10 seconds."));
            *cached = Some((Instant::now(), read.clone()));
            read
        }
    };
    let (status, windows, error) = match read {
        Ok(windows) => ("ok", windows, None),
        Err(error) => ("unavailable", Vec::new(), Some(error)),
    };
    Some(json!({
        "id": "antigravity", "harness": "antigravity", "name": "Model quotas",
        "via": "Antigravity", "status": status, "windows": windows, "error": error,
    }))
}

async fn read(program: &Path) -> Read {
    let mut command = crate::platform::tokio_command(program);
    command
        .env("PATH", crate::harness::detect::search_path())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let version = command
        .arg("--version")
        .output()
        .await
        .map_err(|_| "Could not start Antigravity CLI.")?;
    // Earlier versions interpret this slash command as a model prompt.
    if !version.status.success() || !supports_usage(&String::from_utf8_lossy(&version.stdout)) {
        return Err("Update Antigravity CLI to 1.1.11 or later to see quotas.");
    }

    // An expired login can send the CLI down its OAuth flow. Suppress its
    // browser launchers and keep the command out of the user's project.
    const PREPARE_ERROR: &str = "Could not prepare the Antigravity quota read.";
    let directory = tempfile::tempdir().map_err(|_| PREPARE_ERROR)?;
    for name in ["open", "xdg-open", "gio"] {
        let launcher = directory.path().join(name);
        std::fs::write(&launcher, "#!/bin/sh\nexit 1\n").map_err(|_| PREPARE_ERROR)?;
        std::fs::set_permissions(launcher, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| PREPARE_ERROR)?;
    }
    let path = crate::harness::detect::search_path();
    let path = std::env::join_paths(
        std::iter::once(directory.path().to_path_buf()).chain(std::env::split_paths(&path)),
    )
    .map_err(|_| PREPARE_ERROR)?;
    let report = crate::platform::tokio_command(program)
        .args(["--print", "/usage", "--output-format", "json"])
        .current_dir(directory.path())
        .env("PATH", path)
        .env("BROWSER", directory.path().join("open"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| "Could not start Antigravity CLI.")?;
    if !report.status.success() {
        return Err("Run agy to check its sign-in, then press Refresh.");
    }
    let value = serde_json::from_slice(&report.stdout)
        .map_err(|_| "Antigravity CLI did not return a quota report.")?;
    parse_report(&value).ok_or("Antigravity CLI could not report quota. Check its sign-in in agy.")
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
}
