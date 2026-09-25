use super::{ApprovalPrompt, ControllerSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalChoice {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    NoActiveSession,
    InvalidWorkspace(String),
    Session(String),
    Model(String),
    Provider(String),
    ChannelClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnUpdate {
    PromptStarted(String),
    TextDelta(String),
    ToolStarted {
        id: String,
        name: String,
        detail: Option<String>,
    },
    ToolFinished {
        id: String,
        content: String,
        success: bool,
        pending: bool,
    },
    /// The approval prompt batch is available in owned form to the frontend.
    ApprovalRequested(ApprovalPrompt),
    QuestionRequested(crate::controller::QuestionPrompt),
    TurnFinished,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerUpdate {
    Snapshot(ControllerSnapshot),
    Turn(TurnUpdate),
    Error(ControllerError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerEvent {
    pub generation: u64,
    pub update: ControllerUpdate,
}

pub fn accepts_generation(current: u64, event: &ControllerEvent) -> bool {
    event.generation == current
}

/// Convert an internal streaming event into owned records safe for any frontend.
#[allow(dead_code)]
pub(crate) fn from_agent_ui_event(
    generation: u64,
    event: crate::network::ui_adapter::AgentUiEvent,
) -> Option<ControllerEvent> {
    use crate::network::ui_adapter::AgentUiEvent;

    let update = match event {
        AgentUiEvent::PromptStarted { prompt } => {
            ControllerUpdate::Turn(TurnUpdate::PromptStarted(prompt))
        }
        AgentUiEvent::TextDelta { text } => ControllerUpdate::Turn(TurnUpdate::TextDelta(text)),
        AgentUiEvent::ToolStarted { id, name, detail } => {
            ControllerUpdate::Turn(TurnUpdate::ToolStarted { id, name, detail })
        }
        AgentUiEvent::ToolFinished { id, result } => {
            ControllerUpdate::Turn(TurnUpdate::ToolFinished {
                id,
                content: result.content,
                success: result.metadata.success,
                pending: result.metadata.pending,
            })
        }
        AgentUiEvent::TurnFinished { .. } => ControllerUpdate::Turn(TurnUpdate::TurnFinished),
        AgentUiEvent::Cancelled { .. } => ControllerUpdate::Turn(TurnUpdate::Cancelled),
        #[cfg(test)]
        AgentUiEvent::Error { message, .. } => {
            ControllerUpdate::Error(ControllerError::Provider(message))
        }
        #[cfg(test)]
        AgentUiEvent::TurnRecovered { message } => {
            ControllerUpdate::Error(ControllerError::Provider(message))
        }
        AgentUiEvent::ApprovalRequested { actions } => {
            let actions = actions
                .into_iter()
                .map(|action| action.with_generation(generation))
                .collect();
            ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(ApprovalPrompt::new(actions)))
        }
        AgentUiEvent::QuestionRequested { prompt } => {
            ControllerUpdate::Turn(TurnUpdate::QuestionRequested(prompt))
        }
        AgentUiEvent::SubagentUpdated { .. } => return None,
    };
    Some(ControllerEvent { generation, update })
}

#[cfg(test)]
mod tests {
    use super::{ControllerUpdate, TurnUpdate, from_agent_ui_event};

    #[test]
    fn approval_requests_are_observable_and_tagged_with_the_generation() {
        let event = crate::network::ui_adapter::AgentUiEvent::ApprovalRequested {
            actions: vec![super::super::ApprovalAction::new(
                "call-13".to_owned(),
                "write_file".to_owned(),
                "write_file · src/main.rs".to_owned(),
                "This action requires your approval before it can continue.".to_owned(),
                r#"{"path":"src/main.rs"}"#.to_owned(),
            )],
        };
        let public = from_agent_ui_event(13, event).expect("approval request should be projected");

        assert_eq!(public.generation, 13);
        assert_eq!(
            public.update,
            ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(
                super::super::ApprovalPrompt::new(vec![super::super::ApprovalAction::new(
                    "13:call-13".to_owned(),
                    "write_file".to_owned(),
                    "write_file · src/main.rs".to_owned(),
                    "This action requires your approval before it can continue.".to_owned(),
                    r#"{"path":"src/main.rs"}"#.to_owned(),
                )]),
            ))
        );
        let ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(approval)) = public.update else {
            unreachable!();
        };
        assert_eq!(approval.request_id, "batch:1:10:13:call-13");
        let action = &approval.actions[0];
        assert_eq!(action.request_id, "13:call-13");
        assert_eq!(action.action_summary, "write_file · src/main.rs");
        assert!(action.risk_context.contains("requires your approval"));
        assert_eq!(action.description, r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn streamed_approval_batch_keeps_every_action_and_bounds_argument_preview() {
        let large_value = "x".repeat(2_000);
        let public = from_agent_ui_event(
            7,
            crate::network::ui_adapter::AgentUiEvent::ApprovalRequested {
                actions: vec![
                    super::super::ApprovalAction::new(
                        "call-a".to_owned(),
                        "write_file".to_owned(),
                        "write_file · src/a.txt".to_owned(),
                        "This action requires your approval before it can continue.".to_owned(),
                        r#"{"path":"src/a.txt","content":"first"}"#.to_owned(),
                    ),
                    super::super::ApprovalAction::new(
                        "call-b".to_owned(),
                        "run_command".to_owned(),
                        "run_command · cargo test".to_owned(),
                        "This action requires your approval before it can continue.".to_owned(),
                        serde_json::json!({ "command": "cargo test", "payload": large_value })
                            .to_string(),
                    ),
                ],
            },
        )
        .expect("approval request should be projected");

        let ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(approval)) = public.update else {
            unreachable!();
        };
        assert_eq!(approval.actions.len(), 2);
        assert_eq!(approval.actions[0].action_summary, "write_file · src/a.txt");
        assert_eq!(
            approval.actions[1].action_summary,
            "run_command · cargo test"
        );
        assert!(approval.actions[1].description.chars().count() <= 340);
        assert!(approval.actions[1].description.ends_with("… [truncated]"));
    }

    #[test]
    fn question_requests_are_observable_and_tagged_with_the_generation() {
        let public = from_agent_ui_event(
            23,
            crate::network::ui_adapter::AgentUiEvent::QuestionRequested {
                prompt: crate::controller::QuestionPrompt {
                    header: "Question".to_owned(),
                    text: "Choose".to_owned(),
                    options: vec!["One".to_owned(), "Two".to_owned()],
                    descriptions: vec![],
                    multiple: true,
                },
            },
        )
        .expect("question prompt should be projected");

        assert_eq!(public.generation, 23);
        assert_eq!(
            public.update,
            ControllerUpdate::Turn(TurnUpdate::QuestionRequested(
                crate::controller::QuestionPrompt {
                    header: "Question".to_owned(),
                    text: "Choose".to_owned(),
                    options: vec!["One".to_owned(), "Two".to_owned()],
                    descriptions: vec![],
                    multiple: true,
                }
            ))
        );
    }

    #[test]
    fn internal_tool_events_become_owned_frontend_records() {
        let event = crate::network::ui_adapter::AgentUiEvent::ToolStarted {
            id: "call-9".to_owned(),
            name: "read_file".to_owned(),
            detail: Some("src/main.rs".to_owned()),
        };
        let public = from_agent_ui_event(12, event).expect("event should be projected");
        assert_eq!(public.generation, 12);
        assert_eq!(
            public.update,
            ControllerUpdate::Turn(TurnUpdate::ToolStarted {
                id: "call-9".to_owned(),
                name: "read_file".to_owned(),
                detail: Some("src/main.rs".to_owned()),
            })
        );
    }
}
