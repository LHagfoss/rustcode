use crate::app::{AppState, ChatMessage, SubAgent, SubAgentStatus};
use futures_util::FutureExt;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Notify, Semaphore, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

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
pub(crate) struct SubagentCompletion {
    pub(crate) id: SubagentId,
    pub(crate) status: SubAgentStatus,
    pub(crate) output: String,
    pub(crate) truncated: bool,
}

struct ActiveChild {
    cancel_token: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

struct SupervisorState {
    active: HashMap<SubagentId, ActiveChild>,
    results: HashMap<SubagentId, SubagentCompletion>,
    result_order: VecDeque<SubagentId>,
}

struct SupervisorInner {
    semaphore: Arc<Semaphore>,
    state: StdMutex<SupervisorState>,
    activity: Notify,
    max_results: usize,
    max_result_bytes: usize,
}

#[derive(Clone)]
pub(crate) struct SubagentSupervisor {
    inner: Arc<SupervisorInner>,
}

impl SubagentSupervisor {
    pub(crate) fn new(concurrency_limit: usize) -> Self {
        Self::with_result_limits(concurrency_limit, 64, 8 * 1024)
    }

    pub(crate) fn with_result_limits(
        concurrency_limit: usize,
        max_results: usize,
        max_result_bytes: usize,
    ) -> Self {
        Self {
            inner: Arc::new(SupervisorInner {
                semaphore: Arc::new(Semaphore::new(concurrency_limit.max(1))),
                state: StdMutex::new(SupervisorState {
                    active: HashMap::new(),
                    results: HashMap::new(),
                    result_order: VecDeque::new(),
                }),
                activity: Notify::new(),
                max_results: max_results.max(1),
                max_result_bytes: max_result_bytes.max(1),
            }),
        }
    }

    pub(crate) fn spawn<F>(
        &self,
        id: SubagentId,
        parent_cancel: CancellationToken,
        child: F,
    ) -> Result<(), SubagentError>
    where
        F: Future<Output = Result<String, String>> + Send + 'static,
    {
        self.spawn_with_token_and_completion(id, parent_cancel, move |_| child, |_| async {})
    }

    pub(crate) fn spawn_with_token_and_completion<Factory, Child, Callback, CallbackFuture>(
        &self,
        id: SubagentId,
        parent_cancel: CancellationToken,
        child_factory: Factory,
        on_completion: Callback,
    ) -> Result<(), SubagentError>
    where
        Factory: FnOnce(CancellationToken) -> Child + Send + 'static,
        Child: Future<Output = Result<String, String>> + Send + 'static,
        Callback: FnOnce(SubagentCompletion) -> CallbackFuture + Send + 'static,
        CallbackFuture: Future<Output = ()> + Send + 'static,
    {
        let child_cancel = CancellationToken::new();
        let child = child_factory(child_cancel.clone());
        let (start_tx, start_rx) = oneshot::channel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.active.contains_key(&id) {
                return Err(SubagentError::AlreadyRunning(id));
            }
            state.results.remove(&id);
            state.result_order.retain(|stored_id| *stored_id != id);
            state.active.insert(
                id,
                ActiveChild {
                    cancel_token: child_cancel.clone(),
                    handle: None,
                },
            );
        }

        let inner = Arc::clone(&self.inner);
        let handle = tokio::spawn(async move {
            if start_rx.await.is_err() {
                return;
            }
            let run_inner = Arc::clone(&inner);
            let run = async move {
                let permit = tokio::select! {
                    permit = Arc::clone(&run_inner.semaphore).acquire_owned() => permit.ok(),
                    _ = child_cancel.cancelled() => None,
                    _ = parent_cancel.cancelled() => None,
                };
                let result = if let Some(_permit) = permit {
                    tokio::select! {
                        result = child => Some(result),
                        _ = child_cancel.cancelled() => None,
                        _ = parent_cancel.cancelled() => None,
                    }
                } else {
                    None
                };
                match result {
                    Some(Ok(output)) => SubagentCompletion {
                        id,
                        status: SubAgentStatus::Completed,
                        output,
                        truncated: false,
                    },
                    Some(Err(output))
                        if child_cancel.is_cancelled() || parent_cancel.is_cancelled() =>
                    {
                        SubagentCompletion {
                            id,
                            status: SubAgentStatus::Cancelled,
                            output,
                            truncated: false,
                        }
                    }
                    Some(Err(output)) => SubagentCompletion {
                        id,
                        status: SubAgentStatus::Failed,
                        output,
                        truncated: false,
                    },
                    None => SubagentCompletion {
                        id,
                        status: SubAgentStatus::Cancelled,
                        output: "error: cancelled".to_owned(),
                        truncated: false,
                    },
                }
            };
            let completion = match std::panic::AssertUnwindSafe(run).catch_unwind().await {
                Ok(completion) => completion,
                Err(_) => SubagentCompletion {
                    id,
                    status: SubAgentStatus::Failed,
                    output: "error: subagent task panicked".to_owned(),
                    truncated: false,
                },
            };
            let _ = std::panic::AssertUnwindSafe(on_completion(completion.clone()))
                .catch_unwind()
                .await;
            SubagentSupervisor { inner }.record_completion(completion);
        });

        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(active) = state.active.get_mut(&id) {
            active.handle = Some(handle);
        }
        let _ = start_tx.send(());
        Ok(())
    }

    fn record_completion(&self, mut completion: SubagentCompletion) {
        let id = completion.id;
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active.remove(&id);
        if state.results.contains_key(&id) {
            return;
        }
        if completion.output.len() > self.inner.max_result_bytes {
            let mut end = self.inner.max_result_bytes;
            while !completion.output.is_char_boundary(end) {
                end -= 1;
            }
            completion.output.truncate(end);
            completion.truncated = true;
        }
        state.results.insert(id, completion);
        state.result_order.push_back(id);
        while state.result_order.len() > self.inner.max_results {
            if let Some(expired) = state.result_order.pop_front() {
                state.results.remove(&expired);
            }
        }
        drop(state);
        self.inner.activity.notify_waiters();
    }

    pub(crate) fn is_active(&self, id: SubagentId) -> bool {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .contains_key(&id)
    }

    pub(crate) fn cancel(&self, id: SubagentId) -> Result<(), SubagentError> {
        let cancel_token = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active
            .get(&id)
            .map(|child| child.cancel_token.clone())
            .ok_or(SubagentError::MissingId(id))?;
        cancel_token.cancel();
        Ok(())
    }

    pub(crate) fn shutdown(&self) {
        let active = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut state.active)
        };
        for (id, child) in active {
            child.cancel_token.cancel();
            if let Some(handle) = child.handle {
                handle.abort();
            }
            self.record_completion(SubagentCompletion {
                id,
                status: SubAgentStatus::Cancelled,
                output: "error: cancelled by parent/session shutdown".to_owned(),
                truncated: false,
            });
        }
    }

    pub(crate) async fn wait(&self, id: SubagentId) -> Result<SubagentCompletion, SubagentError> {
        loop {
            let notified = self.inner.activity.notified();
            {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(result) = state.results.get(&id) {
                    return Ok(result.clone());
                }
                if !state.active.contains_key(&id) {
                    return Err(SubagentError::MissingId(id));
                }
            }
            notified.await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubagentError {
    MissingId(SubagentId),
    CannotSendToTerminal(SubagentId),
    AlreadyRunning(SubagentId),
}

impl fmt::Display for SubagentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingId(id) => write!(f, "no subagent with id {}", id.raw()),
            Self::CannotSendToTerminal(id) => {
                write!(f, "subagent {} is not available for follow-up", id.raw())
            }
            Self::AlreadyRunning(id) => write!(f, "subagent {} is already running", id.raw()),
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
        if agent.active_turn || agent.status == SubAgentStatus::Running {
            return Err(SubagentError::AlreadyRunning(id));
        }
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
    use super::{
        SubagentCompletion, SubagentController, SubagentError, SubagentId, SubagentSupervisor,
    };
    use crate::app::{AppState, ChatMessage, SubAgentStatus};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Notify, oneshot};
    use tokio_util::sync::CancellationToken;

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
    fn send_input_rejects_a_child_with_an_active_turn() {
        let mut state = AppState::new();
        let controller = SubagentController;
        let id = controller.spawn(
            &mut state,
            "still working",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );

        assert_eq!(
            controller.send_input(&mut state, id, "do this too"),
            Err(SubagentError::AlreadyRunning(id))
        );
        assert_eq!(state.subagents[0].history.len(), 1);
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

    #[tokio::test]
    async fn supervisor_spawn_returns_before_child_completion() {
        let supervisor = SubagentSupervisor::new(1);
        let release = Arc::new(Notify::new());
        let child_release = Arc::clone(&release);

        supervisor
            .spawn(
                SubagentId::from_raw(1),
                CancellationToken::new(),
                async move {
                    child_release.notified().await;
                    Ok("finished".to_owned())
                },
            )
            .unwrap();

        assert!(supervisor.is_active(SubagentId::from_raw(1)));
        release.notify_one();
        let result = supervisor.wait(SubagentId::from_raw(1)).await.unwrap();
        assert_eq!(result.output, "finished");
    }

    #[tokio::test(start_paused = true)]
    async fn supervisor_allows_two_children_to_overlap_but_queues_the_third() {
        let supervisor = SubagentSupervisor::new(2);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = CancellationToken::new();
        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel(3);

        for raw_id in 1..=3 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let release = release.clone();
            let started_tx = started_tx.clone();
            supervisor
                .spawn(
                    SubagentId::from_raw(raw_id),
                    CancellationToken::new(),
                    async move {
                        let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                        maximum.fetch_max(running, Ordering::SeqCst);
                        started_tx.send(()).await.unwrap();
                        release.cancelled().await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(format!("child-{raw_id}"))
                    },
                )
                .unwrap();
        }

        started_rx.recv().await.unwrap();
        started_rx.recv().await.unwrap();
        let third_started_early =
            tokio::time::timeout(std::time::Duration::from_millis(25), started_rx.recv())
                .await
                .is_ok();
        release.cancel();
        for raw_id in 1..=3 {
            supervisor.wait(SubagentId::from_raw(raw_id)).await.unwrap();
        }

        assert!(!third_started_early);
        assert_eq!(maximum.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn supervisor_cancellation_stops_an_in_flight_child() {
        struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let supervisor = SubagentSupervisor::new(1);
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let child_dropped = Arc::clone(&dropped);
        let (started_tx, started_rx) = oneshot::channel();
        let id = SubagentId::from_raw(1);
        supervisor
            .spawn(id, CancellationToken::new(), async move {
                let _drop_flag = DropFlag(child_dropped);
                let _ = started_tx.send(());
                std::future::pending::<Result<String, String>>().await
            })
            .unwrap();
        started_rx.await.unwrap();

        supervisor.cancel(id).unwrap();
        let result = supervisor.wait(id).await.unwrap();

        assert_eq!(result.status, SubAgentStatus::Cancelled);
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn parent_cancellation_propagates_to_an_in_flight_child() {
        let supervisor = SubagentSupervisor::new(1);
        let parent_cancel = CancellationToken::new();
        let (started_tx, started_rx) = oneshot::channel();
        let id = SubagentId::from_raw(1);
        supervisor
            .spawn(id, parent_cancel.clone(), async move {
                let _ = started_tx.send(());
                std::future::pending::<Result<String, String>>().await
            })
            .unwrap();
        started_rx.await.unwrap();

        parent_cancel.cancel();
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(25), supervisor.wait(id)).await;
        if waited.is_err() {
            supervisor.cancel(id).unwrap();
            let _ = supervisor.wait(id).await;
        }

        let result = waited
            .expect("parent cancellation must wake wait_agent")
            .unwrap();
        assert_eq!(result.status, SubAgentStatus::Cancelled);
    }

    #[tokio::test(start_paused = true)]
    async fn child_panic_becomes_a_failed_terminal_result_and_cleans_up() {
        let supervisor = SubagentSupervisor::new(1);
        let id = SubagentId::from_raw(1);
        supervisor
            .spawn(id, CancellationToken::new(), async move {
                panic!("child exploded");
                #[allow(unreachable_code)]
                Ok("unreachable".to_owned())
            })
            .unwrap();

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(25), supervisor.wait(id))
                .await
                .expect("panics must notify waiters")
                .unwrap();

        assert_eq!(result.status, SubAgentStatus::Failed);
        assert!(result.output.contains("panicked"));
        assert!(!supervisor.is_active(id));
    }

    #[tokio::test]
    async fn completion_delivery_is_bounded_and_idempotent() {
        let supervisor = SubagentSupervisor::with_result_limits(2, 2, 8);
        let first = SubagentId::from_raw(1);
        supervisor
            .spawn(first, CancellationToken::new(), async {
                Ok("abcdefghijkl".to_owned())
            })
            .unwrap();

        let delivered = supervisor.wait(first).await.unwrap();
        let replayed = supervisor.wait(first).await.unwrap();
        assert_eq!(delivered, replayed);
        assert_eq!(delivered.output, "abcdefgh");
        assert!(delivered.truncated);

        for raw_id in 2..=3 {
            let id = SubagentId::from_raw(raw_id);
            supervisor
                .spawn(id, CancellationToken::new(), async move {
                    Ok(format!("child-{raw_id}"))
                })
                .unwrap();
            supervisor.wait(id).await.unwrap();
        }

        assert_eq!(
            supervisor.wait(first).await,
            Err(SubagentError::MissingId(first))
        );
    }

    #[tokio::test]
    async fn duplicate_terminal_completion_cannot_replace_the_first_result() {
        let supervisor = SubagentSupervisor::new(1);
        let id = SubagentId::from_raw(1);
        supervisor.record_completion(SubagentCompletion {
            id,
            status: SubAgentStatus::Completed,
            output: "first".to_owned(),
            truncated: false,
        });
        supervisor.record_completion(SubagentCompletion {
            id,
            status: SubAgentStatus::Failed,
            output: "second".to_owned(),
            truncated: false,
        });

        let result = supervisor.wait(id).await.unwrap();
        assert_eq!(result.output, "first");
        assert_eq!(result.status, SubAgentStatus::Completed);
        assert_eq!(
            supervisor
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .results
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn supervisor_shutdown_cancels_running_and_queued_children() {
        let supervisor = SubagentSupervisor::new(1);
        let (started_tx, started_rx) = oneshot::channel();
        supervisor
            .spawn(
                SubagentId::from_raw(1),
                CancellationToken::new(),
                async move {
                    let _ = started_tx.send(());
                    std::future::pending::<Result<String, String>>().await
                },
            )
            .unwrap();
        supervisor
            .spawn(
                SubagentId::from_raw(2),
                CancellationToken::new(),
                std::future::pending::<Result<String, String>>(),
            )
            .unwrap();
        started_rx.await.unwrap();

        supervisor.shutdown();

        for raw_id in 1..=2 {
            let id = SubagentId::from_raw(raw_id);
            let result = supervisor.wait(id).await.unwrap();
            assert_eq!(result.status, SubAgentStatus::Cancelled);
            assert!(!supervisor.is_active(id));
        }
    }
}
