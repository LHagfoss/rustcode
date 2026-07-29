use crate::app::ChatMessage;

pub const MAX_CONTEXT_FRAGMENT_CHARS: usize = 16 * 1024;
pub const MAX_CONTEXT_TAIL_CHARS: usize = 48 * 1024;

/// A bounded, named piece of turn-varying context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextFragment {
    pub name: String,
    pub content: String,
}

impl ContextFragment {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
        }
    }

    fn render(&self) -> String {
        let mut content = self.content.clone();
        if content.len() > MAX_CONTEXT_FRAGMENT_CHARS {
            content.truncate(MAX_CONTEXT_FRAGMENT_CHARS);
            content.push_str("\n[context fragment truncated]");
        }
        content
    }
}

pub(crate) fn render_context_fragments(fragments: &[ContextFragment]) -> String {
    let mut rendered = String::new();
    for fragment in fragments {
        let content = fragment.render();
        if content.is_empty() {
            continue;
        }
        if !rendered.is_empty() {
            rendered.push_str("\n\n");
        }
        rendered.push_str(&content);
        if rendered.len() >= MAX_CONTEXT_TAIL_CHARS {
            rendered.truncate(MAX_CONTEXT_TAIL_CHARS);
            rendered.push_str("\n[context tail truncated]");
            break;
        }
    }
    rendered
}

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

    #[test]
    fn bounds_context_fragments_and_total_tail() {
        let fragment = ContextFragment::new("large", "x".repeat(MAX_CONTEXT_FRAGMENT_CHARS + 100));
        let rendered = render_context_fragments(&[fragment]);
        assert!(rendered.len() <= MAX_CONTEXT_FRAGMENT_CHARS + 32);
        assert!(rendered.contains("context fragment truncated"));
    }
}
