//! Slash-command definitions and suggestion cycling for the input buffer.
//! When the user types `/`, Tab cycles through matching commands.

/// A slash command: name plus short description for the autocomplete menu.
pub struct CommandInfo {
    pub name: &'static str,
    pub desc: &'static str,
}

/// Single source of truth for every implemented slash command.
/// Powers Tab-cycling, the autocomplete popup, and /help output.
pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/cancel",
        desc: "Cancel active stream or queued prompt",
    },
    CommandInfo {
        name: "/clear",
        desc: "Clear conversation history",
    },
    CommandInfo {
        name: "/copy",
        desc: "Copy last assistant reply to clipboard",
    },
    CommandInfo {
        name: "/exit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/help",
        desc: "Show help info",
    },
    CommandInfo {
        name: "/history",
        desc: "Resume previous chat history",
    },
    CommandInfo {
        name: "/model",
        desc: "Switch model profile or override model",
    },
    CommandInfo {
        name: "/models",
        desc: "Open the model picker",
    },
    CommandInfo {
        name: "/new",
        desc: "Start a new conversation",
    },
    CommandInfo {
        name: "/ollama",
        desc: "Configure or list Ollama models",
    },
    CommandInfo {
        name: "/provider",
        desc: "Add/update model provider profile",
    },
    CommandInfo {
        name: "/quit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/resume",
        desc: "Resume previous chat history",
    },
];

fn matching_command_names(prefix: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .map(|c| c.name)
        .filter(|name| name.starts_with(prefix))
        .collect()
}

#[derive(Debug, Default)]
pub struct SuggestionCycle {
    /// Original prefix the user typed before cycling started.
    pub original_prefix: Option<String>,
    /// Current index into the filtered match list (None = no active cycle).
    pub suggestion_index: Option<usize>,
}

impl SuggestionCycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cycle the match list forward, updating internal state. Returns true if advanced.
    pub fn cycle(&mut self, input_buffer: &str) -> bool {
        if !input_buffer.starts_with('/') || input_buffer.is_empty() {
            return false;
        }

        let prefix = if let Some(ref p) = self.original_prefix {
            p.clone()
        } else {
            let p = input_buffer.to_string();
            self.original_prefix = Some(p.clone());
            p
        };

        let matches = matching_command_names(&prefix);
        if matches.is_empty() {
            return false;
        }

        let next_idx = match self.suggestion_index {
            Some(idx) => (idx + 1) % matches.len(),
            None => 0,
        };

        self.suggestion_index = Some(next_idx);
        true
    }

    /// Returns the suffix to render as a completion hint (text after `input_buffer`).
    pub fn get_completion_suffix(&self, input_buffer: &str) -> Option<String> {
        if !input_buffer.starts_with('/') || input_buffer.is_empty() {
            return None;
        }

        let prefix = self.original_prefix.as_deref().unwrap_or(input_buffer);
        let matches = matching_command_names(prefix);
        if matches.is_empty() {
            return None;
        }

        let idx = self.suggestion_index?;
        Some(
            matches[idx]
                .strip_prefix(input_buffer)
                .unwrap_or("")
                .to_string(),
        )
    }

    /// Reset the cycle state (called on any keypress other than Tab).
    pub fn reset(&mut self) {
        self.original_prefix = None;
        self.suggestion_index = None;
    }
}
