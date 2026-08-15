use crate::app::{AppState, ChatMessage, SubAgent, SubAgentStatus};
use std::fmt;
use std::path::PathBuf;

/// Stable identity for a subagent inside one RustCode session.
///
/// The network tool protocol continues to expose the existing numeric id. The
/// newtype keeps that wire detail out of controller code and makes accidental
/// mixing with unrelated integers harder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct SubagentId(u32);

impl SubagentId {
    pub(crate) fn from_raw(id: u32) -> Self {
        Self(id)
    }

    pub(crate) fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubagentContext {
    pub(crate) id: SubagentId,
    pub(crate) name: String,
    pub(crate) status: SubAgentStatus,
    pub(crate) history: Vec<ChatMessage>,
    pub(crate) active_turn: bool,
    pub(crate) parent_id: Option<SubagentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubagentError {
    MissingId(SubagentId),
    CannotSendToTerminal(SubagentId),
}

impl fmt::Display for SubagentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingId(id) => write!(f, "no subagent with id {}", id.raw()),
            Self::CannotSendToTerminal(id) => {
                write!(f, "subagent {} is not available for follow-up", id.raw())
            }
        }
    }
}

impl std::error::Error for SubagentError {}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SubagentController;

impl SubagentController {
    pub(crate) fn spawn(
        &self,
        state: &mut AppState,
        task: impl Into<String>,
        model: Option<String>,
        parent_id: Option<SubagentId>,
        write_access: bool,
        allowed_paths: Vec<String>,
        verification_command: Option<String>,
        workspace_root: Option<PathBuf>,
    ) -> SubagentId {
        let id = SubagentId::from_raw(state.next_subagent_id);
        state.next_subagent_id = state.next_subagent_id.saturating_add(1);
        let task = task.into();
        state.subagents.push(SubAgent {
            id: id.raw(),
            name: format!("agent-{}", id.raw()),
            task: task.clone(),
            model,
            history: vec![ChatMessage::new("user", &task)],
            status: SubAgentStatus::Running,
            active_turn: true,
            parent_id: parent_id.map(SubagentId::raw),
            write_access,
            allowed_paths,
            verification_command,
            workspace_root,
            review_manifest: None,
        });
        state.request_redraw();
        id
    }

    pub(crate) fn send_input(
        &self,
        state: &mut AppState,
        id: SubagentId,
        message: impl Into<String>,
    ) -> Result<(), SubagentError> {
        let Some(agent) = state
            .subagents
            .iter_mut()
            .find(|agent| agent.id == id.raw())
        else {
            return Err(SubagentError::MissingId(id));
        };
        if matches!(
            agent.status,
            SubAgentStatus::Failed | SubAgentStatus::Cancelled
        ) {
            return Err(SubagentError::CannotSendToTerminal(id));
        }
        agent.status = SubAgentStatus::Running;
        agent.active_turn = true;
        agent.history.push(ChatMessage::new("user", message.into()));
        state.request_redraw();
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn interrupt(
        &self,
        state: &mut AppState,
        id: SubagentId,
    ) -> Result<(), SubagentError> {
        let Some(agent) = state
            .subagents
            .iter_mut()
            .find(|agent| agent.id == id.raw())
        else {
            return Err(SubagentError::MissingId(id));
        };
        agent.status = SubAgentStatus::Cancelled;
        agent.active_turn = false;
        state.request_redraw();
        Ok(())
    }

    pub(crate) fn set_status(
        &self,
        state: &mut AppState,
        id: SubagentId,
        status: SubAgentStatus,
    ) -> Result<(), SubagentError> {
        let Some(agent) = state
            .subagents
            .iter_mut()
            .find(|agent| agent.id == id.raw())
        else {
            return Err(SubagentError::MissingId(id));
        };
        agent.status = status;
        agent.active_turn = matches!(status, SubAgentStatus::Running);
        state.request_redraw();
        Ok(())
    }

    pub(crate) fn select(&self, state: &mut AppState, id: SubagentId) -> Result<(), SubagentError> {
        if !state.subagents.iter().any(|agent| agent.id == id.raw()) {
            return Err(SubagentError::MissingId(id));
        }
        state.selected_subagent_id = Some(id.raw());
        state.request_redraw();
        Ok(())
    }

    pub(crate) fn select_root(&self, state: &mut AppState) {
        state.selected_subagent_id = None;
        state.request_redraw();
    }

    pub(crate) fn list(&self, state: &AppState) -> Vec<SubagentContext> {
        state
            .subagents
            .iter()
            .map(|agent| SubagentContext {
                id: SubagentId::from_raw(agent.id),
                name: agent.name.clone(),
                status: agent.status,
                history: agent.history.clone(),
                active_turn: agent.active_turn,
                parent_id: agent.parent_id.map(SubagentId::from_raw),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{SubagentController, SubagentError, SubagentId};
    use crate::app::{AppState, ChatMessage, SubAgentStatus};

    #[test]
    fn spawn_registers_context_with_parent_and_running_turn() {
        let mut state = AppState::new();
        let controller = SubagentController;
        let parent = controller.spawn(
            &mut state,
            "inspect the parent",
            Some("high".to_owned()),
            None,
            false,
            Vec::new(),
            None,
            None,
        );
        let child = controller.spawn(
            &mut state,
            "inspect the child",
            None,
            Some(parent),
            false,
            Vec::new(),
            None,
            None,
        );

        let contexts = controller.list(&state);
        assert_eq!(contexts.len(), 2);
        assert_eq!(contexts[1].id, child);
        assert_eq!(contexts[1].parent_id, Some(parent));
        assert_eq!(contexts[1].status, SubAgentStatus::Running);
        assert!(contexts[1].active_turn);
        assert_eq!(
            contexts[1].history[0],
            ChatMessage::new("user", "inspect the child")
        );
    }

    #[test]
    fn send_input_and_status_transitions_preserve_history_and_lifecycle() {
        let mut state = AppState::new();
        let controller = SubagentController;
        let id = controller.spawn(
            &mut state,
            "run checks",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );

        controller
            .set_status(&mut state, id, SubAgentStatus::Completed)
            .unwrap();
        assert!(!state.subagents[0].active_turn);
        controller.send_input(&mut state, id, "follow up").unwrap();
        assert_eq!(state.subagents[0].status, SubAgentStatus::Running);
        assert!(state.subagents[0].active_turn);
        assert_eq!(
            state.subagents[0].history.last().unwrap().content,
            "follow up"
        );

        controller
            .set_status(&mut state, id, SubAgentStatus::Cancelled)
            .unwrap();
        assert!(controller.send_input(&mut state, id, "too late").is_err());
    }

    #[test]
    fn selection_preserves_parent_and_rejects_unknown_ids() {
        let mut state = AppState::new();
        let controller = SubagentController;
        let id = controller.spawn(
            &mut state,
            "find the issue",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );
        state
            .history
            .push(ChatMessage::new("user", "parent context"));

        controller.select(&mut state, id).unwrap();
        assert_eq!(state.selected_subagent_id, Some(id.raw()));
        assert_eq!(state.history[0].content, "parent context");
        controller.select_root(&mut state);
        assert_eq!(state.selected_subagent_id, None);
        assert_eq!(
            controller.select(&mut state, SubagentId::from_raw(99)),
            Err(SubagentError::MissingId(SubagentId::from_raw(99)))
        );
    }
}
