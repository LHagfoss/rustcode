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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashInteraction {
    Move(usize),
    Complete { value: String, cursor_offset: usize },
    Dismiss,
    Ignore,
}

/// Decide how the slash menu responds to a key while its draft is active.
pub fn slash_interaction(
    draft: &str,
    selected: usize,
    dismissed: bool,
    key: &str,
) -> SlashInteraction {
    let suggestions = suggestions(draft);
    if dismissed || suggestions.is_empty() {
        return SlashInteraction::Ignore;
    }

    match key {
        "up" | "ArrowUp" | "arrowup" => {
            SlashInteraction::Move(move_selection(selected, suggestions.len(), false))
        }
        "down" | "ArrowDown" | "arrowdown" => {
            SlashInteraction::Move(move_selection(selected, suggestions.len(), true))
        }
        "enter" | "Enter" => suggestions
            .get(selected)
            .map(|suggestion| {
                let value = complete(suggestion.name);
                SlashInteraction::Complete {
                    cursor_offset: value.chars().count(),
                    value,
                }
            })
            .unwrap_or(SlashInteraction::Ignore),
        "escape" | "Escape" => SlashInteraction::Dismiss,
        _ => SlashInteraction::Ignore,
    }
}

/// The textarea currently applies styles to the whole value, so only style a
/// recognized command when no non-whitespace argument text has been entered.
pub fn is_recognized_command(draft: &str) -> bool {
    let token = draft.trim();
    COMMANDS
        .iter()
        .any(|command| command.name.eq_ignore_ascii_case(token))
}

#[cfg(test)]
mod tests {
    use super::{
        SlashInteraction, complete, complete_selection, is_recognized_command, move_selection,
        slash_interaction, suggestions,
    };

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

    #[test]
    fn arrow_key_names_move_selection_and_wrap() {
        assert_eq!(
            slash_interaction("/", 0, false, "ArrowUp"),
            SlashInteraction::Move(5)
        );
        assert_eq!(
            slash_interaction("/", 5, false, "up"),
            SlashInteraction::Move(4)
        );
        assert_eq!(
            slash_interaction("/", 5, false, "ArrowDown"),
            SlashInteraction::Move(0)
        );
        assert_eq!(
            slash_interaction("/", 0, false, "down"),
            SlashInteraction::Move(1)
        );
    }

    #[test]
    fn enter_completes_the_selected_command_and_returns_its_cursor_offset() {
        assert_eq!(
            slash_interaction("/m", 0, false, "Enter"),
            SlashInteraction::Complete {
                value: "/model ".to_owned(),
                cursor_offset: 7,
            }
        );
    }

    #[test]
    fn escape_dismisses_and_dismissed_or_irrelevant_keys_are_ignored() {
        assert_eq!(
            slash_interaction("/m", 0, false, "Escape"),
            SlashInteraction::Dismiss
        );
        assert_eq!(
            slash_interaction("/m", 0, true, "ArrowDown"),
            SlashInteraction::Ignore
        );
        assert_eq!(
            slash_interaction("ordinary prompt", 0, false, "Enter"),
            SlashInteraction::Ignore
        );
        assert_eq!(
            slash_interaction("/model args", 0, false, "ArrowDown"),
            SlashInteraction::Ignore
        );
    }

    #[test]
    fn recognized_command_styling_excludes_arguments_and_unknown_commands() {
        assert!(is_recognized_command("/model"));
        assert!(is_recognized_command("/MODEL"));
        assert!(is_recognized_command("/model "));
        assert!(!is_recognized_command("/model deepseek"));
        assert!(!is_recognized_command("/not-a-command"));
        assert!(!is_recognized_command("plain text"));
    }
}
