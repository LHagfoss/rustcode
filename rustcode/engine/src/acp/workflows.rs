//! `_rustcode/workflows/*`: the app's view of durable workflow runs
//! (`docs/workflows.md`, #3394).
//!
//! The engine drives a run and keeps it on disk under the thread that started
//! it. The app asks for a thread's runs, hears of each change as a
//! `_rustcode/workflows/changed` notification carrying the run's summary, stops
//! a run, and claims the one wake that tells the starting thread the run ended.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::clients::{Clients, Topic};

/// How long changes are gathered before the runs are read and announced: a
/// run that starts ten workers moves ten times in a moment.
const COALESCE: Duration = Duration::from_millis(300);

/// `_rustcode/workflows/list`: a thread's runs, newest first. Asking also
/// subscribes the client to every run's changes; without a thread it only
/// subscribes. Answers `{runs}`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/workflows/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowListRequest {
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `_rustcode/workflows/stop`: end a run and interrupt its workers. Answers
/// `{run}`, or `{error}`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/workflows/stop", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowStopRequest {
    pub session_id: String,
    pub run_id: String,
}

/// `_rustcode/workflows/wake`: claim the wake of an ended run. Answers
/// `{claimed}`: true once, for the client that should tell the thread.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/workflows/wake", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowWakeRequest {
    pub session_id: String,
    pub run_id: String,
}

/// A run moved: its summary as the app shows it.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcNotification,
)]
#[notification(method = "_rustcode/workflows/changed")]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowChangedNotification {
    pub session_id: String,
    pub run: Value,
}

pub(crate) async fn list(request: WorkflowListRequest, clients: &Clients) -> Value {
    start_announcing(clients);
    let Some(thread) = request.session_id else {
        return json!({ "runs": [] });
    };
    let runs = tokio::task::spawn_blocking(move || crate::workflow::run_summaries(&thread))
        .await
        .unwrap_or_default();
    json!({ "runs": runs })
}

pub(crate) async fn stop(request: WorkflowStopRequest) -> Value {
    match crate::workflow::stop(&request.session_id, &request.run_id, false).await {
        Ok(run) => json!({ "run": run }),
        Err(message) => json!({ "error": { "code": "failed", "message": message } }),
    }
}

pub(crate) fn wake(request: &WorkflowWakeRequest) -> Value {
    match crate::workflow::claim_wake(&request.session_id, &request.run_id) {
        Ok(claimed) => json!({ "claimed": claimed }),
        Err(message) => json!({ "error": { "code": "failed", "message": message } }),
    }
}

/// Tell every subscribed client of each run change, once per engine.
fn start_announcing(clients: &Clients) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let notify = clients.clone();
    let mut changes = crate::workflow::changes();
    tokio::spawn(async move {
        loop {
            let mut pending = HashSet::new();
            match changes.recv().await {
                Ok(change) => {
                    pending.insert(change);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
            tokio::time::sleep(COALESCE).await;
            while let Ok(change) = changes.try_recv() {
                pending.insert(change);
            }
            let runs = tokio::task::spawn_blocking(move || {
                pending
                    .into_iter()
                    .filter_map(|change| {
                        crate::workflow::run_summary(&change.thread, &change.run_id)
                            .map(|run| (change.thread, run))
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            for (session_id, run) in runs {
                notify.publish(
                    Topic::Workflows,
                    WorkflowChangedNotification { session_id, run },
                );
            }
        }
    });
}
