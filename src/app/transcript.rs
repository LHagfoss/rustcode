use crate::app::state::{AppState, History, TokenUsage};
use ratatui::layout::Rect;
use std::sync::Arc;

#[allow(dead_code)]
pub(crate) struct TranscriptState<'a> {
    history: &'a mut History,
    history_display_start: &'a mut usize,
    current_response: &'a mut Arc<String>,
    current_token_usage: &'a mut Option<TokenUsage>,
    scroll_row: &'a mut u16,
    is_scroll_locked_to_bottom: &'a mut bool,
    last_max_scroll: &'a mut u16,
    conversation_content_height: &'a mut u16,
    viewport_height: &'a mut u16,
    chat_area: &'a mut Option<Rect>,
    redraw_requested: &'a mut bool,
    render_revision: &'a mut u64,
}

impl<'a> TranscriptState<'a> {
    pub(crate) fn new(state: &'a mut AppState) -> Self {
        Self {
            history: &mut state.history,
            history_display_start: &mut state.history_display_start,
            current_response: &mut state.current_response,
            current_token_usage: &mut state.current_token_usage,
            scroll_row: &mut state.scroll_row,
            is_scroll_locked_to_bottom: &mut state.is_scroll_locked_to_bottom,
            last_max_scroll: &mut state.last_max_scroll,
            conversation_content_height: &mut state.conversation_content_height,
            viewport_height: &mut state.viewport_height,
            chat_area: &mut state.chat_area,
            redraw_requested: &mut state.redraw_requested,
            render_revision: &mut state.render_revision,
        }
    }

    pub(crate) fn live_response(&self) -> &str {
        self.current_response.as_ref()
    }

    pub(crate) fn history_len(&self) -> usize {
        self.history.len()
    }

    pub(crate) fn request_replay(&mut self) {
        *self.history_display_start = 0;
        *self.redraw_requested = true;
        *self.render_revision = self.render_revision.wrapping_add(1);
    }

    #[allow(dead_code)]
    pub(crate) fn clear_live_response(&mut self) {
        Arc::make_mut(self.current_response).clear();
        *self.current_token_usage = None;
        *self.redraw_requested = true;
        *self.render_revision = self.render_revision.wrapping_add(1);
    }

    #[allow(dead_code)]
    pub(crate) fn scroll_position(&self) -> (u16, u16, bool) {
        (
            *self.scroll_row,
            *self.last_max_scroll,
            *self.is_scroll_locked_to_bottom,
        )
    }
}

impl AppState {
    pub(crate) fn transcript(&mut self) -> TranscriptState<'_> {
        TranscriptState::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::TranscriptState;
    use crate::app::AppState;

    #[test]
    fn transcript_view_tracks_live_response_and_replay_boundary() {
        let mut state = AppState::new();
        state.replace_current_response("streaming");

        {
            let mut transcript = TranscriptState::new(&mut state);
            assert_eq!(transcript.live_response(), "streaming");
            transcript.request_replay();
        }

        assert_eq!(state.history_display_start, 0);
        assert!(state.redraw_requested);
    }
}
