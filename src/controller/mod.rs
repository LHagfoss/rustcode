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
    use crate::app::{AppState, AppStatus, ChatMessage, PendingQuestion, ToolConfirmation};

    #[test]
    fn snapshot_projects_session_transcript_runtime_state_without_terminal_fields() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let mut state = AppState::new_with_workspace_session(workspace.path(), Some("session-7"));
        state.workspace_root = Some(workspace.path().to_path_buf());
        let nested_workspace = workspace.path().join("nested");
        std::fs::create_dir(&nested_workspace).expect("nested workspace");
        state.task_working_directory = Some(nested_workspace.clone());
        state
            .history_picker_sessions
            .push(rustcode_session::SessionMeta {
                path: workspace.path().join("sessions/session-8.jsonl"),
                title: "Saved session".to_owned(),
                when: "today".to_owned(),
                message_count: 4,
            });
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
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            tool_name: "write_file".to_owned(),
            path: "src/main.rs".to_owned(),
            content_preview: "fn main() {}".to_owned(),
            content_bytes: 12,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }]);

        let snapshot = ControllerSnapshot::from_state(7, &state);

        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.session_id.as_deref(), Some("session-7"));
        assert_eq!(
            snapshot.workspace.as_deref(),
            Some(nested_workspace.as_path())
        );
        assert_eq!(snapshot.selected_model.as_deref(), Some("model-7"));
        assert_eq!(
            snapshot.sessions,
            [super::SessionChoice {
                id: "session-8".to_owned(),
                title: "Saved session".to_owned(),
                when: "today".to_owned(),
                message_count: 4,
            }]
        );
        assert_eq!(
            snapshot.models,
            state
                .config
                .models
                .iter()
                .map(|model| super::ModelChoice {
                    id: model.model.clone(),
                    label: model.name.clone(),
                })
                .collect::<Vec<_>>()
        );
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
        let approval = snapshot.pending_approval.expect("approval projection");
        assert_eq!(approval.tool_name, "write_file");
        assert_eq!(approval.description, "src/main.rs\nfn main() {}");
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
