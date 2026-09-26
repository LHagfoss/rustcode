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
    SubmitWithMode {
        prompt: String,
        mode: PromptSubmitMode,
    },
    RestorePendingPrompt(PendingPrompt),
    RemovePendingPrompt(PendingPrompt),
    Cancel,
    SetAutoApprove(bool),
    SelectModel(String),
    AnswerQuestion(String),
    /// Legacy unbound decision. The controller rejects it because it cannot
    /// identify which pending batch the caller reviewed.
    Approval(ApprovalChoice),
    /// Resolve only the controller-owned batch the caller reviewed.
    ApprovalBatch {
        batch_id: String,
        choice: ApprovalChoice,
    },
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
    pub workspace: Option<PathBuf>,
}

impl SessionChoice {
    pub(crate) fn from_meta(session: &crate::config::SessionMeta) -> Self {
        let id = crate::config::session_id_from_path(&session.path).unwrap_or_default();
        let workspace = crate::config::load_session_workspace(&id).map(|record| record.cwd);
        Self {
            id,
            title: session.title.clone(),
            when: session.when.clone(),
            message_count: session.message_count,
            workspace,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSubmitMode {
    Steer,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingPromptKind {
    Steer,
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    pub kind: PendingPromptKind,
    /// Index in the controller's source collection. This intentionally keeps
    /// duplicate prompts independently addressable.
    pub position: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptItem {
    pub role: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_detail: Option<String>,
    pub tool_success: Option<bool>,
    pub tool_pending: bool,
    pub response_time_ms: Option<u64>,
    pub thought_time_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionPrompt {
    pub header: String,
    pub text: String,
    pub options: Vec<String>,
    pub descriptions: Vec<String>,
    pub multiple: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalAction {
    pub request_id: String,
    pub tool_name: String,
    pub action_summary: String,
    pub risk_context: String,
    pub description: String,
    /// Complete literal arguments, retained independently of the preview.
    pub full_details: String,
}

impl ApprovalAction {
    pub fn new(
        request_id: String,
        tool_name: String,
        action_summary: String,
        risk_context: String,
        description: String,
    ) -> Self {
        Self {
            request_id,
            tool_name,
            action_summary: bounded_preview(&action_summary, 160),
            risk_context,
            description: bounded_preview(&description, 320),
            full_details: description,
        }
    }

    pub(crate) fn from_confirmation(
        confirmation: &crate::app::ToolConfirmation,
        index: usize,
    ) -> Self {
        let request_id = confirmation.request_id.clone().unwrap_or_else(|| {
            format!(
                "local:{index}:{}:{}:{}",
                confirmation.tool_name, confirmation.path, confirmation.content_bytes
            )
        });
        let description = if confirmation.content_preview.is_empty() {
            confirmation.path.clone()
        } else {
            format!("{}\n{}", confirmation.path, confirmation.content_preview)
        };
        Self::new(
            request_id,
            confirmation.tool_name.clone(),
            format!("{} · {}", confirmation.tool_name, confirmation.path),
            "This action requires your approval before it can continue.".to_owned(),
            description,
        )
    }

    pub(crate) fn with_generation(mut self, generation: u64) -> Self {
        self.request_id = format!("{generation}:{}", self.request_id);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub tool_name: String,
    pub description: String,
}

/// Exact, controller-owned approval batch for batch-aware frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalBatchPrompt {
    /// Presentation signature for the action request IDs in this batch.
    pub request_id: String,
    /// Controller-owned authorization token for this exact pending batch.
    pub batch_id: String,
    pub actions: Vec<ApprovalAction>,
}

impl ApprovalBatchPrompt {
    pub fn new(actions: Vec<ApprovalAction>) -> Self {
        let mut request_id = format!("batch:{}", actions.len());
        for action in &actions {
            request_id.push_str(&format!(
                ":{}:{}",
                action.request_id.len(),
                action.request_id
            ));
        }
        Self {
            batch_id: String::new(),
            request_id,
            actions,
        }
    }

    pub fn with_batch_id(mut self, batch_id: String) -> Self {
        self.batch_id = batch_id;
        self
    }
}

fn bounded_preview(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}… [truncated]")
    } else {
        preview
    }
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
    pub can_steer: bool,
    pub pending_prompts: Vec<PendingPrompt>,
    pub turn_active: bool,
    pub auto_approve: bool,
    pub pending_question: Option<QuestionPrompt>,
    /// Legacy presentation-only projection. It carries no authorization token.
    pub pending_approval: Option<ApprovalPrompt>,
    /// Exact batch-aware approval data for native frontends.
    pub pending_approval_batch: Option<ApprovalBatchPrompt>,
}

impl ControllerSnapshot {
    #[allow(dead_code)]
    pub(crate) fn from_state(generation: u64, state: &AppState) -> Self {
        let pending_approval_batch = state
            .pending_tool_confirmation
            .as_ref()
            .filter(|confirmations| !confirmations.is_empty())
            .and_then(|confirmations| {
                let actions = confirmations
                    .iter()
                    .enumerate()
                    .map(|(index, confirmation)| {
                        let mut action = ApprovalAction::from_confirmation(confirmation, index);
                        if let Some(full_details) = state
                            .pending_approval_details
                            .as_ref()
                            .and_then(|details| details.get(index))
                        {
                            action.full_details = full_details.clone();
                        }
                        action.with_generation(generation)
                    })
                    .collect();
                let prompt = ApprovalBatchPrompt::new(actions);
                state
                    .pending_approval_batch_id
                    .as_ref()
                    .map(|batch_id| prompt.with_batch_id(batch_id.clone()))
            });
        let pending_approval = pending_approval_batch
            .as_ref()
            .and_then(|batch| batch.actions.first())
            .map(|action| ApprovalPrompt {
                tool_name: action.tool_name.clone(),
                description: action.description.clone(),
            });
        let pending_prompts: Vec<PendingPrompt> = state
            .pending_steers
            .iter()
            .enumerate()
            .map(|(position, prompt)| PendingPrompt {
                kind: PendingPromptKind::Steer,
                position,
                text: prompt.text.clone(),
            })
            .chain(
                state
                    .pending_queue
                    .iter()
                    .enumerate()
                    .filter(|(_, prompt)| !prompt.starts_with("__task_wakeup__:"))
                    .map(|(position, prompt)| PendingPrompt {
                        kind: PendingPromptKind::Queue,
                        position,
                        text: prompt.clone(),
                    }),
            )
            .collect();
        let mut details = std::collections::HashMap::new();
        let transcript = state
            .history
            .iter()
            .map(|message| {
                if message.role == "assistant" {
                    for call in
                        crate::tools::resolve_tool_calls(message, state.active_tool_protocol())
                    {
                        let id = call.call_id.clone().unwrap_or_else(|| {
                            format!(
                                "local_{}",
                                crate::network::tool_exec::stable_arguments_hash(&call.arguments)
                            )
                        });
                        details.insert(
                            id,
                            crate::network::ui_adapter::tool_display_detail(
                                &call.name,
                                &call.arguments,
                            ),
                        );
                    }
                }
                let tool_detail = message.tool_result.as_ref().and_then(|record| {
                    let id = message
                        .tool_call_id
                        .clone()
                        .unwrap_or_else(|| format!("local_{}", record.arguments_hash));
                    details
                        .get(&id)
                        .cloned()
                        .flatten()
                        .or_else(|| {
                            record.command.as_ref().and_then(|command| {
                                crate::network::ui_adapter::tool_display_detail(
                                    "run_command",
                                    &serde_json::json!({"command": command}),
                                )
                            })
                        })
                        .or_else(|| {
                            record.changed_paths.first().map(|path| {
                                crate::app::activity::sanitize_tool_parameter(path, 120)
                            })
                        })
                });
                TranscriptItem {
                    role: message.role.clone(),
                    content: message.content.clone(),
                    tool_name: message
                        .tool_result
                        .as_ref()
                        .map(|result| result.tool_name.clone()),
                    tool_detail,
                    tool_success: message.tool_result.as_ref().map(|result| result.success),
                    tool_pending: message
                        .tool_result
                        .as_ref()
                        .is_some_and(|result| result.pending),
                    response_time_ms: message.response_time_ms,
                    thought_time_ms: message.thought_time_ms,
                }
            })
            .collect();
        let mut sessions = state
            .history_picker_sessions
            .iter()
            .map(SessionChoice::from_meta)
            .collect::<Vec<_>>();
        if crate::config::session_has_content(&state.history) {
            sessions.retain(|choice| choice.id != state.active_session_id);
            sessions.insert(
                0,
                SessionChoice {
                    id: state.active_session_id.clone(),
                    title: crate::config::load_session_title(&state.active_session_id)
                        .unwrap_or_else(|| crate::config::session_title(&state.history)),
                    when: state
                        .history
                        .first()
                        .map(|message| message.timestamp.clone())
                        .unwrap_or_default(),
                    message_count: state.history.len(),
                    workspace: state
                        .task_working_directory
                        .clone()
                        .or_else(|| state.effective_workspace_root()),
                },
            );
        }
        Self {
            generation,
            workspace: state
                .task_working_directory
                .clone()
                .or_else(|| state.effective_workspace_root()),
            session_id: Some(state.active_session_id.clone()),
            sessions,
            models: state
                .config
                .models
                .iter()
                .map(|model| ModelChoice {
                    id: model.name.clone(),
                    label: model.name.clone(),
                })
                .collect(),
            selected_model: Some(
                state
                    .config
                    .models
                    .iter()
                    .find(|profile| {
                        profile.model == state.model_name && profile.url == state.api_base_url
                    })
                    .map(|profile| profile.name.clone())
                    .unwrap_or_else(|| state.model_name.clone()),
            ),
            transcript,
            live_response: state.current_response.as_ref().clone(),
            queued_count: pending_prompts.len(),
            can_steer: state.can_accept_steer(),
            pending_prompts,
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
                    header: question.header.clone(),
                    text: question.question.clone(),
                    options: question.options.clone(),
                    descriptions: question.descriptions.clone(),
                    multiple: question.is_multi_select,
                }),
            pending_approval,
            pending_approval_batch,
        }
    }
}

#[cfg(test)]
mod detail_tests {
    use super::{ControllerSnapshot, PendingPrompt, PendingPromptKind};
    use crate::app::{AppState, AppStatus, ChatMessage, ToolCallRef, ToolResultRecord};

    #[test]
    fn snapshot_exposes_user_pending_prompts_and_steer_capability() {
        let mut state = AppState::new();
        state.pending_queue = vec![
            "first follow-up".to_owned(),
            "__task_wakeup__:background-task".to_owned(),
            "second follow-up".to_owned(),
        ];
        state.status = AppStatus::Streaming;
        state.active_turn_steerable_session = Some(state.active_session_id.clone());
        assert!(state.queue_steer("correct the approach".to_owned()));

        let snapshot = ControllerSnapshot::from_state(1, &state);

        assert!(snapshot.can_steer);
        assert_eq!(
            snapshot.pending_prompts,
            vec![
                PendingPrompt {
                    kind: PendingPromptKind::Steer,
                    position: 0,
                    text: "correct the approach".to_owned(),
                },
                PendingPrompt {
                    kind: PendingPromptKind::Queue,
                    position: 0,
                    text: "first follow-up".to_owned(),
                },
                PendingPrompt {
                    kind: PendingPromptKind::Queue,
                    position: 2,
                    text: "second follow-up".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn snapshot_workspace_falls_back_to_the_session_source_boundary() {
        let source = tempfile::tempdir().unwrap();
        let state = AppState::new_with_workspace_session(source.path(), Some("snapshot-source"));

        assert_eq!(state.workspace_root, None);
        assert_eq!(
            ControllerSnapshot::from_state(1, &state)
                .workspace
                .as_deref(),
            Some(source.path())
        );
    }

    #[test]
    fn saved_tool_details_follow_call_ids_not_result_order() {
        let mut state = AppState::new();
        state
            .history
            .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
                ToolCallRef {
                    id: "read-a".into(),
                    name: "view_file".into(),
                    arguments: r#"{"path":"src/a.rs","start_line":3,"end_line":9}"#.into(),
                },
                ToolCallRef {
                    id: "read-b".into(),
                    name: "view_file".into(),
                    arguments: r#"{"path":"src/b.rs"}"#.into(),
                },
            ]));
        for id in ["read-b", "read-a"] {
            state.history.push(
                ChatMessage::new("tool", "contents")
                    .answering(Some(id.into()))
                    .with_tool_result(ToolResultRecord {
                        tool_name: "view_file".into(),
                        success: true,
                        ..Default::default()
                    }),
            );
        }
        let snapshot = ControllerSnapshot::from_state(1, &state);
        let details = snapshot
            .transcript
            .iter()
            .filter(|item| item.role == "tool")
            .map(|item| item.tool_detail.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(details, [Some("src/b.rs"), Some("src/a.rs (lines 3-9)")]);
    }

    #[test]
    fn legacy_command_details_are_bounded_and_read_results_are_not_guessed() {
        let mut state = AppState::new();
        state.history.push(
            ChatMessage::new("tool", "output").with_tool_result(ToolResultRecord {
                tool_name: "run_command".into(),
                command: Some("cargo check\n--tests".into()),
                success: true,
                ..Default::default()
            }),
        );
        state.history.push(
            ChatMessage::new("tool", "private file contents").with_tool_result(ToolResultRecord {
                tool_name: "view_file".into(),
                success: true,
                ..Default::default()
            }),
        );
        let snapshot = ControllerSnapshot::from_state(1, &state);
        let tools = snapshot
            .transcript
            .iter()
            .filter(|item| item.role == "tool")
            .collect::<Vec<_>>();
        assert_eq!(tools[0].tool_detail.as_deref(), Some("cargo check --tests"));
        assert_eq!(tools[1].tool_detail, None);
    }
}
