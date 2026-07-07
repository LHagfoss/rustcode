use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone)]
pub enum AppStatus {
    Idle,
    Streaming,
    Queued,
}

/// Token-count snapshot for the status bar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

fn current_timestamp() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// A message in conversation history.
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
}

impl ChatMessage {
    /// Create a message with no token-usage data.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            token_usage: None,
            timestamp: current_timestamp(),
            response_time_ms: None,
        }
    }
}

/// The single source of truth for the application's state.
pub struct AppState {
    pub input_buffer: String,
    pub history: Vec<ChatMessage>,
    pub current_response: String,
    pub current_token_usage: Option<TokenUsage>,
    pub pending_queue: Vec<String>,
    pub status: AppStatus,
    pub cursor_position: usize,
    /// Command-suggestion cycling state (Tab/Enter keys).
    pub suggestion_cycle: crate::app::suggestion::SuggestionCycle,
    pub response_time: Option<std::time::Duration>,
    pub history_index: Option<usize>,
    pub temp_input: String,

    // Dynamic config settings
    pub api_base_url: String,
    pub model_name: String,
    pub config: crate::config::AppConfig,

    // Welcome screen details
    pub cwd_and_branch: String,

    // Autocomplete menu index
    pub active_suggestion_index: Option<usize>,

    // Model Picker Modal state
    pub show_model_picker: bool,
    pub model_picker_index: usize,
    pub model_picker_search: String,

    // Command Picker Modal state
    pub show_command_picker: bool,
    pub command_picker_index: usize,
    pub command_picker_search: String,

    // Scroll state
    pub scroll_row: u16,
    pub is_scroll_locked_to_bottom: bool,
    pub last_max_scroll: u16,
}

fn get_cwd_and_branch() -> String {
    let absolute_path = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/Users/lagos/code/fm_harness".to_string());

    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/lagos".to_string());
    let path_with_tildes = if absolute_path.starts_with(&home) {
        absolute_path.replacen(&home, "~", 1)
    } else {
        absolute_path
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
        let (api_base_url, model_name, config) = crate::config::load_config();
        let cwd_and_branch = get_cwd_and_branch();

        Self {
            input_buffer: String::new(),
            history: Vec::new(),
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
            scroll_row: 0,
            is_scroll_locked_to_bottom: true,
            last_max_scroll: 0,
        }
    }

    // ── Input editing ────────────────────────────────────────────────

    pub fn insert_char(&mut self, c: char) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        self.input_buffer.insert(self.cursor_position, c);
        self.cursor_position += 1;
        self.reset_suggestion_index();
    }

    pub fn delete_char_backspace(&mut self) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position > 0 {
            self.input_buffer.remove(self.cursor_position - 1);
            self.cursor_position -= 1;
        }
        self.reset_suggestion_index();
    }

    pub fn delete_char_delete(&mut self) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position < self.input_buffer.len() {
            self.input_buffer.remove(self.cursor_position);
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
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_position < self.input_buffer.len() {
            self.cursor_position += 1;
        }
    }

    pub fn move_cursor_word_left(&mut self) {
        let chars: Vec<char> = self.input_buffer.chars().collect();
        let mut pos = self.cursor_position.min(chars.len());
        if pos == 0 {
            return;
        }
        // Skip trailing whitespace.
        while pos > 0 && chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        // Skip the current word leftwards.
        while pos > 0 && !chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        self.cursor_position = pos;
    }

    pub fn move_cursor_word_right(&mut self) {
        let chars: Vec<char> = self.input_buffer.chars().collect();
        let len = chars.len();
        let mut pos = self.cursor_position.min(len);
        if pos == len {
            return;
        }
        // Skip current word.
        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }
        // Skip whitespace.
        while pos < len && chars[pos].is_whitespace() {
            pos += 1;
        }
        self.cursor_position = pos;
    }

    pub fn move_cursor_to_start(&mut self) {
        self.cursor_position = 0;
    }

    pub fn move_cursor_to_end(&mut self) {
        self.cursor_position = self.input_buffer.len();
    }

    // ── Suggestion helpers ───────────────────────────────────────────

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
                .copied()
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

    // ── History navigation ───────────────────────────────────────────

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

    // ── Scrolling logic ──────────────────────────────────────────────

    pub fn scroll_up(&mut self, amount: u16) {
        self.is_scroll_locked_to_bottom = false;
        self.scroll_row = self.scroll_row.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: u16) {
        let max = self.last_max_scroll;
        let next = self.scroll_row.saturating_add(amount).min(max);
        self.scroll_row = next;
        if next >= max {
            self.is_scroll_locked_to_bottom = true;
        }
    }
}
