//! JSON-backed session persistence for RustCode.
//!
//! The store is parameterized by its root directory so it does not depend on
//! the application's configuration loader. This keeps persistence reusable by
//! future frontends while retaining the existing on-disk format and paths.

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use rustcode_core::{ChatMessage, History, rebuild_from_compaction_boundary};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, atomic::AtomicU64, atomic::Ordering};
use std::time::Duration;

pub const HISTORY_FILE: &str = "history.json";
pub const SESSIONS_DIR: &str = "sessions";
pub const IMAGE_CACHE_FILE: &str = "image_cache.json";
pub const SESSION_METADATA_FILE: &str = "metadata.json";
pub const SESSION_METADATA_SCHEMA_VERSION: u32 = 1;
const HISTORY_WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_SESSIONS: usize = 30;

mod workspace;
pub use workspace::*;

/// Remove composer paste framing from display titles, including old titles
/// truncated before the closing marker. Conversation/input text stays intact.
pub fn unwrap_title_paste_markers(raw: &str) -> std::borrow::Cow<'_, str> {
    const MARKER: &str = "<!--PASTE:";
    if !raw.contains(MARKER) {
        return std::borrow::Cow::Borrowed(raw);
    }
    let mut text = String::new();
    let mut rest = raw;
    while let Some(index) = rest.find(MARKER) {
        text.push_str(&rest[..index]);
        let after = &rest[index + MARKER.len()..];
        let (payload, remaining) = after.split_once("-->").unwrap_or((after, ""));
        if let Some((_, body)) = payload.split_once(':') {
            text.push_str(body);
        } else {
            text.push_str("Pasted text");
        }
        rest = remaining;
    }
    text.push_str(rest);
    std::borrow::Cow::Owned(text)
}

fn title_from_prompt(content: &str) -> String {
    let text = unwrap_title_paste_markers(content);
    let title = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if title.chars().count() > 48 {
        format!("{}...", title.chars().take(45).collect::<String>())
    } else {
        title.to_string()
    }
}

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
    static LAST_SESSION_MILLIS: AtomicU64 = AtomicU64::new(0);
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let mut previous = LAST_SESSION_MILLIS.load(Ordering::Relaxed);
    loop {
        let candidate = next_session_id_value(now, previous);
        match LAST_SESSION_MILLIS.compare_exchange_weak(
            previous,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let process = std::process::id() as u64;
                let entropy = sequence
                    .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                    .wrapping_add(process.rotate_left(17))
                    .wrapping_add(candidate.rotate_left(29));
                // UUIDv7-shaped: the first 48 bits are milliseconds since the
                // epoch, followed by the version and RFC 9562 variant bits.
                return format!(
                    "{:012x}-7{:03x}-{:04x}-{:04x}-{:012x}",
                    candidate & 0x0000_ffff_ffff_ffff,
                    sequence & 0xfff,
                    0x8000 | ((entropy >> 48) as u16 & 0x3fff),
                    (entropy >> 32) as u16,
                    entropy & 0x0000_ffff_ffff_ffff,
                );
            }
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

/// Versioned information stored alongside a canonical session transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub schema_version: u32,
    pub id: String,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrated_from: Option<String>,
}

impl SessionMetadata {
    fn new(id: &str, created_at_ms: u64, migrated_from: Option<String>) -> Self {
        Self {
            schema_version: SESSION_METADATA_SCHEMA_VERSION,
            id: id.to_string(),
            created_at_ms,
            migrated_from,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMigrationReport {
    pub dry_run: bool,
    pub found: usize,
    pub migrated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl SessionMigrationReport {
    pub fn changed(&self) -> bool {
        self.migrated > 0
    }
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

    /// Return the canonical date-partitioned location for a session ID.
    pub fn canonical_session_dir(&self, session_id: &str) -> PathBuf {
        let (year, month, day) = session_date_parts(session_id).unwrap_or_else(current_date_parts);
        self.root
            .join(SESSIONS_DIR)
            .join(format!("{year:04}"))
            .join(format!("{month:02}"))
            .join(format!("{day:02}"))
            .join(session_id)
    }

    /// Resolve a session directory while retaining the old direct-directory
    /// layout when it already exists. New IDs therefore use the canonical
    /// date-partitioned layout without moving active legacy workspaces.
    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        let canonical = self.canonical_session_dir(session_id);
        if canonical.is_dir() {
            return canonical;
        }
        let legacy = self.root.join(SESSIONS_DIR).join(session_id);
        if legacy.is_dir() { legacy } else { canonical }
    }

    /// Create a new session directory and its compatibility subdirectories.
    pub fn ensure_session(&self, session_id: &str) -> PathBuf {
        let directory = self.session_dir(session_id);
        let _ = std::fs::create_dir_all(&directory);
        let _ = std::fs::create_dir_all(directory.join("sandbox"));
        let _ = std::fs::create_dir_all(directory.join("artifacts"));
        self.write_metadata_if_missing(&directory, session_id, None);
        directory
    }

    fn write_metadata_if_missing(
        &self,
        directory: &Path,
        session_id: &str,
        migrated_from: Option<String>,
    ) {
        let path = directory.join(SESSION_METADATA_FILE);
        if path.exists() {
            return;
        }
        let metadata = SessionMetadata::new(
            session_id,
            session_timestamp_ms(session_id).unwrap_or_else(current_time_ms),
            migrated_from,
        );
        let Ok(json) = serde_json::to_string_pretty(&metadata) else {
            return;
        };
        let _ = std::fs::write(path, json);
    }

    fn history_path_for_id(&self, session_id: &str) -> PathBuf {
        let canonical = self.canonical_session_dir(session_id).join(HISTORY_FILE);
        if canonical.exists() {
            return canonical;
        }
        let legacy_dir = self
            .root
            .join(SESSIONS_DIR)
            .join(session_id)
            .join(HISTORY_FILE);
        if legacy_dir.exists() {
            return legacy_dir;
        }
        let legacy_file = self
            .root
            .join(SESSIONS_DIR)
            .join(format!("{session_id}.json"));
        if legacy_file.exists() {
            return legacy_file;
        }
        self.session_dir(session_id).join(HISTORY_FILE)
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
        history
            .iter()
            .find(|message| message.role == "user" && !message.content.starts_with('/'))
            .map(|message| title_from_prompt(&message.content))
            .unwrap_or_else(|| "(no prompt)".to_string())
    }

    pub fn session_id_from_path(path: &Path) -> Option<String> {
        if path.file_name().is_some_and(|name| name == HISTORY_FILE) {
            let parent = path.parent()?;
            let is_legacy_directory = parent
                .parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|component| component == SESSIONS_DIR);
            let is_partitioned_directory = parent
                .parent()
                .and_then(|day| day.parent())
                .and_then(|month| month.parent())
                .and_then(|year| year.parent())
                .and_then(|sessions| sessions.file_name())
                .is_some_and(|component| component == SESSIONS_DIR);
            if is_legacy_directory || is_partitioned_directory {
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
                let title = messages
                    .iter()
                    .find(|message| message.role == "user" && !message.content.starts_with('/'))
                    .map(|message| title_from_prompt(&message.content))
                    .unwrap_or_else(|| "(no prompt)".to_string());
                if title.is_empty() {
                    "(no prompt)".to_string()
                } else {
                    title
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
        let path = self.history_path_for_id(session_id);
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
        let path = self.history_path_for_id(session_id);
        if path == self.session_dir(session_id).join(HISTORY_FILE) {
            self.ensure_session(session_id);
        }
        queue_history_write(path, history.messages(), history.revision());
    }

    pub fn save_session_title(&self, session_id: &str, title: &str) {
        let session_dir = self.ensure_session(session_id);
        let _ = std::fs::create_dir_all(&session_dir);
        let _ = std::fs::write(session_dir.join("title.txt"), title);
    }

    pub fn load_session_title(&self, session_id: &str) -> Option<String> {
        let path = self.session_dir(session_id).join("title.txt");
        path.exists()
            .then(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|value| unwrap_title_paste_markers(value.trim()).into_owned())
            })
            .flatten()
    }

    pub fn load_session_history_direct(&self, session_id: &str) -> Vec<ChatMessage> {
        self.load_session_file(&self.history_path_for_id(session_id))
    }

    pub fn save_session_image_cache(&self, session_id: &str, cache: &HashMap<String, String>) {
        if cache.is_empty() {
            return;
        }
        let session_dir = self.ensure_session(session_id);
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

    pub fn workspace_manager(&self) -> WorkspaceManager {
        WorkspaceManager::new(&self.root)
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
        let session_dir = self.ensure_session(&next_session_id());
        let path = session_dir.join(HISTORY_FILE);
        let json = serde_json::to_string_pretty(history).ok()?;
        std::fs::write(&path, json).ok()?;
        self.prune_sessions();
        Some(path)
    }

    fn prune_sessions(&self) {
        let mut targets = self
            .discovered_session_paths()
            .into_iter()
            .map(|path| session_storage_root(&path))
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        if targets.len() <= MAX_SESSIONS {
            return;
        }
        targets.sort_by(|left, right| {
            session_sort_key(left)
                .cmp(&session_sort_key(right))
                .then_with(|| left.cmp(right))
        });
        for old in &targets[..targets.len() - MAX_SESSIONS] {
            if old.is_dir() {
                let _ = std::fs::remove_dir_all(old);
            } else {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    pub fn sorted_session_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.discovered_session_paths();
        paths.sort_by(|left, right| {
            session_sort_key(right)
                .cmp(&session_sort_key(left))
                .then_with(|| right.cmp(left))
        });
        paths
    }

    pub fn latest_resumable_session_meta(&self) -> Option<SessionMeta> {
        self.sorted_session_paths()
            .into_iter()
            .find_map(|path| self.load_session_meta(&path))
    }

    pub fn session_meta_by_id(&self, id: &str) -> Option<SessionMeta> {
        self.sorted_session_paths()
            .into_iter()
            .find(|path| Self::session_id_from_path(path).as_deref() == Some(id))
            .and_then(|path| self.load_session_meta(&path))
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
                && is_session_directory(parent)
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

    fn discovered_session_paths(&self) -> Vec<PathBuf> {
        let sessions = self.root.join(SESSIONS_DIR);
        let Ok(entries) = std::fs::read_dir(&sessions) else {
            return Vec::new();
        };

        let mut paths = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() && path.join(HISTORY_FILE).is_file() {
                // Legacy sessions/<id>/history.json.
                paths.push(path.join(HISTORY_FILE));
                continue;
            }
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                // Legacy sessions/<id>.json.
                paths.push(path);
                continue;
            }
            if !path.is_dir() {
                continue;
            }

            // Canonical sessions/YYYY/MM/DD/<id>/history.json. Walk exactly
            // these four levels so unrelated files below sessions/ are not
            // accidentally treated as transcripts.
            for month in read_directories(&path) {
                for day in read_directories(&month) {
                    for session in read_directories(&day) {
                        let history = session.join(HISTORY_FILE);
                        if history.is_file() {
                            paths.push(history);
                        }
                    }
                }
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn legacy_session_sources(&self) -> Vec<(String, PathBuf)> {
        let sessions = self.root.join(SESSIONS_DIR);
        let Ok(entries) = std::fs::read_dir(&sessions) else {
            return Vec::new();
        };

        let mut sources = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_legacy_session_id(name) {
                continue;
            }
            if path.is_dir() {
                // Include artifact-only and otherwise empty legacy session
                // directories. History discovery intentionally omits those,
                // but migration must preserve their complete subtree.
                sources.push((name.to_owned(), path));
            } else if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                sources.push((name.trim_end_matches(".json").to_owned(), path));
            }
        }
        sources.sort_by(|left, right| left.0.cmp(&right.0));
        sources
    }

    /// Migrate legacy direct session directories and `sessions/*.json` files
    /// into the canonical date-partitioned layout. The operation is safe to
    /// repeat: a source is removed only after its destination is complete.
    pub fn migrate_legacy_sessions(&self, dry_run: bool) -> SessionMigrationReport {
        let sources = self.legacy_session_sources();
        let mut report = SessionMigrationReport {
            dry_run,
            found: sources.len(),
            ..Default::default()
        };

        for (id, source) in sources {
            let destination = self.canonical_session_dir(&id);
            if destination.exists() {
                report.skipped += 1;
                continue;
            }
            if dry_run {
                report.migrated += 1;
                continue;
            }

            let source_root = session_storage_root(&source);
            let temporary =
                destination.with_file_name(format!(".{}.migrating-{}", id, std::process::id()));
            if let Some(parent) = destination.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                report.errors.push(format!("{}: {error}", source.display()));
                continue;
            }
            let _ = std::fs::remove_dir_all(&temporary);
            let result = if source_root.is_dir() {
                copy_directory(&source_root, &temporary)
            } else {
                std::fs::create_dir_all(&temporary)
                    .and_then(|()| std::fs::copy(&source_root, temporary.join(HISTORY_FILE)))
                    .map(|_| ())
            }
            .and_then(|()| {
                std::fs::create_dir_all(temporary.join("logs"))?;
                self.write_metadata_if_missing(
                    &temporary,
                    &id,
                    Some(relative_session_path(&self.root, &source)),
                );
                if destination.exists() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "destination appeared during migration",
                    ));
                }
                std::fs::rename(&temporary, &destination)
            });
            if let Err(error) = result {
                let _ = std::fs::remove_dir_all(&temporary);
                report.errors.push(format!("{}: {error}", source.display()));
                continue;
            }
            let remove_result = if source_root.is_dir() {
                std::fs::remove_dir_all(&source_root)
            } else {
                std::fs::remove_file(&source_root)
            };
            if let Err(error) = remove_result {
                report.errors.push(format!("{}: {error}", source.display()));
                continue;
            }
            report.migrated += 1;
        }
        report
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn current_date_parts() -> (i32, u32, u32) {
    let now = DateTime::<Local>::from(std::time::SystemTime::now());
    (now.year(), now.month(), now.day())
}

fn session_timestamp_ms(session_id: &str) -> Option<u64> {
    if session_id
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        return session_id.parse().ok();
    }
    let compact = session_id.replace('-', "");
    (compact.len() >= 12
        && compact[..12]
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then(|| u64::from_str_radix(&compact[..12], 16).ok())
    .flatten()
}

fn session_date_parts(session_id: &str) -> Option<(i32, u32, u32)> {
    let timestamp = session_timestamp_ms(session_id)?;
    let date = Utc
        .timestamp_millis_opt(timestamp as i64)
        .single()?
        .with_timezone(&Local);
    Some((date.year(), date.month(), date.day()))
}

fn read_directories(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn is_session_directory(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    if path
        .parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|component| component == SESSIONS_DIR)
    {
        return true;
    }
    path.parent()
        .and_then(|day| day.parent())
        .and_then(|month| month.parent())
        .and_then(|year| year.parent())
        .and_then(|sessions| sessions.file_name())
        .is_some_and(|component| component == SESSIONS_DIR)
        && !name.is_empty()
}

fn is_legacy_session_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.contains('/')
        && !(id.len() == 4 && id.chars().all(|character| character.is_ascii_digit()))
}

fn session_storage_root(history_path: &Path) -> PathBuf {
    if history_path
        .file_name()
        .is_some_and(|name| name == HISTORY_FILE)
    {
        history_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| history_path.to_path_buf())
    } else {
        history_path.to_path_buf()
    }
}

fn relative_session_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn session_sort_key(path: &Path) -> (u64, String) {
    let id = SessionStore::session_id_from_path(path).unwrap_or_default();
    let timestamp = session_timestamp_ms(&id)
        .or_else(|| {
            std::fs::metadata(path)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        })
        .unwrap_or(0);
    (timestamp, path.to_string_lossy().into_owned())
}

fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_directory(&from, &to)?;
        } else {
            std::fs::copy(from, to)?;
        }
    }
    Ok(())
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
    fn pasted_titles_are_unwrapped_before_first_line_and_truncation() {
        let body = "\n## Build a Chess MCP Server in Rust\n\nImplement the server.";
        let wrapped = format!("<!--PASTE:{}:{body}-->", body.chars().count());
        let history = vec![message("user", &wrapped), message("assistant", "done")];
        assert_eq!(
            SessionStore::session_title(&history),
            "## Build a Chess MCP Server in Rust"
        );
        assert_eq!(
            history[0].content, wrapped,
            "input folding must remain intact"
        );

        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        store.save_session_history("paste", &history);
        flush_history();
        let path = store.session_dir("paste").join(HISTORY_FILE);
        assert_eq!(
            store.load_session_meta(&path).unwrap().title,
            "## Build a Chess MCP Server in Rust"
        );

        let long = "棋".repeat(60);
        let history = vec![message("user", &format!("<!--PASTE:60:{long}-->"))];
        assert_eq!(
            SessionStore::session_title(&history),
            format!("{}...", "棋".repeat(45))
        );
    }

    #[test]
    fn persisted_truncated_paste_titles_are_readable() {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path());
        store.save_session_title("old", "<!--PASTE:1937:Build a Chess MCP...");
        assert_eq!(
            store.load_session_title("old").as_deref(),
            Some("Build a Chess MCP...")
        );
        assert_eq!(
            unwrap_title_paste_markers("Review <!--PASTE:4:this--> please"),
            "Review this please"
        );
        assert_eq!(unwrap_title_paste_markers("<!--PASTE:1937"), "Pasted text");
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

    fn saved_history() -> Vec<ChatMessage> {
        vec![
            message("user", "legacy prompt"),
            message("assistant", "answer"),
        ]
    }

    fn write_history(path: &Path) {
        let history = serde_json::to_string(&saved_history()).expect("serialize history");
        std::fs::create_dir_all(path.parent().expect("history parent")).expect("parent");
        std::fs::write(path, history).expect("history");
    }

    #[test]
    fn canonical_layout_is_date_partitioned_and_metadata_is_versioned() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let id = "018d6b28-0000-7000-8000-000000000001";

        let expected_date = session_date_parts(id).expect("UUIDv7 timestamp");
        let directory = store.ensure_session(id);
        assert_eq!(
            directory,
            root.path()
                .join(SESSIONS_DIR)
                .join(format!("{:04}", expected_date.0))
                .join(format!("{:02}", expected_date.1))
                .join(format!("{:02}", expected_date.2))
                .join(id)
        );
        let metadata: SessionMetadata = serde_json::from_str(
            &std::fs::read_to_string(directory.join(SESSION_METADATA_FILE)).expect("metadata"),
        )
        .expect("valid metadata");
        assert_eq!(
            SessionStore::session_id_from_path(&directory.join(HISTORY_FILE)).as_deref(),
            Some(id)
        );
        assert_eq!(metadata.schema_version, SESSION_METADATA_SCHEMA_VERSION);
        assert_eq!(metadata.id, id);
    }

    #[test]
    fn legacy_directories_and_flat_files_are_discovered_and_sorted() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let sessions = root.path().join(SESSIONS_DIR);
        write_history(&sessions.join("1704067200000").join(HISTORY_FILE));
        write_history(&sessions.join("1704153600000.json"));

        let paths = store.sorted_session_paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            SessionStore::session_id_from_path(&paths[0]).as_deref(),
            Some("1704153600000")
        );
        assert_eq!(
            SessionStore::session_id_from_path(&paths[1]).as_deref(),
            Some("1704067200000")
        );
        assert_eq!(
            store
                .session_meta_by_id("1704067200000")
                .expect("legacy directory")
                .message_count,
            2
        );
        assert_eq!(
            store
                .session_meta_by_id("1704153600000")
                .expect("legacy flat file")
                .message_count,
            2
        );
    }

    #[test]
    fn migration_supports_dry_run_is_idempotent_and_preserves_tree() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let legacy = root.path().join(SESSIONS_DIR).join("1704067200000");
        write_history(&legacy.join(HISTORY_FILE));
        std::fs::create_dir_all(legacy.join("sandbox")).expect("sandbox");
        std::fs::create_dir_all(legacy.join("artifacts")).expect("artifacts");
        std::fs::create_dir_all(legacy.join("subagents").join("agent-1")).expect("subagent");
        std::fs::write(legacy.join("artifacts").join("result.txt"), "artifact").expect("artifact");
        let flat = root.path().join(SESSIONS_DIR).join("1704153600000.json");
        write_history(&flat);

        let dry_run = store.migrate_legacy_sessions(true);
        assert_eq!(dry_run.found, 2);
        assert_eq!(dry_run.migrated, 2);
        assert!(!store.canonical_session_dir("1704067200000").exists());
        assert!(legacy.exists());
        assert!(flat.exists());

        let applied = store.migrate_legacy_sessions(false);
        assert_eq!(applied.migrated, 2);
        assert!(
            store
                .canonical_session_dir("1704067200000")
                .join("artifacts/result.txt")
                .exists()
        );
        assert!(
            store
                .canonical_session_dir("1704067200000")
                .join(SESSION_METADATA_FILE)
                .exists()
        );
        assert!(!legacy.exists());
        assert!(!flat.exists());

        let repeated = store.migrate_legacy_sessions(false);
        assert_eq!(repeated.found, 0);
        assert_eq!(repeated.migrated, 0);
    }

    #[test]
    fn migration_does_not_overwrite_a_destination_collision() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let id = "1704067200000";
        let legacy = root.path().join(SESSIONS_DIR).join(id);
        write_history(&legacy.join(HISTORY_FILE));
        let destination = store.canonical_session_dir(id);
        write_history(&destination.join(HISTORY_FILE));
        std::fs::write(destination.join("sentinel.txt"), "keep").expect("sentinel");

        let report = store.migrate_legacy_sessions(false);
        assert_eq!(report.found, 1);
        assert_eq!(report.skipped, 1);
        assert!(legacy.exists());
        assert_eq!(
            std::fs::read_to_string(destination.join("sentinel.txt")).expect("sentinel"),
            "keep"
        );
    }

    #[test]
    fn migration_preserves_artifact_only_legacy_sessions() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let legacy = root.path().join(SESSIONS_DIR).join("1704240000000");
        std::fs::create_dir_all(legacy.join("sandbox")).expect("sandbox");
        std::fs::create_dir_all(legacy.join("artifacts")).expect("artifacts");
        std::fs::write(legacy.join("sandbox/state.txt"), "preserve").expect("sandbox file");

        let report = store.migrate_legacy_sessions(false);
        assert_eq!(report.found, 1);
        assert_eq!(report.migrated, 1);
        assert!(!legacy.exists());
        let destination = store.canonical_session_dir("1704240000000");
        assert_eq!(
            std::fs::read_to_string(destination.join("sandbox/state.txt")).expect("migrated file"),
            "preserve"
        );
        assert!(destination.join(SESSION_METADATA_FILE).exists());
        assert!(destination.join("logs").is_dir());
    }

    #[test]
    fn legacy_workspace_paths_remain_under_the_legacy_directory() {
        let root = tempfile::tempdir().expect("temp root");
        let store = SessionStore::new(root.path());
        let id = "1704067200000";
        let legacy = root.path().join(SESSIONS_DIR).join(id);
        write_history(&legacy.join(HISTORY_FILE));

        assert_eq!(store.get_active_session_dir(id), legacy);
        assert_eq!(
            store.get_active_session_sandbox_dir(id),
            legacy.join("sandbox")
        );
        assert_eq!(
            store.get_active_session_artifacts_dir(id),
            legacy.join("artifacts")
        );
        assert_eq!(
            store
                .get_active_session_dir(id)
                .join("subagents")
                .join("agent-7"),
            legacy.join("subagents/agent-7")
        );
    }
}
