use super::AppError;
use crate::runtime::events::AppEvent;
use rustcode::app::AppState;
use tokio_util::sync::CancellationToken;

pub(super) fn apply_session_event(
    state: &mut AppState,
    cancel_token: &mut CancellationToken,
    event: AppEvent,
) -> Result<(), AppError> {
    let controller = rustcode::app::session_controller::SessionController::default();
    let archive_only = matches!(&event, AppEvent::ArchiveSession);
    if !archive_only {
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }

    let transition = match event {
        AppEvent::NewSession => controller.start_fresh(state),
        AppEvent::ResumeSession(action) => controller.resume(state, action),
        AppEvent::ForkSession(action) => controller.fork(state, action),
        AppEvent::ClearSession => controller.clear(state),
        AppEvent::ArchiveSession => controller.archive(state),
        AppEvent::DeleteSession(action) => controller.delete(state, action),
        _ => return Err(AppError("not a session event".to_owned())),
    }
    .map_err(|error| AppError(error.to_string()))?;

    if !archive_only {
        state.show_history_picker = false;
        state.pending_delete_session_idx = None;
        state.history_picker_sessions.clear();
    }
    state.set_notice(format_session_transition(&transition));
    state.request_redraw();
    Ok(())
}

fn format_session_transition(
    transition: &rustcode::app::session_controller::SessionTransition,
) -> String {
    use rustcode::app::session_controller::SessionTransition;
    match transition {
        SessionTransition::Started { .. } => "Started a new session".to_owned(),
        SessionTransition::Resumed { .. } => "Resumed session".to_owned(),
        SessionTransition::Forked { .. } => "Forked session".to_owned(),
        SessionTransition::Cleared { .. } => "Cleared transcript view".to_owned(),
        SessionTransition::Archived { .. } => "Archived session".to_owned(),
        SessionTransition::Deleted { .. } => "Deleted session".to_owned(),
    }
}

pub(super) fn open_overlay(state: &mut AppState, overlay: rustcode::app::events::Overlay) {
    if matches!(overlay, rustcode::app::events::Overlay::History) {
        let (sessions, truncated) =
            rustcode::app::actions::build_session_list_with_truncation(state);
        state.history_picker_sessions = sessions;
        state.history_picker_index = 0;
        state.history_picker_truncated = truncated;
    }
    if matches!(overlay, rustcode::app::events::Overlay::Subagents) {
        state.subagent_picker_index = active_context_row(state);
    }
    state.overlays().open(overlay);
}

/// Picker row of the context on screen: row 0 is main, then the subagents in
/// spawn order.
fn active_context_row(state: &AppState) -> usize {
    state
        .selected_subagent_id
        .and_then(|id| state.subagents.iter().position(|agent| agent.id == id))
        .map_or(0, |index| index + 1)
}

/// The context one step before or after the one on screen, wrapping through
/// main (id 0) in spawn order. `None` when there is no subagent to switch to.
pub(super) fn adjacent_context_id(state: &AppState, forward: bool) -> Option<u32> {
    if state.subagents.is_empty() {
        return None;
    }
    let total = state.subagents.len() + 1;
    let row = active_context_row(state);
    let next = if forward {
        (row + 1) % total
    } else {
        (row + total - 1) % total
    };
    Some(if next == 0 {
        0
    } else {
        state.subagents[next - 1].id
    })
}

pub(super) fn apply_subagent_selection(state: &mut AppState, id: u32) -> Result<(), AppError> {
    if id == 0 {
        rustcode::app::SubagentController.select_root(state);
    } else {
        rustcode::app::SubagentController
            .select(state, rustcode::app::SubagentId::from_raw(id))
            .map_err(|error| AppError(error.to_string()))?;
    }
    state.show_subagent_picker = false;
    state.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{adjacent_context_id, open_overlay};
    use rustcode::app::AppState;

    fn spawn(state: &mut AppState, name: &str) -> u32 {
        rustcode::app::SubagentController
            .spawn(state, name, None, None, false, Vec::new(), None, None)
            .raw()
    }

    #[test]
    fn adjacent_context_wraps_through_main_in_spawn_order() {
        let mut state = AppState::new();
        assert_eq!(adjacent_context_id(&state, true), None);

        let first = spawn(&mut state, "first");
        let second = spawn(&mut state, "second");
        assert_eq!(adjacent_context_id(&state, true), Some(first));
        assert_eq!(adjacent_context_id(&state, false), Some(second));

        state.selected_subagent_id = Some(second);
        assert_eq!(adjacent_context_id(&state, true), Some(0));
        assert_eq!(adjacent_context_id(&state, false), Some(first));
    }

    #[test]
    fn agents_overlay_opens_on_the_context_on_screen() {
        let mut state = AppState::new();
        spawn(&mut state, "first");
        let second = spawn(&mut state, "second");
        state.selected_subagent_id = Some(second);

        open_overlay(&mut state, rustcode::app::events::Overlay::Subagents);

        assert!(state.show_subagent_picker);
        assert_eq!(state.subagent_picker_index, 2);
    }
}
