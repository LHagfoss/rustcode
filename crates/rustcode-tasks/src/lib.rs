//! Dependency-neutral background command task management.
//!
//! This crate owns the task lifecycle and event delivery, but deliberately
//! does not know about the application UI, sessions, configuration, or
//! network stack.  The process terminator is injected by the application so
//! platform-specific process-control policy remains at the integration
//! boundary.

use rustcode_command::{CommandOutput, CommandRequest, StartedCallback};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Instant;

/// Opaque identity for one background task.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(String);

impl TaskId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for TaskId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for TaskId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity for the logical session that owns a task.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Input needed to create a background task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub session_id: SessionId,
    pub command: String,
    pub request: CommandRequest,
}

impl TaskSpec {
    pub fn new(session_id: impl Into<SessionId>, request: CommandRequest) -> Self {
        Self {
            session_id: session_id.into(),
            command: request.command.clone(),
            request,
        }
    }
}

/// State of a task which has not reached a terminal state yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Starting,
    Running {
        pid: u32,
    },
    /// Cancellation was requested before the child PID was published.
    ///
    /// Once a PID is known, the manager performs the injected termination
    /// operation while holding the state lock and commits cancellation (or
    /// restores `Running` on failure), so there is no post-PID intermediate
    /// state that can race with completion.
    CancelRequested,
}

/// A point-in-time view of a live task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub session_id: SessionId,
    pub command: String,
    pub started_at: Instant,
    pub state: TaskState,
}

/// Events published by the manager.  Finished and cancelled are terminal;
/// the manager emits at most one of them for each task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskEvent {
    Started {
        id: TaskId,
        session_id: SessionId,
        pid: u32,
    },
    Finished {
        id: TaskId,
        session_id: SessionId,
        command: String,
        output: Result<CommandOutput, String>,
    },
    Cancelled {
        id: TaskId,
        session_id: SessionId,
        command: String,
    },
}

impl TaskEvent {
    pub fn task_id(&self) -> &TaskId {
        match self {
            Self::Started { id, .. } | Self::Finished { id, .. } | Self::Cancelled { id, .. } => id,
        }
    }

    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Started { session_id, .. }
            | Self::Finished { session_id, .. }
            | Self::Cancelled { session_id, .. } => session_id,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished { .. } | Self::Cancelled { .. })
    }
}

/// Application-provided process-tree termination operation.
///
/// The manager invokes this while holding its task-state lock.  Implementors
/// should therefore perform only the short, non-blocking signal/request step;
/// waiting for process exit belongs to the command runner.  Returning `true`
/// commits cancellation, while `false` leaves the task running so a natural
/// completion is still reported accurately.
pub trait ProcessTerminator: Send + Sync + 'static {
    fn terminate(&self, pid: u32) -> bool;
}

impl<F> ProcessTerminator for F
where
    F: Fn(u32) -> bool + Send + Sync + 'static,
{
    fn terminate(&self, pid: u32) -> bool {
        self(pid)
    }
}

/// A handle used to identify and inspect one task.
#[derive(Clone, Debug)]
pub struct TaskHandle {
    id: TaskId,
    session_id: SessionId,
}

impl TaskHandle {
    pub fn id(&self) -> &TaskId {
        &self.id
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

/// Result of requesting cancellation for one task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelResult {
    /// Cancellation is recorded and will be applied once a PID is known.
    Requested,
    /// Process termination succeeded and the terminal event was committed.
    Cancelled,
    /// The task was already removed after completing.
    AlreadyFinished,
    /// No task has this ID.
    NotFound,
    /// The process termination operation failed; the task remains running.
    Failed,
}

/// Aggregate result for cancelling all tasks in a session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CancelSummary {
    pub requested: usize,
    pub cancelled: usize,
    pub failed: usize,
    pub already_finished: usize,
}

/// A receiving end of the manager's event stream.
pub struct TaskSubscription {
    receiver: Receiver<TaskEvent>,
    alive: Arc<AtomicUsize>,
}

impl TaskSubscription {
    pub fn recv(&self) -> Result<TaskEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn try_recv(&self) -> Result<TaskEvent, TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for TaskSubscription {
    fn drop(&mut self) {
        self.alive.store(0, Ordering::Release);
    }
}

struct Subscriber {
    session_id: Option<SessionId>,
    sender: Sender<TaskEvent>,
    alive: Weak<AtomicUsize>,
}

struct TaskRecord {
    session_id: SessionId,
    command: String,
    started_at: Instant,
    state: TaskState,
}

struct Inner {
    tasks: Mutex<HashMap<TaskId, TaskRecord>>,
    subscribers: Mutex<Vec<Subscriber>>,
    terminal_ids: Mutex<TerminalIds>,
    next_id: AtomicU64,
}

const TERMINAL_ID_CAPACITY: usize = 1024;

/// Bounded tombstones let a cancellation racing with completion report
/// `AlreadyFinished` without retaining every task ID forever.
#[derive(Default)]
struct TerminalIds {
    ids: HashSet<TaskId>,
    order: VecDeque<TaskId>,
}

impl TerminalIds {
    fn remember(&mut self, id: TaskId) {
        if !self.ids.insert(id.clone()) {
            return;
        }
        self.order.push_back(id);
        while self.order.len() > TERMINAL_ID_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
    }

    fn contains(&self, id: &TaskId) -> bool {
        self.ids.contains(id)
    }
}

/// Thread-safe manager for background command tasks.
#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Inner>,
    terminator: Arc<dyn ProcessTerminator>,
}

impl TaskManager {
    pub fn new(terminator: Arc<dyn ProcessTerminator>) -> Self {
        Self {
            inner: Arc::new(Inner {
                tasks: Mutex::new(HashMap::new()),
                subscribers: Mutex::new(Vec::new()),
                terminal_ids: Mutex::new(TerminalIds::default()),
                next_id: AtomicU64::new(1),
            }),
            terminator,
        }
    }

    /// Subscribe to every task event.
    pub fn subscribe(&self) -> TaskSubscription {
        self.subscribe_inner(None)
    }

    /// Subscribe only to events belonging to one logical session.
    pub fn subscribe_session(&self, session_id: impl Into<SessionId>) -> TaskSubscription {
        self.subscribe_inner(Some(session_id.into()))
    }

    fn subscribe_inner(&self, session_id: Option<SessionId>) -> TaskSubscription {
        let (sender, receiver) = mpsc::channel();
        let alive = Arc::new(AtomicUsize::new(1));
        self.inner
            .subscribers
            .lock()
            .expect("task subscribers mutex poisoned")
            .push(Subscriber {
                session_id,
                sender,
                alive: Arc::downgrade(&alive),
            });
        TaskSubscription { receiver, alive }
    }

    /// Start a command on a detached worker thread.
    pub fn spawn(&self, spec: TaskSpec) -> Result<TaskHandle, String> {
        let id = self.allocate_id();
        self.spawn_with_task_id(id, spec)
    }

    /// Start a command with an integration-owned ID.
    ///
    /// This is useful while an application migrates from an existing task
    /// registry and must preserve IDs already visible in transcripts or CLI
    /// output. IDs must be unique among live and recently terminal tasks.
    pub fn spawn_with_id(
        &self,
        id: impl Into<TaskId>,
        spec: TaskSpec,
    ) -> Result<TaskHandle, String> {
        self.spawn_with_task_id(id.into(), spec)
    }

    fn spawn_with_task_id(&self, id: TaskId, mut spec: TaskSpec) -> Result<TaskHandle, String> {
        // Background tasks must be process-group capable for the injected
        // terminator to stop descendants as well as the shell.
        spec.request.process_group = true;
        let handle = TaskHandle {
            id: id.clone(),
            session_id: spec.session_id.clone(),
        };
        if !self.insert(id.clone(), &spec) {
            return Err(format!("task ID '{id}' is already in use"));
        }

        let manager = self.clone();
        let thread_name = format!("rustcode-task-{}", id);
        let spawn_result = thread::Builder::new().name(thread_name).spawn(move || {
            let started_manager = manager.clone();
            let started_id = id.clone();
            let started: StartedCallback = Arc::new(move |pid| {
                started_manager.child_started(&started_id, pid);
            });
            let result = catch_unwind(AssertUnwindSafe(|| {
                rustcode_command::run_until_exit(&spec.request, None, Some(started))
            }));
            match result {
                Ok(output) => manager.finish(&id, output),
                Err(_) => {
                    manager.finish(&id, Err("background command runner panicked".to_string()))
                }
            }
        });

        if let Err(error) = spawn_result {
            self.finish(
                &handle.id,
                Err(format!("failed to start task thread: {error}")),
            );
            return Err(format!("failed to start task thread: {error}"));
        }
        Ok(handle)
    }

    /// Return all live tasks for a session, ordered by creation time.
    pub fn list(&self, session_id: impl AsRef<str>) -> Vec<TaskSnapshot> {
        let session_id = session_id.as_ref();
        let mut tasks: Vec<_> = self
            .inner
            .tasks
            .lock()
            .expect("task state mutex poisoned")
            .iter()
            .filter(|(_, task)| task.session_id.as_str() == session_id)
            .map(|(id, task)| TaskSnapshot {
                id: id.clone(),
                session_id: task.session_id.clone(),
                command: task.command.clone(),
                started_at: task.started_at,
                state: task.state,
            })
            .collect();
        tasks.sort_by_key(|task| task.started_at);
        tasks
    }

    pub fn has_running(&self, session_id: impl AsRef<str>) -> bool {
        !self.list(session_id).is_empty()
    }

    /// Request cancellation of one task.
    pub fn cancel(&self, id: impl AsRef<str>) -> CancelResult {
        let id = TaskId::new(id.as_ref());
        let mut event = None;
        let result = {
            let mut tasks = self.inner.tasks.lock().expect("task state mutex poisoned");
            let Some(task) = tasks.get_mut(&id) else {
                return if self
                    .inner
                    .terminal_ids
                    .lock()
                    .expect("terminal task IDs mutex poisoned")
                    .contains(&id)
                {
                    CancelResult::AlreadyFinished
                } else {
                    CancelResult::NotFound
                };
            };
            match task.state {
                TaskState::Starting => {
                    task.state = TaskState::CancelRequested;
                    CancelResult::Requested
                }
                TaskState::Running { pid } => {
                    let terminated =
                        catch_unwind(AssertUnwindSafe(|| self.terminator.terminate(pid)))
                            .unwrap_or(false);
                    if terminated {
                        let task = tasks.remove(&id).expect("task was just observed");
                        self.remember_terminal(&id);
                        event = Some(TaskEvent::Cancelled {
                            id: id.clone(),
                            session_id: task.session_id,
                            command: task.command,
                        });
                        CancelResult::Cancelled
                    } else {
                        CancelResult::Failed
                    }
                }
                TaskState::CancelRequested => CancelResult::Requested,
            }
        };
        if let Some(event) = event {
            self.publish(event);
        }
        result
    }

    /// Request cancellation for all live tasks belonging to one session.
    pub fn cancel_session(&self, session_id: impl AsRef<str>) -> CancelSummary {
        let ids: Vec<_> = self
            .list(session_id)
            .into_iter()
            .map(|task| task.id)
            .collect();
        let mut summary = CancelSummary::default();
        for id in ids {
            match self.cancel(&id) {
                CancelResult::Requested => summary.requested += 1,
                CancelResult::Cancelled => summary.cancelled += 1,
                CancelResult::Failed => summary.failed += 1,
                CancelResult::AlreadyFinished => summary.already_finished += 1,
                CancelResult::NotFound => {}
            }
        }
        summary
    }

    fn allocate_id(&self) -> TaskId {
        let sequence = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        TaskId::new(format!("task_{sequence}"))
    }

    fn insert(&self, id: TaskId, spec: &TaskSpec) -> bool {
        let mut tasks = self.inner.tasks.lock().expect("task state mutex poisoned");
        if tasks.contains_key(&id) {
            return false;
        }
        if self
            .inner
            .terminal_ids
            .lock()
            .expect("terminal task IDs mutex poisoned")
            .contains(&id)
        {
            return false;
        }
        tasks.insert(
            id,
            TaskRecord {
                session_id: spec.session_id.clone(),
                command: spec.command.clone(),
                started_at: Instant::now(),
                state: TaskState::Starting,
            },
        );
        true
    }

    fn child_started(&self, id: &TaskId, pid: u32) {
        let mut started_event = None;
        let mut cancelled_event = None;
        {
            let mut tasks = self.inner.tasks.lock().expect("task state mutex poisoned");
            let Some(task) = tasks.get_mut(id) else {
                // A cancellation may have committed before the runner's
                // callback was delivered.  Its terminal event already won.
                return;
            };
            match task.state {
                TaskState::Starting => {
                    task.state = TaskState::Running { pid };
                    started_event = Some(TaskEvent::Started {
                        id: id.clone(),
                        session_id: task.session_id.clone(),
                        pid,
                    });
                }
                TaskState::CancelRequested => {
                    let terminated =
                        catch_unwind(AssertUnwindSafe(|| self.terminator.terminate(pid)))
                            .unwrap_or(false);
                    if terminated {
                        let task = tasks.remove(id).expect("task was just observed");
                        self.remember_terminal(id);
                        cancelled_event = Some(TaskEvent::Cancelled {
                            id: id.clone(),
                            session_id: task.session_id,
                            command: task.command,
                        });
                    } else {
                        task.state = TaskState::Running { pid };
                        started_event = Some(TaskEvent::Started {
                            id: id.clone(),
                            session_id: task.session_id.clone(),
                            pid,
                        });
                    }
                }
                TaskState::Running { .. } => {
                    // A runner must publish its PID only once.  Keeping the
                    // first state is safer than emitting duplicate Started.
                }
            }
        }
        if let Some(event) = started_event {
            self.publish(event);
        }
        if let Some(event) = cancelled_event {
            self.publish(event);
        }
    }

    fn finish(&self, id: &TaskId, output: Result<CommandOutput, String>) {
        let event = {
            let mut tasks = self.inner.tasks.lock().expect("task state mutex poisoned");
            let Some(task) = tasks.remove(id) else {
                // Cancellation already committed the terminal transition.
                return;
            };
            self.remember_terminal(id);
            if matches!(task.state, TaskState::CancelRequested) {
                TaskEvent::Cancelled {
                    id: id.clone(),
                    session_id: task.session_id,
                    command: task.command,
                }
            } else {
                TaskEvent::Finished {
                    id: id.clone(),
                    session_id: task.session_id,
                    command: task.command,
                    output,
                }
            }
        };
        self.publish(event);
    }

    fn publish(&self, event: TaskEvent) {
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .expect("task subscribers mutex poisoned");
        subscribers.retain(|subscriber| {
            let Some(alive) = subscriber.alive.upgrade() else {
                return false;
            };
            if alive.load(Ordering::Acquire) == 0 {
                return false;
            }
            if subscriber
                .session_id
                .as_ref()
                .is_some_and(|session_id| session_id != event.session_id())
            {
                return true;
            }
            subscriber.sender.send(event.clone()).is_ok()
        });
    }

    fn remember_terminal(&self, id: &TaskId) {
        self.inner
            .terminal_ids
            .lock()
            .expect("terminal task IDs mutex poisoned")
            .remember(id.clone());
    }

    #[cfg(test)]
    fn register_for_test(&self, session_id: &str, command: &str) -> TaskId {
        let id = self.allocate_id();
        self.insert(
            id.clone(),
            &TaskSpec {
                session_id: SessionId::new(session_id),
                command: command.to_owned(),
                request: test_request(command),
            },
        );
        id
    }

    #[cfg(test)]
    fn mark_started_for_test(&self, id: &TaskId, pid: u32) {
        self.child_started(id, pid);
    }

    #[cfg(test)]
    fn finish_for_test(&self, id: &TaskId, output: Result<CommandOutput, String>) {
        self.finish(id, output);
    }

    #[cfg(test)]
    fn subscriber_count_for_test(&self) -> usize {
        self.inner
            .subscribers
            .lock()
            .expect("task subscribers mutex poisoned")
            .len()
    }
}

#[cfg(test)]
fn test_request(command: &str) -> CommandRequest {
    CommandRequest {
        command: command.to_owned(),
        cwd: None,
        env: Vec::new(),
        timeout: std::time::Duration::from_secs(5),
        process_group: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::TryRecvError;
    use std::time::Duration;

    #[derive(Default)]
    struct FakeTerminator {
        calls: Mutex<Vec<u32>>,
        result: AtomicBool,
    }

    impl FakeTerminator {
        fn succeeding() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                result: AtomicBool::new(true),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                result: AtomicBool::new(false),
            })
        }

        fn called_pids(&self) -> Vec<u32> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ProcessTerminator for FakeTerminator {
        fn terminate(&self, pid: u32) -> bool {
            self.calls.lock().unwrap().push(pid);
            self.result.load(Ordering::Acquire)
        }
    }

    fn manager(terminator: Arc<FakeTerminator>) -> TaskManager {
        TaskManager::new(terminator)
    }

    fn successful_output() -> CommandOutput {
        CommandOutput {
            success: true,
            exit_code: Some(0),
            stdout: Default::default(),
            stderr: Default::default(),
        }
    }

    #[test]
    fn typed_ids_are_displayable_without_stringly_state_api() {
        let task = TaskId::from("task-7");
        let session = SessionId::from("session-a");
        assert_eq!(task.as_str(), "task-7");
        assert_eq!(session.as_ref(), "session-a");
        assert_eq!(task.to_string(), "task-7");
        assert_eq!(session.to_string(), "session-a");
    }

    #[test]
    fn session_subscriptions_receive_only_their_scope() {
        let terminator = FakeTerminator::succeeding();
        let manager = manager(terminator);
        let session_a = manager.subscribe_session("a");
        let all = manager.subscribe();
        let first = manager.register_for_test("a", "one");
        let second = manager.register_for_test("b", "two");
        manager.mark_started_for_test(&first, 101);
        manager.mark_started_for_test(&second, 102);
        assert_eq!(session_a.recv().unwrap().task_id(), &first);
        assert_eq!(session_a.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(all.recv().unwrap().task_id(), &first);
        assert_eq!(all.recv().unwrap().task_id(), &second);
    }

    #[test]
    fn dropped_subscribers_are_pruned_on_the_next_event() {
        let manager = manager(FakeTerminator::succeeding());
        let subscription = manager.subscribe();
        assert_eq!(manager.subscriber_count_for_test(), 1);
        drop(subscription);
        let id = manager.register_for_test("a", "one");
        manager.mark_started_for_test(&id, 101);
        assert_eq!(manager.subscriber_count_for_test(), 0);
    }

    #[test]
    fn list_and_running_status_are_session_scoped() {
        let manager = manager(FakeTerminator::succeeding());
        let a = manager.register_for_test("a", "one");
        let _b = manager.register_for_test("b", "two");
        assert_eq!(manager.list("a").len(), 1);
        assert_eq!(manager.list("a")[0].id, a);
        assert!(manager.has_running("a"));
        assert!(!manager.has_running("missing"));
    }

    #[test]
    fn cancellation_before_pid_is_applied_when_child_starts() {
        let terminator = FakeTerminator::succeeding();
        let manager = manager(terminator.clone());
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "sleep");
        assert_eq!(manager.cancel(&id), CancelResult::Requested);
        assert_eq!(manager.list("a")[0].state, TaskState::CancelRequested);
        manager.mark_started_for_test(&id, 42);
        assert_eq!(terminator.called_pids(), vec![42]);
        assert!(
            matches!(events.recv().unwrap(), TaskEvent::Cancelled { id: event_id, .. } if event_id == id)
        );
        assert!(manager.list("a").is_empty());
        assert_eq!(manager.cancel(&id), CancelResult::AlreadyFinished);
    }

    #[test]
    fn cancellation_after_pid_commits_before_runner_finishes() {
        let terminator = FakeTerminator::succeeding();
        let manager = manager(terminator.clone());
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "sleep");
        manager.mark_started_for_test(&id, 43);
        assert!(matches!(events.recv().unwrap(), TaskEvent::Started { .. }));
        assert_eq!(manager.cancel(&id), CancelResult::Cancelled);
        assert!(
            matches!(events.recv().unwrap(), TaskEvent::Cancelled { id: event_id, .. } if event_id == id)
        );
        manager.finish_for_test(&id, Ok(successful_output()));
        assert_eq!(terminator.called_pids(), vec![43]);
        assert_eq!(manager.cancel(&id), CancelResult::AlreadyFinished);
    }

    #[test]
    fn failed_termination_restores_running_and_preserves_finish() {
        let terminator = FakeTerminator::failing();
        let manager = manager(terminator.clone());
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "sleep");
        manager.mark_started_for_test(&id, 44);
        assert!(matches!(events.recv().unwrap(), TaskEvent::Started { .. }));
        assert_eq!(manager.cancel(&id), CancelResult::Failed);
        assert_eq!(manager.list("a")[0].state, TaskState::Running { pid: 44 });
        manager.finish_for_test(&id, Ok(successful_output()));
        assert!(
            matches!(events.recv().unwrap(), TaskEvent::Finished { id: event_id, .. } if event_id == id)
        );
        assert_eq!(terminator.called_pids(), vec![44]);
    }

    #[test]
    fn finish_is_idempotent_and_has_exactly_one_terminal_event() {
        let manager = manager(FakeTerminator::succeeding());
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "true");
        manager.finish_for_test(&id, Ok(successful_output()));
        manager.finish_for_test(&id, Ok(successful_output()));
        assert!(matches!(events.recv().unwrap(), TaskEvent::Finished { .. }));
        assert_eq!(events.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(manager.cancel(&id), CancelResult::AlreadyFinished);
    }

    #[test]
    fn terminal_tombstones_are_bounded() {
        let manager = manager(FakeTerminator::succeeding());
        let mut ids = Vec::with_capacity(TERMINAL_ID_CAPACITY + 1);
        for index in 0..=TERMINAL_ID_CAPACITY {
            let id = manager.register_for_test("a", &format!("command-{index}"));
            manager.finish_for_test(&id, Ok(successful_output()));
            ids.push(id);
        }
        assert_eq!(manager.cancel(&ids[0]), CancelResult::NotFound);
        assert_eq!(
            manager.cancel(&ids[TERMINAL_ID_CAPACITY]),
            CancelResult::AlreadyFinished
        );
    }

    #[test]
    fn multiple_subscribers_each_receive_the_same_terminal_event() {
        let manager = manager(FakeTerminator::succeeding());
        let first = manager.subscribe();
        let second = manager.subscribe();
        let id = manager.register_for_test("a", "true");
        manager.finish_for_test(&id, Ok(successful_output()));
        assert_eq!(first.recv().unwrap().task_id(), &id);
        assert_eq!(second.recv().unwrap().task_id(), &id);
    }

    #[test]
    fn cancelling_a_session_does_not_touch_other_sessions() {
        let terminator = FakeTerminator::succeeding();
        let manager = manager(terminator.clone());
        let events = manager.subscribe();
        let a = manager.register_for_test("a", "one");
        let b = manager.register_for_test("b", "two");
        manager.mark_started_for_test(&a, 51);
        manager.mark_started_for_test(&b, 52);
        let _ = events.recv();
        let _ = events.recv();
        let summary = manager.cancel_session("a");
        assert_eq!(summary.cancelled, 1);
        assert_eq!(summary.failed, 0);
        assert!(manager.list("a").is_empty());
        assert_eq!(manager.list("b").len(), 1);
        assert_eq!(terminator.called_pids(), vec![51]);
        assert!(matches!(events.recv().unwrap(), TaskEvent::Cancelled { id, .. } if id == a));
        manager.finish_for_test(&b, Ok(successful_output()));
        assert!(matches!(events.recv().unwrap(), TaskEvent::Finished { id, .. } if id == b));
    }

    #[test]
    fn spawn_executes_command_and_publishes_started_then_finished() {
        let manager = manager(FakeTerminator::succeeding());
        let events = manager.subscribe();
        let request = test_request(if cfg!(target_os = "windows") {
            "echo ok"
        } else {
            "printf ok"
        });
        let handle = manager.spawn(TaskSpec::new("a", request)).unwrap();
        let started = events.recv().unwrap();
        assert!(matches!(started, TaskEvent::Started { ref id, .. } if id == handle.id()));
        let finished = events.recv().unwrap();
        assert!(
            matches!(finished, TaskEvent::Finished { ref id, output: Ok(ref output), .. } if id == handle.id() && output.success)
        );
        assert!(manager.list("a").is_empty());
    }

    #[test]
    fn cancelled_task_suppresses_late_runner_completion() {
        let terminator = FakeTerminator::succeeding();
        let manager = manager(terminator);
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "long");
        manager.mark_started_for_test(&id, 61);
        let _ = events.recv();
        assert_eq!(manager.cancel(&id), CancelResult::Cancelled);
        manager.finish_for_test(&id, Err("late failure".to_owned()));
        assert!(matches!(
            events.recv().unwrap(),
            TaskEvent::Cancelled { .. }
        ));
        assert_eq!(events.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn cancellation_requested_twice_is_not_a_second_transition() {
        let manager = manager(FakeTerminator::succeeding());
        let id = manager.register_for_test("a", "long");
        assert_eq!(manager.cancel(&id), CancelResult::Requested);
        assert_eq!(manager.cancel(&id), CancelResult::Requested);
        assert_eq!(manager.list("a")[0].state, TaskState::CancelRequested);
    }

    #[test]
    fn event_helpers_identify_scope_and_terminal_state() {
        let id = TaskId::from("id");
        let session = SessionId::from("session");
        let event = TaskEvent::Cancelled {
            id: id.clone(),
            session_id: session.clone(),
            command: String::new(),
        };
        assert_eq!(event.task_id(), &id);
        assert_eq!(event.session_id(), &session);
        assert!(event.is_terminal());
    }

    #[test]
    fn task_spec_copies_original_command_for_display() {
        let request = test_request("printf hello");
        let spec = TaskSpec::new("a", request);
        assert_eq!(spec.command, "printf hello");
        assert!(spec.request.process_group);
    }

    #[test]
    fn cancelled_before_pid_with_failed_termination_can_finish_normally() {
        let terminator = FakeTerminator::failing();
        let manager = manager(terminator);
        let events = manager.subscribe();
        let id = manager.register_for_test("a", "long");
        assert_eq!(manager.cancel(&id), CancelResult::Requested);
        manager.mark_started_for_test(&id, 71);
        assert!(matches!(events.recv().unwrap(), TaskEvent::Started { .. }));
        assert_eq!(manager.list("a")[0].state, TaskState::Running { pid: 71 });
        manager.finish_for_test(&id, Ok(successful_output()));
        assert!(matches!(events.recv().unwrap(), TaskEvent::Finished { .. }));
    }

    #[test]
    fn subscribe_session_accepts_typed_session_id() {
        let manager = manager(FakeTerminator::succeeding());
        let _subscription = manager.subscribe_session(SessionId::new("typed"));
        assert_eq!(manager.subscriber_count_for_test(), 1);
    }

    #[test]
    fn task_ids_are_unique() {
        let manager = manager(FakeTerminator::succeeding());
        let first = manager.register_for_test("a", "one");
        let second = manager.register_for_test("a", "two");
        assert_ne!(first, second);
    }

    #[test]
    fn terminal_events_are_scoped_even_when_all_subscriber_is_present() {
        let manager = manager(FakeTerminator::succeeding());
        let session = manager.subscribe_session("a");
        let other = manager.subscribe_session("b");
        let id = manager.register_for_test("a", "one");
        manager.finish_for_test(&id, Ok(successful_output()));
        assert!(matches!(
            session.recv().unwrap(),
            TaskEvent::Finished { .. }
        ));
        assert_eq!(other.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn a_subscription_remains_usable_after_unrelated_event() {
        let manager = manager(FakeTerminator::succeeding());
        let session = manager.subscribe_session("a");
        let first = manager.register_for_test("b", "other");
        manager.finish_for_test(&first, Ok(successful_output()));
        assert_eq!(session.try_recv(), Err(TryRecvError::Empty));
        let second = manager.register_for_test("a", "wanted");
        manager.finish_for_test(&second, Ok(successful_output()));
        assert!(matches!(session.recv().unwrap(), TaskEvent::Finished { id, .. } if id == second));
    }

    #[test]
    fn spawn_failure_is_reported_as_a_terminal_finish() {
        // This exercises only the normal command path on all platforms.  A
        // command that cannot be found is still a shell completion with a
        // failed status, not a manager-level thread failure.
        let manager = manager(FakeTerminator::succeeding());
        let events = manager.subscribe();
        let request = test_request(if cfg!(target_os = "windows") {
            "exit /b 7"
        } else {
            "exit 7"
        });
        let handle = manager.spawn(TaskSpec::new("a", request)).unwrap();
        assert!(
            matches!(events.recv().unwrap(), TaskEvent::Started { id, .. } if id == *handle.id())
        );
        match events.recv().unwrap() {
            TaskEvent::Finished {
                id,
                output: Ok(output),
                ..
            } => {
                assert_eq!(id, *handle.id());
                assert!(!output.success);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn snapshots_retain_creation_time_and_state() {
        let manager = manager(FakeTerminator::succeeding());
        let id = manager.register_for_test("a", "one");
        let snapshot = manager.list("a").pop().unwrap();
        assert_eq!(snapshot.id, id);
        assert_eq!(snapshot.state, TaskState::Starting);
        assert!(snapshot.started_at.elapsed() < Duration::from_secs(1));
    }
}
