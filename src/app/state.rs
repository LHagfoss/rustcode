use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone)]
pub enum AppStatus {
    Idle,
    Streaming,
    Queued,

    AwaitingToolConfirmation,
}

#[derive(Debug, Clone)]
pub struct ToolConfirmation {
    pub tool_name: String,
    pub path: String,
    pub content_preview: String,
    pub content_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
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
    "Type /help to see all commands and keybindings",
];

#[derive(Clone, Serialize, Deserialize)]
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
        }
    }

    pub fn with_diff(mut self, diff: Option<String>) -> Self {
        self.diff = diff;
        self
    }
}

/// A subagent spawned by the main agent via the spawn_agent tool. Keeps its
/// own conversation history so the main agent can follow up with send_agent.
pub struct SubAgent {
    pub id: u32,
    pub task: String,
    pub model: Option<String>,
    pub history: Vec<ChatMessage>,
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

pub struct AppState {
    pub input_buffer: String,
    pub history: Vec<ChatMessage>,
    pub current_response: String,
    pub current_token_usage: Option<TokenUsage>,
    pub pending_queue: Vec<String>,
    pub status: AppStatus,
    pub cursor_position: usize,

    pub suggestion_cycle: crate::app::suggestion::SuggestionCycle,
    pub response_time: Option<std::time::Duration>,
    pub history_index: Option<usize>,
    pub temp_input: String,

    pub api_base_url: String,
    pub model_name: String,
    pub config: crate::config::AppConfig,

    pub cwd_and_branch: String,

    pub active_suggestion_index: Option<usize>,

    pub show_model_picker: bool,
    pub model_picker_index: usize,
    pub model_picker_search: String,

    pub show_command_picker: bool,
    pub command_picker_index: usize,
    pub command_picker_search: String,

    pub show_history_picker: bool,
    pub history_picker_index: usize,
    pub history_picker_sessions: Vec<crate::config::SessionMeta>,
    pub active_session_id: String,

    pub show_mcp_config: bool,
    pub mcp_picker_index: usize,
    pub mcp_edit_state: Option<McpEditState>,

    pub pending_tool_confirmation: Option<ToolConfirmation>,

    pub tool_confirmation_response: Option<tokio::sync::oneshot::Sender<bool>>,

    /// The names of user-approved tools currently running in the background.
    /// While this is not empty, the modal overlay stays closed and the user can
    /// keep working normally.
    pub running_tools: Vec<String>,

    pub stream_tracker: Option<StreamTracker>,

    pub auto_confirm: bool,

    pub subagents: Vec<SubAgent>,
    pub next_subagent_id: u32,

    pub scroll_row: u16,
    pub is_scroll_locked_to_bottom: bool,
    pub last_max_scroll: u16,
    pub viewport_height: u16,
    pub mouse_capture_enabled: bool,
    pub sel_start: Option<(u16, u16)>,
    pub sel_end: Option<(u16, u16)>,
    pub selecting: bool,
    pub expanded_thoughts: std::collections::HashSet<usize>,
    pub thought_toggle_rows: Vec<(u16, usize)>,
    pub scrollbar_col: u16,
    pub scrollbar_top: u16,
    pub scrollbar_height: u16,
    pub dragging_scrollbar: bool,
    pub raw_cli_mode: bool,
    pub tip_index: usize,

    pub current_terminal_title: Option<String>,
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
    pub fn new() -> Self {
        let (api_base_url, model_name, mut config) = crate::config::load_config();
        let active_session_id = crate::config::init_active_session(&mut config);
        let history = crate::config::load_session_history_direct(&active_session_id);
        let cwd_and_branch = get_cwd_and_branch();

        Self {
            input_buffer: String::new(),
            history,
            current_response: String::new(),
            current_token_usage: None,
            pending_queue: Vec::new(),
            status: AppStatus::Idle,
            cursor_position: 0,
            suggestion_cycle: crate::app::suggestion::SuggestionCycle::new(),
            response_time: None,
            history_index: None,
            temp_input: String::new(),
            api_base_url,
            model_name,
            config,
            cwd_and_branch,
            active_suggestion_index: None,
            show_model_picker: false,
            model_picker_index: 0,
            model_picker_search: String::new(),
            show_command_picker: false,
            command_picker_index: 0,
            command_picker_search: String::new(),
            show_history_picker: false,
            history_picker_index: 0,
            history_picker_sessions: Vec::new(),
            show_mcp_config: false,
            mcp_picker_index: 0,
            mcp_edit_state: None,
            pending_tool_confirmation: None,
            tool_confirmation_response: None,
            running_tools: Vec::new(),
            stream_tracker: None,
            auto_confirm: false,
            active_session_id,
            subagents: Vec::new(),
            next_subagent_id: 1,
            scroll_row: 0,
            is_scroll_locked_to_bottom: true,
            current_terminal_title: None,
            last_max_scroll: 0,
            viewport_height: 0,
            mouse_capture_enabled: true,
            sel_start: None,
            sel_end: None,
            selecting: false,
            expanded_thoughts: std::collections::HashSet::new(),
            thought_toggle_rows: Vec::new(),
            scrollbar_col: 0,
            scrollbar_top: 0,
            scrollbar_height: 0,
            dragging_scrollbar: false,
            raw_cli_mode: false,
            tip_index: random_tip_index(),
        }
    }

    /// True when any modal overlay is open (pickers or tool confirmation);
    /// the background content renders dimmed.
    pub fn modal_open(&self) -> bool {
        self.show_model_picker
            || self.show_command_picker
            || self.show_history_picker
            || self.show_mcp_config
            || self.status == AppStatus::AwaitingToolConfirmation
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
            .find(|m| m.name == self.config.default)
            .and_then(|p| p.context_window)
            .unwrap_or(crate::config::DEFAULT_CONTEXT_WINDOW)
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
            if let Some(idx) = self.suggestion_cycle.suggestion_index {
                if idx < matches.len() {
                    self.input_buffer = matches[idx].to_string();
                    self.cursor_position = self.input_buffer.len();
                }
            }
        }
    }

    pub fn reset_suggestion_cycle(&mut self) {
        self.suggestion_cycle.reset();
    }

    pub fn history_up(&mut self) {
        let user_msgs: Vec<String> = self
            .history
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
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
        let user_msgs: Vec<String> = self
            .history
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
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

    pub fn clear_selection(&mut self) {
        self.sel_start = None;
        self.sel_end = None;
        self.selecting = false;
    }

    pub fn toggle_thought(&mut self, idx: usize) {
        if !self.expanded_thoughts.remove(&idx) {
            self.expanded_thoughts.insert(idx);
        }
    }

    /// Map a click on the scrollbar track to a scroll position and jump there.
    pub fn scrollbar_drag_to(&mut self, row: u16) {
        self.clear_selection();
        let max = self.last_max_scroll;
        if max == 0 || self.scrollbar_height <= 1 {
            return;
        }
        let rel =
            row.saturating_sub(self.scrollbar_top) as f32 / (self.scrollbar_height - 1) as f32;
        let target = (rel.clamp(0.0, 1.0) * max as f32).round() as u16;
        self.scroll_row = target.min(max);
        self.is_scroll_locked_to_bottom = self.scroll_row >= max;
    }
}
