//! A thread moved from an external harness to the built-in one. The external
//! transcript stays in `history.json` for display, but its tool calls were
//! recorded for cards, not as replayable provider calls. So the switch marker
//! carries the `<context>` preamble once, and the built-in request sees only
//! what came after the marker, with that preamble ahead of the first user
//! message.

use std::borrow::Cow;

use rustcode_core::ChatMessage;
use serde_json::Value;

/// The marker's field holding the preamble the built-in requests start from.
const CONTEXT: &str = "context";

/// `marker` with `context` the built-in harness hands the model instead of
/// the earlier transcript.
pub(crate) fn with_context(mut marker: ChatMessage, context: Option<String>) -> ChatMessage {
    let Some(context) = context else {
        return marker;
    };
    if let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(&marker.content) {
        fields.insert(CONTEXT.to_owned(), Value::String(context));
        marker.content = Value::Object(fields).to_string();
    }
    marker
}

/// The preamble of a switch to the built-in harness, from its marker.
fn context_of(message: &ChatMessage) -> Option<String> {
    if message.role != super::ROLE {
        return None;
    }
    let value = serde_json::from_str::<Value>(&message.content).ok()?;
    if value.get("to").and_then(Value::as_str) != Some(super::super::BUILTIN_ID) {
        return None;
    }
    Some(
        value
            .get(CONTEXT)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

/// `history` as the built-in harness requests it: after the latest switch to
/// it, only the messages since, the first user message carrying the handed
/// over context. Unchanged when the thread never switched to it.
pub(crate) fn request_view(history: &[ChatMessage]) -> Cow<'_, [ChatMessage]> {
    let Some((marker, context)) = history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| context_of(message).map(|context| (index, context)))
    else {
        return Cow::Borrowed(history);
    };
    let mut after = history[marker + 1..].to_vec();
    if !context.trim().is_empty()
        && let Some(first) = after.iter_mut().find(|message| message.role == "user")
    {
        first.content = format!("{context}{}", first.content);
    }
    Cow::Owned(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_with_switch() -> Vec<ChatMessage> {
        let earlier = vec![
            ChatMessage::new("user", "Fix the login bug"),
            ChatMessage::new("assistant", "Found it in auth.rs"),
        ];
        let context = crate::harness::handoff::context_for(
            &earlier,
            "Claude Code",
            crate::harness::BUILTIN_ID,
            Some("thread-1"),
        );
        let marker = with_context(
            crate::harness::handoff::marker_with(
                "claude-code",
                None,
                crate::harness::BUILTIN_ID,
                None,
                None,
            ),
            context,
        );
        let mut history = earlier;
        history.push(marker);
        history
    }

    #[test]
    fn the_first_prompt_after_the_switch_carries_the_context_once() {
        let mut history = history_with_switch();
        history.push(ChatMessage::new("user", "Now add a test"));
        history.push(ChatMessage::new("assistant", "Added"));
        history.push(ChatMessage::new("user", "Thanks"));

        let view = request_view(&history);
        assert_eq!(view.len(), 3);
        assert!(view[0].content.contains("Fix the login bug"));
        assert!(view[0].content.ends_with("Now add a test"));
        assert_eq!(view[2].content, "Thanks");
        let mentions = view
            .iter()
            .filter(|message| message.content.contains("Found it in auth.rs"))
            .count();
        assert_eq!(mentions, 1);
    }

    #[test]
    fn a_thread_that_never_switched_to_rustcode_is_unchanged() {
        let mut history = vec![ChatMessage::new("user", "hi")];
        history.push(crate::harness::handoff::marker_with(
            crate::harness::BUILTIN_ID,
            None,
            "codex",
            None,
            None,
        ));
        history.push(ChatMessage::new("user", "go on"));
        assert!(matches!(request_view(&history), Cow::Borrowed(_)));
    }

    #[test]
    fn the_marker_still_draws_its_divider() {
        let history = history_with_switch();
        let marker = history.last().unwrap();
        assert!(crate::harness::handoff::divider(marker).is_some());
        assert!(context_of(marker).is_some_and(|context| !context.is_empty()));
    }
}
