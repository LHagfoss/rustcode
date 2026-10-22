use agent_client_protocol::schema::v1::{
    ContentBlock, SessionInfoUpdate, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{Client, ConnectionTo};
use rustcode_tasks::{TaskEvent, TaskManager};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use tokio::sync::Mutex;

use super::permissions::AcpPolicy;
use super::session::{KnownTaskIds, ScheduledSessionTurn};
use super::streaming::{AcpEventStream, acp_stop_reason};

struct BackgroundTurnTasks {
    existing: HashSet<String>,
    pending: HashSet<String>,
    terminal: HashSet<String>,
}

struct SessionTitleTracker {
    last_title: Option<String>,
    checked_after_prompt_start: bool,
}

impl SessionTitleTracker {
    fn new(title: Option<String>) -> Self {
        Self {
            last_title: title,
            checked_after_prompt_start: false,
        }
    }

    fn observe_title(&mut self, title: Option<String>) -> Option<SessionUpdate> {
        if self.last_title == title {
            return None;
        }
        self.last_title = title.clone();
        title.map(|title| SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)))
    }

    fn observe_event(
        &mut self,
        event: &crate::network::AgentUiEvent,
        session_id: &str,
    ) -> Option<SessionUpdate> {
        let check = match event {
            crate::network::AgentUiEvent::PromptStarted { .. } => false,
            crate::network::AgentUiEvent::ToolFinished { result, .. }
                if result.tool_name == "set_session_title" && result.metadata.success =>
            {
                true
            }
            _ if !self.checked_after_prompt_start => {
                self.checked_after_prompt_start = true;
                true
            }
            _ => false,
        };
        check
            .then(|| crate::config::load_session_title(session_id))
            .flatten()
            .and_then(|title| self.observe_title(Some(title)))
    }
}

impl BackgroundTurnTasks {
    fn new(
        manager: &TaskManager,
        session_id: &str,
        known_task_ids: &Arc<std::sync::Mutex<KnownTaskIds>>,
    ) -> Self {
        let mut existing = manager
            .list(session_id)
            .into_iter()
            .map(|task| task.id.to_string())
            .collect::<HashSet<_>>();
        existing.extend(
            manager
                .completions(session_id)
                .into_iter()
                .map(|completion| completion.id.to_string()),
        );
        existing.extend(
            known_task_ids
                .lock()
                .expect("ACP known task IDs mutex poisoned")
                .snapshot(),
        );
        Self {
            existing,
            pending: HashSet::new(),
            terminal: HashSet::new(),
        }
    }

    fn observe_live_tasks(&mut self, manager: &TaskManager, session_id: &str) {
        for task in manager.list(session_id) {
            let id = task.id.to_string();
            if task.notify_on_complete && !self.existing.contains(&id) {
                self.pending.insert(id);
            }
        }
    }

    fn observe_retained_completions(
        &mut self,
        completions: impl IntoIterator<Item = rustcode_tasks::TaskCompletion>,
    ) {
        for completion in completions {
            let id = completion.id.to_string();
            if completion.notify_on_complete && !self.existing.contains(&id) {
                self.pending.insert(id.clone());
                self.terminal.insert(id);
            }
        }
    }

    /// Mark as finished the pending tasks that are no longer `live`, whether
    /// or not their completion is still retained.
    fn observe_finished(&mut self, live: &HashSet<String>) {
        let finished = self
            .pending
            .iter()
            .filter(|id| !live.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        self.terminal.extend(finished);
    }

    fn complete(&self) -> bool {
        !self.pending.is_empty() && self.pending.len() == self.terminal.len()
    }
}

/// Largest base64 payload accepted before decoding: 20 MiB of image bytes.
const MAX_ENCODED_IMAGE_LEN: usize = (20usize * 1024 * 1024).div_ceil(3) * 4;

/// The user turn built from an ACP prompt: text blocks verbatim, each image
/// saved as an attachment and referenced by a `![image](file://…)` marker in
/// block order, the same marker the TUI and the remote client send.
#[derive(Debug)]
pub(crate) struct PromptInput {
    pub(crate) text: String,
}

pub(crate) fn prompt_input(prompt: &[ContentBlock]) -> Result<PromptInput, String> {
    prompt_input_with(prompt, |bytes, extension| {
        crate::controller::save_image_attachment(bytes, extension)
    })
}

fn prompt_input_with(
    prompt: &[ContentBlock],
    mut save_image: impl FnMut(&[u8], &str) -> std::io::Result<PathBuf>,
) -> Result<PromptInput, String> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let mut text = String::new();
    for block in prompt {
        match block {
            ContentBlock::Text(block) => text.push_str(&block.text),
            ContentBlock::Image(image) => {
                let extension = match image.mime_type.to_ascii_lowercase().as_str() {
                    "image/png" => "png",
                    "image/jpeg" | "image/jpg" => "jpg",
                    "image/gif" => "gif",
                    "image/webp" => "webp",
                    other => {
                        return Err(format!(
                            "unsupported image type {other}; use PNG, JPEG, GIF or WebP"
                        ));
                    }
                };
                if image.data.len() > MAX_ENCODED_IMAGE_LEN {
                    return Err("images must be 20 MiB or smaller".into());
                }
                let bytes = STANDARD
                    .decode(image.data.trim())
                    .map_err(|error| format!("image data is not valid base64: {error}"))?;
                let path = save_image(&bytes, extension)
                    .map_err(|error| format!("could not save the image: {error}"))?;
                if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                    text.push('\n');
                }
                text.push_str(&format!("![image](file://{})", path.display()));
                text.push('\n');
            }
            _ => {}
        }
    }
    Ok(PromptInput { text })
}

/// Answers a report command (`commands::is_report`) without taking the
/// session's turn: reports only read the session, so they need not wait for a
/// running turn to end. The reply, then the usage, as a command answers.
pub(crate) async fn report_updates(
    state: &Mutex<crate::app::AppState>,
    text: &str,
) -> Vec<SessionUpdate> {
    use agent_client_protocol::schema::v1::{ContentChunk, TextContent};
    let mut state = state.lock().await;
    let reply = match super::commands::classify(&mut state, text) {
        Some(super::commands::AcpCommand::Reply(reply)) => reply,
        _ => format!("`{}` finished.", text.trim()),
    };
    // `report` tells a client this answer is the report's, not text of a
    // turn that may be streaming into the same session at the same time.
    let mut meta = if super::commands::is_context_report(text) {
        super::usage::context_reply_meta(&state)
    } else {
        serde_json::Map::new()
    };
    if let Some(rustcode) = meta
        .entry("rustcode")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    {
        rustcode.insert("report".into(), true.into());
    }
    let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(reply))).meta(meta);
    vec![
        SessionUpdate::AgentMessageChunk(chunk),
        super::usage::usage_update(&state),
    ]
}

/// Sends a report's answer and ends its prompt.
pub(crate) async fn run_report(
    state: &Mutex<crate::app::AppState>,
    session_id: &str,
    text: &str,
    connection: &ConnectionTo<Client>,
) -> Result<StopReason, agent_client_protocol::Error> {
    send_updates(connection, session_id, report_updates(state, text).await)?;
    Ok(StopReason::EndTurn)
}

pub(crate) async fn run_prompt(
    state: Arc<Mutex<crate::app::AppState>>,
    cwd: PathBuf,
    scheduled_turn: ScheduledSessionTurn,
    session_id: String,
    task_events: Arc<std::sync::Mutex<Receiver<TaskEvent>>>,
    known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    terminal_backlog: Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
    terminal_overflow: Arc<AtomicBool>,
    text: String,
    connection: ConnectionTo<Client>,
    client: Arc<reqwest::Client>,
    auto_approve: bool,
    remote: Option<super::remote::RemoteManager>,
    turns: std::sync::Weak<super::session::SessionTurnState>,
) -> Result<StopReason, agent_client_protocol::Error> {
    let turn = scheduled_turn.begin().await;
    // ACP turns get the same identity as terminal turns. Remote projection,
    // cancel, and the live transcript all key off it.
    let turn_id = {
        let mut state = state.lock().await;
        state.status = crate::app::AppStatus::Streaming;
        state.current_turn_started_at = Some(std::time::Instant::now());
        state.current_turn_token_usage = None;
        state.current_turn_token_usage_is_estimated = false;
        super::steer::begin(&mut state, &session_id);
        state.begin_turn_identity()
    };
    // Unscoped debug lines and operational events follow the session whose
    // turn is running. The process-wide active session below is only the
    // fallback: concurrent turns of other sessions overwrite it.
    let result = crate::logger::scope_session(session_id.clone(), async {
        crate::tools::set_active_session_id(Some(session_id.clone()));
        crate::config::set_active_session_id(&session_id);
        let (workspace_root, sandbox_mode) = {
            let mut state = state.lock().await;
            state.auto_confirm = auto_approve;
            (
                state
                    .workspace_root
                    .clone()
                    .or_else(|| std::env::current_dir().ok()),
                state.effective_sandbox_mode(),
            )
        };
        crate::tools::set_active_workspace_context(
            workspace_root,
            Some(cwd.clone()),
            false,
            Some(sandbox_mode),
        );
        let stale_events = drain_task_events(&task_events, &terminal_backlog);
        for event in stale_events {
            if event.is_terminal() {
                known_task_ids
                    .lock()
                    .expect("ACP known task IDs mutex poisoned")
                    .remove(event.task_id().as_str());
            }
        }
        if terminal_overflow.load(Ordering::Acquire) {
            return Err(agent_client_protocol::Error::internal_error().data(
                "ACP background task completion backlog overflowed; session must be reopened",
            ));
        }
        // Slash commands answer in the transcript and never reach the model or
        // provider history, as for the remote client. `/goal` is the exception:
        // it becomes the turn's prompt.
        let (command, context_meta) = {
            let mut state = state.lock().await;
            let command = super::commands::classify(&mut state, &text);
            let context_meta = (command.is_some() && super::commands::is_context_report(&text))
                .then(|| super::usage::context_reply_meta(&state));
            (command, context_meta)
        };
        let agent_text = |text: String| {
            SessionUpdate::AgentMessageChunk(agent_client_protocol::schema::v1::ContentChunk::new(
                ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(text)),
            ))
        };
        let text = match command {
            None => text,
            Some(super::commands::AcpCommand::Goal(prompt)) => {
                // The command card says what started; the goal prompt is the
                // user turn the model answers, as a session replay shows it.
                send_updates(
                    &connection,
                    &session_id,
                    vec![
                        agent_text(
                            "**Goal**\n\nContinuous mode is on until the goal is complete.".into(),
                        ),
                        SessionUpdate::UserMessageChunk(
                            agent_client_protocol::schema::v1::ContentChunk::new(
                                ContentBlock::Text(
                                    agent_client_protocol::schema::v1::TextContent::new(
                                        prompt.clone(),
                                    ),
                                ),
                            ),
                        ),
                    ],
                )?;
                prompt
            }
            Some(command) => {
                let reply = super::commands::run(
                    command,
                    &state,
                    &client,
                    turn.cancel_token(),
                    |progress| {
                        let _ = send_updates(&connection, &session_id, vec![agent_text(progress)]);
                    },
                )
                .await;
                // A command such as `/compact` can change the context, so the
                // usage follows its reply.
                let usage = super::usage::usage_update(&*state.lock().await);
                let reply = match (agent_text(reply), context_meta) {
                    (SessionUpdate::AgentMessageChunk(chunk), Some(meta)) => {
                        SessionUpdate::AgentMessageChunk(chunk.meta(meta))
                    }
                    (reply, _) => reply,
                };
                send_updates(&connection, &session_id, vec![reply, usage])?;
                return Ok(if turn.cancel_token().is_cancelled() {
                    StopReason::Cancelled
                } else {
                    StopReason::EndTurn
                });
            }
        };
        state
            .lock()
            .await
            .history
            .push(crate::app::ChatMessage::new("user", text.clone()));

        let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
        let policy = Arc::new(AcpPolicy {
            connection: connection.clone(),
            session_id: session_id.clone(),
            auto_approve,
        });
        let task_manager = crate::tools::background_task_manager();
        let mut turn_tasks = BackgroundTurnTasks::new(task_manager, &session_id, &known_task_ids);
        let (events, mut receiver) = crate::network::AgentUiEventSender::channel();
        let mut event_stream = AcpEventStream::new().with_cwd(cwd.clone());
        let mut context = run_prompt_turn(
            &client,
            &state,
            turn.cancel_token(),
            &policy,
            &stream_buffer,
            text,
            events.clone(),
            &mut receiver,
            &connection,
            &session_id,
            &mut event_stream,
            remote.as_ref(),
        )
        .await?;

        if matches!(
            context.lifecycle.stop_reason,
            Some(crate::network::lifecycle::StopReason::BackgroundPending)
        ) {
            turn_tasks.observe_live_tasks(task_manager, &session_id);
            turn_tasks.observe_retained_completions(task_manager.completions(&session_id));
            if turn_tasks.pending.is_empty() {
                // The detached task requested history/client updates only; do not
                // wait for its terminal event or schedule a model continuation.
                context.lifecycle.task_completed = true;
                context.lifecycle.stop_reason =
                    Some(crate::network::lifecycle::StopReason::Completed);
            } else {
                // The model is done until a background command it is owed
                // finishes. The turn ends here, as in the terminal, so the
                // chat is free meanwhile; a server that never exits used to
                // keep it busy for good (#2280). The command's completion
                // resumes the turn.
                tokio::spawn(resume_after_background(BackgroundWakeup {
                    state: Arc::clone(&state),
                    cwd: cwd.clone(),
                    turns: turns.clone(),
                    session_id: session_id.clone(),
                    client: Arc::clone(&client),
                    connection: connection.clone(),
                    auto_approve,
                    remote: remote.clone(),
                    turn_tasks,
                    context,
                }));
                return Ok(StopReason::EndTurn);
            }
        }
        let harness_reason = context
            .lifecycle
            .stop_reason
            .as_ref()
            .map(ToString::to_string);
        Ok(acp_stop_reason(
            turn.cancel_token().is_cancelled(),
            harness_reason.as_deref(),
        ))
    })
    .await;
    {
        let mut state = state.lock().await;
        state.end_turn_identity(&turn_id);
        state.enter_idle();
        if state.active_turn_steerable_session.as_deref() == Some(session_id.as_str()) {
            state.active_turn_steerable_session = None;
        }
    }
    result
}

/// A turn parked on background commands, resumed once they finish.
struct BackgroundWakeup {
    state: Arc<Mutex<crate::app::AppState>>,
    cwd: PathBuf,
    /// Gone once the session closes, which ends the wait.
    turns: std::sync::Weak<super::session::SessionTurnState>,
    session_id: String,
    client: Arc<reqwest::Client>,
    connection: ConnectionTo<Client>,
    auto_approve: bool,
    remote: Option<super::remote::RemoteManager>,
    turn_tasks: BackgroundTurnTasks,
    context: crate::network::TurnContext,
}

/// How often a parked turn checks its background commands.
const BACKGROUND_WAKEUP_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Wait for the commands `wakeup` is owed, then continue its turn under the
/// session's turn gate, streaming to the client as a prompt would. A prompt
/// sent meanwhile runs first; the continuation waits for it.
fn resume_after_background(
    wakeup: BackgroundWakeup,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(resume_after_background_inner(wakeup))
}

async fn resume_after_background_inner(mut wakeup: BackgroundWakeup) {
    let task_manager = crate::tools::background_task_manager();
    let session_id = wakeup.session_id.clone();
    loop {
        tokio::time::sleep(BACKGROUND_WAKEUP_POLL).await;
        if wakeup.turns.upgrade().is_none() {
            return;
        }
        let live = task_manager
            .list(&session_id)
            .into_iter()
            .map(|task| task.id.to_string())
            .collect::<HashSet<_>>();
        wakeup.turn_tasks.observe_finished(&live);
        if wakeup.turn_tasks.complete() {
            break;
        }
    }
    let Some(turns) = wakeup.turns.upgrade() else {
        return;
    };
    let scheduled = turns.schedule();
    drop(turns);
    let turn = scheduled.begin().await;
    let turn_id = {
        let mut state = wakeup.state.lock().await;
        state.status = crate::app::AppStatus::Streaming;
        state.current_turn_started_at = Some(std::time::Instant::now());
        state.begin_turn_identity()
    };
    let state = Arc::clone(&wakeup.state);
    crate::logger::scope_session(session_id.clone(), async {
        crate::tools::set_active_session_id(Some(session_id.clone()));
        crate::config::set_active_session_id(&session_id);
        crate::tools::set_active_workspace_context(
            state.lock().await.workspace_root.clone(),
            Some(wakeup.cwd.clone()),
            false,
            None,
        );
        let policy = Arc::new(AcpPolicy {
            connection: wakeup.connection.clone(),
            session_id: session_id.clone(),
            auto_approve: wakeup.auto_approve,
        });
        let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
        let (events, mut receiver) = crate::network::AgentUiEventSender::channel();
        let mut event_stream = AcpEventStream::new().with_cwd(wakeup.cwd.clone());
        let context = run_prompt_continuation(
            &wakeup.client,
            &state,
            turn.cancel_token(),
            &policy,
            &stream_buffer,
            String::new(),
            events,
            &mut receiver,
            &wakeup.connection,
            &session_id,
            &mut event_stream,
            wakeup.context,
            wakeup.remote.as_ref(),
        )
        .await;
        // Parked again on a later command: wait for that one in turn.
        if let Ok(context) = context
            && matches!(
                context.lifecycle.stop_reason,
                Some(crate::network::lifecycle::StopReason::BackgroundPending)
            )
            && !turn.cancel_token().is_cancelled()
        {
            let mut turn_tasks = BackgroundTurnTasks {
                existing: wakeup.turn_tasks.pending,
                pending: HashSet::new(),
                terminal: HashSet::new(),
            };
            turn_tasks.observe_live_tasks(task_manager, &session_id);
            if !turn_tasks.pending.is_empty() {
                tokio::spawn(resume_after_background(BackgroundWakeup {
                    state: Arc::clone(&state),
                    cwd: wakeup.cwd.clone(),
                    turns: wakeup.turns.clone(),
                    session_id: session_id.clone(),
                    client: Arc::clone(&wakeup.client),
                    connection: wakeup.connection.clone(),
                    auto_approve: wakeup.auto_approve,
                    remote: wakeup.remote.clone(),
                    turn_tasks,
                    context,
                }));
            }
        }
    })
    .await;
    let mut state = state.lock().await;
    state.end_turn_identity(&turn_id);
    state.enter_idle();
}

fn drain_task_events(
    task_events: &Arc<std::sync::Mutex<Receiver<TaskEvent>>>,
    terminal_backlog: &Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
) -> Vec<TaskEvent> {
    let receiver = task_events.lock().expect("ACP task event mutex poisoned");
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }
    drop(receiver);
    let mut backlog = terminal_backlog
        .lock()
        .expect("ACP terminal backlog mutex poisoned");
    let mut seen = events
        .iter()
        .map(|event| event.task_id().to_string())
        .collect::<HashSet<_>>();
    while let Some(event) = backlog.pop_front() {
        if seen.insert(event.task_id().to_string()) {
            events.push(event);
        }
    }
    events
}

async fn run_prompt_turn<P: crate::network::policy::TurnPolicy + 'static>(
    client: &Arc<reqwest::Client>,
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<crate::network::StreamBuffer>>,
    text: String,
    events: crate::network::AgentUiEventSender,
    receiver: &mut crate::network::AgentUiEventReceiver,
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event_stream: &mut AcpEventStream,
    remote: Option<&super::remote::RemoteManager>,
) -> Result<crate::network::TurnContext, agent_client_protocol::Error> {
    forward_prompt_run(
        crate::network::ui_adapter::run_agent_turn_with_events_for_acp(
            client,
            state,
            cancel_token,
            policy,
            stream_buffer,
            text,
            events,
        ),
        receiver,
        connection,
        session_id,
        event_stream,
        state,
        cancel_token,
        remote,
    )
    .await
}

async fn run_prompt_continuation<P: crate::network::policy::TurnPolicy + 'static>(
    client: &Arc<reqwest::Client>,
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<crate::network::StreamBuffer>>,
    text: String,
    events: crate::network::AgentUiEventSender,
    receiver: &mut crate::network::AgentUiEventReceiver,
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event_stream: &mut AcpEventStream,
    context: crate::network::TurnContext,
    remote: Option<&super::remote::RemoteManager>,
) -> Result<crate::network::TurnContext, agent_client_protocol::Error> {
    forward_prompt_run(
        crate::network::ui_adapter::run_agent_turn_with_events_and_context_for_acp(
            client,
            state,
            cancel_token,
            policy,
            stream_buffer,
            text,
            events,
            context,
        ),
        receiver,
        connection,
        session_id,
        event_stream,
        state,
        cancel_token,
        remote,
    )
    .await
}

async fn forward_prompt_run<F>(
    run: F,
    receiver: &mut crate::network::AgentUiEventReceiver,
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event_stream: &mut AcpEventStream,
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    remote: Option<&super::remote::RemoteManager>,
) -> Result<crate::network::TurnContext, agent_client_protocol::Error>
where
    F: std::future::Future<Output = crate::network::TurnContext>,
{
    let mut run = Box::pin(run);
    let mut title_tracker = SessionTitleTracker::new(crate::config::load_session_title(session_id));
    let asked = super::questions::AskedQuestions::default();
    let context = loop {
        tokio::select! {
            context = &mut run => break context,
            event = receiver.recv() => {
                let Some(event) = event else { continue };
                let abort_background = matches!(
                    &event,
                    crate::network::AgentUiEvent::Cancelled { ..}
                ) || {
                    #[cfg(test)]
                    {
                        matches!(&event, crate::network::AgentUiEvent::Error { .. })
                    }
                    #[cfg(not(test))]
                    {
                        false
                    }
                };
                if abort_background {
                    crate::tools::abort_background_starts(session_id);
                }
                if matches!(&event, crate::network::AgentUiEvent::QuestionRequested { .. }) {
                    ask_pending_questions(connection, session_id, state, cancel_token, &asked)?;
                }
                let title_update = title_tracker.observe_event(&event, session_id);
                if let Some(remote) = remote {
                    remote.observe(session_id, &event);
                }
                send_updates(connection, session_id, event_stream.updates(event))?;
                if let Some(update) = title_update {
                    send_updates(connection, session_id, vec![update])?;
                }
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        if matches!(
            &event,
            crate::network::AgentUiEvent::QuestionRequested { .. }
        ) {
            ask_pending_questions(connection, session_id, state, cancel_token, &asked)?;
        }
        let title_update = title_tracker.observe_event(&event, session_id);
        if let Some(remote) = remote {
            remote.observe(session_id, &event);
        }
        send_updates(connection, session_id, event_stream.updates(event))?;
        if let Some(update) = title_update {
            send_updates(connection, session_id, vec![update])?;
        }
    }
    if let Some(title) = crate::config::load_session_title(session_id)
        && let Some(update) = title_tracker.observe_title(Some(title))
    {
        send_updates(connection, session_id, vec![update])?;
    }
    // Every run (the turn and each background continuation) ends with the
    // session's context and token usage, so the client's meter stays current.
    let usage = super::usage::usage_update(&*state.lock().await);
    send_updates(connection, session_id, vec![usage])?;
    Ok(context)
}

/// Forward the questions the turn is waiting on to the client. The question
/// is read from state, not from the event, so a stale event cannot pair old
/// text with a newer question's id.
fn ask_pending_questions(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    asked: &super::questions::AskedQuestions,
) -> Result<(), agent_client_protocol::Error> {
    super::questions::spawn_question_requests(
        connection,
        session_id,
        Arc::clone(state),
        cancel_token.clone(),
        Arc::clone(asked),
    )
}

#[cfg(test)]
mod tests {
    use super::{BackgroundTurnTasks, SessionTitleTracker, drain_task_events, prompt_input_with};
    use agent_client_protocol::schema::v1::{ContentBlock, ImageContent, TextContent};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use rustcode_tasks::TaskEvent;
    use std::collections::{HashSet, VecDeque};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn image(bytes: &[u8], mime: &str) -> ContentBlock {
        ContentBlock::Image(ImageContent::new(STANDARD.encode(bytes), mime))
    }

    #[test]
    fn image_blocks_become_markers_in_block_order() {
        let mut saved = Vec::new();
        let prompt = vec![
            ContentBlock::Text(TextContent::new("compare")),
            image(b"png", "image/png"),
            ContentBlock::Text(TextContent::new("with")),
            image(b"jpg", "image/JPEG"),
            image(b"gif", "image/gif"),
            image(b"webp", "image/webp"),
        ];
        let input = prompt_input_with(&prompt, |bytes, extension| {
            saved.push((bytes.to_vec(), extension.to_string()));
            Ok(PathBuf::from(format!("/a/{}.{extension}", saved.len())))
        })
        .unwrap();
        assert_eq!(
            input.text,
            "compare\n![image](file:///a/1.png)\nwith\n![image](file:///a/2.jpg)\n\
             ![image](file:///a/3.gif)\n![image](file:///a/4.webp)\n"
        );
        assert_eq!(saved[1], (b"jpg".to_vec(), "jpg".to_string()));
    }

    #[test]
    fn text_only_prompt_has_no_images() {
        let prompt = vec![ContentBlock::Text(TextContent::new("hi"))];
        let input = prompt_input_with(&prompt, |_, _| unreachable!()).unwrap();
        assert_eq!(input.text, "hi");
    }

    #[test]
    fn rejects_unsupported_oversized_and_malformed_images() {
        let save = |_: &[u8], _: &str| -> std::io::Result<PathBuf> { unreachable!() };
        assert!(
            prompt_input_with(&[image(b"x", "image/tiff")], save)
                .unwrap_err()
                .contains("unsupported image type")
        );
        let huge = ContentBlock::Image(ImageContent::new(
            "A".repeat(super::MAX_ENCODED_IMAGE_LEN + 4),
            "image/png",
        ));
        assert!(
            prompt_input_with(&[huge], save)
                .unwrap_err()
                .contains("20 MiB")
        );
        let malformed = ContentBlock::Image(ImageContent::new("not base64!", "image/png"));
        assert!(
            prompt_input_with(&[malformed], save)
                .unwrap_err()
                .contains("base64")
        );
    }

    #[test]
    fn title_tracker_emits_protocol_updates_when_persisted_title_changes() {
        let mut tracker = SessionTitleTracker::new(None);

        let update = tracker
            .observe_title(Some("Initial project title".to_owned()))
            .expect("new title should be announced");
        assert_eq!(
            serde_json::to_value(update).unwrap(),
            serde_json::json!({
                "sessionUpdate": "session_info_update",
                "title": "Initial project title"
            })
        );
        assert!(
            tracker
                .observe_title(Some("Initial project title".to_owned()))
                .is_none()
        );

        let update = tracker
            .observe_title(Some("Refined project title".to_owned()))
            .expect("changed title should be announced");
        assert_eq!(
            serde_json::to_value(update).unwrap()["title"],
            "Refined project title"
        );
    }

    #[test]
    fn a_parked_turn_wakes_once_its_tasks_leave_the_live_list() {
        let mut tracker = BackgroundTurnTasks {
            existing: HashSet::new(),
            pending: ["build".to_owned(), "tests".to_owned()]
                .into_iter()
                .collect(),
            terminal: HashSet::new(),
        };
        let live = |ids: &[&str]| {
            ids.iter()
                .map(|id| (*id).to_owned())
                .collect::<HashSet<_>>()
        };
        tracker.observe_finished(&live(&["build", "tests", "server"]));
        assert!(!tracker.complete());
        tracker.observe_finished(&live(&["tests", "server"]));
        assert!(!tracker.complete(), "one owed task still runs");
        // A server the turn is not owed keeps running; it does not hold it.
        tracker.observe_finished(&live(&["server"]));
        assert!(tracker.complete());
    }

    #[test]
    fn queued_completion_is_drained_before_a_new_prompt() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(2);
        sender
            .send(TaskEvent::Finished {
                id: "previous-task".into(),
                session_id: "session".into(),
                call_id: None,
                command: "cargo test".to_owned(),
                notify_on_complete: true,
                output: Ok(rustcode_command::CommandOutput {
                    success: true,
                    exit_code: Some(0),
                    signal: None,
                    downstream_consumer_terminated: false,
                    stdout: Default::default(),
                    stderr: Default::default(),
                }),
            })
            .unwrap();
        let events = drain_task_events(
            &Arc::new(std::sync::Mutex::new(receiver)),
            &Arc::new(std::sync::Mutex::new(VecDeque::new())),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id().as_str(), "previous-task");
        assert!(events[0].is_terminal());
    }
}

fn send_updates(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    updates: Vec<SessionUpdate>,
) -> Result<(), agent_client_protocol::Error> {
    for update in updates {
        let pending_call_id = match &update {
            SessionUpdate::ToolCallUpdate(update)
                if update.fields.status
                    == Some(agent_client_protocol::schema::v1::ToolCallStatus::InProgress) =>
            {
                Some(update.tool_call_id.to_string())
            }
            _ => None,
        };
        let result =
            connection.send_notification(SessionNotification::new(session_id.to_owned(), update));
        if pending_call_id.is_some() {
            // The pending history result is the start barrier: release only
            // after its ACP notification was accepted by the connection.
            crate::tools::release_background_start(
                pending_call_id.as_deref().expect("pending call ID missing"),
            );
        }
        result?;
    }
    Ok(())
}
