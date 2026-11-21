//! Quota for the machine's Antigravity CLI login, using its read-only JSON report.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Mutex;

const TIMEOUT: Duration = Duration::from_secs(10);
const FRESH: Duration = Duration::from_secs(60);
const RETRY: Duration = Duration::from_secs(10);

type Read = Result<Vec<Value>, String>;
static CACHE: Mutex<Option<(Instant, Read)>> = Mutex::const_new(None);
/// A read behind a stale answer is under way, so another caller does not start one.
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// The last quota groups the CLI reported, kept across engine restarts so the
/// first look after one draws numbers at once while a new read runs behind them.
const RECORD_FILE: &str = "antigravity-usage.json";

/// The quota groups, read at most once a minute (ten seconds after a failed
/// read); `force` reads again now and waits for it. A good read that went stale
/// (or the one recorded before the engine started) answers at once while a new
/// read runs in the background, so a look never waits on `agy` once it has
/// answered. None when the CLI is not installed.
pub(crate) async fn read_cached(force: bool) -> Option<Read> {
    let mut cached = CACHE.lock().await;
    if !force && cached.is_none() {
        // A fresh engine: what the last one read, marked stale, so the read
        // below starts in the background instead of keeping this look waiting.
        if let Some(rows) = crate::harness::config::config_dir()
            .ok()
            .and_then(|dir| recorded_in(&dir))
        {
            let stale = Instant::now()
                .checked_sub(FRESH)
                .unwrap_or_else(Instant::now);
            *cached = Some((stale, Ok(rows)));
        }
    }
    if let Some((at, read)) = cached.as_ref() {
        let fresh = if read.is_ok() { FRESH } else { RETRY };
        if !force && at.elapsed() < fresh {
            return Some(read.clone().map(|rows| fold_resets(rows, now())));
        }
        if !force && let Ok(rows) = read {
            let rows = fold_resets(rows.clone(), now());
            drop(cached);
            configured()?;
            refresh_in_background();
            return Some(Ok(rows));
        }
    }
    let read = read_now().await?;
    *cached = Some((Instant::now(), read.clone()));
    Some(read.map(|rows| fold_resets(rows, now())))
}

/// Whether a read behind a stale answer is still under way: the answer's rows
/// say `refreshing`, so the app asks again shortly for the new numbers.
pub(crate) fn refreshing() -> bool {
    REFRESHING.load(Ordering::Acquire)
}

/// One read in the background, never two at once. It does not hold the cache
/// while `agy` runs, so every look meanwhile answers from the stale numbers.
fn refresh_in_background() {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async {
        if let Some(read) = read_now().await {
            let mut cached = CACHE.lock().await;
            // A failed read keeps the good numbers on screen; it is retried
            // after `RETRY` rather than at once.
            match (&read, cached.as_mut()) {
                (Err(_), Some((at, Ok(_)))) => {
                    *at = Instant::now()
                        .checked_sub(FRESH.saturating_sub(RETRY))
                        .unwrap_or_else(Instant::now);
                }
                _ => *cached = Some((Instant::now(), read)),
            }
        }
        REFRESHING.store(false, Ordering::Release);
    });
}

/// One read of the CLI, with its timeout; a good one is recorded. None when
/// the CLI is not installed.
async fn read_now() -> Option<Read> {
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
    match &read {
        Ok(rows) => {
            if let Ok(dir) = crate::harness::config::config_dir() {
                record_in(&dir, rows, now());
            }
        }
        Err(error) => {
            crate::dbg_log!("antigravity quota: {} failed: {error}", program.display());
        }
    }
    Some(read)
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A window whose reset passed since it was read starts over: nothing used, and
/// no reset time until the next read reports one.
fn fold_resets(rows: Vec<Value>, now: i64) -> Vec<Value> {
    rows.into_iter()
        .map(|mut row| {
            if row["resetsAt"].as_i64().is_some_and(|at| at <= now) {
                row["usedPercent"] = json!(0.0);
                row["resetsAt"] = Value::Null;
            }
            row
        })
        .collect()
}

fn recorded_in(dir: &Path) -> Option<Vec<Value>> {
    let text = std::fs::read_to_string(dir.join(RECORD_FILE)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let rows = value["windows"].as_array()?.clone();
    (!rows.is_empty()).then_some(rows)
}

fn record_in(dir: &Path, rows: &[Value], read_at: i64) {
    if rows.is_empty() {
        return;
    }
    let body = json!({ "readAt": read_at, "windows": rows });
    if let Err(error) =
        crate::service::write_private(&dir.join(RECORD_FILE), body.to_string().as_bytes())
    {
        crate::dbg_log!("antigravity quota: could not record the read: {error:#}");
    }
}

/// Settings › Usage's row. An absent CLI has no row. Refresh can bypass the
/// cache after sign-in. `refreshing` says newer numbers are on the way.
pub(crate) async fn plan(force: bool) -> Option<Value> {
    let (status, windows, error) = match read_cached(force).await? {
        Ok(windows) => ("ok", windows, None),
        Err(error) => ("unavailable", Vec::new(), Some(error)),
    };
    Some(json!({
        "id": "antigravity", "harness": "antigravity", "name": "Model quotas",
        "via": "Antigravity", "status": status, "windows": windows, "error": error,
        "refreshing": refreshing(),
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
            &format!(
                "Antigravity CLI ({}) could not read the quota. Check its sign-in, then press \
                 Refresh.",
                program.display()
            ),
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
    fn a_window_whose_reset_passed_starts_over() {
        let rows = vec![
            json!({"label": "a", "usedPercent": 40.0, "resetsAt": 100}),
            json!({"label": "b", "usedPercent": 40.0, "resetsAt": 300}),
            json!({"label": "c", "usedPercent": 40.0, "resetsAt": null}),
        ];
        let rows = fold_resets(rows, 200);
        assert_eq!(rows[0]["usedPercent"], 0.0);
        assert!(rows[0]["resetsAt"].is_null());
        assert_eq!(rows[1]["usedPercent"], 40.0);
        assert_eq!(rows[1]["resetsAt"], 300);
        assert_eq!(rows[2]["usedPercent"], 40.0);
    }

    #[test]
    fn a_good_read_is_recorded_for_the_next_engine() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(recorded_in(dir.path()), None);
        record_in(dir.path(), &[], 1);
        assert_eq!(recorded_in(dir.path()), None);
        let rows = vec![json!({"label": "Gemini Models · Weekly", "usedPercent": 56.0})];
        record_in(dir.path(), &rows, 1);
        assert_eq!(recorded_in(dir.path()), Some(rows));
        let mode = std::fs::metadata(dir.path().join(RECORD_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
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
