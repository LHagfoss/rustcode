pub struct CommandInfo {
    pub name: &'static str,
    pub desc: &'static str,
}

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
        name: "/context",
        desc: "Show or set active profile's context window",
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
        desc: "Pick a previous session to resume",
    },
    CommandInfo {
        name: "/memory",
        desc: "Show current process RAM usage",
    },
    CommandInfo {
        name: "/mcp",
        desc: "Configure Model Context Protocol (MCP) servers",
    },
    CommandInfo {
        name: "/model",
        desc: "Open model picker, switch profile, or override model",
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
        name: "/parser",
        desc: "Show or set current tool protocol (json, xml)",
    },
    CommandInfo {
        name: "/provider",
        desc: "Add/update model provider profile",
    },
    CommandInfo {
        name: "/protocol",
        desc: "Show or set current tool protocol (json, xml)",
    },
    CommandInfo {
        name: "/quit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/resume",
        desc: "Resume most recent session",
    },
    CommandInfo {
        name: "/stats",
        desc: "Show token usage and context statistics",
    },
    CommandInfo {
        name: "/status",
        desc: "Show token usage and context statistics",
    },
    CommandInfo {
        name: "/tools",
        desc: "List available tools",
    },
    CommandInfo {
        name: "/usage",
        desc: "Show token usage and context stats",
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
    pub original_prefix: Option<String>,
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
