use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, InitializeRequest,
    InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, SessionUpdate, StopReason, TextContent,
    ToolCall as AcpToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Stdio};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

struct SessionTurnState {
    gate: Arc<Mutex<()>>,
    next_id: std::sync::atomic::AtomicU64,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
}

impl SessionTurnState {
    fn new() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            next_id: std::sync::atomic::AtomicU64::new(1),
            accepted: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn schedule(&self) -> ScheduledSessionTurn {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((id, cancel_token.clone()));
        ScheduledSessionTurn {
            id,
            gate: Arc::clone(&self.gate),
            cancel_token,
            accepted: Arc::clone(&self.accepted),
            registered: true,
        }
    }

    async fn begin(&self) -> SessionTurnGuard {
        self.schedule().begin().await
    }

    async fn cancel_active(&self) -> bool {
        let accepted = self
            .accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (_, token) in accepted.iter() {
            token.cancel();
        }
        !accepted.is_empty()
    }
}

struct ScheduledSessionTurn {
    id: u64,
    gate: Arc<Mutex<()>>,
    cancel_token: tokio_util::sync::CancellationToken,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
    registered: bool,
}

impl ScheduledSessionTurn {
    async fn begin(mut self) -> SessionTurnGuard {
        let gate = Arc::clone(&self.gate).lock_owned().await;
        self.registered = false;
        SessionTurnGuard {
            id: self.id,
            _gate: gate,
            cancel_token: self.cancel_token.clone(),
            accepted: Arc::clone(&self.accepted),
        }
    }
}

impl Drop for ScheduledSessionTurn {
    fn drop(&mut self) {
        if self.registered {
            self.accepted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(id, _)| *id != self.id);
        }
    }
}

struct SessionTurnGuard {
    id: u64,
    _gate: tokio::sync::OwnedMutexGuard<()>,
    cancel_token: tokio_util::sync::CancellationToken,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
}

impl SessionTurnGuard {
    fn cancel_token(&self) -> &tokio_util::sync::CancellationToken {
        &self.cancel_token
    }
}

impl Drop for SessionTurnGuard {
    fn drop(&mut self) {
        self.accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| *id != self.id);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ApprovalRequirement {
    Allow,
    Request,
    Deny(String),
}

fn approval_requirement(
    tool_calls: &[crate::tools::ToolCall],
    mode: crate::config::AgentMode,
    auto_approve: bool,
) -> ApprovalRequirement {
    let mut requires_permission = false;
    for call in tool_calls {
        match crate::tools::authorize_tool_with_args(
            &call.name,
            &call.arguments,
            mode,
            auto_approve,
            false,
        ) {
            crate::tools::AuthorizationDecision::Allow => {}
            crate::tools::AuthorizationDecision::RequireConfirmation => {
                requires_permission = true;
            }
            crate::tools::AuthorizationDecision::Deny(reason) => {
                return ApprovalRequirement::Deny(reason);
            }
        }
    }
    if requires_permission {
        ApprovalRequirement::Request
    } else {
        ApprovalRequirement::Allow
    }
}

struct AcpEventStream {
    streamed_prose: String,
    pending: String,
    in_thought: bool,
}

impl AcpEventStream {
    fn new() -> Self {
        Self {
            streamed_prose: String::new(),
            pending: String::new(),
            in_thought: false,
        }
    }

    fn text_update(text: String) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn thought_update(text: String) -> SessionUpdate {
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn flush(&mut self) -> Vec<SessionUpdate> {
        let mut updates = Vec::new();
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            if self.in_thought {
                updates.push(Self::thought_update(pending));
            } else {
                self.streamed_prose.push_str(&pending);
                updates.push(Self::text_update(pending));
            }
        }
        updates
    }

    fn process_text_delta(&mut self, text: String) -> Vec<SessionUpdate> {
        const OPEN_TAG: &str = "<think>";
        const CLOSE_TAG: &str = "</think>";
        const OPEN_PREFIXES: &[&str] = &["<think", "<thin", "<thi", "<th", "<t", "<"];
        const CLOSE_PREFIXES: &[&str] = &["</think", "</thin", "</thi", "</th", "</t", "</", "<"];

        let mut updates = Vec::new();
        self.pending.push_str(&text);

        while !self.pending.is_empty() {
            if self.in_thought {
                if let Some(idx) = self.pending.find(CLOSE_TAG) {
                    let thought = self.pending[..idx].to_string();
                    if !thought.is_empty() {
                        updates.push(Self::thought_update(thought));
                    }
                    self.pending.drain(..idx + CLOSE_TAG.len());
                    self.in_thought = false;
                } else {
                    let matched_prefix_len = CLOSE_PREFIXES
                        .iter()
                        .find(|prefix| self.pending.ends_with(**prefix))
                        .map(|prefix| prefix.len())
                        .unwrap_or(0);

                    let emit_len = self.pending.len() - matched_prefix_len;
                    if emit_len > 0 {
                        let thought: String = self.pending.drain(..emit_len).collect();
                        updates.push(Self::thought_update(thought));
                    }
                    break;
                }
            } else {
                if let Some(idx) = self.pending.find(OPEN_TAG) {
                    let prose = self.pending[..idx].to_string();
                    if !prose.is_empty() {
                        self.streamed_prose.push_str(&prose);
                        updates.push(Self::text_update(prose));
                    }
                    self.pending.drain(..idx + OPEN_TAG.len());
                    self.in_thought = true;
                } else {
                    let matched_prefix_len = OPEN_PREFIXES
                        .iter()
                        .find(|prefix| self.pending.ends_with(**prefix))
                        .map(|prefix| prefix.len())
                        .unwrap_or(0);

                    let emit_len = self.pending.len() - matched_prefix_len;
                    if emit_len > 0 {
                        let prose: String = self.pending.drain(..emit_len).collect();
                        self.streamed_prose.push_str(&prose);
                        updates.push(Self::text_update(prose));
                    }
                    break;
                }
            }
        }

        updates
    }

    fn updates(&mut self, event: crate::network::AgentUiEvent) -> Vec<SessionUpdate> {
        match event {
            crate::network::AgentUiEvent::TextDelta { text } => self.process_text_delta(text),
            crate::network::AgentUiEvent::ToolStarted { name, id } => {
                let mut updates = self.flush();
                updates.push(SessionUpdate::ToolCall(
                    AcpToolCall::new(id, name).status(ToolCallStatus::InProgress),
                ));
                updates
            }
            crate::network::AgentUiEvent::ToolFinished { id, result } => {
                let status = if result.metadata.success {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                let mut updates = self.flush();
                updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    id,
                    ToolCallUpdateFields::new()
                        .status(status)
                        .raw_output(serde_json::json!({
                            "content": result.content,
                            "exitCode": result.metadata.exit_code,
                            "changedPaths": result.metadata.changed_paths,
                            "truncated": result.metadata.truncated,
                        })),
                )));
                updates
            }
            crate::network::AgentUiEvent::TurnRecovered { message } => {
                let mut updates = self.flush();
                updates.push(Self::thought_update(message));
                updates
            }
            crate::network::AgentUiEvent::TurnFinished { content, .. } => {
                let mut updates = self.flush();
                let promoted = crate::network::text::promote_bare_thought_markers(&content);
                let prose = crate::network::text::strip_think_blocks(&promoted);
                let trimmed_prose = prose.trim();
                if !trimmed_prose.is_empty()
                    && !self.streamed_prose.ends_with(&prose)
                    && !self.streamed_prose.ends_with(trimmed_prose)
                {
                    self.streamed_prose.push_str(&prose);
                    updates.push(Self::text_update(prose));
                }
                updates
            }
            crate::network::AgentUiEvent::PromptStarted { .. }
            | crate::network::AgentUiEvent::SubagentUpdated { .. }
            | crate::network::AgentUiEvent::ApprovalRequested { .. }
            | crate::network::AgentUiEvent::Cancelled { .. }
            | crate::network::AgentUiEvent::Error { .. } => Vec::new(),
        }
    }
}

fn acp_stop_reason(cancelled: bool, harness_reason: Option<&str>) -> StopReason {
    if cancelled {
        StopReason::Cancelled
    } else if harness_reason
        .is_some_and(|reason| reason.starts_with("budget:") || reason == "loop_escalation")
    {
        StopReason::MaxTurnRequests
    } else {
        StopReason::EndTurn
    }
}

struct AcpSession {
    state: Arc<Mutex<crate::app::AppState>>,
    cwd: PathBuf,
    turns: Arc<SessionTurnState>,
}

type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

struct AcpPolicy {
    connection: ConnectionTo<Client>,
    session_id: String,
    auto_approve: bool,
}

impl crate::network::policy::TurnPolicy for AcpPolicy {
    fn should_approve(
        &self,
        state: &Arc<Mutex<crate::app::AppState>>,
        tool_calls: &[crate::tools::ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let connection = self.connection.clone();
        let session_id = self.session_id.clone();
        let auto_approve = self.auto_approve;
        let state = Arc::clone(state);
        let tool_calls = tool_calls.to_vec();
        async move {
            let mode = state.lock().await.agent_mode;
            if let ApprovalRequirement::Deny(_) =
                approval_requirement(&tool_calls, mode, auto_approve)
            {
                return false;
            }
            if auto_approve {
                return true;
            }

            for (index, call) in tool_calls.iter().enumerate() {
                match crate::tools::authorize_tool_with_args(
                    &call.name,
                    &call.arguments,
                    mode,
                    false,
                    false,
                ) {
                    crate::tools::AuthorizationDecision::Allow => continue,
                    crate::tools::AuthorizationDecision::Deny(_) => return false,
                    crate::tools::AuthorizationDecision::RequireConfirmation => {}
                }

                let call_id = call
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("acp-{index}-{}", call.name));
                let tool_call = ToolCallUpdate::new(
                    call_id,
                    ToolCallUpdateFields::new()
                        .title(call.name.clone())
                        .raw_input(call.arguments.clone()),
                );
                let request = RequestPermissionRequest::new(
                    session_id.clone(),
                    tool_call,
                    vec![
                        PermissionOption::new(
                            "allow_once",
                            "Allow once",
                            PermissionOptionKind::AllowOnce,
                        ),
                        PermissionOption::new(
                            "reject_once",
                            "Reject",
                            PermissionOptionKind::RejectOnce,
                        ),
                    ],
                );
                let Ok(response) = connection.send_request(request).block_task().await else {
                    return false;
                };
                let approved = matches!(
                    response.outcome,
                    RequestPermissionOutcome::Selected(selected)
                        if selected.option_id.0.as_ref() == "allow_once"
                );
                if !approved {
                    return false;
                }
            }
            true
        }
    }

    fn should_verify_completion(&self) -> bool {
        true
    }
}

pub async fn run_acp(auto_approve: bool) -> Result<(), Box<dyn std::error::Error>> {
    let startup_state = crate::app::AppState::new();
    crate::mcp::start_enabled_servers(&startup_state.config.mcp_servers, |name| async move {
        crate::mcp::start_server_by_name(&name).await
    })
    .await;

    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let sessions = Arc::clone(&sessions);
                async move |request: NewSessionRequest, responder, _connection| {
                    let mut state = crate::app::AppState::new();
                    let session_id = state.active_session_id.clone();
                    state.raw_cli_mode = false;
                    state.workspace_root = Some(request.cwd.clone());
                    sessions.lock().await.insert(
                        session_id.clone(),
                        AcpSession {
                            state: Arc::new(Mutex::new(state)),
                            cwd: request.cwd,
                            turns: Arc::new(SessionTurnState::new()),
                        },
                    );
                    responder.respond(NewSessionResponse::new(session_id))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let sessions = Arc::clone(&sessions);
                async move |notification: CancelNotification, _connection| {
                    let turns = sessions
                        .lock()
                        .await
                        .get(notification.session_id.0.as_ref())
                        .map(|session| Arc::clone(&session.turns));
                    if let Some(turns) = turns {
                        turns.cancel_active().await;
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let sessions = Arc::clone(&sessions);
                async move |request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let text = prompt_text(&request.prompt);
                    let session = sessions.lock().await.get(&session_id).map(|session| {
                        (
                            Arc::clone(&session.state),
                            session.cwd.clone(),
                            Arc::clone(&session.turns),
                        )
                    });
                    let Some((state, cwd, turns)) = session else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    // Request handlers run on ACP's dispatch loop. A model turn can
                    // perform many network and tool operations, so awaiting it here
                    // prevents the connection from processing any other messages.
                    let task_connection = connection.clone();
                    let scheduled_turn = turns.schedule();
                    connection.spawn(async move {
                        let result = async {
                            let turn = scheduled_turn.begin().await;
                            crate::tools::set_active_session_id(Some(session_id.clone()));
                            crate::tools::set_active_workspace_root(Some(cwd.clone()));
                            let prompt_tokens = text.split_whitespace().collect::<Vec<_>>();
                            if prompt_tokens.first() == Some(&"/memory") && prompt_tokens.len() > 1
                            {
                                if let Some(message) =
                                    crate::memory::command(Some(&cwd), &prompt_tokens[1..])
                                {
                                    state
                                        .lock()
                                        .await
                                        .history
                                        .push(crate::app::ChatMessage::new(
                                            "system",
                                            message.clone(),
                                        ));
                                    return Ok((message, StopReason::EndTurn));
                                }
                            }
                            state
                                .lock()
                                .await
                                .history
                                .push(crate::app::ChatMessage::new("user", text.clone()));

                            let client = reqwest::Client::builder()
                                .connect_timeout(std::time::Duration::from_secs(10))
                                .build()
                                .map_err(|error| {
                                    agent_client_protocol::Error::internal_error()
                                        .data(error.to_string())
                                })?;
                            let stream_buffer =
                                Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
                            let policy = Arc::new(AcpPolicy {
                                connection: task_connection.clone(),
                                session_id: session_id.clone(),
                                auto_approve,
                            });
                            let (events, mut receiver) =
                                crate::network::AgentUiEventSender::channel();
                            let mut event_stream = AcpEventStream::new();
                            let mut run =
                                Box::pin(crate::network::ui_adapter::run_agent_turn_with_events(
                                    &client,
                                    &state,
                                    turn.cancel_token(),
                                    &policy,
                                    &stream_buffer,
                                    text.clone(),
                                    events,
                                ));
                            let context = loop {
                                tokio::select! {
                                    context = &mut run => break context,
                                    event = receiver.recv() => {
                                        let Some(event) = event else { continue };
                                        for update in event_stream.updates(event) {
                                            task_connection.send_notification(
                                                SessionNotification::new(session_id.clone(), update)
                                            )?;
                                        }
                                    }
                                }
                            };
                            while let Ok(event) = receiver.try_recv() {
                                for update in event_stream.updates(event) {
                                    task_connection.send_notification(SessionNotification::new(
                                        session_id.clone(),
                                        update,
                                    ))?;
                                }
                            }
                            let harness_reason =
                                context.stop_reason.as_ref().map(ToString::to_string);
                            let stop_reason = acp_stop_reason(
                                turn.cancel_token().is_cancelled(),
                                harness_reason.as_deref(),
                            );
                            Ok((String::new(), stop_reason))
                        }
                        .await;

                        match result {
                            Ok((prose, stop_reason)) => {
                                if !prose.is_empty() {
                                    task_connection.send_notification(SessionNotification::new(
                                        session_id.clone(),
                                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                            ContentBlock::Text(TextContent::new(prose)),
                                        )),
                                    ))?;
                                }
                                responder.respond(PromptResponse::new(stop_reason))?;
                            }
                            Err(error) => {
                                responder.respond_with_error(error)?;
                            }
                        }
                        Ok(())
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await?;
    Ok(())
}

fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};

    #[test]
    fn extracts_text_blocks_from_prompt() {
        let prompt = vec![
            ContentBlock::Text(TextContent::new("inspect the project")),
            ContentBlock::Text(TextContent::new(" and explain it")),
        ];

        assert_eq!(prompt_text(&prompt), "inspect the project and explain it");
    }

    #[test]
    fn session_state_can_keep_workspace_root() {
        let mut state = crate::app::AppState::new();
        state.workspace_root = Some(PathBuf::from("/tmp/project"));
        assert_eq!(state.workspace_root, Some(PathBuf::from("/tmp/project")));
    }

    fn call(name: &str) -> crate::tools::ToolCall {
        crate::tools::ToolCall {
            name: name.to_string(),
            arguments: serde_json::json!({}),
            call_id: Some(format!("call-{name}")),
        }
    }

    #[test]
    fn acp_permissions_allow_reads_but_request_mutations() {
        assert_eq!(
            approval_requirement(&[call("grep")], crate::config::AgentMode::Build, false),
            ApprovalRequirement::Allow
        );
        assert_eq!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Build,
                false
            ),
            ApprovalRequirement::Request
        );
    }

    #[test]
    fn acp_yolo_is_an_explicit_permission_bypass() {
        assert_eq!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Build,
                true
            ),
            ApprovalRequirement::Allow
        );
    }

    #[test]
    fn acp_plan_mode_denies_mutations_even_with_yolo() {
        assert!(matches!(
            approval_requirement(
                &[call("write_to_file")],
                crate::config::AgentMode::Plan,
                true
            ),
            ApprovalRequirement::Deny(_)
        ));
    }

    #[tokio::test]
    async fn session_turns_are_serialized() {
        let turn_state = Arc::new(SessionTurnState::new());
        let first = turn_state.begin().await;
        let second_state = Arc::clone(&turn_state);
        let second = tokio::spawn(async move { second_state.begin().await });

        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "a second prompt must wait for the active turn"
        );

        drop(first);
        let second = second.await.expect("queued prompt task");
        assert!(!second.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_a_session_cancels_only_the_active_turn() {
        let turn_state = SessionTurnState::new();
        let first = turn_state.begin().await;

        assert!(turn_state.cancel_active().await);
        assert!(first.cancel_token().is_cancelled());
        drop(first);

        assert!(!turn_state.cancel_active().await);
        let next = turn_state.begin().await;
        assert!(!next.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_reaches_an_accepted_prompt_waiting_for_the_session_gate() {
        let turn_state = Arc::new(SessionTurnState::new());
        let first = turn_state.begin().await;
        let queued_state = Arc::clone(&turn_state);
        let queued = tokio::spawn(async move { queued_state.begin().await });
        tokio::task::yield_now().await;

        assert!(turn_state.cancel_active().await);
        assert!(first.cancel_token().is_cancelled());
        drop(first);

        let queued = queued.await.expect("queued prompt task");
        assert!(queued.cancel_token().is_cancelled());
        drop(queued);

        let later = turn_state.begin().await;
        assert!(!later.cancel_token().is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_reaches_a_prompt_before_its_task_starts() {
        let turn_state = SessionTurnState::new();
        let scheduled = turn_state.schedule();

        assert!(turn_state.cancel_active().await);
        let turn = scheduled.begin().await;
        assert!(turn.cancel_token().is_cancelled());
    }

    #[test]
    fn acp_event_stream_emits_incremental_text_without_final_duplication() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "hello".to_string(),
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "hello")
        ));

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "hello".to_string(),
            completed: true,
        });
        assert!(
            updates.is_empty(),
            "final content must not repeat streamed text"
        );
    }

    #[test]
    fn acp_event_stream_uses_final_text_when_no_delta_was_observed() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "fallback".to_string(),
            completed: true,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "fallback")
        ));
    }

    #[test]
    fn acp_event_stream_keeps_final_text_after_an_earlier_tool_round_delta() {
        let mut stream = AcpEventStream::new();
        stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "checking".to_string(),
        });

        let updates = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "done".to_string(),
            completed: true,
        });
        assert!(matches!(
            updates.as_slice(),
            [SessionUpdate::AgentMessageChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "done")
        ));
    }

    #[test]
    fn acp_event_stream_plain_text_remains_unchanged() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Hello, how can I help you today?".to_string(),
        });
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Hello, how can I help you today?")
        ));
    }

    #[test]
    fn acp_event_stream_complete_think_blocks_emitted_as_thought_chunks() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>Let me analyze the problem first.</think>".to_string(),
        });
        assert_eq!(updates.len(), 1);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Let me analyze the problem first.")
        ));
    }

    #[test]
    fn acp_event_stream_thought_tags_split_across_multiple_text_delta_events() {
        let mut stream = AcpEventStream::new();

        // Opening tag split: "<th" + "ink>Internal reasoning</th" + "ink>Visible answer"
        let mut all_updates = Vec::new();
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<th".to_string(),
        }));
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "ink>Internal reasoning</th".to_string(),
        }));
        all_updates.extend(stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "ink>Visible answer".to_string(),
        }));

        let message_chunks: Vec<String> = all_updates
            .iter()
            .filter_map(|u| match u {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        let thought_chunks: Vec<String> = all_updates
            .iter()
            .filter_map(|u| match u {
                SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();

        assert_eq!(thought_chunks.join(""), "Internal reasoning");
        assert_eq!(message_chunks.join(""), "Visible answer");
    }

    #[test]
    fn acp_event_stream_text_before_and_after_thought_block() {
        let mut stream = AcpEventStream::new();
        let updates = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "Prefix prose. <think>Secret reasoning.</think> Suffix prose.".to_string(),
        });

        assert_eq!(updates.len(), 3);
        assert!(matches!(
            &updates[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Prefix prose. ")
        ));
        assert!(matches!(
            &updates[1],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Secret reasoning.")
        ));
        assert!(matches!(
            &updates[2],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == " Suffix prose.")
        ));
    }

    #[test]
    fn acp_event_stream_tool_call_round_followed_by_final_answer() {
        let mut stream = AcpEventStream::new();

        // Round 1: Model thinks about calling get_time, then calls tool
        let r1_deltas = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>I need to check the current time.</think>".to_string(),
        });
        assert!(matches!(
            r1_deltas.as_slice(),
            [SessionUpdate::AgentThoughtChunk(chunk)]
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "I need to check the current time.")
        ));

        let tool_start = stream.updates(crate::network::AgentUiEvent::ToolStarted {
            name: "get_time".to_string(),
            id: "call-1".to_string(),
        });
        assert!(matches!(
            tool_start.as_slice(),
            [SessionUpdate::ToolCall(call)]
                if call.tool_call_id.to_string() == "call-1"
        ));

        let tool_finish = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "get_time".to_string(),
                content: "12:20 PM".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                },
            },
        });
        assert!(matches!(
            tool_finish.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
        ));

        // Round 2: Model thinks about the result and emits the final answer
        let r2_deltas = stream.updates(crate::network::AgentUiEvent::TextDelta {
            text: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM.".to_string(),
        });
        assert_eq!(r2_deltas.len(), 2);
        assert!(matches!(
            &r2_deltas[0],
            SessionUpdate::AgentThoughtChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "The time is 12:20 PM.")
        ));
        assert!(matches!(
            &r2_deltas[1],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "The current time is 12:20 PM.")
        ));

        // TurnFinished does not duplicate
        let turn_finished = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM.".to_string(),
            completed: true,
        });
        assert!(turn_finished.is_empty(), "TurnFinished must not duplicate final answer");
    }

    #[test]
    fn acp_event_stream_turn_finished_fallback_strips_think_blocks_when_not_streamed() {
        let mut stream = AcpEventStream::new();
        let finished = stream.updates(crate::network::AgentUiEvent::TurnFinished {
            content: "<think>Reasoning scratchpad</think>Unstreamed answer".to_string(),
            completed: true,
        });
        assert_eq!(finished.len(), 1);
        assert!(matches!(
            &finished[0],
            SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, ContentBlock::Text(text) if text.text == "Unstreamed answer")
        ));
    }

    #[test]
    fn acp_event_stream_reports_tool_start_and_completion() {
        let mut stream = AcpEventStream::new();
        let started = stream.updates(crate::network::AgentUiEvent::ToolStarted {
            name: "grep".to_string(),
            id: "call-1".to_string(),
        });
        assert!(matches!(
            started.as_slice(),
            [SessionUpdate::ToolCall(call)]
                if call.tool_call_id.to_string() == "call-1"
                    && call.status == agent_client_protocol::schema::v1::ToolCallStatus::InProgress
        ));

        let finished = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "grep".to_string(),
                content: "match".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                },
            },
        });
        assert!(matches!(
            finished.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
                    && update.fields.status
                        == Some(agent_client_protocol::schema::v1::ToolCallStatus::Completed)
        ));
    }

    #[test]
    fn acp_stop_reason_preserves_cancellation_and_turn_limits() {
        assert_eq!(
            acp_stop_reason(true, Some("provider_error")),
            StopReason::Cancelled
        );
        assert_eq!(
            acp_stop_reason(false, Some("budget:max_tool_rounds")),
            StopReason::MaxTurnRequests
        );
        assert_eq!(
            acp_stop_reason(false, Some("loop_escalation")),
            StopReason::MaxTurnRequests
        );
        assert_eq!(
            acp_stop_reason(false, Some("completed")),
            StopReason::EndTurn
        );
    }
}
