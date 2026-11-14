//! One workflow run: the script in its sandbox, and what `agent()` does when
//! the script calls it (`docs/workflows.md`).
//!
//! An `agent()` call is keyed by a hash of its prompt and options plus how
//! many calls with the same hash came before it. A call whose key finished in
//! an earlier attempt of the run is answered from `calls.jsonl` without
//! starting a thread, so a restarted run replays what it had and runs on from
//! the first call it had not finished. A call that had started a worker but not
//! finished is picked up by waiting on that worker again.
//!
//! The first slice's limits are enforced here: how many workers run at once
//! (a permit per started call), how many distinct calls a run may make (the
//! call budget), and how long a run may take (its timeout).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::reply;
use super::sandbox::{self, Bounds, Host, Reply};
use super::store::{
    CallRecord, CallState, Limits, LogLine, MAX_LINE_CHARS, MAX_LOG_LINES, Phase, RunRecord,
    RunStatus, Store, clip, now_ms,
};

pub(crate) type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a worker is started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerRequest {
    /// The prompt as the worker reads it, with the schema asked for.
    pub prompt: String,
    pub title: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The same for the same call of the same run, so asking again after a
    /// restart finds the thread the first ask started.
    pub request_id: String,
}

/// A thread a call started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worker {
    pub session_id: String,
    pub link_id: String,
}

/// How the run starts a worker thread and learns what it answered. The
/// engine's own is [`super::threads::ThreadRunner`]; tests use a fake.
pub(crate) trait AgentRunner: Send + Sync + 'static {
    /// Start a normal delegate thread, nested under the starting thread.
    /// Waits only as long as `cancel` has not fired.
    fn start<'a>(
        &'a self,
        request: &'a WorkerRequest,
        cancel: &'a CancellationToken,
    ) -> Fut<'a, Result<Worker, String>>;
    /// The worker's last reply once it is done; an error when it failed, was
    /// cancelled, or `cancel` fired first.
    fn wait<'a>(
        &'a self,
        worker: &'a Worker,
        cancel: &'a CancellationToken,
    ) -> Fut<'a, Result<String, String>>;
    /// Stop a worker whose run ended before it did, as well as it can.
    fn abandon<'a>(&'a self, worker: &'a Worker) -> Fut<'a, ()>;
}

/// What `agent()`'s second argument may hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct AgentOptions {
    pub schema: Option<Value>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub title: Option<String>,
}

impl AgentOptions {
    const KNOWN: [&'static str; 5] = ["schema", "harness", "model", "effort", "title"];

    pub(super) fn parse(options: &Value) -> Result<Self, String> {
        let Some(map) = options.as_object() else {
            return Err("agent options must be an object".to_owned());
        };
        if let Some(unknown) = map.keys().find(|key| !Self::KNOWN.contains(&key.as_str())) {
            return Err(format!(
                "agent has no option \"{unknown}\"; it takes {}",
                Self::KNOWN.join(", ")
            ));
        }
        let text = |key: &str| -> Result<Option<String>, String> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
                Some(_) => Err(format!("agent option \"{key}\" must be a non-empty string")),
            }
        };
        let schema = match map.get("schema") {
            None | Some(Value::Null) => None,
            Some(schema @ Value::Object(_)) => Some(schema.clone()),
            Some(_) => {
                return Err("agent option \"schema\" must be a JSON Schema object".to_owned());
            }
        };
        Ok(Self {
            schema,
            harness: text("harness")?,
            model: text("model")?,
            effort: text("effort")?,
            title: text("title")?,
        })
    }
}

struct Inner {
    record: RunRecord,
    /// How many calls of each hash this attempt has made so far.
    seen: HashMap<String, u32>,
    /// Every call the run has made, from earlier attempts and this one.
    known: HashMap<String, CallRecord>,
    next_seq: u64,
}

pub(super) struct Run {
    store: Store,
    runner: Arc<dyn AgentRunner>,
    cancel: CancellationToken,
    permits: Semaphore,
    inner: Mutex<Inner>,
}

/// What the sandbox's `agent()` found out about a call before it awaits.
enum Begun {
    Cached(Value),
    Fresh(Box<Fresh>),
}

struct Fresh {
    key: String,
    hash: String,
    seq: u64,
    phase: Option<String>,
    prompt: String,
    options: AgentOptions,
}

impl Run {
    /// A run over `record`, picking up the calls its folder already holds.
    pub(super) fn new(store: Store, runner: Arc<dyn AgentRunner>, record: RunRecord) -> Arc<Self> {
        let known = store
            .calls(&record.id)
            .into_iter()
            .map(|call| (call.key.clone(), call))
            .collect();
        let permits = record.limits.concurrency.max(1);
        Arc::new(Self {
            store,
            runner,
            cancel: CancellationToken::new(),
            permits: Semaphore::new(permits),
            inner: Mutex::new(Inner {
                record,
                seen: HashMap::new(),
                known,
                next_seq: 0,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn id(&self) -> String {
        self.lock().record.id.clone()
    }

    /// Write the record as it stands. A disk that refuses is logged and
    /// leaves the run going: the record is what a restart reads, not what the
    /// script runs on.
    fn save(&self) {
        let record = {
            let mut inner = self.lock();
            inner.record.updated_at = now_ms();
            inner.record.clone()
        };
        if let Err(reason) = self.store.save(&record) {
            crate::dbg_log!("workflow {}: {reason}", record.id);
        }
    }

    fn record_call(&self, call: CallRecord) {
        let run_id = {
            let mut inner = self.lock();
            inner.known.insert(call.key.clone(), call.clone());
            inner.record.id.clone()
        };
        if let Err(reason) = self.store.append_call(&run_id, &call) {
            crate::dbg_log!("workflow {run_id}: {reason}");
        }
    }

    /// The synchronous half of `agent()`: take the call's place in the order,
    /// answer it from the cache, or charge the budget.
    fn begin(&self, prompt: String, options: &str) -> Result<Begun, String> {
        let options_value: Value = serde_json::from_str(options)
            .map_err(|error| format!("agent options are not JSON: {error}"))?;
        let parsed = AgentOptions::parse(&options_value)?;
        let hash = call_hash(&prompt, &options_value);
        let mut inner = self.lock();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let count = inner.seen.entry(hash.clone()).or_insert(0);
        let key = format!("{hash}#{count}");
        *count += 1;
        let phase = inner.record.phases.last().map(|phase| phase.title.clone());
        if let Some(known) = inner.known.get(&key) {
            if known.state == CallState::Done
                && let Some(value) = known.result.clone()
            {
                return Ok(Begun::Cached(value));
            }
        } else {
            if inner.known.len() >= inner.record.limits.call_budget {
                return Err(format!(
                    "the call budget of {} agent calls is spent; raise it with call_budget when starting the run",
                    inner.record.limits.call_budget
                ));
            }
            // Held now, so calls started together cannot all pass the check.
            inner.known.insert(
                key.clone(),
                CallRecord {
                    key: key.clone(),
                    seq,
                    phase: phase.clone(),
                    prompt: clip(&prompt, MAX_LINE_CHARS),
                    harness: parsed.harness.clone(),
                    model: parsed.model.clone(),
                    state: CallState::Running,
                    attempt: 0,
                    session_id: None,
                    link_id: None,
                    started_at: now_ms(),
                    finished_at: None,
                    result: None,
                    error: None,
                },
            );
        }
        Ok(Begun::Fresh(Box::new(Fresh {
            key,
            hash,
            seq,
            phase,
            prompt,
            options: parsed,
        })))
    }

    /// The asynchronous half: start a worker (or find the one an earlier
    /// attempt started), wait for it, and keep what it answered.
    async fn execute(&self, call: Fresh) -> Result<Value, String> {
        let permit = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(STOPPED.to_owned()),
            permit = self.permits.acquire() => permit.map_err(|_| STOPPED.to_owned())?,
        };
        let earlier = self.lock().known.get(&call.key).cloned();
        let mut attempt = earlier.as_ref().map_or(0, |known| known.attempt);
        let mut record = CallRecord {
            key: call.key.clone(),
            seq: call.seq,
            phase: call.phase.clone(),
            prompt: clip(&call.prompt, MAX_LINE_CHARS),
            harness: call.options.harness.clone(),
            model: call.options.model.clone(),
            state: CallState::Running,
            attempt,
            session_id: None,
            link_id: None,
            started_at: now_ms(),
            finished_at: None,
            result: None,
            error: None,
        };
        let schema = call.options.schema.as_ref();

        // A worker an earlier attempt started and never saw finish.
        if let Some(earlier) = earlier.as_ref()
            && earlier.state == CallState::Running
            && let (Some(session_id), Some(link_id)) =
                (earlier.session_id.clone(), earlier.link_id.clone())
        {
            let worker = Worker {
                session_id,
                link_id,
            };
            record.session_id = Some(worker.session_id.clone());
            record.link_id = Some(worker.link_id.clone());
            record.started_at = earlier.started_at;
            match self.runner.wait(&worker, &self.cancel).await {
                Ok(text) => return self.finish(record, reply::result(&text, schema), permit),
                Err(_) if self.cancel.is_cancelled() => return Err(STOPPED.to_owned()),
                // It failed or is gone: this key gets a worker of its own.
                Err(_) => {
                    attempt += 1;
                    record.attempt = attempt;
                    record.session_id = None;
                    record.link_id = None;
                    record.started_at = now_ms();
                }
            }
        } else if earlier
            .as_ref()
            .is_some_and(|known| known.state == CallState::Failed)
        {
            attempt += 1;
            record.attempt = attempt;
        }

        self.record_call(record.clone());
        let request = WorkerRequest {
            prompt: match schema {
                Some(schema) => reply::ask_for_json(&call.prompt, schema),
                None => call.prompt.clone(),
            },
            title: call
                .options
                .title
                .clone()
                .unwrap_or_else(|| clip(&call.prompt, 80)),
            harness: call.options.harness.clone(),
            model: call.options.model.clone(),
            effort: call.options.effort.clone(),
            request_id: format!(
                "wf:{}:{}-{}-{}",
                self.id(),
                &call.hash[..12.min(call.hash.len())],
                call.key.rsplit('#').next().unwrap_or("0"),
                attempt
            ),
        };
        let worker = match self.runner.start(&request, &self.cancel).await {
            Ok(worker) => worker,
            Err(_) if self.cancel.is_cancelled() => return Err(STOPPED.to_owned()),
            Err(reason) => return self.finish(record, Err(reason), permit),
        };
        record.session_id = Some(worker.session_id.clone());
        record.link_id = Some(worker.link_id.clone());
        self.record_call(record.clone());
        match self.runner.wait(&worker, &self.cancel).await {
            Ok(text) => self.finish(record, reply::result(&text, schema), permit),
            Err(_) if self.cancel.is_cancelled() => Err(STOPPED.to_owned()),
            Err(reason) => self.finish(record, Err(reason), permit),
        }
    }

    /// Keep a call's outcome and give back what the script gets.
    fn finish(
        &self,
        mut record: CallRecord,
        outcome: Result<Value, String>,
        permit: tokio::sync::SemaphorePermit<'_>,
    ) -> Result<Value, String> {
        record.finished_at = Some(now_ms());
        match &outcome {
            Ok(value) => {
                record.state = CallState::Done;
                record.result = Some(value.clone());
            }
            Err(reason) => {
                record.state = CallState::Failed;
                record.error = Some(clip(reason, MAX_LINE_CHARS));
            }
        }
        self.record_call(record);
        drop(permit);
        outcome
    }

    /// The workers still running, for a run that ended before they did.
    fn unfinished(&self) -> Vec<Worker> {
        self.lock()
            .known
            .values()
            .filter(|call| call.state == CallState::Running)
            .filter_map(|call| {
                Some(Worker {
                    session_id: call.session_id.clone()?,
                    link_id: call.link_id.clone()?,
                })
            })
            .collect()
    }
}

/// What a script is told when its run was stopped under it.
const STOPPED: &str = "the run was stopped";

/// A call's identity: its prompt and options, spelled the same however the
/// script ordered the options.
pub(super) fn call_hash(prompt: &str, options: &Value) -> String {
    let hash = crate::orchestration::args_hash(
        "workflow.agent",
        &json!({ "prompt": prompt, "options": options }),
    );
    hash[..24.min(hash.len())].to_owned()
}

/// The sandbox's view of a run.
struct Handle(Arc<Run>);

impl Host for Handle {
    fn phase(&self, title: &str) {
        {
            let mut inner = self.0.lock();
            inner.record.phases.push(Phase {
                title: clip(title, MAX_LINE_CHARS),
                started_at: now_ms(),
            });
        }
        self.0.save();
    }

    fn log(&self, line: &str) {
        {
            let mut inner = self.0.lock();
            inner.record.logs.push(LogLine {
                at: now_ms(),
                text: clip(line, MAX_LINE_CHARS),
            });
            let extra = inner.record.logs.len().saturating_sub(MAX_LOG_LINES);
            inner.record.logs.drain(..extra);
        }
        self.0.save();
    }

    fn agent(&self, prompt: String, options: String) -> Reply {
        let run = Arc::clone(&self.0);
        // The call's place in the order is taken now, while the script is
        // still running synchronously, not when the future is first polled.
        let begun = run.begin(prompt, &options);
        Box::pin(async move {
            let outcome = match begun {
                Ok(Begun::Cached(value)) => Ok(value),
                Ok(Begun::Fresh(call)) => run.execute(*call).await,
                Err(reason) => Err(reason),
            };
            match outcome {
                Ok(value) => json!({ "value": value }),
                Err(reason) => json!({ "error": reason }),
            }
            .to_string()
        })
    }
}

/// Run `source` to its end, a stop, or its timeout, and record how it ended.
pub(super) async fn drive(run: Arc<Run>, source: String) {
    let (timeout, args) = {
        let inner = run.lock();
        (
            Duration::from_secs(inner.record.limits.timeout_secs),
            inner.record.args.clone(),
        )
    };
    let started = Instant::now();
    let host: Arc<dyn Host> = Arc::new(Handle(Arc::clone(&run)));
    let script = sandbox::run(&source, &args, host, Bounds::until(started + timeout));
    enum Ended {
        Stopped,
        TimedOut,
        Finished(Result<Value, String>),
    }
    let ended = tokio::select! {
        biased;
        _ = run.cancel.cancelled() => Ended::Stopped,
        _ = tokio::time::sleep(timeout) => Ended::TimedOut,
        result = script => Ended::Finished(result),
    };
    // Wake whatever still waits on a worker, then stop the workers the run
    // leaves behind.
    let leftovers = match &ended {
        Ended::Finished(Ok(_)) => Vec::new(),
        _ => {
            run.cancel.cancel();
            run.unfinished()
        }
    };
    for worker in &leftovers {
        run.runner.abandon(worker).await;
    }
    let mut result_file = None;
    {
        let mut inner = run.lock();
        inner.record.finished_at = Some(now_ms());
        match ended {
            Ended::Stopped => {
                inner.record.status = RunStatus::Cancelled;
                inner.record.error = Some("the run was stopped".to_owned());
            }
            Ended::TimedOut => {
                inner.record.status = RunStatus::TimedOut;
                inner.record.error = Some(format!(
                    "the run took longer than its timeout of {} s",
                    timeout.as_secs()
                ));
            }
            Ended::Finished(Ok(value)) => {
                inner.record.status = RunStatus::Done;
                result_file = Some(value.clone());
                inner.record.result = Some(value);
            }
            Ended::Finished(Err(reason)) if reason == "the script ran past its time limit" => {
                inner.record.status = RunStatus::TimedOut;
                inner.record.error = Some(format!(
                    "the script ran past the run's timeout of {} s",
                    timeout.as_secs()
                ));
            }
            Ended::Finished(Err(reason)) => {
                inner.record.status = RunStatus::Failed;
                inner.record.error = Some(reason);
            }
        }
    }
    if let Some(value) = result_file {
        let id = run.id();
        if let Err(reason) = run.store.save_result(&id, &value) {
            crate::dbg_log!("workflow {id}: {reason}");
        }
    }
    run.save();
}

/// A fresh record for a run about to start.
pub(super) fn new_record(
    thread: &str,
    name: Option<String>,
    args: Value,
    limits: Limits,
) -> RunRecord {
    let now = now_ms();
    RunRecord {
        id: super::store::new_run_id(),
        thread: thread.to_owned(),
        name,
        status: RunStatus::Running,
        args,
        limits,
        started_at: now,
        updated_at: now,
        finished_at: None,
        resumes: 0,
        phases: Vec::new(),
        logs: Vec::new(),
        result: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A runner that answers from a closure and counts what it was asked.
    struct Fake {
        started: Mutex<Vec<WorkerRequest>>,
        running: AtomicUsize,
        most_running: AtomicUsize,
        abandoned: AtomicUsize,
        delay: Duration,
        /// The reply for a prompt, or an error.
        reply: Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>,
    }

    impl Fake {
        fn new(
            reply: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                started: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                most_running: AtomicUsize::new(0),
                abandoned: AtomicUsize::new(0),
                delay: Duration::from_millis(20),
                reply: Box::new(reply),
            })
        }

        fn echo() -> Arc<Self> {
            Self::new(|prompt| Ok(format!("answer to: {prompt}")))
        }

        fn starts(&self) -> usize {
            self.started.lock().unwrap().len()
        }
    }

    impl AgentRunner for Fake {
        fn start<'a>(
            &'a self,
            request: &'a WorkerRequest,
            _cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<Worker, String>> {
            Box::pin(async move {
                let mut started = self.started.lock().unwrap();
                started.push(request.clone());
                let n = started.len();
                Ok(Worker {
                    session_id: format!("session-{n}"),
                    link_id: format!("link-{n}"),
                })
            })
        }

        fn wait<'a>(
            &'a self,
            worker: &'a Worker,
            cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<String, String>> {
            Box::pin(async move {
                let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_running.fetch_max(now, Ordering::SeqCst);
                let outcome = tokio::select! {
                    _ = cancel.cancelled() => Err("cancelled".to_owned()),
                    _ = tokio::time::sleep(self.delay) => {
                        let n: usize = worker.link_id.trim_start_matches("link-").parse().unwrap();
                        let prompt = self.started.lock().unwrap()[n - 1].prompt.clone();
                        (self.reply)(&prompt)
                    }
                };
                self.running.fetch_sub(1, Ordering::SeqCst);
                outcome
            })
        }

        fn abandon<'a>(&'a self, _worker: &'a Worker) -> Fut<'a, ()> {
            Box::pin(async move {
                self.abandoned.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    fn limits() -> Limits {
        Limits {
            concurrency: 3,
            call_budget: 20,
            timeout_secs: 30,
        }
    }

    struct Setup {
        _dir: tempfile::TempDir,
        store: Store,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        Setup { _dir: dir, store }
    }

    /// Start a run of `source` and drive it to its end.
    async fn run_to_end(
        setup: &Setup,
        runner: Arc<Fake>,
        source: &str,
        limits: Limits,
    ) -> RunRecord {
        let record = new_record(
            "1784626803679",
            None,
            json!({"files": ["a", "b", "c"]}),
            limits,
        );
        let id = record.id.clone();
        setup.store.create(&record, source).unwrap();
        drive(
            Run::new(setup.store.clone(), runner, record),
            source.to_owned(),
        )
        .await;
        setup.store.load(&id).unwrap()
    }

    /// Drive an existing run again, as a restart does.
    async fn resume(setup: &Setup, runner: Arc<Fake>, id: &str) -> RunRecord {
        let mut record = setup.store.load(id).unwrap();
        record.status = RunStatus::Running;
        record.resumes += 1;
        let source = setup.store.script(id).unwrap();
        drive(Run::new(setup.store.clone(), runner, record), source).await;
        setup.store.load(id).unwrap()
    }

    #[tokio::test]
    async fn a_two_phase_pipeline_runs_and_returns_its_result() {
        let setup = setup();
        let runner = Fake::echo();
        let record = run_to_end(
            &setup,
            runner.clone(),
            r#"
            phase("review");
            const reviews = await parallel(args.files.map((f) => () => agent("review " + f)));
            phase("verify");
            const verdict = await agent("verify " + reviews.length + " reviews");
            log("verified");
            return { reviews, verdict };
            "#,
            limits(),
        )
        .await;
        assert_eq!(record.status, RunStatus::Done, "{:?}", record.error);
        assert_eq!(
            record.result,
            Some(json!({
                "reviews": ["answer to: review a", "answer to: review b", "answer to: review c"],
                "verdict": "answer to: verify 3 reviews",
            }))
        );
        assert_eq!(
            record
                .phases
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            ["review", "verify"]
        );
        assert_eq!(record.logs[0].text, "verified");
        assert_eq!(runner.starts(), 4);
        let calls = setup.store.calls(&record.id);
        assert_eq!(calls.len(), 4);
        assert!(calls.iter().all(|call| call.state == CallState::Done));
        assert_eq!(calls[0].phase.as_deref(), Some("review"));
        assert_eq!(calls[3].phase.as_deref(), Some("verify"));
        assert!(calls.iter().all(|call| call.session_id.is_some()));
        assert!(record.finished_at.is_some());
    }

    #[tokio::test]
    async fn a_resumed_run_replays_finished_calls_and_continues_from_the_first_new_one() {
        let setup = setup();
        let source = r#"
            const first = await agent("one");
            const second = await agent("two");
            const third = await agent("three");
            return [first, second, third];
        "#;
        // The first attempt dies at "three": the worker fails.
        let first = Fake::new(|prompt| {
            if prompt == "three" {
                Err("the worker failed".to_owned())
            } else {
                Ok(format!("answer to: {prompt}"))
            }
        });
        let record = run_to_end(&setup, first.clone(), source, limits()).await;
        assert_eq!(record.status, RunStatus::Failed);
        assert_eq!(first.starts(), 3);

        // The restart starts no worker for "one" and "two".
        let second = Fake::echo();
        let record = resume(&setup, second.clone(), &record.id).await;
        assert_eq!(record.status, RunStatus::Done, "{:?}", record.error);
        assert_eq!(
            record.result,
            Some(json!([
                "answer to: one",
                "answer to: two",
                "answer to: three"
            ]))
        );
        assert_eq!(second.starts(), 1);
        assert_eq!(second.started.lock().unwrap()[0].prompt, "three");
        // A failed call is tried again as a worker of its own.
        assert!(second.started.lock().unwrap()[0].request_id.ends_with("-1"));
        assert_eq!(record.resumes, 1);
    }

    #[tokio::test]
    async fn identical_calls_are_separate_calls_and_replay_in_order() {
        let setup = setup();
        let source = r#"
            const a = await agent("same");
            const b = await agent("same");
            return [a, b];
        "#;
        let counter = Arc::new(AtomicUsize::new(0));
        let numbered = {
            let counter = counter.clone();
            Fake::new(move |_| Ok(format!("reply {}", counter.fetch_add(1, Ordering::SeqCst))))
        };
        let record = run_to_end(&setup, numbered.clone(), source, limits()).await;
        assert_eq!(record.result, Some(json!(["reply 0", "reply 1"])));
        assert_eq!(numbered.starts(), 2);
        let again = Fake::echo();
        let record = resume(&setup, again.clone(), &record.id).await;
        assert_eq!(record.result, Some(json!(["reply 0", "reply 1"])));
        assert_eq!(again.starts(), 0);
    }

    #[tokio::test]
    async fn a_changed_prompt_is_not_answered_from_the_cache() {
        let setup = setup();
        let record = run_to_end(
            &setup,
            Fake::echo(),
            r#"return await agent("old prompt");"#,
            limits(),
        )
        .await;
        // Edit the saved script, then resume: the new call has no cached answer.
        let script = setup.store.run_dir(&record.id).unwrap().join("script.js");
        std::fs::write(script, r#"return await agent("new prompt");"#).unwrap();
        let runner = Fake::echo();
        let record = resume(&setup, runner.clone(), &record.id).await;
        assert_eq!(record.result, Some(json!("answer to: new prompt")));
        assert_eq!(runner.starts(), 1);
    }

    #[tokio::test]
    async fn the_call_budget_is_enforced_and_a_resume_does_not_spend_it_again() {
        let setup = setup();
        let source = r#"
            const results = [];
            for (const name of ["a", "b", "c"]) {
              try { results.push(await agent("call " + name)); }
              catch (error) { results.push("refused: " + error.message); }
            }
            return results;
        "#;
        let tight = Limits {
            call_budget: 2,
            ..limits()
        };
        let runner = Fake::echo();
        let record = run_to_end(&setup, runner.clone(), source, tight).await;
        assert_eq!(record.status, RunStatus::Done);
        let results = record.result.clone().unwrap();
        assert_eq!(results[0], json!("answer to: call a"));
        assert_eq!(results[1], json!("answer to: call b"));
        assert!(
            results[2]
                .as_str()
                .unwrap()
                .starts_with("refused: the call budget of 2"),
            "{results}"
        );
        assert_eq!(runner.starts(), 2);
        // Replaying the two finished calls costs nothing, and the third is
        // still refused rather than let through by a fresh count.
        let again = Fake::echo();
        let record = resume(&setup, again.clone(), &record.id).await;
        assert_eq!(record.result.unwrap()[2], results[2]);
        assert_eq!(again.starts(), 0);
    }

    #[tokio::test]
    async fn no_more_workers_run_at_once_than_the_concurrency_limit() {
        let setup = setup();
        let runner = Fake::echo();
        let narrow = Limits {
            concurrency: 2,
            ..limits()
        };
        let record = run_to_end(
            &setup,
            runner.clone(),
            r#"return await parallel([1, 2, 3, 4, 5, 6].map((n) => () => agent("job " + n)));"#,
            narrow,
        )
        .await;
        assert_eq!(record.status, RunStatus::Done, "{:?}", record.error);
        assert_eq!(runner.starts(), 6);
        assert_eq!(runner.most_running.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_run_past_its_timeout_stops_and_abandons_its_workers() {
        let setup = setup();
        let runner = Arc::new(Fake {
            delay: Duration::from_secs(60),
            ..Arc::into_inner(Fake::echo()).unwrap()
        });
        let quick = Limits {
            timeout_secs: 1,
            ..limits()
        };
        let record = run_to_end(
            &setup,
            runner.clone(),
            r#"return await agent("slow");"#,
            quick,
        )
        .await;
        assert_eq!(record.status, RunStatus::TimedOut);
        assert!(record.error.unwrap().contains("timeout"));
        assert_eq!(runner.abandoned.load(Ordering::SeqCst), 1);
        // The unfinished call stays running on disk, so a resume would wait on it.
        let calls = setup.store.calls(&record.id);
        assert_eq!(calls[0].state, CallState::Running);
    }

    #[tokio::test]
    async fn a_schema_turns_the_reply_into_a_value_or_an_error_the_script_sees() {
        let setup = setup();
        let runner = Fake::new(|prompt| {
            assert!(prompt.contains("JSON Schema"), "{prompt}");
            if prompt.starts_with("good") {
                Ok("```json\n{\"verdict\": \"pass\"}\n```".to_owned())
            } else {
                Ok("looks fine to me".to_owned())
            }
        });
        let record = run_to_end(
            &setup,
            runner.clone(),
            r#"
            const schema = { type: "object", properties: { verdict: { type: "string" } }, required: ["verdict"] };
            const good = await agent("good", { schema });
            let bad;
            try { await agent("bad", { schema }); bad = "accepted"; } catch (e) { bad = e.message; }
            return { good, bad };
            "#,
            limits(),
        )
        .await;
        assert_eq!(record.status, RunStatus::Done, "{:?}", record.error);
        let result = record.result.unwrap();
        assert_eq!(result["good"], json!({"verdict": "pass"}));
        assert!(
            result["bad"].as_str().unwrap().contains("no JSON"),
            "{result}"
        );
    }

    #[tokio::test]
    async fn options_reach_the_worker_and_unknown_ones_are_refused() {
        let setup = setup();
        let runner = Fake::echo();
        let record = run_to_end(
            &setup,
            runner.clone(),
            r#"
            await agent("a", { harness: "codex", model: "gpt-5", effort: "high", title: "Reviewer" });
            try { await agent("b", { colour: "red" }); return "accepted"; }
            catch (error) { return error.message; }
            "#,
            limits(),
        )
        .await;
        let started = runner.started.lock().unwrap().clone();
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].harness.as_deref(), Some("codex"));
        assert_eq!(started[0].model.as_deref(), Some("gpt-5"));
        assert_eq!(started[0].effort.as_deref(), Some("high"));
        assert_eq!(started[0].title, "Reviewer");
        assert!(
            record
                .result
                .unwrap()
                .as_str()
                .unwrap()
                .contains("no option \"colour\"")
        );
    }

    #[tokio::test]
    async fn a_script_error_fails_the_run_with_its_message() {
        let setup = setup();
        let record = run_to_end(
            &setup,
            Fake::echo(),
            r#"throw new Error("bad input");"#,
            limits(),
        )
        .await;
        assert_eq!(record.status, RunStatus::Failed);
        assert_eq!(record.error.as_deref(), Some("bad input"));
    }
}
