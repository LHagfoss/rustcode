use crate::app::ChatMessage;

/// Convert normalized conversation history into provider message values.
///
/// Tool outputs are represented as user-context messages and user messages
/// retain multimodal content. Keeping this conversion in one place prevents
/// the raw CLI and TUI from drifting as the context manager evolves.
pub(crate) fn to_messages(
    history: &[ChatMessage],
    system_prompt: impl Into<String>,
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt.into(),
    })];
    let mut first_user = true;

    messages.extend(history.iter().map(|message| match message.role.as_str() {
        "tool" => serde_json::json!({
            "role": "user",
            "content": format!("<tool_result>\n{}\n</tool_result>", message.content),
        }),
        "user" if first_user => {
            first_user = false;
            serde_json::json!({
                "role": "user",
                "content": super::parse_multimodal_content(&message.content),
            })
        }
        "user" => serde_json::json!({
            "role": "user",
            "content": super::parse_multimodal_content(&message.content),
        }),
        _ => serde_json::json!({
            "role": message.role,
            "content": message.content,
        }),
    }));

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_system_user_and_tool_message_contracts() {
        let history = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new("assistant", "I will check."),
            ChatMessage::new("tool", "grep: found a match"),
        ];

        let messages = to_messages(&history, "system prompt");

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(
            messages[3]["content"],
            "<tool_result>\ngrep: found a match\n</tool_result>"
        );
    }
}
