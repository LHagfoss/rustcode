use crate::atomic_file::replace_file;
use serde::{Deserialize, Serialize};
use serde_millis;
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_CONTEXT_TOKENS: u32 = 2048;
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;
pub const DEFAULT_MAX_TOOL_ROUNDS: usize = 40;
pub const DEFAULT_SUBAGENT_CONCURRENCY_LIMIT: usize = 4;
/// Tool rounds should be short and action-oriented. Reasoning models often
/// spend their entire completion allowance thinking before emitting a tool
/// call; a smaller cap keeps local agent turns responsive while final prose
/// still uses the profile's full completion budget.
pub const DEFAULT_TOOL_ROUND_MAX_TOKENS: u32 = 8192;

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
    /// Return the completion cap for one request. Tool-enabled requests are
    /// deliberately bounded for reasoning-capable models: a long speculative
    /// chain is expensive and cannot be useful until it produces an action.
    /// Requests without tools retain the configured cap for normal answers.
    pub fn completion_token_limit(&self, allow_tools: bool) -> u32 {
        let configured = self.context_budget().completion_reserve;
        let reasoning_capable = self.enable_thinking == Some(true)
            || self.reasoning_effort.as_deref().is_some_and(|effort| {
                !effort.eq_ignore_ascii_case("off") && !effort.eq_ignore_ascii_case("none")
            })
            || self.thinking_budget.is_some();
        if allow_tools && reasoning_capable {
            configured.min(DEFAULT_TOOL_ROUND_MAX_TOKENS)
        } else {
            configured
        }
    }

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

    let result = replace_file(&temporary, path);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

mod session;
pub use session::*;

#[cfg(test)]
use session::{next_session_id_value, queue_history_write, write_history_file};

#[cfg(test)]
mod tests;
