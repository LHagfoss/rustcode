use crate::app::{AppState, ChatMessage};
use std::io::{self, Write};
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::Mutex;
const MAX_ROUNDS: u32 = 10;

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
    let system_prompt = crate::tools::tool_system_prompt(false, protocol);

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

    let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt,
    })];

    let mut first_user = true;
    for m in history_snapshot {
        let msg = match m.role.as_str() {
            "tool" => serde_json::json!({
                "role": "user",
                "content": format!("<tool_result>\n{}\n</tool_result>", m.content),
            }),
            "user" if first_user => {
                first_user = false;
                serde_json::json!({
                    "role": "user",
                    "content": crate::network::parse_multimodal_content(&m.content),
                })
            }
            "user" => serde_json::json!({
                "role": "user",
                "content": crate::network::parse_multimodal_content(&m.content),
            }),
            _ => serde_json::json!({"role": m.role, "content": m.content}),
        };
        msgs.push(msg);
    }

    msgs
}

/// Prompt the user to confirm tool execution and run it if confirmed.
pub async fn execute_tool_if_approved(
    state_arc: &Arc<Mutex<AppState>>,
    response_content: String,
) -> Option<String> {
    let protocol = { state_arc.lock().await.config.tool_protocol };

    let (tool_name, tool_args) = crate::tools::parse_tool_call(&response_content, protocol)?;

    println!("\nDetected Tool Call:");
    println!("  Name: {}", tool_name);
    println!(
        "  Arguments: {}",
        serde_json::to_string_pretty(&tool_args).unwrap_or_default()
    );

    print!("\nExecute tool? (y/N): ");
    let _ = io::stdout().flush();

    let mut user_input = String::new();
    if io::stdin().read_line(&mut user_input).is_err() {
        println!("Failed to read input. Exiting.");
        return None;
    }

    match user_input.trim().to_lowercase().as_str() {
        "y" | "yes" => {
            println!("Executing tool...");
            let result = crate::tools::execute(&tool_name, &tool_args);
            println!("Result: {}", result);

            // Record assistant response and tool result in history.
            let mut s = state_arc.lock().await;
            s.history.push(ChatMessage::new("assistant", response_content));
            s.history.push(ChatMessage::new("tool", result.clone()));
            Some(result)
        }
        _ => {
            println!("Tool call rejected. Exiting agent loop.");
            None
        }
    }
}

/// Execute the main agent round loop: stream a response, detect tool calls,
/// and repeat until no tool is invoked or max rounds are reached.
pub async fn run_round_loop(
    client: &reqwest::Client,
    state_arc: Arc<Mutex<AppState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cancel_token = tokio_util::sync::CancellationToken::new();

    for rounds in 0..=MAX_ROUNDS {
        println!("\n=== Round {} ===", rounds);

        // Reset streaming buffer.
        {
            let mut s = state_arc.lock().await;
            s.current_response.clear();
        }

        let msgs = {
            let state_guard = state_arc.lock().await;
            build_messages(state_guard.deref()).await
        };

        let (api_base_url, model_name) = {
            let s = state_arc.lock().await;
            (s.api_base_url.clone(), s.model_name.clone())
        };

        println!("Streaming response from {}...", model_name);

        let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer {
            content: String::new(),
        }));

        if let Err(e) = crate::network::stream_request(
            client,
            state_arc.clone(),
            cancel_token.clone(),
            &api_base_url,
            &model_name,
            &msgs,
            stream_buffer.clone(),
            false,
        )
        .await
        {
            println!("Stream error: {}", e);
            break;
        }

        println!();

        let response_content = { state_arc.lock().await.current_response.clone() };

        if let Some(result) = execute_tool_if_approved(&state_arc, response_content).await {
            // Tool was executed — loop continues with updated history.
            drop(result);
        } else {
            if rounds < MAX_ROUNDS {
                println!("\nNo tool call detected. Agent loop finished.");
            }
            break;
        }

        if rounds + 1 >= MAX_ROUNDS {
            println!("Reached max rounds ({}). Exiting.", MAX_ROUNDS);
            break;
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
        if let Some(title) = crate::network::generate_title(&client_clone, &config_clone, &prompt_str).await {
            crate::config::save_session_title(&session_id, &title);
        }
    });

    let state_arc = Arc::new(Mutex::new(state));

    run_round_loop(&client, state_arc).await
}
