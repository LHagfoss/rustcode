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

/// A message in conversation history.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

impl ChatMessage {
    /// Create a message with no token-usage data.
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            token_usage: None,
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
}

impl AppState {
    pub fn new() -> Self {
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
        }
    }

    // ── Input editing ────────────────────────────────────────────────

    pub fn insert_char(&mut self, c: char) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        self.input_buffer.insert(self.cursor_position, c);
        self.cursor_position += 1;
    }

    pub fn delete_char_backspace(&mut self) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position > 0 {
            self.input_buffer.remove(self.cursor_position - 1);
            self.cursor_position -= 1;
        }
    }

    pub fn delete_char_delete(&mut self) {
        self.history_index = None;
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position < self.input_buffer.len() {
            self.input_buffer.remove(self.cursor_position);
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
        self.suggestion_cycle.get_completion_suffix(&self.input_buffer)
    }

    pub fn cycle_suggestion(&mut self) {
        if self.suggestion_cycle.cycle(&self.input_buffer) {
            let prefix = self.suggestion_cycle.original_prefix.as_deref().unwrap_or(&self.input_buffer);
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
        let user_msgs: Vec<String> = self.history.iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
        if user_msgs.is_empty() { return; }

        let next_idx = match self.history_index {
            None => {
                self.temp_input = self.input_buffer.clone();
                user_msgs.len() - 1
            }
            Some(idx) => {
                if idx > 0 { idx - 1 } else { 0 }
            }
        };

        self.history_index = Some(next_idx);
        self.input_buffer = user_msgs[next_idx].clone();
        self.cursor_position = self.input_buffer.len();
    }

    pub fn history_down(&mut self) {
        let user_msgs: Vec<String> = self.history.iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
        if user_msgs.is_empty() { return; }

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
}
