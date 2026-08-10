use crate::app::{ChatMessage, ToolResultRecord};
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
    ToolResult {
        tool_name: String,
        content: String,
        metadata: Option<ToolResultRecord>,
    },
    System(String),
    CompactionSummary(String),
    Lifecycle(String),
}

pub(crate) fn normalize_history(history: &[ChatMessage]) -> Vec<HistoryEntry> {
    history
        .iter()
        .map(|message| match message.role.as_str() {
            "user" => HistoryEntry::User(message.content.clone()),
            "assistant" => {
                let calls = message.resolved_tool_calls(crate::config::ToolProtocol::Json);
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
                HistoryEntry::ToolResult {
                    tool_name,
                    content,
                    metadata: message.tool_result.clone(),
                }
            }
            "system"
                if message
                    .content
                    .starts_with(crate::network::compaction::SUMMARY_MARKER) =>
            {
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

    // A message the provider gave call ids for is replayed as the structured
    // call it actually was, and its result as the answer to that call id.
    // Rendering those back as prose would teach the model that tool calls are
    // text it writes — which is what lets a model narrate results for calls that
    // never ran. Messages without ids keep the text form.
    let entries = normalize_history(history);
    debug_assert_eq!(entries.len(), history.len());

    // Ids that some later message answers. A turn can end between announcing a
    // call and running it — the user interrupts, the provider drops the stream —
    // and an unanswered id makes the whole request invalid. Rather than trusting
    // every path that records a call to also record its outcome, the gap is
    // closed here, where the request is actually built.
    let answered: std::collections::HashSet<&str> = history
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    // Compaction can drop the assistant message that announced a call while
    // keeping its result. An answer to a call the request never mentions is just
    // as invalid as an unanswered call, so those fall back to the text form.
    let announced: std::collections::HashSet<&str> = history
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| call.id.as_str())
        .collect();

    for (message, entry) in history.iter().zip(entries) {
        let orphan_result = message
            .tool_call_id
            .as_deref()
            .is_some_and(|id| !announced.contains(id));
        if let Some(structured) = (!orphan_result)
            .then(|| structured_message(message))
            .flatten()
        {
            messages.push(structured);
            // Speak for the calls nothing else answered, in the order they were
            // made, so the model sees which of them never ran.
            for call in message
                .tool_calls
                .iter()
                .filter(|call| !answered.contains(call.id.as_str()))
            {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": "error: this call did not run — the turn ended before it could",
                }));
            }
            continue;
        }
        messages.push(match entry {
        HistoryEntry::ToolResult { tool_name, content, metadata } => {
            let metadata_line = metadata
                .as_ref()
                .map(|value| format!("\nmetadata: {}", serde_json::to_string(value).unwrap_or_default()))
                .unwrap_or_default();
            serde_json::json!({
                "role": "user",
                "content": format!("<tool_result>\n{}: {}{}\n</tool_result>", tool_name, content, metadata_line),
            })
        },
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
        HistoryEntry::System(content) |
        HistoryEntry::CompactionSummary(content) | HistoryEntry::Lifecycle(content) => serde_json::json!({
            "role": "system",
            "content": content,
        }),
        });
    }

    messages
}

/// Provider message for one history entry when the transcript carries call ids,
/// or `None` when this history has none and the text rendering applies.
fn structured_message(message: &ChatMessage) -> Option<serde_json::Value> {
    match message.role.as_str() {
        "assistant" if !message.tool_calls.is_empty() => {
            let calls: Vec<serde_json::Value> = message
                .tool_calls
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments,
                            "thought_signature": "context",
                        },
                    })
                })
                .collect();
            // Keep whatever the model said alongside the call. The call itself is
            // carried structurally, so its text form is redundant — but the
            // reasoning around it is the only record of why this step was taken,
            // and replaying a turn as a bare call leaves the model re-deciding
            // the same step from scratch every round.
            let prose = super::text::strip_tool_call_syntax(&message.content);
            let prose = prose.trim();
            let content = if prose.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(prose.to_string())
            };
            Some(serde_json::json!({
                "role": "assistant",
                "content": content,
                "tool_calls": calls,
            }))
        }
        "tool" => {
            let call_id = message.tool_call_id.as_ref()?;
            let content = message
                .content
                .split_once(": ")
                .map(|(_, rest)| rest.to_string())
                .unwrap_or_else(|| message.content.clone());
            Some(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: replaying a structured call as ```tool prose taught the model
    // that tool calls are text it writes, which is what let it emit a whole
    // session of calls with narrated results for calls that never ran.
    #[test]
    fn calls_with_ids_replay_as_structured_messages() {
        let history = vec![
            ChatMessage::new("user", "find the config"),
            ChatMessage::new("assistant", "```tool\n{\"name\": \"grep\"}\n```").with_tool_calls(
                vec![crate::app::ToolCallRef {
                    id: "call_abc".to_string(),
                    name: "grep".to_string(),
                    arguments: "{\"pattern\":\"config\"}".to_string(),
                }],
            ),
            ChatMessage::new("tool", "grep: src/config.rs:1")
                .answering(Some("call_abc".to_string())),
        ];

        let msgs = to_messages(&history, "sys");

        assert_eq!(msgs[1]["role"], "user");
        // The assistant message carries the call itself, not prose about it.
        assert_eq!(msgs[2]["role"], "assistant");
        assert!(msgs[2]["content"].is_null());
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_abc");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "grep");
        assert_eq!(
            msgs[2]["tool_calls"][0]["function"]["arguments"],
            "{\"pattern\":\"config\"}"
        );
        // The result names the call it answers.
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_abc");
        assert_eq!(msgs[3]["content"], "src/config.rs:1");
    }

    // A turn can end between announcing a call and running it (user interrupt,
    // dropped stream). An unanswered id makes the whole request invalid.
    // Regression: session 1785600769226. Every assistant turn was replayed as a
    // bare tool call with null content, so the model lost its own reasoning
    // between rounds and re-derived the same step — it issued the identical
    // one-line read 25 times before the loop detector killed the turn.
    #[test]
    fn the_models_own_words_survive_alongside_its_calls() {
        let history = vec![
            ChatMessage::new("user", "add the comment"),
            ChatMessage::new(
                "assistant",
                "The comment is already on line 1, so nothing needs adding.\n\n```tool\n{\"name\": \"view_file\"}\n```",
            )
            .with_tool_calls(vec![crate::app::ToolCallRef {
                id: "call_1".to_string(),
                name: "view_file".to_string(),
                arguments: "{}".to_string(),
            }]),
            ChatMessage::new("tool", "view_file: 1: // scratch").answering(Some("call_1".to_string())),
        ];

        let msgs = to_messages(&history, "sys");

        let content = msgs[2]["content"].as_str().expect("prose is kept");
        assert!(content.contains("already on line 1"), "got: {content}");
        // The call travels structurally, so its text form is not repeated.
        assert!(!content.contains("```tool"), "got: {content}");
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn calls_the_turn_never_ran_are_answered_in_the_request() {
        let history = vec![
            ChatMessage::new("user", "go"),
            ChatMessage::new("assistant", "```tool\n{}\n```").with_tool_calls(vec![
                crate::app::ToolCallRef {
                    id: "call_dead".to_string(),
                    name: "run_command".to_string(),
                    arguments: "{}".to_string(),
                },
            ]),
            ChatMessage::new("user", "what happened"),
        ];

        let msgs = to_messages(&history, "sys");

        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_dead");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_dead");
        assert!(msgs[3]["content"].as_str().unwrap().contains("did not run"));
        assert_eq!(msgs[4]["content"], "what happened");
    }

    // Compaction can drop the announcing message while keeping the result.
    #[test]
    fn results_for_forgotten_calls_fall_back_to_text() {
        let history = vec![
            ChatMessage::new("user", "go"),
            ChatMessage::new("tool", "grep: found it").answering(Some("call_gone".to_string())),
        ];

        let msgs = to_messages(&history, "sys");

        assert_eq!(msgs[2]["role"], "user");
        assert!(msgs[2]["tool_call_id"].is_null());
        assert!(
            msgs[2]["content"]
                .as_str()
                .unwrap()
                .contains("<tool_result>")
        );
    }

    #[test]
    fn history_without_ids_keeps_the_text_rendering() {
        let history = vec![
            ChatMessage::new("user", "hi"),
            ChatMessage::new(
                "assistant",
                "```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```",
            ),
            ChatMessage::new("tool", "grep: no matches"),
        ];

        let msgs = to_messages(&history, "sys");

        assert_eq!(msgs.len(), history.len() + 1);
        assert_eq!(msgs[2]["role"], "assistant");
        assert!(msgs[2]["tool_calls"].is_null());
        // Text-protocol results stay user-context messages.
        assert_eq!(msgs[3]["role"], "user");
        assert!(
            msgs[3]["content"]
                .as_str()
                .unwrap()
                .contains("<tool_result>")
        );
    }

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
        assert!(matches!(
            entries[2],
            HistoryEntry::ToolResult { metadata: None, .. }
        ));
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

    #[test]
    fn preserves_structured_tool_metadata_in_provider_context() {
        let message = ChatMessage::new("tool", "grep: found").with_tool_result(ToolResultRecord {
            tool_name: "grep".to_string(),
            arguments_hash: "abc".to_string(),
            success: true,
            exit_code: Some(0),
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
        });
        let entries = normalize_history(std::slice::from_ref(&message));
        assert!(matches!(
            entries[0],
            HistoryEntry::ToolResult {
                metadata: Some(_),
                ..
            }
        ));
        let messages = to_messages(&[message], "system");
        assert!(
            messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("metadata:")
        );
    }
}
