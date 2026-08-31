use agent_client_protocol::schema::v1::{
    ContentBlock, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{Client, ConnectionTo};
use rustcode_tasks::{TaskEvent, TaskManager};
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use tokio::sync::Mutex;

use super::permissions::AcpPolicy;
use super::session::{KnownTaskIds, ScheduledSessionTurn};
use super::streaming::{AcpEventStream, acp_stop_reason};

struct BackgroundTurnTasks {
    existing: HashSet<String>,
    pending: HashSet<String>,
    terminal: HashSet<String>,
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
            if !self.existing.contains(&id) {
                self.pending.insert(id);
            }
        }
    }

    fn observe_event(
        &mut self,
        event: &TaskEvent,
        known_task_ids: &Arc<std::sync::Mutex<KnownTaskIds>>,
    ) -> bool {
        let id = event.task_id().to_string();
        if self.existing.contains(&id) {
            if event.is_terminal() {
                known_task_ids
                    .lock()
                    .expect("ACP known task IDs mutex poisoned")
                    .remove(&id);
            }
            return false;
        }
        self.pending.insert(id.clone());
        if event.is_terminal() {
            self.terminal.insert(id);
            known_task_ids
                .lock()
                .expect("ACP known task IDs mutex poisoned")
                .remove(event.task_id().as_str());
        }
        true
    }

    fn complete(&self) -> bool {
        !self.pending.is_empty() && self.pending.len() == self.terminal.len()
    }
}

pub(crate) fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
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
) -> Result<StopReason, agent_client_protocol::Error> {
    let turn = scheduled_turn.begin().await;
    crate::tools::set_active_session_id(Some(session_id.clone()));
    crate::tools::set_active_workspace_root(Some(cwd.clone()));
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
        return Err(agent_client_protocol::Error::internal_error()
            .data("ACP background task completion backlog overflowed; session must be reopened"));
    }
    let prompt_tokens = text.split_whitespace().collect::<Vec<_>>();
    if prompt_tokens.first() == Some(&"/memory") && prompt_tokens.len() > 1 {
        if let Some(message) = crate::memory::command(Some(&cwd), &prompt_tokens[1..]) {
            state
                .lock()
                .await
                .history
                .push(crate::app::ChatMessage::new("system", message.clone()));
            return Ok(StopReason::EndTurn);
        }
    }
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
    let mut event_stream = AcpEventStream::new();
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
    )
    .await?;

    while matches!(
        context.lifecycle.stop_reason,
        Some(crate::network::lifecycle::StopReason::BackgroundPending)
    ) {
        if turn.cancel_token().is_cancelled() {
            return Ok(StopReason::Cancelled);
        }
        turn_tasks.observe_live_tasks(task_manager, &session_id);
        loop {
            if turn.cancel_token().is_cancelled() {
                return Ok(StopReason::Cancelled);
            }
            if terminal_overflow.load(Ordering::Acquire) {
                return Err(agent_client_protocol::Error::internal_error().data(
                    "ACP background task completion backlog overflowed; session must be reopened",
                ));
            }
            let event = loop {
                match try_receive_task_event(&task_events, &terminal_backlog) {
                    Ok(event) => break event,
                    Err(TryRecvError::Empty) => {
                        if let Some(event) = try_receive_terminal_backlog(&terminal_backlog) {
                            break event;
                        }
                        tokio::select! {
                            _ = turn.cancel_token().cancelled() => {
                                return Ok(StopReason::Cancelled);
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                        }
                    }
                    Err(TryRecvError::Disconnected) => {
                        return Err(agent_client_protocol::Error::internal_error()
                            .data("ACP background task subscription closed"));
                    }
                }
            };
            if turn.cancel_token().is_cancelled() {
                return Ok(StopReason::Cancelled);
            }
            turn_tasks.observe_event(&event, &known_task_ids);
            turn_tasks.observe_live_tasks(task_manager, &session_id);
            if turn_tasks.complete() {
                break;
            }
        }
        context = run_prompt_continuation(
            &client,
            &state,
            turn.cancel_token(),
            &policy,
            &stream_buffer,
            String::new(),
            events.clone(),
            &mut receiver,
            &connection,
            &session_id,
            &mut event_stream,
            context,
        )
        .await?;
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
}

fn try_receive_task_event(
    task_events: &Arc<std::sync::Mutex<Receiver<TaskEvent>>>,
    terminal_backlog: &Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
) -> Result<TaskEvent, TryRecvError> {
    let result = task_events
        .lock()
        .expect("ACP task event mutex poisoned")
        .try_recv();
    if let Ok(event) = &result
        && event.is_terminal()
    {
        terminal_backlog
            .lock()
            .expect("ACP terminal backlog mutex poisoned")
            .retain(|queued| queued.task_id() != event.task_id());
    }
    result
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

fn try_receive_terminal_backlog(
    terminal_backlog: &Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
) -> Option<TaskEvent> {
    terminal_backlog
        .lock()
        .expect("ACP terminal backlog mutex poisoned")
        .pop_front()
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
) -> Result<crate::network::TurnContext, agent_client_protocol::Error> {
    forward_prompt_run(
        crate::network::ui_adapter::run_agent_turn_with_events(
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
) -> Result<crate::network::TurnContext, agent_client_protocol::Error> {
    forward_prompt_run(
        crate::network::ui_adapter::run_agent_turn_with_events_and_context(
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
    )
    .await
}

async fn forward_prompt_run<F>(
    run: F,
    receiver: &mut crate::network::AgentUiEventReceiver,
    connection: &ConnectionTo<Client>,
    session_id: &str,
    event_stream: &mut AcpEventStream,
) -> Result<crate::network::TurnContext, agent_client_protocol::Error>
where
    F: std::future::Future<Output = crate::network::TurnContext>,
{
    let mut run = Box::pin(run);
    let context = loop {
        tokio::select! {
            context = &mut run => break context,
            event = receiver.recv() => {
                let Some(event) = event else { continue };
                send_updates(connection, session_id, event_stream.updates(event))?;
            }
        }
    };
    while let Ok(event) = receiver.try_recv() {
        send_updates(connection, session_id, event_stream.updates(event))?;
    }
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::{BackgroundTurnTasks, drain_task_events};
    use rustcode_tasks::TaskEvent;
    use std::collections::VecDeque;
    use std::sync::Arc;

    #[test]
    fn background_tracker_ignores_old_tasks_and_handles_cancellation() {
        let known_task_ids = Arc::new(std::sync::Mutex::new({
            let mut ids = super::super::session::KnownTaskIds::default();
            ids.insert("old-task".to_owned());
            ids
        }));
        let mut tracker = BackgroundTurnTasks {
            existing: ["old-task".to_owned()].into_iter().collect(),
            pending: Default::default(),
            terminal: Default::default(),
        };
        let old = TaskEvent::Finished {
            id: "old-task".into(),
            session_id: "session".into(),
            call_id: None,
            command: "sleep 1".to_owned(),
            output: Ok(rustcode_command::CommandOutput {
                success: true,
                exit_code: Some(0),
                stdout: Default::default(),
                stderr: Default::default(),
            }),
        };
        assert!(!tracker.observe_event(&old, &known_task_ids));
        assert!(
            !known_task_ids
                .lock()
                .unwrap()
                .snapshot()
                .any(|id| id == "old-task")
        );

        let started = TaskEvent::Started {
            id: "new-task".into(),
            session_id: "session".into(),
            call_id: None,
            pid: 42,
        };
        assert!(tracker.observe_event(&started, &known_task_ids));
        assert!(!tracker.complete());
        let cancelled = TaskEvent::Cancelled {
            id: "new-task".into(),
            session_id: "session".into(),
            call_id: None,
            command: "sleep 1".to_owned(),
        };
        assert!(tracker.observe_event(&cancelled, &known_task_ids));
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
                output: Ok(rustcode_command::CommandOutput {
                    success: true,
                    exit_code: Some(0),
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
        connection.send_notification(SessionNotification::new(session_id.to_owned(), update))?;
    }
    Ok(())
}
