use super::*;
use crate::app::ChatMessage;
use rustcode_session::SessionStore;
pub use rustcode_session::{HistorySnapshot, SessionMeta};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const HISTORY_FILE: &str = rustcode_session::HISTORY_FILE;
const SESSIONS_DIR: &str = rustcode_session::SESSIONS_DIR;

fn store() -> Option<SessionStore> {
    get_config_dir().map(SessionStore::new)
}

pub(super) fn next_session_id_value(now: u64, previous: u64) -> u64 {
    rustcode_session::next_session_id_value(now, previous)
}

fn next_session_id() -> String {
    rustcode_session::next_session_id()
}

pub(super) fn queue_history_write(
    path: PathBuf,
    history: &[ChatMessage],
    revision: Option<u64>,
) -> bool {
    rustcode_session::queue_history_write(path, history, revision)
}

pub(super) fn write_history_file(path: &Path, history: &[ChatMessage]) {
    rustcode_session::write_history_file(path, history)
}

pub fn flush_history() {
    rustcode_session::flush_history()
}

pub fn flush_history_async() {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(flush_history);
        }
        Err(_) => flush_history(),
    }
}

pub fn set_active_session_id(session_id: &str) {
    let mut cache = lock(active_session_cache());
    *cache = if session_id.is_empty() {
        None
    } else {
        Some(session_id.to_string())
    };
}

fn active_session_cache() -> &'static Mutex<Option<String>> {
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

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

pub fn session_has_content(history: &[ChatMessage]) -> bool {
    SessionStore::session_has_content(history)
}

#[allow(dead_code)]
pub fn session_is_resumable(history: &[ChatMessage]) -> bool {
    SessionStore::session_is_resumable(history)
}

pub(crate) fn session_title(history: &[ChatMessage]) -> String {
    SessionStore::session_title(history)
}

pub(crate) fn session_id_from_path(path: &Path) -> Option<String> {
    SessionStore::session_id_from_path(path)
}

pub fn load_session_meta(path: &Path) -> Option<SessionMeta> {
    store()?.load_session_meta(path)
}

pub fn session_id_has_content(session_id: &str) -> bool {
    store().is_some_and(|session_store| session_store.session_id_has_content(session_id))
}

pub fn save_history<H: HistorySnapshot + ?Sized>(history: &H) {
    if let Some(session_store) = store() {
        session_store.save_history(active_session_id().as_deref(), history);
    }
}

pub fn save_session_history<H: HistorySnapshot + ?Sized>(session_id: &str, history: &H) {
    if let Some(session_store) = store() {
        session_store.save_session_history(session_id, history);
    }
}

pub fn save_session_title(session_id: &str, title: &str) {
    if let Some(session_store) = store() {
        session_store.save_session_title(session_id, title);
    }
}

pub fn load_session_title(session_id: &str) -> Option<String> {
    store()?.load_session_title(session_id)
}

pub fn load_session_history_direct(session_id: &str) -> Vec<ChatMessage> {
    store().map_or_else(Vec::new, |session_store| {
        session_store.load_session_history_direct(session_id)
    })
}

pub fn save_session_image_cache(session_id: &str, cache: &HashMap<String, String>) {
    if let Some(session_store) = store() {
        session_store.save_session_image_cache(session_id, cache);
    }
}

pub fn load_session_image_cache(session_id: &str) -> HashMap<String, String> {
    store().map_or_else(HashMap::new, |session_store| {
        session_store.load_session_image_cache(session_id)
    })
}

pub fn get_active_session_dir(session_id: &str) -> Option<PathBuf> {
    store().map(|session_store| session_store.get_active_session_dir(session_id))
}

pub fn get_active_session_sandbox_dir(session_id: &str) -> Option<PathBuf> {
    store().map(|session_store| session_store.get_active_session_sandbox_dir(session_id))
}

pub fn get_active_session_artifacts_dir(session_id: &str) -> Option<PathBuf> {
    store().map(|session_store| session_store.get_active_session_artifacts_dir(session_id))
}

pub fn create_subagent_workspace(session_id: &str, agent_id: u32) -> Result<PathBuf, String> {
    store()
        .ok_or_else(|| "active session directory unavailable".to_string())?
        .create_subagent_workspace(session_id, agent_id)
}

pub fn write_subagent_review_manifest(workspace: &Path, agent_id: u32) -> Option<PathBuf> {
    SessionStore::write_subagent_review_manifest(workspace, agent_id)
}

pub fn init_active_session(config: &mut AppConfig) -> String {
    let Some(dir) = get_config_dir() else {
        return String::new();
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
    let Some(dir) = get_config_dir() else {
        return String::new();
    };
    let session_id = next_session_id();
    let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
    let _ = fs::create_dir_all(&session_dir);
    let _ = fs::create_dir_all(session_dir.join("sandbox"));
    let _ = fs::create_dir_all(session_dir.join("artifacts"));
    config.last_active_session_id = Some(session_id.clone());
    save_entire_config(config);
    flush_history();
    set_active_session_id(&session_id);
    session_id
}

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
    store()?.archive_session(history)
}

pub fn sorted_session_paths() -> Vec<PathBuf> {
    store().map_or_else(Vec::new, |session_store| {
        session_store.sorted_session_paths()
    })
}

pub fn latest_resumable_session_meta() -> Option<SessionMeta> {
    store()?.latest_resumable_session_meta()
}

pub fn session_meta_by_id(id: &str) -> Option<SessionMeta> {
    store()?.session_meta_by_id(id)
}

pub fn list_sessions_limited(limit: usize) -> (Vec<SessionMeta>, bool) {
    store().map_or_else(
        || (Vec::new(), false),
        |session_store| session_store.list_sessions_limited(limit),
    )
}

#[allow(dead_code)]
pub fn list_sessions() -> Vec<SessionMeta> {
    store().map_or_else(Vec::new, |session_store| session_store.list_sessions())
}

pub fn archive_live_history() {}

pub fn live_session_meta() -> Option<SessionMeta> {
    let (_, _, config) = load_config();
    let session_id = config.last_active_session_id?;
    let path = store()?.session_dir(&session_id).join(HISTORY_FILE);
    path.exists().then(|| load_session_meta(&path)).flatten()
}

pub fn load_session_file(path: &Path) -> Vec<ChatMessage> {
    store().map_or_else(Vec::new, |session_store| {
        session_store.load_session_file(path)
    })
}

#[allow(dead_code)]
pub fn delete_session_file(path: &Path) {
    SessionStore::delete_session_file(path);
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
