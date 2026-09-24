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
    },
    ToolFinished {
        id: String,
        content: String,
    },
    /// The approval prompt batch is available in owned form to the frontend.
    ApprovalRequested(Vec<ApprovalPrompt>),
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
        AgentUiEvent::ToolStarted { id, name } => {
            ControllerUpdate::Turn(TurnUpdate::ToolStarted { id, name })
        }
        AgentUiEvent::ToolFinished { id, result } => {
            ControllerUpdate::Turn(TurnUpdate::ToolFinished {
                id,
                content: result.content,
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
                    .map(|call| ApprovalPrompt {
                        tool_name: call.name,
                        description: call.arguments.to_string(),
                    })
                    .collect(),
            ))
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
                    tool_name: "write_file".to_owned(),
                    description: r#"{"path":"src/main.rs"}"#.to_owned(),
                },
            ]))
        );
    }

    #[test]
    fn internal_tool_events_become_owned_frontend_records() {
        let event = crate::network::ui_adapter::AgentUiEvent::ToolStarted {
            id: "call-9".to_owned(),
            name: "read_file".to_owned(),
        };
        let public = from_agent_ui_event(12, event).expect("event should be projected");
        assert_eq!(public.generation, 12);
        assert_eq!(
            public.update,
            ControllerUpdate::Turn(TurnUpdate::ToolStarted {
                id: "call-9".to_owned(),
                name: "read_file".to_owned(),
            })
        );
    }
}
