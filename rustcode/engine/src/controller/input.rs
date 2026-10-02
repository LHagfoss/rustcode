//! Composer assistance and activity display for frontends (issue #1442).
//!
//! Completion, file-mention, help-text, panel search and
//! activity-classification helpers the composer, panel and status render
//! paths call per frame. Thin contract over
//! the engine implementations: identical output by construction. The input
//! *driving* itself (key handling mutating `AppState`) lives in the
//! frontend event loop by design — see the composer move in this epic.

use crate::app::{AppStatus, LiveToolCall};

pub use crate::app::activity::{ActivityKind, ActivitySnapshot};
pub use crate::app::fuzzy::{fuzzy_match_positions, fuzzy_matches};
pub use crate::app::suggestion::CommandInfo;

/// Classify session activity for the status line from the turn status and
/// the currently running tools.
pub fn classify_activity(status: &AppStatus, running_tools: &[String]) -> ActivitySnapshot {
    crate::app::activity::classify_activity(status, running_tools)
}

/// Classify live tool-call activity, if any call determines the display.
pub fn classify_live_tools(calls: &[LiveToolCall]) -> Option<ActivitySnapshot> {
    crate::app::activity::classify_live_tools(calls)
}

/// Short action/target pair describing a tool call for live cells.
pub fn summarize_tool_call(name: &str, args: &serde_json::Value) -> (String, String) {
    crate::app::activity::summarize_tool_call(name, args)
}

/// Braille spinner glyph for a point on the shared animation timeline.
pub fn spinner_frame(elapsed: std::time::Duration) -> char {
    crate::app::activity::spinner_frame(elapsed)
}

/// Spinner frame index for a point on the shared animation timeline.
pub fn spinner_frame_index(elapsed: std::time::Duration) -> u64 {
    crate::app::activity::spinner_frame_index(elapsed)
}

/// Slash-command token from the first input line, if any.
pub fn command_token(input: &str) -> Option<&str> {
    crate::app::suggestion::command_token(input)
}

/// Slash commands matching the current input token, exact matches first.
pub fn filtered_commands(input: &str) -> Vec<&'static CommandInfo> {
    crate::app::suggestion::filtered_commands(input)
}

/// Length of the completion suffix available at the cursor, if any.
pub fn get_completion_len(input_buffer: &str, cursor_position: usize) -> usize {
    crate::app::get_completion_len(input_buffer, cursor_position)
}

/// `@`-mentionable project paths matching `query` (at most 25).
pub fn list_project_file_paths(query: &str) -> Vec<String> {
    crate::app::list_project_file_paths(query)
}

/// Static help text for the `?` shortcut and the help card.
pub fn build_help_text() -> String {
    crate::app::actions::build_help_text()
}

/// The key that copies a transcript selection on the platform this session is
/// running on.
///
/// This is always `ctrl+c`. RustCode enables raw mode, which disables the
/// line discipline's ISIG handling, so `ctrl+c` arrives as a key event the
/// selection handler owns (`selection_owns_key` in the TUI runtime) instead
/// of raising SIGINT. Apple Terminal reserves `cmd+c` for its own native
/// selection, which knows nothing about RustCode's semantic or keyboard
/// selection range, so advertising `cmd+c` would promise a copy the app never
/// receives. `ctrl+c` is the chord delivered to the handler on macOS and
/// everywhere else.
///
/// This is the one piece of selection policy the engine owns: the chord is the
/// same in every frontend, and a hint that hardcodes it would be wrong on one
/// platform or the other.
pub fn copy_selection_binding() -> &'static str {
    "ctrl+c"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistance_helpers_match_the_engine_sources() {
        assert_eq!(
            command_token("/model foo"),
            crate::app::suggestion::command_token("/model foo")
        );
        assert_eq!(
            filtered_commands("/mod")
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>(),
            crate::app::suggestion::filtered_commands("/mod")
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            get_completion_len("/he", 3),
            crate::app::get_completion_len("/he", 3)
        );
        assert_eq!(build_help_text(), crate::app::actions::build_help_text());
        let args = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(
            summarize_tool_call("view_file", &args),
            crate::app::activity::summarize_tool_call("view_file", &args)
        );
    }

    #[test]
    fn activity_classification_unifies_with_the_engine_source() {
        let snapshot = classify_activity(&AppStatus::Streaming, &[]);
        assert_eq!(
            snapshot,
            crate::app::activity::classify_activity(&AppStatus::Streaming, &[])
        );
        assert!(classify_live_tools(&[]).is_none());
    }

    /// The advertised copy chord must name the key the platform actually
    /// delivers, so the footer is not a promise the terminal breaks (#1566).
    /// Raw mode delivers `ctrl+c` on every platform including macOS; `cmd+c`
    /// is the terminal's native selection and never reaches the handler.
    #[test]
    fn copy_binding_is_ctrl_c_on_every_platform() {
        assert_eq!(copy_selection_binding(), "ctrl+c");
    }
}
