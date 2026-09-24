use std::path::PathBuf;

use crate::app::{AppState, AppStatus};

use super::{ApprovalChoice, ControllerError};

/// Commands a frontend can send to the session controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ListSessions,
    StartNew(PathBuf),
    Resume {
        session_id: String,
        workspace: PathBuf,
    },
    Submit(String),
    Cancel,
    SetAutoApprove(bool),
    SelectModel(String),
    AnswerQuestion(String),
    Approval(ApprovalChoice),
    Shutdown,
}

/// A cloneable command sender shared by frontend components.
#[derive(Clone)]
pub struct ControllerHandle {
    sender: tokio::sync::mpsc::UnboundedSender<Command>,
}

impl ControllerHandle {
    pub fn new(sender: tokio::sync::mpsc::UnboundedSender<Command>) -> Self {
        Self { sender }
    }

    pub fn send(&self, command: Command) -> Result<(), ControllerError> {
        self.sender
            .send(command)
            .map_err(|_| ControllerError::ChannelClosed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChoice {
    pub id: String,
    pub title: String,
    pub when: String,
    pub message_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptItem {
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPrompt {
    pub text: String,
    pub options: Vec<String>,
    pub multiple: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub tool_name: String,
    pub description: String,
}

/// An owned, presentation-independent view of the current interactive session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSnapshot {
    pub generation: u64,
    pub workspace: Option<PathBuf>,
    pub session_id: Option<String>,
    pub sessions: Vec<SessionChoice>,
    pub models: Vec<ModelChoice>,
    pub selected_model: Option<String>,
    pub transcript: Vec<TranscriptItem>,
    pub live_response: String,
    pub queued_count: usize,
    pub turn_active: bool,
    pub auto_approve: bool,
    pub pending_question: Option<QuestionPrompt>,
    pub pending_approval: Option<ApprovalPrompt>,
}

impl ControllerSnapshot {
    #[allow(dead_code)]
    pub(crate) fn from_state(generation: u64, state: &AppState) -> Self {
        let pending_approval = state
            .pending_tool_confirmation
            .as_ref()
            .and_then(|confirmations| confirmations.first())
            .map(|confirmation| ApprovalPrompt {
                tool_name: confirmation.tool_name.clone(),
                description: if confirmation.content_preview.is_empty() {
                    confirmation.path.clone()
                } else {
                    format!("{}\n{}", confirmation.path, confirmation.content_preview)
                },
            });
        Self {
            generation,
            workspace: state
                .task_working_directory
                .clone()
                .or_else(|| state.workspace_root.clone()),
            session_id: Some(state.active_session_id.clone()),
            sessions: state
                .history_picker_sessions
                .iter()
                .map(|session| SessionChoice {
                    id: crate::config::session_id_from_path(&session.path).unwrap_or_default(),
                    title: session.title.clone(),
                    when: session.when.clone(),
                    message_count: session.message_count,
                })
                .collect(),
            models: state
                .config
                .models
                .iter()
                .map(|model| ModelChoice {
                    id: model.model.clone(),
                    label: model.name.clone(),
                })
                .collect(),
            selected_model: Some(state.model_name.clone()),
            transcript: state
                .history
                .iter()
                .map(|message| TranscriptItem {
                    role: message.role.clone(),
                    content: message.content.clone(),
                    tool_name: message
                        .tool_result
                        .as_ref()
                        .map(|result| result.tool_name.clone()),
                })
                .collect(),
            live_response: state.current_response.as_ref().clone(),
            queued_count: state.pending_queue.len(),
            turn_active: matches!(
                state.status,
                AppStatus::Streaming
                    | AppStatus::Queued
                    | AppStatus::AwaitingToolConfirmation
                    | AppStatus::AwaitingQuestion
            ) || state.orchestrator_running,
            auto_approve: state.auto_confirm,
            pending_question: state
                .pending_question
                .as_ref()
                .map(|question| QuestionPrompt {
                    text: question.question.clone(),
                    options: question.options.clone(),
                    multiple: question.is_multi_select,
                }),
            pending_approval,
        }
    }
}
