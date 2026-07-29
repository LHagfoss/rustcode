use crate::app::ChatMessage;
use crate::tools::ToolCall;

pub const MAX_CONTEXT_FRAGMENT_CHARS: usize = 16 * 1024;
pub const MAX_CONTEXT_TAIL_CHARS: usize = 48 * 1024;

/// Provider-independent history representation. Persisted `ChatMessage`
/// values remain backward-compatible, while requests are normalized through
/// explicit variants so tool calls and results cannot silently change roles.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HistoryEntry {
    User(String),
    Assistant(String),
    ToolCall(ToolCall),
    ToolResult { tool_name: String, content: String },
    System(String),
    ContextFragment(String),
    CompactionSummary(String),
    Lifecycle(String),
}

pub(crate) fn normalize_history(history: &[ChatMessage]) -> Vec<HistoryEntry> {
    history
        .iter()
        .map(|message| match message.role.as_str() {
            "user" => HistoryEntry::User(message.content.clone()),
            "assistant" => {
                let calls = crate::tools::parse_tool_calls(
                    &message.content,
                    crate::config::ToolProtocol::Json,
                );
                if calls.len() == 1 {
                    HistoryEntry::ToolCall(calls.into_iter().next().expect("one call"))
                } else {
                    HistoryEntry::Assistant(message.content.clone())
                }
            }
            "tool" => {
                let (tool_name, content) = message
                    .content
                    .split_once(": ")
                    .map(|(name, content)| (name.to_string(), content.to_string()))
                    .unwrap_or_else(|| ("tool".to_string(), message.content.clone()));
                HistoryEntry::ToolResult { tool_name, content }
            }
            "system" if message.content.starts_with(crate::network::compaction::SUMMARY_MARKER) => {
                HistoryEntry::CompactionSummary(message.content.clone())
            }
            "system" if message.content.starts_with('[') => {
                HistoryEntry::Lifecycle(message.content.clone())
            }
            "system" => HistoryEntry::System(message.content.clone()),
            _ => HistoryEntry::Lifecycle(message.content.clone()),
        })
        .collect()
}

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

    messages.extend(normalize_history(history).into_iter().map(|entry| match entry {
        HistoryEntry::ToolResult { tool_name, content } => serde_json::json!({
            "role": "user",
            "content": format!("<tool_result>\n{}: {}\n</tool_result>", tool_name, content),
        }),
        HistoryEntry::User(content) if first_user => {
            first_user = false;
            serde_json::json!({
                "role": "user",
                "content": super::parse_multimodal_content(&content),
            })
        }
        HistoryEntry::User(content) => serde_json::json!({
            "role": "user",
            "content": super::parse_multimodal_content(&content),
        }),
        HistoryEntry::ToolCall(call) => serde_json::json!({
            "role": "assistant",
            "content": format!("```tool\n{}\n```", serde_json::json!({"name": call.name, "arguments": call.arguments})),
        }),
        HistoryEntry::Assistant(content) => serde_json::json!({
            "role": "assistant",
            "content": content,
        }),
        HistoryEntry::System(content) | HistoryEntry::ContextFragment(content) |
        HistoryEntry::CompactionSummary(content) | HistoryEntry::Lifecycle(content) => serde_json::json!({
            "role": "system",
            "content": content,
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
    fn normalizes_tool_calls_and_results_into_typed_entries() {
        let history = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new(
                "assistant",
                "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"TODO\"}}\n```",
            ),
            ChatMessage::new("tool", "grep: found a match"),
        ];
        let entries = normalize_history(&history);
        assert!(matches!(entries[1], HistoryEntry::ToolCall(_)));
        assert!(matches!(entries[2], HistoryEntry::ToolResult { .. }));
        let messages = to_messages(&history, "system");
        assert_eq!(messages[3]["role"], "user");
        assert!(messages[3]["content"].as_str().unwrap().contains("grep:"));
    }

    #[test]
    fn bounds_context_fragments_and_total_tail() {
        let fragment = ContextFragment::new("large", "x".repeat(MAX_CONTEXT_FRAGMENT_CHARS + 100));
        let rendered = render_context_fragments(&[fragment]);
        assert!(rendered.len() <= MAX_CONTEXT_FRAGMENT_CHARS + 32);
        assert!(rendered.contains("context fragment truncated"));
    }
}
