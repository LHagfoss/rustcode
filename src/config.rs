use crate::app::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_millis;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

pub const MAX_CONTEXT_TOKENS: u32 = 2048;
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

const CONFIG_FILE: &str = "config.toml";
const HISTORY_FILE: &str = "history.json";
const SESSIONS_DIR: &str = "sessions";
#[allow(dead_code)]
const MAX_SESSIONS: usize = 30;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ModelProfile {
    pub name: String,
    pub url: String,
    pub model: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub env_key: Option<String>,
    /// Forces a tool protocol for this profile, overriding provider detection.
    /// Set it when a self-hosted server implements OpenAI-style function
    /// calling (or advertises it but gets it wrong).
    #[serde(default)]
    pub tool_protocol: Option<ToolProtocol>,
    /// Sends Qwen3's top-level `enable_thinking` request field when set.
    /// `Some(false)` skips `<think>` generation entirely at the chat-template
    /// level (much faster, no reasoning trace); `Some(true)` forces it on;
    /// `None` (default) leaves the server's own template default in place —
    /// matches prior behavior for profiles that don't opt in.
    #[serde(default)]
    pub enable_thinking: Option<bool>,
    /// Per-profile completion token cap sent as `max_tokens`. `None` falls
    /// back to the shared default, overriding whatever a Modelfile's
    /// `PARAMETER num_predict` says.
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Fallback `max_tokens` used when a `ModelProfile` doesn't set its own.
pub const DEFAULT_REQUEST_MAX_TOKENS: u32 = 32768;

impl ModelProfile {
    pub fn endpoint_url(&self) -> String {
        let trimmed = self.url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        if let Some(ref env_name) = self.env_key
            && let Ok(val) = std::env::var(env_name)
            && !val.trim().is_empty()
        {
            return Some(val);
        }
        if let Some(ref k) = self.api_key {
            if let Some(var_name) = k.strip_prefix("env:") {
                if let Ok(val) = std::env::var(var_name)
                    && !val.trim().is_empty()
                {
                    return Some(val);
                }
            } else if let Ok(val) = std::env::var(k) {
                if !val.trim().is_empty() {
                    return Some(val);
                }
            } else if !k.trim().is_empty() {
                return Some(k.clone());
            }
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ToolProtocol {
    #[default]
    Json,
    Native,
    /// True API function-calling: the tool schema is sent in the request's
    /// `tools` field and the model replies with a structured `tool_calls`
    /// field instead of text.
    ///
    /// Used automatically for providers known to implement it, because a call
    /// the provider returns as data cannot be confused with prose — a model
    /// writing tool calls as text can just as easily write their results, and
    /// nothing in the transcript contradicts it. The text protocols remain for
    /// servers without function calling.
    ApiNative,
}

/// Hosts whose OpenAI-compatible endpoints are known to implement function
/// calling, so no probe request is needed before using it.
///
/// Absence proves nothing: most setups reach these providers through a local
/// gateway (`localhost:3000`, an ollama port, a tailnet address), where the
/// hostname says nothing about what the endpoint supports. Anything not listed
/// here is probed instead of assumed.
const FUNCTION_CALLING_HOSTS: &[&str] = &[
    "api.openai.com",
    "api.anthropic.com",
    "generativelanguage.googleapis.com",
    "openrouter.ai",
    "api.groq.com",
    "api.mistral.ai",
    "api.deepseek.com",
    "api.x.ai",
    "api.together.xyz",
    "api.fireworks.ai",
    "api.cerebras.ai",
    "openai.azure.com",
];

/// Whether `url` is a provider already known to implement function calling.
pub fn provider_supports_function_calling(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    FUNCTION_CALLING_HOSTS.iter().any(|host| url.contains(host))
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DefaultConfigTable {
    #[serde(alias = "big_model")]
    pub big: String,
    #[serde(alias = "small_model")]
    pub small: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum DefaultConfig {
    Simple(String),
    Table {
        #[serde(alias = "big_model")]
        big: String,
        #[serde(alias = "small_model")]
        small: String,
    },
    Array(Vec<DefaultConfigTable>),
}

impl DefaultConfig {
    pub fn big(&self) -> &str {
        match self {
            DefaultConfig::Simple(s) => s,
            DefaultConfig::Table { big, .. } => big,
            DefaultConfig::Array(v) => {
                if let Some(first) = v.first() {
                    &first.big
                } else {
                    ""
                }
            }
        }
    }

    pub fn small(&self) -> &str {
        match self {
            DefaultConfig::Simple(s) => s,
            DefaultConfig::Table { small, .. } => small,
            DefaultConfig::Array(v) => {
                if let Some(first) = v.first() {
                    &first.small
                } else {
                    ""
                }
            }
        }
    }

    pub fn set_big(&mut self, new_big: String) {
        match self {
            DefaultConfig::Simple(s) => *s = new_big,
            DefaultConfig::Table { big, .. } => *big = new_big,
            DefaultConfig::Array(v) => {
                if let Some(first) = v.first_mut() {
                    first.big = new_big;
                }
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub default: DefaultConfig,
    pub models: Vec<ModelProfile>,
    #[serde(default)]
    pub tool_protocol: ToolProtocol,
    #[serde(default)]
    pub last_active_session_id: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,

    #[serde(default)]
    pub agent_mode: AgentMode,
    #[serde(default)]
    pub verbosity: crate::app::state::Verbosity,
    /// Opt-in: log the full outbound chat-completion payload (entire message
    /// array, tool schemas) on every request round instead of a metadata-only
    /// summary. Off by default — the full payload is what blows debug.log up
    /// to hundreds of MB over a long session. Turn on only when actually
    /// diagnosing a request-shape issue.
    #[serde(default = "default_false")]
    pub debug_verbose_network_logging: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    #[serde(with = "serde_millis")]
    pub start_time: Option<std::time::SystemTime>,
    #[serde(skip, default = "default_true")]
    pub is_valid: bool,
}

fn default_false() -> bool {
    false
}

fn default_theme() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum AgentMode {
    #[default]
    Build,
    Plan,
}

#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UserSettings {
    #[serde(default)]
    pub auto_confirm: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default: DefaultConfig::Table {
                big: "gemini-3.6-flash".to_string(),
                small: "gemini-3.6-flash".to_string(),
            },
            models: vec![
                ModelProfile {
                    name: "qwen3.6-dense".to_string(),
                    url: "http://100.90.28.23:11434/v1/chat/completions".to_string(),
                    model: "qwen3.6:27b-coding-mxfp8".to_string(),
                    context_window: Some(128000),
                    engine: Some("ollama".to_string()),
                    api_key: None,
                    env_key: None,
                    tool_protocol: None,
                    enable_thinking: None,
                    max_tokens: None,
                },
                ModelProfile {
                    name: "gemini-3.6-flash".to_string(),
                    url: "http://localhost:3000/v1/chat/completions".to_string(),
                    model: "gemini-3.6-flash".to_string(),
                    context_window: Some(128000),
                    engine: Some("openai".to_string()),
                    api_key: None,
                    env_key: None,
                    tool_protocol: None,
                    enable_thinking: None,
                    max_tokens: None,
                },
                ModelProfile {
                    name: "gemma4:e2b-it-qat".to_string(),
                    url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
                    model: "gemma4:e2b-it-qat".to_string(),
                    context_window: Some(128000),
                    engine: Some("ollama".to_string()),
                    api_key: None,
                    env_key: None,
                    tool_protocol: None,
                    enable_thinking: None,
                    max_tokens: None,
                },
                ModelProfile {
                    name: "tinkerer".to_string(),
                    url: "https://tinker.thinkingmachines.dev/services/tinker-prod/oai/api/v1/chat/completions".to_string(),
                    model: "thinkingmachines/Inkling".to_string(),
                    context_window: Some(128000),
                    engine: Some("tinker".to_string()),
                    api_key: None,
                    env_key: Some("TINKER_API_KEY".to_string()),
                    tool_protocol: None,
                    enable_thinking: None,
                    max_tokens: None,
                },
            ],
            tool_protocol: ToolProtocol::default(),
            last_active_session_id: None,
            mcp_servers: vec![McpServerConfig {
                name: "socraticode".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "socraticode@latest".to_string()],
                env: std::collections::HashMap::new(),
                enabled: true,
            }],

            agent_mode: AgentMode::default(),
            verbosity: crate::app::state::Verbosity::default(),
            debug_verbose_network_logging: false,
            theme: default_theme(),
            start_time: None,
            is_valid: true,
        }
    }
}

pub fn get_config_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("RUSTCODE_CONFIG_DIR")
        && !override_dir.trim().is_empty()
    {
        let dir = PathBuf::from(override_dir);
        let _ = fs::create_dir_all(&dir);
        return Some(dir);
    }
    if cfg!(test) {
        let dir = std::env::temp_dir().join("rustcode_test_config");
        let _ = fs::create_dir_all(&dir);
        return Some(dir);
    }
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

pub fn resolve_model_endpoint(config: &AppConfig, name: &str) -> (String, String) {
    config
        .models
        .iter()
        .find(|m| m.name == name)
        .or_else(|| config.models.first())
        .map(|p| (p.url.clone(), p.model.clone()))
        .unwrap_or_else(|| default_endpoint(&AppConfig::default()))
}

pub fn load_config_from(dir: &Path) -> (String, String, AppConfig) {
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
        Err(e) => {
            eprintln!(
                "[rustcode] WARNING: Failed to parse config.toml ({e}). Keeping existing config on disk to prevent overwriting custom profiles."
            );
            let backup_path = file_path.with_extension("toml.bak");
            if let Err(backup_err) = std::fs::copy(&file_path, &backup_path) {
                eprintln!("Warning: could not backup config: {backup_err}");
            } else {
                eprintln!("Backed up malformed config to {}", backup_path.display());
            }
            let mut fallback = default_config;
            fallback.is_valid = false;
            fallback
        }
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

    let (url, model) = resolve_model_endpoint(&config, config.default.big());

    (url, model, config)
}

pub fn save_entire_config(config: &AppConfig) {
    if !config.is_valid {
        return;
    }
    if let Some(dir) = get_config_dir() {
        save_config_to(&dir, config);
    }
}

fn save_config_to(dir: &Path, config: &AppConfig) {
    if !config.is_valid {
        return;
    }
    let _ = fs::create_dir_all(dir);
    if let Ok(toml_str) = toml::to_string_pretty(config) {
        let _ = fs::write(dir.join(CONFIG_FILE), toml_str);
    }
}

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
    pending: Mutex<HashMap<PathBuf, Vec<ChatMessage>>>,
    wakeup: Condvar,
    /// Serializes take-snapshot-then-write, so a slow write of an older
    /// snapshot can never land on top of a newer one when the background
    /// thread and an explicit flush run concurrently.
    write_slot: Mutex<()>,
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

fn queue_history_write(path: PathBuf, history: &[ChatMessage]) {
    let writer = history_writer();
    lock(&writer.pending).insert(path, history.to_vec());
    writer.wakeup.notify_all();
}

fn drain_history_writes(writer: &HistoryWriter) {
    let _slot = lock(&writer.write_slot);
    let batch = std::mem::take(&mut *lock(&writer.pending));
    for (path, history) in batch {
        write_history_file(&path, &history);
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
fn write_history_file(path: &Path, history: &[ChatMessage]) {
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

/// A session is worth showing in `/history` or resuming only once a real
/// exchange happened: a genuine user prompt AND at least one assistant reply.
/// This hides abandoned prompt-only or goal-only sessions (e.g. a `/goal` that
/// never produced output) so they don't bury the real chats or get picked by
/// `/resume`.
pub fn session_is_resumable(history: &[ChatMessage]) -> bool {
    session_has_content(history) && history.iter().any(|m| m.role == "assistant")
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
    // Try to load custom title from session directory
    let title = if let Some(session_dir) = path.parent().and_then(|p| p.parent()) {
        if session_dir
            .components()
            .next_back()
            .map(|c| c.as_os_str() == SESSIONS_DIR)
            .unwrap_or(false)
        {
            if let Some(session_id) = session_dir.file_name() {
                load_session_title(session_id.to_str().unwrap_or(""))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    SessionMeta {
        title: title.unwrap_or_else(|| session_title(history)),
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
pub fn save_history(history: &[ChatMessage]) {
    match active_session_id() {
        Some(session_id) => save_session_history(&session_id, history),
        None => {
            if let Some(dir) = get_config_dir() {
                queue_history_write(dir.join(HISTORY_FILE), history);
            }
        }
    }
}

pub fn save_session_history(session_id: &str, history: &[ChatMessage]) {
    if let Some(dir) = get_config_dir() {
        let path = dir.join(SESSIONS_DIR).join(session_id).join(HISTORY_FILE);
        queue_history_write(path, history);
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
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_millis();
        let session_id = ts.to_string();
        let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
        let _ = fs::create_dir_all(&session_dir);
        let _ = fs::create_dir_all(session_dir.join("sandbox"));
        let _ = fs::create_dir_all(session_dir.join("artifacts"));

        write_history_file(&session_dir.join(HISTORY_FILE), &legacy_history);
        let _ = fs::remove_file(&legacy_history_path);

        config.last_active_session_id = Some(session_id.clone());
        save_config_to(&dir, config);
        set_active_session_id(&session_id);
        return session_id;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_millis();
    let session_id = ts.to_string();
    let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
    let _ = fs::create_dir_all(&session_dir);
    let _ = fs::create_dir_all(session_dir.join("sandbox"));
    let _ = fs::create_dir_all(session_dir.join("artifacts"));

    config.last_active_session_id = Some(session_id.clone());
    save_config_to(&dir, config);
    set_active_session_id(&session_id);
    session_id
}

pub fn create_new_session(config: &mut AppConfig) -> String {
    let dir = match get_config_dir() {
        Some(d) => d,
        None => return "".to_string(),
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_millis();
    let session_id = ts.to_string();
    let session_dir = dir.join(SESSIONS_DIR).join(&session_id);
    let _ = fs::create_dir_all(&session_dir);
    let _ = fs::create_dir_all(session_dir.join("sandbox"));
    let _ = fs::create_dir_all(session_dir.join("artifacts"));

    config.last_active_session_id = Some(session_id.clone());
    save_config_to(&dir, config);
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
    if session_has_content(&load_session_history_direct(&last)) {
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
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let session_dir = dir.join(format!("{ts}"));
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

pub fn list_sessions() -> Vec<SessionMeta> {
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
        .into_iter()
        .filter_map(|p| {
            let history = load_session_file(&p);
            if session_is_resumable(&history) {
                Some(session_meta_from(p, &history))
            } else {
                None
            }
        })
        .collect()
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
        let history = load_session_file(&path);
        if session_is_resumable(&history) {
            return Some(session_meta_from(path, &history));
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

pub fn init_sync_repo(remote_url: &str) -> Result<(), String> {
    let dir = get_config_dir().ok_or("Failed to get config directory")?;
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {e}"))?;
    }

    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        let default_gitignore =
            "debug.log\ndebug.log.*\n*.log\n*.bak\nsymbols.db\ntool_output/\nattachments/\n";
        let _ = fs::write(&gitignore_path, default_gitignore);
    }

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

    let pull_out = std::process::Command::new("git")
        .args(["pull", "--rebase", "origin", "main"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("Failed to pull updates: {e}"))?;

    if pull_out.status.success() {
        let msg = String::from_utf8_lossy(&pull_out.stdout);
        if !msg.contains("Already up to date") && !msg.contains("Current branch main is up to date")
        {
            println!("Pull result: {}", msg.trim());
        }
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&pull_out.stderr);
        Err(format!("Pull failed: {}", err.trim()))
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

    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "device".to_string());

    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        let default_gitignore =
            "debug.log\ndebug.log.*\n*.log\n*.bak\nsymbols.db\ntool_output/\nattachments/\n";
        let _ = fs::write(&gitignore_path, default_gitignore);
    }

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
            .args(["push", "-u", "origin", "main"])
            .current_dir(&dir)
            .output()
            .map_err(|e| format!("Failed to push to remote: {e}"))?;

        if push_out.status.success() {
            println!("Successfully pushed config to remote origin/main! 🚀");
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&push_out.stderr);
            Err(format!("Push failed: {}", err.trim()))
        }
    } else {
        Ok(()) // Nothing to push if nothing was committed
    }
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
        let config = AppConfig {
            default: DefaultConfig::Simple("gemma4:e2b-it-qat".to_string()),
            ..AppConfig::default()
        };
        save_config_to(&dir, &config);

        let (url, model, loaded) = load_config_from(&dir);
        assert_eq!(loaded.default.big(), "gemma4:e2b-it-qat");
        let expected = &loaded
            .models
            .iter()
            .find(|m| m.name == "gemma4:e2b-it-qat")
            .unwrap();
        assert_eq!(url, expected.url);
        assert_eq!(model, expected.model);
    }

    #[test]
    fn test_default_profile_is_source_of_truth() {
        let dir = temp_dir("latest");
        let config = AppConfig {
            default: DefaultConfig::Simple("gemma4:e2b-it-qat".to_string()),
            ..AppConfig::default()
        };
        save_config_to(&dir, &config);

        let (url, model, _) = load_config_from(&dir);
        let expected = &config
            .models
            .iter()
            .find(|m| m.name == "gemma4:e2b-it-qat")
            .unwrap();
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
                .find(|m| m.name == "qwen3.6-dense")
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
        write_history_file(&dir.join(HISTORY_FILE), &msgs);
        let loaded = load_session_file(&dir.join(HISTORY_FILE));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].role, "user");
        assert_eq!(loaded[0].content, "Hello");
        assert_eq!(loaded[1].role, "assistant");
        assert_eq!(loaded[1].content, "Hi there");
    }

    #[test]
    fn test_history_is_written_compactly_and_atomically() {
        let dir = temp_dir("history-compact");
        let msgs = vec![ChatMessage::new("user", "Hello")];
        write_history_file(&dir.join(HISTORY_FILE), &msgs);

        let raw = fs::read_to_string(dir.join(HISTORY_FILE)).unwrap();
        assert!(
            !raw.contains('\n'),
            "history must be compact JSON, got: {raw}"
        );
        assert_eq!(load_session_file(&dir.join(HISTORY_FILE)).len(), 1);

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != HISTORY_FILE)
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write left temp files behind: {leftovers:?}"
        );
    }

    #[test]
    fn test_queued_history_writes_coalesce_and_flush() {
        let dir = temp_dir("history-queue");
        let path = dir.join(HISTORY_FILE);

        for i in 0..5 {
            let msgs: Vec<ChatMessage> = (0..=i)
                .map(|n| ChatMessage::new("user", format!("msg {n}")))
                .collect();
            queue_history_write(path.clone(), &msgs);
        }
        flush_history();

        // The flush must persist the newest snapshot, not an earlier one.
        let loaded = load_session_file(&path);
        assert_eq!(loaded.len(), 5);
        assert_eq!(loaded[4].content, "msg 4");
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
    fn test_history_persists_full_log() {
        let dir = temp_dir("history-full");
        let msgs: Vec<ChatMessage> = (0..80)
            .map(|i| ChatMessage::new("user", format!("msg {}", i)))
            .collect();
        write_history_file(&dir.join(HISTORY_FILE), &msgs);
        let loaded = load_session_file(&dir.join(HISTORY_FILE));
        assert_eq!(loaded.len(), msgs.len());
        assert_eq!(loaded[0].content, "msg 0");
    }

    #[test]
    fn test_default_config_parsing() {
        // String format
        let toml_str1 = r#"default = "my-big-model""#;
        #[derive(Deserialize)]
        struct TempConfig {
            default: DefaultConfig,
        }
        let parsed1: TempConfig = toml::from_str(toml_str1).unwrap();
        assert_eq!(parsed1.default.big(), "my-big-model");
        assert_eq!(parsed1.default.small(), "my-big-model");

        // Table format
        let toml_str2 = r#"
            [default]
            big_model = "my-big-model"
            small_model = "my-small-model"
        "#;
        let parsed2: TempConfig = toml::from_str(toml_str2).unwrap();
        assert_eq!(parsed2.default.big(), "my-big-model");
        assert_eq!(parsed2.default.small(), "my-small-model");

        // Table format (alternate names)
        let toml_str2_alt = r#"
            [default]
            big = "alt-big"
            small = "alt-small"
        "#;
        let parsed2_alt: TempConfig = toml::from_str(toml_str2_alt).unwrap();
        assert_eq!(parsed2_alt.default.big(), "alt-big");
        assert_eq!(parsed2_alt.default.small(), "alt-small");

        // Double brackets format [[default]]
        let toml_str3 = r#"
            [[default]]
            big_model = "my-big-model"
            small_model = "my-small-model"
        "#;
        let parsed3: TempConfig = toml::from_str(toml_str3).unwrap();
        assert_eq!(parsed3.default.big(), "my-big-model");
        assert_eq!(parsed3.default.small(), "my-small-model");
    }

    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_load_valid_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(b"default = { big = \"test_model\", small = \"test_small\" }\n[[models]]\nname = \"test_model\"\nurl = \"http://test/v1/chat/completions\"\nmodel = \"test\"\n").unwrap();

        let (url, model, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), "test_model");
        assert_eq!(config.models[0].name, "test_model");
        assert_eq!(url, "http://test/v1/chat/completions");
        assert_eq!(model, "test");
    }

    #[test]
    fn test_load_invalid_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&config_path).unwrap();
        f.write_all(b"invalid toml content").unwrap();

        let (_url, _model, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), AppConfig::default().default.big());

        let backup_path = dir.path().join("config.toml.bak");
        assert!(backup_path.exists());
    }

    #[test]
    fn test_load_missing_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let (_url, _model, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), AppConfig::default().default.big());

        assert!(!dir.path().join("models.json").exists());
        assert!(!dir.path().join("config.json").exists());
        assert!(!dir.path().join("config.toml").exists());
    }

    #[test]
    fn test_load_json_configuration_files() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("models.json"),
            r#"{
                "default": {"big": "custom", "small": "custom-small"},
                "models": [{
                    "name": "custom",
                    "url": "http://custom/v1/chat/completions",
                    "model": "custom-model"
                }]
            }"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{
                "theme": "nord",
                "tool_protocol": "native"
            }"#,
        )
        .unwrap();

        let (url, model, config) = load_config_from(dir.path());

        assert_eq!(config.default.big(), "custom");
        assert_eq!(config.default.small(), "custom-small");
        assert_eq!(config.models[0].name, "custom");
        assert_eq!(config.theme, "nord");
        assert_eq!(config.tool_protocol, ToolProtocol::Native);
        assert_eq!(url, "http://custom/v1/chat/completions");
        assert_eq!(model, "custom-model");
    }
}
