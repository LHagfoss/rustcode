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
) -> Option<SessionUsage> {
    let id = native_session_id.filter(|id| !id.trim().is_empty())?;
    match harness {
        "claude-code" => claude_log(id, cwd).map(|path| read_claude(&path)),
        "pi" => pi_log(id).map(|path| read_pi(&path)),
        _ => None,
    }
}

/// Claude Code's transcript: the project folder named after the working
/// directory, else any project folder that holds this session.
fn claude_log(id: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude")))?;
    let projects = base.join("projects");
    if let Some(cwd) = cwd {
        let candidate = projects.join(project_slug(cwd)).join(format!("{id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for entry in std::fs::read_dir(&projects).ok()?.flatten() {
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
    let home = std::env::var_os("HOME")?;
    let sessions = PathBuf::from(home).join(".pi/agent/sessions");
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
fn read_claude(path: &Path) -> SessionUsage {
    let mut seen: HashSet<String> = HashSet::new();
    let mut usage = SessionUsage::default();
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
    usage
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
            session_usage("codex", Some("abc"), None),
            None,
            "no log format for codex"
        );
        assert_eq!(session_usage("claude-code", None, None), None);
        assert_eq!(session_usage("pi", Some("../etc/passwd"), None), None);
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
}
