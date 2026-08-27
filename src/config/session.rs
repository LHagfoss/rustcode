use super::*;
use crate::app::{ChatMessage, History};
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock, atomic::AtomicU64, atomic::Ordering};
use std::time::Duration;

/// How long the writer thread waits after the first queued change before it
/// writes. A single agent turn saves the history many times (after the model
/// response, after every tool batch, after loop-detector injections); the
/// debounce collapses that burst into one write of the newest snapshot.
const HISTORY_WRITE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Coalescing, off-runtime writer for `history.json`.
///
/// Callers hand over a snapshot and return immediately; a dedicated OS thread
/// (never a runtime worker) performs the serialization and the blocking write.
struct HistoryWriter {
    /// Newest unwritten snapshot per destination file.
    pending: Mutex<HashMap<PathBuf, PendingHistoryWrite>>,
    wakeup: Condvar,
    /// Serializes take-snapshot-then-write, so a slow write of an older
    /// snapshot can never land on top of a newer one when the background
    /// thread and an explicit flush run concurrently.
    write_slot: Mutex<()>,
}

struct PendingHistoryWrite {
    history: Vec<ChatMessage>,
    revision: Option<u64>,
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
                                .unwrap_or_else(|e| e.into_inner());
                        }
                    }
                    std::thread::sleep(HISTORY_WRITE_DEBOUNCE);
                    drain_history_writes(writer);
                }
            });
        writer
    })
}

/// Poisoned mutexes are not fatal here: the protected data is a plain snapshot
/// map, so recovering the inner value keeps history saving alive.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn next_session_id_value(now: u64, previous: u64) -> u64 {
    now.max(previous.saturating_add(1))
}

fn next_session_id() -> String {
    static LAST_SESSION_ID: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
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

pub(super) fn queue_history_write(
    path: PathBuf,
    history: &[ChatMessage],
    revision: Option<u64>,
) -> bool {
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

fn drain_history_writes(writer: &HistoryWriter) {
    let _slot = lock(&writer.write_slot);
    let batch = std::mem::take(&mut *lock(&writer.pending));
    for (path, pending) in batch {
        write_history_file(&path, &pending.history);
    }
}

/// Write every queued history snapshot to disk before returning. Call on turn
/// end and on shutdown so nothing queued can be lost.
pub fn flush_history() {
    drain_history_writes(history_writer());
}

/// Same guarantee as [`flush_history`], but the blocking write is moved to a
/// blocking pool thread when called from inside a Tokio runtime.
pub fn flush_history_async() {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(flush_history);
        }
        Err(_) => flush_history(),
    }
}

/// Write a history file atomically: serialize compactly into a sibling temp
/// file, then rename over the target. A crash mid-write leaves the previous
/// complete file intact instead of a truncated one.
pub(super) fn write_history_file(path: &Path, history: &[ChatMessage]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Compact, not pretty: this file is machine-read, and pretty-printing
    // roughly doubles the bytes written on every save.
    let Ok(json_str) = serde_json::to_string(history) else {
        return;
    };
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    if fs::write(&tmp, json_str).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// Process-wide cache of the active session id, so history saves never have to
/// re-read and re-parse `config.toml` just to learn where to write.
fn active_session_cache() -> &'static Mutex<Option<String>> {
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Record the session that history saves should target. Called whenever the
/// active session is created, resumed or switched.
pub fn set_active_session_id(session_id: &str) {
    let mut cache = lock(active_session_cache());
    *cache = if session_id.is_empty() {
        None
    } else {
        Some(session_id.to_string())
    };
}

/// The active session id, read from `config.toml` at most once per process.
fn active_session_id() -> Option<String> {
    let mut cache = lock(active_session_cache());
    if cache.is_none() {
        let (_, _, config) = load_config();
        *cache = config
            .last_active_session_id
            .filter(|id| !id.trim().is_empty());
    }
    cache.clone()
}

/// A saved chat session on disk, listed by /history and /resume.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub path: PathBuf,
    pub title: String,
    pub when: String,
    pub message_count: usize,
}

/// True when the history contains at least one real prompt (not a slash command).
pub fn session_has_content(history: &[ChatMessage]) -> bool {
    history
        .iter()
        .any(|m| m.role == "user" && !m.content.starts_with('/'))
}

#[allow(dead_code)]
pub fn session_is_resumable(history: &[ChatMessage]) -> bool {
    session_has_content(history) && history.iter().any(|m| m.role == "assistant")
}

pub(crate) fn session_title(history: &[ChatMessage]) -> String {
    let title = history
        .iter()
        .find(|m| m.role == "user" && !m.content.starts_with('/'))
        .map(|m| m.content.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_else(|| "(no prompt)".to_string());
    if title.chars().count() > 48 {
        let truncated: String = title.chars().take(45).collect();
        format!("{truncated}...")
    } else {
        title
    }
}

pub(crate) fn session_id_from_path(path: &Path) -> Option<String> {
    if path.file_name().map(|n| n == HISTORY_FILE).unwrap_or(false) {
        let parent = path.parent()?;
        if parent
            .parent()
            .and_then(|p| p.file_name())
            .map(|c| c == SESSIONS_DIR)
            .unwrap_or(false)
        {
            return parent
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_owned);
        }
    } else if path
        .parent()
        .and_then(|p| p.file_name())
        .map(|c| c == SESSIONS_DIR)
        .unwrap_or(false)
    {
        return path.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
    }
    None
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

pub fn load_session_meta(path: &Path) -> Option<SessionMeta> {
    let content = fs::read_to_string(path).ok()?;
    let messages: Vec<ChatMessageMetaRef> = serde_json::from_str(&content).ok()?;
    let has_user = messages
        .iter()
        .any(|m| m.role == "user" && !m.content.starts_with('/'));
    let has_assistant = messages.iter().any(|m| m.role == "assistant");
    if !has_user || !has_assistant {
        return None;
    }

    let title = session_id_from_path(path)
        .as_deref()
        .and_then(load_session_title)
        .unwrap_or_else(|| {
            let first_user = messages
                .iter()
                .find(|m| m.role == "user" && !m.content.starts_with('/'))
                .map(|m| m.content.lines().next().unwrap_or("").trim())
                .unwrap_or("(no prompt)");
            if first_user.chars().count() > 48 {
                let truncated: String = first_user.chars().take(45).collect();
                format!("{truncated}...")
            } else if first_user.is_empty() {
                "(no prompt)".to_string()
            } else {
                first_user.to_string()
            }
        });

    let when = messages
        .first()
        .map(|m| m.timestamp.to_string())
        .unwrap_or_default();

    Some(SessionMeta {
        title,
        when,
        message_count: messages.len(),
        path: path.to_path_buf(),
    })
}

pub fn session_id_has_content(session_id: &str) -> bool {
    if let Some(dir) = get_config_dir() {
        let path = dir.join(SESSIONS_DIR).join(session_id).join(HISTORY_FILE);
        if let Ok(content) = fs::read_to_string(&path)
            && let Ok(messages) = serde_json::from_str::<Vec<ChatMessageMetaRef>>(&content)
        {
            return messages
                .iter()
                .any(|m| m.role == "user" && !m.content.starts_with('/'));
        }
    }
    false
}

/// A history input that can expose its immutable messages and, when available,
/// a mutation revision for queued-write deduplication.
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

/// Archive a chat into the sessions directory. No-op for histories without
/// a real prompt. Returns the archive path on success.
pub fn save_history<H: HistorySnapshot + ?Sized>(history: &H) {
    match active_session_id() {
        Some(session_id) => save_session_history(&session_id, history),
        None => {
            if let Some(dir) = get_config_dir() {
                queue_history_write(
                    dir.join(HISTORY_FILE),
                    history.messages(),
                    history.revision(),
                );
            }
        }
    }
}

pub fn save_session_history<H: HistorySnapshot + ?Sized>(session_id: &str, history: &H) {
    if let Some(dir) = get_config_dir() {
        let path = dir.join(SESSIONS_DIR).join(session_id).join(HISTORY_FILE);
        queue_history_write(path, history.messages(), history.revision());
    }
}

/// Save a custom title for a session.
pub fn save_session_title(session_id: &str, title: &str) {
    if let Some(dir) = get_config_dir() {
        let session_dir = dir.join(SESSIONS_DIR).join(session_id);
        let _ = fs::create_dir_all(&session_dir);
        let _ = fs::write(session_dir.join("title.txt"), title);
    }
}

/// Load a custom title for a session. Returns None if no custom title exists.
pub fn load_session_title(session_id: &str) -> Option<String> {
    if let Some(dir) = get_config_dir() {
        let path = dir.join(SESSIONS_DIR).join(session_id).join("title.txt");
        if path.exists() {
            return fs::read_to_string(path).ok().map(|s| s.trim().to_string());
        }
    }
    None
}

pub fn load_session_history_direct(session_id: &str) -> Vec<ChatMessage> {
    if let Some(dir) = get_config_dir() {
        let path = dir.join(SESSIONS_DIR).join(session_id).join("history.json");
        return load_session_file(&path);
    }
    Vec::new()
}

pub fn save_session_image_cache(
    session_id: &str,
    cache: &std::collections::HashMap<String, String>,
) {
    if cache.is_empty() {
        return;
    }
    if let Some(dir) = get_config_dir() {
        let session_dir = dir.join(SESSIONS_DIR).join(session_id);
        let _ = fs::create_dir_all(&session_dir);
        if let Ok(json) = serde_json::to_string_pretty(cache) {
            let _ = fs::write(session_dir.join("image_cache.json"), json);
        }
    }
}

pub fn load_session_image_cache(session_id: &str) -> std::collections::HashMap<String, String> {
    if let Some(dir) = get_config_dir() {
        let path = dir
            .join(SESSIONS_DIR)
            .join(session_id)
            .join("image_cache.json");
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(cache) = serde_json::from_str(&content) {
                    return cache;
                }
            }
        }
    }
    std::collections::HashMap::new()
}

pub fn get_active_session_dir(session_id: &str) -> Option<PathBuf> {
    let dir = get_config_dir()?;
    Some(dir.join(SESSIONS_DIR).join(session_id))
}

pub fn get_active_session_sandbox_dir(session_id: &str) -> Option<PathBuf> {
    let dir = get_active_session_dir(session_id)?;
    Some(dir.join("sandbox"))
}

pub fn get_active_session_artifacts_dir(session_id: &str) -> Option<PathBuf> {
    let dir = get_active_session_dir(session_id)?;
    Some(dir.join("artifacts"))
}

/// Create a detached worktree for a write-enabled subagent. The worktree is
/// intentionally retained after completion so the parent can inspect and
/// selectively merge the review manifest.
pub fn create_subagent_workspace(session_id: &str, agent_id: u32) -> Result<PathBuf, String> {
    let root = get_active_session_dir(session_id)
        .ok_or_else(|| "active session directory unavailable".to_string())?
        .join("subagents")
        .join(format!("agent-{agent_id}"));
    if root.exists() {
        return Ok(root);
    }
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create subagent directory: {e}"))?;
    }
    let repo = std::env::current_dir().map_err(|e| format!("resolve repository: {e}"))?;
    let status = std::process::Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(&root)
        .arg("HEAD")
        .current_dir(repo)
        .status()
        .map_err(|e| format!("create git worktree: {e}"))?;
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

pub fn init_active_session(config: &mut AppConfig) -> String {
    let dir = match get_config_dir() {
        Some(d) => d,
        None => return "".to_string(),
    };

    if let Some(ref session_id) = config.last_active_session_id {
        let session_dir = dir.join(SESSIONS_DIR).join(session_id);
        if session_dir.exists() {
            let _ = fs::create_dir_all(session_dir.join("sandbox"));
            let _ = fs::create_dir_all(session_dir.join("artifacts"));
            set_active_session_id(session_id);
            return session_id.clone();
        }
    }

    let legacy_history_path = dir.join(HISTORY_FILE);
    let legacy_history = load_session_file(&legacy_history_path);
    if session_has_content(&legacy_history) {
        let session_id = next_session_id();
        let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
        let _ = fs::create_dir_all(&session_dir);
        let _ = fs::create_dir_all(session_dir.join("sandbox"));
        let _ = fs::create_dir_all(session_dir.join("artifacts"));

        write_history_file(&session_dir.join(HISTORY_FILE), &legacy_history);
        let _ = fs::remove_file(&legacy_history_path);

        config.last_active_session_id = Some(session_id.clone());
        save_entire_config(config);
        set_active_session_id(&session_id);
        return session_id;
    }

    let session_id = next_session_id();
    let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
    let _ = fs::create_dir_all(&session_dir);
    let _ = fs::create_dir_all(session_dir.join("sandbox"));
    let _ = fs::create_dir_all(session_dir.join("artifacts"));

    config.last_active_session_id = Some(session_id.clone());
    save_entire_config(config);
    set_active_session_id(&session_id);
    session_id
}

pub fn create_new_session(config: &mut AppConfig) -> String {
    let dir = match get_config_dir() {
        Some(d) => d,
        None => return "".to_string(),
    };
    let session_id = next_session_id();
    let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
    let _ = fs::create_dir_all(&session_dir);
    let _ = fs::create_dir_all(session_dir.join("sandbox"));
    let _ = fs::create_dir_all(session_dir.join("artifacts"));

    config.last_active_session_id = Some(session_id.clone());
    save_entire_config(config);
    // Any history still queued belongs to the session we are leaving; write it
    // out before the active session id changes.
    flush_history();
    set_active_session_id(&session_id);
    session_id
}

/// Choose the session to open at startup. Reuses the last active session when it
/// is still empty, so relaunching the app repeatedly doesn't litter the sessions
/// directory with abandoned empty chats. If the last session already has real
/// content, a fresh session is started (a new chat), as before.
pub fn start_session(config: &mut AppConfig) -> String {
    let last = init_active_session(config);
    if last.is_empty() {
        return last;
    }
    if session_id_has_content(&last) {
        create_new_session(config)
    } else {
        last
    }
}

#[allow(dead_code)]
pub fn archive_session(history: &[ChatMessage]) -> Option<PathBuf> {
    if !session_has_content(history) {
        return None;
    }
    let dir = get_config_dir()?.join(SESSIONS_DIR);
    let session_dir = dir.join(next_session_id());
    fs::create_dir_all(&session_dir).ok()?;
    fs::create_dir_all(session_dir.join("sandbox")).ok()?;
    fs::create_dir_all(session_dir.join("artifacts")).ok()?;
    let path = session_dir.join("history.json");
    let json_str = serde_json::to_string_pretty(history).ok()?;
    fs::write(&path, json_str).ok()?;
    prune_sessions(&dir);
    Some(path)
}

#[allow(dead_code)]
fn prune_sessions(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut targets: Vec<PathBuf> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            if p.join("history.json").exists() {
                targets.push(p);
            }
        } else if p.extension().map(|e| e == "json").unwrap_or(false) {
            targets.push(p);
        }
    }
    if targets.len() <= MAX_SESSIONS {
        return;
    }
    targets.sort();
    for old in &targets[..targets.len() - MAX_SESSIONS] {
        if old.is_dir() {
            let _ = fs::remove_dir_all(old);
        } else {
            let _ = fs::remove_file(old);
        }
    }
}

pub fn sorted_session_paths() -> Vec<PathBuf> {
    let Some(dir) = get_config_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir.join(SESSIONS_DIR)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_dir() {
            let history_file = p.join("history.json");
            if history_file.exists() {
                paths.push(history_file);
            }
        } else if p.extension().map(|e| e == "json").unwrap_or(false) {
            paths.push(p);
        }
    }
    paths.sort();
    paths.reverse();
    paths
}

pub fn latest_resumable_session_meta() -> Option<SessionMeta> {
    for path in sorted_session_paths() {
        if let Some(meta) = load_session_meta(&path) {
            return Some(meta);
        }
    }
    None
}

pub fn session_meta_by_id(id: &str) -> Option<SessionMeta> {
    let dir = get_config_dir()?;
    let dir_path = dir.join(SESSIONS_DIR).join(id).join("history.json");
    if dir_path.exists() {
        if let Some(meta) = load_session_meta(&dir_path) {
            return Some(meta);
        }
    }
    let flat_path = dir.join(SESSIONS_DIR).join(format!("{id}.json"));
    if flat_path.exists() {
        if let Some(meta) = load_session_meta(&flat_path) {
            return Some(meta);
        }
    }
    None
}

pub fn list_sessions_limited(limit: usize) -> (Vec<SessionMeta>, bool) {
    let mut list = Vec::new();
    let mut truncated = false;
    for path in sorted_session_paths() {
        if let Some(meta) = load_session_meta(&path) {
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

#[allow(dead_code)]
pub fn list_sessions() -> Vec<SessionMeta> {
    let (list, _) = list_sessions_limited(usize::MAX);
    list
}

pub fn archive_live_history() {
    // Deprecated/no-op in new per-session structure
}

pub fn live_session_meta() -> Option<SessionMeta> {
    let (_, _, config) = load_config();
    if let Some(session_id) = config.last_active_session_id {
        let path = get_config_dir()?
            .join(SESSIONS_DIR)
            .join(&session_id)
            .join("history.json");
        if path.exists() {
            return load_session_meta(&path);
        }
    }
    None
}

pub fn load_session_file(path: &Path) -> Vec<ChatMessage> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<ChatMessage>>(&content).unwrap_or_default()
}

#[allow(dead_code)]
pub fn delete_session_file(path: &Path) {
    if path
        .file_name()
        .map(|n| n == "history.json")
        .unwrap_or(false)
    {
        if let Some(parent) = path.parent()
            && parent
                .parent()
                .map(|p| p.ends_with(SESSIONS_DIR))
                .unwrap_or(false)
        {
            let _ = fs::remove_dir_all(parent);
        }
    } else if path
        .parent()
        .map(|p| p.ends_with(SESSIONS_DIR))
        .unwrap_or(false)
    {
        let _ = fs::remove_file(path);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct MonthlyUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub calls: u64,
}

pub fn track_usage(prompt_tokens: u64, completion_tokens: u64) {
    let dir = match get_config_dir() {
        Some(d) => d,
        None => return,
    };
    let path = dir.join("usage_stats.json");
    let mut stats: std::collections::BTreeMap<String, MonthlyUsage> = if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            std::collections::BTreeMap::new()
        }
    } else {
        std::collections::BTreeMap::new()
    };

    let month_str = chrono::Local::now().format("%Y-%m").to_string();
    let entry = stats.entry(month_str).or_default();
    entry.prompt_tokens += prompt_tokens;
    entry.completion_tokens += completion_tokens;
    entry.total_tokens += prompt_tokens + completion_tokens;
    entry.calls += 1;

    if let Ok(json_str) = serde_json::to_string_pretty(&stats) {
        let _ = fs::write(&path, json_str);
    }
}

pub fn get_usage_history() -> std::collections::BTreeMap<String, MonthlyUsage> {
    let dir = match get_config_dir() {
        Some(d) => d,
        None => return std::collections::BTreeMap::new(),
    };
    let path = dir.join("usage_stats.json");
    if path.exists()
        && let Ok(content) = fs::read_to_string(&path)
    {
        return serde_json::from_str(&content).unwrap_or_default();
    }
    std::collections::BTreeMap::new()
}

pub const DEFAULT_SYNC_GITIGNORE: &str = r#"debug.log
debug.log.*
*.log
*.bak
symbols.db
tool_output/
attachments/
sessions/*/sandbox/
sessions/*/artifacts/
sessions/*/subagents/
sessions/*/image_cache.json
.DS_Store
*.tmp
"#;

pub fn ensure_sync_gitignore(dir: &Path) -> Result<(), String> {
    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        return fs::write(&gitignore_path, DEFAULT_SYNC_GITIGNORE)
            .map_err(|e| format!("Failed to write .gitignore: {e}"));
    }

    let current = fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut missing = Vec::new();
    for line in DEFAULT_SYNC_GITIGNORE.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !current.lines().any(|l| l.trim() == trimmed) {
            missing.push(trimmed);
        }
    }

    if !missing.is_empty() {
        let mut updated = current;
        if !updated.ends_with('\n') && !updated.is_empty() {
            updated.push('\n');
        }
        for item in missing {
            updated.push_str(item);
            updated.push('\n');
        }
        fs::write(&gitignore_path, updated)
            .map_err(|e| format!("Failed to update .gitignore: {e}"))?;
    }
    Ok(())
}

pub fn get_sync_branch(dir: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output();
    if let Ok(out) = output
        && out.status.success()
    {
        let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !branch.is_empty() && branch != "HEAD" {
            return branch;
        }
    }
    "main".to_string()
}

pub fn init_sync_repo(remote_url: &str) -> Result<(), String> {
    let dir = get_config_dir().ok_or("Failed to get config directory")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }

    ensure_sync_gitignore(&dir)?;

    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        let init_status = std::process::Command::new("git")
            .args(["init"])
            .current_dir(&dir)
            .status()
            .map_err(|e| format!("Failed to run git init: {e}"))?;
        if !init_status.success() {
            return Err("git init failed".to_string());
        }
    }

    // Set remote origin
    let _ = std::process::Command::new("git")
        .args(["remote", "remove", "origin"])
        .current_dir(&dir)
        .status();

    let remote_status = std::process::Command::new("git")
        .args(["remote", "add", "origin", remote_url])
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("Failed to add git remote: {e}"))?;

    if !remote_status.success() {
        return Err("git remote add origin failed".to_string());
    }

    Ok(())
}

pub fn sync_config_pull() -> Result<(), String> {
    let dir = get_config_dir().ok_or("Failed to get config directory")?;
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        return Err(
            "Sync repo not initialized. Please run: rustcode sync init <remote-git-url>"
                .to_string(),
        );
    }

    ensure_sync_gitignore(&dir)?;
    let branch = get_sync_branch(&dir);

    // A config directory can have a .git directory and a remote configured
    // without having a local commit yet (for example after `sync init`). Git
    // refuses to pull in that state when remote files would overwrite the
    // existing untracked config. Record the local snapshot first so the
    // normal rebase pull can merge it with the remote history.
    let has_head = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(&dir)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !has_head {
        let add_out = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&dir)
            .status()
            .map_err(|e| format!("Failed to stage initial config snapshot: {e}"))?;
        if !add_out.success() {
            return Err("Failed to stage initial config snapshot".to_string());
        }

        let commit_out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=rustcode",
                "-c",
                "user.email=rustcode@localhost",
                "commit",
                "-m",
                "Initialize local config snapshot",
            ])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("Failed to create initial config snapshot: {e}"))?;
        if !commit_out.status.success() {
            let err = String::from_utf8_lossy(&commit_out.stderr);
            return Err(format!(
                "Failed to create initial config snapshot: {}",
                err.trim()
            ));
        }
    }

    let pull_out = std::process::Command::new("git")
        .args(["pull", "--rebase", "--autostash", "origin", &branch])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("Failed to pull updates: {e}"))?;

    if pull_out.status.success() {
        let msg = String::from_utf8_lossy(&pull_out.stdout);
        if !msg.contains("Already up to date") && !msg.contains("Current branch") {
            println!("Pull result: {}", msg.trim());
        }
        Ok(())
    } else {
        // Abort rebase if in progress to keep the repo clean and usable
        let _ = std::process::Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(&dir)
            .status();

        let err = String::from_utf8_lossy(&pull_out.stderr);
        let out = String::from_utf8_lossy(&pull_out.stdout);
        let combined = if !err.trim().is_empty() {
            err.trim()
        } else {
            out.trim()
        };
        Err(format!("Pull failed (rebase aborted): {combined}"))
    }
}

pub fn sync_config_push() -> Result<(), String> {
    let dir = get_config_dir().ok_or("Failed to get config directory")?;
    let git_dir = dir.join(".git");
    if !git_dir.exists() {
        return Err(
            "Sync repo not initialized. Please run: rustcode sync init <remote-git-url>"
                .to_string(),
        );
    }

    ensure_sync_gitignore(&dir)?;
    let branch = get_sync_branch(&dir);

    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "device".to_string())
        });

    // 1. Stage all files in config directory
    let add_out = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&dir)
        .status()
        .map_err(|e| format!("Failed to stage files: {e}"))?;

    if !add_out.success() {
        return Err("git add failed".to_string());
    }

    // 2. Commit changes if any
    let commit_msg = format!(
        "sync: {} ({})",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        host
    );
    let commit_status = std::process::Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .current_dir(&dir)
        .status();

    let mut committed = false;
    if let Ok(st) = commit_status {
        if st.success() {
            println!("Committed local changes: {}", commit_msg);
            committed = true;
        } else {
            println!("No new local changes to commit.");
        }
    }

    // 3. Push to remote
    if committed {
        let push_out = std::process::Command::new("git")
            .args(["push", "-u", "origin", &branch])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("Failed to push to remote: {e}"))?;

        if push_out.status.success() {
            println!("Successfully pushed config to remote origin/{branch}! 🚀");
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&push_out.stderr);
            Err(format!("Push failed: {}", err.trim()))
        }
    } else {
        Ok(()) // Nothing to push if nothing was committed
    }
}
