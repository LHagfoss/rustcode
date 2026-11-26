//! A job's `workflow` action (ADR 0009): each run starts a fresh coordinator
//! thread in the project, starts the project's named workflow there
//! (`_rustcode/workflows/start`), and ends when the workflow run ends. The
//! job run's output names both, so its row links the thread and the run.

use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{LoopRunEnd, LoopThread, open_job_engine, start_job_thread};
use crate::acp::workflows::WorkflowStartRequest;

/// How often the run's record is read while it goes.
const POLL: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// One `workflow` job run.
pub(crate) struct WorkflowThread {
    pub id: String,
    pub name: String,
    pub workflow: String,
    pub args: Value,
    pub workspace: String,
    pub harness: Option<String>,
    pub account: Option<String>,
}

/// How a `workflow` job run ended.
pub(crate) enum WorkflowRunEnd {
    /// The workflow run settled with `status` (`done`, `failed`,
    /// `timed_out`, `cancelled`).
    Settled {
        thread_id: String,
        run_id: String,
        status: String,
        error: Option<String>,
    },
    /// No run started, or the job run was cancelled.
    NotRun(LoopRunEnd),
}

/// Where a workflow named `name` is looked for in `workspace`.
pub(crate) fn script_path(workspace: &str, name: &str) -> std::path::PathBuf {
    std::path::Path::new(workspace)
        .join(".rustcode")
        .join("workflows")
        .join(format!("{name}.js"))
}

pub(crate) async fn run_workflow_thread(
    run: WorkflowThread,
    cancellation: CancellationToken,
) -> WorkflowRunEnd {
    let failed = |thread_id: Option<String>, error: String| {
        WorkflowRunEnd::NotRun(LoopRunEnd::Failed { thread_id, error })
    };
    if !std::path::Path::new(&run.workspace).is_dir() {
        return failed(None, format!("{} is not a folder", run.workspace));
    }
    let script = script_path(&run.workspace, &run.workflow);
    if !script.is_file() {
        return failed(
            None,
            format!("no workflow named {} at {}", run.workflow, script.display()),
        );
    }
    let (engine, _) = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return WorkflowRunEnd::NotRun(LoopRunEnd::Cancelled { thread_id: None }),
        opened = open_job_engine(super::Access::Full, None) => match opened {
            Ok(engine) => engine,
            Err(value) => return failed(None, super::refusal(value)),
        },
    };
    let thread = LoopThread {
        id: run.id.clone(),
        name: run.name.clone(),
        prompt: String::new(),
        workspace: run.workspace.clone(),
        harness: run.harness.clone(),
        account: run.account.clone(),
        model: None,
        effort: None,
        read_only: false,
        thread_id: None,
    };
    let thread_id = match start_job_thread(&engine, &thread, &cancellation).await {
        Ok((thread_id, _)) => thread_id,
        Err(end) => return WorkflowRunEnd::NotRun(end),
    };
    let started = engine
        .request(
            WorkflowStartRequest {
                session_id: thread_id.clone(),
                name: run.workflow.clone(),
                args: run.args.clone(),
            },
            START_TIMEOUT,
        )
        .await;
    let run_id = match started {
        Ok(answer) => match answer["runId"].as_str() {
            Some(run_id) => run_id.to_owned(),
            None => return failed(Some(thread_id), "the workflow run did not start".to_owned()),
        },
        Err(message) => {
            return failed(
                Some(thread_id),
                format!("the workflow could not start: {message}"),
            );
        }
    };
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = crate::workflow::stop(&thread_id, &run_id, false).await;
                return WorkflowRunEnd::NotRun(LoopRunEnd::Cancelled { thread_id: Some(thread_id) });
            }
            _ = tokio::time::sleep(POLL) => {}
        }
        let Some(summary) = crate::workflow::run_summary(&thread_id, &run_id) else {
            return failed(Some(thread_id), format!("the workflow run {run_id} is gone"));
        };
        if let Some(end) = settled(&summary) {
            let (status, error) = end;
            return WorkflowRunEnd::Settled {
                thread_id,
                run_id,
                status,
                error,
            };
        }
    }
}

/// A run summary's status and error once it is no longer running.
fn settled(summary: &Value) -> Option<(String, Option<String>)> {
    let status = summary["status"].as_str()?;
    (status != "running").then(|| {
        (
            status.to_owned(),
            summary["error"].as_str().map(str::to_owned),
        )
    })
}

/// The workflow's `args`, with the webhook delivery that started the run as
/// `webhook: { event, delivery, body }` (the body parsed when it is JSON).
pub(crate) fn args_with_delivery(
    args: &Value,
    delivery: Option<&crate::daemon::webhook::Delivery>,
) -> Value {
    let Some(delivery) = delivery else {
        return args.clone();
    };
    let body = serde_json::from_str::<Value>(&delivery.body)
        .unwrap_or_else(|_| Value::String(delivery.body.clone()));
    let webhook = json!({ "event": delivery.event, "delivery": delivery.delivery, "body": body });
    match args {
        Value::Object(map) => {
            let mut map = map.clone();
            map.insert("webhook".to_owned(), webhook);
            Value::Object(map)
        }
        Value::Null => json!({ "webhook": webhook }),
        other => json!({ "args": other, "webhook": webhook }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_webhook_delivery_joins_the_args() {
        let delivery = crate::daemon::webhook::Delivery {
            event: Some("check_suite".into()),
            delivery: Some("d-1".into()),
            body: r#"{"action":"completed"}"#.into(),
        };
        let args = args_with_delivery(&json!({"branch": "main"}), Some(&delivery));
        assert_eq!(args["branch"], "main");
        assert_eq!(args["webhook"]["event"], "check_suite");
        assert_eq!(args["webhook"]["body"]["action"], "completed");
        assert_eq!(args_with_delivery(&json!({"a": 1}), None), json!({"a": 1}));
        assert_eq!(
            args_with_delivery(&Value::Null, Some(&delivery))["webhook"]["delivery"],
            "d-1"
        );
    }

    #[test]
    fn a_run_settles_once_it_is_not_running() {
        assert!(settled(&json!({"status": "running"})).is_none());
        assert_eq!(
            settled(&json!({"status": "failed", "error": "boom"})),
            Some(("failed".to_owned(), Some("boom".to_owned())))
        );
    }

    #[tokio::test]
    async fn an_unknown_workflow_fails_with_the_path_it_looked_for() {
        let dir = tempfile::tempdir().unwrap();
        let end = run_workflow_thread(
            WorkflowThread {
                id: "job-1".into(),
                name: "Nightly".into(),
                workflow: "missing".into(),
                args: Value::Null,
                workspace: dir.path().display().to_string(),
                harness: None,
                account: None,
            },
            CancellationToken::new(),
        )
        .await;
        let WorkflowRunEnd::NotRun(LoopRunEnd::Failed { error, .. }) = end else {
            panic!("expected a failed run");
        };
        assert!(error.contains("missing.js"), "{error}");
    }
}
