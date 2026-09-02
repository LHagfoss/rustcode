mod config;
mod permissions;
mod prompt;
mod session;
mod streaming;

pub(crate) use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionCapabilities, SessionCloseCapabilities, SessionConfigId,
    SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOptions, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::{Agent, Client, Stdio};
use rustcode_tasks::TaskEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

pub(crate) use config::{build_session_config_options, handle_set_config_option};
pub(crate) use permissions::{ApprovalRequirement, approval_requirement};
pub(crate) use prompt::prompt_text;
pub(crate) use session::{AcpSession, KnownTaskIds, SessionTurnState, Sessions, new_registry};
pub(crate) use streaming::{AcpEventStream, acp_stop_reason};

const ACP_TERMINAL_LEDGER_CAPACITY: usize = 1024;
const ACP_TERMINAL_BACKLOG_CAPACITY: usize = 256;
static ACTIVE_ACP_SESSIONS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();

pub(crate) fn is_acp_session(session_id: &str) -> bool {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .contains(session_id)
}

fn mark_acp_session(session_id: &str) {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .insert(session_id.to_owned());
}

fn unmark_acp_session(session_id: &str) {
    if let Some(sessions) = ACTIVE_ACP_SESSIONS.get() {
        sessions
            .lock()
            .expect("ACP session registry mutex poisoned")
            .remove(session_id);
    }
}

#[derive(Default)]
struct TerminalLedger {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl TerminalLedger {
    fn claim(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > ACP_TERMINAL_LEDGER_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

struct SessionTaskSink {
    session_id: String,
    state: Arc<Mutex<crate::app::AppState>>,
    connection: agent_client_protocol::ConnectionTo<Client>,
    runtime: tokio::runtime::Handle,
    terminal_ledger: std::sync::Mutex<TerminalLedger>,
    terminal_backlog: Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
    terminal_overflow: Arc<AtomicBool>,
}

impl SessionTaskSink {
    fn handle_terminal(
        &self,
        event: TaskEvent,
        prompt_sender: std::sync::mpsc::SyncSender<TaskEvent>,
    ) {
        let id = event.task_id().to_string();
        if !self
            .terminal_ledger
            .lock()
            .expect("ACP terminal ledger mutex poisoned")
            .claim(&id)
        {
            return;
        }
        let Some((task_id, event_session_id, output)) =
            crate::tools::task_event_to_tool_output(event.clone())
        else {
            return;
        };
        let call_id = event
            .call_id()
            .map(str::to_owned)
            .unwrap_or_else(|| task_id.clone());
        let state = Arc::clone(&self.state);
        let connection = self.connection.clone();
        let session_id = self.session_id.clone();
        let terminal_backlog = Arc::clone(&self.terminal_backlog);
        let terminal_overflow = Arc::clone(&self.terminal_overflow);
        self.runtime.spawn(async move {
            {
                let mut state = state.lock().await;
                state
                    .history
                    .push(crate::background_task_history_message_with_call_id(
                        &task_id,
                        output.clone(),
                        Some(call_id.clone()),
                    ));
                crate::config::save_session_history(&event_session_id, &state.history);
            }
            let update = SessionUpdate::ToolCallUpdate(
                agent_client_protocol::schema::v1::ToolCallUpdate::new(
                    call_id,
                    agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
                        .status(if output.success {
                            agent_client_protocol::schema::v1::ToolCallStatus::Completed
                        } else {
                            agent_client_protocol::schema::v1::ToolCallStatus::Failed
                        })
                        .raw_output(serde_json::json!({
                            "content": output.content,
                            "exitCode": output.exit_code,
                            "changedPaths": [],
                            "truncated": output.truncated,
                        })),
                ),
            );
            let _ = connection.send_notification(SessionNotification::new(session_id, update));
            // Do not expose the fallback event until its history and ACP
            // notification have been handled. This keeps a prompt that wins
            // the receive race from resuming before durable completion.
            {
                let mut backlog = terminal_backlog
                    .lock()
                    .expect("ACP terminal backlog mutex poisoned");
                backlog.push_back(event.clone());
                while backlog.len() > ACP_TERMINAL_BACKLOG_CAPACITY {
                    backlog.pop_front();
                    terminal_overflow.store(true, Ordering::Release);
                }
            }
            // Release the correlated wakeup only after history persistence and
            // the ACP notification have been scheduled, so a continuation
            // cannot race ahead of its durable tool evidence.
            let _ = prompt_sender.try_send(event);
        });
    }
}

#[derive(Clone)]
struct TaskRoute {
    sender: std::sync::mpsc::SyncSender<TaskEvent>,
    known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    sink: Option<Arc<SessionTaskSink>>,
}

type TaskRoutes = Arc<std::sync::Mutex<HashMap<String, TaskRoute>>>;

fn route_task_event(routes: &TaskRoutes, event: TaskEvent) {
    let session_id = event.session_id().to_string();
    let mut routes = routes.lock().expect("ACP task routes mutex poisoned");
    let Some(route) = routes.get(&session_id) else {
        return;
    };
    route
        .known_task_ids
        .lock()
        .expect("ACP known task IDs mutex poisoned")
        .insert(event.task_id().to_string());
    if event.is_terminal()
        && let Some(sink) = &route.sink
    {
        sink.handle_terminal(event, route.sender.clone());
        return;
    }
    match route.sender.try_send(event) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            routes.remove(&session_id);
        }
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            // Terminal events are already persisted and retained in the
            // bounded sink backlog. Keep the route alive so the prompt can
            // consume the coalesced completion once its queue drains.
        }
    }
}

#[derive(Clone)]
struct AcpServer {
    sessions: Sessions,
    client: Arc<reqwest::Client>,
    auto_approve: bool,
    task_routes: TaskRoutes,
}

impl AcpServer {
    fn new(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        let task_routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let task_subscription = crate::tools::background_task_manager().subscribe();
        let router_routes = Arc::clone(&task_routes);
        std::thread::Builder::new()
            .name("rustcode-acp-task-router".to_owned())
            .spawn(move || {
                while let Ok(event) = task_subscription.recv() {
                    route_task_event(&router_routes, event);
                }
            })
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        Ok(Self {
            sessions: new_registry(),
            client: Arc::new(client),
            auto_approve,
            task_routes,
        })
    }
}

pub async fn run_acp(auto_approve: bool) -> Result<(), Box<dyn std::error::Error>> {
    let startup_state = crate::app::AppState::new();
    crate::mcp::start_enabled_servers(&startup_state.config.mcp_servers, |name| async move {
        crate::mcp::start_server_by_name(&name).await
    })
    .await;

    let server = AcpServer::new(auto_approve)?;
    Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version).agent_capabilities(
                        AgentCapabilities::new().session_capabilities(
                            SessionCapabilities::new().close(SessionCloseCapabilities::new()),
                        ),
                    ),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: NewSessionRequest, responder, _connection| {
                    let mut state = crate::app::AppState::new();
                    let session_id = state.active_session_id.clone();
                    state.raw_cli_mode = false;
                    state.workspace_root = Some(request.cwd.clone());
                    let config_options = build_session_config_options(&state);
                    let (task_sender, task_receiver) = std::sync::mpsc::sync_channel(64);
                    let known_task_ids = Arc::new(std::sync::Mutex::new(KnownTaskIds::default()));
                    let terminal_backlog = Arc::new(std::sync::Mutex::new(VecDeque::new()));
                    let terminal_overflow = Arc::new(AtomicBool::new(false));
                    let state = Arc::new(Mutex::new(state));
                    let task_sink = Arc::new(SessionTaskSink {
                        session_id: session_id.clone(),
                        state: Arc::clone(&state),
                        connection: _connection.clone(),
                        runtime: tokio::runtime::Handle::current(),
                        terminal_ledger: std::sync::Mutex::new(TerminalLedger::default()),
                        terminal_backlog: Arc::clone(&terminal_backlog),
                        terminal_overflow: Arc::clone(&terminal_overflow),
                    });
                    mark_acp_session(&session_id);
                    server
                        .task_routes
                        .lock()
                        .expect("ACP task routes mutex poisoned")
                        .insert(
                            session_id.clone(),
                            TaskRoute {
                                sender: task_sender,
                                known_task_ids: Arc::clone(&known_task_ids),
                                sink: Some(task_sink),
                            },
                        );
                    server.sessions.lock().await.insert(
                        session_id.clone(),
                        AcpSession {
                            state,
                            cwd: request.cwd,
                            turns: Arc::new(SessionTurnState::new()),
                            task_events: Arc::new(std::sync::Mutex::new(task_receiver)),
                            known_task_ids,
                            terminal_backlog,
                            terminal_overflow,
                        },
                    );
                    responder
                        .respond(NewSessionResponse::new(session_id).config_options(config_options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: CloseSessionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    server
                        .task_routes
                        .lock()
                        .expect("ACP task routes mutex poisoned")
                        .remove(&session_id);
                    let session = server.sessions.lock().await.remove(&session_id);
                    unmark_acp_session(&session_id);
                    if let Some(session) = session {
                        session.turns.cancel_active().await;
                    }
                    crate::tools::abort_background_starts(&session_id);
                    crate::tools::stop_background_tasks(&session_id);
                    responder.respond(CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    let state = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| Arc::clone(&session.state));
                    let Some(state) = state else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    let mut state = state.lock().await;
                    let config_options =
                        handle_set_config_option(&mut state, &request.config_id, &request.value)?;
                    responder.respond(SetSessionConfigOptionResponse::new(config_options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let server = server.clone();
                async move |notification: CancelNotification, _connection| {
                    let turns = server
                        .sessions
                        .lock()
                        .await
                        .get(notification.session_id.0.as_ref())
                        .map(|session| Arc::clone(&session.turns));
                    if let Some(turns) = turns {
                        turns.cancel_active().await;
                    }
                    crate::tools::abort_background_starts(notification.session_id.0.as_ref());
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let text = prompt_text(&request.prompt);
                    let session = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| {
                            (
                                Arc::clone(&session.state),
                                session.cwd.clone(),
                                Arc::clone(&session.turns),
                                Arc::clone(&session.task_events),
                                Arc::clone(&session.known_task_ids),
                                Arc::clone(&session.terminal_backlog),
                                Arc::clone(&session.terminal_overflow),
                            )
                        });
                    let Some((
                        state,
                        cwd,
                        turns,
                        task_events,
                        known_task_ids,
                        terminal_backlog,
                        terminal_overflow,
                    )) = session
                    else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    // Request handlers run on ACP's dispatch loop. A model turn can
                    // perform many network and tool operations, so awaiting it here
                    // prevents the connection from processing any other messages.
                    let scheduled_turn = turns.schedule();
                    let prompt_server = server.clone();
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        let result = prompt::run_prompt(
                            state,
                            cwd,
                            scheduled_turn,
                            session_id.clone(),
                            task_events,
                            known_task_ids,
                            terminal_backlog,
                            terminal_overflow,
                            text,
                            task_connection,
                            prompt_server.client,
                            prompt_server.auto_approve,
                        )
                        .await;

                        match result {
                            Ok(stop_reason) => {
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
        .on_close({
            let server = server.clone();
            async move |_connection| {
                let route_ids = server
                    .task_routes
                    .lock()
                    .expect("ACP task routes mutex poisoned")
                    .drain()
                    .map(|(session_id, _)| session_id)
                    .collect::<Vec<_>>();
                let sessions = server
                    .sessions
                    .lock()
                    .await
                    .drain()
                    .map(|(session_id, session)| (session_id, session.turns))
                    .collect::<Vec<_>>();
                for (session_id, turns) in sessions {
                    unmark_acp_session(&session_id);
                    turns.cancel_active().await;
                }
                for session_id in route_ids {
                    unmark_acp_session(&session_id);
                    crate::tools::abort_background_starts(&session_id);
                    crate::tools::stop_background_tasks(&session_id);
                }
                Ok(())
            }
        })
        .connect_to(Stdio::new())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentBlock, TextContent};

    fn task_event(session_id: &str, task_id: &str) -> TaskEvent {
        TaskEvent::Finished {
            id: task_id.into(),
            session_id: session_id.into(),
            call_id: None,
            command: "cargo test".to_owned(),
            output: Ok(rustcode_command::CommandOutput {
                success: true,
                exit_code: Some(0),
                stdout: Default::default(),
                stderr: Default::default(),
            }),
        }
    }

    #[test]
    fn acp_task_router_isolates_session_events() {
        let routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (a_sender, a_receiver) = std::sync::mpsc::sync_channel(2);
        let (b_sender, b_receiver) = std::sync::mpsc::sync_channel(2);
        routes.lock().unwrap().insert(
            "a".to_owned(),
            TaskRoute {
                sender: a_sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );
        routes.lock().unwrap().insert(
            "b".to_owned(),
            TaskRoute {
                sender: b_sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );

        route_task_event(&routes, task_event("a", "task-a"));
        assert_eq!(a_receiver.try_recv().unwrap().task_id().as_str(), "task-a");
        assert!(b_receiver.try_recv().is_err());
        route_task_event(&routes, task_event("b", "task-b"));
        assert_eq!(b_receiver.try_recv().unwrap().task_id().as_str(), "task-b");
    }

    #[test]
    fn acp_task_router_removes_disconnected_bounded_route() {
        let routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        routes.lock().unwrap().insert(
            "closed".to_owned(),
            TaskRoute {
                sender,
                known_task_ids: Arc::new(std::sync::Mutex::new(KnownTaskIds::default())),
                sink: None,
            },
        );
        drop(receiver);

        route_task_event(&routes, task_event("closed", "task-closed"));

        assert!(!routes.lock().unwrap().contains_key("closed"));
    }

    #[test]
    fn acp_terminal_ledger_is_bounded() {
        let mut ledger = TerminalLedger::default();
        for index in 0..(ACP_TERMINAL_LEDGER_CAPACITY + 1) {
            assert!(ledger.claim(&format!("task-{index}")));
        }
        assert_eq!(ledger.ids.len(), ACP_TERMINAL_LEDGER_CAPACITY);
        assert!(!ledger.ids.contains("task-0"));
    }

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
                    pending: false,
                    command: None,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    completeness: rustcode_core::ToolResultCompleteness::Complete,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                    inspection: None,
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
            content: "<think>The time is 12:20 PM.</think>The current time is 12:20 PM."
                .to_string(),
            completed: true,
        });
        assert!(
            turn_finished.is_empty(),
            "TurnFinished must not duplicate final answer"
        );
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

        let pending = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "run_command".to_string(),
                content: "Task started in background".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    pending: true,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    ..Default::default()
                },
            },
        });
        assert!(matches!(
            pending.as_slice(),
            [SessionUpdate::ToolCallUpdate(update)]
                if update.tool_call_id.to_string() == "call-1"
                    && update.fields.status
                        == Some(agent_client_protocol::schema::v1::ToolCallStatus::InProgress)
        ));

        let finished = stream.updates(crate::network::AgentUiEvent::ToolFinished {
            id: "call-1".to_string(),
            result: crate::network::events::ToolResult {
                tool_name: "grep".to_string(),
                content: "match".to_string(),
                diff: None,
                file_preview: None,
                metadata: crate::network::events::ToolResultMetadata {
                    pending: false,
                    command: None,
                    call_id: Some("call-1".to_string()),
                    arguments_hash: "hash".to_string(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    completeness: rustcode_core::ToolResultCompleteness::Complete,
                    full_output_artifact: None,
                    replayed: false,
                    error_kind: None,
                    retryable: false,
                    inspection: None,
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

    #[test]
    fn acp_session_new_returns_model_option_with_all_profiles_and_default_big_selected() {
        let state = crate::app::AppState::new();
        let options = build_session_config_options(&state);

        assert_eq!(options.len(), 1);
        let model_opt = &options[0];
        assert_eq!(model_opt.id.0.as_ref(), "model");
        assert_eq!(model_opt.name, "Model");
        assert_eq!(model_opt.category, Some(SessionConfigOptionCategory::Model));

        match &model_opt.kind {
            SessionConfigKind::Select(select) => {
                assert_eq!(
                    select.current_value.0.as_ref(),
                    state.config.default.big(),
                    "default.big profile must be initially selected"
                );
                match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => {
                        assert_eq!(opts.len(), state.config.models.len());
                        for (opt, profile) in opts.iter().zip(state.config.models.iter()) {
                            assert_eq!(opt.value.0.as_ref(), profile.name);
                            assert_eq!(opt.name, profile.name);
                            if profile.model.is_empty() {
                                assert_eq!(opt.description, None);
                            } else {
                                assert_eq!(
                                    opt.description.as_deref(),
                                    Some(profile.model.as_str())
                                );
                            }
                        }
                    }
                    _ => panic!("expected ungrouped select options"),
                }
            }
            _ => panic!("expected select config option kind"),
        }
    }

    #[test]
    fn acp_new_session_response_contains_config_options() {
        let state = crate::app::AppState::new();
        let options = build_session_config_options(&state);
        let resp = NewSessionResponse::new("test-session").config_options(options);
        assert!(resp.config_options.is_some());
        let config_opts = resp.config_options.unwrap();
        assert_eq!(config_opts.len(), 1);
        assert_eq!(config_opts[0].id.0.as_ref(), "model");
    }

    #[test]
    fn acp_selecting_valid_model_updates_session_endpoint_and_model() {
        let mut state = crate::app::AppState::new();
        let target_profile = state
            .config
            .models
            .iter()
            .find(|m| m.name != state.config.default.big())
            .expect("should have at least one non-default model profile")
            .clone();

        let updated_options = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(target_profile.name.clone()),
        )
        .expect("selecting a valid model should succeed");

        assert_eq!(state.api_base_url, target_profile.url);
        assert_eq!(state.model_name, target_profile.model);

        assert_eq!(updated_options.len(), 1);
        match &updated_options[0].kind {
            SessionConfigKind::Select(select) => {
                assert_eq!(select.current_value.0.as_ref(), target_profile.name);
            }
            _ => panic!("expected select config option kind"),
        }
    }

    #[test]
    fn acp_selecting_unknown_model_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let initial_url = state.api_base_url.clone();
        let initial_model = state.model_name.clone();

        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id("non_existent_profile_12345"),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
        assert_eq!(state.api_base_url, initial_url);
        assert_eq!(state.model_name, initial_model);
    }

    #[test]
    fn acp_selecting_unknown_config_id_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("unsupported_config"),
            &SessionConfigOptionValue::value_id("gemini-3.6-flash"),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    #[test]
    fn acp_selecting_boolean_value_for_model_returns_invalid_params() {
        let mut state = crate::app::AppState::new();
        let err = handle_set_config_option(
            &mut state,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::boolean(true),
        )
        .unwrap_err();

        assert_eq!(err.code, agent_client_protocol::ErrorCode::InvalidParams);
    }

    #[test]
    fn acp_two_sessions_can_select_different_models_independently() {
        let mut state1 = crate::app::AppState::new();
        let mut state2 = crate::app::AppState::new();

        assert!(
            state1.config.models.len() >= 2,
            "need at least 2 profiles to test independence"
        );
        let profile1 = state1.config.models[0].clone();
        let profile2 = state1.config.models[1].clone();

        handle_set_config_option(
            &mut state1,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(profile1.name.clone()),
        )
        .expect("setting model for session 1");

        handle_set_config_option(
            &mut state2,
            &SessionConfigId::new("model"),
            &SessionConfigOptionValue::value_id(profile2.name.clone()),
        )
        .expect("setting model for session 2");

        assert_eq!(state1.api_base_url, profile1.url);
        assert_eq!(state1.model_name, profile1.model);

        assert_eq!(state2.api_base_url, profile2.url);
        assert_eq!(state2.model_name, profile2.model);
    }
}
