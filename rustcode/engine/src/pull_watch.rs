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
//!
//! A *link* (#3369) is a watch an agent keeps on a pull request until it
//! merges, closes, is unlinked, GitHub stops answering or its limit runs out;
//! the [`answer`] function is the `manage_pull_links` tool, which links,
//! inspects and unlinks one. Every thread that links the same pull request is
//! checked by one `gh` read per round, and each thread is handed each event
//! once ([`Watch::pending`], [`WatchStore::take`]), so nothing repeats.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How often a watch is checked while GitHub answers.
pub const INTERVAL_SECS: i64 = 120;
/// The longest wait between checks after failures.
pub const MAX_BACKOFF_SECS: i64 = 30 * 60;
/// Failed checks in a row before a watch stops by itself.
pub const MAX_FAILURES: u32 = 6;
/// A watch stops by itself after this long.
pub const MAX_AGE_SECS: i64 = 7 * 24 * 60 * 60;
/// Manual watches at once, across every thread. Links have their own budget
/// ([`MAX_LINKS`]) and do not take these slots.
pub const MAX_WATCHES: usize = 10;
/// Events kept on a watch, newest last.
pub const MAX_EVENTS: usize = 20;
/// Pull requests one round checks at most, so a long list never bursts at
/// GitHub: three reads every 30 s, 360 an hour at worst, well inside a
/// GitHub token's 5000.
pub const PER_ROUND: usize = 3;
/// A link stops by itself after two weeks: a pull request still open then is
/// linked again by the thread that still cares, rather than polled forever.
pub const MAX_LINK_AGE_SECS: i64 = 14 * 24 * 60 * 60;
/// Links one thread keeps at once.
pub const MAX_LINKS_PER_THREAD: usize = 5;
/// Links watched at once, across every thread.
pub const MAX_LINKS: usize = 20;
/// The tool an agent links, inspects and unlinks a pull request with (#3369).
pub const LINK_TOOL_NAME: &str = "manage_pull_links";
/// What that tool does.
pub const LINK_ACTIONS: [&str; 3] = ["link", "status", "unlink"];

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
    /// `owner/name` when the watch names its repository, empty when it reads
    /// the pull request through `cwd`.
    #[serde(default)]
    pub repo: String,
    /// Started as a link (#3369) the thread keeps, rather than a one-off
    /// watch: unchecked for longer, and read by the `manage_pull_links` tool.
    #[serde(default)]
    pub link: bool,
    /// Events already handed to the thread; the ones after it are new to it,
    /// so an event is never delivered twice.
    #[serde(default)]
    pub delivered: usize,
    /// Unix seconds of the last answer that could be read.
    #[serde(default)]
    pub checked_at: Option<i64>,
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

    /// The pull request this watch follows: one entry per repository and
    /// number, however many threads follow it.
    fn key(&self) -> (String, u64) {
        let repo = if self.repo.is_empty() {
            self.cwd.to_string_lossy().into_owned()
        } else {
            self.repo.clone()
        };
        (repo, self.number)
    }

    /// How long this watch keeps checking before it stops by itself.
    pub fn limit_secs(&self) -> i64 {
        if self.link {
            MAX_LINK_AGE_SECS
        } else {
            MAX_AGE_SECS
        }
    }

    fn past_limit(&self, now: i64) -> bool {
        self.is_watching() && now - self.created_at >= self.limit_secs()
    }

    /// What the thread has not been told yet.
    pub fn pending(&self) -> &[WatchEvent] {
        &self.events[self.delivered.min(self.events.len())..]
    }

    /// Mark every event so far as told to the thread.
    fn deliver(&mut self) {
        self.delivered = self.events.len();
    }

    fn stop(&mut self, reason: impl Into<String>) {
        self.status = "stopped".to_owned();
        self.stop_reason = Some(reason.into());
        self.stopped_at = Some(chrono::Utc::now().timestamp());
    }

    fn push_event(&mut self, kind: String, message: String, now: i64) -> WatchEvent {
        let event = WatchEvent {
            kind,
            message,
            at: chrono::DateTime::from_timestamp(now, 0)
                .map(|time| time.to_rfc3339())
                .unwrap_or_default(),
        };
        self.events.push(event.clone());
        let extra = self.events.len().saturating_sub(MAX_EVENTS);
        self.events.drain(..extra);
        // The cursor counts from the front, so it moves with the dropped ones.
        self.delivered = self.delivered.saturating_sub(extra);
        event
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
        // The events this answer produced, exactly the ones pushed here: an
        // older event dropped off the front of `events` must not take a new one
        // with it, or the check would report nothing to deliver.
        let mut fresh = Vec::new();
        match &self.snapshot {
            Some(old) => {
                let authors = authors_since(value, &old.last_activity);
                for (kind, message) in changes(old, &new, &authors, self.interests) {
                    fresh.push(self.push_event(kind, message, now));
                }
            }
            None if new.state != "open" => {
                fresh.push(
                    self.push_event(
                        "closed".to_owned(),
                        if new.state == "merged" {
                            "Already merged"
                        } else {
                            "Already closed"
                        }
                        .to_owned(),
                        now,
                    ),
                );
            }
            None if self.link => {
                for (kind, message) in changes(&Snapshot::default(), &new, &[], self.interests) {
                    fresh.push(self.push_event(kind, message, now));
                }
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
        // A read that works again after failed ones is news too: the thread
        // learns its link recovered instead of wondering about the gap.
        let failed = self.failures;
        self.failures = 0;
        self.last_error = None;
        self.checked_at = Some(now);
        self.next_check_at = now + INTERVAL_SECS;
        if failed > 0 {
            fresh.push(self.push_event(
                "sync_recovered".to_owned(),
                format!(
                    "GitHub reads work again after {failed} failed check{}",
                    if failed == 1 { "" } else { "s" }
                ),
                now,
            ));
        }
        fresh
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
            return vec![self.push_event(
                "stopped".to_owned(),
                "Stopped watching: GitHub could not be read".to_owned(),
                now,
            )];
        }
        Vec::new()
    }

    /// Stop a watch GitHub will not answer again (the folder is no repository,
    /// there is no sign-in, or `gh` is missing), and tell the thread why once:
    /// a link that silently stops is one the thread waits on for nothing.
    pub fn stop_unreadable(&mut self, message: String, now: i64) -> Vec<WatchEvent> {
        self.failures += 1;
        self.last_error = Some(message.clone());
        self.stop(message.clone());
        vec![self.push_event(
            "stopped".to_owned(),
            format!("Stopped watching: {message}"),
            now,
        )]
    }

    /// Stop a watch that ran past its limit: [`MAX_LINK_AGE_SECS`] for a link,
    /// [`MAX_AGE_SECS`] for a one-off watch. The thread hears why, once.
    pub fn expire(&mut self, now: i64) -> Vec<WatchEvent> {
        if !self.past_limit(now) {
            return Vec::new();
        }
        let reason = if self.link {
            "linked for 14 days"
        } else {
            "watched for 7 days"
        };
        self.stop(reason);
        vec![self.push_event(
            "stopped".to_owned(),
            format!("Stopped watching: {reason}"),
            now,
        )]
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
            // Restarting a manual watch replaces it; a link the thread keeps
            // is a different record (`repo`, its own limit) and stays.
            watches.retain(|watch| {
                !(!watch.link && watch.session_id == session_id && watch.number == number)
            });
            // Links are their own budget (`MAX_LINKS`): a thread that linked a
            // pull request must not fill the manual watches' slots.
            if watches
                .iter()
                .filter(|watch| watch.is_watching() && !watch.link)
                .count()
                >= MAX_WATCHES
            {
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
                repo: String::new(),
                link: false,
                delivered: 0,
                checked_at: None,
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

    /// Link a thread to a pull request (#3369), kept until it merges, closes,
    /// is unlinked, GitHub stops answering or [`MAX_LINK_AGE_SECS`] runs out.
    /// `value` is the answer read by the caller, so the thread learns the
    /// state now; the first answer is the baseline and reports nothing that
    /// was already true.
    pub fn link(
        &self,
        session_id: &str,
        cwd: &Path,
        repo: &str,
        number: u64,
        value: &Value,
        now: i64,
    ) -> Result<Watch, String> {
        let repo = repo.trim().to_owned();
        self.update(|watches| {
            // Linking again is not a new link: what the thread saw stays seen,
            // and a stopped link starts checking again. A manual watch of the
            // same pull request is a different record and is left alone.
            if let Some(existing) = watches.iter_mut().find(|watch| {
                watch.link
                    && watch.session_id == session_id
                    && watch.number == number
                    && watch.repo == repo
            }) {
                if existing.is_watching() {
                    return Ok(existing.clone());
                }
                existing.status = "watching".to_owned();
                existing.stop_reason = None;
                existing.stopped_at = None;
                existing.failures = 0;
                existing.last_error = None;
                existing.link = true;
                existing.created_at = now;
                existing.next_check_at = now;
                existing.apply(value, now);
                return Ok(existing.clone());
            }
            if watches
                .iter()
                .filter(|watch| watch.link && watch.is_watching())
                .count()
                >= MAX_LINKS
            {
                return Err(format!(
                    "At most {MAX_LINKS} pull requests are linked at once, across every thread; unlink one first."
                ));
            }
            if watches
                .iter()
                .filter(|watch| {
                    watch.link && watch.is_watching() && watch.session_id == session_id
                })
                .count()
                >= MAX_LINKS_PER_THREAD
            {
                return Err(format!(
                    "A thread keeps at most {MAX_LINKS_PER_THREAD} linked pull requests; unlink one first."
                ));
            }
            let mut watch = Watch {
                session_id: session_id.to_owned(),
                cwd: cwd.to_owned(),
                number,
                title: String::new(),
                url: String::new(),
                interests: Interests::default(),
                snapshot: None,
                events: Vec::new(),
                repo,
                link: true,
                delivered: 0,
                checked_at: None,
                status: "watching".to_owned(),
                stop_reason: None,
                stopped_at: None,
                failures: 0,
                last_error: None,
                created_at: now,
                next_check_at: now,
            };
            watch.apply(value, now);
            watches.push(watch.clone());
            Ok(watch)
        })
        .map_err(|error| error.to_string())?
    }

    /// What the thread has not been told about its link to `number`, handed
    /// over once (#3369): the same event is never reported twice.
    pub fn take(
        &self,
        session_id: &str,
        number: u64,
    ) -> std::io::Result<Option<(Watch, Vec<WatchEvent>)>> {
        self.update(|watches| {
            let watch = watches.iter_mut().find(|watch| {
                watch.session_id == session_id && watch.number == number && watch.link
            })?;
            let events = watch.pending().to_vec();
            watch.deliver();
            Some((watch.clone(), events))
        })
    }

    /// The thread's links, one pull request each, newest first.
    pub fn links(&self, session_id: &str) -> Vec<Watch> {
        let mut links: Vec<Watch> = self
            .load()
            .into_iter()
            .filter(|watch| watch.link && watch.session_id == session_id)
            .collect();
        links.sort_by_key(|watch| std::cmp::Reverse(watch.created_at));
        links
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

    /// Forget a thread's link to `number`, and only its link: a manual watch
    /// of the same pull request from the same thread stays.
    pub fn unlink(&self, session_id: &str, number: u64) -> std::io::Result<Option<Watch>> {
        self.update(|watches| {
            let index = watches.iter().position(|watch| {
                watch.link && watch.session_id == session_id && watch.number == number
            })?;
            let mut watch = watches.remove(index);
            watch.stop("unlinked");
            Some(watch)
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

    /// The pull requests due a check at `now`, soonest first: one entry per
    /// pull request, holding every thread that follows it, so a pull request
    /// several threads link is read from GitHub once (#3369).
    pub fn due_groups(&self, now: i64) -> Vec<Vec<Watch>> {
        let mut watching: Vec<Watch> = self
            .load()
            .into_iter()
            .filter(|watch| watch.is_watching())
            .collect();
        watching.sort_by_key(|watch| watch.next_check_at);
        let mut groups: Vec<((String, u64), Vec<Watch>)> = Vec::new();
        for watch in watching {
            if watch.next_check_at > now {
                break;
            }
            let key = watch.key();
            match groups.iter_mut().position(|(other, _)| *other == key) {
                Some(at) => groups[at].1.push(watch),
                None if groups.len() < PER_ROUND => groups.push((key, vec![watch])),
                // Past this round's reads: the next round takes it, soon.
                None => {}
            }
        }
        groups.into_iter().map(|(_, group)| group).collect()
    }

    /// The watches due a check at `now`, soonest first.
    pub fn due(&self, now: i64) -> Vec<Watch> {
        self.due_groups(now).into_iter().flatten().collect()
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
/// caller holds the lease. A pull request that several threads watch is read
/// once, and every one of them gets the same answer (#3369).
pub async fn check_due<F>(store: &WatchStore, mut changed: F)
where
    F: FnMut(&Watch, &[WatchEvent]),
{
    let now = chrono::Utc::now().timestamp();
    for group in store.due_groups(now) {
        // A link past its limit is stopped without asking GitHub; only a
        // watch still inside one is worth a read.
        let answer = if group.iter().any(|watch| !watch.past_limit(now)) {
            Some(read(&group).await)
        } else {
            None
        };
        for mut watch in group {
            let mut events = if watch.past_limit(now) {
                watch.expire(now)
            } else {
                match &answer {
                    Some(Ok(value)) => watch.apply(value, chrono::Utc::now().timestamp()),
                    Some(Err(error))
                        if matches!(
                            error.code,
                            "not_a_repository" | "no_provider" | "not_connected" | "gh_missing"
                        ) =>
                    {
                        watch.stop_unreadable(error.message.clone(), chrono::Utc::now().timestamp())
                    }
                    Some(Err(error)) => {
                        watch.fail(error.message.clone(), chrono::Utc::now().timestamp())
                    }
                    None => Vec::new(),
                }
            };
            if events.iter().any(|event| event.kind == "checks_failed") {
                if let Some(Ok(value)) = &answer {
                    let tail = failed_log_tail(&watch.cwd, &watch.repo, value).await;
                    if !tail.is_empty() {
                        for event in events
                            .iter_mut()
                            .filter(|event| event.kind == "checks_failed")
                        {
                            event.message.push_str(&format!("\n{tail}"));
                            if let Some(saved) = watch
                                .events
                                .iter_mut()
                                .find(|saved| saved.at == event.at && saved.kind == event.kind)
                            {
                                saved.message = event.message.clone();
                            }
                        }
                    }
                }
            }
            if let Some(snapshot) = watch.snapshot.as_ref() {
                crate::data_store::record_thread_pull_request(
                    &watch.session_id,
                    &watch.url,
                    &snapshot.state,
                    &snapshot.head,
                );
            }
            if store.put(&watch).unwrap_or(false) {
                changed(&watch, &events);
            }
        }
    }
}

/// One `gh` read for a pull request, through the first folder of the group
/// that can answer: a link outlives a worktree another thread followed it in.
async fn read(group: &[Watch]) -> Result<Value, crate::pull_requests::PullError> {
    let number = group.first().map(|watch| watch.number).unwrap_or_default();
    let repo = group
        .first()
        .map(|watch| watch.repo.as_str())
        .unwrap_or_default();
    let mut error = None;
    for folder in read_folders(group) {
        match crate::pull_requests::watch_view_on(&folder, repo, number).await {
            Ok(value) => return Ok(value),
            // Not this folder's repository: another one of the group's may be.
            Err(failed) if matches!(failed.code, "not_a_repository" | "no_provider") => {
                error = Some(failed)
            }
            Err(failed) => return Err(failed),
        }
    }
    Err(error.unwrap_or_else(|| crate::pull_requests::PullError {
        code: "failed",
        message: "there is no folder to read the pull request in".to_owned(),
    }))
}

/// The folders one group's read tries, in order. A folder that was removed
/// cannot run `gh` at all (and `gh` would report that as a missing binary), so
/// it is left to the others. A link names its repository and `gh --repo`
/// answers from any folder, so with no folder left the engine's own is used; a
/// manual watch reads through its own folder and gets no stand-in, since
/// `gh pr view` there would read another repository's pull request.
fn read_folders(group: &[Watch]) -> Vec<PathBuf> {
    let repo = group
        .first()
        .map(|watch| watch.repo.as_str())
        .unwrap_or_default();
    let mut folders: Vec<PathBuf> = Vec::new();
    for watch in group {
        if watch.cwd.is_dir() && !folders.contains(&watch.cwd) {
            folders.push(watch.cwd.clone());
        }
    }
    if folders.is_empty()
        && crate::pull_requests::is_repository(repo)
        && let Ok(cwd) = std::env::current_dir()
    {
        folders.push(cwd);
    }
    folders
}

/// A pull request number the caller sent as a number or as digits.
fn number_of(args: &Value) -> Result<Option<u64>, String> {
    let Some(value) = args.get("number") else {
        return Ok(None);
    };
    let number = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()));
    match number {
        Some(number) if number > 0 => Ok(Some(number)),
        _ => Err("number must be a pull request number, 1 or more".to_owned()),
    }
}

// Fetch diagnostics only for a newly observed failure, never on every round.
async fn failed_log_tail(cwd: &Path, repo: &str, value: &Value) -> String {
    let Some(detail) = crate::pull_requests::detail(value) else {
        return String::new();
    };
    let mut tails = Vec::new();
    let mut seen = BTreeSet::new();
    for check in detail
        .check_runs
        .iter()
        .filter(|check| check.status == "failed")
    {
        let Some(url) = &check.url else { continue };
        let Some(run) = url
            .split("/actions/runs/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
        else {
            continue;
        };
        if run.is_empty()
            || !run.bytes().all(|b| b.is_ascii_digit())
            || !seen.insert(run.to_owned())
        {
            continue;
        }
        if seen.len() > 3 {
            break;
        }
        let mut args = vec!["run", "view", run, "--log-failed"];
        if !repo.is_empty() {
            args.extend(["--repo", repo]);
        }
        if let Ok(bytes) = crate::pull_requests::gh(cwd, &args, None).await {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<_> = text.lines().rev().take(20).collect();
            tails.push(
                lines
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
                    .chars()
                    .take(6000)
                    .collect::<String>(),
            );
        }
    }
    tails.join("\n")
}

/// The `manage_pull_links` answer (#3369), shared by the built-in tool and the
/// orchestrator MCP server, so both say the same thing.
///
/// `link` records the thread's interest and reads the pull request once, so
/// the answer holds its state now. `status` reports the state last read and
/// hands over the events the thread has not seen, once each. `unlink` stops
/// checking and forgets the link, which is also what merging or closing does
/// by itself.
pub async fn answer(args: &Value, session_id: &str, cwd: &Path) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if !LINK_ACTIONS.contains(&action.as_str()) {
        return Err(format!("action must be one of {}", LINK_ACTIONS.join(", ")));
    }
    let store = WatchStore::default_store()
        .ok_or("there is no config directory to keep pull request links in")?;
    let now = chrono::Utc::now().timestamp();
    match action.as_str() {
        "link" => {
            let number = number_of(args)?.ok_or("number is required to link a pull request")?;
            let repo = args
                .get("repo")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !repo.is_empty() && !crate::pull_requests::is_repository(repo) {
                return Err(format!("repo must be owner/name, not '{repo}'"));
            }
            let value = crate::pull_requests::watch_view_on(cwd, repo, number)
                .await
                .map_err(|error| {
                    format!("could not read pull request #{number}: {}", error.message)
                })?;
            let linked = store.link(session_id, cwd, repo, number, &value, now)?;
            let handed = store
                .take(session_id, number)
                .map_err(|error| error.to_string())?
                .unwrap_or((linked, Vec::new()));
            let (watch, events) = handed;
            Ok(serde_json::to_string_pretty(&json!({
                "link": describe(&watch, &events),
                "detail": "Linked. This thread hears about checks, reviews, conflicts, and the pull request merging or closing; `status` hands over what it has not seen yet.",
            }))
            .unwrap_or_default())
        }
        "status" => {
            let mut links = Vec::new();
            match number_of(args)? {
                Some(number) => {
                    let Some((watch, events)) = store
                        .take(session_id, number)
                        .map_err(|error| error.to_string())?
                    else {
                        return Err(format!("this thread has no link to pull request #{number}"));
                    };
                    links.push(describe(&watch, &events));
                }
                None => {
                    for watch in store.links(session_id) {
                        let number = watch.number;
                        let handed = store
                            .take(session_id, number)
                            .map_err(|error| error.to_string())?
                            .unwrap_or((watch, Vec::new()));
                        links.push(describe(&handed.0, &handed.1));
                    }
                }
            }
            Ok(serde_json::to_string_pretty(&json!({ "links": links })).unwrap_or_default())
        }
        _ => {
            let number = number_of(args)?.ok_or("number is required to unlink a pull request")?;
            let removed = store
                .unlink(session_id, number)
                .map_err(|error| error.to_string())?;
            let unlinked = removed.is_some();
            Ok(serde_json::to_string_pretty(&json!({
                "number": number,
                "unlinked": unlinked,
                "detail": if unlinked {
                    "Stopped checking and forgot the link."
                } else {
                    "This thread had no link to that pull request."
                },
            }))
            .unwrap_or_default())
        }
    }
}

/// One link as the tool reports it, with the events handed over now.
pub fn describe(watch: &Watch, events: &[WatchEvent]) -> Value {
    let snapshot = watch.snapshot.as_ref();
    json!({
        "repo": (!watch.repo.is_empty()).then(|| watch.repo.clone()),
        "number": watch.number,
        "title": watch.title,
        "url": watch.url,
        "status": watch.status,
        "stopReason": watch.stop_reason,
        "state": snapshot.map(|snapshot| snapshot.state.clone()),
        "checks": snapshot.map(|snapshot| json!({
            "failing": snapshot.failing.iter().collect::<Vec<&String>>(),
            "pending": snapshot.pending,
            "passed": snapshot.passed,
        })),
        "conflicts": snapshot.map(|snapshot| snapshot.conflicting),
        "lastActivity": snapshot.map(|snapshot| snapshot.last_activity.clone()),
        "checkedAt": watch.checked_at.and_then(at),
        "failures": watch.failures,
        "lastError": watch.last_error,
        "expiresAt": at(watch.created_at + watch.limit_secs()),
        "events": events,
    })
}

fn at(seconds: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(seconds, 0).map(|time| time.to_rfc3339())
}

/// The links whose news this engine has not passed on to its clients yet.
///
/// Only the engine holding the checking lease reads GitHub, and it tells its
/// own clients. When that is another process (a `rustcode server` service, a
/// second app window's engine), this engine's clients would hear nothing
/// until they reconnected and listed the watches again: every event arrived
/// at once after an app restart, as if typed then. Each round, an engine
/// without the lease passes on what the store holds for a link and its thread
/// has not been handed. `announced` remembers how many events it already
/// passed on per link, so a round adds a notification only for news. The
/// client claims events through [`WatchStore::take`], so a notification the
/// checking engine also sent repeats nothing.
pub fn unannounced(
    watches: &[Watch],
    announced: &mut std::collections::HashMap<(String, String, u64), usize>,
) -> Vec<(Watch, Vec<WatchEvent>)> {
    let key = |watch: &Watch| (watch.session_id.clone(), watch.repo.clone(), watch.number);
    announced.retain(|known, _| watches.iter().any(|watch| key(watch) == *known));
    let mut out = Vec::new();
    for watch in watches.iter().filter(|watch| watch.link) {
        let pending = watch.pending();
        if pending.is_empty() {
            announced.remove(&key(watch));
            continue;
        }
        let total = watch.events.len();
        if announced.insert(key(watch), total) == Some(total) {
            continue;
        }
        out.push((watch.clone(), pending.to_vec()));
    }
    out
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
        let events = watch.expire(now + MAX_AGE_SECS);
        assert!(!watch.is_watching());
        assert_eq!(events[0].kind, "stopped", "a watch that runs out says so");
        assert!(events[0].message.contains("7 days"));
    }

    fn open() -> Value {
        view(
            "open",
            "a1",
            json!([check("test", "SUCCESS")]),
            "MERGEABLE",
            json!([]),
        )
    }

    // These lifecycle tests start before CI settles. Already-green linking is
    // covered separately by linking_after_checks_finish_reports_the_result_once.
    fn linked(store: &WatchStore, session: &str, cwd: &str, now: i64) -> Watch {
        let pending = view(
            "open",
            "a1",
            json!([check("test", "PENDING")]),
            "MERGEABLE",
            json!([]),
        );
        store
            .link(session, Path::new(cwd), "o/r", 7, &pending, now)
            .unwrap()
    }

    #[test]
    fn linking_after_checks_finish_reports_the_result_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let value = view(
            "open",
            "abc",
            json!([{"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS"}]),
            "MERGEABLE",
            json!([]),
        );
        store
            .link("s", Path::new("/repo"), "o/r", 7, &value, 100)
            .unwrap();
        let (_, events) = store.take("s", 7).unwrap().unwrap();
        assert_eq!(events[0].kind, "checks_passed");
        store
            .link("s", Path::new("/repo"), "o/r", 7, &value, 101)
            .unwrap();
        assert!(store.take("s", 7).unwrap().unwrap().1.is_empty());
    }

    #[test]
    fn a_link_is_kept_with_its_state_and_the_thread_is_told_each_change_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let link = linked(&store, "s1", "/repo", now);
        assert!(link.link && link.is_watching());
        assert_eq!(
            link.snapshot
                .as_ref()
                .map(|snapshot| snapshot.state.as_str()),
            Some("open")
        );
        assert_eq!(link.checked_at, Some(now));
        assert!(
            link.events.is_empty(),
            "a link with pending checks has no result to report yet"
        );

        // Another engine, or this one after a restart, reads the same link.
        let again = WatchStore::new(dir.path());
        let restored = again.links("s1");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].repo, "o/r");

        // A failing check is news, and is handed over exactly once.
        let mut watch = restored[0].clone();
        let failing = view(
            "open",
            "a1",
            json!([check("test", "FAILURE")]),
            "MERGEABLE",
            json!([]),
        );
        assert_eq!(watch.apply(&failing, now + 120)[0].kind, "checks_failed");
        assert!(again.put(&watch).unwrap());
        let (told, events) = again.take("s1", 7).unwrap().expect("a link");
        assert_eq!(events.len(), 1);
        assert!(told.pending().is_empty());
        let (_, again_events) = again.take("s1", 7).unwrap().expect("a link");
        assert!(again_events.is_empty(), "an event is never replayed");
    }

    #[test]
    fn threads_sharing_one_pull_request_are_checked_once_and_each_told_the_change() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        linked(&store, "s1", "/a", now);
        linked(&store, "s2", "/b", now);

        // Two threads, two worktrees, one pull request: one read per round.
        let groups = store.due_groups(now + INTERVAL_SECS);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);

        // The same answer reaches each thread, and each hears it once.
        let failing = view(
            "open",
            "a1",
            json!([check("test", "FAILURE")]),
            "MERGEABLE",
            json!([]),
        );
        for mut watch in store.due_groups(now + INTERVAL_SECS).remove(0) {
            watch.apply(&failing, now + INTERVAL_SECS);
            store.put(&watch).unwrap();
        }
        for session in ["s1", "s2"] {
            let (_, events) = store.take(session, 7).unwrap().expect("a link");
            assert_eq!(events[0].kind, "checks_failed");
            assert!(store.take(session, 7).unwrap().unwrap().1.is_empty());
        }
    }

    #[test]
    fn a_link_is_capped_per_thread_and_across_threads() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        for number in 1..=MAX_LINKS_PER_THREAD as u64 {
            store
                .link("s1", Path::new("/r"), "o/r", number, &open(), now)
                .unwrap();
        }
        assert!(
            store
                .link("s1", Path::new("/r"), "o/r", 99, &open(), now)
                .is_err(),
            "a thread keeps a handful"
        );
        // Other threads keep theirs until everyone's limit is reached.
        let mut created = MAX_LINKS_PER_THREAD;
        let mut session = 2;
        let mut number = 100;
        while created < MAX_LINKS {
            store
                .link(
                    &format!("s{session}"),
                    Path::new("/r"),
                    "o/r",
                    number,
                    &open(),
                    now,
                )
                .unwrap();
            created += 1;
            number += 1;
            if created.is_multiple_of(MAX_LINKS_PER_THREAD) {
                session += 1;
            }
        }
        assert!(
            store
                .link("s9", Path::new("/r"), "o/r", 999, &open(), now)
                .is_err()
        );
    }

    #[test]
    fn linking_again_keeps_what_the_thread_was_told() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        linked(&store, "s1", "/repo", now);
        let mut watch = store.links("s1").remove(0);
        let failing = view(
            "open",
            "a1",
            json!([check("test", "FAILURE")]),
            "MERGEABLE",
            json!([]),
        );
        watch.apply(&failing, now + 120);
        store.put(&watch).unwrap();
        let relinked = store
            .link("s1", Path::new("/repo"), "o/r", 7, &open(), now + 240)
            .unwrap();
        assert_eq!(relinked.events.len(), 1, "what it saw stays seen");
        assert_eq!(relinked.created_at, now, "a running link is not restarted");
    }

    #[test]
    fn a_link_stops_once_the_pull_request_merges_or_closes_and_unlinking_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut watch = linked(&store, "s1", "/repo", now);
        let merged = view(
            "merged",
            "a1",
            json!([check("test", "SUCCESS")]),
            "UNKNOWN",
            json!([]),
        );
        watch.apply(&merged, now + 120);
        assert!(!watch.is_watching());
        assert_eq!(watch.stop_reason.as_deref(), Some("merged"));
        assert_eq!(
            watch
                .pending()
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["checks_passed", "closed"]
        );
        store.put(&watch).unwrap();
        assert!(store.due(now + 10 * INTERVAL_SECS).is_empty());
        assert!(store.stop("s1", 7, true).unwrap().is_some());
        assert!(store.links("s1").is_empty());
    }

    #[test]
    fn an_unreadable_github_backs_off_and_its_recovery_is_told() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut watch = linked(&store, "s1", "/repo", now);
        assert!(watch.fail("gh timed out".into(), now + 120).is_empty());
        assert_eq!(watch.failures, 1);
        assert_eq!(watch.last_error.as_deref(), Some("gh timed out"));
        assert_eq!(watch.next_check_at, now + 120 + 2 * INTERVAL_SECS);

        // The next read that works clears the failure and says so once.
        let events = watch.apply(&open(), now + 240);
        assert_eq!(events[0].kind, "sync_recovered");
        assert!(events[0].message.contains("1 failed check"));
        assert_eq!(watch.failures, 0);
        assert!(watch.last_error.is_none());
        assert!(watch.apply(&open(), now + 360).is_empty());

        // Six failures in a row stop the link, with the reason kept.
        for _ in 0..MAX_FAILURES {
            watch.fail("offline".into(), now);
        }
        assert!(!watch.is_watching());
        assert!(watch.stop_reason.is_some_and(|reason| reason.contains('6')));
    }

    #[test]
    fn a_link_expires_after_two_weeks_and_a_watch_after_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut link = linked(&store, "s1", "/repo", now);
        assert_eq!(link.limit_secs(), MAX_LINK_AGE_SECS);
        assert!(link.expire(now + MAX_AGE_SECS).is_empty());
        let events = link.expire(now + MAX_LINK_AGE_SECS);
        assert_eq!(link.stop_reason.as_deref(), Some("linked for 14 days"));
        assert_eq!(events[0].kind, "stopped");
        assert!(events[0].message.contains("14 days"));

        let mut watch = watch(&store);
        assert_eq!(watch.expire(now + MAX_AGE_SECS).len(), 1);
        assert_eq!(watch.stop_reason.as_deref(), Some("watched for 7 days"));
    }

    #[test]
    fn an_older_store_without_links_still_loads_and_a_link_reports_itself() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        // A watch written before links existed has none of their fields.
        std::fs::create_dir_all(dir.path().join("storage")).unwrap();
        std::fs::write(
            dir.path().join("storage/pull_watches.json"),
            json!({
                "watches": [{
                    "sessionId": "s1", "cwd": "/r", "number": 7,
                    "status": "watching", "createdAt": 1_000, "nextCheckAt": 1_000
                }]
            })
            .to_string(),
        )
        .unwrap();
        let watches = store.list(None);
        assert_eq!(watches.len(), 1);
        assert!(!watches[0].link);
        assert_eq!(watches[0].pending().len(), 0);
        assert_eq!(watches[0].limit_secs(), MAX_AGE_SECS);

        let now = chrono::Utc::now().timestamp();
        let link = linked(&store, "s2", "/repo", now);
        let described = describe(&link, &[]);
        assert_eq!(described["repo"], "o/r");
        assert_eq!(described["number"], 7);
        assert_eq!(described["state"], "open");
        assert_eq!(described["status"], "watching");
        assert!(described["expiresAt"].is_string());
    }

    #[test]
    fn a_number_is_taken_as_a_number_or_digits_and_a_repo_must_be_owner_name() {
        assert_eq!(number_of(&json!({"number": 7})).unwrap(), Some(7));
        assert_eq!(number_of(&json!({"number": "7"})).unwrap(), Some(7));
        assert_eq!(number_of(&json!({})).unwrap(), None);
        assert!(number_of(&json!({"number": 0})).is_err());
        assert!(number_of(&json!({"number": "seven"})).is_err());
        assert!(crate::pull_requests::is_repository("LHagfoss/rustcode"));
        assert!(crate::pull_requests::is_repository("o/r.mirror"));
        assert!(!crate::pull_requests::is_repository("/Users/me/repo"));
        assert!(!crate::pull_requests::is_repository("rustcode"));
        assert!(!crate::pull_requests::is_repository("o/"));
        assert!(!crate::pull_requests::is_repository("/r"));
        assert!(!crate::pull_requests::is_repository("o/r/extra"));
        assert!(!crate::pull_requests::is_repository("../r"));
    }

    #[test]
    fn a_change_at_the_event_cap_is_still_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut watch = linked(&store, "s1", "/repo", now);
        // The list is full, so the oldest events drop as new ones arrive.
        for n in 0..MAX_EVENTS {
            watch.push_event("activity".into(), format!("old {n}"), now);
        }
        watch.deliver();
        let failing = view(
            "open",
            "a1",
            json!([check("test", "FAILURE")]),
            "MERGEABLE",
            json!([]),
        );
        let events = watch.apply(&failing, now + INTERVAL_SECS);
        assert_eq!(events.len(), 1, "the change is what this check produced");
        assert_eq!(events[0].kind, "checks_failed");
        assert_eq!(
            watch.events.len(),
            MAX_EVENTS,
            "the oldest went, not the newest"
        );
        assert_eq!(
            watch.pending().len(),
            1,
            "and it is still there to hand over"
        );
    }

    #[test]
    fn a_link_and_a_manual_watch_of_one_pull_request_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        linked(&store, "s1", "/repo", now);
        // The app watches the same pull request from the same thread: the
        // manual watch neither replaces the link nor is mistaken for it.
        store
            .start("s1", Path::new("/repo"), 7, Interests::default(), now)
            .unwrap();
        assert_eq!(store.list(Some("s1")).len(), 2);
        let stopped = store.unlink("s1", 7).unwrap().expect("the link");
        assert_eq!(stopped.stop_reason.as_deref(), Some("unlinked"));
        let left = store.list(Some("s1"));
        assert_eq!(left.len(), 1, "unlinking forgets only the link");
        assert!(!left[0].link);
        assert!(left[0].is_watching());
        assert!(store.unlink("s1", 7).unwrap().is_none());
    }

    #[test]
    fn links_do_not_fill_the_manual_watch_slots() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut number = 1;
        let mut session = 0;
        let mut created = 0;
        while created < MAX_LINKS {
            store
                .link(
                    &format!("s{session}"),
                    Path::new("/r"),
                    "o/r",
                    number,
                    &open(),
                    now,
                )
                .unwrap();
            number += 1;
            created += 1;
            if created.is_multiple_of(MAX_LINKS_PER_THREAD) {
                session += 1;
            }
        }
        assert!(
            store
                .start("s9", Path::new("/r"), 7, Interests::default(), now)
                .is_ok(),
            "the manual budget is its own"
        );
    }

    #[test]
    fn a_link_github_will_not_answer_stops_with_a_reason_and_one_event() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let mut watch = linked(&store, "s1", "/repo", now);
        let events = watch.stop_unreadable("The GitHub CLI (gh) is not installed".into(), now);
        assert!(!watch.is_watching());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "stopped");
        assert!(events[0].message.contains("not installed"));
        assert_eq!(
            watch.last_error.as_deref(),
            Some("The GitHub CLI (gh) is not installed")
        );
    }

    #[test]
    fn a_read_falls_back_to_the_engine_folder_only_for_a_named_repository() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let now = chrono::Utc::now().timestamp();
        let gone = dir.path().join("gone");
        let link = store.link("s1", &gone, "o/r", 7, &open(), now).unwrap();
        // The named repository is read from anywhere: the removed worktree is
        // no reason to stop a link.
        assert_eq!(
            read_folders(&[link.clone()]),
            std::env::current_dir()
                .map(|cwd| vec![cwd])
                .unwrap_or_default()
        );
        // A manual watch has no repository to name, so with its folder gone it
        // is left with nothing to read rather than another repository's #7.
        let mut manual = link.clone();
        manual.repo = String::new();
        assert!(read_folders(&[manual]).is_empty());
        // A link whose folder still exists reads there.
        let mut live = link;
        live.cwd = dir.path().to_path_buf();
        assert_eq!(read_folders(&[live]), vec![dir.path().to_path_buf()]);
    }

    #[test]
    fn one_engine_holds_the_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let held = store.lease();
        assert!(held.is_some());
        assert!(store.lease().is_none());
        drop(held);
        // A child another test forks while the lease is held shares its flock
        // until the child execs (#2360, #2763): wait for it under a full run.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.lease().is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the lease stayed held after its holder let go"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn an_engine_without_the_lease_passes_on_link_news_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = WatchStore::new(dir.path());
        let mut link = watch(&store);
        link.link = true;
        let mut plain = link.clone();
        plain.link = false;
        plain.session_id = "s2".to_owned();
        let event = |kind: &str| WatchEvent {
            kind: kind.to_owned(),
            message: kind.to_owned(),
            at: String::new(),
        };
        link.events = vec![event("checks_passed")];
        plain.events = vec![event("checks_passed")];
        let mut announced = std::collections::HashMap::new();

        // News on a link is passed on; a one-off watch is not a link.
        let first = unannounced(&[link.clone(), plain.clone()], &mut announced);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, vec![event("checks_passed")]);
        // The same news is not passed on again the next round.
        assert!(unannounced(&[link.clone(), plain.clone()], &mut announced).is_empty());
        // A new event is, with everything the thread still has not been handed.
        link.events.push(event("closed"));
        let second = unannounced(&[link.clone()], &mut announced);
        assert_eq!(second[0].1, vec![event("checks_passed"), event("closed")]);
        // Once handed over, nothing is pending and nothing is passed on.
        link.delivered = link.events.len();
        assert!(unannounced(&[link.clone()], &mut announced).is_empty());
        assert!(announced.is_empty());
    }
}
