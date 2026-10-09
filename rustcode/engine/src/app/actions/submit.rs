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

/// Accept a prompt that did not come from the composer (a remote client).
/// Same routing as [`submit_plain_prompt`] for the chosen `mode`, but the
/// terminal's unsent draft, cursor and saved submit mode are left untouched.
/// A steer the active turn cannot take is refused, never silently queued.
pub(crate) fn submit_detached_prompt(
    state: &mut AppState,
    text: &str,
    mode: DraftSubmitMode,
) -> SubmitOutcome {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return SubmitOutcome::Empty;
    }
    match mode {
        DraftSubmitMode::Steer => {
            if !state.queue_steer(text) {
                return SubmitOutcome::Empty;
            }
            state.request_redraw();
            SubmitOutcome::Steered
        }
        DraftSubmitMode::Queue => {
            begin_task_delegation(state);
            state.pending_queue.push(text);
            state.request_redraw();
            SubmitOutcome::Queued
        }
    }
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

/// Give a run the harness started (a background task finished while idle) the
/// session's standing delegation setting. Nothing was submitted, so nothing
/// armed delegation for it, and going idle had switched it off: the wakeup went
/// out without the agent tools the turns around it had, which also replaced
/// the cached `tools` block. A one-shot `/delegate` is not carried over.
pub(crate) fn resume_delegation_for_wakeup(state: &mut AppState) {
    if !state.delegation_active {
        state.delegation_active = state.config.delegation_enabled && state.delegation_sticky;
    }
}
