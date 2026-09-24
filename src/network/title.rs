use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

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
