//! A workflow run on disk (`docs/workflows.md`), under the thread that started
//! it: `<thread folder>/workflows/<run id>/`.
//!
//! - `run.json`: the run's record (status, phases, log lines, final result),
//!   replaced atomically on every change.
//! - `script.js`: the script as it was started, so a restart runs the same one
//!   even when the named file changed meanwhile.
//! - `calls.jsonl`: one line per `agent()` call state change, appended and
//!   folded by key when read. The last line of a key wins, and a torn last
//!   line (the engine died mid-write) is skipped.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const RUN_FILE: &str = "run.json";
const SCRIPT_FILE: &str = "script.js";
const CALLS_FILE: &str = "calls.jsonl";
const RESULT_FILE: &str = "result.json";
/// The most log lines a run record keeps: the newest ones.
pub(super) const MAX_LOG_LINES: usize = 200;
/// The longest a log line or a prompt preview is kept.
pub(super) const MAX_LINE_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RunStatus {
    /// Started, or interrupted by a restart and waiting to be resumed.
    Running,
    Done,
    Failed,
    TimedOut,
    Cancelled,
}

impl RunStatus {
    pub(super) fn settled(self) -> bool {
        self != Self::Running
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Phase {
    pub title: String,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LogLine {
    pub at: i64,
    pub text: String,
}

/// The limits a run started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Limits {
    pub concurrency: usize,
    pub call_budget: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RunRecord {
    pub id: String,
    /// The thread that started the run; its workers nest under it.
    pub thread: String,
    /// The name in `.rustcode/workflows/`, or `None` for an inline script.
    pub name: Option<String>,
    pub status: RunStatus,
    pub args: Value,
    pub limits: Limits,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    /// How many times a restart picked the run up again.
    pub resumes: u32,
    pub phases: Vec<Phase>,
    pub logs: Vec<LogLine>,
    pub result: Option<Value>,
    pub error: Option<String>,
    /// Whether the thread that started the run was told it ended: a client
    /// claims that once (`claim_wake`), so no reconnect tells it twice.
    #[serde(default)]
    pub woken: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CallState {
    Running,
    Done,
    Failed,
}

/// One `agent()` call, as its last line in `calls.jsonl` says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CallRecord {
    /// `<hash of prompt and options>#<n>`: `n` counts the earlier calls with
    /// the same hash, so two identical calls are two keys and a call is found
    /// again whatever order other calls settled in.
    pub key: String,
    /// Where the call came in the run's call order.
    pub seq: u64,
    pub phase: Option<String>,
    pub prompt: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub state: CallState,
    /// How many times this key failed before this try; a retry after a restart
    /// starts a thread of its own instead of finding the failed one again.
    #[serde(default)]
    pub attempt: u32,
    pub session_id: Option<String>,
    pub link_id: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// The `workflows` folder of one thread.
#[derive(Debug, Clone)]
pub(super) struct Store {
    root: PathBuf,
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64)
}

/// Cut `text` to `max` characters, with an ellipsis when it was longer.
pub(super) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A run id is made of what [`new_run_id`] writes and nothing else, so it can
/// never name a folder outside `workflows/`.
pub(super) fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn new_run_id() -> String {
    format!("wf-{:x}-{:08x}", now_ms(), rand::random::<u32>())
}

impl Store {
    pub(super) fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The `workflows` folder of a thread, in the config folder.
    pub(super) fn for_thread(thread_id: &str) -> Option<Self> {
        if !rustcode_session::valid_session_id(thread_id) {
            return None;
        }
        let config = crate::config::get_config_dir()?;
        let folder = rustcode_session::SessionStore::new(config).session_dir(thread_id);
        Some(Self::at(folder.join("workflows")))
    }

    pub(super) fn run_dir(&self, run_id: &str) -> Option<PathBuf> {
        valid_run_id(run_id).then(|| self.root.join(run_id))
    }

    fn file(&self, run_id: &str, name: &str) -> Option<PathBuf> {
        self.run_dir(run_id).map(|dir| dir.join(name))
    }

    /// Start a run's folder with its record and script.
    pub(super) fn create(&self, record: &RunRecord, script: &str) -> Result<(), String> {
        let dir = self
            .run_dir(&record.id)
            .ok_or_else(|| "invalid run id".to_owned())?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        write_atomically(&dir.join(SCRIPT_FILE), script.as_bytes())?;
        self.save(record)
    }

    pub(super) fn save(&self, record: &RunRecord) -> Result<(), String> {
        let path = self
            .file(&record.id, RUN_FILE)
            .ok_or_else(|| "invalid run id".to_owned())?;
        let text = serde_json::to_vec_pretty(record)
            .map_err(|error| format!("could not write the run record: {error}"))?;
        write_atomically(&path, &text)
    }

    /// The final result as a file of its own, for a result too large to answer.
    pub(super) fn save_result(&self, run_id: &str, result: &Value) -> Result<PathBuf, String> {
        let path = self
            .file(run_id, RESULT_FILE)
            .ok_or_else(|| "invalid run id".to_owned())?;
        let text = serde_json::to_vec_pretty(result)
            .map_err(|error| format!("could not write the result: {error}"))?;
        write_atomically(&path, &text)?;
        Ok(path)
    }

    pub(super) fn load(&self, run_id: &str) -> Option<RunRecord> {
        let bytes = std::fs::read(self.file(run_id, RUN_FILE)?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub(super) fn script(&self, run_id: &str) -> Option<String> {
        std::fs::read_to_string(self.file(run_id, SCRIPT_FILE)?).ok()
    }

    /// Every run of the thread, newest first.
    pub(super) fn list(&self) -> Vec<RunRecord> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut runs: Vec<RunRecord> = entries
            .flatten()
            .filter_map(|entry| self.load(entry.file_name().to_str()?))
            .collect();
        runs.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        runs
    }

    pub(super) fn append_call(&self, run_id: &str, call: &CallRecord) -> Result<(), String> {
        let path = self
            .file(run_id, CALLS_FILE)
            .ok_or_else(|| "invalid run id".to_owned())?;
        let mut line = serde_json::to_vec(call)
            .map_err(|error| format!("could not write the call record: {error}"))?;
        line.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        // One write per line: a line is never interleaved with another.
        file.write_all(&line)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    /// The calls of a run in call order, each as its last line left it.
    pub(super) fn calls(&self, run_id: &str) -> Vec<CallRecord> {
        let Some(text) = self
            .file(run_id, CALLS_FILE)
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return Vec::new();
        };
        let mut order: Vec<String> = Vec::new();
        let mut latest: HashMap<String, CallRecord> = HashMap::new();
        for line in text.lines() {
            // A line the engine died in the middle of is not a call.
            let Ok(call) = serde_json::from_str::<CallRecord>(line) else {
                continue;
            };
            if !latest.contains_key(&call.key) {
                order.push(call.key.clone());
            }
            latest.insert(call.key.clone(), call);
        }
        let mut calls: Vec<CallRecord> = order
            .into_iter()
            .filter_map(|key| latest.remove(&key))
            .collect();
        calls.sort_by_key(|call| call.seq);
        calls
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    crate::atomic_file::replace_file(&temporary, path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str) -> RunRecord {
        RunRecord {
            id: id.to_owned(),
            thread: "1784626803679".to_owned(),
            name: None,
            status: RunStatus::Running,
            args: json!({}),
            limits: Limits {
                concurrency: 2,
                call_budget: 10,
                timeout_secs: 60,
            },
            started_at: 1,
            updated_at: 1,
            finished_at: None,
            resumes: 0,
            phases: Vec::new(),
            logs: Vec::new(),
            result: None,
            error: None,
            woken: false,
        }
    }

    fn call(key: &str, seq: u64, state: CallState) -> CallRecord {
        CallRecord {
            key: key.to_owned(),
            seq,
            phase: None,
            prompt: "p".to_owned(),
            harness: None,
            model: None,
            state,
            attempt: 0,
            session_id: None,
            link_id: None,
            started_at: 1,
            finished_at: None,
            result: None,
            error: None,
        }
    }

    #[test]
    fn a_run_id_cannot_leave_the_workflows_folder() {
        for bad in ["", "..", "../x", "a/b", "A", "wf_1", "wf-1/../../x"] {
            assert!(!valid_run_id(bad), "{bad}");
        }
        assert!(valid_run_id(&new_run_id()));
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf());
        assert!(store.run_dir("../escape").is_none());
        assert!(store.load("../escape").is_none());
        assert!(
            store
                .append_call("../escape", &call("k#0", 0, CallState::Done))
                .is_err()
        );
    }

    #[test]
    fn a_run_is_saved_and_listed_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let mut older = record("wf-1-aaaaaaaa");
        older.started_at = 10;
        let mut newer = record("wf-2-bbbbbbbb");
        newer.started_at = 20;
        store.create(&older, "return 1").unwrap();
        store.create(&newer, "return 2").unwrap();
        let ids: Vec<String> = store.list().into_iter().map(|run| run.id).collect();
        assert_eq!(ids, ["wf-2-bbbbbbbb", "wf-1-aaaaaaaa"]);
        assert_eq!(store.script("wf-1-aaaaaaaa").as_deref(), Some("return 1"));
        let mut changed = store.load("wf-1-aaaaaaaa").unwrap();
        changed.status = RunStatus::Done;
        store.save(&changed).unwrap();
        assert_eq!(store.load("wf-1-aaaaaaaa").unwrap().status, RunStatus::Done);
    }

    #[test]
    fn the_last_line_of_a_call_wins_and_a_torn_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        store.create(&record("wf-1-aaaaaaaa"), "").unwrap();
        store
            .append_call("wf-1-aaaaaaaa", &call("a#0", 0, CallState::Running))
            .unwrap();
        store
            .append_call("wf-1-aaaaaaaa", &call("b#0", 1, CallState::Running))
            .unwrap();
        let mut done = call("a#0", 0, CallState::Done);
        done.result = Some(json!("answer"));
        store.append_call("wf-1-aaaaaaaa", &done).unwrap();
        // The engine died while writing the next line.
        let path = store.run_dir("wf-1-aaaaaaaa").unwrap().join(CALLS_FILE);
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(br#"{"key":"c#0","seq":2,"state":"do"#)
            .unwrap();
        let calls = store.calls("wf-1-aaaaaaaa");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].key, "a#0");
        assert_eq!(calls[0].state, CallState::Done);
        assert_eq!(calls[0].result, Some(json!("answer")));
        assert_eq!(calls[1].state, CallState::Running);
    }

    #[test]
    fn clip_keeps_short_text_and_marks_a_cut() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }
}
