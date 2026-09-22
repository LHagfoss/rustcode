use crate::app::{AppState, ChatMessage};
use rustcode_tasks::{TaskEvent, TaskManager};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

fn background_task_history_message(
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
) -> ChatMessage {
    let command = output
        .command
        .as_deref()
        .map(|command| format!(" Command: {command}."))
        .unwrap_or_default();
    let prefix = format!("background_task: Task {task_id} completed.{command} Output:\n");
    crate::network::bounded_tool_result_history_message(
        crate::network::ToolResult {
            tool_name: "background_task".to_string(),
            content: output.content,
            diff: None,
            file_preview: None,
            metadata: crate::network::ToolResultMetadata {
                success: output.success,
                exit_code: output.exit_code,
                command: output.command,
                truncated: output.truncated,
                completeness: output.completeness,
                replayed: output.replayed,
                error_kind: output.error_kind,
                retryable: output.retryable,
                command_status: output.command_status,
                ..Default::default()
            },
        },
        &prefix,
        None,
    )
}

/// Build the initial application state and apply any model overrides.
pub fn build_state(prompt: &str, model_override: Option<&str>) -> AppState {
    let mut state = AppState::new();
    state.raw_cli_mode = true;

    if let Some(m_name) = model_override {
        if let Some(profile) = state.config.models.iter().find(|m| m.name == m_name) {
            state.api_base_url = profile.url.clone();
            state.model_name = profile.model.clone();
            println!(
                "Overriding model profile to: {} ({})",
                m_name, profile.model
            );
        } else {
            println!(
                "Warning: Model profile '{}' not found in models.json. Using default.",
                m_name
            );
        }
    }

    state
        .history
        .push(ChatMessage::new("user", prompt.to_string()));
    state.session_title_tool_available = true;
    state
}

/// Load a recorded workspace and session, without consulting or replacing the
/// user's active session. MCP startup is separate so callers own its lifetime.
pub fn build_scheduled_state(
    prompt: &str,
    workspace: &std::path::Path,
    model_profile: Option<&str>,
    session_id: &str,
    settings: Option<&crate::config::SessionSettingsSnapshot>,
) -> Result<AppState, String> {
    if !workspace.is_absolute() || !workspace.is_dir() {
        return Err("scheduled workspace must be an existing absolute directory".into());
    }
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("invalid scheduled session id".into());
    }
    let mut state = AppState::new_with_workspace_session(workspace, Some(session_id));
    let saved = settings
        .cloned()
        .or_else(|| crate::config::load_session_settings(session_id));
    if let Some(snapshot) = &saved {
        let mut recorded: crate::config::AppConfig =
            serde_json::from_value(snapshot.config.clone())
                .map_err(|error| format!("invalid recorded prompt settings: {error}"))?;
        // Session logs redact secrets. Resolve only those credentials, never
        // model parameters or endpoints, from matching current configuration.
        for model in &mut recorded.models {
            if model.api_key.is_none() {
                model.api_key = state
                    .config
                    .models
                    .iter()
                    .find(|current| current.name == model.name && current.url == model.url)
                    .and_then(|current| current.api_key.clone());
            }
        }
        for server in &mut recorded.mcp_servers {
            for (key, value) in &mut server.env {
                if value == "<redacted>" {
                    *value = state
                        .config
                        .mcp_servers
                        .iter()
                        .find(|current| {
                            current.name == server.name
                                && current.command == server.command
                                && current.args == server.args
                        })
                        .and_then(|current| current.env.get(key))
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "recorded MCP credential unavailable: {} / {key}",
                                server.name
                            )
                        })?;
                }
            }
        }
        state.config = recorded;
    }
    let name = model_profile
        .or_else(|| {
            saved
                .as_ref()
                .map(|snapshot| snapshot.active_profile.as_str())
        })
        .ok_or("scheduled prompt requires recorded settings or an explicit model profile")?;
    let profile = state
        .config
        .models
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| format!("unknown model profile: {name}"))?;
    state.api_base_url = profile.url.clone();
    state.model_name = profile.model.clone();
    state.config.default = crate::config::DefaultConfig::Table {
        big: name.to_owned(),
        small: state.config.default.small().to_owned(),
    };
    state.agent_mode = state.config.agent_mode;
    state.verbosity = state.config.verbosity.clone();
    state.workspace_root = Some(workspace.to_path_buf());
    state.task_working_directory = Some(workspace.to_path_buf());
    state.history = crate::config::load_session_history_direct(session_id).into();
    state
        .history
        .push(ChatMessage::new("user", prompt.to_owned()));
    state.session_title_tool_available = state.history.len() == 1;
    Ok(state)
}

pub(crate) async fn run_scheduled_turn(
    state: AppState,
    workspace: &std::path::Path,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<String, ScheduledTurnError> {
    let mut owned = crate::mcp::ScheduledServers::new();
    for server in state
        .config
        .mcp_servers
        .iter()
        .filter(|server| server.enabled)
    {
        let result = tokio::select! {
            _ = cancellation.cancelled() => return Err(ScheduledTurnError::Transient("cancelled before prompt dispatch".into())),
            result = crate::mcp::start_owned_server(server, workspace) => result,
        };
        match result {
            Ok(client) => {
                if let Err(error) = owned.insert(client) {
                    dbg_log!("[daemon] MCP startup registration failed: {error}");
                }
            }
            Err(error) => {
                dbg_log!("[daemon] MCP startup failed; continuing without server: {error}");
            }
        }
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| ScheduledTurnError::Transient(error.to_string()))?;
    crate::mcp::DIRECT_MCP_REGISTRY
        .scope(owned.0.clone(), async {
            run_headless_turn_cancellable(&client, Arc::new(Mutex::new(state)), cancellation)
                .await
                .map_err(|error| {
                    if error.downcast_ref::<PreEffectTurnFailure>().is_some() {
                        ScheduledTurnError::Transient(error.to_string())
                    } else {
                        ScheduledTurnError::Ambiguous(error.to_string())
                    }
                })
        })
        .await
}

pub(crate) enum ScheduledTurnError {
    Transient(String),
    Ambiguous(String),
}

#[derive(Debug)]
struct PreEffectTurnFailure(String);
impl std::fmt::Display for PreEffectTurnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PreEffectTurnFailure {}

/// Non-interactive turn policy for `--prompt` execution. Auto-approves tool
/// calls (printing each to stdout) but still enforces plan-mode safety and runs
/// the shared completion/finish gate, so headless runs match interactive
/// execution without ever blocking for TUI confirmation.
pub(crate) struct HeadlessPolicy {
    pub(crate) quiet: bool,
}

fn headless_failure(ctx: &crate::network::TurnContext) -> Option<String> {
    if ctx.lifecycle.task_completed
        && matches!(
            ctx.lifecycle.stop_reason.as_ref(),
            Some(
                crate::network::lifecycle::StopReason::Completed
                    | crate::network::lifecycle::StopReason::CompletedWithWarning(_)
            )
        )
    {
        return None;
    }
    ctx.lifecycle
        .stop_reason
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| Some("turn did not complete".to_string()))
}

/// Tracks only background tasks created by one headless turn. Existing
/// session tasks may continue independently and must not delay this turn's
/// wakeup or have their results injected into its context.
#[derive(Debug, Default)]
struct BackgroundTurnTasks {
    existing: HashSet<String>,
    pending: HashSet<String>,
    terminal: HashSet<String>,
}

impl BackgroundTurnTasks {
    fn new(manager: &TaskManager, session_id: &str) -> Self {
        Self {
            existing: manager
                .list(session_id)
                .into_iter()
                .map(|task| task.id.to_string())
                .collect(),
            ..Self::default()
        }
    }

    fn observe_live_tasks(&mut self, manager: &TaskManager, session_id: &str) {
        for task in manager.list(session_id) {
            let id = task.id.to_string();
            if !self.existing.contains(&id) {
                self.pending.insert(id);
            }
        }
    }

    fn observe_event(&mut self, event: &TaskEvent) -> bool {
        let id = event.task_id().to_string();
        if self.existing.contains(&id) {
            return false;
        }
        self.pending.insert(id.clone());
        if event.is_terminal() {
            self.terminal.insert(id);
        }
        true
    }

    fn complete(&self) -> bool {
        !self.pending.is_empty() && self.pending.len() == self.terminal.len()
    }
}

impl crate::network::policy::TurnPolicy for HeadlessPolicy {
    fn should_approve(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[crate::tools::ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let calls = tool_calls.to_vec();
        let s_clone = Arc::clone(state);
        let quiet = self.quiet;
        async move {
            let s = s_clone.lock().await;
            for call in &calls {
                if !quiet {
                    println!("\n[Headless] Executing Tool: {}", call.name);
                }
                if s.agent_mode == crate::config::AgentMode::Plan
                    && !crate::tools::allowed_in_plan_mode(&call.name)
                {
                    if !quiet {
                        println!("[Headless] Rejected: mutating tool in plan_mode");
                    }
                    return false;
                }
            }
            true
        }
    }

    fn should_verify_completion(&self) -> bool {
        true
    }

    fn is_headless(&self) -> bool {
        true
    }
}

pub async fn run_headless_turn(
    client: &reqwest::Client,
    state_arc: Arc<Mutex<AppState>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cancel_token = tokio_util::sync::CancellationToken::new();
    run_headless_turn_cancellable(client, state_arc, cancel_token).await
}

pub(crate) async fn run_headless_turn_cancellable(
    client: &reqwest::Client,
    state_arc: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
) -> Result<String, Box<dyn std::error::Error>> {
    let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));

    let quiet = !state_arc.lock().await.raw_cli_mode;
    let policy = Arc::new(HeadlessPolicy { quiet });

    if !quiet {
        println!("Starting headless agent loop...");
    }
    let session_id = state_arc.lock().await.active_session_id.clone();
    let task_manager = crate::tools::background_task_manager();
    let task_subscription = task_manager.subscribe_session(session_id.clone());
    let mut turn_tasks = BackgroundTurnTasks::new(task_manager, &session_id);

    // Drive the prompt through the same lifecycle as the interactive
    // orchestrator. A background command pauses a logical turn rather than
    // completing it: wait for its session-scoped task event, inject the
    // terminal result, and resume with the existing budgets/verification
    // ledger.
    let mut ctx =
        crate::network::run_agent_turn(client, &state_arc, &cancel_token, &policy, &stream_buffer)
            .await;
    while matches!(
        ctx.lifecycle.stop_reason,
        Some(crate::network::lifecycle::StopReason::BackgroundPending)
    ) {
        let session_id = state_arc.lock().await.active_session_id.clone();
        turn_tasks.observe_live_tasks(task_manager, &session_id);
        loop {
            let first_event = loop {
                match task_subscription.try_recv() {
                    Ok(event) => break event,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        // TaskSubscription uses a synchronous channel, but
                        // polling it this way keeps Tokio's executor free.
                        tokio::select! {
                            _ = cancel_token.cancelled() => return Err(std::io::Error::other("headless turn cancelled").into()),
                            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return Err(std::io::Error::other(
                            "background task subscription closed before completion",
                        )
                        .into());
                    }
                }
            };
            let mut events = vec![first_event];
            if !crate::tools::has_background_tasks(&session_id) {
                while let Ok(event) = task_subscription.try_recv() {
                    events.push(event);
                }
            }
            for event in events {
                let is_turn_event = turn_tasks.observe_event(&event);
                if let Some((task_id, event_session_id, output)) = is_turn_event
                    .then(|| crate::tools::task_event_to_tool_output(event))
                    .flatten()
                {
                    let mut state = state_arc.lock().await;
                    state
                        .history
                        .push(background_task_history_message(&task_id, output));
                    crate::config::save_session_history(&event_session_id, &state.history);
                }
            }
            turn_tasks.observe_live_tasks(task_manager, &session_id);
            if turn_tasks.complete() {
                break;
            }
        }
        ctx = crate::network::run_agent_turn_with_context(
            client,
            &state_arc,
            &cancel_token,
            &policy,
            &stream_buffer,
            ctx,
        )
        .await;
    }

    let prose = crate::network::text::strip_tool_call_syntax(&ctx.response.final_content);
    if !quiet && !prose.trim().is_empty() {
        println!("\nAssistant: {}", prose.trim());
    }

    if let Some(reason) = headless_failure(&ctx) {
        if ctx.metrics.mutating_tool_calls == 0
            && ctx.metrics.provider_errors > 0
        {
            return Err(Box::new(PreEffectTurnFailure(format!(
                "headless startup failed ({reason})"
            ))));
        }
        return Err(std::io::Error::other(format!(
            "headless turn incomplete ({reason}); task is not complete"
        ))
        .into());
    }

    Ok(prose.trim().to_string())
}

/// Autonomous headless loop (BigHead-style): repeat headless turns on one
/// session until the model emits `<loop:done/>` or the iteration cap hits.
/// The cap (max 10) is the circuit breaker against runaway sessions.
pub const LOOP_DONE_MARKER: &str = "<loop:done/>";
pub const MAX_LOOP_ITERS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopReport {
    pub iters: usize,
    pub completed_via_done: bool,
}

pub fn loop_should_continue(output: &str, iter: usize, max: usize) -> bool {
    iter < max && !output.contains(LOOP_DONE_MARKER)
}

pub async fn run_raw_cli_loop(
    prompt: &str,
    model_override: Option<&str>,
    max_iters: usize,
) -> Result<LoopReport, Box<dyn std::error::Error>> {
    let cap = max_iters.clamp(1, MAX_LOOP_ITERS);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;

    let state = build_state(prompt, model_override);
    let state_arc = Arc::new(Mutex::new(state));

    let mcp_servers = state_arc.lock().await.config.mcp_servers.clone();
    for warning in crate::mcp::start_enabled_servers(&mcp_servers, |name| async move {
        crate::mcp::start_server_by_name(&name).await
    })
    .await
    {
        eprintln!("{warning}");
    }

    let mut completed_via_done = false;
    let mut iters = 0;
    for iter in 1..=cap {
        iters = iter;
        let output = run_headless_turn(&client, Arc::clone(&state_arc)).await?;
        if !loop_should_continue(&output, iter, cap) {
            completed_via_done = output.contains(LOOP_DONE_MARKER);
            break;
        }
        state_arc.lock().await.history.push(ChatMessage::new(
            "user",
            "Continue the task. Emit `<loop:done/>` when fully complete.".to_string(),
        ));
    }
    Ok(LoopReport {
        iters,
        completed_via_done,
    })
}

/// Entry point for the raw CLI agent mode.
pub async fn run_raw_cli(
    prompt: &str,
    model_override: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let tokens = prompt.split_whitespace().collect::<Vec<_>>();
    if tokens.first() == Some(&"/memory") && tokens.len() > 1 {
        if let Some(message) =
            crate::memory::command(std::env::current_dir().ok().as_deref(), &tokens[1..])
        {
            println!("{message}");
            return Ok(());
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;

    let state = build_state(prompt, model_override);

    let state_arc = Arc::new(Mutex::new(state));

    let mcp_servers = state_arc.lock().await.config.mcp_servers.clone();
    for warning in crate::mcp::start_enabled_servers(&mcp_servers, |name| async move {
        crate::mcp::start_server_by_name(&name).await
    })
    .await
    {
        eprintln!("{warning}");
    }

    run_headless_turn(&client, state_arc).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::policy::TurnPolicy;
    use crate::tools::ToolCall;

    fn call(name: &str) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            arguments: serde_json::json!({}),
            call_id: None,
        }
    }

    #[test]
    fn build_state_seeds_user_prompt_and_raw_mode() {
        let state = build_state("inspect the project", None);
        assert!(state.raw_cli_mode);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].role, "user");
        assert_eq!(state.history[0].content, "inspect the project");
    }

    #[tokio::test]
    async fn headless_policy_auto_approves_in_build_mode() {
        let mut state = build_state("edit a file", None);
        state.agent_mode = crate::config::AgentMode::Build;
        let state = Arc::new(Mutex::new(state));

        let approved = HeadlessPolicy { quiet: true }
            .should_approve(&state, &[call("write_file")])
            .await;
        assert!(approved, "headless build mode must not block on approval");
    }

    #[tokio::test]
    async fn headless_policy_rejects_mutating_tool_in_plan_mode() {
        let mut state = build_state("plan only", None);
        state.agent_mode = crate::config::AgentMode::Plan;
        let state = Arc::new(Mutex::new(state));

        let approved = HeadlessPolicy { quiet: true }
            .should_approve(&state, &[call("write_file")])
            .await;
        assert!(!approved, "plan mode must reject mutating tools headlessly");
    }

    #[test]
    fn headless_policy_verifies_completion_like_interactive() {
        // The finish gate (compiler/build verification before accepting done)
        // is driven by this flag; raw CLI must match interactive behavior.
        assert!(HeadlessPolicy { quiet: true }.should_verify_completion());
    }

    #[test]
    fn headless_failure_reports_unfinished_turns() {
        let mut ctx = crate::network::TurnContext::default();
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::BudgetExceeded(
            "token budget".to_string(),
        ));
        assert_eq!(
            headless_failure(&ctx).as_deref(),
            Some("budget:token budget")
        );

        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::Completed);
        assert!(headless_failure(&ctx).is_none());
    }

    #[test]
    fn exhausted_reasoning_recovery_with_honest_terminal_message_is_incomplete() {
        let mut ctx = crate::network::TurnContext::default();
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::LoopEscalation);
        ctx.recovery.reasoning_recovery_attempts = 1;
        ctx.response.final_content =
            "I stopped after repeated reasoning to avoid looping. Please review the current changes and continue from there.".to_string();

        assert_eq!(headless_failure(&ctx).as_deref(), Some("loop_escalation"));
        assert!(!ctx.lifecycle.task_completed);
    }

    #[test]
    fn reasoning_recovery_without_usable_content_stays_incomplete() {
        let mut ctx = crate::network::TurnContext::default();
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::LoopEscalation);
        ctx.recovery.reasoning_recovery_attempts = 1;
        ctx.response.final_content = "<think>still reasoning</think>".to_string();

        assert_eq!(headless_failure(&ctx).as_deref(), Some("loop_escalation"));
    }

    #[test]
    fn forced_tool_loop_final_remains_incomplete_even_with_prose() {
        let mut ctx = crate::network::TurnContext::default();
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::LoopEscalation);
        ctx.recovery.reasoning_recovery_attempts = 1;
        ctx.recovery.force_final = true;
        ctx.response.final_content = "I stopped safely.".to_string();

        assert_eq!(headless_failure(&ctx).as_deref(), Some("loop_escalation"));
    }

    #[test]
    fn completed_read_only_review_has_a_successful_headless_terminal_status() {
        let mut ctx = crate::network::TurnContext::default();
        ctx.progress.complete_inspection_results = 4;
        ctx.lifecycle.task_completed = true;
        ctx.lifecycle.stop_reason = Some(crate::network::lifecycle::StopReason::Completed);

        assert!(headless_failure(&ctx).is_none());
    }

    #[test]
    fn background_completion_is_preserved_as_typed_tool_evidence() {
        let message = background_task_history_message(
            "task-7",
            crate::tools::ToolExecutionOutput {
                content: "tests failed".to_string(),
                success: false,
                pending: false,
                command: Some("cargo test".to_string()),
                exit_code: Some(1),
                truncated: false,
                completeness: rustcode_core::ToolResultCompleteness::Complete,
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
                command_status: None,
            },
        );

        assert_eq!(message.role, "tool");
        assert!(message.content.contains("Task task-7 completed"));
        assert!(message.content.contains("tests failed"));
        let record = message.tool_result.expect("typed result metadata");
        assert!(!record.success);
        assert_eq!(record.exit_code, Some(1));
        assert_eq!(record.command.as_deref(), Some("cargo test"));
    }

    #[test]
    fn headless_turn_tracks_only_new_tasks_and_waits_for_all_terminals() {
        let mut tracker = BackgroundTurnTasks {
            existing: ["old-task".to_owned()].into_iter().collect(),
            ..BackgroundTurnTasks::default()
        };
        let old = TaskEvent::Finished {
            id: "old-task".into(),
            session_id: "session".into(),
            call_id: None,
            command: "sleep 1".to_owned(),
            output: Ok(rustcode_command::CommandOutput {
                success: true,
                exit_code: Some(0),
                signal: None,
                downstream_consumer_terminated: false,
                stdout: Default::default(),
                stderr: Default::default(),
            }),
        };
        assert!(!tracker.observe_event(&old));

        let first = TaskEvent::Started {
            id: "new-a".into(),
            session_id: "session".into(),
            call_id: None,
            pid: 10,
        };
        let second = TaskEvent::Started {
            id: "new-b".into(),
            session_id: "session".into(),
            call_id: None,
            pid: 11,
        };
        assert!(tracker.observe_event(&first));
        assert!(tracker.observe_event(&second));
        assert!(!tracker.complete());

        let finished = TaskEvent::Finished {
            id: "new-a".into(),
            session_id: "session".into(),
            call_id: None,
            command: "cargo test".to_owned(),
            output: Ok(rustcode_command::CommandOutput {
                success: true,
                exit_code: Some(0),
                signal: None,
                downstream_consumer_terminated: false,
                stdout: Default::default(),
                stderr: Default::default(),
            }),
        };
        assert!(tracker.observe_event(&finished));
        assert!(!tracker.complete());

        let cancelled = TaskEvent::Cancelled {
            id: "new-b".into(),
            session_id: "session".into(),
            call_id: None,
            command: "cargo check".to_owned(),
        };
        assert!(tracker.observe_event(&cancelled));
        assert!(tracker.complete());
    }

    #[test]
    fn loop_breaker_stops_at_done_marker_or_cap() {
        assert!(super::loop_should_continue("work continues", 1, 5));
        assert!(!super::loop_should_continue("all done <loop:done/>", 1, 5));
        assert!(!super::loop_should_continue("work continues", 5, 5));
        assert_eq!(super::MAX_LOOP_ITERS, 10);
    }
}
