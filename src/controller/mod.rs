//! UI-neutral contract for frontends that control and observe a RustCode session.

mod events;
mod snapshot;

pub use events::{
    ApprovalChoice, ControllerError, ControllerEvent, ControllerUpdate, TurnUpdate,
    accepts_generation,
};
pub use snapshot::{
    ApprovalPrompt, Command, ControllerHandle, ControllerSnapshot, ModelChoice, QuestionPrompt,
    SessionChoice, TranscriptItem,
};

#[cfg(test)]
mod tests {
    use super::{ControllerSnapshot, accepts_generation};
    use crate::app::{AppState, AppStatus, ChatMessage, PendingQuestion};

    #[test]
    fn snapshot_projects_session_transcript_runtime_state_without_terminal_fields() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let mut state = AppState::new_with_workspace_session(workspace.path(), Some("session-7"));
        state.workspace_root = Some(workspace.path().to_path_buf());
        state.active_session_id = "session-7".to_owned();
        state.model_name = "model-7".to_owned();
        state.history.push(ChatMessage::new("user", "first"));
        state.history.push(ChatMessage::new("assistant", "second"));
        state.current_response = std::sync::Arc::new("live".to_owned());
        state.pending_queue = vec!["queued one".to_owned(), "queued two".to_owned()];
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(PendingQuestion::new(
            "Pick one".to_owned(),
            vec!["A".to_owned(), "B".to_owned()],
            false,
        ));

        let snapshot = ControllerSnapshot::from_state(7, &state);

        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.session_id.as_deref(), Some("session-7"));
        assert_eq!(snapshot.workspace.as_deref(), Some(workspace.path()));
        assert_eq!(snapshot.selected_model.as_deref(), Some("model-7"));
        assert_eq!(
            snapshot
                .transcript
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(snapshot.queued_count, 2);
        assert!(snapshot.turn_active);
        assert_eq!(snapshot.live_response, "live");
        let question = snapshot.pending_question.expect("question projection");
        assert_eq!(question.text, "Pick one");
        assert_eq!(question.options, ["A", "B"]);
        assert!(!question.multiple);
    }

    #[test]
    fn generation_filter_rejects_events_from_an_older_session() {
        let event = super::ControllerEvent {
            generation: 6,
            update: super::ControllerUpdate::Turn(super::TurnUpdate::Cancelled),
        };
        assert!(!accepts_generation(7, &event));
        assert!(accepts_generation(6, &event));
    }
}
