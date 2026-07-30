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

/// Convert internal chat history into API message format, with tool results
/// wrapped in `<tool_result>` tags and user messages passing through multimodal parsing.
/// Applies history compaction to keep the prompt under token budget.
pub async fn build_messages(state: &AppState) -> Vec<serde_json::Value> {
    let protocol = state.config.tool_protocol;
    let system_prompt = crate::tools::tool_system_prompt(false, protocol, state.agent_mode);

    let mut history_snapshot: Vec<ChatMessage> = state
        .history
        .iter()
        .filter(|m| {
            matches!(m.role.as_str(), "user" | "assistant" | "tool")
                && !m.content.starts_with('/')
        })
        .cloned()
        .collect();

    let budget_token_limit = state.get_history_token_budget();
    crate::network::compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

    crate::network::history::to_messages(&history_snapshot, system_prompt)
}

/// Prompt the user to confirm tool execution and run it if confirmed.
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
    let mut ctx = crate::network::TurnContext::new();

    println!("Starting headless agent loop...");
    while !cancel_token.is_cancelled() {
        if !crate::network::run_single_turn(client, &state_arc, &cancel_token, &policy, &stream_buffer, &mut ctx).await {
            break;
        }
    }

    if !ctx.final_content.is_empty() {
        let prose = crate::network::text::strip_tool_call_syntax(&ctx.final_content);
        if !prose.trim().is_empty() {
            println!("\nAssistant: {}", prose.trim());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_messages_keeps_system_prompt_first() {
        let state = build_state("inspect the project", None);

        let messages = build_messages(&state).await;

        assert_eq!(
            messages.first().and_then(|m| m["role"].as_str()),
            Some("system")
        );
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("rustcode")
        );
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "inspect the project");
    }

    #[tokio::test]
    async fn build_messages_encodes_tool_results_as_user_context() {
        let mut state = build_state("make the check pass", None);
        state
            .history
            .push(ChatMessage::new("assistant", "I will inspect the error."));
        state
            .history
            .push(ChatMessage::new("tool", "run_command: error output"));

        let messages = build_messages(&state).await;

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(
            messages[3]["content"],
            "<tool_result>\nrun_command: error output\n</tool_result>"
        );
    }

    #[tokio::test]
    async fn build_messages_excludes_slash_commands_from_model_history() {
        let mut state = build_state("continue", None);
        state.history.push(ChatMessage::new("user", "/delegate"));
        state.history.push(ChatMessage::new("assistant", "working"));

        let messages = build_messages(&state).await;

        assert!(messages.iter().all(|message| {
            message["content"]
                .as_str()
                .map(|content| !content.starts_with('/'))
                .unwrap_or(true)
        }));
        assert_eq!(
            messages.last().and_then(|m| m["content"].as_str()),
            Some("working")
        );
    }

    #[tokio::test]
    async fn build_messages_preserves_multimodal_user_content() {
        let mut state = build_state("look at this", None);
        state.history.push(ChatMessage::new(
            "user",
            "before ![image](file:///tmp/missing.png) after",
        ));

        let messages = build_messages(&state).await;
        let content = messages.last().unwrap()["content"].as_array().unwrap();

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "before ");
        assert_eq!(content[1]["text"], "![image](file:///tmp/missing.png)");
        assert_eq!(content[2]["text"], " after");
    }
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
        if let Some(title) = crate::network::generate_title(&client_clone, &config_clone, &prompt_str).await {
            crate::config::save_session_title(&session_id, &title);
        }
    });

    let state_arc = Arc::new(Mutex::new(state));

    run_round_loop(&client, state_arc).await
}
