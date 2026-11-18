//! `_rustcode/loops/*`: the app's Loops page (`docs/daemon.md` › Loops).
//!
//! A Loop is a daemon job whose action is [`JobAction::Loop`]: a prompt run on
//! a schedule, each run in a fresh thread on the app's engine. These methods
//! go through the same client and store as `rustcode cron` and
//! `_rustcode/scheduled/*`, so `rustcode cron list` shows the same jobs. Run
//! now and delete are `_rustcode/scheduled/run` and `_rustcode/scheduled/delete`.
//!
//! Unlike the scheduled methods, `start_daemon` starts the scheduler: the
//! Loops page offers it as a button the user presses.

use serde_json::{Value, json};

use crate::daemon::model::{JobAction, JobRecord, JobRunRecord, MisfirePolicy, ScheduleSpec};
use crate::daemon::{
    client::DaemonClient,
    command::create_job,
    lifecycle::DaemonLifecycle,
    protocol::{DaemonRequest, DaemonResponse},
};

/// Runs each Loop in the list carries: the page shows the last result and a
/// short history, and the review list looks back seven days.
const HISTORY_RUNS: usize = 20;

/// `_rustcode/loops/list`: every Loop with its recent runs, and whether the
/// scheduler answered.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsListRequest {}

/// `_rustcode/loops/save`: create a Loop, or edit the one `id` names.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/save", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsSaveRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub workspace: String,
    pub prompt: String,
    /// `{ "kind": "cron", "expression", "timezone"? }` or
    /// `{ "kind": "at", "at" }`, as `_rustcode/scheduled/create` takes it.
    pub schedule: Value,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default = "read_only_default")]
    pub read_only: bool,
    /// Send every run to this thread instead of starting a fresh one.
    #[serde(default)]
    pub thread_id: Option<String>,
}

fn read_only_default() -> bool {
    true
}

/// `_rustcode/loops/set_paused`: pause or resume a Loop.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/set_paused", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsSetPausedRequest {
    pub id: String,
    pub paused: bool,
}

/// `_rustcode/loops/start_daemon`: start the scheduler, as
/// `rustcode daemon start` does.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/start_daemon", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsStartDaemonRequest {}

fn lifecycle() -> Result<DaemonLifecycle, String> {
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    Ok(DaemonLifecycle::new(config_dir))
}

fn client() -> Result<DaemonClient, String> {
    Ok(DaemonClient::new(lifecycle()?.socket_path()))
}

async fn ask(client: &DaemonClient, request: DaemonRequest) -> Result<DaemonResponse, String> {
    client.request(request).await.map_err(|error| {
        format!("the scheduler is not running ({error}); start it from the Loops page")
    })
}

/// What a run's output says about it: the thread it started, or why it was
/// skipped (`daemon::executor::loop_outcome`).
fn run_json(run: &JobRunRecord) -> Value {
    let output: Value = run
        .output
        .as_deref()
        .and_then(|output| serde_json::from_str(output).ok())
        .unwrap_or(Value::Null);
    json!({
        "id": run.id,
        "state": run.state,
        "scheduledAt": run.scheduled_at,
        "startedAt": run.started_at,
        "finishedAt": run.finished_at,
        "error": run.result_summary.as_ref().filter(|_| run.error_class.is_some()),
        "errorClass": run.error_class,
        "threadId": output.get("thread_id"),
        "quiet": output.get("quiet").and_then(Value::as_bool).unwrap_or(false),
        "skipped": output.get("skipped"),
    })
}

/// A Loop as the page shows it. `None` for a job that is not a Loop.
pub(crate) fn loop_json(job: &JobRecord, runs: &[JobRunRecord]) -> Option<Value> {
    let JobAction::Loop {
        prompt,
        workspace,
        harness,
        account,
        model,
        effort,
        read_only,
        thread_id,
    } = &job.action
    else {
        return None;
    };
    Some(json!({
        "id": job.id,
        "name": job.name,
        "paused": job.paused,
        "schedule": job.schedule,
        "workspace": workspace,
        "prompt": prompt,
        "harness": harness,
        "account": account,
        "model": model,
        "effort": effort,
        "readOnly": read_only,
        "threadId": thread_id,
        "nextDueAt": job.next_due_at,
        "createdAt": job.created_at,
        "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
    }))
}

pub(crate) async fn list() -> Result<Value, String> {
    list_with(&client()?).await
}

async fn list_with(client: &DaemonClient) -> Result<Value, String> {
    let Ok(DaemonResponse::Jobs { jobs }) = ask(client, DaemonRequest::List).await else {
        return Ok(json!({ "daemonRunning": false, "loops": [] }));
    };
    let mut loops = Vec::new();
    for job in jobs
        .iter()
        .filter(|job| matches!(job.action, JobAction::Loop { .. }))
        .take(super::scheduled::MAX_TASKS)
    {
        let runs = match ask(
            client,
            DaemonRequest::History {
                job_id: job.id.clone(),
                limit: HISTORY_RUNS,
            },
        )
        .await?
        {
            DaemonResponse::History { runs } => runs,
            _ => Vec::new(),
        };
        loops.extend(loop_json(job, &runs));
    }
    Ok(json!({ "daemonRunning": true, "loops": loops }))
}

/// The schedule a Loop runs on: what `schedule_task` takes, with a missed
/// cron run firing once when the machine wakes instead of being skipped.
pub(crate) fn loop_schedule(value: &Value) -> Result<ScheduleSpec, String> {
    let mut spec = super::scheduled::schedule_spec(value)?;
    if let ScheduleSpec::Cron { misfire_policy, .. } = &mut spec {
        *misfire_policy = MisfirePolicy::RunOnce;
    }
    Ok(spec)
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The job a save request describes, checked before the daemon sees it.
pub(crate) fn job_from(request: LoopsSaveRequest, id: &str) -> Result<JobRecord, String> {
    let prompt = request.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("a Loop needs a prompt".to_owned());
    }
    let name = request.name.trim().to_owned();
    if name.is_empty() {
        return Err("a Loop needs a name".to_owned());
    }
    let workspace = std::path::Path::new(&request.workspace);
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err(format!("{} is not a project folder", request.workspace));
    }
    let schedule = loop_schedule(&request.schedule)?;
    let action = JobAction::Loop {
        prompt,
        workspace: request.workspace.clone(),
        harness: trimmed(request.harness),
        account: trimmed(request.account),
        model: trimmed(request.model),
        effort: trimmed(request.effort),
        read_only: request.read_only,
        thread_id: trimmed(request.thread_id),
    };
    let schedule_json =
        serde_json::to_string(&schedule).map_err(|error| format!("schedule: {error}"))?;
    let action_json = serde_json::to_string(&action).map_err(|error| format!("action: {error}"))?;
    create_job(
        id,
        &name,
        &request.workspace,
        &schedule_json,
        &action_json,
        None,
        None,
    )
    .map_err(|error| error.message)
}

pub(crate) async fn save(request: LoopsSaveRequest) -> Result<Value, String> {
    save_with(&client()?, request).await
}

async fn save_with(client: &DaemonClient, request: LoopsSaveRequest) -> Result<Value, String> {
    let existing = trimmed(request.id.clone());
    let id = existing.clone().unwrap_or_else(|| {
        format!(
            "loop-{}-{:04x}",
            chrono::Utc::now().timestamp_millis(),
            rand::random::<u16>()
        )
    });
    let job = job_from(request, &id)?;
    let request = match existing {
        Some(_) => DaemonRequest::Update { job },
        None => DaemonRequest::Create { job },
    };
    match ask(client, request).await? {
        DaemonResponse::Job { job } => Ok(json!({ "loop": loop_json(&job, &[]) })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

pub(crate) async fn set_paused(request: LoopsSetPausedRequest) -> Result<Value, String> {
    set_paused_with(&client()?, request).await
}

async fn set_paused_with(
    client: &DaemonClient,
    request: LoopsSetPausedRequest,
) -> Result<Value, String> {
    match ask(
        client,
        DaemonRequest::SetPaused {
            job_id: request.id.clone(),
            paused: request.paused,
        },
    )
    .await?
    {
        DaemonResponse::Ack => Ok(json!({ "id": request.id, "paused": request.paused })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

/// Start the scheduler the way `rustcode daemon start` does: this same binary
/// running `daemon run`, detached, with this config directory.
pub(crate) async fn start_daemon() -> Result<Value, String> {
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    let exe = std::env::current_exe().map_err(|error| format!("no RustCode binary: {error}"))?;
    let mut command = tokio::process::Command::new(exe);
    command
        .args(["daemon", "run"])
        .env("RUSTCODE_CONFIG_DIR", &config_dir);
    let status = lifecycle()?
        .start(command)
        .await
        .map_err(|error| format!("the scheduler did not start: {error}"))?;
    Ok(json!({ "daemonRunning": true, "pid": status.pid }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(workspace: &str) -> LoopsSaveRequest {
        LoopsSaveRequest {
            id: None,
            name: " CI watch ".into(),
            workspace: workspace.into(),
            prompt: " Check CI on main ".into(),
            schedule: json!({"kind": "cron", "expression": "0 */4 * * *", "timezone": "Europe/Oslo"}),
            harness: Some("codex".into()),
            account: Some(" work ".into()),
            model: Some("  ".into()),
            effort: None,
            read_only: true,
            thread_id: None,
        }
    }

    #[test]
    fn a_missed_cron_run_fires_once() {
        let spec = loop_schedule(
            &json!({"kind": "cron", "expression": "0 9 * * *", "timezone": "Europe/Oslo"}),
        )
        .unwrap();
        assert_eq!(spec.misfire_policy(), MisfirePolicy::RunOnce);
    }

    #[test]
    fn a_saved_loop_is_a_trimmed_read_only_loop_job() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let job = job_from(request(&workspace), "loop-1").unwrap();
        assert_eq!(job.id, "loop-1");
        assert_eq!(job.name, "CI watch");
        match &job.action {
            JobAction::Loop {
                prompt,
                harness,
                model,
                read_only,
                ..
            } => {
                assert_eq!(prompt, "Check CI on main");
                assert_eq!(harness.as_deref(), Some("codex"));
                assert_eq!(model, &None);
                assert!(read_only);
            }
            other => panic!("{other:?}"),
        }
        let shown = loop_json(&job, &[]).unwrap();
        assert_eq!(shown["readOnly"], true);
        assert_eq!(shown["account"], "work");
        assert_eq!(shown["prompt"], "Check CI on main");
        assert_eq!(shown["threadId"], Value::Null);
    }

    #[test]
    fn a_loop_can_send_every_run_to_one_thread() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let mut saved = request(&workspace);
        saved.thread_id = Some(" thread-7 ".into());
        let job = job_from(saved, "loop-1").unwrap();
        assert!(matches!(
            &job.action,
            JobAction::Loop { thread_id: Some(id), .. } if id == "thread-7"
        ));
        assert_eq!(loop_json(&job, &[]).unwrap()["threadId"], "thread-7");
    }

    #[test]
    fn a_loop_needs_a_prompt_a_name_and_a_folder() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let mut empty = request(&workspace);
        empty.prompt = "  ".into();
        assert!(job_from(empty, "x").is_err());
        let mut nameless = request(&workspace);
        nameless.name = String::new();
        assert!(job_from(nameless, "x").is_err());
        assert!(job_from(request("relative/path"), "x").is_err());
    }

    #[test]
    fn a_run_shows_its_thread_or_why_it_was_skipped() {
        let now = chrono::Utc::now();
        let mut run = JobRunRecord {
            id: "run-1".into(),
            job_id: "loop-1".into(),
            schedule_revision: 1,
            idempotency_key: "key".into(),
            scheduled_at: now,
            state: crate::daemon::model::JobRunState::Succeeded,
            attempt: 1,
            lease_owner: None,
            lease_fence: 1,
            lease_expires_at: None,
            started_at: Some(now),
            finished_at: Some(now),
            result_summary: Some("The run's thread finished".into()),
            error_class: None,
            output: Some(r#"{"thread_id":"t-1"}"#.into()),
        };
        let shown = run_json(&run);
        assert_eq!(shown["threadId"], "t-1");
        assert!(shown["error"].is_null());
        run.state = crate::daemon::model::JobRunState::Cancelled;
        run.error_class = Some("cancelled".into());
        run.result_summary = None;
        run.output = Some(r#"{"skipped":"RustCode was not open"}"#.into());
        let skipped = run_json(&run);
        assert_eq!(skipped["skipped"], "RustCode was not open");
        assert!(skipped["threadId"].is_null());
    }

    mod daemon {
        use super::*;

        struct Running {
            task: tokio::task::JoinHandle<anyhow::Result<()>>,
            shutdown: std::sync::Arc<tokio::sync::Notify>,
            lifecycle: DaemonLifecycle,
            _directory: tempfile::TempDir,
        }

        impl Running {
            fn new() -> Self {
                let directory = crate::daemon::test_directory();
                let lifecycle = DaemonLifecycle::new(directory.path());
                let server = lifecycle.bind().unwrap();
                let shutdown = server.shutdown_handle();
                Self {
                    shutdown,
                    task: tokio::spawn(server.run()),
                    lifecycle,
                    _directory: directory,
                }
            }

            fn client(&self) -> DaemonClient {
                DaemonClient::new(self.lifecycle.socket_path())
            }
        }

        impl Drop for Running {
            fn drop(&mut self) {
                self.shutdown.notify_one();
                self.task.abort();
            }
        }

        #[tokio::test]
        async fn a_loop_is_saved_edited_paused_and_listed_through_the_daemon() {
            let running = Running::new();
            let client = running.client();
            let folder = tempfile::tempdir().unwrap();
            let workspace = folder.path().to_string_lossy().into_owned();

            let created = save_with(&client, request(&workspace)).await.unwrap();
            let id = created["loop"]["id"].as_str().unwrap().to_owned();
            assert!(id.starts_with("loop-"), "{id}");

            let mut edit = request(&workspace);
            edit.id = Some(id.clone());
            edit.name = "Nightly CI".into();
            edit.read_only = false;
            let edited = save_with(&client, edit).await.unwrap();
            assert_eq!(edited["loop"]["id"], id.as_str());
            assert_eq!(edited["loop"]["name"], "Nightly CI");
            assert_eq!(edited["loop"]["readOnly"], false);

            set_paused_with(
                &client,
                LoopsSetPausedRequest {
                    id: id.clone(),
                    paused: true,
                },
            )
            .await
            .unwrap();
            let listed = list_with(&client).await.unwrap();
            assert_eq!(listed["daemonRunning"], true);
            let loops = listed["loops"].as_array().unwrap();
            assert_eq!(loops.len(), 1);
            assert_eq!(loops[0]["paused"], true);
            assert_eq!(loops[0]["name"], "Nightly CI");
            assert!(loops[0]["runs"].as_array().unwrap().is_empty());
        }

        #[tokio::test]
        async fn a_list_with_nothing_listening_says_so() {
            let directory = crate::daemon::test_directory();
            let client = DaemonClient::new(DaemonLifecycle::new(directory.path()).socket_path());
            let answer = list_with(&client).await.unwrap();
            assert_eq!(answer["daemonRunning"], false);
            assert!(answer["loops"].as_array().unwrap().is_empty());
        }
    }
}
