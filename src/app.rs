use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone)]
pub enum AppStatus {
    Idle,
    Streaming,
    Queued,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
}

pub struct AppState {
    pub input_buffer: String,
    pub history: Vec<ChatMessage>,
    pub current_response: String,
    pub current_token_usage: Option<TokenUsage>,
    pub pending_queue: Vec<String>,
    pub status: AppStatus,
    pub available_commands: Vec<&'static str>,
    pub original_prefix: Option<String>,
    pub suggestion_index: Option<usize>,
    pub cursor_position: usize,
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
            available_commands: vec!["/help", "/clear", "/new", "/cancel", "/exit", "/quit"],
            original_prefix: None,
            suggestion_index: None,
            cursor_position: 0,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        self.input_buffer.insert(self.cursor_position, c);
        self.cursor_position += 1;
    }

    pub fn delete_char_backspace(&mut self) {
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position > 0 {
            self.input_buffer.remove(self.cursor_position - 1);
            self.cursor_position -= 1;
        }
    }

    pub fn delete_char_delete(&mut self) {
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
        self.cursor_position = self.cursor_position.min(self.input_buffer.len());
        if self.cursor_position == 0 {
            return;
        }
        let chars: Vec<char> = self.input_buffer.chars().collect();
        let mut pos = self.cursor_position;

        // Skip spaces to the left
        while pos > 0 && chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        // Skip word characters to the left
        while pos > 0 && !chars[pos - 1].is_whitespace() {
            pos -= 1;
        }
        self.cursor_position = pos;
    }

    pub fn move_cursor_word_right(&mut self) {
        let len = self.input_buffer.len();
        self.cursor_position = self.cursor_position.min(len);
        if self.cursor_position == len {
            return;
        }
        let chars: Vec<char> = self.input_buffer.chars().collect();
        let mut pos = self.cursor_position;

        // Skip word characters to the right
        while pos < len && !chars[pos].is_whitespace() {
            pos += 1;
        }
        // Skip spaces to the right
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

    pub fn get_command_suggestion(&self) -> Option<&'static str> {
        if self.suggestion_index.is_some() {
            return None;
        }
        if self.input_buffer.starts_with('/') && !self.input_buffer.is_empty() {
            for cmd in &self.available_commands {
                if cmd.starts_with(&self.input_buffer) && *cmd != self.input_buffer {
                    return Some(cmd);
                }
            }
        }
        None
    }

    pub fn cycle_suggestion(&mut self) {
        if !self.input_buffer.starts_with('/') {
            return;
        }

        let prefix = match &self.original_prefix {
            Some(p) => p.clone(),
            None => {
                let p = self.input_buffer.clone();
                self.original_prefix = Some(p.clone());
                p
            }
        };

        let matches: Vec<&'static str> = self.available_commands
            .iter()
            .copied()
            .filter(|cmd| cmd.starts_with(&prefix))
            .collect();

        if matches.is_empty() {
            return;
        }

        let next_idx = match self.suggestion_index {
            Some(idx) => (idx + 1) % matches.len(),
            None => 0,
        };

        self.suggestion_index = Some(next_idx);
        self.input_buffer = matches[next_idx].to_string();
        self.cursor_position = self.input_buffer.len();
    }

    pub fn reset_suggestion_cycle(&mut self) {
        self.original_prefix = None;
        self.suggestion_index = None;
    }
}
