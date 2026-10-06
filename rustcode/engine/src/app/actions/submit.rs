use crate::app::{AppState, state::DraftSubmitMode};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SubmitOutcome {
    Empty,
    Steered,
    Queued,
}

pub(crate) fn submit_plain_prompt_with_mode(
    state: &mut AppState,
    text: String,
    mode: DraftSubmitMode,
) -> SubmitOutcome {
    state.draft_submit_mode = mode;
    submit_plain_prompt(state, text)
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

    begin_task_delegation(state);
    state.pending_queue.push(text);
    state.input_buffer.clear();
    state.cursor_position = 0;
    state.draft_submit_mode = DraftSubmitMode::Steer;
    state.request_redraw();
    SubmitOutcome::Queued
}

/// Decide whether the task may use subagents, consuming the one-shot
/// `/delegate` arming. Delegation is available by default for the session, but
/// the user config setting is a hard gate and `/delegate off` remains in force
/// until an explicit `/delegate` or `/delegate on` command.
pub(crate) fn begin_task_delegation(state: &mut AppState) {
    state.delegation_active =
        state.config.delegation_enabled && (state.delegation_armed || state.delegation_sticky);
    state.delegation_armed = false;
}
