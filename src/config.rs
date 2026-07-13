use crate::app::ChatMessage;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_CONTEXT_TOKENS: u32 = 2048;
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

const CONFIG_FILE: &str = "config.toml";
const HISTORY_FILE: &str = "history.json";
const HISTORY_LIMIT: usize = 50;
const SESSIONS_DIR: &str = "sessions";
const MAX_SESSIONS: usize = 30;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ModelProfile {
    pub name: String,
    pub url: String,
    pub model: String,
    #[serde(default)]
    pub context_window: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolProtocol {
    Json,
    Xml,
}

impl Default for ToolProtocol {
    fn default() -> Self {
        ToolProtocol::Json
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub default: String,
    pub models: Vec<ModelProfile>,
    #[serde(default)]
    pub tool_protocol: ToolProtocol,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserSettings {
    #[serde(default)]
    pub auto_confirm: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            auto_confirm: false,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default: "apple-fm".to_string(),
            models: vec![
                ModelProfile {
                    name: "apple-fm".to_string(),
                    url: "http://127.0.0.1:1976/v1/chat/completions".to_string(),
                    model: "system".to_string(),
                    context_window: Some(MAX_CONTEXT_TOKENS),
                },
                ModelProfile {
                    name: "ollama".to_string(),
                    url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
                    model: "llama3.2:latest".to_string(),
                    context_window: Some(32768),
                },
            ],
            tool_protocol: ToolProtocol::default(),
        }
    }
}

pub fn get_config_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let config_root = PathBuf::from(home).join(".config");
    let dir = config_root.join("rustcode");

    if !dir.exists() {
        let legacy = config_root.join("fmr");
        if legacy.exists() && fs::rename(&legacy, &dir).is_ok() {
            let old_history = dir.join("fmr_history.json");
            if old_history.exists() {
                let _ = fs::rename(&old_history, dir.join(HISTORY_FILE));
            }
        }
    }

    Some(dir)
}

fn default_endpoint(config: &AppConfig) -> (String, String) {
    let profile = config.models[0].clone();
    (profile.url, profile.model)
}

pub fn load_config() -> (String, String, AppConfig) {
    match get_config_dir() {
        Some(dir) => load_config_from(&dir),
        None => {
            let config = AppConfig::default();
            let (url, model) = default_endpoint(&config);
            (url, model, config)
        }
    }
}

fn load_config_from(dir: &Path) -> (String, String, AppConfig) {
    let default_config = AppConfig::default();

    let file_path = dir.join(CONFIG_FILE);
    if !file_path.exists() {
        save_config_to(dir, &default_config);
        let (url, model) = default_endpoint(&default_config);
        return (url, model, default_config);
    }

    let Ok(content) = fs::read_to_string(&file_path) else {
        let (url, model) = default_endpoint(&default_config);
        return (url, model, default_config);
    };

    let mut config = match toml::from_str::<AppConfig>(&content) {
        Ok(c) => c,
        Err(_) => default_config,
    };

    // backfill windows for profiles saved before the context_window field
    let defaults = AppConfig::default();
    for profile in &mut config.models {
        if profile.context_window.is_none()
            && let Some(d) = defaults.models.iter().find(|m| m.name == profile.name)
        {
            profile.context_window = d.context_window;
        }
    }

    let (url, model) = config
        .models
        .iter()
        .find(|m| m.name == config.default)
        .or_else(|| config.models.first())
        .map(|p| (p.url.clone(), p.model.clone()))
        .unwrap_or_else(|| default_endpoint(&AppConfig::default()));

    (url, model, config)
}

pub fn save_entire_config(config: &AppConfig) {
    if let Some(dir) = get_config_dir() {
        save_config_to(&dir, config);
    }
}

fn save_config_to(dir: &Path, config: &AppConfig) {
    let _ = fs::create_dir_all(dir);
    if let Ok(toml_str) = toml::to_string_pretty(config) {
        let _ = fs::write(dir.join(CONFIG_FILE), toml_str);
    }
}

pub fn save_history(history: &[ChatMessage]) {
    if let Some(dir) = get_config_dir() {
        save_history_to(&dir, history);
    }
}

fn save_history_to(dir: &Path, history: &[ChatMessage]) {
    let _ = fs::create_dir_all(dir);
    let start_idx = history.len().saturating_sub(HISTORY_LIMIT);
    if let Ok(json_str) = serde_json::to_string_pretty(&history[start_idx..]) {
        let _ = fs::write(dir.join(HISTORY_FILE), json_str);
    }
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

fn session_title(history: &[ChatMessage]) -> String {
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

fn session_meta_from(path: PathBuf, history: &[ChatMessage]) -> SessionMeta {
    SessionMeta {
        title: session_title(history),
        when: history
            .first()
            .map(|m| m.timestamp.clone())
            .unwrap_or_default(),
        message_count: history.len(),
        path,
    }
}

/// Archive a chat into the sessions directory. No-op for histories without
/// a real prompt. Returns the archive path on success.
pub fn archive_session(history: &[ChatMessage]) -> Option<PathBuf> {
    if !session_has_content(history) {
        return None;
    }
    let dir = get_config_dir()?.join(SESSIONS_DIR);
    fs::create_dir_all(&dir).ok()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!("{ts}.json"));
    let start_idx = history.len().saturating_sub(HISTORY_LIMIT);
    let json_str = serde_json::to_string_pretty(&history[start_idx..]).ok()?;
    fs::write(&path, json_str).ok()?;
    prune_sessions(&dir);
    Some(path)
}

fn prune_sessions(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    if files.len() <= MAX_SESSIONS {
        return;
    }
    files.sort();
    for old in &files[..files.len() - MAX_SESSIONS] {
        let _ = fs::remove_file(old);
    }
}

/// Archived sessions, newest first.
pub fn list_sessions() -> Vec<SessionMeta> {
    let Some(dir) = get_config_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir.join(SESSIONS_DIR)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    paths.sort();
    paths.reverse();
    paths
        .into_iter()
        .filter_map(|p| {
            let history = load_session_file(&p);
            if history.is_empty() {
                None
            } else {
                Some(session_meta_from(p, &history))
            }
        })
        .collect()
}

/// Move the previous run's live history into the sessions archive so a new
/// run starts fresh without silently overwriting the old chat. Call once at
/// startup, before any history is saved.
pub fn archive_live_history() {
    let Some(dir) = get_config_dir() else { return };
    let path = dir.join(HISTORY_FILE);
    let history = load_session_file(&path);
    if session_has_content(&history) && archive_session(&history).is_some() {
        let _ = fs::remove_file(&path);
    }
}

/// Meta for the live history file (the previous run's chat), if it has content.
pub fn live_session_meta() -> Option<SessionMeta> {
    let path = get_config_dir()?.join(HISTORY_FILE);
    let history = load_session_file(&path);
    if session_has_content(&history) {
        Some(session_meta_from(path, &history))
    } else {
        None
    }
}

pub fn load_session_file(path: &Path) -> Vec<ChatMessage> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<ChatMessage>>(&content).unwrap_or_default()
}

pub fn delete_session_file(path: &Path) {
    // only ever remove files inside the sessions dir; the live history
    // file is rewritten by save_history instead
    if path
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
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            return serde_json::from_str(&content).unwrap_or_default();
        }
    }
    std::collections::BTreeMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("rustcode-tests").join(format!(
            "{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_config_save_load() {
        let dir = temp_dir("config");
        let mut config = AppConfig::default();
        config.default = "ollama".to_string();
        save_config_to(&dir, &config);

        let (url, model, loaded) = load_config_from(&dir);
        assert_eq!(loaded.default, "ollama");
        let expected = &loaded.models.iter().find(|m| m.name == "ollama").unwrap();
        assert_eq!(url, expected.url);
        assert_eq!(model, expected.model);
    }

    #[test]
    fn test_default_profile_is_source_of_truth() {
        let dir = temp_dir("latest");
        let mut config = AppConfig::default();
        config.default = "ollama".to_string();
        save_config_to(&dir, &config);

        let (url, model, _) = load_config_from(&dir);
        let expected = &config.models.iter().find(|m| m.name == "ollama").unwrap();
        assert_eq!(url, expected.url);
        assert_eq!(model, expected.model);
    }

    #[test]
    fn test_context_window_optional() {
        let dir = temp_dir("ctxwin");
        let mut config = AppConfig::default();
        config.models[0].context_window = Some(4096);
        save_config_to(&dir, &config);
        let (_, _, loaded) = load_config_from(&dir);
        assert_eq!(
            loaded
                .models
                .iter()
                .find(|m| m.name == "apple-fm")
                .unwrap()
                .context_window,
            Some(4096)
        );
    }

    #[test]
    fn test_history_save_load() {
        let dir = temp_dir("history");
        let msgs = vec![
            ChatMessage::new("user", "Hello"),
            ChatMessage::new("assistant", "Hi there"),
        ];
        save_history_to(&dir, &msgs);
        let loaded = load_session_file(&dir.join(HISTORY_FILE));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "Hello");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "Hi there");
    }

    #[test]
    fn test_session_has_content_ignores_commands() {
        let cmds_only = vec![
            ChatMessage::new("user", "/help"),
            ChatMessage::new("system", "help text"),
        ];
        assert!(!session_has_content(&cmds_only));
        let real = vec![ChatMessage::new("user", "fix the bug")];
        assert!(session_has_content(&real));
    }

    #[test]
    fn test_session_title_first_prompt_truncated() {
        let history = vec![
            ChatMessage::new("user", "/model"),
            ChatMessage::new("user", "x".repeat(100)),
        ];
        let title = session_title(&history);
        assert!(title.ends_with("..."));
        assert_eq!(title.chars().count(), 48);
        assert_eq!(session_title(&[]), "(no prompt)");
    }

    #[test]
    fn test_delete_session_file_only_in_sessions_dir() {
        let dir = temp_dir("delete-guard");
        let outside = dir.join("history.json");
        fs::write(&outside, "[]").unwrap();
        delete_session_file(&outside);
        assert!(outside.exists(), "live history file must not be deleted");

        let sessions = dir.join(SESSIONS_DIR);
        fs::create_dir_all(&sessions).unwrap();
        let inside = sessions.join("123.json");
        fs::write(&inside, "[]").unwrap();
        delete_session_file(&inside);
        assert!(!inside.exists());
    }

    #[test]
    fn test_history_capped_at_limit() {
        let dir = temp_dir("history-cap");
        let msgs: Vec<ChatMessage> = (0..80)
            .map(|i| ChatMessage::new("user", format!("msg {}", i)))
            .collect();
        save_history_to(&dir, &msgs);
        let loaded = load_session_file(&dir.join(HISTORY_FILE));
        assert_eq!(loaded.len(), HISTORY_LIMIT);
        assert_eq!(loaded[0].content, "msg 30");
    }
}
