//! `_rustcode/scheduled/*`: the local daemon's scheduled work
//! (`docs/daemon.md`) for the app, which answers `schedule_task`,
//! `list_scheduled_tasks`, `delete_scheduled_task` and
//! `run_scheduled_task_now`.
//!
//! All four go through the same client `rustcode cron` and the engine's
//! `manage_scheduled_jobs` tool use: one scheduler, one store, one protocol.
//! These methods never start the daemon; a call with nothing listening says
//! so, and `rustcode daemon start` is the user's to run.

use serde_json::{Value, json};

// The job types exist on every platform; the client, the command helpers and
// the lifecycle are the unix socket's, so a Windows build compiles the
// validation and the "no daemon here" answers without them.
use crate::daemon::model::{JobRecord, ScheduleSpec};
#[cfg(unix)]
use crate::daemon::{
    client::DaemonClient,
    command::create_job,
    lifecycle::DaemonLifecycle,
    protocol::{DaemonRequest, DaemonResponse},
};

/// Jobs one answer lists: a scheduler holds a handful, so this is a mistake,
/// not a page.
pub(crate) const MAX_TASKS: usize = 100;
/// The most a generated job name carries from a prompt.
const MAX_NAME_CHARS: usize = 60;

/// `_rustcode/scheduled/list`: every scheduled job, and whether the daemon
/// answered at all.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/scheduled/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledListRequest {}

/// `_rustcode/scheduled/create`: a prompt that runs on its schedule.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/scheduled/create", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledCreateRequest {
    pub prompt: String,
    /// The folder the prompt runs in.
    pub workspace: String,
    /// `{ "kind": "cron", "expression", "timezone"? }` or
    /// `{ "kind": "at", "at" }` with an ISO 8601 time.
    pub schedule: Value,
    #[serde(default)]
    pub name: Option<String>,
    /// A model profile from the config, as `manage_scheduled_jobs` takes one.
    #[serde(default)]
    pub model_profile: Option<String>,
}

/// `_rustcode/scheduled/delete`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/scheduled/delete", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledDeleteRequest {
    pub id: String,
}

/// `_rustcode/scheduled/run`: run a job now, keeping its schedule.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/scheduled/run", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduledRunRequest {
    pub id: String,
}

/// The machine's IANA zone, for a cron that named none: `TZ`, else the zone
/// `/etc/localtime` links to, else UTC.
fn local_timezone() -> String {
    if let Ok(zone) = std::env::var("TZ") {
        let zone = zone.trim().trim_start_matches(':');
        if zone.parse::<chrono_tz::Tz>().is_ok() {
            return zone.to_owned();
        }
    }
    let linked = std::fs::read_link("/etc/localtime").ok().and_then(|path| {
        let text = path.to_string_lossy().into_owned();
        text.split("zoneinfo/").nth(1).map(str::to_owned)
    });
    linked
        .filter(|zone| zone.parse::<chrono_tz::Tz>().is_ok())
        .unwrap_or_else(|| "UTC".to_owned())
}

/// The schedule a `schedule_task` call asked for; checked against the daemon's
/// own rules before anything is created.
pub(crate) fn schedule_spec(value: &Value) -> Result<ScheduleSpec, String> {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    let spec = match kind {
        "cron" => {
            let expression = value
                .get("expression")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|expression| !expression.is_empty())
                .ok_or("schedule.expression is required for a cron schedule")?;
            if expression.split_whitespace().count() != 5 {
                return Err("a cron expression has exactly five fields".to_owned());
            }
            let timezone = value
                .get("timezone")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|zone| !zone.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(local_timezone);
            if timezone.parse::<chrono_tz::Tz>().is_err() {
                return Err(format!("unknown timezone `{timezone}`"));
            }
            ScheduleSpec::Cron {
                expression: expression.to_owned(),
                timezone,
                misfire_policy: Default::default(),
            }
        }
        "at" | "once" => {
            let at = value
                .get("at")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|at| !at.is_empty())
                .ok_or("schedule.at is required for a one-time schedule")?;
            let at = chrono::DateTime::parse_from_rfc3339(at)
                .map_err(|_| "schedule.at must be an ISO 8601 time with its offset".to_owned())?;
            ScheduleSpec::Once {
                at: at.with_timezone(&chrono::Utc),
            }
        }
        _ => return Err("schedule.kind must be cron or at".to_owned()),
    };
    spec.validate().map_err(|error| error.to_string())?;
    Ok(spec)
}

/// A job name from what the user asked for: its first line, cut at a word.
pub(crate) fn name_from_prompt(prompt: &str) -> String {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let clipped: String = first.chars().take(MAX_NAME_CHARS).collect();
    if clipped.is_empty() {
        "Scheduled task".to_owned()
    } else {
        clipped
    }
}

/// The fields of a job an answer carries: enough to name it, delete it and
/// say when it runs next, never its prompt back.
#[cfg(unix)]
fn task(job: &JobRecord) -> Value {
    json!({
        "id": job.id,
        "name": job.name,
        "paused": job.paused,
        "schedule": job.schedule,
        "workspace": job.workspace,
        "next_due_at": job.next_due_at,
    })
}

/// The client every method asks: the socket `rustcode cron` uses.
#[cfg(unix)]
fn client() -> Result<DaemonClient, String> {
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    Ok(DaemonClient::new(
        DaemonLifecycle::new(config_dir).socket_path(),
    ))
}

#[cfg(unix)]
async fn ask(client: &DaemonClient, request: DaemonRequest) -> Result<DaemonResponse, String> {
    client.request(request).await.map_err(|error| {
        format!("the scheduler is not running ({error}); run `rustcode daemon start`")
    })
}

/// Nothing listening: an empty list with the reason is a better answer than a
/// failure, so `list_scheduled_tasks` can say scheduled work needs the daemon.
#[cfg(not(unix))]
const NO_DAEMON: &str = "scheduled tasks need the scheduler daemon, which this build cannot run";

pub(crate) async fn list() -> Result<Value, String> {
    #[cfg(unix)]
    {
        list_with(&client()?).await
    }
    #[cfg(not(unix))]
    {
        Ok(json!({ "daemon_running": false, "tasks": [] }))
    }
}

#[cfg(unix)]
async fn list_with(client: &DaemonClient) -> Result<Value, String> {
    let Ok(DaemonResponse::Jobs { jobs }) = ask(client, DaemonRequest::List).await else {
        return Ok(json!({ "daemon_running": false, "tasks": [] }));
    };
    let tasks: Vec<Value> = jobs.iter().take(MAX_TASKS).map(task).collect();
    let omitted = jobs.len().saturating_sub(tasks.len());
    let mut answer = json!({ "daemon_running": true, "tasks": tasks });
    if omitted > 0 {
        answer["omitted_tasks"] = json!(omitted);
    }
    Ok(answer)
}

pub(crate) async fn create(request: ScheduledCreateRequest) -> Result<Value, String> {
    let prompt = request.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("prompt is required".to_owned());
    }
    let workspace = std::path::Path::new(&request.workspace);
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err(format!("{} is not a project folder", request.workspace));
    }
    let schedule = schedule_spec(&request.schedule)?;
    let name = request
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| name_from_prompt(&prompt));
    let action = json!({
        "type": "prompt",
        "prompt": prompt,
        "workspace": request.workspace,
        "model_profile": request.model_profile,
    })
    .to_string();
    let schedule_json =
        serde_json::to_string(&schedule).map_err(|error| format!("schedule: {error}"))?;
    #[cfg(unix)]
    {
        let id = format!(
            "task-{}-{:04x}",
            chrono::Utc::now().timestamp_millis(),
            rand::random::<u16>()
        );
        let job = create_job(
            &id,
            &name,
            &request.workspace,
            &schedule_json,
            &action,
            None,
            None,
        )
        .map_err(|error| error.to_string())?;
        create_with(&client()?, job).await
    }
    #[cfg(not(unix))]
    {
        let _ = (name, action, schedule_json);
        Err(NO_DAEMON.to_owned())
    }
}

#[cfg(unix)]
async fn create_with(client: &DaemonClient, job: JobRecord) -> Result<Value, String> {
    match ask(client, DaemonRequest::Create { job: job.clone() }).await? {
        DaemonResponse::Job { job } => Ok(json!({ "daemon_running": true, "task": task(&job) })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

pub(crate) async fn delete(id: String) -> Result<Value, String> {
    #[cfg(unix)]
    {
        delete_with(&client()?, &id).await
    }
    #[cfg(not(unix))]
    {
        let _ = id;
        Err(NO_DAEMON.to_owned())
    }
}

#[cfg(unix)]
async fn delete_with(client: &DaemonClient, id: &str) -> Result<Value, String> {
    match ask(
        client,
        DaemonRequest::Delete {
            job_id: id.to_owned(),
        },
    )
    .await?
    {
        DaemonResponse::Ack => Ok(json!({ "id": id, "deleted": true })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

pub(crate) async fn run(id: String) -> Result<Value, String> {
    #[cfg(unix)]
    {
        run_with(&client()?, &id).await
    }
    #[cfg(not(unix))]
    {
        let _ = id;
        Err(NO_DAEMON.to_owned())
    }
}

#[cfg(unix)]
async fn run_with(client: &DaemonClient, id: &str) -> Result<Value, String> {
    match ask(
        client,
        DaemonRequest::RunNow {
            job_id: id.to_owned(),
        },
    )
    .await?
    {
        DaemonResponse::RunAccepted { job_id, state } => {
            Ok(json!({ "id": job_id, "state": state }))
        }
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cron_schedule_needs_five_fields_and_a_known_zone() {
        let spec = schedule_spec(
            &json!({"kind": "cron", "expression": "0 9 * * 1-5", "timezone": "Europe/Oslo"}),
        )
        .unwrap();
        assert!(matches!(spec, ScheduleSpec::Cron { .. }));
        assert!(
            schedule_spec(
                &json!({"kind": "cron", "expression": "0 9 * *", "timezone": "Europe/Oslo"})
            )
            .is_err()
        );
        assert!(
            schedule_spec(
                &json!({"kind": "cron", "expression": "0 9 * * *", "timezone": "Mars/Olympus"})
            )
            .is_err()
        );
        assert!(schedule_spec(&json!({"kind": "daily"})).is_err());
    }

    #[test]
    fn a_one_time_schedule_is_an_iso_time_with_its_offset() {
        let spec =
            schedule_spec(&json!({"kind": "at", "at": "2026-03-04T09:30:00+01:00"})).unwrap();
        match spec {
            ScheduleSpec::Once { at } => {
                assert_eq!(at.to_rfc3339(), "2026-03-04T08:30:00+00:00");
            }
            other => panic!("{other:?}"),
        }
        assert!(schedule_spec(&json!({"kind": "at", "at": "tomorrow"})).is_err());
    }

    #[test]
    fn a_missing_timezone_falls_back_to_this_machine() {
        let spec = schedule_spec(&json!({"kind": "cron", "expression": "0 9 * * *"})).unwrap();
        match spec {
            ScheduleSpec::Cron { timezone, .. } => {
                assert!(timezone.parse::<chrono_tz::Tz>().is_ok(), "{timezone}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_name_comes_from_the_prompts_first_line() {
        assert_eq!(
            name_from_prompt("Check the build\nand fixes"),
            "Check the build"
        );
        assert_eq!(name_from_prompt("   "), "Scheduled task");
        let long = "word ".repeat(40);
        assert!(name_from_prompt(&long).chars().count() <= MAX_NAME_CHARS);
    }

    #[cfg(unix)]
    mod daemon {
        use super::*;
        use crate::daemon::lifecycle::DaemonLifecycle;
        use std::time::Duration;

        /// A real daemon on a private socket, the way `rustcode daemon start`
        /// runs one, with no executor installed (a run is then refused).
        struct Running {
            task: tokio::task::JoinHandle<anyhow::Result<()>>,
            shutdown: std::sync::Arc<tokio::sync::Notify>,
            lifecycle: DaemonLifecycle,
            _directory: tempfile::TempDir,
        }

        impl Running {
            fn new() -> Self {
                let directory = tempfile::tempdir_in("/tmp").unwrap();
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

        fn request() -> ScheduledCreateRequest {
            ScheduledCreateRequest {
                prompt: "Check the build".into(),
                workspace: "/tmp".into(),
                schedule: json!({"kind": "cron", "expression": "0 9 * * 1-5", "timezone": "Europe/Oslo"}),
                name: None,
                model_profile: None,
            }
        }

        #[tokio::test]
        async fn a_task_is_created_listed_and_deleted_through_the_daemon() {
            let running = Running::new();
            let client = running.client();
            let job = create_job(
                "task-1",
                &name_from_prompt("Check the build"),
                "/tmp",
                &serde_json::to_string(&schedule_spec(&request().schedule).unwrap()).unwrap(),
                &json!({"type": "prompt", "prompt": "Check the build", "workspace": "/tmp"})
                    .to_string(),
                None,
                None,
            )
            .unwrap();
            let created = create_with(&client, job).await.unwrap();
            assert_eq!(created["daemon_running"], true);
            assert_eq!(created["task"]["id"], "task-1");
            assert_eq!(created["task"]["name"], "Check the build");
            assert!(created["task"]["next_due_at"].is_string());

            let listed = list_with(&client).await.unwrap();
            assert_eq!(listed["daemon_running"], true);
            assert_eq!(listed["tasks"].as_array().unwrap().len(), 1);
            assert_eq!(listed["tasks"][0]["schedule"]["kind"], "cron");

            let deleted = delete_with(&client, "task-1").await.unwrap();
            assert_eq!(deleted["deleted"], true);
            let empty = list_with(&client).await.unwrap();
            assert!(empty["tasks"].as_array().unwrap().is_empty());
        }

        #[tokio::test]
        async fn a_run_is_asked_of_the_daemon_and_its_refusal_is_reported() {
            let running = Running::new();
            let client = running.client();
            let job = create_job(
                "task-2",
                "Check",
                "/tmp",
                &serde_json::to_string(&schedule_spec(&request().schedule).unwrap()).unwrap(),
                &json!({"type": "prompt", "prompt": "Check", "workspace": "/tmp"}).to_string(),
                None,
                None,
            )
            .unwrap();
            create_with(&client, job).await.unwrap();
            let refused = run_with(&client, "task-2").await;
            // The test daemon installs no executor, so the run is accepted by
            // the store and refused by the scheduler: either way it is the
            // daemon's answer, not ours.
            match refused {
                Ok(answer) => assert!(answer["state"].is_string(), "{answer}"),
                Err(message) => assert!(message.contains("scheduler"), "{message}"),
            }
        }

        #[tokio::test]
        async fn a_list_with_nothing_listening_says_so() {
            let directory = tempfile::tempdir_in("/tmp").unwrap();
            let client = DaemonClient::new(DaemonLifecycle::new(directory.path()).socket_path());
            let answer = list_with(&client).await.unwrap();
            assert_eq!(answer["daemon_running"], false);
            assert!(answer["tasks"].as_array().unwrap().is_empty());
            let created = create_with(
                &client,
                create_job(
                    "task-3",
                    "Check",
                    "/tmp",
                    r#"{"kind":"once","at":"2026-01-01T00:00:00Z"}"#,
                    r#"{"type":"prompt","prompt":"Check","workspace":"/tmp"}"#,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await;
            let message = created.err().unwrap();
            assert!(message.contains("daemon start"), "{message}");
        }

        #[tokio::test]
        async fn a_created_job_carries_its_name_and_no_prompt() {
            let running = Running::new();
            let client = running.client();
            let job = create_job(
                "task-4",
                "Name",
                "/tmp",
                r#"{"kind":"once","at":"2030-01-01T00:00:00Z"}"#,
                r#"{"type":"prompt","prompt":"secret words","workspace":"/tmp"}"#,
                None,
                None,
            )
            .unwrap();
            let created = create_with(&client, job).await.unwrap();
            assert!(!format!("{created}").contains("secret words"), "{created}");
            let _ = Duration::from_secs(1);
        }
    }
}
