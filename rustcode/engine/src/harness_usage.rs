//! Token and cost totals read from a harness's own session log.
//!
//! Some harnesses report only a context gauge over ACP (`used`/`size`) and
//! keep the real token counts in their transcript. Two of them are read here,
//! by [`session_usage`]:
//!
//! - Claude Code: `<claude>/projects/<slug>/<native id>.jsonl`, one assistant
//!   line per content block, each repeating the same `message.usage`; lines
//!   are deduplicated by `message.id` or the sum is roughly doubled.
//! - Pi: `~/.pi/agent/sessions/<dir>/<started>_<native id>.jsonl`, one
//!   `message` line per model response with its usage and cost.
//!
//! Only numbers are taken: token counts, cost, a timestamp, a message id and
//! a model name. No prompt, answer or tool output is read into memory, and
//! nothing is written anywhere. A log that is missing, too large, or full of
//! lines this cannot parse yields `None` — never a guessed number.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::data_store::{SessionUsage, TokenTotals};

/// A log larger than this is not read; the turn keeps NULL totals.
const MAX_LOG_BYTES: u64 = 512 * 1024 * 1024;

/// One line longer than this is skipped rather than buffered whole.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// The usage of `thread`'s native session on `harness`, from its log.
///
/// `native_session_id` and `cwd` come from the thread's `harness.json`;
/// `None` when the harness keeps no log we can read, or the file is gone.
pub fn session_usage(
    harness: &str,
    native_session_id: Option<&str>,
    cwd: Option<&Path>,
    account_config_dir: Option<&Path>,
) -> Option<SessionUsage> {
    let id = native_session_id.filter(|id| !id.trim().is_empty())?;
    match harness {
        "claude-code" => {
            let logs = claude_logs(id, cwd, account_config_dir);
            (!logs.is_empty()).then(|| read_claude_logs(&logs))
        }
        "pi" => pi_log(id).map(|path| read_pi(&path)),
        _ => None,
    }
}

/// Whether a harness keeps its token counts only in its own log. Those report
/// nothing over ACP as a turn runs, so the client's gauge has to be fed from
/// the log (`poll_logs`) and not only when the turn ends.
pub fn log_based(harness: &str) -> bool {
    matches!(harness, "claude-code" | "pi")
}

/// The same logs [`session_usage`] reads, for a poller that wants to stat them
/// before paying for a read. Empty for a harness that keeps none.
pub fn session_logs(
    harness: &str,
    native_session_id: Option<&str>,
    cwd: Option<&Path>,
    account_config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let Some(id) = native_session_id.filter(|id| !id.trim().is_empty()) else {
        return Vec::new();
    };
    match harness {
        "claude-code" => claude_logs(id, cwd, account_config_dir),
        "pi" => pi_log(id).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A set of logs' identity: a poll is worth a read only when this changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogStamp {
    /// Total bytes across the logs.
    pub len: u64,
    /// The latest modification among them.
    pub modified: Option<std::time::SystemTime>,
}

impl LogStamp {
    /// `None` when none of the paths exists: a session with no log yet.
    fn of(paths: &[PathBuf]) -> Option<Self> {
        let mut stamp = LogStamp::default();
        let mut found = false;
        for path in paths {
            let Ok(metadata) = std::fs::metadata(path) else {
                continue;
            };
            found = true;
            stamp.len += metadata.len();
            stamp.modified = stamp.modified.max(metadata.modified().ok());
        }
        found.then_some(stamp)
    }
}

/// One poll of a running turn's log: what the client's gauge should show, and
/// the stamp to compare against next time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveRead {
    /// The logs' identity now; `None` when there is no log to read.
    pub stamp: Option<LogStamp>,
    /// The tokens the session's latest request put in the context window;
    /// `None` when the logs have not changed since `since`, or hold no usage
    /// line yet.
    pub used: Option<u64>,
}

/// The context a running turn's latest request occupies, read only when the
/// logs grew since `since`: an hour-long turn over a large log costs one
/// `stat` per poll, not one read.
pub fn poll_logs(harness: &str, paths: &[PathBuf], since: Option<LogStamp>) -> LiveRead {
    let stamp = LogStamp::of(paths);
    if stamp.is_none() || stamp == since {
        return LiveRead { stamp, used: None };
    }
    let used = match harness {
        "claude-code" => read_claude_context(paths),
        "pi" => paths.first().and_then(|path| read_pi_context(path)),
        _ => None,
    };
    LiveRead { stamp, used }
}

/// Claude Code's transcripts of a session: in each config folder, the project
/// folder named after the working directory, else any project folder that
/// holds it. A session resumed under another profile has a log in both.
fn claude_logs(id: &str, cwd: Option<&Path>, account_config_dir: Option<&Path>) -> Vec<PathBuf> {
    if !safe_id(id) {
        return Vec::new();
    }
    let home = crate::home::home_dir();
    let configured = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    claude_config_dirs(configured, home.as_deref(), account_config_dir)
        .iter()
        .filter_map(|base| claude_log_in(&base.join("projects"), id, cwd))
        .collect()
}

/// Where Claude Code's logs may be: `CLAUDE_CONFIG_DIR`, `~/.claude`, then
/// other profiles such as `~/.claude-work`. A thread keeps its log in the
/// profile it ran under, whichever one this process was started with. Session
/// ids are UUIDs, so looking in another profile cannot find the wrong log.
fn claude_config_dirs(
    configured: Option<PathBuf>,
    home: Option<&Path>,
    account_config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    // The account's own directory first: that is where the thread ran.
    let mut dirs: Vec<PathBuf> = account_config_dir
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    dirs.extend(configured);
    if let Some(home) = home {
        dirs.push(home.join(".claude"));
        let mut profiles: Vec<PathBuf> = std::fs::read_dir(home)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".claude-"))
            .map(|entry| entry.path())
            .filter(|path| path.join("projects").is_dir())
            .collect();
        profiles.sort();
        dirs.append(&mut profiles);
    }
    dirs
}

fn claude_log_in(projects: &Path, id: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if let Some(cwd) = cwd {
        let candidate = projects.join(project_slug(cwd)).join(format!("{id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for entry in std::fs::read_dir(projects).ok()?.flatten() {
        let candidate = entry.path().join(format!("{id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// How Claude Code names a project folder: the path with every other
/// character replaced by `-` (`/Users/me/code` → `-Users-me-code`).
fn project_slug(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

/// Pi's session file: `<started>_<id>.jsonl` under a workspace folder.
fn pi_log(id: &str) -> Option<PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let home = crate::home::home_dir()?;
    let sessions = home.join(".pi/agent/sessions");
    let wanted = format!("_{id}.jsonl");
    for entry in std::fs::read_dir(&sessions).ok()?.flatten() {
        let dir = entry.path();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in entries.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if name.ends_with(&wanted) || name == format!("{id}.jsonl") {
                return Some(file.path());
            }
        }
    }
    None
}

/// A session id is used in a file name: no separators, no `..`.
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Streams a log's lines, skipping any that are too long or not text. A file
/// that is missing or over [`MAX_LOG_BYTES`] reads as nothing.
fn scan(path: &Path, mut visit: impl FnMut(&str)) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if metadata.len() > MAX_LOG_BYTES {
        crate::dbg_log!(
            "harness usage: {} is {} bytes, over the cap; totals stay empty",
            path.display(),
            metadata.len()
        );
        return;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.len() > MAX_LINE_BYTES {
            continue;
        }
        let Ok(line) = std::str::from_utf8(&buffer) else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        visit(line);
    }
}

fn count(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|count| *count >= 0)
}

/// Keeps `candidate` when it is later than what `current` holds; RFC 3339
/// UTC sorts by its text, so string order is time order.
fn latest(current: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else { return };
    if current
        .as_ref()
        .is_none_or(|known| candidate.as_str() > known.as_str())
    {
        *current = Some(candidate);
    }
}

/// Claude and Pi both stamp entries; Claude as RFC 3339 text, Pi as epoch
/// milliseconds.
fn timestamp(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let millis = value.as_i64()?;
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

/// Sums Claude Code's assistant usage, one entry per `message.id`.
#[cfg(test)]
fn read_claude(path: &Path) -> SessionUsage {
    read_claude_logs(&[path.to_path_buf()])
}

/// [`read_claude`] over every log of a session. A log copied to another
/// profile repeats its messages, and the shared `message.id` set counts them once.
fn read_claude_logs(paths: &[PathBuf]) -> SessionUsage {
    let mut seen: HashSet<String> = HashSet::new();
    let mut usage = SessionUsage::default();
    for path in paths {
        read_claude_log(path, &mut seen, &mut usage);
    }
    usage
}

fn read_claude_log(path: &Path, seen: &mut HashSet<String>, usage: &mut SessionUsage) {
    scan(path, |line| {
        // A cheap filter before parsing: most lines are not assistant usage.
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(message) = value.get("message") else {
            return;
        };
        // A local placeholder that never reached the API.
        if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
            return;
        }
        let Some(entry) = message.get("usage") else {
            return;
        };
        if let Some(id) = message.get("id").and_then(Value::as_str)
            && !seen.insert(id.to_owned())
        {
            return;
        }
        latest(&mut usage.at, timestamp(value.get("timestamp")));
        usage.tokens.add(&TokenTotals {
            input: count(entry.get("input_tokens")),
            output: count(entry.get("output_tokens")),
            cached: count(entry.get("cache_read_input_tokens")),
            cache_write: count(entry.get("cache_creation_input_tokens")),
        });
        if let Some(cost) = value.get("costUSD").and_then(Value::as_f64)
            && cost.is_finite()
        {
            usage.cost_usd = Some(usage.cost_usd.unwrap_or(0.0) + cost);
        }
    });
}

/// Sums Pi's per-response usage; Pi prices its own requests in USD.
fn read_pi(path: &Path) -> SessionUsage {
    let mut usage = SessionUsage::default();
    scan(path, |line| {
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            return;
        }
        let Some(message) = value.get("message") else {
            return;
        };
        let Some(entry) = message.get("usage") else {
            return;
        };
        latest(&mut usage.at, timestamp(message.get("timestamp")));
        usage.tokens.add(&TokenTotals {
            input: count(entry.get("input")),
            output: count(entry.get("output")),
            cached: count(entry.get("cacheRead")),
            cache_write: count(entry.get("cacheWrite")),
        });
        if let Some(cost) = entry
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64)
            && cost.is_finite()
        {
            usage.cost_usd = Some(usage.cost_usd.unwrap_or(0.0) + cost);
        }
    });
    usage
}

/// Keeps the newest entry's token count: a later timestamp wins, and an entry
/// with none is the file's tail, so it supersedes what came before it.
fn keep_latest(current: &mut Option<(Option<String>, u64)>, at: Option<String>, tokens: u64) {
    let newer = match current.as_ref() {
        Some((Some(known), _)) => at.as_ref().is_none_or(|at| at >= known),
        _ => true,
    };
    if newer {
        *current = Some((at, tokens));
    }
}

/// The tokens Claude Code's latest assistant message put in the window: what
/// it read (input and both cache counters) plus what it wrote. No
/// `totalTokens`, so the four are summed here.
fn read_claude_context(paths: &[PathBuf]) -> Option<u64> {
    let mut latest: Option<(Option<String>, u64)> = None;
    for path in paths {
        scan(path, |line| {
            if !line.contains("\"usage\"") {
                return;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                return;
            };
            if value.get("type").and_then(Value::as_str) != Some("assistant") {
                return;
            }
            let Some(message) = value.get("message") else {
                return;
            };
            if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
                return;
            }
            let Some(entry) = message.get("usage") else {
                return;
            };
            let tokens = [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ]
            .iter()
            .filter_map(|key| count(entry.get(*key)))
            .map(|count| count.max(0) as u64)
            .sum::<u64>();
            keep_latest(&mut latest, timestamp(value.get("timestamp")), tokens);
        });
    }
    latest.map(|(_, tokens)| tokens)
}

/// The tokens Pi's latest response put in the window. Pi stamps each response
/// with `totalTokens`; a log that lacks it still sums to the same number.
fn read_pi_context(path: &Path) -> Option<u64> {
    let mut latest: Option<(Option<String>, u64)> = None;
    scan(path, |line| {
        if !line.contains("\"usage\"") {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if value.get("type").and_then(Value::as_str) != Some("message") {
            return;
        }
        let Some(message) = value.get("message") else {
            return;
        };
        let Some(entry) = message.get("usage") else {
            return;
        };
        let tokens = ["input", "output", "cacheRead", "cacheWrite"]
            .iter()
            .filter_map(|key| count(entry.get(*key)))
            .map(|count| count.max(0) as u64)
            .sum::<u64>();
        keep_latest(&mut latest, timestamp(message.get("timestamp")), tokens);
    });
    latest.map(|(_, tokens)| tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn claude_line(id: &str, input: i64, output: i64, cache_read: i64, cache_write: i64) -> String {
        format!(
            concat!(
                r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-5-5","#,
                r#""usage":{{"input_tokens":{input},"output_tokens":{output},"#,
                r#""cache_read_input_tokens":{cache_read},"#,
                r#""cache_creation_input_tokens":{cache_write}}}}}}}"#,
            ),
            id = id,
            input = input,
            output = output,
            cache_read = cache_read,
            cache_write = cache_write,
        )
    }

    #[test]
    fn logs_of_one_session_in_two_profiles_count_each_message_once() {
        let dir = tempfile::tempdir().unwrap();
        let line = |id: &str, output: i64| {
            format!(
                "{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"output_tokens\":{output}}}}}}}\n"
            )
        };
        let first = dir.path().join("first.jsonl");
        let second = dir.path().join("second.jsonl");
        std::fs::write(&first, line("a", 10) + &line("b", 20)).unwrap();
        std::fs::write(&second, line("b", 20) + &line("c", 40)).unwrap();
        let usage = read_claude_logs(&[first, second]);
        assert_eq!(
            usage.tokens.output,
            Some(70),
            "a + b + c, with b counted once"
        );
    }

    #[test]
    fn a_log_in_another_claude_profile_is_found() {
        let home = tempfile::tempdir().unwrap();
        let id = "01c534e5-afc8-4477-a154-cdafb3c7754a";
        let folder = home.path().join(".claude-personal/projects/-Users-me-code");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{id}.jsonl")), "").unwrap();
        // A file named like a profile, and a profile with no logs, are skipped.
        std::fs::write(home.path().join(".claude-notes"), "").unwrap();
        std::fs::create_dir_all(home.path().join(".claude-empty")).unwrap();
        std::fs::create_dir_all(home.path().join(".claude/projects")).unwrap();

        let account = home.path().join(".claude-paral-2");
        let dirs = claude_config_dirs(
            Some(PathBuf::from("/set")),
            Some(home.path()),
            Some(&account),
        );
        assert_eq!(
            dirs,
            vec![
                account,
                PathBuf::from("/set"),
                home.path().join(".claude"),
                home.path().join(".claude-personal"),
            ],
            "the account's own folder first, then the configured one, the default, other profiles"
        );
        let found = dirs
            .iter()
            .find_map(|base| claude_log_in(&base.join("projects"), id, None));
        assert_eq!(found, Some(folder.join(format!("{id}.jsonl"))));
    }

    #[test]
    fn claude_counts_a_message_once_however_many_lines_it_spans() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "log.jsonl",
            &[
                &claude_line("msg_1", 2, 100, 1000, 50),
                &claude_line("msg_1", 2, 100, 1000, 50),
                &claude_line("msg_1", 2, 100, 1000, 50),
                &claude_line("msg_2", 3, 200, 2000, 0),
                r#"{"type":"user","message":{"content":"ignored"}}"#,
                r#"{"type":"assistant","message":{"model":"<synthetic>","usage":{"input_tokens":9}}}"#,
            ],
        );
        let usage = read_claude(&path);
        assert_eq!(
            usage.tokens,
            TokenTotals {
                input: Some(5),
                output: Some(300),
                cached: Some(3000),
                cache_write: Some(50),
            }
        );
        assert_eq!(usage.cost_usd, None, "no costUSD on any line");
    }

    #[test]
    fn claude_reads_a_reported_cost_and_uses_the_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "log.jsonl",
            &[
                r#"{"type":"assistant","costUSD":0.5,"message":{"id":"m1","usage":{"input_tokens":1}}}"#,
                r#"{"type":"assistant","costUSD":0.25,"message":{"id":"m2","usage":{"input_tokens":2}}}"#,
                "not json at all",
                r#"{"type":"assistant","message":{"id":"m3","usage":{"input_tokens":"nope"}}}"#,
            ],
        );
        let usage = read_claude(&path);
        assert_eq!(usage.tokens.input, Some(3));
        assert_eq!(usage.tokens.output, None);
        assert_eq!(usage.cost_usd, Some(0.75));
    }

    #[test]
    fn pi_sums_each_response_usage_and_cost() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "pi.jsonl",
            &[
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":100,"output":10,"cacheRead":900,"cacheWrite":5,"totalTokens":1015,"cost":{"total":0.01}}}}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":200,"output":20,"cacheRead":800,"cacheWrite":0,"totalTokens":1020,"cost":{"total":0.02}}}}"#,
                r#"{"type":"model_change","model":"x"}"#,
                r#"{"type":"message","message":{"role":"user","content":"no usage"}}"#,
            ],
        );
        let usage = read_pi(&path);
        assert_eq!(
            usage.tokens,
            TokenTotals {
                input: Some(300),
                output: Some(30),
                cached: Some(1700),
                cache_write: Some(5),
            }
        );
        assert_eq!(usage.cost_usd, Some(0.03));
    }

    #[test]
    fn a_missing_file_or_unknown_harness_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            read_claude(&dir.path().join("absent.jsonl"))
                .tokens
                .is_empty()
        );
        assert_eq!(
            session_usage("codex", Some("abc"), None, None),
            None,
            "no log format for codex"
        );
        assert_eq!(session_usage("claude-code", None, None, None), None);
        assert_eq!(session_usage("pi", Some("../etc/passwd"), None, None), None);
    }

    #[test]
    fn a_huge_line_is_skipped_and_the_rest_still_counts() {
        let dir = tempfile::tempdir().unwrap();
        let mut lines = vec![claude_line("a", 1, 1, 1, 1)];
        lines.push(format!(
            r#"{{"type":"assistant","pad":"{}"}}"#,
            "x".repeat(MAX_LINE_BYTES + 1)
        ));
        lines.push(claude_line("b", 2, 2, 2, 2));
        let path = write(
            dir.path(),
            "big.jsonl",
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let usage = read_claude(&path);
        assert_eq!(usage.tokens.input, Some(3));
        assert_eq!(usage.tokens.output, Some(3));
    }

    const PI_LINE: &str = r#"{"type":"message","message":{"role":"assistant","timestamp":7,"usage":{"input":100,"output":10,"cacheRead":900,"cacheWrite":0,"totalTokens":1010}}}"#;

    #[test]
    fn a_log_that_did_not_grow_is_not_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "pi.jsonl", &[PI_LINE]);
        let first = poll_logs("pi", std::slice::from_ref(&path), None);
        assert_eq!(first.used, Some(1010), "the latest request's context");
        let again = poll_logs("pi", &[path], first.stamp);
        assert_eq!(again.stamp, first.stamp);
        assert_eq!(again.used, None, "nothing changed, nothing is published");
    }

    #[test]
    fn a_grown_log_reports_the_new_context_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "pi.jsonl", &[PI_LINE]);
        let first = poll_logs("pi", std::slice::from_ref(&path), None);
        let mut grown = std::fs::read_to_string(&path).unwrap();
        grown.push('\n');
        grown.push_str(
            r#"{"type":"message","message":{"role":"assistant","timestamp":8,"usage":{"input":200,"output":20,"cacheRead":800,"cacheWrite":5,"totalTokens":1025}}}"#,
        );
        std::fs::write(&path, grown).unwrap();
        let read = poll_logs("pi", std::slice::from_ref(&path), first.stamp);
        assert_ne!(read.stamp, first.stamp);
        assert_eq!(read.used, Some(1025));
        assert_eq!(
            poll_logs("pi", &[path], read.stamp).used,
            None,
            "the same numbers are not published twice"
        );
    }

    #[test]
    fn claude_context_is_its_latest_message_not_its_total() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "claude.jsonl",
            &[
                &claude_line("m1", 2, 100, 1000, 50),
                &claude_line("m2", 3, 200, 2000, 0),
            ],
        );
        let read = poll_logs("claude-code", &[path], None);
        assert_eq!(read.used, Some(2203), "m2: in + out + both cache counters");
    }

    #[test]
    fn a_session_with_no_log_or_no_usage_line_reads_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            poll_logs("pi", &[dir.path().join("absent.jsonl")], None),
            LiveRead::default(),
            "a session that has not written a log yet"
        );
        let path = write(
            dir.path(),
            "early.jsonl",
            &[r#"{"type":"session","id":"x"}"#],
        );
        let read = poll_logs("pi", &[path], None);
        assert!(
            read.stamp.is_some(),
            "the log is there, it just says nothing yet"
        );
        assert_eq!(read.used, None);
        assert_eq!(
            poll_logs("codex", &[dir.path().join("absent.jsonl")], None),
            LiveRead::default(),
            "a harness with no log format"
        );
    }

    #[test]
    fn the_turn_end_totals_are_unchanged_and_the_live_read_adds_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "pi.jsonl", &[PI_LINE, PI_LINE]);
        // What the turn row still records: both lines, summed once.
        let totals = read_pi(&path);
        assert_eq!(
            totals.tokens,
            TokenTotals {
                input: Some(200),
                output: Some(20),
                cached: Some(1800),
                cache_write: Some(0),
            }
        );
        // What a poll reports: one number, and no totals of its own.
        assert_eq!(poll_logs("pi", &[path], None).used, Some(1010));
    }
}
