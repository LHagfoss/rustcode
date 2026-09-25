//! Native slash-command suggestions for the composer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashSuggestion {
    pub name: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[SlashSuggestion] = &[
    SlashSuggestion {
        name: "/help",
        description: "Show available commands",
    },
    SlashSuggestion {
        name: "/new",
        description: "Start a new chat",
    },
    SlashSuggestion {
        name: "/clear",
        description: "Start a new chat",
    },
    SlashSuggestion {
        name: "/cancel",
        description: "Stop the current turn",
    },
    SlashSuggestion {
        name: "/model",
        description: "Show or choose a model profile",
    },
    SlashSuggestion {
        name: "/change_title",
        description: "Rename this chat",
    },
];

/// Suggestions appear while the first token is being typed. Once the user
/// starts arguments, the popup closes and the draft remains untouched.
pub fn suggestions(input: &str) -> Vec<&'static SlashSuggestion> {
    let draft = input.trim_start();
    if !draft.starts_with('/') || draft.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = draft.to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(&query))
        .collect()
}

pub fn complete(command: &str) -> String {
    if matches!(command, "/model" | "/change_title") {
        format!("{command} ")
    } else {
        command.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{complete, suggestions};

    #[test]
    fn suggestions_follow_only_the_command_token() {
        assert_eq!(
            suggestions("/mo")
                .iter()
                .map(|item| item.name)
                .collect::<Vec<_>>(),
            vec!["/model"]
        );
        assert_eq!(
            suggestions("/MODEL")
                .iter()
                .map(|item| item.name)
                .collect::<Vec<_>>(),
            vec!["/model"]
        );
        assert!(suggestions("/model deepseek").is_empty());
        assert!(suggestions("plain prompt").is_empty());
        assert_eq!(complete("/change_title"), "/change_title ");
    }
}
