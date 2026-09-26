use super::{ApprovalBatchPrompt, ApprovalPrompt, ControllerSnapshot};

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
    /// Legacy presentation-only approval prompts. Decisions made from these
    /// prompts cannot be authorized without a batch identity.
    ApprovalRequested(Vec<ApprovalPrompt>),
    /// Exact controller-owned approval batch for batch-aware frontends.
    ApprovalBatchRequested(ApprovalBatchPrompt),
    QuestionRequested(crate::controller::QuestionPrompt),
    TurnFinished,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerUpdate {
    Snapshot(ControllerSnapshot),
    PromptRestored(String),
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
) -> Vec<ControllerEvent> {
    use crate::network::ui_adapter::AgentUiEvent;

    let updates = match event {
        AgentUiEvent::PromptStarted { prompt } => {
            vec![ControllerUpdate::Turn(TurnUpdate::PromptStarted(prompt))]
        }
        AgentUiEvent::TextDelta { text } => {
            vec![ControllerUpdate::Turn(TurnUpdate::TextDelta(text))]
        }
        AgentUiEvent::ToolStarted { id, name, detail } => {
            vec![ControllerUpdate::Turn(TurnUpdate::ToolStarted {
                id,
                name,
                detail,
            })]
        }
        AgentUiEvent::ToolFinished { id, result } => {
            vec![ControllerUpdate::Turn(TurnUpdate::ToolFinished {
                id,
                content: result.content,
                success: result.metadata.success,
                pending: result.metadata.pending,
            })]
        }
        AgentUiEvent::TurnFinished { .. } => vec![ControllerUpdate::Turn(TurnUpdate::TurnFinished)],
        AgentUiEvent::Cancelled { .. } => vec![ControllerUpdate::Turn(TurnUpdate::Cancelled)],
        #[cfg(test)]
        AgentUiEvent::Error { message, .. } => {
            vec![ControllerUpdate::Error(ControllerError::Provider(message))]
        }
        #[cfg(test)]
        AgentUiEvent::TurnRecovered { message } => {
            vec![ControllerUpdate::Error(ControllerError::Provider(message))]
        }
        AgentUiEvent::ApprovalRequested { batch_id, actions } => {
            let actions = actions
                .into_iter()
                .map(|action| action.with_generation(generation))
                .collect();
            let batch = ApprovalBatchPrompt::new(actions).with_batch_id(batch_id);
            let legacy = batch
                .actions
                .iter()
                .map(|action| ApprovalPrompt {
                    tool_name: action.tool_name.clone(),
                    description: action.description.clone(),
                })
                .collect();
            vec![
                ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(legacy)),
                ControllerUpdate::Turn(TurnUpdate::ApprovalBatchRequested(batch)),
            ]
        }
        AgentUiEvent::QuestionRequested { prompt } => {
            vec![ControllerUpdate::Turn(TurnUpdate::QuestionRequested(
                prompt,
            ))]
        }
        AgentUiEvent::SubagentUpdated { .. } => return Vec::new(),
    };
    updates
        .into_iter()
        .map(|update| ControllerEvent { generation, update })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ControllerUpdate, TurnUpdate, from_agent_ui_event};

    #[test]
    fn approval_requests_are_observable_and_tagged_with_the_generation() {
        let event = crate::network::ui_adapter::AgentUiEvent::ApprovalRequested {
            batch_id: "controller:13:1".to_owned(),
            actions: vec![super::super::ApprovalAction::new(
                "call-13".to_owned(),
                "write_file".to_owned(),
                "write_file · src/main.rs".to_owned(),
                "This action requires your approval before it can continue.".to_owned(),
                r#"{"path":"src/main.rs"}"#.to_owned(),
            )],
        };
        let public = from_agent_ui_event(13, event);
        assert_eq!(public.len(), 2);
        assert!(public.iter().all(|event| event.generation == 13));
        assert!(matches!(
            &public[0].update,
            ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(prompts))
                if prompts == &[super::super::ApprovalPrompt {
                    tool_name: "write_file".to_owned(),
                    description: r#"{"path":"src/main.rs"}"#.to_owned(),
                }]
        ));
        let ControllerUpdate::Turn(TurnUpdate::ApprovalBatchRequested(approval)) =
            public[1].update.clone()
        else {
            unreachable!();
        };
        assert_eq!(approval.request_id, "batch:1:10:13:call-13");
        assert_eq!(approval.batch_id, "controller:13:1");
        let action = &approval.actions[0];
        assert_eq!(action.request_id, "13:call-13");
        assert_eq!(action.action_summary, "write_file · src/main.rs");
        assert!(action.risk_context.contains("requires your approval"));
        assert_eq!(action.description, r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn legacy_approval_prompt_construction_and_match_remain_available() {
        let prompt = super::super::ApprovalPrompt {
            tool_name: "write_file".to_owned(),
            description: "src/main.rs".to_owned(),
        };
        let update = super::ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(vec![prompt]));
        let super::ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(prompts)) = update else {
            unreachable!();
        };
        let [
            super::super::ApprovalPrompt {
                tool_name,
                description,
            },
        ] = prompts.as_slice()
        else {
            unreachable!();
        };
        assert_eq!(tool_name, "write_file");
        assert_eq!(description, "src/main.rs");
    }

    #[test]
    fn streamed_approval_batch_keeps_every_action_and_bounds_argument_preview() {
        let large_value = "x".repeat(2_000);
        let public = from_agent_ui_event(
            7,
            crate::network::ui_adapter::AgentUiEvent::ApprovalRequested {
                batch_id: "controller:7:1".to_owned(),
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
        );

        let ControllerUpdate::Turn(TurnUpdate::ApprovalBatchRequested(approval)) =
            public[1].update.clone()
        else {
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
        );
        let public = public
            .into_iter()
            .next()
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
        let public = from_agent_ui_event(12, event)
            .into_iter()
            .next()
            .expect("event should be projected");
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
