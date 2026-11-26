//! The engine's [`AgentRunner`]: a workflow's workers are ordinary delegate
//! threads, started and waited on with the same `start_session` and
//! `wait_session` calls an agent makes (`docs/orchestrator.md`), as the thread
//! that started the run. They nest under it, count against its delegate slots
//! and the user's approvals, and show in the app like any delegate.
//!
//! A call goes to the app when one hosts the thread's tools, and to the
//! engine's own answer (`orchestration/local.rs`) when none does, exactly as
//! the hub routes the thread's other calls.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::run::{AgentRunner, Fut, Worker, WorkerRequest};
use crate::orchestration::{INTERRUPT_SESSION, START_SESSION, WAIT_SESSION};
use crate::preview::PreviewClient;

/// How long one `wait_session` waits before the loop asks again.
const WAIT_SLICE_MS: u64 = 10 * 60 * 1000;
/// How long to wait before asking again when no delegate slot was free, or the
/// worker waits for the user.
const RETRY_AFTER: Duration = Duration::from_secs(5);

pub(crate) struct ThreadRunner {
    caller: String,
    client: Option<Arc<dyn PreviewClient>>,
    version: u8,
}

impl ThreadRunner {
    pub(crate) fn new(
        caller: &str,
        client: Option<Arc<dyn PreviewClient>>,
        version: u8,
    ) -> Arc<Self> {
        Arc::new(Self {
            caller: caller.to_owned(),
            client,
            version,
        })
    }

    async fn call(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        let output = match &self.client {
            Some(client) => {
                crate::orchestration::execute_versioned(
                    Some(Arc::clone(client)),
                    self.version,
                    cancel,
                    name,
                    &args,
                )
                .await
            }
            None => crate::orchestration::local::answer(&self.caller, cancel, name, &args).await,
        };
        if !output.success {
            return Err(output.content);
        }
        serde_json::from_str(&output.content)
            .map_err(|error| format!("{name} answered something that is not JSON: {error}"))
    }
}

/// The `start_session` call a worker is started with, its delegate preset
/// expanded as an agent's own call would be (#4384).
pub(super) fn start_args(request: &WorkerRequest) -> Result<Value, String> {
    let mut args = json!({
        "prompt": request.prompt,
        "title": request.title,
        "client_request_id": request.request_id,
    });
    for (key, value) in [
        ("harness", &request.harness),
        ("model", &request.model),
        ("effort", &request.effort),
        ("preset", &request.preset),
        ("access", &request.access),
        ("account", &request.account),
        ("project", &request.project),
    ] {
        if let Some(value) = value {
            args[key] = json!(value);
        }
    }
    if let Some(worktree) = request.worktree {
        args["worktree"] = json!(worktree);
    }
    match crate::orchestration::with_preset(START_SESSION, &args) {
        Ok(expanded) => Ok(expanded.unwrap_or(args)),
        Err(refused) => Err(refused.content),
    }
}

async fn pause(cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(RETRY_AFTER) => true,
    }
}

impl AgentRunner for ThreadRunner {
    fn start<'a>(
        &'a self,
        request: &'a WorkerRequest,
        cancel: &'a CancellationToken,
    ) -> Fut<'a, Result<Worker, String>> {
        Box::pin(async move {
            let args = start_args(request)?;
            loop {
                match self.call(START_SESSION, args.clone(), cancel).await {
                    Ok(answer) => {
                        let text = |key: &str| {
                            answer
                                .get(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .ok_or_else(|| format!("start_session answered without a {key}"))
                        };
                        return Ok(Worker {
                            session_id: text("session_id")?,
                            link_id: text("link_id")?,
                        });
                    }
                    // Every delegate slot is taken: the run waits its turn.
                    Err(reason) if reason.contains("start_session busy") => {
                        if !pause(cancel).await {
                            return Err("the run was stopped".to_owned());
                        }
                    }
                    Err(reason) => return Err(reason),
                }
            }
        })
    }

    fn wait<'a>(
        &'a self,
        worker: &'a Worker,
        cancel: &'a CancellationToken,
    ) -> Fut<'a, Result<String, String>> {
        Box::pin(async move {
            loop {
                if cancel.is_cancelled() {
                    return Err("the run was stopped".to_owned());
                }
                let answer = self
                    .call(
                        WAIT_SESSION,
                        json!({ "link_id": worker.link_id, "timeout_ms": WAIT_SLICE_MS }),
                        cancel,
                    )
                    .await?;
                let row = answer
                    .get("links")
                    .and_then(Value::as_array)
                    .and_then(|links| links.first())
                    .ok_or_else(|| "wait_session answered without the worker's link".to_owned())?;
                let state = row
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                let text = row.get("answer").and_then(Value::as_str).unwrap_or("");
                match state {
                    "done" => return Ok(text.to_owned()),
                    "queued" | "working" => {}
                    // The user has to answer it in the app; the run keeps waiting.
                    "needs_user" => {
                        if !pause(cancel).await {
                            return Err("the run was stopped".to_owned());
                        }
                    }
                    other => {
                        return Err(format!(
                            "the worker thread {} ended as {other}{}",
                            worker.session_id,
                            if text.is_empty() {
                                String::new()
                            } else {
                                format!(": {text}")
                            }
                        ));
                    }
                }
            }
        })
    }

    fn abandon<'a>(&'a self, worker: &'a Worker) -> Fut<'a, ()> {
        Box::pin(async move {
            // Best effort: a worker that already ended has nothing to stop.
            let never = CancellationToken::new();
            let _ = self
                .call(
                    INTERRUPT_SESSION,
                    json!({ "session_id": worker.session_id }),
                    &never,
                )
                .await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> WorkerRequest {
        WorkerRequest {
            prompt: "Review a.rs".to_owned(),
            title: "Review".to_owned(),
            harness: Some("codex".to_owned()),
            model: None,
            effort: None,
            preset: None,
            access: Some("ask".to_owned()),
            account: Some("work".to_owned()),
            project: Some("/code/app".to_owned()),
            worktree: Some(false),
            request_id: "wf-1-2-call-0".to_owned(),
        }
    }

    #[test]
    fn a_workers_start_carries_every_option_it_was_given() {
        let args = start_args(&request()).unwrap();
        assert_eq!(
            args,
            json!({
                "prompt": "Review a.rs",
                "title": "Review",
                "client_request_id": "wf-1-2-call-0",
                "harness": "codex",
                "access": "ask",
                "account": "work",
                "project": "/code/app",
                "worktree": false,
            })
        );
    }
}
