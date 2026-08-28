use super::AppError;
use crate::app::{AppEvent, AppState};
use tokio_util::sync::CancellationToken;

pub(super) fn apply_session_event(
    state: &mut AppState,
    cancel_token: &mut CancellationToken,
    event: AppEvent,
) -> Result<(), AppError> {
    let controller = crate::app::session_controller::SessionController::default();
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
    transition: &crate::app::session_controller::SessionTransition,
) -> String {
    use crate::app::session_controller::SessionTransition;
    match transition {
        SessionTransition::Started { .. } => "Started a new session".to_owned(),
        SessionTransition::Resumed { .. } => "Resumed session".to_owned(),
        SessionTransition::Forked { .. } => "Forked session".to_owned(),
        SessionTransition::Cleared { .. } => "Cleared transcript view".to_owned(),
        SessionTransition::Archived { .. } => "Archived session".to_owned(),
        SessionTransition::Deleted { .. } => "Deleted session".to_owned(),
    }
}

pub(super) fn open_overlay(state: &mut AppState, overlay: crate::app::events::Overlay) {
    if matches!(overlay, crate::app::events::Overlay::History) {
        let (sessions, truncated) = crate::app::actions::build_session_list_with_truncation(state);
        state.history_picker_sessions = sessions;
        state.history_picker_index = 0;
        state.history_picker_truncated = truncated;
    }
    if matches!(overlay, crate::app::events::Overlay::Subagents) {
        state.subagent_picker_index = 0;
    }
    state.overlays().open(overlay);
}

pub(super) fn apply_subagent_selection(state: &mut AppState, id: u32) -> Result<(), AppError> {
    if id == 0 {
        crate::app::SubagentController.select_root(state);
    } else {
        crate::app::SubagentController
            .select(state, crate::app::SubagentId::from_raw(id))
            .map_err(|error| AppError(error.to_string()))?;
    }
    state.show_subagent_picker = false;
    state.request_redraw();
    Ok(())
}
