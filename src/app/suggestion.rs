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
        name: "/change_title",
        desc: "Rename the current session title",
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
        name: "/goal",
        desc: "Run a task in continuous autoloop mode until complete_task is called",
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
        name: "/delete_chat",
        desc: "Delete current session and start fresh",
    },
    CommandInfo {
        name: "/ollama",
        desc: "Configure or list Ollama models",
    },
    CommandInfo {
        name: "/parser",
        desc: "Show or set current tool protocol (json only)",
    },
    CommandInfo {
        name: "/provider",
        desc: "Add/update model provider profile",
    },
    CommandInfo {
        name: "/protocol",
        desc: "Show or set current tool protocol (json only)",
    },
    CommandInfo {
        name: "/quit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/quota",
        desc: "Show model quota percentages",
    },
    CommandInfo {
        name: "/resume",
        desc: "Resume most recent session",
    },
    CommandInfo {
        name: "/session",
        desc: "Show current session ID, token budget, and active model",
    },
    CommandInfo {
        name: "/skills",
        desc: "Show available skills and their locations",
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

pub fn list_project_file_paths(query: &str) -> Vec<String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut files = Vec::new();
    let query_lower = query.to_lowercase();

    let walker = ignore::WalkBuilder::new(&cwd)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(6))
        .build();

    for result in walker {
        if let Ok(entry) = result
            && entry.file_type().is_some_and(|ft| ft.is_file())
                && let Ok(rel) = entry.path().strip_prefix(&cwd) {
                    let rel_str = rel.to_string_lossy().to_string();
                    if query_lower.is_empty() || rel_str.to_lowercase().contains(&query_lower) {
                        files.push(format!("@{}", rel_str));
                        if files.len() >= 25 {
                            break;
                        }
                    }
                }
    }
    files
}

pub fn get_at_word_query(input_buffer: &str, cursor_pos: usize) -> Option<(usize, String)> {
    let pos = cursor_pos.min(input_buffer.len());
    let before = &input_buffer[..pos];
    if let Some(at_idx) = before.rfind('@') {
        let query = &before[at_idx + 1..];
        if !query.contains(' ') && !query.contains('\n') {
            return Some((at_idx, query.to_string()));
        }
    }
    None
}
