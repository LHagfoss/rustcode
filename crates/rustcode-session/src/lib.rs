//! JSON-backed session persistence for RustCode.
//!
//! The store is parameterized by its root directory so it does not depend on
//! the application's configuration loader. This keeps persistence reusable by
//! future frontends while retaining the existing on-disk format and paths.

use rustcode_core::{ChatMessage, History, rebuild_from_compaction_boundary};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, atomic::AtomicU64, atomic::Ordering};
use std::time::Duration;

pub const HISTORY_FILE: &str = "history.json";
pub const SESSIONS_DIR: &str = "sessions";
pub const IMAGE_CACHE_FILE: &str = "image_cache.json";
const HISTORY_WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_SESSIONS: usize = 30;

/// A history input that exposes immutable messages and, when available, a
/// mutation revision for queued-write deduplication.
pub trait HistorySnapshot {
    fn messages(&self) -> &[ChatMessage];
    fn revision(&self) -> Option<u64>;
}

impl HistorySnapshot for History {
    fn messages(&self) -> &[ChatMessage] {
        self.as_slice()
    }

    fn revision(&self) -> Option<u64> {
        Some(History::revision(self))
    }
}

impl HistorySnapshot for Vec<ChatMessage> {
    fn messages(&self) -> &[ChatMessage] {
        self
    }

    fn revision(&self) -> Option<u64> {
        None
    }
}

impl HistorySnapshot for [ChatMessage] {
    fn messages(&self) -> &[ChatMessage] {
        self
    }

    fn revision(&self) -> Option<u64> {
        None
    }
}

struct HistoryWriter {
    pending: Mutex<HashMap<PathBuf, PendingHistoryWrite>>,
    wakeup: Condvar,
    write_slot: Mutex<()>,
}

struct PendingHistoryWrite {
    history: Vec<ChatMessage>,
    revision: Option<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn history_writer() -> &'static HistoryWriter {
    static WRITER: OnceLock<&'static HistoryWriter> = OnceLock::new();
    WRITER.get_or_init(|| {
        let writer: &'static HistoryWriter = Box::leak(Box::new(HistoryWriter {
            pending: Mutex::new(HashMap::new()),
            wakeup: Condvar::new(),
            write_slot: Mutex::new(()),
        }));
        let _ = std::thread::Builder::new()
            .name("history-writer".to_string())
            .spawn(move || {
                loop {
                    {
                        let mut pending = lock(&writer.pending);
                        while pending.is_empty() {
                            pending = writer
                                .wakeup
                                .wait(pending)
                                .unwrap_or_else(|error| error.into_inner());
                        }
                    }
                    std::thread::sleep(HISTORY_WRITE_DEBOUNCE);
                    drain_history_writes(writer);
                }
            });
        writer
    })
}

fn drain_history_writes(writer: &HistoryWriter) {
    let _slot = lock(&writer.write_slot);
    let batch = std::mem::take(&mut *lock(&writer.pending));
    for (path, pending) in batch {
        write_history_file(&path, &pending.history);
    }
}

pub fn flush_history() {
    drain_history_writes(history_writer());
}

pub fn next_session_id_value(now: u64, previous: u64) -> u64 {
    now.max(previous.saturating_add(1))
}

pub fn next_session_id() -> String {
    static LAST_SESSION_ID: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let mut previous = LAST_SESSION_ID.load(Ordering::Relaxed);
    loop {
        let candidate = next_session_id_value(now, previous);
        match LAST_SESSION_ID.compare_exchange_weak(
            previous,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return candidate.to_string(),
            Err(actual) => previous = actual,
        }
    }
}

pub fn queue_history_write(path: PathBuf, history: &[ChatMessage], revision: Option<u64>) -> bool {
    let writer = history_writer();
    let mut pending = lock(&writer.pending);
    if revision.is_some() && pending.get(&path).and_then(|write| write.revision) == revision {
        return false;
    }
    pending.insert(
        path,
        PendingHistoryWrite {
            history: history.to_vec(),
            revision,
        },
    );
    writer.wakeup.notify_all();
    true
}

pub fn write_history_file(path: &Path, history: &[ChatMessage]) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(json_str) = serde_json::to_string(history) else {
        return;
    };
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    if std::fs::write(&tmp, json_str).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A saved chat session on disk, listed by `/history` and `/resume`.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub path: PathBuf,
    pub title: String,
    pub when: String,
    pub message_count: usize,
}

#[derive(Deserialize)]
struct ChatMessageMetaRef<'a> {
    #[serde(borrow)]
    role: std::borrow::Cow<'a, str>,
    #[serde(borrow, default)]
    content: std::borrow::Cow<'a, str>,
    #[serde(borrow, default)]
    timestamp: std::borrow::Cow<'a, str>,
}

#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.root.join(SESSIONS_DIR).join(session_id)
    }

    pub fn session_has_content(history: &[ChatMessage]) -> bool {
        history
            .iter()
            .any(|message| message.role == "user" && !message.content.starts_with('/'))
    }

    pub fn session_is_resumable(history: &[ChatMessage]) -> bool {
        Self::session_has_content(history)
            && history.iter().any(|message| message.role == "assistant")
    }

    pub fn session_title(history: &[ChatMessage]) -> String {
        let title = history
            .iter()
            .find(|message| message.role == "user" && !message.content.starts_with('/'))
            .map(|message| {
                message
                    .content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
            .unwrap_or_else(|| "(no prompt)".to_string());
        if title.chars().count() > 48 {
            format!("{}...", title.chars().take(45).collect::<String>())
        } else {
            title
        }
    }

    pub fn session_id_from_path(path: &Path) -> Option<String> {
        if path.file_name().is_some_and(|name| name == HISTORY_FILE) {
            let parent = path.parent()?;
            if parent
                .parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|component| component == SESSIONS_DIR)
            {
                return parent
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned);
            }
        } else if path
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|component| component == SESSIONS_DIR)
        {
            return path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
        }
        None
    }

    pub fn load_session_meta(&self, path: &Path) -> Option<SessionMeta> {
        let content = std::fs::read_to_string(path).ok()?;
        let messages: Vec<ChatMessageMetaRef<'_>> = serde_json::from_str(&content).ok()?;
        let has_user = messages
            .iter()
            .any(|message| message.role == "user" && !message.content.starts_with('/'));
        let has_assistant = messages.iter().any(|message| message.role == "assistant");
        if !has_user || !has_assistant {
            return None;
        }

        let title = Self::session_id_from_path(path)
            .as_deref()
            .and_then(|id| self.load_session_title(id))
            .unwrap_or_else(|| {
                let first_user = messages
                    .iter()
                    .find(|message| message.role == "user" && !message.content.starts_with('/'))
                    .map(|message| message.content.lines().next().unwrap_or("").trim())
                    .unwrap_or("(no prompt)");
                if first_user.chars().count() > 48 {
                    format!("{}...", first_user.chars().take(45).collect::<String>())
                } else if first_user.is_empty() {
                    "(no prompt)".to_string()
                } else {
                    first_user.to_string()
                }
            });

        Some(SessionMeta {
            title,
            when: messages
                .first()
                .map(|message| message.timestamp.to_string())
                .unwrap_or_default(),
            message_count: messages.len(),
            path: path.to_path_buf(),
        })
    }

    pub fn session_id_has_content(&self, session_id: &str) -> bool {
        let path = self.session_dir(session_id).join(HISTORY_FILE);
        let Ok(content) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(messages) = serde_json::from_str::<Vec<ChatMessageMetaRef<'_>>>(&content) else {
            return false;
        };
        messages
            .iter()
            .any(|message| message.role == "user" && !message.content.starts_with('/'))
    }

    pub fn save_history<H: HistorySnapshot + ?Sized>(
        &self,
        active_session_id: Option<&str>,
        history: &H,
    ) {
        match active_session_id.filter(|id| !id.is_empty()) {
            Some(session_id) => self.save_session_history(session_id, history),
            None => {
                queue_history_write(
                    self.root.join(HISTORY_FILE),
                    history.messages(),
                    history.revision(),
                );
            }
        }
    }

    pub fn save_session_history<H: HistorySnapshot + ?Sized>(&self, session_id: &str, history: &H) {
        queue_history_write(
            self.session_dir(session_id).join(HISTORY_FILE),
            history.messages(),
            history.revision(),
        );
    }

    pub fn save_session_title(&self, session_id: &str, title: &str) {
        let session_dir = self.session_dir(session_id);
        let _ = std::fs::create_dir_all(&session_dir);
        let _ = std::fs::write(session_dir.join("title.txt"), title);
    }

    pub fn load_session_title(&self, session_id: &str) -> Option<String> {
        let path = self.session_dir(session_id).join("title.txt");
        path.exists()
            .then(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|value| value.trim().to_string())
            })
            .flatten()
    }

    pub fn load_session_history_direct(&self, session_id: &str) -> Vec<ChatMessage> {
        self.load_session_file(&self.session_dir(session_id).join(HISTORY_FILE))
    }

    pub fn save_session_image_cache(&self, session_id: &str, cache: &HashMap<String, String>) {
        if cache.is_empty() {
            return;
        }
        let session_dir = self.session_dir(session_id);
        let _ = std::fs::create_dir_all(&session_dir);
        if let Ok(json) = serde_json::to_string_pretty(cache) {
            let _ = std::fs::write(session_dir.join(IMAGE_CACHE_FILE), json);
        }
    }

    pub fn load_session_image_cache(&self, session_id: &str) -> HashMap<String, String> {
        std::fs::read_to_string(self.session_dir(session_id).join(IMAGE_CACHE_FILE))
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn get_active_session_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id)
    }

    pub fn get_active_session_sandbox_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("sandbox")
    }

    pub fn get_active_session_artifacts_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("artifacts")
    }

    pub fn create_subagent_workspace(
        &self,
        session_id: &str,
        agent_id: u32,
    ) -> Result<PathBuf, String> {
        let root = self
            .session_dir(session_id)
            .join("subagents")
            .join(format!("agent-{agent_id}"));
        if root.exists() {
            return Ok(root);
        }
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create subagent directory: {error}"))?;
        }
        let repo =
            std::env::current_dir().map_err(|error| format!("resolve repository: {error}"))?;
        let status = std::process::Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&root)
            .arg("HEAD")
            .current_dir(repo)
            .status()
            .map_err(|error| format!("create git worktree: {error}"))?;
        if !status.success() {
            return Err(format!("git worktree add exited with {status}"));
        }
        Ok(root)
    }

    pub fn write_subagent_review_manifest(workspace: &Path, agent_id: u32) -> Option<PathBuf> {
        let output = std::process::Command::new("git")
            .args(["status", "--short"])
            .current_dir(workspace)
            .output()
            .ok()?;
        let manifest = workspace
            .parent()
            .unwrap_or(workspace)
            .join(format!("agent-{agent_id}-review.txt"));
        let mut content = format!("Subagent {agent_id} workspace review manifest\n\n");
        content.push_str("Changed paths (git status --short):\n");
        content.push_str(&String::from_utf8_lossy(&output.stdout));
        std::fs::write(&manifest, content).ok()?;
        Some(manifest)
    }

    pub fn archive_session(&self, history: &[ChatMessage]) -> Option<PathBuf> {
        if !Self::session_has_content(history) {
            return None;
        }
        let session_dir = self.root.join(SESSIONS_DIR).join(next_session_id());
        std::fs::create_dir_all(&session_dir).ok()?;
        std::fs::create_dir_all(session_dir.join("sandbox")).ok()?;
        std::fs::create_dir_all(session_dir.join("artifacts")).ok()?;
        let path = session_dir.join(HISTORY_FILE);
        let json = serde_json::to_string_pretty(history).ok()?;
        std::fs::write(&path, json).ok()?;
        self.prune_sessions();
        Some(path)
    }

    fn prune_sessions(&self) {
        let Ok(entries) = std::fs::read_dir(self.root.join(SESSIONS_DIR)) else {
            return;
        };
        let mut targets = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() && path.join(HISTORY_FILE).exists() {
                targets.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                targets.push(path);
            }
        }
        if targets.len() <= MAX_SESSIONS {
            return;
        }
        targets.sort();
        for old in &targets[..targets.len() - MAX_SESSIONS] {
            if old.is_dir() {
                let _ = std::fs::remove_dir_all(old);
            } else {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    pub fn sorted_session_paths(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.root.join(SESSIONS_DIR)) else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() && path.join(HISTORY_FILE).exists() {
                paths.push(path.join(HISTORY_FILE));
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                paths.push(path);
            }
        }
        paths.sort();
        paths.reverse();
        paths
    }

    pub fn latest_resumable_session_meta(&self) -> Option<SessionMeta> {
        self.sorted_session_paths()
            .into_iter()
            .find_map(|path| self.load_session_meta(&path))
    }

    pub fn session_meta_by_id(&self, id: &str) -> Option<SessionMeta> {
        let nested = self.session_dir(id).join(HISTORY_FILE);
        if nested.exists() {
            self.load_session_meta(&nested)
        } else {
            let flat = self.root.join(SESSIONS_DIR).join(format!("{id}.json"));
            flat.exists()
                .then(|| self.load_session_meta(&flat))
                .flatten()
        }
    }

    pub fn list_sessions_limited(&self, limit: usize) -> (Vec<SessionMeta>, bool) {
        let mut list = Vec::new();
        let mut truncated = false;
        for path in self.sorted_session_paths() {
            if let Some(meta) = self.load_session_meta(&path) {
                if list.len() < limit {
                    list.push(meta);
                } else {
                    truncated = true;
                    break;
                }
            }
        }
        (list, truncated)
    }

    pub fn list_sessions(&self) -> Vec<SessionMeta> {
        self.list_sessions_limited(usize::MAX).0
    }

    pub fn load_session_file(&self, path: &Path) -> Vec<ChatMessage> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Vec<ChatMessage>>(&content).ok())
            .map(rebuild_from_compaction_boundary)
            .unwrap_or_default()
    }

    pub fn delete_session_file(path: &Path) {
        if path.file_name().is_some_and(|name| name == HISTORY_FILE) {
            if let Some(parent) = path.parent()
                && parent
                    .parent()
                    .is_some_and(|root| root.ends_with(SESSIONS_DIR))
            {
                let _ = std::fs::remove_dir_all(parent);
            }
        } else if path
            .parent()
            .is_some_and(|parent| parent.ends_with(SESSIONS_DIR))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> ChatMessage {
        ChatMessage::new(role, content)
    }

    #[test]
    fn persisted_history_round_trips_with_session_paths() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let history = vec![
            message("user", "inspect files"),
            message("assistant", "done"),
        ];
        store.save_session_history("123", &history);
        flush_history();
        let path = store.session_dir("123").join(HISTORY_FILE);
        assert_eq!(store.load_session_file(&path), history);
        assert_eq!(
            SessionStore::session_id_from_path(&path).as_deref(),
            Some("123")
        );
    }

    #[test]
    fn metadata_and_title_keep_legacy_rules() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let history = vec![
            message("user", "a real prompt"),
            message("assistant", "answer"),
        ];
        store.save_session_history("abc", &history);
        flush_history();
        let path = store.session_dir("abc").join(HISTORY_FILE);
        assert_eq!(
            store.load_session_meta(&path).unwrap().title,
            "a real prompt"
        );
        store.save_session_title("abc", "Custom title");
        assert_eq!(
            store.load_session_meta(&path).unwrap().title,
            "Custom title"
        );
    }

    #[test]
    fn resumed_history_rebuilds_from_the_persisted_compaction_anchor() {
        use rustcode_core::{CompactionBoundary, CompactionEntry, ToolCallRef};

        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let retained_call =
            ChatMessage::new("assistant", "inspect recent module").with_tool_calls(vec![
                ToolCallRef {
                    id: "call-recent".to_string(),
                    name: "view_file".to_string(),
                    arguments: "{\"path\":\"src/recent.rs\"}".to_string(),
                },
            ]);
        let summary = ChatMessage::new("system", "[Session History Summary]\nprior facts")
            .with_compaction_boundary(CompactionBoundary {
                version: 1,
                summary: "prior facts".to_string(),
                first_retained_entry: Some(CompactionEntry::from_message(&retained_call)),
            });
        let retained_result = ChatMessage::new("tool", "view_file: recent contents")
            .answering(Some("call-recent".to_string()));
        let history = vec![
            message("user", "old summarized request"),
            summary,
            message("assistant", "stale entry from an interrupted write"),
            retained_call.clone(),
            retained_result.clone(),
        ];

        store.save_session_history("resume", &history);
        flush_history();

        let resumed = store.load_session_history_direct("resume");
        assert_eq!(resumed.len(), 3);
        assert_eq!(resumed[1], retained_call);
        assert_eq!(resumed[2], retained_result);
        assert!(
            resumed
                .iter()
                .all(|entry| !entry.content.contains("old summarized")
                    && !entry.content.contains("stale entry"))
        );
    }

    #[test]
    fn image_cache_uses_session_scoped_json_file() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let cache = HashMap::from([(String::from("hash"), String::from("result"))]);
        store.save_session_image_cache("abc", &cache);
        assert_eq!(store.load_session_image_cache("abc"), cache);
    }
}
