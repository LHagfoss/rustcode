//! Bounded action execution. Durable claiming and ambiguity recovery stay in the scheduler.
use super::model::{JobAction, JobRecord, JobRunRecord};
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub type ExecutionFuture = Pin<Box<dyn Future<Output = RunOutcome> + Send + 'static>>;

#[derive(Clone)]
pub struct JobRunContext {
    pub job: JobRecord,
    pub run: JobRunRecord,
    pub cancellation: CancellationToken,
}

/// Implementations must honor cancellation at safe action boundaries and report
/// Ambiguous whenever an external effect may have happened without confirmation.
pub trait JobExecutor: Send + Sync {
    fn execute(&self, context: JobRunContext) -> ExecutionFuture;
}

#[derive(Debug, Clone)]
pub enum RunOutcome {
    Succeeded {
        summary: String,
        output: Option<String>,
    },
    Transient {
        error: String,
        output: Option<String>,
    },
    Permanent {
        error: String,
        output: Option<String>,
    },
    Cancelled {
        output: Option<String>,
    },
    Ambiguous {
        error: String,
        output: Option<String>,
    },
}

pub const MAX_RUN_OUTPUT_BYTES: usize = 16 * 1024;

/// The only injected boundary: one direct action, never a polling loop.
pub trait ActionBackend: Send + Sync {
    fn run(
        &self,
        action: JobAction,
        session: String,
        cancellation: CancellationToken,
    ) -> ExecutionFuture;
}

pub struct ActionExecutor {
    backend: Arc<dyn ActionBackend>,
    timeout: Duration,
}

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::with_backend(Arc::new(ProductionActions), Duration::from_secs(300))
    }
}

impl ActionExecutor {
    pub fn with_backend(backend: Arc<dyn ActionBackend>, timeout: Duration) -> Self {
        Self { backend, timeout }
    }
}

impl JobExecutor for ActionExecutor {
    fn execute(&self, context: JobRunContext) -> ExecutionFuture {
        let backend = self.backend.clone();
        let timeout = self.timeout;
        Box::pin(async move {
            let session = match &context.job.action {
                JobAction::Prompt {
                    session_id: Some(id),
                    ..
                } => id.clone(),
                _ => context
                    .job
                    .target_session
                    .clone()
                    .unwrap_or_else(|| format!("daemon-{}", context.job.id)),
            };
            let cancellation = context.cancellation;
            let (action, interval, max_runs, deadline, stop_on_change) = match context.job.action {
                JobAction::Poll {
                    action,
                    interval_seconds,
                    max_runs,
                    deadline_seconds,
                    stop_on_change,
                } => {
                    if interval_seconds == 0
                        || max_runs == 0
                        || deadline_seconds == 0
                        || !matches!(
                            *action,
                            JobAction::McpCall { .. } | JobAction::ShellCommand { .. }
                        )
                    {
                        return permanent(
                            "poll requires a direct action and positive interval, max-runs and deadline",
                        );
                    }
                    (
                        *action,
                        Duration::from_secs(interval_seconds),
                        max_runs,
                        Duration::from_secs(deadline_seconds).min(timeout),
                        stop_on_change,
                    )
                }
                action => (action, Duration::ZERO, 1, timeout, false),
            };
            let Some(deadline) = tokio::time::Instant::now().checked_add(deadline) else {
                return permanent("execution deadline is out of range");
            };
            let mut previous_hash = None;
            let mut last = RunOutcome::Cancelled { output: None };
            for index in 0..max_runs {
                if cancellation.is_cancelled() {
                    return RunOutcome::Cancelled {
                        output: outcome_output(&last),
                    };
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                let child = cancellation.child_token();
                // Dropping a future must also request cancellation of blocking adapters.
                let _cancel_on_drop = child.clone().drop_guard();
                let future = backend.run(action.clone(), session.clone(), child.clone());
                tokio::pin!(future);
                let result = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => None,
                    _ = tokio::time::sleep_until(deadline) => None,
                    result = &mut future => Some(result),
                };
                let result = match result {
                    Some(result) => result,
                    None => {
                        child.cancel();
                        // Allow adapters to reap processes and return partial output,
                        // but never let cleanup turn an unknown effect into a retry.
                        let output = tokio::time::timeout(Duration::from_secs(1), &mut future)
                            .await
                            .ok()
                            .and_then(|result| outcome_output(&result))
                            .or_else(|| outcome_output(&last));
                        return ambiguous("action cancelled or timed out after dispatch", output);
                    }
                };
                // Hash the complete result before truncating the persisted output.
                let hash = match &result {
                    RunOutcome::Succeeded { output, .. } => {
                        canonical_hash(output.as_deref().unwrap_or(""))
                    }
                    _ => return bounded(result),
                };
                last = bounded(result);
                if stop_on_change && previous_hash.is_some_and(|previous| previous != hash) {
                    break;
                }
                previous_hash = Some(hash);
                if index + 1 == max_runs {
                    break;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return RunOutcome::Cancelled { output: outcome_output(&last) },
                    _ = tokio::time::sleep(interval.min(remaining)) => {}
                }
            }
            last
        })
    }
}

fn canonical_hash(output: &str) -> [u8; 32] {
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonical).collect())
            }
            value => value,
        }
    }
    let text = serde_json::from_str(output)
        .map(|value| canonical(value).to_string())
        .unwrap_or_else(|_| output.to_owned());
    Sha256::digest(text.as_bytes()).into()
}

fn truncate(text: &mut String) {
    let mut end = text.len().min(MAX_RUN_OUTPUT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn bounded(mut outcome: RunOutcome) -> RunOutcome {
    let output = match &mut outcome {
        RunOutcome::Succeeded { summary, output } => {
            truncate(summary);
            output
        }
        RunOutcome::Transient { error, output }
        | RunOutcome::Permanent { error, output }
        | RunOutcome::Ambiguous { error, output } => {
            truncate(error);
            output
        }
        RunOutcome::Cancelled { output } => output,
    };
    if let Some(output) = output {
        truncate(output);
    }
    outcome
}

fn outcome_output(outcome: &RunOutcome) -> Option<String> {
    match outcome {
        RunOutcome::Succeeded { output, .. }
        | RunOutcome::Transient { output, .. }
        | RunOutcome::Permanent { output, .. }
        | RunOutcome::Cancelled { output }
        | RunOutcome::Ambiguous { output, .. } => output.clone(),
    }
}

fn permanent(error: impl Into<String>) -> RunOutcome {
    bounded(RunOutcome::Permanent {
        error: error.into(),
        output: None,
    })
}

fn transient(error: impl Into<String>) -> RunOutcome {
    bounded(RunOutcome::Transient {
        error: error.into(),
        output: None,
    })
}

fn ambiguous(error: impl Into<String>, output: Option<String>) -> RunOutcome {
    bounded(RunOutcome::Ambiguous {
        error: error.into(),
        output,
    })
}

struct ProductionActions;

impl ActionBackend for ProductionActions {
    fn run(
        &self,
        action: JobAction,
        session: String,
        cancellation: CancellationToken,
    ) -> ExecutionFuture {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return RunOutcome::Cancelled { output: None };
            }
            match action {
                JobAction::McpCall {
                    server,
                    tool,
                    arguments,
                    workspace,
                    server_config,
                } => {
                    let Some(config) = server_config else {
                        return permanent(
                            "scheduled MCP action requires a recorded server configuration",
                        );
                    };
                    if config.name != server || !config.enabled {
                        return permanent("recorded MCP server is mismatched or disabled");
                    }
                    let client = match crate::mcp::start_owned_server(
                        &config,
                        std::path::Path::new(&workspace),
                    )
                    .await
                    {
                        Ok(client) => client,
                        Err(error) => return transient(error),
                    };
                    if cancellation.is_cancelled() {
                        client.shutdown().await;
                        return RunOutcome::Cancelled { output: None };
                    }
                    let result = client.call_tool(&tool, arguments).await;
                    client.shutdown().await;
                    match result {
                        Ok(value)
                            if value.get("isError").and_then(|v| v.as_bool()) == Some(true) =>
                        {
                            bounded(RunOutcome::Permanent {
                                error: "MCP tool reported an error".into(),
                                output: Some(value.to_string()),
                            })
                        }
                        Ok(value) => RunOutcome::Succeeded {
                            summary: "MCP call completed".into(),
                            output: Some(value.to_string()),
                        },
                        Err(error) => ambiguous(error, None),
                    }
                }
                JobAction::Prompt {
                    prompt,
                    workspace,
                    model_profile,
                    settings,
                    ..
                } => {
                    let mut state = match crate::raw_cli::build_scheduled_state(
                        &prompt,
                        std::path::Path::new(&workspace),
                        model_profile.as_deref(),
                        &session,
                        settings.as_ref(),
                    ) {
                        Ok(state) => state,
                        Err(error)
                            if error.starts_with("unknown model profile:")
                                || error.starts_with("recorded MCP credential unavailable:") =>
                        {
                            return transient(error);
                        }
                        Err(error) => return permanent(error),
                    };
                    // The recorded session is an input. Persist daemon output in
                    // a separate session, never over the interactive transcript.
                    static NEXT_SESSION: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    state.active_session_id = format!(
                        "daemon-{}-{}-{}",
                        std::process::id(),
                        chrono::Utc::now().timestamp_micros(),
                        NEXT_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    );
                    match crate::raw_cli::run_scheduled_turn(
                        state,
                        std::path::Path::new(&workspace),
                        cancellation,
                    )
                    .await
                    {
                        Ok(output) => RunOutcome::Succeeded {
                            summary: "Headless turn completed".into(),
                            output: Some(output),
                        },
                        Err(crate::raw_cli::ScheduledTurnError::Transient(error)) => {
                            transient(error)
                        }
                        Err(crate::raw_cli::ScheduledTurnError::Ambiguous(error)) => {
                            ambiguous(error, None)
                        }
                    }
                }
                JobAction::ShellCommand {
                    command,
                    working_directory,
                    environment_allowlist,
                    timeout_seconds,
                    authorized,
                } => {
                    if timeout_seconds == 0 {
                        return permanent("shell timeout must be positive");
                    }
                    if !matches!(
                        crate::tools::authorize_tool_with_args(
                            "run_command",
                            &serde_json::json!({"command": command}),
                            crate::config::AgentMode::Build,
                            false,
                            authorized,
                        ),
                        crate::tools::AuthorizationDecision::Allow
                    ) {
                        return permanent(
                            "scheduled command requires confirmation; no stored command authorization is available",
                        );
                    }
                    let cwd = std::path::PathBuf::from(working_directory);
                    if !cwd.is_absolute() || !cwd.is_dir() {
                        return permanent(
                            "shell working directory must be an existing absolute directory",
                        );
                    }
                    let writable_roots = vec![cwd.clone()];
                    let sandbox_mode = crate::config::load_config_for_workspace(&cwd)
                        .2
                        .sandbox_mode;
                    let sandboxed_command = match crate::tools::exec::sandbox::command(
                        &command,
                        crate::tools::exec::sandbox::SandboxPolicy {
                            command_cwd: Some(&cwd),
                            workspace_root: Some(&cwd),
                            writable_roots: &writable_roots,
                            session_scratch_roots: &[],
                            one_shot_writable_roots: &[],
                            write_access: sandbox_mode.allows_workspace_write(),
                            network_access: sandbox_mode.allows_network(),
                        },
                    ) {
                        Ok(command) => command,
                        Err(error) => return permanent(error),
                    };
                    let output = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
                    let captured = output.clone();
                    let progress: rustcode_command::ProgressCallback = Arc::new(move |bytes, _| {
                        let mut captured = captured.lock().unwrap();
                        let count = bytes
                            .len()
                            .min(MAX_RUN_OUTPUT_BYTES.saturating_sub(captured.len()));
                        captured.extend_from_slice(&bytes[..count]);
                    });
                    let result = tokio::task::spawn_blocking(move || {
                        if cancellation.is_cancelled() {
                            return Err("cancelled before shell dispatch".to_owned());
                        }
                        let request = rustcode_command::CommandRequest {
                            command: sandboxed_command.command,
                            status_command: None,
                            sandboxed_shell: true,
                            cwd: Some(cwd),
                            env: vec![],
                            timeout: Duration::from_secs(timeout_seconds),
                            process_group: true,
                            inherited_fds: sandboxed_command.inherited_fds,
                        };
                        rustcode_command::run_with_timeout_cancellable_env(
                            &request,
                            Some(progress),
                            Some(Arc::new(move || cancellation.is_cancelled())),
                            &environment_allowlist,
                        )
                    })
                    .await;
                    let output =
                        Some(String::from_utf8_lossy(&output.lock().unwrap()).into_owned());
                    match result {
                        Ok(Ok(result)) if !result.success && result.signal.is_some() => ambiguous("command process was lost", output),
                        Ok(Ok(result)) if result.stdout.is_truncated() || result.stderr.is_truncated() => {
                            RunOutcome::Permanent {
                                error: "command output exceeded the capture limit; cannot compare an incomplete polling result".into(),
                                output,
                            }
                        }
                        Ok(Ok(result)) if result.success => RunOutcome::Succeeded {
                            summary: "Command completed".into(),
                            // Keep complete bounded streams until the executor hashes
                            // them; callback capture is only for interrupted commands.
                            output: Some(format!("{}{}", String::from_utf8_lossy(result.stdout.bytes()), String::from_utf8_lossy(result.stderr.bytes()))),
                        },
                        Ok(Ok(result)) if result.signal.is_none() => RunOutcome::Permanent {
                            error: format!("Command exited with {:?}", result.exit_code),
                            output,
                        },
                        Ok(Ok(_)) => ambiguous("command process was lost", output),
                        Ok(Err(error)) if error == "cancelled before shell dispatch" => {
                            RunOutcome::Cancelled { output }
                        }
                        Ok(Err(error)) if error.starts_with("failed to spawn process:") => {
                            transient(error)
                        }
                        Ok(Err(error)) => ambiguous(error, output),
                        Err(error) => ambiguous(error.to_string(), output),
                    }
                }
                JobAction::Poll { .. } => permanent("nested polling is not supported"),
            }
        })
    }
}
