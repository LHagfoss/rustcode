//! Durable workflow runs that fan out to threads (#3394, first slice;
//! `docs/workflows.md`).
//!
//! A workflow is a JavaScript script. It runs in a sandbox with no filesystem,
//! shell, network or clock ([`sandbox`]) and reaches the outside only through
//! `agent(prompt, options)`, `parallel(items)`, `phase(title)` and `log(...)`.
//! Each `agent()` call starts a normal delegate thread under the thread that
//! started the run ([`threads`]), and the run keeps every finished call on disk
//! under that thread's folder ([`store`]), so a run that is cut short by a
//! restart replays what it had and continues from the first call it had not
//! finished ([`run`]).
//!
//! The agent starts, reads and stops a run with three tools the `rustcode` MCP
//! server offers, `run_workflow`, `workflow_status` and `stop_workflow`; the
//! engine's hub answers them ([`start`], [`status`], [`stop`]). The app shows a
//! run from the same record: every change is announced ([`changes`]) and the
//! app reads a run's summary ([`run_summary`]), stops it and claims the one
//! wake that tells the starting thread it ended ([`claim_wake`]).

mod reply;
mod run;
mod sandbox;
mod store;
mod threads;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use self::run::{AgentRunner, Run, drive, new_record};
use self::store::{CallState, Limits, RunRecord, RunStatus, Store, clip, now_ms};
pub(crate) use self::threads::ThreadRunner;

pub(crate) const RUN_TOOL_NAME: &str = "run_workflow";
pub(crate) const STATUS_TOOL_NAME: &str = "workflow_status";
pub(crate) const STOP_TOOL_NAME: &str = "stop_workflow";
pub(crate) const TOOL_NAMES: [&str; 3] = [RUN_TOOL_NAME, STATUS_TOOL_NAME, STOP_TOOL_NAME];

/// Whether `name` is one of the workflow tools.
pub(crate) fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// The longest script `run_workflow` takes inline.
pub(crate) const MAX_SCRIPT_BYTES: usize = 64 * 1024;
/// The longest `wait_ms` a `workflow_status` call may ask for.
pub(crate) const MAX_WAIT_MS: u64 = crate::orchestration::MAX_WAIT_MS;
/// The folder, under a project, that holds the named scripts.
const WORKFLOW_FOLDER: [&str; 2] = [".rustcode", "workflows"];
/// The most a `workflow_status` answer carries of a final result inline; a
/// larger one is in the run's `result.json`.
const MAX_INLINE_RESULT_BYTES: usize = 96 * 1024;
/// How many runs a listing names.
const LISTED_RUNS: usize = 20;
/// How many log lines a status carries.
const STATUS_LOG_LINES: usize = 20;
/// How often `workflow_status` re-reads a run it waits on.
const STATUS_POLL: Duration = Duration::from_millis(500);
/// How long a stop waits for the run to wind down (its workers are
/// interrupted first) before answering that it is stopping.
const STOP_WAIT: Duration = Duration::from_secs(15);

/// The limits when the user has not set any, and the most any may be set to.
/// The settings are `workflow-concurrency`, `workflow-call-budget` and
/// `workflow-timeout-minutes` (`acp/settings.rs`).
pub(crate) const DEFAULT_CONCURRENCY: u64 = 3;
pub(crate) const MAX_CONCURRENCY: u64 = 8;
pub(crate) const DEFAULT_CALL_BUDGET: u64 = 40;
pub(crate) const MAX_CALL_BUDGET: u64 = 500;
pub(crate) const DEFAULT_TIMEOUT_MINUTES: u64 = 60;
pub(crate) const MAX_TIMEOUT_MINUTES: u64 = 24 * 60;

/// The limits a run gets: what the user's settings say, read through `read`.
fn configured_limits(read: impl Fn(&str) -> Option<u64>) -> Limits {
    let pick = |key: &str, default: u64, max: u64| read(key).unwrap_or(default).clamp(1, max);
    Limits {
        concurrency: pick("workflow-concurrency", DEFAULT_CONCURRENCY, MAX_CONCURRENCY) as usize,
        call_budget: pick("workflow-call-budget", DEFAULT_CALL_BUDGET, MAX_CALL_BUDGET) as usize,
        timeout_secs: pick(
            "workflow-timeout-minutes",
            DEFAULT_TIMEOUT_MINUTES,
            MAX_TIMEOUT_MINUTES,
        ) * 60,
    }
}

fn settings_limits() -> Limits {
    configured_limits(|key| crate::acp::settings::value(key).and_then(|value| value.as_u64()))
}

/// `configured` lowered by what the call asked for. A call can ask for less
/// than the user allows, never more.
fn lowered(configured: Limits, args: &Value) -> Limits {
    let ask = |key: &str| {
        args.get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
    };
    Limits {
        concurrency: ask("concurrency").map_or(configured.concurrency, |asked| {
            configured.concurrency.min(asked as usize)
        }),
        call_budget: ask("call_budget").map_or(configured.call_budget, |asked| {
            configured.call_budget.min(asked as usize)
        }),
        timeout_secs: ask("timeout_minutes").map_or(configured.timeout_secs, |asked| {
            configured.timeout_secs.min(asked.saturating_mul(60))
        }),
    }
}

pub(crate) fn run_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "script": {"type": "string", "minLength": 1, "maxLength": MAX_SCRIPT_BYTES, "description": "The workflow's JavaScript, run as the body of an async function: it may await and return a JSON value. Give this or name."},
            "name": {"type": "string", "minLength": 1, "maxLength": 100, "description": "The workflow to run from .rustcode/workflows/<name>.js in this thread's project, instead of a script"},
            "args": {"description": "Any JSON; the script reads it as the global `args`"},
            "concurrency": {"type": "integer", "minimum": 1, "maximum": MAX_CONCURRENCY, "description": "How many workers may run at once; the user's setting is the most"},
            "call_budget": {"type": "integer", "minimum": 1, "maximum": MAX_CALL_BUDGET, "description": "How many agent() calls the run may make in all; the user's setting is the most"},
            "timeout_minutes": {"type": "integer", "minimum": 1, "maximum": MAX_TIMEOUT_MINUTES, "description": "How long the run may take; the user's setting is the most"}
        },
        "additionalProperties": false
    })
}

pub(crate) fn status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "run_id": {"type": "string", "minLength": 1, "maxLength": 64, "description": "A run run_workflow returned; omit to list this thread's runs"},
            "wait_ms": {"type": "integer", "minimum": 1, "maximum": MAX_WAIT_MS, "description": "Wait up to this long for the run to finish, and answer sooner if it does. Without it the answer is the run's state now"}
        },
        "additionalProperties": false
    })
}

pub(crate) fn stop_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "run_id": {"type": "string", "minLength": 1, "maxLength": 64, "description": "The run to stop, from run_workflow or workflow_status"}
        },
        "required": ["run_id"],
        "additionalProperties": false
    })
}

/// Check a workflow tool's arguments before anything runs.
pub(crate) fn validate(tool: &str, args: &Value) -> Result<(), String> {
    let known: &[&str] = match tool {
        RUN_TOOL_NAME => &[
            "script",
            "name",
            "args",
            "concurrency",
            "call_budget",
            "timeout_minutes",
        ],
        STATUS_TOOL_NAME => &["run_id", "wait_ms"],
        STOP_TOOL_NAME => &["run_id"],
        _ => return Err(format!("{tool} is not a workflow tool")),
    };
    let Some(map) = args.as_object() else {
        return Err("arguments must be an object".to_owned());
    };
    if let Some(unknown) = map.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(format!("unknown argument \"{unknown}\""));
    }
    let number = |key: &str, max: u64| -> Result<(), String> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(()),
            Some(value) => match value.as_u64() {
                Some(number) if (1..=max).contains(&number) => Ok(()),
                _ => Err(format!("{key} must be a whole number from 1 to {max}")),
            },
        }
    };
    if tool == STOP_TOOL_NAME {
        return match map.get("run_id") {
            Some(Value::String(id)) if store::valid_run_id(id) => Ok(()),
            _ => Err("run_id must be a run id from run_workflow or workflow_status".to_owned()),
        };
    }
    if tool == STATUS_TOOL_NAME {
        match map.get("run_id") {
            None | Some(Value::Null) => {}
            Some(Value::String(id)) if store::valid_run_id(id) => {}
            Some(_) => return Err("run_id is not a run id".to_owned()),
        }
        return number("wait_ms", MAX_WAIT_MS);
    }
    number("concurrency", MAX_CONCURRENCY)?;
    number("call_budget", MAX_CALL_BUDGET)?;
    number("timeout_minutes", MAX_TIMEOUT_MINUTES)?;
    let text = |key: &str| match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.as_str())),
        Some(_) => Err(format!("{key} must be a non-empty string")),
    };
    match (text("script")?, text("name")?) {
        (Some(_), Some(_)) => Err("give a script or a name, not both".to_owned()),
        (None, None) => Err("give a script, or the name of one in .rustcode/workflows/".to_owned()),
        (Some(script), None) if script.len() > MAX_SCRIPT_BYTES => Err(format!(
            "the script is over {MAX_SCRIPT_BYTES} bytes; keep it in .rustcode/workflows/ and run it by name"
        )),
        (None, Some(name)) => valid_name(name),
        _ => Ok(()),
    }
}

/// A workflow's name is the file's, without a folder or an extension.
fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err("name is the file's name in .rustcode/workflows/ without .js: letters, digits, '-', '_' and '.' only".to_owned())
    }
}

/// The script behind a name, from the project the thread works in.
fn named_script(thread: &str, name: &str) -> Result<String, String> {
    let cwd = crate::checkpoints::thread_cwd(thread)
        .filter(|cwd| cwd.is_dir())
        .ok_or_else(|| "this thread has no project folder to read workflows from".to_owned())?;
    let path = WORKFLOW_FOLDER
        .iter()
        .fold(cwd, |path, part| path.join(part))
        .join(format!("{name}.js"));
    let script = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "there is no workflow named \"{name}\" ({} not found)",
            path.display()
        )
    })?;
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(format!(
            "{} is over {MAX_SCRIPT_BYTES} bytes",
            path.display()
        ));
    }
    Ok(script)
}

/// The runs this process is driving, by id, with what stops each.
static ACTIVE: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn active(id: &str) -> bool {
    ACTIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(id)
}

/// A run that changed: a phase began, a worker started or ended, a log line
/// was added, or the run ended.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Change {
    pub thread: String,
    pub run_id: String,
}

static CHANGES: LazyLock<tokio::sync::broadcast::Sender<Change>> =
    LazyLock::new(|| tokio::sync::broadcast::channel(512).0);

/// What the app listens to: one [`Change`] each time a run's record moves.
pub(crate) fn changes() -> tokio::sync::broadcast::Receiver<Change> {
    CHANGES.subscribe()
}

fn announce(thread: &str, run_id: &str) {
    // Nobody listening is not an error: no app is open.
    let _ = CHANGES.send(Change {
        thread: thread.to_owned(),
        run_id: run_id.to_owned(),
    });
}

/// Drive `record`'s run in the background until it ends.
fn spawn(store: Store, runner: Arc<dyn AgentRunner>, record: RunRecord, source: String) {
    let id = record.id.clone();
    let run = Run::new(store, runner, record);
    ACTIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(id.clone(), run.stopper());
    tokio::spawn(async move {
        drive(run, source).await;
        ACTIVE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    });
}

/// Create a run in `store` and start it. The run id.
fn launch(
    store: Store,
    runner: Arc<dyn AgentRunner>,
    thread: &str,
    name: Option<String>,
    source: String,
    args: Value,
    limits: Limits,
) -> Result<String, String> {
    let record = new_record(thread, name, args, limits);
    store.create(&record, &source)?;
    let id = record.id.clone();
    spawn(store, runner, record, source);
    Ok(id)
}

/// `run_workflow`: start the run and answer with its id at once.
pub(crate) fn start(
    thread: &str,
    runner: Arc<dyn AgentRunner>,
    args: &Value,
) -> Result<String, String> {
    validate(RUN_TOOL_NAME, args)?;
    let (name, source) = match args.get("name").and_then(Value::as_str) {
        Some(name) => (Some(name.to_owned()), named_script(thread, name)?),
        None => (
            None,
            args.get("script")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
    };
    let store = Store::for_thread(thread)
        .ok_or_else(|| "this thread has no folder to keep a run in".to_owned())?;
    let limits = lowered(settings_limits(), args);
    let script_args = args.get("args").cloned().unwrap_or(Value::Null);
    let id = launch(store, runner, thread, name, source, script_args, limits)?;
    announce(thread, &id);
    Ok(json!({
        "run_id": id,
        "status": "running",
        "limits": {
            "concurrency": limits.concurrency,
            "call_budget": limits.call_budget,
            "timeout_minutes": limits.timeout_secs / 60,
        },
        "note": "Read it with workflow_status (wait_ms waits for the end), stop it with stop_workflow. Its workers are threads nested under this one."
    })
    .to_string())
}

/// Pick up the runs of `thread` that a restart cut short: each continues from
/// its first unfinished call. Called when the thread comes back to the engine;
/// a run this process already drives is left alone. How many were resumed.
pub(crate) fn resume(thread: &str, runner: Arc<dyn AgentRunner>) -> usize {
    if tokio::runtime::Handle::try_current().is_err() {
        return 0;
    }
    match Store::for_thread(thread) {
        Some(store) => resume_in(&store, &runner),
        None => 0,
    }
}

fn resume_in(store: &Store, runner: &Arc<dyn AgentRunner>) -> usize {
    let mut resumed = 0;
    for mut record in store.list() {
        if record.status != RunStatus::Running || active(&record.id) {
            continue;
        }
        let Some(source) = store.script(&record.id) else {
            continue;
        };
        record.resumes += 1;
        if store.save(&record).is_err() {
            continue;
        }
        spawn(store.clone(), Arc::clone(runner), record, source);
        resumed += 1;
    }
    resumed
}

/// `workflow_status`: one run in full, or this thread's runs in brief.
pub(crate) async fn status(
    thread: &str,
    args: &Value,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let store = Store::for_thread(thread)
        .ok_or_else(|| "this thread has no folder that keeps runs".to_owned())?;
    status_in(&store, args, cancel).await
}

async fn status_in(
    store: &Store,
    args: &Value,
    cancel: &CancellationToken,
) -> Result<String, String> {
    validate(STATUS_TOOL_NAME, args)?;
    let Some(run_id) = args.get("run_id").and_then(Value::as_str) else {
        return Ok(listing(store).to_string());
    };
    let wait = args
        .get("wait_ms")
        .and_then(Value::as_u64)
        .map(Duration::from_millis);
    let deadline = wait.map(|wait| tokio::time::Instant::now() + wait);
    loop {
        let Some(record) = store.load(run_id) else {
            return Err(format!("no run {run_id} in this thread"));
        };
        let waiting = deadline.is_some_and(|deadline| tokio::time::Instant::now() < deadline);
        if record.status.settled() || !waiting || cancel.is_cancelled() {
            // The agent reads how it ended: its thread need not be told again.
            if record.status.settled() {
                mark_told(store, &record.id);
            }
            return Ok(report(store, &record).to_string());
        }
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = tokio::time::sleep(STATUS_POLL) => {}
        }
    }
}

fn listing(store: &Store) -> Value {
    let runs: Vec<Value> = store
        .list()
        .into_iter()
        .take(LISTED_RUNS)
        .map(|record| {
            json!({
                "run_id": record.id,
                "status": record.status,
                "name": record.name,
                "started_at": record.started_at,
                "finished_at": record.finished_at,
                "phase": record.phases.last().map(|phase| phase.title.clone()),
                "workers": store.calls(&record.id).len(),
            })
        })
        .collect();
    json!({ "runs": runs })
}

/// `stop_workflow` and the app's Stop: end the run. The script stops, no
/// further `agent()` call starts, and the workers still running are
/// interrupted; the run is `cancelled`. Answers with the run once it has wound
/// down, or `stopping` when that takes longer than a few seconds. A run that
/// already ended is answered as it is.
///
/// `told` is whether the one who stopped it is the thread's own agent: it has
/// the answer, so the thread is not woken for the end.
pub(crate) async fn stop(thread: &str, run_id: &str, told: bool) -> Result<Value, String> {
    validate(STOP_TOOL_NAME, &json!({ "run_id": run_id }))?;
    let store = Store::for_thread(thread)
        .ok_or_else(|| "this thread has no folder that keeps runs".to_owned())?;
    let answer = stop_in(&store, run_id).await?;
    if told && answer["status"] != json!("running") && answer.get("stopping").is_none() {
        mark_told(&store, run_id);
    }
    Ok(answer)
}

async fn stop_in(store: &Store, run_id: &str) -> Result<Value, String> {
    let Some(mut record) = store.load(run_id) else {
        return Err(format!("no run {run_id} in this thread"));
    };
    if record.status.settled() {
        return Ok(summary(store, &record));
    }
    let token = ACTIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(run_id)
        .cloned();
    let Some(token) = token else {
        // A restart left it running and nothing drives it: end it on its
        // record, so it is not picked up again. Its workers, if any are still
        // alive, are threads the user can stop like any other.
        record.status = RunStatus::Cancelled;
        record.finished_at = Some(now_ms());
        record.updated_at = now_ms();
        record.error = Some("the run was stopped".to_owned());
        store.save(&record)?;
        announce(&record.thread, &record.id);
        return Ok(summary(store, &record));
    };
    token.cancel();
    let deadline = tokio::time::Instant::now() + STOP_WAIT;
    loop {
        if let Some(now) = store.load(run_id)
            && now.status.settled()
            && !active(run_id)
        {
            return Ok(summary(store, &now));
        }
        if tokio::time::Instant::now() >= deadline {
            let mut answer = summary(store, &record);
            answer["stopping"] = json!(true);
            return Ok(answer);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Serializes claims, so two clients cannot both win one.
static WAKES: Mutex<()> = Mutex::new(());

/// Claim the one wake for an ended run: true for the first caller after it
/// ended, false for a run still going and for every later caller. The app
/// that wins tells the thread that started the run.
pub(crate) fn claim_wake(thread: &str, run_id: &str) -> Result<bool, String> {
    let store = Store::for_thread(thread)
        .ok_or_else(|| "this thread has no folder that keeps runs".to_owned())?;
    claim_wake_in(&store, run_id)
}

/// Record that the thread knows how the run ended, so nothing wakes it for it.
fn mark_told(store: &Store, run_id: &str) {
    let _claim = WAKES.lock().unwrap_or_else(|error| error.into_inner());
    if active(run_id) {
        return;
    }
    if let Some(mut record) = store.load(run_id)
        && record.status.settled()
        && !record.woken
    {
        record.woken = true;
        if let Err(reason) = store.save(&record) {
            crate::dbg_log!("workflow {run_id}: {reason}");
        }
    }
}

fn claim_wake_in(store: &Store, run_id: &str) -> Result<bool, String> {
    let _claim = WAKES.lock().unwrap_or_else(|error| error.into_inner());
    let Some(mut record) = store.load(run_id) else {
        return Err(format!("no run {run_id} in this thread"));
    };
    if !record.status.settled() || record.woken || active(run_id) {
        return Ok(false);
    }
    record.woken = true;
    store.save(&record)?;
    Ok(true)
}

/// A run as the app shows it: [`report`] without the result (which can be
/// large, and is read with `workflow_status`), plus whether it has one,
/// whether this engine drives it and whether its thread was told it ended.
fn summary(store: &Store, record: &RunRecord) -> Value {
    let mut answer = report(store, record);
    if let Some(map) = answer.as_object_mut() {
        for key in ["result", "result_truncated", "result_bytes", "result_file"] {
            map.remove(key);
        }
        map.insert("thread".to_owned(), json!(record.thread));
        map.insert("has_result".to_owned(), json!(record.result.is_some()));
        map.insert("woken".to_owned(), json!(record.woken));
        map.insert("driven".to_owned(), json!(active(&record.id)));
    }
    answer
}

/// One run of `thread` as the app shows it.
pub(crate) fn run_summary(thread: &str, run_id: &str) -> Option<Value> {
    let store = Store::for_thread(thread)?;
    let record = store.load(run_id)?;
    Some(summary(&store, &record))
}

/// The newest runs of `thread` as the app shows them.
pub(crate) fn run_summaries(thread: &str) -> Vec<Value> {
    let Some(store) = Store::for_thread(thread) else {
        return Vec::new();
    };
    store
        .list()
        .iter()
        .take(LISTED_RUNS)
        .map(|record| summary(&store, record))
        .collect()
}

/// A run as `workflow_status` tells it: phases, workers and the result.
fn report(store: &Store, record: &RunRecord) -> Value {
    let calls = store.calls(&record.id);
    let count = |state: CallState| calls.iter().filter(|call| call.state == state).count();
    let phases: Vec<Value> = record
        .phases
        .iter()
        .map(|phase| {
            let in_phase = calls
                .iter()
                .filter(|call| call.phase.as_deref() == Some(phase.title.as_str()));
            let mut workers = 0;
            let mut running = 0;
            let mut done = 0;
            let mut failed = 0;
            for call in in_phase {
                workers += 1;
                match call.state {
                    CallState::Running => running += 1,
                    CallState::Done => done += 1,
                    CallState::Failed => failed += 1,
                }
            }
            json!({
                "title": phase.title,
                "started_at": phase.started_at,
                "workers": workers,
                "running": running,
                "done": done,
                "failed": failed,
            })
        })
        .collect();
    let workers: Vec<Value> = calls
        .iter()
        .map(|call| {
            json!({
                "seq": call.seq,
                "phase": call.phase,
                "state": call.state,
                "prompt": clip(&call.prompt, 160),
                "harness": call.harness,
                "model": call.model,
                "session_id": call.session_id,
                "link_id": call.link_id,
                "started_at": call.started_at,
                "finished_at": call.finished_at,
                "error": call.error,
            })
        })
        .collect();
    let logs: Vec<&store::LogLine> = record
        .logs
        .iter()
        .skip(record.logs.len().saturating_sub(STATUS_LOG_LINES))
        .collect();
    let end = record.finished_at.unwrap_or_else(now_ms);
    let mut answer = json!({
        "run_id": record.id,
        "status": record.status,
        "name": record.name,
        "started_at": record.started_at,
        "finished_at": record.finished_at,
        "elapsed_ms": end.saturating_sub(record.started_at),
        "resumes": record.resumes,
        "limits": {
            "concurrency": record.limits.concurrency,
            "call_budget": record.limits.call_budget,
            "timeout_minutes": record.limits.timeout_secs / 60,
        },
        "calls": {
            "total": calls.len(),
            "running": count(CallState::Running),
            "done": count(CallState::Done),
            "failed": count(CallState::Failed),
        },
        "phases": phases,
        "workers": workers,
        "logs": logs,
        "error": record.error,
    });
    if record.status == RunStatus::Running && !active(&record.id) {
        answer["note"] = json!(
            "This run is not being driven right now: the engine restarted. It continues from its first unfinished call when its thread is next opened."
        );
    }
    if let Some(result) = record.result.as_ref() {
        let text = result.to_string();
        if text.len() <= MAX_INLINE_RESULT_BYTES {
            answer["result"] = result.clone();
        } else {
            answer["result_truncated"] = json!(true);
            answer["result_bytes"] = json!(text.len());
            if let Some(dir) = store.run_dir(&record.id) {
                answer["result_file"] = json!(dir.join("result.json").to_string_lossy());
            }
        }
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::run::{Fut, Worker, WorkerRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Echo {
        started: AtomicUsize,
    }

    impl AgentRunner for Echo {
        fn start<'a>(
            &'a self,
            _request: &'a WorkerRequest,
            _cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<Worker, String>> {
            Box::pin(async move {
                let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(Worker {
                    session_id: format!("session-{n}"),
                    link_id: format!("link-{n}"),
                })
            })
        }

        fn wait<'a>(
            &'a self,
            worker: &'a Worker,
            _cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<String, String>> {
            Box::pin(async move { Ok(format!("done by {}", worker.session_id)) })
        }

        fn abandon<'a>(&'a self, _worker: &'a Worker) -> Fut<'a, ()> {
            Box::pin(async {})
        }
    }

    fn echo() -> (Arc<Echo>, Arc<dyn AgentRunner>) {
        let echo = Arc::new(Echo {
            started: AtomicUsize::new(0),
        });
        (echo.clone(), echo)
    }

    async fn settled(store: &Store, id: &str) -> RunRecord {
        for _ in 0..200 {
            if let Some(record) = store.load(id)
                && record.status.settled()
                && !active(id)
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("run {id} did not settle");
    }

    #[test]
    fn limits_come_from_the_settings_and_a_call_can_only_lower_them() {
        let none = configured_limits(|_| None);
        assert_eq!(
            none,
            Limits {
                concurrency: 3,
                call_budget: 40,
                timeout_secs: 3600
            }
        );
        let set = configured_limits(|key| match key {
            "workflow-concurrency" => Some(100),
            "workflow-call-budget" => Some(7),
            "workflow-timeout-minutes" => Some(0),
            _ => None,
        });
        // Capped at the most, and never below one.
        assert_eq!(set.concurrency, MAX_CONCURRENCY as usize);
        assert_eq!(set.call_budget, 7);
        assert_eq!(set.timeout_secs, 60);
        let lower = lowered(
            none,
            &json!({"concurrency": 2, "call_budget": 5, "timeout_minutes": 10}),
        );
        assert_eq!(
            (lower.concurrency, lower.call_budget, lower.timeout_secs),
            (2, 5, 600)
        );
        let higher = lowered(
            none,
            &json!({"concurrency": 8, "call_budget": 500, "timeout_minutes": 999}),
        );
        assert_eq!(higher, none);
    }

    #[test]
    fn arguments_are_checked_before_a_run_starts() {
        let ok = |tool: &str, args: Value| validate(tool, &args);
        assert!(ok(RUN_TOOL_NAME, json!({"script": "return 1"})).is_ok());
        assert!(
            ok(
                RUN_TOOL_NAME,
                json!({"name": "review-pipeline", "args": {"n": 1}})
            )
            .is_ok()
        );
        assert!(ok(RUN_TOOL_NAME, json!({})).is_err());
        assert!(ok(RUN_TOOL_NAME, json!({"script": "x", "name": "y"})).is_err());
        assert!(ok(RUN_TOOL_NAME, json!({"script": "  "})).is_err());
        assert!(ok(RUN_TOOL_NAME, json!({"script": "x", "concurrency": 0})).is_err());
        assert!(ok(RUN_TOOL_NAME, json!({"script": "x", "concurrency": 9})).is_err());
        assert!(ok(RUN_TOOL_NAME, json!({"script": "x", "colour": 1})).is_err());
        for name in ["../x", "a/b", ".hidden", "a b", ""] {
            assert!(ok(RUN_TOOL_NAME, json!({"name": name})).is_err(), "{name}");
        }
        let long = "x".repeat(MAX_SCRIPT_BYTES + 1);
        assert!(ok(RUN_TOOL_NAME, json!({"script": long})).is_err());
        assert!(ok(STATUS_TOOL_NAME, json!({})).is_ok());
        assert!(
            ok(
                STATUS_TOOL_NAME,
                json!({"run_id": "wf-1-abcdef01", "wait_ms": 1000})
            )
            .is_ok()
        );
        assert!(ok(STATUS_TOOL_NAME, json!({"run_id": "../escape"})).is_err());
        assert!(ok(STATUS_TOOL_NAME, json!({"wait_ms": MAX_WAIT_MS + 1})).is_err());
    }

    #[tokio::test]
    async fn a_started_run_finishes_and_its_status_reports_phases_workers_and_result() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let (echo, runner) = echo();
        let limits = Limits {
            concurrency: 2,
            call_budget: 10,
            timeout_secs: 30,
        };
        let id = launch(
            store.clone(),
            runner,
            "1784626803679",
            Some("review".to_owned()),
            r#"
            phase("review");
            const found = await parallel(args.map((x) => () => agent("look at " + x)));
            phase("verify");
            return { found, count: found.length };
            "#
            .to_owned(),
            json!(["a", "b"]),
            limits,
        )
        .unwrap();
        let record = settled(&store, &id).await;
        assert_eq!(record.status, RunStatus::Done, "{:?}", record.error);
        assert_eq!(echo.started.load(Ordering::SeqCst), 2);

        let report = report(&store, &record);
        assert_eq!(report["status"], json!("done"));
        assert_eq!(report["name"], json!("review"));
        assert_eq!(report["calls"]["done"], json!(2));
        assert_eq!(report["phases"][0]["title"], json!("review"));
        assert_eq!(report["phases"][0]["done"], json!(2));
        assert_eq!(report["phases"][1]["workers"], json!(0));
        assert_eq!(report["workers"].as_array().unwrap().len(), 2);
        assert_eq!(report["workers"][0]["session_id"], json!("session-1"));
        assert_eq!(report["result"]["count"], json!(2));
        assert!(report.get("note").is_none());
        assert_eq!(listing(&store)["runs"][0]["run_id"], json!(id));
    }

    #[tokio::test]
    async fn a_restart_resumes_a_run_that_was_left_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let (echo, runner) = echo();
        // A run the previous process left behind: running, one call finished.
        let record = new_record(
            "1784626803679",
            None,
            Value::Null,
            Limits {
                concurrency: 2,
                call_budget: 10,
                timeout_secs: 30,
            },
        );
        let source = r#"return [await agent("one"), await agent("two")];"#;
        store.create(&record, source).unwrap();
        let hash = run::call_hash("one", &json!({}));
        store
            .append_call(
                &record.id,
                &store::CallRecord {
                    key: format!("{hash}#0"),
                    seq: 0,
                    phase: None,
                    prompt: "one".to_owned(),
                    harness: None,
                    model: None,
                    state: CallState::Done,
                    attempt: 0,
                    session_id: Some("old-session".to_owned()),
                    link_id: Some("old-link".to_owned()),
                    started_at: 1,
                    finished_at: Some(2),
                    result: Some(json!("cached answer")),
                    error: None,
                },
            )
            .unwrap();
        let before = report(&store, &store.load(&record.id).unwrap());
        assert!(before["note"].as_str().unwrap().contains("restarted"));

        assert_eq!(resume_in(&store, &runner), 1);
        let finished = settled(&store, &record.id).await;
        assert_eq!(finished.status, RunStatus::Done, "{:?}", finished.error);
        assert_eq!(
            finished.result,
            Some(json!(["cached answer", "done by session-1"]))
        );
        assert_eq!(finished.resumes, 1);
        // Only "two" started a worker.
        assert_eq!(echo.started.load(Ordering::SeqCst), 1);
        // A finished run is not picked up again.
        assert_eq!(resume_in(&store, &runner), 0);
    }

    #[tokio::test]
    async fn status_waits_for_a_run_to_end_and_answers_at_the_deadline_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let record = new_record(
            "1784626803679",
            None,
            Value::Null,
            Limits {
                concurrency: 1,
                call_budget: 1,
                timeout_secs: 30,
            },
        );
        store.create(&record, "return 1").unwrap();
        let cancel = CancellationToken::new();
        let args = json!({"run_id": record.id, "wait_ms": 300});
        // Nothing drives it: the wait ends at its deadline with the run running.
        let waited = status_in(&store, &args, &cancel).await.unwrap();
        let waited: Value = serde_json::from_str(&waited).unwrap();
        assert_eq!(waited["status"], json!("running"));

        // Settled while a wait is open: the wait answers with the end.
        let settle = {
            let store = store.clone();
            let id = record.id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let mut record = store.load(&id).unwrap();
                record.status = RunStatus::Done;
                record.result = Some(json!({"answer": 42}));
                store.save(&record).unwrap();
            })
        };
        let args = json!({"run_id": record.id, "wait_ms": 20_000});
        let started = tokio::time::Instant::now();
        let ended = status_in(&store, &args, &cancel).await.unwrap();
        settle.await.unwrap();
        let ended: Value = serde_json::from_str(&ended).unwrap();
        assert_eq!(ended["status"], json!("done"));
        assert_eq!(ended["result"], json!({"answer": 42}));
        assert!(started.elapsed() < Duration::from_secs(10));

        let missing = status_in(&store, &json!({"run_id": "wf-0-00000000"}), &cancel).await;
        assert!(missing.unwrap_err().contains("no run"));
        let listed = status_in(&store, &json!({}), &cancel).await.unwrap();
        assert!(listed.contains(&record.id));
    }

    /// A runner whose workers wait until the run is stopped.
    struct Hang {
        started: AtomicUsize,
        abandoned: AtomicUsize,
    }

    impl AgentRunner for Hang {
        fn start<'a>(
            &'a self,
            _request: &'a WorkerRequest,
            _cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<Worker, String>> {
            Box::pin(async move {
                let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(Worker {
                    session_id: format!("session-{n}"),
                    link_id: format!("link-{n}"),
                })
            })
        }

        fn wait<'a>(
            &'a self,
            _worker: &'a Worker,
            cancel: &'a CancellationToken,
        ) -> Fut<'a, Result<String, String>> {
            Box::pin(async move {
                cancel.cancelled().await;
                Err("the run was stopped".to_owned())
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
            concurrency: 2,
            call_budget: 10,
            timeout_secs: 60,
        }
    }

    #[tokio::test]
    async fn stopping_a_run_ends_its_script_and_interrupts_its_workers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let hang = Arc::new(Hang {
            started: AtomicUsize::new(0),
            abandoned: AtomicUsize::new(0),
        });
        let id = launch(
            store.clone(),
            hang.clone(),
            "1784626803679",
            None,
            r#"
            phase("review");
            await parallel([() => agent("one"), () => agent("two")]);
            await agent("never started");
            "#
            .to_owned(),
            Value::Null,
            limits(),
        )
        .unwrap();
        // Both workers are running.
        for _ in 0..200 {
            if hang.started.load(Ordering::SeqCst) == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(hang.started.load(Ordering::SeqCst), 2);

        let stopped = stop_in(&store, &id).await.unwrap();
        assert_eq!(stopped["status"], json!("cancelled"));
        assert_eq!(stopped["error"], json!("the run was stopped"));
        assert_eq!(stopped["driven"], json!(false));
        assert!(stopped.get("result").is_none());
        // The workers it left running were interrupted, and no third call began.
        assert_eq!(hang.abandoned.load(Ordering::SeqCst), 2);
        assert_eq!(hang.started.load(Ordering::SeqCst), 2);
        let record = store.load(&id).unwrap();
        assert_eq!(record.status, RunStatus::Cancelled);
        assert!(record.finished_at.is_some());
        // A stopped run is not picked up by a restart.
        let runner: Arc<dyn AgentRunner> = hang;
        assert_eq!(resume_in(&store, &runner), 0);
        // Stopping again answers the run as it is.
        let again = stop_in(&store, &id).await.unwrap();
        assert_eq!(again["status"], json!("cancelled"));
        assert!(stop_in(&store, "wf-0-00000000").await.is_err());
    }

    #[tokio::test]
    async fn stopping_a_run_nothing_drives_ends_it_on_its_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let record = new_record("1784626803679", None, Value::Null, limits());
        store.create(&record, "return 1").unwrap();
        let stopped = stop_in(&store, &record.id).await.unwrap();
        assert_eq!(stopped["status"], json!("cancelled"));
        assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Cancelled);
        let (_, runner) = echo();
        assert_eq!(resume_in(&store, &runner), 0);
    }

    #[tokio::test]
    async fn only_one_client_claims_the_wake_of_a_run_that_ended() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let record = new_record("1784626803679", None, Value::Null, limits());
        store.create(&record, "return 1").unwrap();
        // Still running: nothing to wake the thread for.
        assert!(!claim_wake_in(&store, &record.id).unwrap());
        stop_in(&store, &record.id).await.unwrap();
        assert!(
            !summary(&store, &store.load(&record.id).unwrap())["woken"]
                .as_bool()
                .unwrap()
        );
        assert!(claim_wake_in(&store, &record.id).unwrap());
        assert!(!claim_wake_in(&store, &record.id).unwrap());
        assert!(
            summary(&store, &store.load(&record.id).unwrap())["woken"]
                .as_bool()
                .unwrap()
        );
        assert!(claim_wake_in(&store, "wf-0-00000000").is_err());
    }

    #[tokio::test]
    async fn every_change_of_a_run_is_announced_and_a_summary_leaves_the_result_out() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let mut heard = changes();
        let (_, runner) = echo();
        let id = launch(
            store.clone(),
            runner,
            "1784626803679",
            Some("review".to_owned()),
            r#"phase("one"); return await agent("look");"#.to_owned(),
            Value::Null,
            limits(),
        )
        .unwrap();
        let record = settled(&store, &id).await;
        let mut announced = 0;
        while let Ok(change) = heard.try_recv() {
            if change.run_id == id {
                assert_eq!(change.thread, "1784626803679");
                announced += 1;
            }
        }
        // The phase, the call's start and end, and the end of the run.
        assert!(announced >= 4, "{announced}");
        let summary = summary(&store, &record);
        assert_eq!(summary["status"], json!("done"));
        assert_eq!(summary["has_result"], json!(true));
        assert_eq!(summary["thread"], json!("1784626803679"));
        assert!(summary.get("result").is_none());
        assert_eq!(summary["workers"][0]["state"], json!("done"));
    }

    #[tokio::test]
    async fn an_agent_that_read_how_a_run_ended_is_not_woken_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("workflows"));
        let record = new_record("1784626803679", None, Value::Null, limits());
        store.create(&record, "return 1").unwrap();
        let cancel = CancellationToken::new();
        // Read while it runs: nothing is told yet.
        let args = json!({"run_id": record.id});
        status_in(&store, &args, &cancel).await.unwrap();
        assert!(!store.load(&record.id).unwrap().woken);
        stop_in(&store, &record.id).await.unwrap();
        assert!(!store.load(&record.id).unwrap().woken);
        // Read after it ended: the agent has the answer.
        status_in(&store, &args, &cancel).await.unwrap();
        assert!(store.load(&record.id).unwrap().woken);
        assert!(!claim_wake_in(&store, &record.id).unwrap());
    }

    #[test]
    fn stop_workflow_needs_a_run_id() {
        assert!(validate(STOP_TOOL_NAME, &json!({"run_id": "wf-1-abcdef01"})).is_ok());
        assert!(validate(STOP_TOOL_NAME, &json!({})).is_err());
        assert!(validate(STOP_TOOL_NAME, &json!({"run_id": "../escape"})).is_err());
        assert!(validate(STOP_TOOL_NAME, &json!({"run_id": "wf-1-abcdef01", "x": 1})).is_err());
        assert!(is_tool(STOP_TOOL_NAME));
    }
}
