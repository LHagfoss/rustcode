//! One event-driven owner; durable state is re-read after every wakeup.
use super::{
    DaemonError, Result,
    executor::{JobExecutor, JobRunContext, RunOutcome},
    model::{JobRunRecord, JobRunState, RunSettlement},
    store::JobStore,
};
use chrono::{DateTime, Duration, Utc};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{sync::Notify, task::JoinSet};
use tokio_util::sync::CancellationToken;

pub trait SchedulerClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
pub struct SystemClock;
impl SchedulerClock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

pub type SchedulerHandle = Arc<Scheduler>;
pub struct Scheduler {
    store: Arc<Mutex<JobStore>>,
    clock: Arc<dyn SchedulerClock>,
    executor: Arc<dyn JobExecutor>,
    owner: String,
    limit: usize,
    changed: Notify,
    cancellation: CancellationToken,
    started: AtomicBool,
    status: Mutex<(usize, Option<DateTime<Utc>>)>,
}

impl Scheduler {
    pub fn new(
        store: Arc<Mutex<JobStore>>,
        clock: Arc<dyn SchedulerClock>,
        executor: Arc<dyn JobExecutor>,
        owner: impl Into<String>,
        limit: usize,
    ) -> SchedulerHandle {
        Arc::new(Self {
            store,
            clock,
            executor,
            owner: owner.into(),
            limit: limit.max(1),
            changed: Notify::new(),
            cancellation: CancellationToken::new(),
            started: AtomicBool::new(false),
            status: Mutex::new((0, None)),
        })
    }
    pub fn store(&self) -> Arc<Mutex<JobStore>> {
        self.store.clone()
    }
    pub fn notify_changed(&self) {
        self.changed.notify_one();
    }
    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }
    pub fn snapshot(&self) -> (usize, Option<DateTime<Utc>>) {
        *self.status.lock().unwrap()
    }
    pub fn run_now(&self, id: &str) -> Result<()> {
        self.store.lock().unwrap().run_now(id, self.clock.now())?;
        self.notify_changed();
        Ok(())
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        if self.started.swap(true, Ordering::SeqCst) {
            return Err(DaemonError::Conflict("scheduler already started".into()));
        }
        let mut tasks = JoinSet::new();
        let mut sessions = HashSet::new();
        let mut active_jobs = HashSet::new();
        let result = self
            .drive(&mut tasks, &mut sessions, &mut active_jobs)
            .await;
        self.shutdown();
        // Every dispatched task settles itself; drain even on a loop/storage error.
        let mut result = result;
        while let Some(done) = tasks.join_next().await {
            match done {
                Ok((_, _, Err(error))) => {
                    if result.is_ok() {
                        result = Err(error);
                    }
                }
                Err(error) => {
                    if result.is_ok() {
                        result = Err(DaemonError::Storage(error.to_string()));
                    }
                }
                _ => {}
            }
        }
        *self.status.lock().unwrap() = (0, None);
        result
    }

    async fn drive(
        self: &Arc<Self>,
        tasks: &mut JoinSet<(String, Option<String>, Result<()>)>,
        sessions: &mut HashSet<String>,
        active_jobs: &mut HashSet<String>,
    ) -> Result<()> {
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
            let now = self.clock.now();
            let deadline = {
                let mut store = self.store.lock().unwrap();
                let mut reserved = sessions.clone();
                let claims = store.claim_due_matching(
                    now,
                    &self.owner,
                    Duration::hours(1),
                    self.limit - tasks.len(),
                    |job| {
                        if active_jobs.contains(&job.id) {
                            return false;
                        }
                        if let Some(session) = &job.target_session {
                            return reserved.insert(session.clone());
                        }
                        true
                    },
                )?;
                // Rebuild reservations from actually claimed jobs only.
                for mut run in claims {
                    let job = store.get(&run.job_id)?;
                    store.settle_run(
                        &run.id,
                        &self.owner,
                        run.lease_fence,
                        RunSettlement {
                            state: JobRunState::Running,
                            finished_at: now,
                            result_summary: None,
                            error_class: None,
                            output: None,
                        },
                    )?;
                    active_jobs.insert(job.id.clone());
                    if let Some(session) = &job.target_session {
                        sessions.insert(session.clone());
                    }
                    run.state = JobRunState::Running;
                    run.started_at = Some(now);
                    let this = self.clone();
                    tasks.spawn(async move {
                        let id = job.id.clone();
                        let session = job.target_session.clone();
                        let cancellation = this.cancellation.child_token();
                        let context = JobRunContext { job, run: run.clone(), cancellation: cancellation.clone() };
                        // Catch executor panics as unknown effects, and bound an
                        // executor that ignores cancellation. Never retry either.
                        let executor = this.executor.clone();
                        let mut execution = tokio::spawn(async move { executor.execute(context).await });
                        let outcome = tokio::select! {
                            result = &mut execution => result.unwrap_or_else(|e| RunOutcome::Ambiguous { error: e.to_string(), output: None }),
                            _ = cancellation.cancelled() => {
                                match tokio::time::timeout(std::time::Duration::from_secs(5), &mut execution).await {
                                    Ok(result) => result.unwrap_or_else(|e| RunOutcome::Ambiguous { error: e.to_string(), output: None }),
                                    Err(_) => { execution.abort(); let _ = execution.await; RunOutcome::Ambiguous { error: "cancellation deadline exceeded".into(), output: None } }
                                }
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_secs(1800)) => {
                                cancellation.cancel(); execution.abort(); let _ = execution.await;
                                RunOutcome::Ambiguous { error: "execution deadline exceeded".into(), output: None }
                            }
                        };
                        let result = this.settle(&run, outcome);
                        (id, session, result)
                    });
                }
                store.next_wake(now, active_jobs, sessions, tasks.len() < self.limit)?
            };
            *self.status.lock().unwrap() = (tasks.len(), deadline);
            let wake = async {
                if let Some(deadline) = deadline {
                    let delay = (deadline - self.clock.now()).to_std().unwrap_or_default();
                    tokio::time::sleep_until(tokio::time::Instant::now() + delay).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Ok(()),
                done = tasks.join_next(), if !tasks.is_empty() => {
                    let (id, session, result) = done.unwrap().map_err(|e| DaemonError::Storage(e.to_string()))?;
                    active_jobs.remove(&id);
                    if let Some(session) = session { sessions.remove(&session); }
                    result?;
                }
                _ = self.changed.notified() => {},
                _ = wake => {},
            }
        }
    }

    fn settle(&self, run: &JobRunRecord, outcome: RunOutcome) -> Result<()> {
        let now = self.clock.now();
        let (state, summary, error, output) = match outcome {
            RunOutcome::Succeeded { summary, output } => {
                (JobRunState::Succeeded, Some(summary), None, output)
            }
            RunOutcome::Transient { error, output } => {
                return self.store.lock().unwrap().retry_run(
                    &run.id,
                    &self.owner,
                    run.lease_fence,
                    now,
                    error,
                    output,
                );
            }
            RunOutcome::Permanent { error, output } => (
                JobRunState::Failed,
                Some(error),
                Some("permanent".into()),
                output,
            ),
            RunOutcome::Cancelled { output } => (
                JobRunState::Cancelled,
                None,
                Some("cancelled".into()),
                output,
            ),
            RunOutcome::Ambiguous { error, output } => (
                JobRunState::Ambiguous,
                Some(error),
                Some("ambiguous".into()),
                output,
            ),
        };
        let result = self.store.lock().unwrap().settle_run(
            &run.id,
            &self.owner,
            run.lease_fence,
            RunSettlement {
                state,
                finished_at: now,
                result_summary: summary,
                error_class: error,
                output,
            },
        );
        // Deletion explicitly invalidates the lease and removes history.
        if matches!(result, Err(DaemonError::Conflict(_)))
            && self
                .store
                .lock()
                .unwrap()
                .get(&run.job_id)
                .is_err_and(|e| matches!(e, DaemonError::NotFound(_)))
        {
            return Ok(());
        }
        result
    }
}

#[cfg(unix)]
impl super::server::SchedulerControl for Scheduler {
    fn notify_changed(&self) {
        Scheduler::notify_changed(self);
    }
    fn run_now(&self, id: &str) -> Result<super::protocol::DaemonResponse> {
        Scheduler::run_now(self, id)?;
        Ok(super::protocol::DaemonResponse::Ack)
    }
    fn snapshot(&self) -> (usize, Option<String>) {
        let (active, next) = Scheduler::snapshot(self);
        (active, next.map(|v| v.to_rfc3339()))
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::model::{JobAction, JobRecord, MisfirePolicy, RetryPolicy, ScheduleSpec};
    use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    use super::{Scheduler, SchedulerClock};
    use crate::daemon::executor::{JobExecutor, JobRunContext, RunOutcome};
    use crate::daemon::store::JobStore;

    #[derive(Clone)]
    struct TestClock(Arc<Mutex<DateTime<Utc>>>);

    impl TestClock {
        fn new(now: DateTime<Utc>) -> Self {
            Self(Arc::new(Mutex::new(now)))
        }
        fn set(&self, now: DateTime<Utc>) {
            *self.0.lock().unwrap() = now;
        }
    }

    impl SchedulerClock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }

    #[derive(Clone, Default)]
    struct RecordingExecutor {
        calls: Arc<Mutex<Vec<JobRunContext>>>,
        gate: Arc<tokio::sync::Notify>,
        block: bool,
    }

    impl JobExecutor for RecordingExecutor {
        fn execute(&self, context: JobRunContext) -> crate::daemon::executor::ExecutionFuture {
            let calls = self.calls.clone();
            let gate = self.gate.clone();
            let block = self.block;
            Box::pin(async move {
                calls.lock().unwrap().push(context.clone());
                if block {
                    gate.notified().await;
                }
                if context.cancellation.is_cancelled() {
                    RunOutcome::Cancelled { output: None }
                } else {
                    RunOutcome::Succeeded {
                        summary: "ok".into(),
                        output: None,
                    }
                }
            })
        }
    }

    fn at(minute: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap() + Duration::minutes(minute)
    }

    fn job(id: &str, due: DateTime<Utc>, session: Option<&str>) -> JobRecord {
        JobRecord {
            id: id.into(),
            name: id.into(),
            paused: false,
            schedule: ScheduleSpec::daily(
                due.hour() as u8,
                due.minute() as u8,
                "UTC",
                MisfirePolicy::RunOnce,
            )
            .unwrap(),
            action: JobAction::McpCall {
                server: "test".into(),
                tool: "call".into(),
                arguments: serde_json::json!({}),
                workspace: "/tmp".into(),
            },
            workspace: "/tmp".into(),
            target_session: session.map(str::to_owned),
            retry_policy: RetryPolicy {
                max_attempts: 3,
                initial_backoff_seconds: 10,
                max_backoff_seconds: 60,
            },
            next_due_at: due,
            schedule_revision: 1,
            created_at: at(-10),
            updated_at: at(-10),
        }
    }

    async fn wait_calls(executor: &RecordingExecutor, count: usize) {
        for _ in 0..100 {
            if executor.calls.lock().unwrap().len() >= count {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("executor did not receive {count} calls");
    }

    fn scheduler(
        store: JobStore,
        clock: TestClock,
        executor: RecordingExecutor,
        limit: usize,
    ) -> Arc<Scheduler> {
        Scheduler::new(
            Arc::new(Mutex::new(store)),
            Arc::new(clock),
            Arc::new(executor),
            "test-owner",
            limit,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn dispatches_jobs_due_on_first_iteration() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor::default();
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0), None)).unwrap();
        let scheduler = scheduler(store, clock, executor.clone(), 2);
        let task = tokio::spawn(scheduler.clone().run());
        wait_calls(&executor, 1).await;
        scheduler.shutdown();
        task.await.unwrap().unwrap();
        assert_eq!(executor.calls.lock().unwrap()[0].job.id, "due");
    }

    #[tokio::test(start_paused = true)]
    async fn sleeps_until_the_earliest_deadline() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor::default();
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("later", at(2), None)).unwrap();
        store.create(job("first", at(1), None)).unwrap();
        let scheduler = scheduler(store, clock.clone(), executor.clone(), 2);
        let task = tokio::spawn(scheduler.clone().run());
        tokio::task::yield_now().await;
        assert_eq!(scheduler.snapshot().1, Some(at(1)));
        clock.set(at(1));
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        wait_calls(&executor, 1).await;
        assert_eq!(executor.calls.lock().unwrap()[0].job.id, "first");
        scheduler.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn create_notification_wakes_an_old_deadline_and_pause_prevents_claim() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor::default();
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("old", at(10), None)).unwrap();
        let scheduler = scheduler(store, clock.clone(), executor.clone(), 2);
        let task = tokio::spawn(scheduler.clone().run());
        tokio::task::yield_now().await;
        scheduler
            .store()
            .lock()
            .unwrap()
            .create(job("new", at(0), None))
            .unwrap();
        scheduler.notify_changed();
        wait_calls(&executor, 1).await;
        scheduler
            .store()
            .lock()
            .unwrap()
            .set_paused("old", true, at(0))
            .unwrap();
        scheduler.notify_changed();
        clock.set(at(10));
        tokio::time::advance(std::time::Duration::from_secs(600)).await;
        tokio::task::yield_now().await;
        assert_eq!(executor.calls.lock().unwrap().len(), 1);
        scheduler.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn run_now_wakes_and_dispatches_a_future_job() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor::default();
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("future", at(10), None)).unwrap();
        let scheduler = scheduler(store, clock, executor.clone(), 1);
        let task = tokio::spawn(scheduler.clone().run());
        tokio::task::yield_now().await;
        scheduler.run_now("future").unwrap();
        wait_calls(&executor, 1).await;
        scheduler.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn bounds_concurrency_and_serializes_the_same_session() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor {
            block: true,
            ..Default::default()
        };
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("a", at(0), Some("shared"))).unwrap();
        store.create(job("b", at(0), Some("shared"))).unwrap();
        store.create(job("c", at(0), Some("other"))).unwrap();
        let scheduler = scheduler(store, clock, executor.clone(), 2);
        let task = tokio::spawn(scheduler.clone().run());
        wait_calls(&executor, 2).await;
        let sessions: Vec<_> = executor
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.job.target_session.clone())
            .collect();
        assert_eq!(
            sessions
                .iter()
                .filter(|s| s.as_deref() == Some("shared"))
                .count(),
            1
        );
        assert_eq!(scheduler.snapshot().0, 2);
        executor.gate.notify_waiters();
        wait_calls(&executor, 3).await;
        scheduler.shutdown();
        executor.gate.notify_waiters();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_active_execution_and_waits_for_it() {
        let clock = TestClock::new(at(0));
        let executor = RecordingExecutor {
            block: true,
            ..Default::default()
        };
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("due", at(0), None)).unwrap();
        let scheduler = scheduler(store, clock, executor.clone(), 1);
        let task = tokio::spawn(scheduler.clone().run());
        wait_calls(&executor, 1).await;
        let cancellation: CancellationToken =
            executor.calls.lock().unwrap()[0].cancellation.clone();
        scheduler.shutdown();
        assert!(cancellation.is_cancelled());
        executor.gate.notify_waiters();
        task.await.unwrap().unwrap();
    }

    struct OutcomeExecutor(RunOutcome);
    impl JobExecutor for OutcomeExecutor {
        fn execute(&self, _: JobRunContext) -> crate::daemon::executor::ExecutionFuture {
            let outcome = self.0.clone();
            Box::pin(async move { outcome })
        }
    }

    async fn wait_state(
        scheduler: &Scheduler,
        id: &str,
        state: crate::daemon::model::JobRunState,
    ) -> crate::daemon::model::JobRunRecord {
        for _ in 0..200 {
            if let Some(run) = scheduler
                .store()
                .lock()
                .unwrap()
                .history(id, 1)
                .unwrap()
                .first()
                && run.state == state
            {
                return run.clone();
            }
            tokio::task::yield_now().await;
        }
        panic!("run did not reach {state:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn transient_retry_persists_backoff_and_attempt_then_exhausts() {
        use crate::daemon::model::JobRunState;
        let clock = TestClock::new(at(0));
        let mut store = JobStore::in_memory().unwrap();
        let mut record = job("retry", at(0), None);
        record.retry_policy.max_attempts = 2;
        store.create(record).unwrap();
        let scheduler = Scheduler::new(
            Arc::new(Mutex::new(store)),
            Arc::new(clock.clone()),
            Arc::new(OutcomeExecutor(RunOutcome::Transient {
                error: "offline".into(),
                output: Some("details".into()),
            })),
            "owner",
            1,
        );
        let task = tokio::spawn(scheduler.clone().run());
        let retry = loop {
            let run = wait_state(&scheduler, "retry", JobRunState::Claimed).await;
            if run.attempt == 2 {
                break run;
            }
            tokio::task::yield_now().await;
        };
        let deadline = scheduler
            .store()
            .lock()
            .unwrap()
            .get("retry")
            .unwrap()
            .next_due_at;
        assert!(deadline >= at(0) + Duration::seconds(10));
        assert!(deadline <= at(0) + Duration::seconds(11));
        assert_eq!(retry.error_class.as_deref(), Some("transient"));
        assert_eq!(retry.output.as_deref(), Some("details"));
        clock.set(deadline);
        scheduler.notify_changed();
        let failed = wait_state(&scheduler, "retry", JobRunState::Failed).await;
        assert_eq!(failed.attempt, 2);
        assert_eq!(failed.id, retry.id);
        assert!(failed.lease_fence > retry.lease_fence);
        assert_eq!(
            scheduler
                .store()
                .lock()
                .unwrap()
                .get("retry")
                .unwrap()
                .next_due_at,
            at(1440)
        );
        scheduler.shutdown();
        task.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn permanent_and_ambiguous_outcomes_advance_without_retry() {
        use crate::daemon::model::JobRunState;
        for (outcome, expected) in [
            (
                RunOutcome::Permanent {
                    error: "invalid".into(),
                    output: Some("details".into()),
                },
                JobRunState::Failed,
            ),
            (
                RunOutcome::Ambiguous {
                    error: "unknown effect".into(),
                    output: None,
                },
                JobRunState::Ambiguous,
            ),
        ] {
            let mut store = JobStore::in_memory().unwrap();
            store.create(job("job", at(0), None)).unwrap();
            let scheduler = Scheduler::new(
                Arc::new(Mutex::new(store)),
                Arc::new(TestClock::new(at(0))),
                Arc::new(OutcomeExecutor(outcome)),
                "owner",
                1,
            );
            let task = tokio::spawn(scheduler.clone().run());
            let run = wait_state(&scheduler, "job", expected).await;
            assert_eq!(run.attempt, 1);
            assert_eq!(
                scheduler
                    .store()
                    .lock()
                    .unwrap()
                    .get("job")
                    .unwrap()
                    .next_due_at,
                at(1440)
            );
            scheduler.shutdown();
            task.await.unwrap().unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn startup_recovers_claim_but_never_replays_expired_running_action() {
        use crate::daemon::model::{JobRunState, RunSettlement};
        let mut store = JobStore::in_memory().unwrap();
        store.create(job("claimed", at(0), None)).unwrap();
        store.create(job("running", at(0), None)).unwrap();
        let claims = store
            .claim_due(at(0), "dead-owner", Duration::seconds(10), 2)
            .unwrap();
        let running = claims.iter().find(|run| run.job_id == "running").unwrap();
        store
            .settle_run(
                &running.id,
                "dead-owner",
                running.lease_fence,
                RunSettlement {
                    state: JobRunState::Running,
                    finished_at: at(0),
                    result_summary: None,
                    error_class: None,
                    output: None,
                },
            )
            .unwrap();
        let executor = RecordingExecutor::default();
        let scheduler = scheduler(store, TestClock::new(at(1)), executor.clone(), 2);
        let task = tokio::spawn(scheduler.clone().run());
        wait_state(&scheduler, "claimed", JobRunState::Succeeded).await;
        let ambiguous = wait_state(&scheduler, "running", JobRunState::Ambiguous).await;
        assert_eq!(ambiguous.error_class.as_deref(), Some("expired_lease"));
        assert_eq!(executor.calls.lock().unwrap().len(), 1);
        assert_eq!(executor.calls.lock().unwrap()[0].job.id, "claimed");
        scheduler.shutdown();
        task.await.unwrap().unwrap();
    }
}
