use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Generate a title from the first user message using the small model.
/// Returns None if the message starts with '/' (slash command).
pub async fn generate_title(
    client: &reqwest::Client,
    config: &crate::config::AppConfig,
    first_message: &str,
) -> Option<String> {
    if first_message.trim().starts_with('/') {
        return None;
    }

    let small_model_name = config.default.small();
    let (url, model) = crate::config::resolve_model_endpoint(config, small_model_name);

    let first_line = first_message.lines().next()?;
    let prompt = format!(
        "Generate a short, concise title (max 5 words) summarizing this user's coding request/intent. Do not use quotes, punctuation, or any introductory text. Return only the title itself.\n\nIntent: {}",
        first_line.trim()
    );

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": prompt
    })];

    let payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": 30,
        "temperature": 0.3,
    });

    let res = client.post(&url).json(&payload).send().await.ok()?;

    if !res.status().is_success() {
        return None;
    }

    let json: serde_json::Value = res.json().await.ok()?;
    let title = json
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?;

    let cleaned_title = title.trim().trim_matches('"').trim().to_string();
    if cleaned_title.is_empty() {
        None
    } else {
        Some(cleaned_title)
    }
}

/// Push an incoming user prompt onto history, then reset per-response scratch
/// fields. A background wakeup already has a durable tool result in history, so
/// adding a second system notice would create redundant transcript chatter.
fn prompt_history_message(is_wakeup: bool, next_prompt: &str) -> Option<ChatMessage> {
    (!is_wakeup).then(|| ChatMessage::new("user", next_prompt.to_string()))
}

pub(crate) async fn record_prompt_to_history(
    state: &Arc<Mutex<AppState>>,
    is_wakeup: bool,
    next_prompt: &str,
    expected_session_id: &str,
) -> bool {
    let mut s = state.lock().await;
    if s.active_session_id != expected_session_id {
        return false;
    }
    let active_id = s.active_session_id.clone();
    if let Some(message) = prompt_history_message(is_wakeup, next_prompt) {
        s.history.push(message);
        crate::config::save_session_title_if_absent(&active_id, &s.history);
    }
    crate::config::save_session_history(&active_id, &s.history);
    s.clear_current_response();
    s.current_token_usage = None;
    s.response_time = None;
    true
}

#[cfg(test)]
mod tests {
    use super::prompt_history_message;

    #[test]
    fn background_wakeup_does_not_add_redundant_system_chatter() {
        assert!(prompt_history_message(true, "__task_wakeup__:task_42").is_none());
        let user = prompt_history_message(false, "continue").expect("user prompt");
        assert_eq!(user.role, "user");
        assert_eq!(user.content, "continue");
    }
}
