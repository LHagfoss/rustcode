use crate::app::{AppState, state::DraftSubmitMode};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubmitOutcome {
    Empty,
    Steered,
    Queued,
}

pub(crate) fn submit_plain_prompt(state: &mut AppState, text: String) -> SubmitOutcome {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return SubmitOutcome::Empty;
    }

    if state.can_accept_steer()
        && state.draft_submit_mode == DraftSubmitMode::Steer
        && state.queue_steer(text.clone())
    {
        state.input_buffer.clear();
        state.cursor_position = 0;
        state.draft_submit_mode = DraftSubmitMode::Steer;
        state.request_redraw();
        return SubmitOutcome::Steered;
    }

    state.delegation_active = state.delegation_armed;
    state.delegation_armed = false;
    state.pending_queue.push(text);
    state.input_buffer.clear();
    state.cursor_position = 0;
    state.draft_submit_mode = DraftSubmitMode::Steer;
    state.request_redraw();
    SubmitOutcome::Queued
}
