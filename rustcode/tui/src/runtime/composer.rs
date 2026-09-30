use crate::inline_terminal::Frame;
use crate::ui::keymap::{KeyAction, KeyMap};
use crate::ui::render_snapshot::RenderSnapshot;
use crossterm::event::KeyEvent;
use ratatui::layout::{Margin, Rect};
use rustcode::app::{AppState, ChatMessage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerAction {
    Handled,
    Submit,
    Paste,
    ClearScreen,
    ToggleExpand,
    Unhandled,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Composer {
    keymap: KeyMap,
}

/// Pasted text at or above this many characters is framed as a
/// `<!--PASTE:<chars>:<payload>-->` marker. The engine expands the marker at
/// the provider boundary (`rustcode_core::paste`), so a short paste sends the
/// verbatim text while a large one travels as a single cheap marker.
///
/// Single owner of the threshold and the marker format: every paste insert
/// path frames its payload through `frame_pasted_text` (#1527).
const PASTE_THRESHOLD: usize = 300;

/// Normalize a pasted payload's newlines and frame it when it is large enough
/// to be worth a marker. Both the clipboard read and a bracketed-paste event
/// hand raw text, which may carry `\r\n`; normalizing here keeps the composer
/// buffer identical whichever path delivered it.
fn frame_pasted_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.chars().count() >= PASTE_THRESHOLD {
        format!("<!--PASTE:{}:{}-->", normalized.chars().count(), normalized)
    } else {
        normalized
    }
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
                        .push(ChatMessage::new("system", rustcode::app::build_help_text()));
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
            KeyAction::ClearScreen => ComposerAction::ClearScreen,
            KeyAction::Paste => ComposerAction::Paste,
            // The transcript owns the expand state, so the composer only
            // reports the intent; the runtime applies it (#1541).
            KeyAction::ToggleExpand => ComposerAction::ToggleExpand,
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
                if state.can_accept_steer()
                    && !state.input_buffer.trim().is_empty()
                    && rustcode::app::get_completion_len(&state.input_buffer, state.cursor_position)
                        == 0
                {
                    state.draft_submit_mode = match state.draft_submit_mode {
                        rustcode::app::state::DraftSubmitMode::Steer => {
                            rustcode::app::state::DraftSubmitMode::Queue
                        }
                        rustcode::app::state::DraftSubmitMode::Queue => {
                            rustcode::app::state::DraftSubmitMode::Steer
                        }
                    };
                    state.request_redraw();
                } else {
                    self.complete(state);
                }
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
            KeyAction::ToggleAgentMode => {
                self.toggle_agent_mode(state);
                ComposerAction::Handled
            }
            KeyAction::Escape | KeyAction::Unhandled => ComposerAction::Unhandled,
        }
    }

    /// Insert a pasted payload into the composer. Every insert path — keymap
    /// paste, Ctrl/Cmd+V fallback and bracketed paste — goes through here, so
    /// the newline normalization and the large-paste marker are framed in
    /// exactly one place (#1527).
    pub(crate) fn handle_paste(&self, state: &mut AppState, text: &str) {
        for c in frame_pasted_text(text).chars() {
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
            rustcode::app::get_completion_len(&state.input_buffer, state.cursor_position);
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
            rustcode::app::get_completion_len(&state.input_buffer, state.cursor_position);
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
        crate::ui::render_input(frame, chunks, state)
    }

    fn cycle_suggestion(&self, state: &mut AppState, next: bool) -> bool {
        let completion_len =
            rustcode::app::get_completion_len(&state.input_buffer, state.cursor_position);
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

    fn complete(&self, state: &mut AppState) {
        state.dismissed_completion = None;
        let has_at =
            rustcode_core::input::get_at_word_query(&state.input_buffer, state.cursor_position)
                .is_some();
        if state.active_suggestion_index.is_some() || has_at {
            rustcode::app::apply_autocomplete(state);
        } else if rustcode::app::suggestion::command_token(&state.input_buffer).is_some() {
            state.cycle_suggestion();
        }
    }

    fn toggle_agent_mode(&self, state: &mut AppState) {
        state.agent_mode = match state.agent_mode {
            rustcode::controller::AgentMode::Build => rustcode::controller::AgentMode::Plan,
            rustcode::controller::AgentMode::Plan => rustcode::controller::AgentMode::Build,
        };
        state.config.agent_mode = state.agent_mode;
        rustcode::controller::save_config(&state.config);
        let notice = match state.agent_mode {
            rustcode::controller::AgentMode::Build => "Switched to Build Mode (Full Code Editing)",
            rustcode::controller::AgentMode::Plan => {
                "Switched to Plan Mode (Read-only / Design only)"
            }
        };
        rustcode::app::actions::push_ephemeral_status(state, notice.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{Composer, ComposerAction};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rustcode::app::AppState;

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

    /// Ctrl+O expands a collapsed tool body whether or not the composer holds
    /// a draft, so the hint the transcript advertises is never a lie (#1541).
    #[test]
    fn ctrl_o_reports_expand_and_leaves_the_draft_untouched() {
        for draft in ["", "in progress"] {
            let mut state = AppState::new();
            for character in draft.chars() {
                state.insert_char(character);
            }
            let before = state.input_buffer.clone();

            assert_eq!(
                Composer::default().handle_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)
                ),
                ComposerAction::ToggleExpand
            );
            assert_eq!(state.input_buffer, before, "draft survives ctrl+o");
        }
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

    /// The marker framed at insert time must still expand at the provider
    /// boundary, so a threshold or format change cannot strand a paste (#1527).
    #[test]
    fn framed_large_paste_round_trips_through_the_provider_boundary() {
        let payload = "å→".repeat(150);
        let mut state = AppState::new();
        Composer::default().handle_paste(&mut state, &payload);

        assert!(state.input_buffer.starts_with("<!--PASTE:"));
        assert_eq!(rustcode_core::paste::expand(&state.input_buffer), payload);
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

    #[test]
    fn shift_tab_toggles_agent_mode_without_changing_auto_confirm() {
        let composer = Composer::default();
        let mut state = AppState::new();
        state.agent_mode = rustcode::controller::AgentMode::Build;
        state.auto_confirm = false;
        state.input_buffer = "cargo test".to_owned();
        state.cursor_position = state.input_buffer.chars().count();
        state.active_suggestion_index = Some(0);

        assert_eq!(
            composer.handle_key(
                &mut state,
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            ),
            ComposerAction::Handled
        );
        assert_eq!(state.agent_mode, rustcode::controller::AgentMode::Plan);
        assert!(!state.auto_confirm);

        composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(state.agent_mode, rustcode::controller::AgentMode::Build);
        assert!(!state.auto_confirm);
    }

    #[test]
    fn plain_tab_does_not_toggle_agent_mode_without_completion() {
        let composer = Composer::default();
        let mut state = AppState::new();
        state.agent_mode = rustcode::controller::AgentMode::Plan;
        state.input_buffer = "/context".to_owned();
        state.cursor_position = state.input_buffer.chars().count();
        state.active_suggestion_index = Some(0);

        assert_eq!(
            composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),),
            ComposerAction::Handled
        );
        assert_eq!(state.agent_mode, rustcode::controller::AgentMode::Plan);
    }

    #[test]
    fn tab_toggles_draft_mode_without_an_available_completion() {
        use rustcode::app::{AppStatus, state::DraftSubmitMode};

        let composer = Composer::default();
        let mut state = AppState::new();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(state.active_session_id.clone());
        state.input_buffer = "Keep the same channel".to_owned();
        state.cursor_position = state.input_buffer.len();

        assert_eq!(
            composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            ComposerAction::Handled
        );
        assert_eq!(state.draft_submit_mode, DraftSubmitMode::Queue);
        assert_eq!(state.input_buffer, "Keep the same channel");

        composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.draft_submit_mode, DraftSubmitMode::Steer);
    }

    #[test]
    fn tab_does_not_toggle_draft_mode_for_empty_or_unsteerable_drafts() {
        use rustcode::app::{AppStatus, state::DraftSubmitMode};

        let composer = Composer::default();
        let mut empty = AppState::new();
        empty.status = AppStatus::Streaming;
        empty.active_turn_steerable_session = Some(empty.active_session_id.clone());
        composer.handle_key(&mut empty, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(empty.draft_submit_mode, DraftSubmitMode::Steer);

        let mut unsteerable = AppState::new();
        unsteerable.status = AppStatus::Streaming;
        unsteerable.input_buffer = "ordinary draft".to_owned();
        unsteerable.cursor_position = unsteerable.input_buffer.len();
        composer.handle_key(
            &mut unsteerable,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(unsteerable.draft_submit_mode, DraftSubmitMode::Steer);
    }

    #[test]
    fn tab_keeps_completion_acceptance_ahead_of_draft_mode_toggle() {
        use rustcode::app::{AppStatus, state::DraftSubmitMode};

        let composer = Composer::default();
        let mut state = AppState::new();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(state.active_session_id.clone());
        state.input_buffer = "inspect @Cargo".to_owned();
        state.cursor_position = state.input_buffer.len();
        state.active_suggestion_index = Some(0);

        composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(state.draft_submit_mode, DraftSubmitMode::Steer);
        assert!(state.input_buffer.contains("Cargo"));
        assert!(state.input_buffer.ends_with(' '));
    }

    #[test]
    fn tab_keeps_command_completion_ahead_of_draft_mode_toggle() {
        use rustcode::app::{AppStatus, state::DraftSubmitMode};

        let composer = Composer::default();
        let mut state = AppState::new();
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(state.active_session_id.clone());
        state.input_buffer = "/mo".to_owned();
        state.cursor_position = state.input_buffer.len();
        state.active_suggestion_index = Some(0);

        composer.handle_key(&mut state, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

        assert_eq!(state.draft_submit_mode, DraftSubmitMode::Steer);
        assert_eq!(state.input_buffer, "/model");
    }
}
