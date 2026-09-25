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

pub fn move_selection(current: usize, count: usize, down: bool) -> usize {
    if count == 0 {
        return 0;
    }
    if down {
        (current + 1) % count
    } else {
        current.checked_sub(1).unwrap_or(count - 1) % count
    }
}

pub fn complete_selection(input: &str, selected: usize) -> Option<String> {
    suggestions(input)
        .get(selected)
        .map(|suggestion| complete(suggestion.name))
}

#[cfg(test)]
mod tests {
    use super::{complete, complete_selection, move_selection, suggestions};

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

    #[test]
    fn selection_wraps_and_completion_uses_the_selected_filtered_command() {
        assert_eq!(move_selection(0, 3, false), 2);
        assert_eq!(move_selection(2, 3, true), 0);
        assert_eq!(move_selection(0, 0, true), 0);
        assert_eq!(complete_selection("/c", 0), Some("/clear".to_owned()));
        assert_eq!(complete_selection("/c", 1), Some("/cancel".to_owned()));
        assert_eq!(complete_selection("/m", 0), Some("/model ".to_owned()));
        assert_eq!(complete_selection("/m", 1), None);
    }
}
