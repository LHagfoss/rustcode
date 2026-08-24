use crate::app::{ChatMessage, History};
use serde::{Deserialize, Serialize};
use serde_millis;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, atomic::AtomicU64, atomic::Ordering};
use std::time::Duration;

pub const MAX_CONTEXT_TOKENS: u32 = 2048;
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 40;
pub const DEFAULT_SUBAGENT_CONCURRENCY_LIMIT: usize = 4;

pub const MODELS_FILE: &str = "models.json";
pub const CONFIG_FILE: &str = "config.json";
pub const CONFIG_TOML_FILE: &str = "config.toml";
pub const CONFIG_FORMAT_VERSION: u32 = 1;
pub const PROJECT_CONFIG_DIR: &str = ".rustcode";
pub const PROJECT_CONFIG_FILE: &str = "config.toml";
const PROJECT_GITIGNORE_ENTRY: &str = ".rustcode/config.toml";
const HISTORY_FILE: &str = "history.json";
const SESSIONS_DIR: &str = "sessions";
#[allow(dead_code)]
const MAX_SESSIONS: usize = 30;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct ModelProfile {
    pub name: String,
    pub url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// Forces a tool protocol for this profile, overriding provider detection.
    /// Set it when a self-hosted server implements OpenAI-style function
    /// calling (or advertises it but gets it wrong).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_protocol: Option<ToolProtocol>,
    /// Sends Qwen3's top-level `enable_thinking` request field when set.
    /// `Some(false)` skips `<think>` generation entirely at the chat-template
    /// level (much faster, no reasoning trace); `Some(true)` forces it on;
    /// `None` (default) leaves the server's own template default in place —
    /// matches prior behavior for profiles that don't opt in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
    /// Reasoning effort level (e.g. "low", "medium", "high") sent in OpenAI-compatible payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Hard provider-side cap for reasoning tokens when the OpenAI-compatible
    /// endpoint supports the `thinking_budget` extension. Unlike
    /// `reasoning_effort`, this is an explicit token limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    /// Per-profile completion token cap sent as `max_tokens`. `None` falls
    /// back to the shared default, overriding whatever a Modelfile's
    /// `PARAMETER num_predict` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Whether this model accepts image input. `None` means unsupported until
    /// the profile is explicitly configured, avoiding a provider failure as a
    /// capability probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    /// Effective soft context target at which proactive compaction and context
    /// optimization trigger, preventing requests from routinely driving close to
    /// the theoretical maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_context_target: Option<u32>,
    /// Hard upper limit on estimated prompt tokens plus completion reserve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hard_effective_limit: Option<u32>,
    /// Conservative safety margin for provider chat-template framing and tokenizer variations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_overhead_margin: Option<u32>,
}

/// Fallback `max_tokens` used when a `ModelProfile` doesn't set its own.
pub const DEFAULT_REQUEST_MAX_TOKENS: u32 = 32768;

/// The portion of a model context that is intentionally unavailable to the
/// conversation history. Tool schemas, completion/tool-call output, and (when
/// enabled) reasoning all compete with the transcript for the same provider
/// context window, so history must not be budgeted as a fixed percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub context_window: u32,
    pub soft_context_target: u32,
    pub hard_effective_limit: u32,
    pub completion_reserve: u32,
    pub thinking_reserve: u32,
    pub tool_reserve: u32,
    pub safety_reserve: u32,
    pub provider_overhead_margin: u32,
    pub history_tokens: u32,
}

impl ModelProfile {
    pub fn context_budget(&self) -> ContextBudget {
        // Keep the effective value bounded and honest. In particular, do not
        // inflate a deliberately small profile and then send a request that
        // cannot fit the provider's configured window.
        let context_window = self.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW).max(1);
        let default_completion = if self.is_local() {
            (context_window / 8).clamp(1024, 4096)
        } else {
            DEFAULT_REQUEST_MAX_TOKENS.min((context_window / 4).max(1))
        };
        let configured_completion = self.max_tokens.unwrap_or(default_completion);
        let requested_completion = configured_completion
            .min((context_window / 4).max(1))
            .max(1);
        let requested_thinking = if self.enable_thinking == Some(true)
            || self
                .reasoning_effort
                .as_deref()
                .map(|e| e != "off" && e != "none")
                .unwrap_or(false)
        {
            (context_window / 8).clamp(1, 2048)
        } else {
            0
        };
        let requested_tool = (context_window / 16).clamp(1, 4096);
        let requested_safety = (context_window / 32).clamp(1, 1024);

        let provider_overhead_margin = self
            .provider_overhead_margin
            .unwrap_or_else(|| (context_window / 32).clamp(512, 2048))
            .min(context_window.saturating_sub(1));

        let hard_effective_limit = self
            .hard_effective_limit
            .unwrap_or_else(|| context_window.saturating_sub(provider_overhead_margin))
            .clamp(1, context_window);

        let soft_context_target = self
            .soft_context_target
            .unwrap_or_else(|| {
                if self.is_local() {
                    ((context_window as f64 * 0.70) as u32).clamp(1, hard_effective_limit)
                } else {
                    hard_effective_limit
                }
            })
            .clamp(1, hard_effective_limit);

        // Keep the fields honest even for synthetic or unusually small model
        // profiles: the published reserves must never add up to more than the
        // context window, and history always retains a small inspectable tail.
        let mut reserve_capacity = context_window.saturating_sub(requested_completion);
        let completion_reserve = requested_completion;
        let thinking_reserve = requested_thinking.min(reserve_capacity);
        reserve_capacity = reserve_capacity.saturating_sub(thinking_reserve);
        let tool_reserve = requested_tool.min(reserve_capacity);
        reserve_capacity = reserve_capacity.saturating_sub(tool_reserve);
        let safety_reserve = requested_safety.min(reserve_capacity);
        let reserved = completion_reserve
            .saturating_add(thinking_reserve)
            .saturating_add(tool_reserve)
            .saturating_add(safety_reserve);
        let history_tokens = context_window.saturating_sub(reserved);
        ContextBudget {
            context_window,
            soft_context_target,
            hard_effective_limit,
            completion_reserve,
            thinking_reserve,
            tool_reserve,
            safety_reserve,
            provider_overhead_margin,
            history_tokens,
        }
    }
}

impl ModelProfile {
    pub fn image_input_supported(&self) -> Option<bool> {
        self.supports_vision
    }

    pub fn is_local(&self) -> bool {
        if let Some(ref engine) = self.engine {
            let eng = engine.to_ascii_lowercase();
            if matches!(
                eng.as_str(),
                "local"
                    | "ollama"
                    | "llama.cpp"
                    | "llama_cpp"
                    | "lmstudio"
                    | "omlx"
                    | "mlx"
                    | "vllm"
                    | "tgi"
            ) {
                return true;
            }
        }
        let url_lower = self.url.to_ascii_lowercase();
        url_lower.contains("ollama") || url_lower.contains(":11434") || url_lower.contains(":1234")
    }

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
    "open.bigmodel.cn",
    "api.z.ai",
    "z.ai",
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

/// Local audio generation preferences. Backends are external processes and
/// are discovered lazily when an audio tool is called.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AudioConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_audio_backend")]
    pub sfx_backend: String,
    #[serde(default = "default_audio_backend")]
    pub music_backend: String,
}

fn default_audio_backend() -> String {
    "auto".to_string()
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sfx_backend: default_audio_backend(),
            music_backend: default_audio_backend(),
        }
    }
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
#[serde(untagged)]
enum DefaultOverride {
    Simple(String),
    Table {
        #[serde(default, alias = "big_model")]
        big: Option<String>,
        #[serde(default, alias = "small_model")]
        small: Option<String>,
    },
    Array(Vec<DefaultConfigTable>),
}

impl From<DefaultConfig> for DefaultOverride {
    fn from(value: DefaultConfig) -> Self {
        match value {
            DefaultConfig::Simple(value) => Self::Simple(value),
            DefaultConfig::Table { big, small } => Self::Table {
                big: Some(big),
                small: Some(small),
            },
            DefaultConfig::Array(value) => Self::Array(value),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub default: DefaultConfig,
    pub models: Vec<ModelProfile>,
    /// Profile name (or model id) used for image analysis fallback requests.
    #[serde(default)]
    pub vision_model: Option<String>,
    #[serde(default)]
    pub tool_protocol: ToolProtocol,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: usize,
    #[serde(default = "default_subagent_concurrency_limit")]
    pub subagent_concurrency_limit: usize,
    #[serde(default)]
    pub last_active_session_id: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    pub audio: AudioConfig,

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

#[derive(Debug, Serialize, Deserialize)]
struct ModelsConfig {
    default: DefaultConfig,
    models: Vec<ModelProfile>,
    #[serde(default)]
    vision_model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RuntimeConfig {
    #[serde(default)]
    tool_protocol: ToolProtocol,
    #[serde(default = "default_max_tool_rounds")]
    max_tool_rounds: usize,
    #[serde(default = "default_subagent_concurrency_limit")]
    subagent_concurrency_limit: usize,
    #[serde(default)]
    last_active_session_id: Option<String>,
    #[serde(default)]
    mcp_servers: Vec<McpServerConfig>,
    #[serde(default)]
    audio: AudioConfig,
    #[serde(default)]
    agent_mode: AgentMode,
    #[serde(default)]
    verbosity: crate::app::state::Verbosity,
    #[serde(default = "default_false")]
    debug_verbose_network_logging: bool,
    #[serde(default = "default_theme")]
    theme: String,
    #[serde(default)]
    #[serde(with = "serde_millis")]
    start_time: Option<std::time::SystemTime>,
}

/// Canonical, human-editable configuration file.
///
/// Fields are optional so users can keep a small hand-written config while
/// the runtime still supplies defaults for everything they omit. The two
/// JSON files remain readable as a compatibility path for older installs.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct TomlConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<DefaultOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    models: Option<Vec<ModelProfile>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vision_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_protocol: Option<ToolProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_tool_rounds: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subagent_concurrency_limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_active_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_servers: Option<Vec<McpServerConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audio: Option<AudioConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_mode: Option<AgentMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verbosity: Option<crate::app::state::Verbosity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    debug_verbose_network_logging: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "serde_millis"
    )]
    start_time: Option<std::time::SystemTime>,
}

fn default_false() -> bool {
    false
}

fn default_max_tool_rounds() -> usize {
    DEFAULT_MAX_TOOL_ROUNDS
}

fn default_subagent_concurrency_limit() -> usize {
    DEFAULT_SUBAGENT_CONCURRENCY_LIMIT
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
                    reasoning_effort: None,
                    max_tokens: None,
                    supports_vision: Some(false),
                    ..Default::default()
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
                    reasoning_effort: None,
                    max_tokens: None,
                    supports_vision: Some(true),
                    ..Default::default()
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
                    reasoning_effort: None,
                    max_tokens: None,
                    supports_vision: Some(true),
                    ..Default::default()
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
                    reasoning_effort: None,
                    max_tokens: None,
                    supports_vision: Some(false),
                    ..Default::default()
                },
            ],
            tool_protocol: ToolProtocol::default(),
            max_tool_rounds: DEFAULT_MAX_TOOL_ROUNDS,
            subagent_concurrency_limit: DEFAULT_SUBAGENT_CONCURRENCY_LIMIT,
            vision_model: Some("gemini-3.6-flash".to_string()),
            last_active_session_id: None,
            mcp_servers: vec![McpServerConfig {
                name: "socraticode".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "socraticode@latest".to_string()],
                env: std::collections::HashMap::new(),
                enabled: true,
            }],
            audio: AudioConfig::default(),

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
    #[cfg(test)]
    {
        let dir = test_config_dir();
        let _ = fs::create_dir_all(&dir);
        return Some(dir);
    }
    #[cfg(not(test))]
    {
        #[cfg(windows)]
        let config_root = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("LOCALAPPDATA"))
            .map(PathBuf::from)?;

        #[cfg(not(windows))]
        let config_root = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

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

        return Some(dir);
    }
}

#[cfg(test)]
fn test_config_dir() -> PathBuf {
    let thread = std::thread::current();
    let identity = thread
        .name()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:?}", thread.id()));
    let suffix: String = identity
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    std::env::temp_dir().join(format!(
        "rustcode_test_config_{}_{}",
        std::process::id(),
        suffix
    ))
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

/// Load global configuration and overlay project configuration files from
/// repository ancestors. Later (closer) project files take precedence over
/// earlier ones, and all project files take precedence over the global file.
pub fn load_config_for_workspace(workspace: &Path) -> (String, String, AppConfig) {
    let (_, _, mut config) = load_config();
    for path in project_config_paths(workspace) {
        match read_toml_config(&path) {
            Ok(file) => apply_project_toml_config(&mut config, file),
            Err(error) => eprintln!("[rustcode] WARNING: {error}"),
        }
    }
    let (url, model) = resolve_model_endpoint(&config, config.default.big());
    (url, model, config)
}

fn project_config_paths(workspace: &Path) -> Vec<PathBuf> {
    let workspace = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut ancestors: Vec<PathBuf> = workspace.ancestors().map(Path::to_path_buf).collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|ancestor| ancestor.join(PROJECT_CONFIG_DIR).join(PROJECT_CONFIG_FILE))
        .filter(|path| path.is_file())
        .collect()
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
    let defaults = AppConfig::default();
    let mut config = defaults.clone();
    let mut is_valid = true;

    let toml_path = dir.join(CONFIG_TOML_FILE);
    if toml_path.exists() {
        match fs::read_to_string(&toml_path)
            .ok()
            .and_then(|content| toml::from_str::<TomlConfig>(&content).ok())
        {
            Some(file) => {
                if let Some(version) = file.version
                    && version > CONFIG_FORMAT_VERSION
                {
                    eprintln!(
                        "[rustcode] WARNING: {} uses unsupported config format version {} (this version supports up to {}). Using built-in defaults.",
                        toml_path.display(),
                        version,
                        CONFIG_FORMAT_VERSION
                    );
                    is_valid = false;
                } else {
                    apply_toml_config(&mut config, file);
                }
            }
            None => {
                eprintln!(
                    "[rustcode] WARNING: Failed to parse {}. Using built-in defaults.",
                    toml_path.display()
                );
                is_valid = false;
            }
        }
    } else {
        // Compatibility path for pre-0.30 installations. A successful load
        // is migrated to config.toml on the next normal save.
        let models_path = dir.join(MODELS_FILE);
        if models_path.exists() {
            match fs::read_to_string(&models_path)
                .ok()
                .and_then(|content| serde_json::from_str::<ModelsConfig>(&content).ok())
            {
                Some(models) => {
                    config.default = models.default;
                    config.models = models.models;
                    config.vision_model = models.vision_model.or(config.vision_model);
                }
                None => {
                    eprintln!(
                        "[rustcode] WARNING: Failed to parse {}. Using built-in model defaults.",
                        models_path.display()
                    );
                    is_valid = false;
                }
            }
        }

        let runtime_path = dir.join(CONFIG_FILE);
        if runtime_path.exists() {
            match fs::read_to_string(&runtime_path)
                .ok()
                .and_then(|content| serde_json::from_str::<RuntimeConfig>(&content).ok())
            {
                Some(runtime) => {
                    config.tool_protocol = runtime.tool_protocol;
                    config.max_tool_rounds = runtime.max_tool_rounds;
                    config.subagent_concurrency_limit = runtime.subagent_concurrency_limit;
                    config.last_active_session_id = runtime.last_active_session_id;
                    config.mcp_servers = runtime.mcp_servers;
                    config.agent_mode = runtime.agent_mode;
                    config.verbosity = runtime.verbosity;
                    config.debug_verbose_network_logging = runtime.debug_verbose_network_logging;
                    config.theme = runtime.theme;
                    config.start_time = runtime.start_time;
                    config.audio = runtime.audio;
                }
                None => {
                    eprintln!(
                        "[rustcode] WARNING: Failed to parse {}. Using built-in runtime defaults.",
                        runtime_path.display()
                    );
                    is_valid = false;
                }
            }
        }
    }

    config.is_valid = is_valid;

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
        let mut persisted = config.clone();
        if let Ok(workspace) = std::env::current_dir() {
            let (_, _, global) = load_config_from(&dir);
            for path in project_config_paths(&workspace) {
                if let Ok(file) = read_toml_config(&path) {
                    preserve_project_overrides(&mut persisted, &global, &file);
                }
            }
        }
        save_config_to(&dir, &persisted);
    }
}

fn save_config_to(dir: &Path, config: &AppConfig) {
    if !config.is_valid {
        return;
    }
    if let Err(error) = fs::create_dir_all(dir) {
        eprintln!(
            "[rustcode] WARNING: Failed to create config directory {}: {error}",
            dir.display()
        );
        return;
    }

    let file = TomlConfig {
        version: Some(CONFIG_FORMAT_VERSION),
        default: Some(config.default.clone().into()),
        models: Some(config.models.clone()),
        vision_model: config.vision_model.clone(),
        tool_protocol: Some(config.tool_protocol),
        max_tool_rounds: Some(config.max_tool_rounds),
        subagent_concurrency_limit: Some(config.subagent_concurrency_limit),
        last_active_session_id: config.last_active_session_id.clone(),
        mcp_servers: Some(config.mcp_servers.clone()),
        audio: Some(config.audio.clone()),
        agent_mode: Some(config.agent_mode),
        verbosity: Some(config.verbosity.clone()),
        debug_verbose_network_logging: Some(config.debug_verbose_network_logging),
        theme: Some(config.theme.clone()),
        start_time: config.start_time,
    };

    match toml::to_string_pretty(&file) {
        Ok(contents) => {
            if let Err(error) = write_config_file(&dir.join(CONFIG_TOML_FILE), &contents) {
                eprintln!("[rustcode] WARNING: Failed to save config.toml: {error}");
            }
        }
        Err(error) => eprintln!("[rustcode] WARNING: Failed to serialize config.toml: {error}"),
    }
}

fn read_toml_config(path: &Path) -> Result<TomlConfig, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let file = toml::from_str::<TomlConfig>(&contents)
        .map_err(|error| format!("Failed to parse {}: {error}", path.display()))?;
    if let Some(version) = file.version
        && version > CONFIG_FORMAT_VERSION
    {
        return Err(format!(
            "{} uses unsupported config format version {} (this version supports up to {})",
            path.display(),
            version,
            CONFIG_FORMAT_VERSION
        ));
    }
    Ok(file)
}

fn apply_toml_config(config: &mut AppConfig, file: TomlConfig) {
    if let Some(default) = file.default {
        apply_default_override(config, default);
    }
    if let Some(models) = file.models {
        config.models = models;
    }
    if let Some(vision_model) = file.vision_model {
        config.vision_model = Some(vision_model);
    }
    if let Some(tool_protocol) = file.tool_protocol {
        config.tool_protocol = tool_protocol;
    }
    if let Some(max_tool_rounds) = file.max_tool_rounds {
        config.max_tool_rounds = max_tool_rounds;
    }
    if let Some(limit) = file.subagent_concurrency_limit {
        config.subagent_concurrency_limit = limit;
    }
    if let Some(session_id) = file.last_active_session_id {
        config.last_active_session_id = Some(session_id);
    }
    if let Some(mcp_servers) = file.mcp_servers {
        config.mcp_servers = mcp_servers;
    }
    if let Some(audio) = file.audio {
        config.audio = audio;
    }
    if let Some(agent_mode) = file.agent_mode {
        config.agent_mode = agent_mode;
    }
    if let Some(verbosity) = file.verbosity {
        config.verbosity = verbosity;
    }
    if let Some(enabled) = file.debug_verbose_network_logging {
        config.debug_verbose_network_logging = enabled;
    }
    if let Some(theme) = file.theme {
        config.theme = theme;
    }
    if file.start_time.is_some() {
        config.start_time = file.start_time;
    }
}

fn apply_default_override(config: &mut AppConfig, default: DefaultOverride) {
    match default {
        DefaultOverride::Simple(name) => config.default = DefaultConfig::Simple(name),
        DefaultOverride::Table { big, small } => {
            let current_big = config.default.big().to_string();
            let current_small = config.default.small().to_string();
            config.default = DefaultConfig::Table {
                big: big.unwrap_or(current_big),
                small: small.unwrap_or(current_small),
            };
        }
        DefaultOverride::Array(values) => config.default = DefaultConfig::Array(values),
    }
}

fn apply_project_toml_config(config: &mut AppConfig, mut file: TomlConfig) {
    // Session state belongs to the user config, never to a project checkout.
    file.last_active_session_id = None;
    file.start_time = None;
    apply_toml_config(config, file);
}

fn preserve_project_overrides(persisted: &mut AppConfig, global: &AppConfig, file: &TomlConfig) {
    if file.default.is_some() {
        persisted.default = global.default.clone();
    }
    if file.models.is_some() {
        persisted.models = global.models.clone();
    }
    if file.vision_model.is_some() {
        persisted.vision_model = global.vision_model.clone();
    }
    if file.tool_protocol.is_some() {
        persisted.tool_protocol = global.tool_protocol;
    }
    if file.max_tool_rounds.is_some() {
        persisted.max_tool_rounds = global.max_tool_rounds;
    }
    if file.subagent_concurrency_limit.is_some() {
        persisted.subagent_concurrency_limit = global.subagent_concurrency_limit;
    }
    if file.mcp_servers.is_some() {
        persisted.mcp_servers = global.mcp_servers.clone();
    }
    if file.audio.is_some() {
        persisted.audio = global.audio.clone();
    }
    if file.agent_mode.is_some() {
        persisted.agent_mode = global.agent_mode;
    }
    if file.verbosity.is_some() {
        persisted.verbosity = global.verbosity.clone();
    }
    if file.debug_verbose_network_logging.is_some() {
        persisted.debug_verbose_network_logging = global.debug_verbose_network_logging;
    }
    if file.theme.is_some() {
        persisted.theme = global.theme.clone();
    }
}

/// Create a small project override from global model selection. Deliberately
/// do not copy model profiles, API keys, MCP servers, session state, or other
/// machine-specific state into a project file.
pub fn init_project_config(workspace: &Path) -> Result<PathBuf, String> {
    let workspace = fs::canonicalize(workspace).map_err(|error| {
        format!(
            "could not resolve workspace {}: {error}",
            workspace.display()
        )
    })?;
    let project_dir = workspace.join(PROJECT_CONFIG_DIR);
    let path = project_dir.join(PROJECT_CONFIG_FILE);
    if path.exists() {
        return Err(format!("project config already exists: {}", path.display()));
    }

    let (_, _, global) = load_config();
    let file = TomlConfig {
        version: Some(CONFIG_FORMAT_VERSION),
        default: Some(global.default.into()),
        models: None,
        vision_model: None,
        tool_protocol: None,
        max_tool_rounds: None,
        subagent_concurrency_limit: None,
        last_active_session_id: None,
        mcp_servers: None,
        audio: None,
        agent_mode: None,
        verbosity: None,
        debug_verbose_network_logging: None,
        theme: None,
        start_time: None,
    };
    let contents = toml::to_string_pretty(&file)
        .map_err(|error| format!("could not serialize project config: {error}"))?;

    fs::create_dir_all(&project_dir)
        .map_err(|error| format!("could not create {}: {error}", project_dir.display()))?;
    write_config_file(&path, &contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    ensure_project_gitignore(&workspace)?;
    Ok(path)
}

fn ensure_project_gitignore(workspace: &Path) -> Result<(), String> {
    let path = workspace.join(".gitignore");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current
        .lines()
        .any(|line| line.trim() == PROJECT_GITIGNORE_ENTRY)
    {
        return Ok(());
    }

    let mut updated = current;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(PROJECT_GITIGNORE_ENTRY);
    updated.push('\n');
    fs::write(&path, updated)
        .map_err(|error| format!("could not update {}: {error}", path.display()))
}

fn write_config_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    fs::write(&temporary, contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }

    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) => {
            // Windows does not replace an existing file with rename. Keep the
            // old file until the new file is ready, then replace it.
            let backup = path.with_file_name(format!(".{file_name}.bak-{}", std::process::id()));
            if path.exists() {
                fs::rename(path, &backup)?;
            }
            match fs::rename(&temporary, path) {
                Ok(()) => {
                    let _ = fs::remove_file(backup);
                    Ok(())
                }
                Err(error) => {
                    let _ = fs::rename(&backup, path);
                    let _ = fs::remove_file(&temporary);
                    Err(error)
                }
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
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

fn next_session_id_value(now: u64, previous: u64) -> u64 {
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

fn queue_history_write(path: PathBuf, history: &[ChatMessage], revision: Option<u64>) -> bool {
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
    fn session_id_allocator_advances_when_clock_repeats() {
        assert_eq!(next_session_id_value(1_000, 0), 1_000);
        assert_eq!(next_session_id_value(1_000, 1_000), 1_001);
        assert_eq!(next_session_id_value(999, 1_001), 1_002);
    }

    #[test]
    fn test_config_directory_is_unique_to_the_test_thread() {
        let dir = get_config_dir().expect("test config directory");
        assert!(
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rustcode_test_config_")),
            "unexpected test config directory: {}",
            dir.display()
        );
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
    fn context_budget_reserves_completion_thinking_tools_and_safety() {
        let mut profile = AppConfig::default().models[0].clone();
        profile.context_window = Some(4096);
        profile.max_tokens = Some(2048);
        profile.enable_thinking = Some(true);
        let budget = profile.context_budget();
        assert_eq!(budget.context_window, 4096);
        assert!(budget.completion_reserve > 0);
        assert!(budget.thinking_reserve > 0);
        assert!(budget.tool_reserve > 0);
        assert!(budget.history_tokens < budget.context_window);
        assert_eq!(
            budget.history_tokens
                + budget.completion_reserve
                + budget.thinking_reserve
                + budget.tool_reserve
                + budget.safety_reserve,
            budget.context_window
        );

        profile.context_window = Some(512);
        let tiny = profile.context_budget();
        assert_eq!(
            tiny.history_tokens
                + tiny.completion_reserve
                + tiny.thinking_reserve
                + tiny.tool_reserve
                + tiny.safety_reserve,
            tiny.context_window
        );
    }

    #[test]
    fn local_default_completion_cap_is_4096_and_explicit_max_tokens_is_preserved() {
        let mut profile = ModelProfile {
            name: "local-ollama".to_string(),
            url: "http://127.0.0.1:11434/v1/chat/completions".to_string(),
            model: "qwen2.5:32b".to_string(),
            context_window: Some(128_000),
            engine: Some("ollama".to_string()),
            ..ModelProfile::default()
        };

        assert_eq!(profile.context_budget().completion_reserve, 4096);

        profile.max_tokens = Some(8192);
        assert_eq!(profile.context_budget().completion_reserve, 8192);
    }

    #[test]
    fn context_budget_scales_without_double_reserving_large_or_small_windows() {
        let mut profile = AppConfig::default().models[0].clone();
        profile.max_tokens = Some(u32::MAX);
        for window in [
            1, 32, 64, 128, 256, 512, 4_096, 8_192, 32_768, 128_000, 262_144,
        ] {
            profile.context_window = Some(window);
            profile.enable_thinking = Some(false);
            let budget = profile.context_budget();
            assert_eq!(budget.context_window, window.max(1));
            assert_eq!(
                budget.history_tokens
                    + budget.completion_reserve
                    + budget.thinking_reserve
                    + budget.tool_reserve
                    + budget.safety_reserve,
                budget.context_window
            );
            assert!(
                budget.context_window < 4 || budget.completion_reserve <= budget.context_window / 4
            );
            if window > 1 {
                assert!(budget.history_tokens > 0);
            }

            profile.enable_thinking = Some(true);
            let thinking = profile.context_budget();
            if window >= 64 {
                assert!(thinking.thinking_reserve > 0);
                assert!(thinking.history_tokens < budget.history_tokens);
            }
        }
    }

    #[test]
    fn tool_round_limit_round_trips_through_runtime_config() {
        let dir = temp_dir("tool_round_limit");
        let mut config = AppConfig::default();
        config.max_tool_rounds = 17;
        save_config_to(&dir, &config);

        let (_, _, loaded) = load_config_from(&dir);
        assert_eq!(loaded.max_tool_rounds, 17);
    }

    #[test]
    fn older_runtime_config_defaults_subagent_concurrency_limit() {
        let dir = temp_dir("legacy_subagent_concurrency_limit");
        std::fs::write(dir.join(CONFIG_FILE), "{}").unwrap();

        let (_, _, loaded) = load_config_from(&dir);

        assert_eq!(loaded.subagent_concurrency_limit, 4);
    }

    #[test]
    fn subagent_concurrency_limit_round_trips_through_runtime_config() {
        let dir = temp_dir("subagent_concurrency_limit");
        let mut config = AppConfig::default();
        config.subagent_concurrency_limit = 2;
        save_config_to(&dir, &config);

        let (_, _, loaded) = load_config_from(&dir);

        assert_eq!(loaded.subagent_concurrency_limit, 2);
    }

    #[test]
    fn image_input_capability_is_explicit_and_vision_profile_is_configurable() {
        let mut profile = AppConfig::default().models[0].clone();
        assert_eq!(profile.image_input_supported(), Some(false));

        profile.supports_vision = Some(true);
        assert_eq!(profile.image_input_supported(), Some(true));

        let mut config = AppConfig::default();
        config.vision_model = Some("vision-helper".to_string());
        let json = serde_json::to_string(&config).unwrap();
        let decoded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.vision_model.as_deref(), Some("vision-helper"));
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
            queue_history_write(path.clone(), &msgs, None);
        }
        flush_history();

        // The flush must persist the newest snapshot, not an earlier one.
        let loaded = load_session_file(&path);
        assert_eq!(loaded.len(), 5);
        assert_eq!(loaded[4].content, "msg 4");
    }

    #[test]
    fn revisioned_history_write_skips_an_identical_pending_snapshot() {
        let dir = temp_dir("history-queue-dedup");
        let path = dir.join(HISTORY_FILE);
        let mut history = crate::app::History::default();
        history.push(ChatMessage::new("user", "same"));
        let revision = history.revision();

        assert!(queue_history_write(
            path.clone(),
            history.as_slice(),
            Some(revision)
        ));
        assert!(!queue_history_write(
            path,
            history.as_slice(),
            Some(revision)
        ));
        flush_history();
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

    use tempfile::TempDir;

    #[test]
    fn test_load_valid_config() {
        let dir = TempDir::new().unwrap();
        let models = r#"{
            "default": {"big": "test_model", "small": "test_small"},
            "models": [{"name": "test_model", "url": "http://test/v1/chat/completions", "model": "test"}]
        }"#;
        fs::write(dir.path().join(MODELS_FILE), models).unwrap();

        let (url, model, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), "test_model");
        assert_eq!(config.models[0].name, "test_model");
        assert_eq!(url, "http://test/v1/chat/completions");
        assert_eq!(model, "test");
    }

    #[test]
    fn test_load_invalid_config_returns_default() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join(CONFIG_FILE);
        fs::write(&config_path, b"invalid json content").unwrap();

        let (_url, _model, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), AppConfig::default().default.big());
        assert!(!config.is_valid);

        assert_eq!(fs::read(&config_path).unwrap(), b"invalid json content");
        assert!(!dir.path().join("config.json.bak").exists());
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

    #[test]
    fn test_malformed_json_is_preserved() {
        let dir = TempDir::new().unwrap();
        let malformed_models = b"{ malformed models";
        let malformed_runtime = b"{ malformed runtime";
        fs::write(dir.path().join("models.json"), malformed_models).unwrap();
        fs::write(dir.path().join("config.json"), malformed_runtime).unwrap();

        let (_, _, config) = load_config_from(dir.path());

        let defaults = AppConfig::default();
        assert_eq!(config.default.big(), defaults.default.big());
        assert_eq!(config.models, defaults.models);
        assert_eq!(config.theme, defaults.theme);
        assert_eq!(config.tool_protocol, defaults.tool_protocol);
        assert!(!config.is_valid);
        assert_eq!(
            fs::read(dir.path().join("models.json")).unwrap(),
            malformed_models
        );
        assert_eq!(
            fs::read(dir.path().join("config.json")).unwrap(),
            malformed_runtime
        );
        assert!(!dir.path().join("models.json.bak").exists());
        assert!(!dir.path().join("config.json.bak").exists());
    }

    #[test]
    fn test_malformed_models_preserves_valid_runtime_config() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(MODELS_FILE), b"not json").unwrap();
        fs::write(
            dir.path().join(CONFIG_FILE),
            r#"{"theme":"nord","tool_protocol":"native"}"#,
        )
        .unwrap();

        let (_, _, config) = load_config_from(dir.path());

        assert_eq!(config.theme, "nord");
        assert_eq!(config.tool_protocol, ToolProtocol::Native);
        assert_eq!(config.default.big(), AppConfig::default().default.big());
        assert!(!config.is_valid);
    }

    #[test]
    fn test_empty_models_use_default_endpoint() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(MODELS_FILE),
            r#"{"default":{"big":"missing","small":"missing"},"models":[]}"#,
        )
        .unwrap();

        let (url, model, config) = load_config_from(dir.path());
        let defaults = AppConfig::default();
        assert!(config.models.is_empty());
        assert_eq!(url, defaults.models[0].url);
        assert_eq!(model, defaults.models[0].model);
    }

    #[test]
    fn test_config_save_writes_versioned_toml_without_split_json() {
        let dir = TempDir::new().unwrap();
        let mut config = AppConfig::default();
        config.default = DefaultConfig::Simple("custom".to_string());
        config.models[0].name = "custom".to_string();
        config.theme = "nord".to_string();
        config.tool_protocol = ToolProtocol::Native;

        save_config_to(dir.path(), &config);

        let path = dir.path().join(CONFIG_TOML_FILE);
        assert!(path.exists());
        assert!(!dir.path().join(MODELS_FILE).exists());
        assert!(!dir.path().join(CONFIG_FILE).exists());
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("version = 1"));
        let (_, _, loaded) = load_config_from(dir.path());
        assert_eq!(loaded.default.big(), "custom");
        assert_eq!(loaded.theme, "nord");
        assert_eq!(loaded.tool_protocol, ToolProtocol::Native);
    }

    #[test]
    fn test_legacy_json_is_migrated_when_saved() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(MODELS_FILE),
            r#"{
                "default": "legacy-model",
                "models": [{"name":"legacy-model","url":"http://legacy","model":"legacy"}]
            }"#,
        )
        .unwrap();

        let (_, _, config) = load_config_from(dir.path());
        assert_eq!(config.default.big(), "legacy-model");
        save_config_to(dir.path(), &config);

        assert!(dir.path().join(CONFIG_TOML_FILE).exists());
        assert!(dir.path().join(MODELS_FILE).exists());
        let (_, _, migrated) = load_config_from(dir.path());
        assert_eq!(migrated.default.big(), "legacy-model");
    }

    #[test]
    fn test_invalid_toml_is_preserved_and_does_not_fall_back_to_legacy_json() {
        let dir = TempDir::new().unwrap();
        let invalid = b"[models\nnot valid";
        fs::write(dir.path().join(CONFIG_TOML_FILE), invalid).unwrap();
        fs::write(
            dir.path().join(MODELS_FILE),
            r#"{"default":"legacy","models":[]}"#,
        )
        .unwrap();

        let (_, _, config) = load_config_from(dir.path());

        assert_eq!(config.default.big(), AppConfig::default().default.big());
        assert!(!config.is_valid);
        assert_eq!(
            fs::read(dir.path().join(CONFIG_TOML_FILE)).unwrap(),
            invalid
        );
    }

    #[test]
    fn test_unsupported_toml_version_is_rejected() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join(CONFIG_TOML_FILE),
            format!("version = {}\n", CONFIG_FORMAT_VERSION + 1),
        )
        .unwrap();

        let (_, _, config) = load_config_from(dir.path());

        assert_eq!(config.default.big(), AppConfig::default().default.big());
        assert!(!config.is_valid);
    }

    #[test]
    fn project_config_overrides_global_defaults_from_near_to_far() {
        let root = TempDir::new().unwrap();
        let workspace = root.path().join("nested");
        fs::create_dir_all(workspace.join(PROJECT_CONFIG_DIR)).unwrap();
        fs::create_dir_all(root.path().join(PROJECT_CONFIG_DIR)).unwrap();
        fs::write(
            root.path()
                .join(PROJECT_CONFIG_DIR)
                .join(PROJECT_CONFIG_FILE),
            "version = 1\n[default]\nbig = \"parent\"\nsmall = \"parent-small\"\n",
        )
        .unwrap();
        fs::write(
            workspace.join(PROJECT_CONFIG_DIR).join(PROJECT_CONFIG_FILE),
            "version = 1\n[default]\nbig = \"child\"\n",
        )
        .unwrap();

        let (_, _, config) = load_config_for_workspace(&workspace);

        assert_eq!(config.default.big(), "child");
        assert_eq!(config.default.small(), "parent-small");
    }

    #[test]
    fn project_overrides_are_not_persisted_into_global_config() {
        let global = AppConfig::default();
        let mut merged = global.clone();
        let project: TomlConfig = toml::from_str(
            "version = 1\n[default]\nbig = \"project\"\nsmall = \"project-small\"\n",
        )
        .unwrap();
        apply_project_toml_config(&mut merged, project.clone());
        assert_eq!(merged.default.big(), "project");

        preserve_project_overrides(&mut merged, &global, &project);

        assert_eq!(merged.default.big(), global.default.big());
        assert_eq!(merged.default.small(), global.default.small());
    }

    #[test]
    fn project_init_writes_safe_template_and_gitignore_entry() {
        let workspace = TempDir::new().unwrap();
        let path = init_project_config(workspace.path()).unwrap();

        assert_eq!(
            path,
            fs::canonicalize(workspace.path())
                .unwrap()
                .join(PROJECT_CONFIG_DIR)
                .join(PROJECT_CONFIG_FILE)
        );
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("version = 1"));
        assert!(contents.contains("[default]"));
        assert!(!contents.contains("api_key"));
        assert!(!contents.contains("mcp_servers"));
        assert_eq!(
            fs::read_to_string(workspace.path().join(".gitignore")).unwrap(),
            ".rustcode/config.toml\n"
        );
        assert!(init_project_config(workspace.path()).is_err());
    }

    #[test]
    fn test_provider_supports_function_calling_includes_zai() {
        assert!(provider_supports_function_calling(
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        ));
        assert!(provider_supports_function_calling(
            "https://api.z.ai/v1/chat/completions"
        ));
    }

    #[test]
    fn test_ensure_sync_gitignore_creates_and_updates() {
        let dir = TempDir::new().unwrap();
        ensure_sync_gitignore(dir.path()).unwrap();
        let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("sessions/*/sandbox/"));
        assert!(content.contains("sessions/*/artifacts/"));
        assert!(content.contains("sessions/*/subagents/"));
        assert!(content.contains("sessions/*/image_cache.json"));
        assert!(content.contains("*.bak"));

        // Test updating existing with missing entries
        let custom_dir = TempDir::new().unwrap();
        fs::write(custom_dir.path().join(".gitignore"), "custom_entry\n").unwrap();
        ensure_sync_gitignore(custom_dir.path()).unwrap();
        let updated = fs::read_to_string(custom_dir.path().join(".gitignore")).unwrap();
        assert!(updated.starts_with("custom_entry\n"));
        assert!(updated.contains("sessions/*/sandbox/"));
    }

    #[test]
    fn test_get_sync_branch_fallback() {
        let dir = TempDir::new().unwrap();
        // Non-git directory falls back to main
        assert_eq!(get_sync_branch(dir.path()), "main");
    }

    #[test]
    fn test_load_session_meta_fast_path() {
        let dir = TempDir::new().unwrap();
        let session_dir = dir.path().join(SESSIONS_DIR).join("12345");
        fs::create_dir_all(&session_dir).unwrap();
        let history_file = session_dir.join(HISTORY_FILE);

        let json = r#"[
            {"role": "user", "content": "hello world\nsecond line", "timestamp": "12:00", "images": ["massive_base64_data_12345"]},
            {"role": "assistant", "content": "hi there", "timestamp": "12:01"}
        ]"#;
        fs::write(&history_file, json).unwrap();

        let meta = load_session_meta(&history_file).expect("should parse meta");
        assert_eq!(meta.title, "hello world");
        assert_eq!(meta.when, "12:00");
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.path, history_file);
        assert_eq!(
            session_id_from_path(&history_file).as_deref(),
            Some("12345")
        );
    }

    #[test]
    fn test_load_session_meta_unresumable_abandoned_session() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.json");
        // User prompt with no assistant reply is not resumable
        let json = r#"[{"role": "user", "content": "unfinished", "timestamp": "12:00"}]"#;
        fs::write(&file, json).unwrap();
        assert!(load_session_meta(&file).is_none());
    }

    #[test]
    fn test_session_id_from_path_variations() {
        let path1 = PathBuf::from("/home/user/.config/rustcode/sessions/sess-abc/history.json");
        assert_eq!(session_id_from_path(&path1).as_deref(), Some("sess-abc"));

        let path2 = PathBuf::from("/home/user/.config/rustcode/sessions/sess-xyz.json");
        assert_eq!(session_id_from_path(&path2).as_deref(), Some("sess-xyz"));

        let path3 = PathBuf::from("/tmp/history.json");
        assert_eq!(session_id_from_path(&path3), None);
    }
}
