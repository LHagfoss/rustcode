use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

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
    // Drive the prompt through the SAME shared turn lifecycle the interactive
    // orchestrator uses (TurnMachine, approval/finish gates, build verification,
    // token/history persistence). HeadlessPolicy keeps it non-interactive.
    let ctx =
        crate::network::run_agent_turn(client, &state_arc, &cancel_token, &policy, &stream_buffer)
            .await;

    let prose = crate::network::text::strip_tool_call_syntax(&ctx.final_content);
    if !quiet && !prose.trim().is_empty() {
        println!("\nAssistant: {}", prose.trim());
    }

    Ok(prose.trim().to_string())
}

/// Entry point for the raw CLI agent mode.
pub async fn run_raw_cli(
    prompt: &str,
    model_override: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    run_headless_turn(&client, state_arc).await.map(|_| ())
}

fn format_history_for_terminal(history: &[ChatMessage]) -> String {
    let mut rendered = Vec::new();

    for message in history {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }

        let line = match message.role.as_str() {
            "user" => format!("❯ {content}"),
            "assistant" => format!("Assistant: {content}"),
            "tool" => format!("● {content}"),
            "system" if !is_hidden_terminal_notice(content) => content.to_string(),
            _ => continue,
        };
        rendered.push(line);
    }

    rendered.join("\n\n")
}

fn is_hidden_terminal_notice(content: &str) -> bool {
    content.contains("Loop warning:")
        || content.contains("tool calls in that response were dropped")
        || content.contains("Oversized response:")
        || content.starts_with(crate::network::compaction::SUMMARY_MARKER)
        || content.starts_with("Resumed session ")
}

/// Interactive CLI REPL mode (Codex & Claude CLI style).
/// Operates directly in standard terminal mode without raw mode or alternate screen.
/// Every user input, assistant response, and tool execution appends directly to stdout.
pub async fn run_interactive_cli(
    model_override: Option<&str>,
    resume: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, IsTerminal, Write};

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;

    let mut state = AppState::new();
    state.raw_cli_mode = true;

    if let Some(m_name) = model_override {
        if let Some(profile) = state.config.models.iter().find(|m| m.name == m_name) {
            state.api_base_url = profile.url.clone();
            state.model_name = profile.model.clone();
        }
    }

    crate::config::archive_live_history();

    if resume {
        crate::app::resume_latest_session(&mut state);
        let transcript = format_history_for_terminal(&state.history);
        if !transcript.is_empty() {
            println!("{transcript}\n");
        }
    }

    let state_arc = Arc::new(Mutex::new(state));
    let stdin = io::stdin();

    println!("rustcode v{} · interactive mode", env!("CARGO_PKG_VERSION"));
    println!("Type your question and press Enter. Type /exit or /quit to leave.\n");

    loop {
        print!("❯ ");
        io::stdout().flush()?;

        let mut input = String::new();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "/exit" || trimmed == "/quit" {
            println!("Goodbye!");
            break;
        }

        if trimmed == "/clear" {
            let mut s = state_arc.lock().await;
            s.history.clear();
            println!("Cleared session history.\n");
            continue;
        }

        if !stdin.is_terminal() {
            println!("❯ {trimmed}");
        }
        let history_start = {
            let mut s = state_arc.lock().await;
            s.history
                .push(ChatMessage::new("user", trimmed.to_string()));
            s.status = crate::app::AppStatus::Streaming;
            s.history.len()
        };

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer::new()));
        let policy = Arc::new(HeadlessPolicy { quiet: false });
        let _ctx = crate::network::run_agent_turn(
            &client,
            &state_arc,
            &cancel_token,
            &policy,
            &stream_buffer,
        )
        .await;

        {
            let mut s = state_arc.lock().await;
            s.status = crate::app::AppStatus::Idle;
            let session_id = s.active_session_id.clone();
            let new_history = s
                .history
                .get(history_start..)
                .unwrap_or_default()
                .iter()
                .filter(|message| message.role != "assistant")
                .cloned()
                .collect::<Vec<_>>();
            crate::config::save_session_history(&session_id, &s.history);
            drop(s);

            let transcript = format_history_for_terminal(&new_history);
            if !transcript.is_empty() {
                println!("\n{transcript}");
            }
        }

        println!();
    }

    Ok(())
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

    #[test]
    fn formats_complete_history_for_native_terminal_scrollback() {
        let history = vec![
            ChatMessage::new("user", "inspect the project"),
            ChatMessage::new("assistant", "I found the project."),
            ChatMessage::new("tool", "run_command: cargo check"),
            ChatMessage::new("system", "Workspace is clean"),
        ];

        let rendered = format_history_for_terminal(&history);

        assert!(rendered.contains("❯ inspect the project"));
        assert!(rendered.contains("Assistant: I found the project."));
        assert!(rendered.contains("● run_command: cargo check"));
        assert!(rendered.contains("Workspace is clean"));
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
}
