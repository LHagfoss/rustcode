use super::ControllerSnapshot;

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
    ToolStarted { id: String, name: String },
    ToolFinished { id: String, content: String },
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
        AgentUiEvent::ApprovalRequested { .. } | AgentUiEvent::SubagentUpdated { .. } => {
            return None;
        }
    };
    Some(ControllerEvent { generation, update })
}

#[cfg(test)]
mod tests {
    use super::{ControllerUpdate, TurnUpdate, from_agent_ui_event};

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
