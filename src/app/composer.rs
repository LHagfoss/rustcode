use crate::app::state::AppState;
use crate::app::suggestion::SuggestionCycle;

pub(crate) struct ComposerState<'a> {
    input_buffer: &'a mut String,
    cursor_position: &'a mut usize,
    input_history: &'a mut Vec<String>,
    pending_queue: &'a mut Vec<String>,
    history_index: &'a mut Option<usize>,
    temp_input: &'a mut String,
    suggestion_cycle: &'a mut SuggestionCycle,
}

impl<'a> ComposerState<'a> {
    pub(crate) fn new(state: &'a mut AppState) -> Self {
        Self {
            input_buffer: &mut state.input_buffer,
            cursor_position: &mut state.cursor_position,
            input_history: &mut state.input_history,
            pending_queue: &mut state.pending_queue,
            history_index: &mut state.history_index,
            temp_input: &mut state.temp_input,
            suggestion_cycle: &mut state.suggestion_cycle,
        }
    }

    pub(crate) fn input(&self) -> &str {
        self.input_buffer
    }

    pub(crate) fn replace_input(&mut self, input: impl Into<String>) {
        *self.input_buffer = input.into();
        *self.cursor_position = self.input_buffer.len();
        *self.history_index = None;
    }

    pub(crate) fn reset_suggestion_cycle(&mut self) {
        self.suggestion_cycle.reset();
    }

    pub(crate) fn pop_queued_prompt(&mut self) -> bool {
        let Some(pos) = self
            .pending_queue
            .iter()
            .rposition(|item| !item.starts_with("__task_wakeup__:"))
        else {
            return false;
        };
        *self.input_buffer = self.pending_queue.remove(pos);
        *self.cursor_position = self.input_buffer.len();
        true
    }

    pub(crate) fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }

        let next_idx = match *self.history_index {
            None => {
                *self.temp_input = self.input_buffer.clone();
                self.input_history.len() - 1
            }
            Some(idx) => idx.saturating_sub(1),
        };

        *self.history_index = Some(next_idx);
        *self.input_buffer = self.input_history[next_idx].clone();
        *self.cursor_position = self.input_buffer.len();
    }

    pub(crate) fn history_down(&mut self) {
        if self.input_history.is_empty() {
            return;
        }

        if let Some(idx) = *self.history_index {
            if idx + 1 < self.input_history.len() {
                *self.history_index = Some(idx + 1);
                *self.input_buffer = self.input_history[idx + 1].clone();
                *self.cursor_position = self.input_buffer.len();
            } else {
                *self.history_index = None;
                *self.input_buffer = self.temp_input.clone();
                *self.cursor_position = self.input_buffer.len();
            }
        }
    }
}

impl AppState {
    pub(crate) fn composer(&mut self) -> ComposerState<'_> {
        ComposerState::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::ComposerState;
    use crate::app::AppState;

    #[test]
    fn composer_view_preserves_history_and_queue_editing() {
        let mut state = AppState::new();
        state.input_history = vec!["first".to_string(), "second".to_string()];
        state.input_buffer = "draft".to_string();
        state.pending_queue = vec![
            "__task_wakeup__:done".to_string(),
            "queued prompt".to_string(),
        ];

        {
            let mut composer = ComposerState::new(&mut state);
            composer.history_up();
            assert_eq!(composer.input(), "second");
            composer.pop_queued_prompt();
            assert_eq!(composer.input(), "queued prompt");
        }

        assert_eq!(state.input_buffer, "queued prompt");
        assert_eq!(state.cursor_position, "queued prompt".len());
        assert_eq!(state.pending_queue, vec!["__task_wakeup__:done"]);
    }

    #[test]
    fn replacing_input_resets_cursor_and_history_navigation() {
        let mut state = AppState::new();
        state.input_history = vec!["old".to_string()];
        state.history_index = Some(0);

        {
            let mut composer = ComposerState::new(&mut state);
            composer.replace_input("new draft");
        }

        assert_eq!(state.input_buffer, "new draft");
        assert_eq!(state.cursor_position, "new draft".len());
        assert!(state.history_index.is_none());
    }
}
