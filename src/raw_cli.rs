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
            println!("Overriding model profile to: {} ({})", m_name, profile.model);
        } else {
            println!(
                "Warning: Model profile '{}' not found in config.toml. Using default.",
                m_name
            );
        }
    }

    state.history.push(ChatMessage::new("user", prompt.to_string()));
    state
}

/// Non-interactive turn policy for `--prompt` execution. Auto-approves tool
/// calls (printing each to stdout) but still enforces plan-mode safety and runs
/// the shared completion/finish gate, so headless runs match interactive
/// execution without ever blocking for TUI confirmation.
pub(crate) struct HeadlessPolicy;

impl crate::network::policy::TurnPolicy for HeadlessPolicy {
    fn should_approve(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[crate::tools::ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let calls = tool_calls.to_vec();
        let s_clone = Arc::clone(state);
        async move {
            let s = s_clone.lock().await;
            for call in &calls {
                println!("\n[Headless] Executing Tool: {}", call.name);
                if s.agent_mode == crate::config::AgentMode::Plan && !crate::tools::allowed_in_plan_mode(&call.name) {
                    println!("[Headless] Rejected: mutating tool in plan_mode");
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

pub async fn run_round_loop(
    client: &reqwest::Client,
    state_arc: Arc<Mutex<AppState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer {
        content: String::new(),
    }));

    let policy = Arc::new(HeadlessPolicy);

    println!("Starting headless agent loop...");
    // Drive the prompt through the SAME shared turn lifecycle the interactive
    // orchestrator uses (TurnMachine, approval/finish gates, build verification,
    // token/history persistence). HeadlessPolicy keeps it non-interactive.
    let ctx =
        crate::network::run_agent_turn(client, &state_arc, &cancel_token, &policy, &stream_buffer)
            .await;

    if !ctx.final_content.is_empty() {
        let prose = crate::network::text::strip_tool_call_syntax(&ctx.final_content);
        if !prose.trim().is_empty() {
            println!("\nAssistant: {}", prose.trim());
        }
    }

    Ok(())
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

    run_round_loop(&client, state_arc).await
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

    #[tokio::test]
    async fn headless_policy_auto_approves_in_build_mode() {
        let mut state = build_state("edit a file", None);
        state.agent_mode = crate::config::AgentMode::Build;
        let state = Arc::new(Mutex::new(state));

        let approved = HeadlessPolicy
            .should_approve(&state, &[call("write_file")])
            .await;
        assert!(approved, "headless build mode must not block on approval");
    }

    #[tokio::test]
    async fn headless_policy_rejects_mutating_tool_in_plan_mode() {
        let mut state = build_state("plan only", None);
        state.agent_mode = crate::config::AgentMode::Plan;
        let state = Arc::new(Mutex::new(state));

        let approved = HeadlessPolicy
            .should_approve(&state, &[call("write_file")])
            .await;
        assert!(!approved, "plan mode must reject mutating tools headlessly");
    }

    #[test]
    fn headless_policy_verifies_completion_like_interactive() {
        // The finish gate (compiler/build verification before accepting done)
        // is driven by this flag; raw CLI must match interactive behavior.
        assert!(HeadlessPolicy.should_verify_completion());
    }
}
