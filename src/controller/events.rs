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
    ApprovalRequested(Vec<ApprovalPrompt>),
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
        AgentUiEvent::ApprovalRequested { calls } => {
            ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(
                calls
                    .into_iter()
                    .map(|call| {
                        let action_summary = call
                            .arguments
                            .get("path")
                            .or_else(|| call.arguments.get("command"))
                            .or_else(|| call.arguments.get("url"))
                            .and_then(serde_json::Value::as_str)
                            .map(|target| format!("{} · {target}", call.name))
                            .unwrap_or_else(|| call.name.clone());
                        ApprovalPrompt {
                            request_id: format!(
                                "{generation}:{}",
                                call.call_id.unwrap_or_else(|| {
                                    format!(
                                        "local:{}",
                                        crate::network::tool_exec::stable_arguments_hash(
                                            &call.arguments
                                        )
                                    )
                                })
                            ),
                            tool_name: call.name,
                            action_summary,
                            risk_context:
                                "This action requires your approval before it can continue."
                                    .to_owned(),
                            description: call.arguments.to_string(),
                        }
                    })
                    .collect(),
            ))
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
            calls: vec![crate::tools::ToolCall {
                name: "write_file".to_owned(),
                arguments: serde_json::json!({ "path": "src/main.rs" }),
                call_id: Some("call-13".to_owned()),
            }],
        };
        let public = from_agent_ui_event(13, event).expect("approval request should be projected");

        assert_eq!(public.generation, 13);
        assert_eq!(
            public.update,
            ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(vec![
                super::super::ApprovalPrompt {
                    request_id: "13:call-13".to_owned(),
                    tool_name: "write_file".to_owned(),
                    action_summary: "write_file · src/main.rs".to_owned(),
                    risk_context: "This action requires your approval before it can continue."
                        .to_owned(),
                    description: r#"{"path":"src/main.rs"}"#.to_owned(),
                },
            ]))
        );
        let ControllerUpdate::Turn(TurnUpdate::ApprovalRequested(approvals)) = public.update else {
            unreachable!();
        };
        let approval = &approvals[0];
        assert_eq!(approval.request_id, "13:call-13");
        assert_eq!(approval.action_summary, "write_file · src/main.rs");
        assert!(approval.risk_context.contains("requires your approval"));
        assert_eq!(approval.description, r#"{"path":"src/main.rs"}"#);
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
