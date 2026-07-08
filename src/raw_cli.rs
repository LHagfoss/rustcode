use std::sync::Arc;
use tokio::sync::Mutex;
use crate::app::{AppState, ChatMessage};

pub async fn run_raw_cli(prompt: &str, model_override: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()?;

    let mut state = AppState::new();
    state.raw_cli_mode = true;
    if let Some(m_name) = model_override {
        if let Some(profile) = state.config.models.iter().find(|m| m.name == m_name) {
            state.api_base_url = profile.url.clone();
            state.model_name = profile.model.clone();
            println!("Overriding model profile to: {} ({})", m_name, profile.model);
        } else {
            println!("Warning: Model profile '{}' not found in config.toml. Using default.", m_name);
        }
    }
    state.history.push(ChatMessage::new("user", prompt.to_string()));

    let state_arc = Arc::new(Mutex::new(state));
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let mut rounds = 0;
    loop {
        println!("\n=== Round {} ===", rounds);
        {
            let mut s = state_arc.lock().await;
            s.current_response.clear();
        }
        let history_snapshot: Vec<ChatMessage> = {
            let s = state_arc.lock().await;
            s.history
                .iter()
                .filter(|m| {
                    matches!(m.role.as_str(), "user" | "assistant" | "tool")
                        && !m.content.starts_with('/')
                })
                .cloned()
                .collect()
        };

        let system_prompt = crate::tools::tool_system_prompt(false);
        let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt.clone(),
        })];
        let mut first_user = true;
        msgs.extend(history_snapshot.into_iter().map(|m| {
            if m.role == "tool" {
                serde_json::json!({
                    "role": "user",
                    "content": format!("<tool_result>\n{}\n</tool_result>", m.content),
                })
            } else if m.role == "user" && first_user {
                first_user = false;
                serde_json::json!({
                    "role": "user",
                    "content": crate::network::parse_multimodal_content(&m.content),
                })
            } else if m.role == "user" {
                serde_json::json!({
                    "role": "user",
                    "content": crate::network::parse_multimodal_content(&m.content),
                })
            } else {
                serde_json::json!({"role": m.role, "content": m.content})
            }
        }));

        let stream_buffer = Arc::new(Mutex::new(crate::network::StreamBuffer {
            content: String::new(),
        }));

        let (api_base_url, model_name) = {
            let s = state_arc.lock().await;
            (s.api_base_url.clone(), s.model_name.clone())
        };

        println!("Streaming response from {}...", model_name);
        if let Err(e) = crate::network::stream_request(
            &client,
            state_arc.clone(),
            cancel_token.clone(),
            &api_base_url,
            &model_name,
            &msgs,
            stream_buffer.clone(),
            false,
        )
        .await {
            println!("Stream error: {}", e);
            break;
        }

        println!();

        let response_content = {
            let s = state_arc.lock().await;
            s.current_response.clone()
        };

        if let Some((tool_name, tool_args)) = crate::tools::parse_tool_call(&response_content) {
            println!("\nDetected Tool Call:");
            println!("  Name: {}", tool_name);
            println!("  Arguments: {}", serde_json::to_string_pretty(&tool_args).unwrap_or_default());
            
            print!("\nExecute tool? (y/N): ");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            let mut user_input = String::new();
            std::io::stdin().read_line(&mut user_input)?;
            let trimmed = user_input.trim().to_lowercase();
            if trimmed == "y" || trimmed == "yes" {
                println!("Executing tool...");
                let result = crate::tools::execute(&tool_name, &tool_args);
                println!("Result: {}", result);
                let mut s = state_arc.lock().await;
                s.history.push(ChatMessage::new("assistant", response_content));
                s.history.push(ChatMessage::new("tool", result));
            } else {
                println!("Tool call rejected. Exiting agent loop.");
                break;
            }
        } else {
            println!("\nNo tool call detected. Agent loop finished.");
            break;
        }

        rounds += 1;
        if rounds >= 10 {
            println!("Reached max rounds (10). Exiting.");
            break;
        }
    }

    Ok(())
}
