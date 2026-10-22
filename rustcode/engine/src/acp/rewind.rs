//! `_rustcode/rewind` (#2278): the macOS app's Retry. The chat goes back to
//! just before one of its messages, dropping that message and everything
//! after it, so the app can send it again without a duplicate.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/rewind", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RewindRequest {
    pub session_id: String,
    /// Which of the chat's messages, counting from 0, as a replay shows them.
    pub user_index: usize,
    /// The message's text, so a chat that changed underneath is not cut in
    /// the wrong place.
    pub text: String,
}

/// The history index of the `user_index`th message a replay shows.
fn user_message_position(history: &[crate::app::ChatMessage], user_index: usize) -> Option<usize> {
    history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "user" && !message.content.is_empty())
        .nth(user_index)
        .map(|(position, _)| position)
}

/// Cut the session's history before its `user_index`th message when that
/// message reads `text`. Only an idle chat rewinds.
pub(crate) fn rewind(
    state: &mut crate::app::AppState,
    user_index: usize,
    text: &str,
) -> Result<Value, agent_client_protocol::Error> {
    let invalid =
        |message: &str| agent_client_protocol::Error::invalid_params().data(message.to_owned());
    if state.status != crate::app::AppStatus::Idle {
        return Err(invalid("the chat is busy; retry once its turn ends"));
    }
    let position = user_message_position(&state.history, user_index)
        .filter(|position| state.history[*position].content.trim() == text.trim())
        .ok_or_else(|| invalid("that message is no longer in the chat"))?;
    let removed = state.history.len() - position;
    let mut history = state.history.to_vec();
    history.truncate(position);
    state.history = history.into();
    // A parked turn belonged to the history that is gone.
    state.background_turn_context = None;
    crate::config::save_session_history(&state.active_session_id, &state.history);
    Ok(json!({ "removed": removed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, ChatMessage};

    fn chat() -> AppState {
        let mut state = AppState::new();
        for message in [
            ChatMessage::new("user", "first"),
            ChatMessage::new("assistant", "one"),
            ChatMessage::new("system", "notice"),
            ChatMessage::new("user", "second"),
            ChatMessage::new("assistant", "two"),
        ] {
            state.history.push(message);
        }
        state
    }

    #[test]
    fn rewinds_to_just_before_the_message() {
        let mut state = chat();
        assert_eq!(
            rewind(&mut state, 1, "second").unwrap(),
            json!({"removed": 2})
        );
        assert_eq!(state.history.len(), 3);
        assert_eq!(state.history[0].content, "first");
    }

    #[test]
    fn refuses_a_message_that_changed_or_a_busy_chat() {
        let mut state = chat();
        assert!(rewind(&mut state, 1, "first").is_err());
        assert!(rewind(&mut state, 5, "second").is_err());
        state.status = crate::app::AppStatus::Streaming;
        assert!(rewind(&mut state, 0, "first").is_err());
        assert_eq!(state.history.len(), 5);
    }
}
