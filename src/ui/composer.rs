use super::RenderSnapshot;
use super::keymap::{KeyAction, KeyMap};
use crate::app::{AppState, ChatMessage};
use crate::inline_terminal::Frame;
use crossterm::event::KeyEvent;
use ratatui::layout::{Margin, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    Handled,
    Submit,
    Paste,
    Cancel,
    ClearScreen,
    Unhandled,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Composer {
    keymap: KeyMap,
}

impl Composer {
    pub(crate) fn new() -> Self {
        Self {
            keymap: KeyMap::from_environment(),
        }
    }

    pub(crate) fn handle_key(&self, state: &mut AppState, key: KeyEvent) -> ComposerAction {
        let action = self.keymap.resolve(key);
        match action {
            KeyAction::Insert(c) => {
                if c == '?' && state.input_buffer.is_empty() {
                    state
                        .history
                        .push(ChatMessage::new("system", crate::app::build_help_text()));
                    state.request_redraw();
                } else {
                    state.insert_char(c);
                    state.reset_suggestion_cycle();
                }
                ComposerAction::Handled
            }
            KeyAction::InsertNewline => {
                state.insert_char('\n');
                state.reset_suggestion_cycle();
                ComposerAction::Handled
            }
            KeyAction::Submit => ComposerAction::Submit,
            KeyAction::Cancel => ComposerAction::Cancel,
            KeyAction::ClearScreen => ComposerAction::ClearScreen,
            KeyAction::Paste => ComposerAction::Paste,
            KeyAction::MoveLeft => {
                state.move_cursor_left();
                ComposerAction::Handled
            }
            KeyAction::MoveRight => {
                state.move_cursor_right();
                ComposerAction::Handled
            }
            KeyAction::MoveWordLeft => {
                state.move_cursor_word_left();
                ComposerAction::Handled
            }
            KeyAction::MoveWordRight => {
                state.move_cursor_word_right();
                ComposerAction::Handled
            }
            KeyAction::MoveStart => {
                state.move_cursor_to_start();
                ComposerAction::Handled
            }
            KeyAction::MoveEnd => {
                state.move_cursor_to_end();
                ComposerAction::Handled
            }
            KeyAction::DeleteBackward => {
                state.delete_char_backspace();
                ComposerAction::Handled
            }
            KeyAction::DeleteForward => {
                state.delete_char_delete();
                ComposerAction::Handled
            }
            KeyAction::DeleteWordBackward => {
                state.delete_word_backspace();
                ComposerAction::Handled
            }
            KeyAction::DeleteWordForward => {
                state.delete_word_forward();
                ComposerAction::Handled
            }
            KeyAction::KillLineStart => {
                state.kill_line_to_start();
                state.reset_suggestion_cycle();
                ComposerAction::Handled
            }
            KeyAction::HistoryPrevious => {
                self.recall_previous(state);
                ComposerAction::Handled
            }
            KeyAction::HistoryNext => {
                self.recall_next(state);
                ComposerAction::Handled
            }
            KeyAction::Complete => {
                self.complete_or_toggle_mode(state);
                ComposerAction::Handled
            }
            KeyAction::CommandPaletteOrPreviousSuggestion => {
                if !self.cycle_suggestion(state, false) {
                    state.show_command_picker = true;
                    state.command_picker_index = 0;
                    state.command_picker_search.clear();
                }
                ComposerAction::Handled
            }
            KeyAction::NextSuggestion => {
                self.cycle_suggestion(state, true);
                ComposerAction::Handled
            }
            KeyAction::ToggleAutoConfirm => {
                state.auto_confirm = !state.auto_confirm;
                ComposerAction::Handled
            }
            KeyAction::Escape | KeyAction::Unhandled => ComposerAction::Unhandled,
        }
    }

    pub(crate) fn handle_paste(&self, state: &mut AppState, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        const PASTE_THRESHOLD: usize = 300;
        let text_to_insert = if normalized.chars().count() >= PASTE_THRESHOLD {
            format!("<!--PASTE:{}:{}-->", normalized.chars().count(), normalized)
        } else {
            normalized
        };
        for c in text_to_insert.chars() {
            state.insert_char(c);
        }
        state.reset_suggestion_cycle();
    }

    #[allow(dead_code)]
    pub(crate) fn submit(&self, state: &AppState) -> Option<String> {
        let prompt = state.input_buffer.trim().to_owned();
        (!prompt.is_empty()).then_some(prompt)
    }

    pub(crate) fn recall_previous(&self, state: &mut AppState) {
        let completion_len =
            crate::app::get_completion_len(&state.input_buffer, state.cursor_position);
        if let Some(current) = state.active_suggestion_index
            && completion_len > 0
        {
            state.active_suggestion_index = Some(if current == 0 {
                completion_len - 1
            } else {
                current - 1
            });
            return;
        }
        state.active_suggestion_index = None;
        if state.input_buffer.is_empty() || state.history_index.is_some() {
            let pulled = state.composer().pop_queued_prompt();
            if !pulled {
                state.composer().history_up();
            }
        } else {
            state.move_cursor_line_up();
        }
    }

    pub(crate) fn recall_next(&self, state: &mut AppState) {
        let completion_len =
            crate::app::get_completion_len(&state.input_buffer, state.cursor_position);
        if let Some(current) = state.active_suggestion_index
            && completion_len > 0
        {
            state.active_suggestion_index = Some(if current + 1 >= completion_len {
                0
            } else {
                current + 1
            });
            return;
        }
        state.active_suggestion_index = None;
        if state.history_index.is_some() {
            state.composer().history_down();
        } else {
            state.move_cursor_line_down();
        }
    }

    pub(crate) fn render(
        &self,
        frame: &mut Frame,
        chunks: &[Rect],
        state: &RenderSnapshot,
    ) -> Margin {
        super::render_input(frame, chunks, state)
    }

    fn cycle_suggestion(&self, state: &mut AppState, next: bool) -> bool {
        let completion_len =
            crate::app::get_completion_len(&state.input_buffer, state.cursor_position);
        if state.active_suggestion_index.is_some() && completion_len > 0 {
            let current = state.active_suggestion_index.unwrap_or(0);
            state.active_suggestion_index = Some(if next {
                if current + 1 >= completion_len {
                    0
                } else {
                    current + 1
                }
            } else if current == 0 {
                completion_len - 1
            } else {
                current - 1
            });
            true
        } else {
            false
        }
    }

    fn complete_or_toggle_mode(&self, state: &mut AppState) {
        state.dismissed_completion = None;
        let has_at =
            crate::app::get_at_word_query(&state.input_buffer, state.cursor_position).is_some();
        if state.active_suggestion_index.is_some() || has_at {
            crate::app::apply_autocomplete(state);
        } else if crate::app::suggestion::command_token(&state.input_buffer).is_some() {
            state.cycle_suggestion();
        } else {
            state.agent_mode = match state.agent_mode {
                crate::config::AgentMode::Build => crate::config::AgentMode::Plan,
                crate::config::AgentMode::Plan => crate::config::AgentMode::Build,
            };
            state.config.agent_mode = state.agent_mode;
            crate::config::save_entire_config(&state.config);
            let notice = match state.agent_mode {
                crate::config::AgentMode::Build => "Switched to Build Mode (Full Code Editing)",
                crate::config::AgentMode::Plan => "Switched to Plan Mode (Read-only / Design only)",
            };
            state.set_notice(notice);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Composer, ComposerAction};
    use crate::app::AppState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn unicode_and_multiline_editing_stay_on_character_boundaries() {
        let mut state = AppState::new();
        let composer = Composer::default();

        assert_eq!(
            composer.handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('ø'), KeyModifiers::NONE)
            ),
            ComposerAction::Handled
        );
        assert_eq!(
            composer.handle_key(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
            ),
            ComposerAction::Handled
        );
        composer.handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('界'), KeyModifiers::NONE),
        );

        assert_eq!(state.input_buffer, "ø\n界");
        assert!(state.input_buffer.is_char_boundary(state.cursor_position));
    }

    #[test]
    fn paste_normalizes_newlines_and_large_payloads() {
        let mut state = AppState::new();
        Composer::default().handle_paste(&mut state, "one\r\ntwo\rthree");
        assert_eq!(state.input_buffer, "one\ntwo\nthree");

        let mut large = AppState::new();
        Composer::default().handle_paste(&mut large, &"x".repeat(300));
        assert!(large.input_buffer.starts_with("<!--PASTE:300:"));
    }

    #[test]
    fn recall_prefers_queued_prompts_then_input_history() {
        let composer = Composer::default();

        let mut queued_state = AppState::new();
        queued_state.pending_queue = vec!["queued".to_owned()];
        composer.recall_previous(&mut queued_state);
        assert_eq!(queued_state.input_buffer, "queued");

        let mut history_state = AppState::new();
        history_state.input_history = vec!["old".to_owned()];
        composer.recall_previous(&mut history_state);
        assert_eq!(history_state.input_buffer, "old");

        composer.recall_next(&mut history_state);
        assert_eq!(history_state.input_buffer, "");
    }
}
