use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone)]
pub enum AppStatus {
    Idle,
    Streaming,
    Queued,

    AwaitingToolConfirmation,
    AwaitingQuestion,
    VerbosityPicker,
    ThinkingPicker,
    ProtocolPicker,
}

#[derive(Debug, Clone)]
pub struct ToolConfirmation {
    pub tool_name: String,
    pub path: String,
    pub content_preview: String,
    pub content_bytes: usize,
}

/// An interactive `ask_question` prompt awaiting the user's choice. Rendered as
/// a modal; the selected option(s) are sent back to the agent as the tool result.
#[derive(Debug, Clone)]
pub struct PendingQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub is_multi_select: bool,
    /// Currently highlighted index. Valid range is `0..=options.len()`, where the
    /// final index (`options.len()`) is the always-present "write your own answer" slot.
    pub selected: usize,
    /// For multi-select: which options are ticked (parallel to `options`).
    pub chosen: Vec<bool>,
    /// When `Some`, the user is typing a freeform answer (the "write your own"
    /// slot is active); the string is the in-progress text.
    pub custom_input: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
}

/// Approximate token count accumulated during the current streaming reply.
/// Updated incrementally as SSE chunks arrive; used to compute Tokens/s in the footer.
pub const TOKENS_PER_CHAR_APPROX: f64 = 0.25;

#[derive(Debug, Clone)]
pub struct StreamTracker {
    pub tokens_so_far: u32,
    /// Updated each time a chunk is received; used for per-second rate.
    pub last_update: std::time::Instant,
    prev_tokens: u32,
    history: std::collections::VecDeque<(std::time::Instant, u32)>,
}

impl StreamTracker {
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            tokens_so_far: 0,
            last_update: now,
            prev_tokens: 0,
            history: std::collections::VecDeque::new(),
        }
    }

    /// Called each time a new chunk arrives during streaming. Updates the history.
    pub fn record_chunk(&mut self) {
        let now = std::time::Instant::now();
        let delta = self.tokens_so_far.saturating_sub(self.prev_tokens);
        if delta > 0 {
            self.history.push_back((now, delta));
        }
        self.prev_tokens = self.tokens_so_far;
        self.last_update = now;

        let cutoff = now
            .checked_sub(std::time::Duration::from_millis(1500))
            .unwrap_or(now);
        while let Some(&(time, _)) = self.history.front() {
            if time < cutoff {
                self.history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Returns the current sliding window tokens/sec and total approximated tokens.
    pub fn snapshot(&self) -> (f64, u32) {
        let now = std::time::Instant::now();

        let window_duration = std::time::Duration::from_secs(1);
        let cutoff = now.checked_sub(window_duration).unwrap_or(now);

        let mut total_tokens_in_window = 0;
        let mut first_time_in_window = None;
        let mut last_time_in_window = None;

        for &(time, tokens) in &self.history {
            if time >= cutoff {
                total_tokens_in_window += tokens;
                if first_time_in_window.is_none() {
                    first_time_in_window = Some(time);
                }
                last_time_in_window = Some(time);
            }
        }

        if total_tokens_in_window == 0 {
            return (0.0, self.tokens_so_far);
        }

        // To calculate rate, divide by the actual elapsed time between first and last chunks in the window.
        // If there's only one chunk, default to a minimum time of 0.1s to avoid extreme spikes.
        let elapsed = if let (Some(first), Some(last)) = (first_time_in_window, last_time_in_window)
        {
            (last - first).as_secs_f64().max(0.1)
        } else {
            1.0
        };

        let raw_tps = total_tokens_in_window as f64 / elapsed;

        let silence = (now - self.last_update).as_secs_f64();
        let tps = if silence > 0.5 {
            let decay = (-silence / 0.5).exp();
            raw_tps * decay
        } else {
            raw_tps
        };

        (tps.max(0.0), self.tokens_so_far)
    }
}

fn current_timestamp() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

pub fn random_tip_index() -> usize {
    use rand::RngExt;
    let mut rng = rand::rng();
    rng.random_range(0..TIPS.len())
}

pub const TIPS: &[&str] = &[
    "Use /tools to see what the agent can do",
    "Ask the agent to fix a TODO or explain a file",
    "Press Ctrl+P to open the command palette",
    "Tab auto-completes slash commands",
    "Switch models anytime with /model <name>",
    "Use /usage to see token and response stats",
    "Esc interrupts a running generation",
    "The agent can grep, glob, read, edit, and run commands",
    "Hold Shift+Enter for multi-line input",
    "Type /info for basic info, or /help for all commands and keybindings",
];

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
    #[serde(default = "current_timestamp")]
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(skip)]
    pub diff: Option<String>,
    /// Ephemeral normal code preview for newly written files. It is derived
    /// from the tool arguments and intentionally not persisted in history.
    #[serde(skip)]
    pub file_preview: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResultRecord>,
    /// Structured tool calls this assistant message made, in order. Present only
    /// when the provider returned real function calls; the text protocols write
    /// calls as prose, which has no identity to record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
    /// For a `tool` message: the id of the call this result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Identity of one structured tool call, kept so a result can name the call it
/// answers when the transcript is replayed to the provider.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCallRef {
    pub id: String,
    pub name: String,
    /// Arguments exactly as the provider sent them, so the replayed call is
    /// byte-identical to the one the model made.
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultRecord {
    pub tool_name: String,
    pub arguments_hash: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub changed_paths: Vec<String>,
    pub truncated: bool,
    pub full_output_artifact: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedReadOutput {
    pub(crate) replayable_content: Option<String>,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) truncated: bool,
    pub(crate) full_output_artifact: Option<String>,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            token_usage: None,
            timestamp: current_timestamp(),
            response_time_ms: None,
            diff: None,
            file_preview: None,
            tool_result: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// Attach the structured calls this assistant message made.
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallRef>) -> Self {
        self.tool_calls = calls;
        self
    }

    /// Mark this tool result as the answer to `call_id`.
    pub fn answering(mut self, call_id: Option<String>) -> Self {
        self.tool_call_id = call_id;
        self
    }

    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff;
        self
    }

    pub fn with_file_preview(mut self, preview: Option<(String, String)>) -> Self {
        self.file_preview = preview;
        self
    }

    pub fn with_tool_result(mut self, record: ToolResultRecord) -> Self {
        self.tool_result = Some(record);
        self
    }

    /// Resolves tool calls from structured `tool_calls` fields (ApiNative) if present,
    /// or parses tool calls from text content for text protocols.
    pub fn resolved_tool_calls(&self, protocol: crate::config::ToolProtocol) -> Vec<crate::tools::ToolCall> {
        if !self.tool_calls.is_empty() {
            self.tool_calls
                .iter()
                .filter_map(|tc| {
                    let args = serde_json::from_str(&tc.arguments).unwrap_or(serde_json::Value::Null);
                    Some(crate::tools::ToolCall {
                        name: tc.name.clone(),
                        arguments: args,
                    })
                })
                .collect()
        } else {
            crate::tools::parse_tool_calls(&self.content, protocol)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A subagent spawned by the main agent via the spawn_agent tool. Keeps its
/// own conversation history and explicit lifecycle state.
pub struct SubAgent {
    pub id: u32,
    pub task: String,
    pub model: Option<String>,
    pub history: Vec<ChatMessage>,
    pub status: SubAgentStatus,
    pub write_access: bool,
    pub allowed_paths: Vec<String>,
    pub verification_command: Option<String>,
    pub workspace_root: Option<std::path::PathBuf>,
    pub review_manifest: Option<std::path::PathBuf>,
}

/// One entry of the agent's persistent task plan, managed via the `todo_write` tool.
/// The current list is re-injected into the system prompt every round so the agent
/// can execute its plan across turns instead of re-planning from scratch.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TodoItem {
    pub content: String,
    pub status: String,   // "pending" | "in_progress" | "completed"
    pub priority: String, // "high" | "medium" | "low"
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpEditState {
    pub is_add: bool,
    pub edit_index: Option<usize>,
    pub name_input: String,
    pub command_input: String,
    pub args_input: String,
    pub active_field: usize, // 0 = Name, 1 = Command, 2 = Args
    pub cursor_pos: usize,
}

impl McpEditState {
    pub fn active_buf_and_pos_mut(&mut self) -> (&mut String, &mut usize) {
        match self.active_field {
            0 => (&mut self.name_input, &mut self.cursor_pos),
            1 => (&mut self.command_input, &mut self.cursor_pos),
            _ => (&mut self.args_input, &mut self.cursor_pos),
        }
    }

    pub fn set_active_field(&mut self, field: usize) {
        self.active_field = field % 3;
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = buf.len();
    }

    pub fn insert_char(&mut self, c: char) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        buf.insert(*pos, c);
        *pos += c.len_utf8();
    }

    pub fn delete_char_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos > 0
            && let Some(c) = buf[..*pos].chars().next_back()
        {
            let len = c.len_utf8();
            *pos -= len;
            buf.remove(*pos);
        }
    }

    pub fn delete_char_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos < buf.len() {
            buf.remove(*pos);
        }
    }

    pub fn delete_word_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos == 0 {
            return;
        }
        let end = *pos;
        let mut start = *pos;
        while start > 0
            && buf[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace())
        {
            if let Some(c) = buf[..start].chars().next_back() {
                start -= c.len_utf8();
            }
        }
        while start > 0
            && buf[..start]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
        {
            if let Some(c) = buf[..start].chars().next_back() {
                start -= c.len_utf8();
            }
        }
        buf.drain(start..end);
        *pos = start;
    }

    pub fn delete_line_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        buf.drain(0..*pos);
        *pos = 0;
    }

    pub fn move_cursor_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos > 0
            && let Some(c) = buf[..*pos].chars().next_back()
        {
            *pos -= c.len_utf8();
        }
    }

    pub fn move_cursor_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        if *pos < buf.len()
            && let Some(c) = buf[*pos..].chars().next()
        {
            *pos += c.len_utf8();
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        let mut p = *pos;
        while p > 0
            && buf[..p]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace())
        {
            if let Some(c) = buf[..p].chars().next_back() {
                p -= c.len_utf8();
            }
        }
        while p > 0
            && buf[..p]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
        {
            if let Some(c) = buf[..p].chars().next_back() {
                p -= c.len_utf8();
            }
        }
        *pos = p;
    }

    pub fn move_cursor_word_right(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = (*pos).min(buf.len());
        let mut p = *pos;
        while p < buf.len() && buf[p..].chars().next().is_some_and(|c| c.is_whitespace()) {
            if let Some(c) = buf[p..].chars().next() {
                p += c.len_utf8();
            }
        }
        while p < buf.len() && buf[p..].chars().next().is_some_and(|c| !c.is_whitespace()) {
            if let Some(c) = buf[p..].chars().next() {
                p += c.len_utf8();
            }
        }
        *pos = p;
    }

    pub fn move_cursor_home(&mut self) {
        let (_, pos) = self.active_buf_and_pos_mut();
        *pos = 0;
    }

    pub fn move_cursor_end(&mut self) {
        let (buf, pos) = self.active_buf_and_pos_mut();
        *pos = buf.len();
    }
}

/// Identity of the static prompt artifacts: they only depend on these inputs
/// plus the MCP tool generation. When any differs, the cache rebuilds.
#[derive(Clone, PartialEq)]
struct PromptCacheKey {
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
    mcp_generation: u64,
}

/// Pre-computed static system prompt and native tool schema.
///
/// Both are otherwise rebuilt on every completion turn — including a filesystem
/// skill scan and a full MCP schema re-serialization — even though they only
/// change when the tool protocol, agent mode, or MCP tool set changes. This
/// caches them and rebuilds lazily only when [`PromptCacheKey`] moves (the MCP
/// generation acting as the dirty flag for mid-session server changes).
#[derive(Default)]
pub struct PromptCache {
    key: Option<PromptCacheKey>,
    system_prompt: String,
    native_schema: Vec<serde_json::Value>,
}

impl PromptCache {
    fn ensure(
        &mut self,
        include_agent_tools: bool,
        protocol: crate::config::ToolProtocol,
        agent_mode: crate::config::AgentMode,
    ) {
        let key = PromptCacheKey {
            include_agent_tools,
            protocol,
            agent_mode,
            mcp_generation: crate::mcp::mcp_generation(),
        };
        if self.key.as_ref() != Some(&key) {
            self.system_prompt =
                crate::tools::tool_system_prompt(include_agent_tools, protocol, agent_mode);
            self.native_schema = crate::tools::native_tools_schema(include_agent_tools);
            self.key = Some(key);
        }
    }

    /// The cached static system prompt for these inputs, rebuilding if stale.
    pub fn system_prompt(
        &mut self,
        include_agent_tools: bool,
        protocol: crate::config::ToolProtocol,
        agent_mode: crate::config::AgentMode,
    ) -> &str {
        self.ensure(include_agent_tools, protocol, agent_mode);
        &self.system_prompt
    }

    /// The cached native tool schema for these inputs, rebuilding if stale.
    pub fn native_schema(
        &mut self,
        include_agent_tools: bool,
        protocol: crate::config::ToolProtocol,
        agent_mode: crate::config::AgentMode,
    ) -> &[serde_json::Value] {
        self.ensure(include_agent_tools, protocol, agent_mode);
        &self.native_schema
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Notice,
    Warning,
}

pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
    pub shown_at: std::time::Instant,
}

/// What the pointer is currently over. Only clickable things get a variant, so
/// comparing the previous and current target tells the event loop whether a
/// pointer move actually changed anything worth redrawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HoverTarget {
    #[default]
    None,
    /// The jump-to-latest pill.
    ScrollPill,
    /// A collapsible thought header, at this screen row.
    ThoughtHeader(u16),
    /// A code block's `[Copy]` badge, at this screen row.
    CopyBadge(u16),
}

#[derive(Debug, PartialEq, Clone, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    #[default]
    Low,
    High,
}

pub struct AppState {
    pub input_buffer: String,
    pub history: Vec<ChatMessage>,
    pub current_response: String,
    pub current_token_usage: Option<TokenUsage>,
    pub model_quota_remaining: Option<f32>,
    pub pending_queue: Vec<String>,
    pub status: AppStatus,
    /// Single-flight guard for the agent loop. `status` transiently reads Idle
    /// in windows where an orchestrator is still alive, so gating spawns on it
    /// let a background wakeup (or the event-loop drainer) start a *second*
    /// concurrent orchestrator — two turns streaming the same history produced
    /// duplicate assistant messages. Spawns gate on this instead.
    pub orchestrator_running: bool,
    pub cursor_position: usize,

    pub suggestion_cycle: crate::app::suggestion::SuggestionCycle,
    pub response_time: Option<std::time::Duration>,
    pub history_index: Option<usize>,
    pub temp_input: String,
    /// Every input the user submitted this run, oldest first — both plain text
    /// and slash commands. Arrow-up/down recalls from this, not from chat
    /// history (commands never become chat messages, and generated user blobs
    /// like /goal's "Goal: ..." text aren't typed input).
    pub input_history: Vec<String>,

    pub api_base_url: String,
    pub model_name: String,
    /// Probed function-calling support, keyed by endpoint URL. Populated once
    /// per endpoint per run; a negative result is also written back to the
    /// profile so later runs skip the probe.
    pub function_calling_support: std::collections::HashMap<String, bool>,
    pub config: crate::config::AppConfig,

    pub cwd_and_branch: String,
    /// Workspace root supplied by an external frontend such as ACP.
    pub workspace_root: Option<std::path::PathBuf>,

    pub update_check: crate::update::UpdateState,

    pub active_suggestion_index: Option<usize>,

    pub show_model_picker: bool,
    pub model_picker_index: usize,
    pub modal_picker_index: usize,
    pub model_picker_search: String,

    pub show_theme_picker: bool,
    pub theme_picker_index: usize,
    pub theme_picker_initial: String,

    pub show_command_picker: bool,
    pub command_picker_index: usize,
    pub command_picker_search: String,

    pub show_history_picker: bool,
    pub history_picker_index: usize,
    pub history_picker_sessions: Vec<crate::config::SessionMeta>,
    pub history_picker_truncated: bool,
    pub pending_delete_session_idx: Option<usize>,
    pub active_session_id: String,

    pub show_mcp_config: bool,
    pub mcp_picker_index: usize,
    pub mcp_edit_state: Option<McpEditState>,

    pub last_copy_text: Option<(String, std::time::Instant)>,
    pub generation_start_time: Option<std::time::Instant>,
    pub pending_tool_confirmation: Option<Vec<ToolConfirmation>>,
    pub modal_scroll_row: u16,

    pub tool_confirmation_response: Option<tokio::sync::oneshot::Sender<bool>>,

    /// Active interactive `ask_question` prompt and the channel that delivers the
    /// user's selection back to the awaiting tool call.
    pub pending_question: Option<PendingQuestion>,
    pub question_response: Option<tokio::sync::oneshot::Sender<String>>,

    /// The names of user-approved tools currently running in the background.
    /// While this is not empty, the modal overlay stays closed and the user can
    /// keep working normally.
    pub running_tools: Vec<String>,

    pub stream_tracker: Option<StreamTracker>,

    pub auto_confirm: bool,

    pub subagents: Vec<SubAgent>,
    pub delegation_armed: bool,
    pub delegation_active: bool,
    pub continuous_mode: bool,
    pub next_subagent_id: u32,

    /// Persistent task plan, written via the `todo_write` agent tool.
    pub todos: Vec<TodoItem>,

    /// File paths the agent has read this session, mapped to the file's mtime at
    /// read time. Surfaced back to the model so it doesn't re-read unchanged files,
    /// and used by the repeat guard to ALLOW re-reads when a file changed on disk.
    pub read_file_mtimes: std::collections::HashMap<String, std::time::SystemTime>,

    /// Signatures of recent read-only tool calls, used by the repeat-loop guard
    /// to short-circuit identical re-reads (e.g. viewing the same file twice).
    pub recent_read_calls: std::collections::VecDeque<String>,

    /// Structured facts from recent reads, keyed by the same signature. Content
    /// is retained separately only for small reads, while failure, truncation,
    /// and bounded-output recovery metadata remain available for every entry.
    pub recent_read_outputs: std::collections::HashMap<String, CachedReadOutput>,

    pub scroll_row: u16,
    pub is_scroll_locked_to_bottom: bool,
    pub last_max_scroll: u16,
    pub viewport_height: u16,
    pub mouse_capture_enabled: bool,
    pub agent_mode: crate::config::AgentMode,
    pub chat_area: Option<ratatui::layout::Rect>,
    /// Screen rect of the editable text region inside the bottom input box, so
    /// mouse selection can work there too (distinct from `chat_area`).
    pub input_text_area: Option<ratatui::layout::Rect>,
    pub scroll_to_bottom_btn: Option<ratatui::layout::Rect>,
    /// Clickable element the pointer is over, refreshed on every mouse move.
    pub hover: HoverTarget,
    pub selected_text: Option<String>,
    /// Transient top-right toast: (message, shown_at). Auto-expires (~3s) — the
    /// render path checks elapsed time, so no timer/event is needed to clear it.
    pub notice: Option<Notice>,
    pub sel_start: Option<(u16, u16)>,
    pub sel_end: Option<(u16, u16)>,
    pub selecting: bool,
    /// True when the active selection lives in the input box rather than the
    /// chat. Input has no scroll offset, so highlight/extract use scroll_row 0.
    pub sel_in_input: bool,
    pub expanded_thoughts: std::collections::HashSet<usize>,
    pub thought_toggle_rows: Vec<(u16, usize)>,
    /// Screen rows carrying a code-block `[Copy]` badge, mapped to the block's
    /// text, for click-to-copy hit-testing.
    pub code_copy_rows: Vec<(u16, String)>,

    /// Timestamp of the last escape key press (for double-esc detection)
    pub last_escape_time: Option<std::time::Instant>,

    pub raw_cli_mode: bool,
    pub tip_index: usize,

    pub current_terminal_title: Option<String>,
    /// Cached custom session title: the session id it was read for, and the
    /// title found on disk (`None` when that session has no `title.txt`).
    /// Keeps the draw loop from hitting the filesystem on every frame.
    pub session_title_cache: Option<(String, Option<String>)>,
    /// Set by background tasks that mutate state outside the input path, so the
    /// draw loop knows to render once even though no key was pressed.
    pub redraw_requested: bool,

    /// Snapshot of environment context from the first turn, used for delta diffing.
    pub context_snapshot: Option<crate::context::ContextSnapshot>,
    /// Cached static system prompt + tool schema, rebuilt only when the tool
    /// protocol, agent mode, or MCP tool set changes.
    pub prompt_cache: PromptCache,
    pub verbosity: Verbosity,
}

fn get_cwd_and_branch() -> String {
    let absolute_path = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let path_with_tildes = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && absolute_path.starts_with(&home) => {
            absolute_path.replacen(&home, "~", 1)
        }
        _ => absolute_path,
    };

    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                std::str::from_utf8(&out.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "main".to_string());

    format!("{}:{}", path_with_tildes, branch)
}

impl AppState {
    /// Custom title of the active session, read from disk at most once per
    /// session id. Call [`AppState::invalidate_session_title_cache`] after
    /// writing a new title so the next lookup picks it up.
    pub fn cached_session_title(&mut self) -> Option<String> {
        if let Some((cached_id, title)) = &self.session_title_cache
            && *cached_id == self.active_session_id
        {
            return title.clone();
        }
        let title = crate::config::load_session_title(&self.active_session_id);
        self.session_title_cache = Some((self.active_session_id.clone(), title.clone()));
        title
    }

    /// Forget the cached session title so it is re-read from disk on the next
    /// draw. Use after `save_session_title`.
    pub fn invalidate_session_title_cache(&mut self) {
        self.session_title_cache = None;
    }

    /// Ask the draw loop for one more frame. Background tasks that mutate state
    /// while the app is otherwise idle should call this, since the loop no
    /// longer redraws on a fixed timer.
    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    /// Consume a pending redraw request.
    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_requested)
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_row = self.last_max_scroll;
    }

    pub fn new() -> Self {
        let (api_base_url, model_name, mut config) = crate::config::load_config();
        config.start_time = Some(std::time::SystemTime::now());
        crate::config::save_entire_config(&config);
        let active_session_id = crate::config::start_session(&mut config);
        let agent_mode = config.agent_mode;
        let verbosity = config.verbosity.clone();
        let history = Vec::new();
        let cwd_and_branch = get_cwd_and_branch();
        crate::ui::theme::ensure_themes_dir();

        let mut app = Self {
            input_buffer: String::new(),
            scroll_to_bottom_btn: None,
            hover: HoverTarget::None,
            history,
            current_response: String::new(),
            current_token_usage: None,
            model_quota_remaining: None,
            pending_queue: Vec::new(),
            status: AppStatus::Idle,
            orchestrator_running: false,
            cursor_position: 0,
            suggestion_cycle: crate::app::suggestion::SuggestionCycle::new(),
            response_time: None,
            history_index: None,
            temp_input: String::new(),
            input_history: Vec::new(),
            api_base_url,
            function_calling_support: std::collections::HashMap::new(),
            model_name,
            config,
            cwd_and_branch,
            workspace_root: None,
            update_check: crate::update::UpdateState::Unknown,
            active_suggestion_index: None,
            show_model_picker: false,
            model_picker_index: 0,
            modal_picker_index: 0,
            model_picker_search: String::new(),
            show_theme_picker: false,
            theme_picker_index: 0,
            theme_picker_initial: String::new(),
            verbosity,
            show_command_picker: false,
            command_picker_index: 0,
            command_picker_search: String::new(),
            show_history_picker: false,
            history_picker_index: 0,
            history_picker_sessions: Vec::new(),
            history_picker_truncated: false,
            pending_delete_session_idx: None,
            show_mcp_config: false,
            mcp_picker_index: 0,
            mcp_edit_state: None,
            last_copy_text: None,
            generation_start_time: None,
            pending_tool_confirmation: None,
            modal_scroll_row: 0,
            tool_confirmation_response: None,
            pending_question: None,
            question_response: None,
            running_tools: Vec::new(),
            stream_tracker: None,
            auto_confirm: false,
            active_session_id,
            subagents: Vec::new(),
            delegation_armed: false,
            delegation_active: false,
            next_subagent_id: 1,
            todos: Vec::new(),
            read_file_mtimes: std::collections::HashMap::new(),
            recent_read_calls: std::collections::VecDeque::new(),
            recent_read_outputs: std::collections::HashMap::new(),
            scroll_row: 0,
            is_scroll_locked_to_bottom: true,
            current_terminal_title: None,
            session_title_cache: None,
            redraw_requested: false,
            last_max_scroll: 0,
            viewport_height: 0,
            mouse_capture_enabled: true,
            agent_mode,
            chat_area: None,
            input_text_area: None,
            selected_text: None,
            notice: None,
            sel_start: None,
            sel_end: None,
            selecting: false,
            sel_in_input: false,
            expanded_thoughts: std::collections::HashSet::new(),
            thought_toggle_rows: Vec::new(),
            code_copy_rows: Vec::new(),

            last_escape_time: None,

            raw_cli_mode: false,
            tip_index: random_tip_index(),
            continuous_mode: false,
            context_snapshot: None,
            prompt_cache: PromptCache::default(),
        };
        let last_system_content = app
            .history
            .iter()
            .rfind(|m| m.role == "system" && !m.content.contains("Loop warning:"))
            .map(|m| m.content.clone());
        if let Some(content) = last_system_content {
            app.set_notice(content);
        }
        app
    }

    /// True when any modal overlay is open (pickers or tool confirmation);
    /// the background content renders dimmed.
    pub fn modal_open(&self) -> bool {
        self.show_model_picker
            || self.show_theme_picker
            || self.show_command_picker
            || self.show_history_picker
            || self.show_mcp_config
            || self.status == AppStatus::AwaitingToolConfirmation
            || self.status == AppStatus::AwaitingQuestion
            || self.status == AppStatus::VerbosityPicker
            || self.status == AppStatus::ThinkingPicker
            || self.status == AppStatus::ProtocolPicker
    }

    /// Returns the auto-confirm status label for the UI footer.
    pub fn auto_confirm_status_text(&self) -> &'static str {
        if self.auto_confirm { "ON" } else { "OFF" }
    }

    /// Context window of the active profile, in tokens.
    pub fn active_context_window(&self) -> u32 {
        self.config
            .models
            .iter()
            .find(|m| m.model == self.model_name || m.name == self.model_name)
            .or_else(|| {
                self.config
                    .models
                    .iter()
                    .find(|m| m.name == self.config.default.big())
            })
            .and_then(|p| p.context_window)
            .unwrap_or(crate::config::DEFAULT_CONTEXT_WINDOW)
    }

    pub fn get_history_token_budget(&self) -> u32 {
        let cw = self.active_context_window();
        // Use 75% of the model's context window as the history budget.
        (cw as f64 * 0.75) as u32
    }

    fn clamp_cursor(&mut self) {
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        while !self.input_buffer.is_char_boundary(self.cursor_position) {
            self.cursor_position -= 1;
        }
    }

    fn char_len_before_cursor(&self) -> Option<usize> {
        self.input_buffer[..self.cursor_position]
            .chars()
            .next_back()
            .map(|c| c.len_utf8())
    }

    pub fn insert_char(&mut self, c: char) {
        self.history_index = None;
        self.clamp_cursor();
        self.input_buffer.insert(self.cursor_position, c);
        self.cursor_position += c.len_utf8();
        self.reset_suggestion_index();
    }

    pub fn delete_char_backspace(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        if let Some(len) = self.char_len_before_cursor() {
            self.cursor_position -= len;
            self.input_buffer.remove(self.cursor_position);
        }
        self.reset_suggestion_index();
    }

    pub fn delete_char_delete(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        if self.cursor_position < self.input_buffer.len() {
            self.input_buffer.remove(self.cursor_position);
        }
        self.reset_suggestion_index();
    }

    pub fn delete_word_backspace(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let end = self.cursor_position;
        self.move_cursor_word_left();
        let start = self.cursor_position;
        if start < end {
            self.input_buffer.replace_range(start..end, "");
        }
        self.reset_suggestion_index();
    }

    pub fn delete_word_forward(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let start = self.cursor_position;
        self.move_cursor_word_right();
        let end = self.cursor_position;
        self.cursor_position = start;
        if start < end {
            self.input_buffer.replace_range(start..end, "");
        }
        self.reset_suggestion_index();
    }

    pub fn kill_line_to_start(&mut self) {
        self.history_index = None;
        self.clamp_cursor();
        let end = self.cursor_position;
        let start = self.input_buffer[..end].rfind('\n').map_or(0, |i| i + 1);
        if start < end {
            self.input_buffer.replace_range(start..end, "");
            self.cursor_position = start;
        }
        self.reset_suggestion_index();
    }

    pub fn reset_suggestion_index(&mut self) {
        if self.input_buffer.starts_with('/') && !self.input_buffer.contains(' ') {
            if self.active_suggestion_index.is_none() {
                self.active_suggestion_index = Some(0);
            }
        } else {
            self.active_suggestion_index = None;
        }
    }

    pub fn move_cursor_left(&mut self) {
        self.clamp_cursor();
        if let Some(len) = self.char_len_before_cursor() {
            self.cursor_position -= len;
        }
    }

    pub fn move_cursor_right(&mut self) {
        self.clamp_cursor();
        if let Some(c) = self.input_buffer[self.cursor_position..].chars().next() {
            self.cursor_position += c.len_utf8();
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        self.clamp_cursor();
        let mut pos = self.cursor_position;

        while let Some(c) = self.input_buffer[..pos].chars().next_back() {
            if !c.is_whitespace() {
                break;
            }
            pos -= c.len_utf8();
        }

        while let Some(c) = self.input_buffer[..pos].chars().next_back() {
            if c.is_whitespace() {
                break;
            }
            pos -= c.len_utf8();
        }
        self.cursor_position = pos;
    }

    pub fn move_cursor_word_right(&mut self) {
        self.clamp_cursor();
        let mut pos = self.cursor_position;

        while let Some(c) = self.input_buffer[pos..].chars().next() {
            if c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }

        while let Some(c) = self.input_buffer[pos..].chars().next() {
            if !c.is_whitespace() {
                break;
            }
            pos += c.len_utf8();
        }
        self.cursor_position = pos;
    }

    pub fn move_cursor_to_start(&mut self) {
        self.cursor_position = 0;
    }

    pub fn move_cursor_to_end(&mut self) {
        self.cursor_position = self.input_buffer.len();
    }

    pub fn move_cursor_line_up(&mut self) {
        self.clamp_cursor();
        let pos = self.cursor_position;
        let before = &self.input_buffer[..pos];
        let current_line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let col = before[current_line_start..].chars().count();

        if current_line_start > 0 {
            let prev_line_end = current_line_start - 1;
            let prev_line_start = self.input_buffer[..prev_line_end]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            let prev_line = &self.input_buffer[prev_line_start..prev_line_end];
            let prev_char_count = prev_line.chars().count();
            let target_col = col.min(prev_char_count);
            let target_byte_offset: usize = prev_line
                .chars()
                .take(target_col)
                .map(|c| c.len_utf8())
                .sum();
            self.cursor_position = prev_line_start + target_byte_offset;
        } else {
            self.cursor_position = 0;
        }
    }

    pub fn move_cursor_line_down(&mut self) {
        self.clamp_cursor();
        let pos = self.cursor_position;
        let before = &self.input_buffer[..pos];
        let current_line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let col = before[current_line_start..].chars().count();

        if let Some(next_line_start_rel) = self.input_buffer[pos..].find('\n') {
            let next_line_start = pos + next_line_start_rel + 1;
            let next_line_end = self.input_buffer[next_line_start..]
                .find('\n')
                .map_or(self.input_buffer.len(), |i| next_line_start + i);
            let next_line = &self.input_buffer[next_line_start..next_line_end];
            let next_char_count = next_line.chars().count();
            let target_col = col.min(next_char_count);
            let target_byte_offset: usize = next_line
                .chars()
                .take(target_col)
                .map(|c| c.len_utf8())
                .sum();
            self.cursor_position = next_line_start + target_byte_offset;
        } else {
            self.cursor_position = self.input_buffer.len();
        }
    }

    pub fn get_command_suggestion(&self) -> Option<String> {
        self.suggestion_cycle
            .get_completion_suffix(&self.input_buffer)
    }

    pub fn cycle_suggestion(&mut self) {
        if self.suggestion_cycle.cycle(&self.input_buffer) {
            let prefix = self
                .suggestion_cycle
                .original_prefix
                .as_deref()
                .unwrap_or(&self.input_buffer);
            let matches: Vec<&str> = crate::app::suggestion::COMMANDS
                .iter()
                .map(|c| c.name)
                .filter(|c| c.starts_with(prefix))
                .collect();
            if let Some(idx) = self.suggestion_cycle.suggestion_index
                && idx < matches.len()
            {
                self.input_buffer = matches[idx].to_string();
                self.cursor_position = self.input_buffer.len();
            }
        }
    }

    pub fn reset_suggestion_cycle(&mut self) {
        self.suggestion_cycle.reset();
    }

    /// Pull the most recently queued prompt back into the input box so the
    /// user can edit or drop it. Internal wakeup entries are left untouched.
    /// Returns true when a prompt was pulled.
    pub fn pop_queued_prompt(&mut self) -> bool {
        let Some(pos) = self
            .pending_queue
            .iter()
            .rposition(|item| !item.starts_with("__task_wakeup__:"))
        else {
            return false;
        };
        self.input_buffer = self.pending_queue.remove(pos);
        self.cursor_position = self.input_buffer.len();
        true
    }

    pub fn history_up(&mut self) {
        let user_msgs = &self.input_history;
        if user_msgs.is_empty() {
            return;
        }

        let next_idx = match self.history_index {
            None => {
                self.temp_input = self.input_buffer.clone();
                user_msgs.len() - 1
            }
            Some(idx) => {
                if idx > 0 {
                    idx - 1
                } else {
                    0
                }
            }
        };

        self.history_index = Some(next_idx);
        self.input_buffer = user_msgs[next_idx].clone();
        self.cursor_position = self.input_buffer.len();
    }

    pub fn history_down(&mut self) {
        let user_msgs = &self.input_history;
        if user_msgs.is_empty() {
            return;
        }

        if let Some(idx) = self.history_index {
            if idx + 1 < user_msgs.len() {
                self.history_index = Some(idx + 1);
                self.input_buffer = user_msgs[idx + 1].clone();
                self.cursor_position = self.input_buffer.len();
            } else {
                self.history_index = None;
                self.input_buffer = self.temp_input.clone();
                self.cursor_position = self.input_buffer.len();
            }
        }
    }

    pub fn scroll_up(&mut self, amount: u16) {
        self.clear_selection();
        self.is_scroll_locked_to_bottom = false;
        self.scroll_row = self.scroll_row.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: u16) {
        self.clear_selection();
        let max = self.last_max_scroll;
        let next = self.scroll_row.saturating_add(amount).min(max);
        self.scroll_row = next;
        if next >= max {
            self.is_scroll_locked_to_bottom = true;
        }
    }

    /// One page = the visible conversation height, minus a line of overlap for context.
    pub fn page_rows(&self) -> u16 {
        self.viewport_height.saturating_sub(1).max(1)
    }

    /// Which clickable element sits under a screen cell. Hit-tested against the
    /// rects and rows the last render recorded, so it is only meaningful for
    /// coordinates from the current frame.
    pub fn hover_target_at(&self, column: u16, row: u16) -> HoverTarget {
        if let Some(rect) = self.scroll_to_bottom_btn
            && rect.contains(ratatui::layout::Position::new(column, row))
        {
            return HoverTarget::ScrollPill;
        }
        // Chat rows are recorded in screen coordinates by the renderer.
        if self.thought_toggle_rows.iter().any(|(r, _)| *r == row) {
            return HoverTarget::ThoughtHeader(row);
        }
        if let Some((_, code_text)) = self.code_copy_rows.iter().find(|(r, _)| *r == row) {
            let badge_width = if self
                .last_copy_text
                .as_ref()
                .is_some_and(|(t_text, t)| t_text == code_text && t.elapsed().as_secs() < 2)
            {
                12
            } else {
                9
            };
            let is_on_copy_button = self.chat_area.map_or(true, |ca| {
                column >= (ca.x + ca.width).saturating_sub(badge_width)
            });
            if is_on_copy_button {
                return HoverTarget::CopyBadge(row);
            }
        }
        HoverTarget::None
    }

    /// Tool protocol to use when talking to `url`.
    ///
    /// A profile override wins; otherwise a provider known to implement
    /// function calling gets the structured protocol, because a call returned
    /// as data cannot be confused with prose about a call. Everything else
    /// falls back to the configured text protocol, which is what servers
    /// without function calling need.
    pub fn tool_protocol_for(&self, url: &str) -> crate::config::ToolProtocol {
        if let Some(profile) = self.config.models.iter().find(|profile| profile.url == url)
            && let Some(protocol) = profile.tool_protocol
        {
            return protocol;
        }
        if crate::config::provider_supports_function_calling(url)
            || self.function_calling_support.get(url).copied() == Some(true)
        {
            return crate::config::ToolProtocol::ApiNative;
        }
        self.config.tool_protocol
    }

    /// True when this endpoint's function-calling support is still unknown, so
    /// the caller should probe before building a turn.
    pub fn function_calling_unknown(&self, url: &str) -> bool {
        let overridden = self
            .config
            .models
            .iter()
            .any(|profile| profile.url == url && profile.tool_protocol.is_some());
        !overridden
            && !crate::config::provider_supports_function_calling(url)
            && !self.function_calling_support.contains_key(url)
    }

    /// Record what a probe found, for this run only.
    ///
    /// Deliberately not written to the config file: the answer can change when
    /// a gateway is reconfigured, and silently editing settings the user did not
    /// write would make that change impossible to notice. One tiny request per
    /// endpoint per run is cheaper than a wrong answer cached forever. Use
    /// `/protocol` to pin a choice.
    pub fn record_function_calling_support(&mut self, url: &str, supported: bool) {
        self.function_calling_support
            .insert(url.to_string(), supported);
    }

    /// Tool protocol for the model this session is currently talking to.
    pub fn active_tool_protocol(&self) -> crate::config::ToolProtocol {
        self.tool_protocol_for(&self.api_base_url)
    }

    pub fn clear_selection(&mut self) {
        self.sel_start = None;
        self.sel_end = None;
        self.selecting = false;
        self.sel_in_input = false;
    }

    pub fn has_active_notice(&self) -> bool {
        self.notice
            .as_ref()
            .is_some_and(|n| n.shown_at.elapsed() < crate::ui::NOTICE_TTL)
    }

    /// Show a transient notice toast in the top-right corner.
    pub fn set_notice(&mut self, text: impl Into<String>) {
        let text_str = text.into();
        let is_warning = ["warning", "error", "failed", "blocked", "abort", "loop"]
            .iter()
            .any(|word| text_str.to_ascii_lowercase().contains(word));
        let kind = if is_warning {
            NoticeKind::Warning
        } else {
            NoticeKind::Notice
        };
        self.notice = Some(Notice {
            text: text_str,
            kind,
            shown_at: std::time::Instant::now(),
        });
        self.redraw_requested = true;
    }

    pub fn set_warning_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: NoticeKind::Warning,
            shown_at: std::time::Instant::now(),
        });
        self.redraw_requested = true;
    }

    pub fn toggle_thought(&mut self, idx: usize) {
        if !self.expanded_thoughts.remove(&idx) {
            self.expanded_thoughts.insert(idx);
        }
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::{AppState, NoticeKind};
    use crate::config::ToolProtocol;

    #[test]
    fn test_set_warning_notice() {
        let mut state = AppState::new();
        state.set_warning_notice("Custom warning");
        assert_eq!(state.notice.as_ref().unwrap().kind, NoticeKind::Warning);

        state.set_notice("Execution error occurred");
        assert_eq!(state.notice.as_ref().unwrap().kind, NoticeKind::Warning);
    }

    #[test]
    fn known_providers_get_structured_calls_and_local_servers_keep_text() {
        let mut s = AppState::new();
        s.config.tool_protocol = ToolProtocol::Json;

        assert_eq!(
            s.tool_protocol_for(
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
            ),
            ToolProtocol::ApiNative
        );
        assert_eq!(
            s.tool_protocol_for("https://api.openai.com/v1/chat/completions"),
            ToolProtocol::ApiNative
        );
        // A local server may not implement function calling, so it keeps the
        // configured text protocol.
        assert_eq!(
            s.tool_protocol_for("http://localhost:11434/v1/chat/completions"),
            ToolProtocol::Json
        );
    }

    #[test]
    fn probe_result_decides_for_gateways_and_is_remembered() {
        let mut s = AppState::new();
        s.config.tool_protocol = ToolProtocol::Json;
        let gateway = "http://localhost:3000/v1/chat/completions";

        // A gateway says nothing by its hostname, so it must be probed.
        assert!(s.function_calling_unknown(gateway));
        assert_eq!(s.tool_protocol_for(gateway), ToolProtocol::Json);

        s.record_function_calling_support(gateway, true);
        assert!(!s.function_calling_unknown(gateway));
        assert_eq!(s.tool_protocol_for(gateway), ToolProtocol::ApiNative);
    }

    #[test]
    fn known_hosts_skip_the_probe() {
        let s = AppState::new();
        assert!(!s.function_calling_unknown("https://api.openai.com/v1/chat/completions"));
    }

    #[test]
    fn a_profile_override_beats_detection() {
        let mut s = AppState::new();
        s.config.models.push(crate::config::ModelProfile {
            name: "local-caller".to_string(),
            url: "http://localhost:1234/v1/chat/completions".to_string(),
            model: "qwen".to_string(),
            context_window: None,
            engine: None,
            api_key: None,
            env_key: None,
            tool_protocol: Some(ToolProtocol::ApiNative),
            enable_thinking: None,
            max_tokens: None,
        });

        assert_eq!(
            s.tool_protocol_for("http://localhost:1234/v1/chat/completions"),
            ToolProtocol::ApiNative
        );
    }
}

#[cfg(test)]
mod input_history_tests {
    use super::AppState;

    #[test]
    fn recall_uses_typed_inputs_not_chat_history() {
        let mut s = AppState::new();
        // Slash commands never become chat messages; generated user blobs
        // (e.g. /goal's "Goal: ..." text) aren't typed input.
        s.input_history = vec![
            "fix the parser".to_string(),
            "/verbosity toggle".to_string(),
        ];
        s.history.push(crate::app::state::ChatMessage::new(
            "user",
            "Goal: generated blob that should not be recalled",
        ));

        s.history_up();
        assert_eq!(s.input_buffer, "/verbosity toggle");
        s.history_up();
        assert_eq!(s.input_buffer, "fix the parser");
        s.history_down();
        assert_eq!(s.input_buffer, "/verbosity toggle");
        s.history_down();
        assert_eq!(s.input_buffer, "");
        assert!(s.history_index.is_none());
    }
}

#[cfg(test)]
mod queue_pull_back_tests {
    use super::AppState;

    #[test]
    fn pop_queued_prompt_pulls_latest_user_prompt_skipping_wakeups() {
        let mut s = AppState::new();
        s.pending_queue = vec![
            "first prompt".to_string(),
            "second prompt".to_string(),
            "__task_wakeup__:abc123".to_string(),
        ];

        assert!(s.pop_queued_prompt());
        assert_eq!(s.input_buffer, "second prompt");
        assert_eq!(s.cursor_position, "second prompt".len());
        // The wakeup entry and the older prompt stay queued.
        assert_eq!(s.pending_queue.len(), 2);

        assert!(s.pop_queued_prompt());
        assert_eq!(s.input_buffer, "first prompt");
        // Only the wakeup entry remains — nothing more to pull.
        assert!(!s.pop_queued_prompt());
        assert_eq!(s.pending_queue, vec!["__task_wakeup__:abc123"]);
    }
}

#[cfg(test)]
mod chat_message_tests {
    use super::{ChatMessage, ToolCallRef};
    use crate::config::ToolProtocol;

    #[test]
    fn resolved_tool_calls_prefers_structured_tool_calls() {
        let msg = ChatMessage::new("assistant", "<think>some reasoning</think>")
            .with_tool_calls(vec![ToolCallRef {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path": "src/main.rs"}"#.to_string(),
            }]);

        let calls = msg.resolved_tool_calls(ToolProtocol::ApiNative);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "src/main.rs");
    }

    #[test]
    fn resolved_tool_calls_falls_back_to_parsing_content_text() {
        let msg = ChatMessage::new(
            "assistant",
            "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```",
        );

        let calls = msg.resolved_tool_calls(ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "grep");
    }
}

#[cfg(test)]
mod hover_tests {
    use super::{AppState, HoverTarget};
    use ratatui::layout::Rect;

    #[test]
    fn hover_target_prefers_the_pill_then_clickable_rows() {
        let mut s = AppState::new();
        s.scroll_to_bottom_btn = Some(Rect::new(60, 20, 20, 1));
        s.thought_toggle_rows = vec![(5, 3)];
        s.code_copy_rows = vec![(9, "code".to_string())];

        assert_eq!(s.hover_target_at(65, 20), HoverTarget::ScrollPill);
        assert_eq!(s.hover_target_at(0, 5), HoverTarget::ThoughtHeader(5));
        assert_eq!(s.hover_target_at(40, 9), HoverTarget::CopyBadge(9));
        // Nothing clickable on this row, and outside the pill rect.
        assert_eq!(s.hover_target_at(40, 7), HoverTarget::None);
    }
}
