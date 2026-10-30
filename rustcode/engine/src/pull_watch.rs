//! Watching a pull request for what needs the reader (#2648): failing or
//! recovered checks, new reviews and comments, merge conflicts, and the pull
//! request closing. The reader starts a watch from a thread; the engine
//! checks it through `gh` on its own schedule and tells the app what
//! changed. Nothing here acts on the pull request: no approving, merging,
//! commenting or retrying.
//!
//! Watches live in `storage/pull_watches.json`, so a watch carries on after
//! a restart and compares against what it last saw: what happened while
//! RustCode was closed is reported once, on the first check. One engine at a
//! time checks, the one holding `storage/pull_watches.lock`; the others only
//! edit the list.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How often a watch is checked while GitHub answers.
pub const INTERVAL_SECS: i64 = 120;
/// The longest wait between checks after failures.
pub const MAX_BACKOFF_SECS: i64 = 30 * 60;
/// Failed checks in a row before a watch stops by itself.
pub const MAX_FAILURES: u32 = 6;
/// A watch stops by itself after this long.
pub const MAX_AGE_SECS: i64 = 7 * 24 * 60 * 60;
/// Watching at once, across every thread.
pub const MAX_WATCHES: usize = 10;
/// Events kept on a watch, newest last.
pub const MAX_EVENTS: usize = 20;
/// Watches one round checks at most, so a long list never bursts at GitHub.
pub const PER_ROUND: usize = 3;

const STORE_FILE: &str = "pull_watches.json";
const LOCK_FILE: &str = "pull_watches.lock";

/// What the reader asked to hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interests {
    pub checks: bool,
    pub comments: bool,
    pub conflicts: bool,
    pub closed: bool,
}

impl Default for Interests {
    fn default() -> Self {
        Self {
            checks: true,
            comments: true,
            conflicts: true,
            closed: true,
        }
    }
}

/// What a check saw, to compare the next one against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Snapshot {
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub head: String,
    pub failing: BTreeSet<String>,
    pub pending: usize,
    pub passed: usize,
    /// The newest review or comment's time; later ones are new.
    pub last_activity: String,
    pub conflicting: bool,
}

impl Snapshot {
    /// The snapshot of a `gh pr view --json` answer.
    pub fn from_view(value: &Value) -> Option<Self> {
        let detail = crate::pull_requests::detail(value)?;
        let state = match detail.summary.state {
            crate::pull_requests::PullState::Open => "open",
            crate::pull_requests::PullState::Closed => "closed",
            crate::pull_requests::PullState::Merged => "merged",
        };
        Some(Self {
            state: state.to_owned(),
            head: detail.summary.head_sha.clone(),
            failing: detail
                .check_runs
                .iter()
                .filter(|check| check.status == "failed")
                .map(|check| check.name.clone())
                .collect(),
            pending: detail.summary.checks.pending,
            passed: detail.summary.checks.passed,
            last_activity: detail
                .activity
                .iter()
                .map(|activity| activity.at.clone())
                .max()
                .unwrap_or_default(),
            conflicting: value.get("mergeable").and_then(Value::as_str) == Some("CONFLICTING"),
        })
    }
}

/// One change worth telling the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchEvent {
    /// `checks_failed`, `checks_passed`, `activity`, `conflicts` or `closed`.
    pub kind: String,
    pub message: String,
    pub at: String,
}

/// The changes from `old` to `new` the reader asked for. `activity` is the
/// authors of reviews and comments newer than `old` saw.
pub fn changes(
    old: &Snapshot,
    new: &Snapshot,
    authors: &[String],
    interests: Interests,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if interests.checks {
        // A new commit's checks start over: its failures are all new.
        let before: BTreeSet<String> = if old.head == new.head {
            old.failing.clone()
        } else {
            BTreeSet::new()
        };
        let failed: Vec<&str> = new
            .failing
            .difference(&before)
            .map(String::as_str)
            .collect();
        if !failed.is_empty() {
            out.push((
                "checks_failed".to_owned(),
                format!("Checks failed: {}", failed.join(", ")),
            ));
        }
        let settled = new.failing.is_empty() && new.pending == 0 && new.passed > 0;
        let was_settled =
            old.head == new.head && old.failing.is_empty() && old.pending == 0 && old.passed > 0;
        if settled && !was_settled {
            out.push(("checks_passed".to_owned(), "All checks passed".to_owned()));
        }
    }
    if interests.comments && new.last_activity > old.last_activity && !authors.is_empty() {
        let mut names: Vec<&str> = authors.iter().map(String::as_str).collect();
        names.dedup();
        let count = authors.len();
        out.push((
            "activity".to_owned(),
            format!(
                "{count} new review{} or comment{} from {}",
                if count == 1 { "" } else { "s" },
                if count == 1 { "" } else { "s" },
                names.join(", ")
            ),
        ));
    }
    if interests.conflicts && new.conflicting && !old.conflicting {
        out.push((
            "conflicts".to_owned(),
            "Has merge conflicts with its base".to_owned(),
        ));
    }
    if interests.closed && new.state != old.state && new.state != "open" {
        out.push((
            "closed".to_owned(),
            if new.state == "merged" {
                "Merged".to_owned()
            } else {
                "Closed without merging".to_owned()
            },
        ));
    }
    out
}

/// The authors of reviews and comments after `since`, oldest first.
pub fn authors_since(value: &Value, since: &str) -> Vec<String> {
    let Some(detail) = crate::pull_requests::detail(value) else {
        return Vec::new();
    };
    detail
        .activity
        .iter()
        .filter(|activity| activity.at.as_str() > since)
        .map(|activity| activity.author.clone())
        .collect()
}

/// One watch: a thread's interest in one pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    pub session_id: String,
    pub cwd: PathBuf,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub interests: Interests,
    #[serde(default)]
    pub snapshot: Option<Snapshot>,
    #[serde(default)]
    pub events: Vec<WatchEvent>,
    /// `watching` or `stopped`.
    pub status: String,
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// Unix seconds; a stopped watch is forgotten a week later.
    #[serde(default)]
    pub stopped_at: Option<i64>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    pub next_check_at: i64,
}

impl Watch {
    pub fn is_watching(&self) -> bool {
        self.status == "watching"
    }

    fn same(&self, other: &Watch) -> bool {
        self.session_id == other.session_id
            && self.number == other.number
            && self.created_at == other.created_at
    }

    fn stop(&mut self, reason: impl Into<String>) {
        self.status = "stopped".to_owned();
        self.stop_reason = Some(reason.into());
        self.stopped_at = Some(chrono::Utc::now().timestamp());
    }

    fn push_event(&mut self, kind: String, message: String, now: i64) {
        self.events.push(WatchEvent {
            kind,
            message,
            at: chrono::DateTime::from_timestamp(now, 0)
                .map(|time| time.to_rfc3339())
                .unwrap_or_default(),
        });
        let extra = self.events.len().saturating_sub(MAX_EVENTS);
        self.events.drain(..extra);
    }

    /// Take one check's answer: what changed, now that `value` was read at
    /// `now`. The first answer is the baseline and reports nothing, unless
    /// the pull request is already closed.
    pub fn apply(&mut self, value: &Value, now: i64) -> Vec<WatchEvent> {
        let Some(new) = Snapshot::from_view(value) else {
            return self.fail("gh answered without a pull request".to_owned(), now);
        };
        if let Some(title) = value.get("title").and_then(Value::as_str) {
            self.title = title.to_owned();
        }
        if let Some(url) = value.get("url").and_then(Value::as_str) {
            self.url = url.to_owned();
        }
        let before = self.events.len();
        match &self.snapshot {
            Some(old) => {
                let authors = authors_since(value, &old.last_activity);
                for (kind, message) in changes(old, &new, &authors, self.interests) {
                    self.push_event(kind, message, now);
                }
            }
            None if new.state != "open" => {
                self.push_event(
                    "closed".to_owned(),
                    if new.state == "merged" {
                        "Already merged"
                    } else {
                        "Already closed"
                    }
                    .to_owned(),
                    now,
                );
            }
            None => {}
        }
        if new.state != "open" {
            self.stop(if new.state == "merged" {
                "merged"
            } else {
                "closed"
            });
        }
        self.snapshot = Some(new);
        self.failures = 0;
        self.last_error = None;
        self.next_check_at = now + INTERVAL_SECS;
        self.events[before..].to_vec()
    }

    /// Take a failed check: wait longer each time, and stop after
    /// [`MAX_FAILURES`] in a row. An answer that says the pull request or
    /// GitHub access is gone stops at once.
    pub fn fail(&mut self, message: String, now: i64) -> Vec<WatchEvent> {
        self.failures += 1;
        self.last_error = Some(message.clone());
        let backoff = INTERVAL_SECS.saturating_mul(1 << self.failures.min(10));
        self.next_check_at = now + backoff.min(MAX_BACKOFF_SECS);
        if self.failures >= MAX_FAILURES {
            self.stop(format!(
                "GitHub could not be read {MAX_FAILURES} times in a row: {message}"
            ));
            self.push_event(
                "stopped".to_owned(),
                "Stopped watching: GitHub could not be read".to_owned(),
                now,
            );
            return self.events.last().cloned().into_iter().collect();
        }
        Vec::new()
    }

    /// Stop a watch that ran past [`MAX_AGE_SECS`].
    pub fn expire(&mut self, now: i64) -> bool {
        if self.is_watching() && now - self.created_at >= MAX_AGE_SECS {
            self.stop("watched for 7 days");
            return true;
        }
        false
    }
}

/// The watch list on disk.
#[derive(Debug, Clone)]
pub struct WatchStore {
    dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    watches: Vec<Watch>,
}

impl WatchStore {
    /// The store under `config_dir/storage`.
    pub fn new(config_dir: &Path) -> Self {
        Self {
            dir: config_dir.join("storage"),
        }
    }

    /// The store in the config directory, when there is one.
    pub fn default_store() -> Option<Self> {
        crate::config::get_config_dir().map(|dir| Self::new(&dir))
    }

    pub fn load(&self) -> Vec<Watch> {
        std::fs::read(self.dir.join(STORE_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<StoreFile>(&bytes).ok())
            .map(|file| file.watches)
            .unwrap_or_default()
    }

    pub fn save(&self, watches: &[Watch]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let temporary = self
            .dir
            .join(format!("{STORE_FILE}.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&StoreFile {
            watches: watches.to_vec(),
        })
        .map_err(std::io::Error::other)?;
        std::fs::write(&temporary, bytes)?;
        crate::atomic_file::replace_file(&temporary, &self.dir.join(STORE_FILE))
    }

    /// Change the list under a short in-process lock and save it.
    fn update<T>(&self, change: impl FnOnce(&mut Vec<Watch>) -> T) -> std::io::Result<T> {
        static EDIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = EDIT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut watches = self.load();
        let out = change(&mut watches);
        // Stopped watches are kept a week for the thread to show, then dropped.
        let now = chrono::Utc::now().timestamp();
        watches.retain(|watch| {
            watch
                .stopped_at
                .is_none_or(|stopped| now - stopped < MAX_AGE_SECS)
        });
        self.save(&watches)?;
        Ok(out)
    }

    /// Start watching, or start again with fresh state: the first check is
    /// the baseline, so nothing that already happened is reported.
    pub fn start(
        &self,
        session_id: &str,
        cwd: &Path,
        number: u64,
        interests: Interests,
        now: i64,
    ) -> Result<Watch, String> {
        self.update(|watches| {
            watches.retain(|watch| !(watch.session_id == session_id && watch.number == number));
            if watches.iter().filter(|watch| watch.is_watching()).count() >= MAX_WATCHES {
                return Err(format!(
                    "At most {MAX_WATCHES} pull requests are watched at once; stop one first."
                ));
            }
            let watch = Watch {
                session_id: session_id.to_owned(),
                cwd: cwd.to_owned(),
                number,
                title: String::new(),
                url: String::new(),
                interests,
                snapshot: None,
                events: Vec::new(),
                status: "watching".to_owned(),
                stop_reason: None,
                stopped_at: None,
                failures: 0,
                last_error: None,
                created_at: now,
                next_check_at: now,
            };
            watches.push(watch.clone());
            Ok(watch)
        })
        .map_err(|error| error.to_string())?
    }

    /// Stop a watch; it stays listed, stopped, so the thread still shows
    /// what it saw. `remove` forgets it instead.
    pub fn stop(
        &self,
        session_id: &str,
        number: u64,
        remove: bool,
    ) -> std::io::Result<Option<Watch>> {
        self.update(|watches| {
            let index = watches
                .iter()
                .position(|watch| watch.session_id == session_id && watch.number == number)?;
            if remove {
                let mut watch = watches.remove(index);
                watch.stop("stopped by you");
                return Some(watch);
            }
            if watches[index].is_watching() {
                watches[index].stop("stopped by you");
            }
            Some(watches[index].clone())
        })
    }

    pub fn list(&self, session_id: Option<&str>) -> Vec<Watch> {
        self.load()
            .into_iter()
            .filter(|watch| session_id.is_none_or(|id| watch.session_id == id))
            .collect()
    }

    /// Put back a watch after a check, unless it was stopped or started
    /// again meanwhile.
    pub fn put(&self, checked: &Watch) -> std::io::Result<bool> {
        self.update(
            |watches| match watches.iter_mut().find(|watch| watch.same(checked)) {
                Some(watch) if watch.is_watching() => {
                    *watch = checked.clone();
                    true
                }
                _ => false,
            },
        )
    }

    /// The watches due a check at `now`, soonest first, at most [`PER_ROUND`].
    pub fn due(&self, now: i64) -> Vec<Watch> {
        let mut due: Vec<Watch> = self
            .load()
            .into_iter()
            .filter(|watch| watch.is_watching() && watch.next_check_at <= now)
            .collect();
        due.sort_by_key(|watch| watch.next_check_at);
        due.truncate(PER_ROUND);
        due
    }

    /// The checking lease: held for as long as the file stays open. `None`
    /// while another engine holds it.
    pub fn lease(&self) -> Option<File> {
        std::fs::create_dir_all(&self.dir).ok()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(LOCK_FILE))
            .ok()?;
        file.try_lock().ok()?;
        Some(file)
    }
}

/// Check what is due, and hand each watch that changed to `changed`. The
/// caller holds the lease.
pub async fn check_due<F>(store: &WatchStore, mut changed: F)
where
    F: FnMut(&Watch, &[WatchEvent]),
{
    let now = chrono::Utc::now().timestamp();
    for mut watch in store.due(now) {
        let events = if watch.expire(now) {
            Vec::new()
        } else {
            match crate::pull_requests::watch_view(&watch.cwd, watch.number).await {
                Ok(value) => watch.apply(&value, chrono::Utc::now().timestamp()),
                Err(error)
                    if matches!(
                        error.code,
                        "not_a_repository" | "not_connected" | "gh_missing"
                    ) =>
                {
                    watch.stop(error.message.clone());
                    Vec::new()
                }
                Err(error) => watch.fail(error.message, chrono::Utc::now().timestamp()),
            }
        };
        if store.put(&watch).unwrap_or(false) {
            changed(&watch, &events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view(state: &str, head: &str, checks: Value, mergeable: &str, comments: Value) -> Value {
        json!({
            "number": 7, "title": "feat: x", "author": {"login": "me"}, "state": state.to_uppercase(),
            "isDraft": false, "headRefName": "x", "baseRefName": "main", "headRefOid": head,
            "updatedAt": "2026-10-30T10:00:00Z", "url": "https://github.com/o/r/pull/7",
            "statusCheckRollup": checks, "mergeable": mergeable, "reviews": [], "comments": comments,
        })
    }

    fn check(name: &str, conclusion: &str) -> Value {
        json!({"__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": conclusion})
    }

    fn watch(store: &WatchStore) -> Watch {
        store
            .start("s1", Path::new("/r"), 7, Interests::default(), 1_000)
            .unwrap()
    }

    #[test]
    fn the_first_check_is_a_baseline_and_later_changes_are_told_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let mut watch = watch(&store);
        let quiet = view(
            "open",
            "a1",
            json!([check("test", "SUCCESS")]),
            "MERGEABLE",
            json!([]),
        );
        assert!(watch.apply(&quiet, 1_000).is_empty());

        let failing = view(
            "open",
            "a1",
            json!([check("test", "FAILURE"), check("lint", "SUCCESS")]),
            "MERGEABLE",
            json!([{"author": {"login": "rev"}, "body": "hm", "createdAt": "2026-10-30T11:00:00Z"}]),
        );
        let events = watch.apply(&failing, 1_200);
        let kinds: Vec<&str> = events.iter().map(|event| event.kind.as_str()).collect();
        assert_eq!(kinds, ["checks_failed", "activity"]);
        assert!(events[1].message.contains("rev"));
        // Seen again, nothing is new.
        assert!(watch.apply(&failing, 1_400).is_empty());

        let conflicting = view(
            "open",
            "a1",
            json!([check("test", "FAILURE")]),
            "CONFLICTING",
            json!([]),
        );
        assert_eq!(watch.apply(&conflicting, 1_600)[0].kind, "conflicts");

        let merged = view(
            "merged",
            "a1",
            json!([check("test", "SUCCESS")]),
            "UNKNOWN",
            json!([]),
        );
        let events = watch.apply(&merged, 1_800);
        assert!(events.iter().any(|event| event.message == "Merged"));
        assert!(!watch.is_watching());
    }

    #[test]
    fn a_new_commit_reports_its_failures_and_recovery() {
        let mut old = Snapshot {
            state: "open".into(),
            head: "a".into(),
            failing: ["test".to_owned()].into(),
            passed: 1,
            ..Snapshot::default()
        };
        let mut new = old.clone();
        new.head = "b".into();
        assert_eq!(
            changes(&old, &new, &[], Interests::default())[0].0,
            "checks_failed"
        );
        old = new.clone();
        new.failing.clear();
        let changed = changes(&old, &new, &[], Interests::default());
        assert_eq!(changed[0].0, "checks_passed");
        let quiet = Interests {
            checks: false,
            ..Interests::default()
        };
        assert!(changes(&old, &new, &[], quiet).is_empty());
    }

    #[test]
    fn failures_back_off_and_stop_the_watch() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let mut watch = watch(&store);
        assert!(watch.fail("offline".into(), 1_000).is_empty());
        assert_eq!(watch.next_check_at, 1_000 + 2 * INTERVAL_SECS);
        for _ in 1..MAX_FAILURES - 1 {
            watch.fail("offline".into(), 1_000);
        }
        assert!(watch.next_check_at <= 1_000 + MAX_BACKOFF_SECS);
        let events = watch.fail("offline".into(), 1_000);
        assert_eq!(events[0].kind, "stopped");
        assert!(!watch.is_watching());
    }

    #[test]
    fn the_store_survives_a_restart_and_a_stop_wins_over_a_late_check() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let started = store
            .start("s1", Path::new("/r"), 7, Interests::default(), now)
            .unwrap();
        // Another engine, or this one after a restart, reads the same list.
        let again = WatchStore::new(dir.path());
        assert_eq!(again.due(now).len(), 1);
        let mut checked = started.clone();
        checked.failures = 1;
        store.stop("s1", 7, false).unwrap();
        assert!(
            !again.put(&checked).unwrap(),
            "a stopped watch is not brought back"
        );
        assert_eq!(
            again.list(Some("s1"))[0].stop_reason.as_deref(),
            Some("stopped by you")
        );
        assert!(again.due(now).is_empty());
        assert!(store.stop("s1", 7, true).unwrap().is_some());
        assert!(store.list(None).is_empty());
    }

    #[test]
    fn watches_are_capped_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        for number in 0..MAX_WATCHES as u64 {
            store
                .start("s", Path::new("/r"), number, Interests::default(), now)
                .unwrap();
        }
        assert!(
            store
                .start("s", Path::new("/r"), 99, Interests::default(), now)
                .is_err()
        );
        assert!(store.due(now).len() <= PER_ROUND);
        let mut watch = store.list(None).remove(0);
        assert!(watch.expire(now + MAX_AGE_SECS));
        assert!(!watch.is_watching());
    }

    #[test]
    fn one_engine_holds_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let held = store.lease();
        assert!(held.is_some());
        assert!(store.lease().is_none());
        drop(held);
        assert!(store.lease().is_some());
    }
}
