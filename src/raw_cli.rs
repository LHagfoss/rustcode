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
    state
}

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
}

pub async fn run_headless_turn(
    client: &reqwest::Client,
    state_arc: Arc<Mutex<AppState>>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cancel_token = tokio_util::sync::CancellationToken::new();
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
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
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
        return Err(std::io::Error::other(format!(
            "headless turn incomplete ({reason}); task is not complete"
        ))
        .into());
    }

    Ok(prose.trim().to_string())
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

    let client_clone = client.clone();
    let config_clone = state.config.clone();
    let session_id = state.active_session_id.clone();
    let prompt_str = prompt.to_string();
    tokio::spawn(async move {
        if let Some(title) =
            crate::network::generate_title(&client_clone, &config_clone, &prompt_str).await
        {
            crate::config::save_session_title(&session_id, &title);
        }
    });

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
}
