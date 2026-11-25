//! `_rustcode/loops/*`: the app's Loops page (`docs/daemon.md` › Loops).
//!
//! A Loop is a daemon job whose action is [`JobAction::Loop`]: a prompt run on
//! a schedule, each run in a fresh thread on the app's engine. These methods
//! go through the same client and store as `rc cron` and
//! `_rustcode/scheduled/*`, so `rc cron list` shows the same jobs. Run
//! now and delete are `_rustcode/scheduled/run` and `_rustcode/scheduled/delete`.
//!
//! Unlike the scheduled methods, `start_daemon` starts the scheduler: the
//! Loops page offers it as a button the user presses.
//!
//! A Loop can also have a webhook: `POST /hooks/<loop-id>/<secret>` on the
//! outside-agent listener ([`trigger`]) starts a run with the request body.
//! The daemon keeps only the secret's hash; the URL is shown once, when the
//! webhook is turned on or its secret is replaced.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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
    /// Script mode: run this first; the agent wakes only when it prints.
    /// Set from the app's editor only, never by an agent's `create_loop`.
    #[serde(default)]
    pub script: Option<String>,
    /// Start a run when the Loop's webhook URL is called. Set from the app's
    /// editor only, never by an agent's `create_loop`.
    #[serde(default)]
    pub webhook: bool,
    /// The webhook events the Loop takes (#4386); empty takes every one. Set
    /// from the app's editor only.
    #[serde(default)]
    pub webhook_events: Vec<String>,
    /// Replace the webhook's secret, so the old URL stops working.
    #[serde(default)]
    pub rotate_webhook: bool,
    /// Hub Loops: the member of this hub each run goes to. `workspace` is
    /// then that machine's folder, and the Loop takes no script.
    #[serde(default)]
    pub machine: Option<String>,
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

/// `_rustcode/loops/run`: run a job now, keeping its schedule.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/run", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsRunRequest {
    pub id: String,
}

/// `_rustcode/loops/cancel`: stop an active run of a job.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/cancel", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsCancelRequest {
    pub id: String,
}

/// `_rustcode/loops/delete`: delete a Loop and its run history.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/loops/delete", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoopsDeleteRequest {
    pub id: String,
}

/// `_rustcode/loops/start_daemon`: start the scheduler, as
/// `rc daemon start` does.
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
    client
        .request(request)
        .await
        .map_err(|error| format!("{} ({error})", super::scheduled::DAEMON_STOPPED))
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
        "verdict": output.get("verdict"),
        "quiet": output.get("quiet").and_then(Value::as_bool).unwrap_or(false),
        "silent": output.get("silent").and_then(Value::as_bool).unwrap_or(false),
        "skipped": output.get("skipped"),
        "signedOut": output.get("signed_out").and_then(Value::as_bool).unwrap_or(false),
        // Hub Loops: the member the run was sent to, and whether it waits
        // there for that machine to come back.
        "machine": output.get("machine"),
        "queued": output.get("queued").and_then(Value::as_bool).unwrap_or(false),
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
        script,
        webhook,
        webhook_events,
        machine,
    } = &job.action
    else {
        return None;
    };
    let manual = matches!(job.schedule, ScheduleSpec::Manual);
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
        "script": script,
        "webhook": webhook.is_some(),
        "webhookEvents": webhook_events,
        "machine": machine,
        // A Loop run only by hand or by its webhook is never due.
        "nextDueAt": (!manual).then_some(job.next_due_at),
        "createdAt": job.created_at,
        "runs": runs.iter().map(run_json).collect::<Vec<_>>(),
    }))
}

pub(crate) async fn list() -> Result<Value, String> {
    // The page still lists what an old scheduler holds; the next save says why it waits.
    let _ = current_daemon().await;
    let mut answer = list_with(&client()?).await?;
    if let Some(members) = hub_members().await {
        answer["hubMembers"] = json!(members);
    }
    Ok(answer)
}

/// Members joined to this hub, so paired computers can also schedule jobs for
/// machines reachable only through this hub.
async fn hub_members() -> Option<Vec<Value>> {
    let config_dir = crate::config::get_config_dir()?;
    if !crate::remote_gateway::hub_link::HubConfig::load(&config_dir).hub {
        return None;
    }
    let members = crate::remote_gateway::lifecycle::RemoteLifecycle::new(&config_dir)
        .hub_members()
        .await
        .ok()
        .flatten()?;
    Some(
        members
            .into_iter()
            .map(|member| {
                json!({
                    "hostId": member.host_id,
                    "name": member.name,
                    "online": member.online,
                })
            })
            .collect(),
    )
}

/// Whether a scheduler that reports `version` runs this build's job format.
fn daemon_is_current(version: Option<&str>) -> bool {
    version == Some(crate::daemon::protocol::DAEMON_VERSION)
}

/// Restart a scheduler that an older RustCode started. It keeps running across
/// app updates, and an older build drops the job fields it does not know, such
/// as a Loop's account, from every save. One with a run going is left alone,
/// and the save is refused instead of losing what the user chose.
async fn current_daemon() -> Result<(), String> {
    let lifecycle = lifecycle()?;
    let Ok(Some(status)) = lifecycle.status().await else {
        return Ok(());
    };
    if daemon_is_current(status.version.as_deref()) {
        return Ok(());
    }
    if status.active_runs > 0 {
        return Err(
            "The scheduler is from an older RustCode and would drop some of these settings. \
             Save again once its run ends, and it restarts on the new version."
                .to_owned(),
        );
    }
    lifecycle
        .stop()
        .await
        .map_err(|error| format!("the scheduler did not stop to update: {error}"))?;
    start_daemon().await.map(|_| ())
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
    if value.get("kind").and_then(Value::as_str) == Some("manual") {
        return Ok(ScheduleSpec::Manual);
    }
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

/// A new webhook secret: 256 random bits, in hex.
fn webhook_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

/// What the daemon keeps of a webhook secret.
pub(crate) fn webhook_hash(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Whether `presented` is the secret `stored` is the hash of. Both sides are
/// hashed first, so the comparison time says nothing about the secret.
fn webhook_matches(stored: &str, presented: &str) -> bool {
    let presented = webhook_hash(presented);
    stored.len() == presented.len()
        && stored
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// A Loop's webhook after a save: the hash to keep, and the secret when a new
/// one was made (it is shown to the user once). `kept` is the hash the Loop
/// had before the edit.
fn webhook_for(
    enabled: bool,
    rotate: bool,
    kept: Option<String>,
) -> (Option<String>, Option<String>) {
    if !enabled {
        return (None, None);
    }
    match kept {
        Some(hash) if !rotate => (Some(hash), None),
        _ => {
            let secret = webhook_secret();
            (Some(webhook_hash(&secret)), Some(secret))
        }
    }
}

/// The job a save request describes, checked before the daemon sees it.
/// `webhook` is the hash of the Loop's webhook secret, if it has one.
pub(crate) fn job_from(
    request: LoopsSaveRequest,
    id: &str,
    webhook: Option<String>,
) -> Result<JobRecord, String> {
    let prompt = request.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("a loop needs a prompt".to_owned());
    }
    let name = request.name.trim().to_owned();
    if name.is_empty() {
        return Err("a loop needs a name".to_owned());
    }
    let webhook_events = if webhook.is_some() {
        crate::daemon::webhook::clean_events(&request.webhook_events)?
    } else {
        Vec::new()
    };
    let machine = trimmed(request.machine);
    let script = request
        .script
        .map(|script| script.trim().to_owned())
        .filter(|script| !script.is_empty());
    let workspace = std::path::Path::new(&request.workspace);
    match machine.as_deref() {
        // The folder is on the member, so only its shape is checked here.
        Some(machine) => {
            if !crate::orchestration::absolute_on_any_machine(&request.workspace) {
                return Err(format!(
                    "{} is not an absolute folder on {machine}",
                    request.workspace
                ));
            }
            if script.is_some() {
                return Err(format!(
                    "a Loop that runs on {machine} takes no script: the script would run on this computer"
                ));
            }
        }
        None if !workspace.is_absolute() || !workspace.is_dir() => {
            return Err(format!("{} is not a project folder", request.workspace));
        }
        None => {}
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
        script,
        webhook,
        webhook_events,
        machine,
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
    if let Some(machine) = trimmed(request.machine.clone()) {
        member_of_this_hub(&machine)?;
    }
    current_daemon().await?;
    let mut answer = save_with(&client()?, request).await?;
    if let (Some(id), Some(secret)) = (
        answer["loop"]["id"].as_str(),
        answer["webhook"]["secret"].as_str(),
    ) {
        answer["webhook"] = webhook_urls(id, secret).await;
    }
    Ok(answer)
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
    // An edit keeps the webhook's secret unless it is turned off or replaced.
    let kept = match &existing {
        Some(id) if request.webhook && !request.rotate_webhook => {
            match ask(client, DaemonRequest::Get { job_id: id.clone() }).await? {
                DaemonResponse::Job {
                    job:
                        JobRecord {
                            action: JobAction::Loop { webhook, .. },
                            ..
                        },
                } => webhook,
                _ => None,
            }
        }
        _ => None,
    };
    let (webhook, secret) = webhook_for(request.webhook, request.rotate_webhook, kept);
    let job = job_from(request, &id, webhook)?;
    let request = match existing {
        Some(_) => DaemonRequest::Update { job },
        None => DaemonRequest::Create { job },
    };
    match ask(client, request).await? {
        DaemonResponse::Job { job } => {
            let mut answer = json!({ "loop": loop_json(&job, &[]) });
            if let Some(secret) = secret {
                answer["webhook"] = json!({ "secret": secret });
            }
            Ok(answer)
        }
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

/// A Loop runs on another machine only from the fleet's hub, which keeps
/// time while that machine sleeps and holds each run until it is back.
fn member_of_this_hub(machine: &str) -> Result<(), String> {
    use crate::remote_gateway::hub_link::{HubConfig, Members};
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    if !HubConfig::load(&config_dir).hub {
        return Err(format!(
            "this computer is not the fleet's hub, so it cannot run a Loop on {machine}; save the Loop on {machine}, or make this computer the hub (rc machines hub on)"
        ));
    }
    if !Members::new(&config_dir).knows(machine) {
        return Err(format!(
            "{machine} has not joined this hub; on {machine}, run rc machines hub join with this computer's name"
        ));
    }
    Ok(())
}

/// Where a Loop's webhook answers: through the public address when one is
/// set (`[outside] public_url`), and on this computer's loopback port.
async fn webhook_urls(id: &str, secret: &str) -> Value {
    use crate::orchestration::outside_http;
    let path = format!("{}{id}/{secret}", outside_http::HOOKS_PATH);
    let local = outside_http::ensure()
        .await
        .ok()
        .and_then(|url| url.strip_suffix("/mcp").map(|base| format!("{base}{path}")));
    let public = outside_http::configured_public_base()
        .and_then(Result::ok)
        .map(|base| format!("{base}{path}"));
    json!({
        "url": public.clone().or(local.clone()),
        "localUrl": local,
        "public": public.is_some(),
        "secret": secret,
    })
}

/// A call to a Loop's webhook (`POST /hooks/<id>/<secret>`): start a run
/// whose prompt carries `body`, with the `event` and `delivery` the request
/// headers named (#4386). Answers an HTTP status and its JSON. An unknown
/// Loop, a Loop without a webhook and a wrong secret all answer 404, so a
/// caller cannot tell which Loops exist. A `ping` (GitHub sends one when a
/// webhook is added) checks the secret and answers 200 without a run. A
/// delivery whose event the Loop does not take answers 202 `ignored`, and one
/// the Loop already ran from answers 200 `duplicate`; neither starts a run.
pub(crate) async fn trigger(
    id: &str,
    secret: &str,
    body: &[u8],
    event: Option<&str>,
    delivery: Option<&str>,
) -> (u16, Value) {
    match client() {
        Ok(client) => trigger_with(&client, id, secret, body, event, delivery).await,
        Err(_) => (503, json!({ "error": "scheduler_off" })),
    }
}

async fn trigger_with(
    client: &DaemonClient,
    id: &str,
    secret: &str,
    body: &[u8],
    event: Option<&str>,
    delivery: Option<&str>,
) -> (u16, Value) {
    let not_found = (404, json!({ "error": "not_found" }));
    if id.is_empty() || secret.is_empty() {
        return not_found;
    }
    let job = match client
        .request(DaemonRequest::Get {
            job_id: id.to_owned(),
        })
        .await
    {
        Ok(DaemonResponse::Job { job }) => job,
        Ok(_) => return not_found,
        Err(_) => return (503, json!({ "error": "scheduler_off" })),
    };
    let JobAction::Loop {
        webhook: Some(hash),
        webhook_events,
        ..
    } = &job.action
    else {
        return not_found;
    };
    if !webhook_matches(hash, secret) {
        return not_found;
    }
    if event == Some("ping") {
        return (200, json!({ "pong": true }));
    }
    let delivery = crate::daemon::webhook::Delivery::new(
        event,
        delivery,
        String::from_utf8_lossy(body).into_owned(),
    );
    if !delivery.matches(webhook_events) {
        return (202, json!({ "ignored": "event" }));
    }
    if job.paused {
        return (409, json!({ "error": "paused" }));
    }
    match client
        .request(DaemonRequest::RunNow {
            job_id: job.id.clone(),
            input: Some(delivery.to_input()),
        })
        .await
    {
        Ok(DaemonResponse::Ack | DaemonResponse::RunAccepted { .. }) => {
            (202, json!({ "accepted": true }))
        }
        Ok(DaemonResponse::Error { code, message })
            if code == "conflict" && message.contains(crate::daemon::store::DUPLICATE_DELIVERY) =>
        {
            (200, json!({ "duplicate": true }))
        }
        // The Loop's previous run is still going; a run never overlaps.
        Ok(DaemonResponse::Error { code, .. }) if code == "conflict" => {
            (409, json!({ "error": "busy" }))
        }
        _ => (503, json!({ "error": "scheduler_off" })),
    }
}

pub(crate) async fn set_paused(request: LoopsSetPausedRequest) -> Result<Value, String> {
    set_paused_with(&client()?, request).await
}

/// Stop an active run of `id`.
pub(crate) async fn cancel(id: String) -> Result<Value, String> {
    match ask(&client()?, DaemonRequest::Cancel { job_id: id.clone() }).await? {
        DaemonResponse::Ack => Ok(json!({ "id": id, "cancelled": true })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
}

/// Partial update of an existing Loop, preserving untouched fields (including
/// any script or webhook secret hash). Used by outside agents' `update_loop`.
pub(crate) async fn patch(
    id: &str,
    paused: Option<bool>,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<Value>,
    read_only: Option<bool>,
    harness: Option<String>,
    account: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    thread_id: Option<String>,
) -> Result<Value, String> {
    let client = client()?;
    let has_edit = name.is_some()
        || prompt.is_some()
        || schedule.is_some()
        || read_only.is_some()
        || harness.is_some()
        || account.is_some()
        || model.is_some()
        || effort.is_some()
        || thread_id.is_some();
    if has_edit {
        current_daemon().await?;
        let existing = match ask(
            &client,
            DaemonRequest::Get {
                job_id: id.to_owned(),
            },
        )
        .await?
        {
            DaemonResponse::Job { job } => job,
            DaemonResponse::Error { message, .. } => return Err(message),
            other => return Err(format!("the scheduler answered {other:?}")),
        };
        let JobAction::Loop {
            prompt: old_prompt,
            workspace: old_workspace,
            harness: old_harness,
            account: old_account,
            model: old_model,
            effort: old_effort,
            read_only: old_read_only,
            thread_id: old_thread_id,
            script: old_script,
            webhook: old_webhook,
            webhook_events: old_webhook_events,
            machine: old_machine,
        } = existing.action
        else {
            return Err(format!("job `{id}` is not a loop"));
        };
        let next_schedule = match schedule {
            Some(value) => value,
            None => serde_json::to_value(&existing.schedule)
                .map_err(|error| format!("schedule: {error}"))?,
        };
        let (next_harness, next_account, next_model, next_effort, next_thread) = if thread_id
            .is_some()
        {
            (None, None, None, None, thread_id)
        } else if harness.is_some() || account.is_some() || model.is_some() || effort.is_some() {
            (
                harness.or(old_harness),
                account.or(old_account),
                model.or(old_model),
                effort.or(old_effort),
                None,
            )
        } else {
            (
                old_harness,
                old_account,
                old_model,
                old_effort,
                old_thread_id,
            )
        };
        let request = LoopsSaveRequest {
            id: Some(id.to_owned()),
            name: name.unwrap_or(existing.name),
            workspace: old_workspace,
            prompt: prompt.unwrap_or(old_prompt),
            schedule: next_schedule,
            harness: next_harness,
            account: next_account,
            model: next_model,
            effort: next_effort,
            read_only: read_only.unwrap_or(old_read_only),
            thread_id: next_thread,
            script: old_script,
            webhook: old_webhook.is_some(),
            webhook_events: old_webhook_events,
            rotate_webhook: false,
            machine: old_machine,
        };
        save_with(&client, request).await?;
    }
    if let Some(paused) = paused {
        set_paused_with(
            &client,
            LoopsSetPausedRequest {
                id: id.to_owned(),
                paused,
            },
        )
        .await?;
    }
    match ask(
        &client,
        DaemonRequest::Get {
            job_id: id.to_owned(),
        },
    )
    .await?
    {
        DaemonResponse::Job { job } => Ok(json!({
            "id": job.id,
            "name": job.name,
            "paused": job.paused,
            "schedule": job.schedule,
            "next_due_at": (!matches!(job.schedule, ScheduleSpec::Manual)).then_some(job.next_due_at),
        })),
        DaemonResponse::Error { message, .. } => Err(message),
        other => Err(format!("the scheduler answered {other:?}")),
    }
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

/// Start the scheduler the way `rc daemon start` does: this same binary
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
            script: None,
            webhook: false,
            webhook_events: Vec::new(),
            rotate_webhook: false,
            machine: None,
        }
    }

    #[test]
    fn only_a_scheduler_on_this_version_is_current() {
        assert!(daemon_is_current(Some(
            crate::daemon::protocol::DAEMON_VERSION
        )));
        // A scheduler from before the version field, or from another release.
        assert!(!daemon_is_current(None));
        assert!(!daemon_is_current(Some("0.0.1")));
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
        let job = job_from(request(&workspace), "loop-1", None).unwrap();
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
        let job = job_from(saved, "loop-1", None).unwrap();
        assert!(matches!(
            &job.action,
            JobAction::Loop { thread_id: Some(id), .. } if id == "thread-7"
        ));
        assert_eq!(loop_json(&job, &[]).unwrap()["threadId"], "thread-7");
    }

    #[test]
    fn a_saved_script_is_trimmed_and_a_blank_one_is_none() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let mut saved = request(&workspace);
        saved.script = Some("  gh run list --status failure  ".into());
        let job = job_from(saved, "loop-1", None).unwrap();
        assert_eq!(
            loop_json(&job, &[]).unwrap()["script"],
            "gh run list --status failure"
        );
        let mut blank = request(&workspace);
        blank.script = Some("   ".into());
        let job = job_from(blank, "loop-2", None).unwrap();
        assert!(matches!(&job.action, JobAction::Loop { script: None, .. }));
    }

    #[test]
    fn a_webhook_secret_is_kept_replaced_or_dropped() {
        let (hash, secret) = webhook_for(true, false, None);
        let (hash, secret) = (hash.unwrap(), secret.unwrap());
        assert_eq!(secret.len(), 64);
        assert_ne!(hash, secret, "only the hash is kept");
        assert!(webhook_matches(&hash, &secret));
        assert!(!webhook_matches(&hash, "wrong"));
        assert!(!webhook_matches(&hash, ""));

        assert_eq!(
            webhook_for(true, false, Some(hash.clone())),
            (Some(hash.clone()), None),
            "an edit keeps the URL"
        );
        let (rotated, new_secret) = webhook_for(true, true, Some(hash.clone()));
        assert_ne!(rotated.as_deref(), Some(hash.as_str()));
        assert!(!webhook_matches(&rotated.unwrap(), &secret));
        assert!(new_secret.is_some());
        assert_eq!(webhook_for(false, false, Some(hash)), (None, None));
    }

    #[test]
    fn a_manual_loop_with_a_webhook_is_never_due() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let mut saved = request(&workspace);
        saved.schedule = json!({"kind": "manual"});
        let job = job_from(saved, "loop-1", Some(webhook_hash("s"))).unwrap();
        assert_eq!(job.schedule, ScheduleSpec::Manual);
        let shown = loop_json(&job, &[]).unwrap();
        assert_eq!(shown["webhook"], true);
        assert!(shown["nextDueAt"].is_null());
        assert!(!shown.to_string().contains(&webhook_hash("s")));
    }

    #[test]
    fn a_loop_needs_a_prompt_a_name_and_a_folder() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let mut empty = request(&workspace);
        empty.prompt = "  ".into();
        assert!(job_from(empty, "x", None).is_err());
        let mut nameless = request(&workspace);
        nameless.name = String::new();
        assert!(job_from(nameless, "x", None).is_err());
        assert!(job_from(request("relative/path"), "x", None).is_err());
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
            input: None,
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
        async fn a_webhook_call_needs_the_loops_secret() {
            let running = Running::new();
            let client = running.client();
            let folder = tempfile::tempdir().unwrap();
            let workspace = folder.path().to_string_lossy().into_owned();
            let mut saved = request(&workspace);
            saved.webhook = true;
            let created = save_with(&client, saved).await.unwrap();
            let id = created["loop"]["id"].as_str().unwrap().to_owned();
            assert_eq!(created["loop"]["webhook"], true);
            let secret = created["webhook"]["secret"].as_str().unwrap().to_owned();
            assert_eq!(secret.len(), 64, "{created}");

            assert_eq!(
                trigger_with(&client, &id, "wrong", b"{}", None, None)
                    .await
                    .0,
                404
            );
            assert_eq!(
                trigger_with(&client, &id, "wrong", b"{}", Some("ping"), None)
                    .await
                    .0,
                404
            );
            // GitHub's ping when the webhook is added starts no run.
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", Some("ping"), None)
                    .await
                    .0,
                200
            );
            assert_eq!(
                trigger_with(&client, "loop-none", &secret, b"{}", None, None)
                    .await
                    .0,
                404
            );
            // The right secret reaches the scheduler, which this test does not run.
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", None, None)
                    .await
                    .0,
                503
            );

            // An edit keeps the secret and shows no URL again.
            let mut edit = request(&workspace);
            edit.id = Some(id.clone());
            edit.webhook = true;
            let edited = save_with(&client, edit).await.unwrap();
            assert!(edited.get("webhook").is_none());
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", None, None)
                    .await
                    .0,
                503
            );

            let mut filtered = request(&workspace);
            filtered.id = Some(id.clone());
            filtered.webhook = true;
            filtered.webhook_events = vec![" pull_request ".into(), "pull_request".into()];
            let saved = save_with(&client, filtered).await.unwrap();
            assert_eq!(saved["loop"]["webhookEvents"], json!(["pull_request"]));
            let ignored =
                trigger_with(&client, &id, &secret, b"{}", Some("push"), Some("d-1")).await;
            assert_eq!(ignored, (202, json!({ "ignored": "event" })));
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", None, None)
                    .await
                    .0,
                202
            );
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", Some("pull_request"), None)
                    .await
                    .0,
                503
            );
            let mut bad = request(&workspace);
            bad.id = Some(id.clone());
            bad.webhook = true;
            bad.webhook_events = vec!["push; rm -rf".into()];
            assert!(save_with(&client, bad).await.is_err());

            // Turned off, the old URL is unknown.
            let mut off = request(&workspace);
            off.id = Some(id.clone());
            save_with(&client, off).await.unwrap();
            assert_eq!(
                trigger_with(&client, &id, &secret, b"{}", None, None)
                    .await
                    .0,
                404
            );
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
