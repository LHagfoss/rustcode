use crate::app::{ChatMessage, ToolResultRecord};
use crate::tools::{ToolCall, resolve_tool_calls};

pub const MAX_CONTEXT_FRAGMENT_CHARS: usize = 16 * 1024;
pub const MAX_CONTEXT_TAIL_CHARS: usize = 48 * 1024;

/// Provider-independent history representation. Persisted `ChatMessage`
/// values remain backward-compatible, while requests are normalized through
/// explicit variants so tool calls and results cannot silently change roles.
#[derive(Debug, PartialEq)]
pub(crate) enum HistoryEntry<'a> {
    User(&'a str),
    Assistant(&'a str),
    ToolCall(ToolCall),
    ToolResult {
        tool_name: &'a str,
        content: &'a str,
        metadata: Option<&'a ToolResultRecord>,
    },
    System(&'a str),
    CompactionSummary(&'a str),
    Lifecycle(&'a str),
}

pub(crate) fn normalize_history(history: &[ChatMessage]) -> impl Iterator<Item = HistoryEntry<'_>> {
    history.iter().map(normalize_message)
}

fn normalize_message(message: &ChatMessage) -> HistoryEntry<'_> {
    match message.role.as_str() {
        "user" => HistoryEntry::User(&message.content),
        "assistant" => {
            let calls = resolve_tool_calls(message, crate::config::ToolProtocol::Json);
            if calls.len() == 1 {
                HistoryEntry::ToolCall(calls.into_iter().next().expect("one call"))
            } else {
                HistoryEntry::Assistant(&message.content)
            }
        }
        "tool" => {
            let (tool_name, content) = message
                .content
                .split_once(": ")
                .unwrap_or(("tool", message.content.as_str()));
            HistoryEntry::ToolResult {
                tool_name,
                content,
                metadata: message.tool_result.as_ref(),
            }
        }
        "system"
            if message
                .content
                .starts_with(crate::network::compaction::SUMMARY_MARKER) =>
        {
            HistoryEntry::CompactionSummary(&message.content)
        }
        "system" if message.content.starts_with('[') => HistoryEntry::Lifecycle(&message.content),
        "system" => HistoryEntry::System(&message.content),
        _ => HistoryEntry::Lifecycle(&message.content),
    }
}

/// Indices of older exact-duplicate file reads excluded from the request.
///
/// Pure, non-mutating selection (#985): storage keeps every message verbatim
/// and only the rendered request drops the redundant older copy, while the
/// newer identical read is retained verbatim. Reads with different content
/// remain intact, as do errors, truncated reads, and recent raw context.
///
/// `answered`/`announced` id sets in [`to_messages`] are still computed over
/// the full history, so excluding an older duplicate never orphans its
/// announcing call into a synthetic "did not run" error and never disturbs
/// the `tool_call_id` mapping of the retained pairs.
pub(crate) fn redundant_tool_result_indices(
    history: &[ChatMessage],
    keep_recent_count: usize,
) -> std::collections::HashSet<usize> {
    let cutoff = history.len().saturating_sub(keep_recent_count);
    let mut seen = std::collections::HashSet::new();
    let mut redundant = std::collections::HashSet::new();
    for (index, message) in history.iter().enumerate().rev() {
        let Some(key) = duplicate_file_read_key(&message.content) else {
            continue;
        };
        if !seen.insert(key) && index < cutoff {
            redundant.insert(index);
        }
    }
    redundant
}

fn duplicate_file_read_key(content: &str) -> Option<String> {
    let (name, body) = content.split_once(": ")?;
    if !matches!(name, "view_file" | "read_file") {
        return None;
    }
    // A normal view_file result starts with a path/range header. Require that
    // identity before deduplicating: identical contents from two different
    // files, a failed read, a replay notice, or a truncated read must never be
    // collapsed merely because their rendered bodies happen to match.
    let header = body.lines().next()?;
    if !header.starts_with("[File: ")
        || body.contains("[Truncated:")
        || body.starts_with("[Unchanged since")
    {
        return None;
    }
    Some(format!("{name}\0{header}\0{body}"))
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
            content.truncate(content.floor_char_boundary(MAX_CONTEXT_FRAGMENT_CHARS));
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
            rendered.truncate(rendered.floor_char_boundary(MAX_CONTEXT_TAIL_CHARS));
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

    // Older exact-duplicate file reads are excluded from the request while
    // storage keeps them verbatim (#985). The id sets above still cover the
    // full history, so an excluded duplicate never synthesizes a spurious
    // "did not run" error for its announcer.
    let redundant = redundant_tool_result_indices(history, super::compaction::KEEP_RECENT_TURNS);

    for (index, message) in history.iter().enumerate() {
        if redundant.contains(&index) {
            continue;
        }
        if message.conversation_recap {
            continue;
        }
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
        let entry = normalize_message(message);
        messages.push(match entry {
        HistoryEntry::ToolResult { tool_name, content, metadata } => {
            let metadata_line = metadata
                .map(|value| {
                    let mut value = value.clone();
                    value.completeness = value.resolved_completeness();
                    format!(
                        "\nmetadata: {}",
                        serde_json::to_string(&value).unwrap_or_default()
                    )
                })
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
        HistoryEntry::Assistant(content) => {
            let prose = super::text::strip_think_blocks(&content);
            let prose = prose.trim();
            let final_content = if prose.is_empty() {
                "(completed reasoning)".to_string()
            } else {
                prose.to_string()
            };
            serde_json::json!({
                "role": "assistant",
                "content": final_content,
            })
        }
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
            // Native providers receive only this content field for a tool
            // answer. Keep the terminal's presentation text unchanged, but
            // include the durable execution contract in the model payload so
            // completeness cannot be inferred from a collapsed UI row.
            let content = if let Some(metadata) = message.tool_result.as_ref() {
                let mut metadata = metadata.clone();
                metadata.completeness = metadata.resolved_completeness();
                format!(
                    "{content}\n[result_metadata: {}]",
                    serde_json::to_string(&metadata).unwrap_or_default()
                )
            } else {
                content
            };
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
        let entries: Vec<_> = normalize_history(&history).collect();
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
    fn does_not_replay_ui_conversation_recaps_to_the_model() {
        let history = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new("assistant", "short recap").as_conversation_recap(),
            ChatMessage::new("assistant", "I inspected it."),
        ];

        let messages = to_messages(&history, "system");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2]["content"], "I inspected it.");
    }

    #[test]
    fn bounds_context_fragments_and_total_tail() {
        let fragment = ContextFragment::new("large", "x".repeat(MAX_CONTEXT_FRAGMENT_CHARS + 100));
        let rendered = render_context_fragments(&[fragment]);
        assert!(rendered.len() <= MAX_CONTEXT_FRAGMENT_CHARS + 32);
        assert!(rendered.contains("context fragment truncated"));
    }

    #[test]
    fn context_fragment_truncation_preserves_utf8_boundaries() {
        for character in ["é", "界", "🦀"] {
            for split in 1..character.len() {
                let prefix = "x".repeat(MAX_CONTEXT_FRAGMENT_CHARS - split);
                let fragment = ContextFragment::new("unicode", prefix.clone() + character);
                assert_eq!(fragment.render(), prefix + "\n[context fragment truncated]");
            }
        }
    }

    #[test]
    fn context_tail_truncation_preserves_utf8_boundaries() {
        for character in ["é", "界", "🦀"] {
            for split in 1..character.len() {
                let fragments = [
                    ContextFragment::new("first", "a".repeat(MAX_CONTEXT_FRAGMENT_CHARS)),
                    ContextFragment::new("second", "b".repeat(MAX_CONTEXT_FRAGMENT_CHARS)),
                    ContextFragment::new(
                        "third",
                        "c".repeat(MAX_CONTEXT_FRAGMENT_CHARS - 4 - split) + character,
                    ),
                ];
                let rendered = render_context_fragments(&fragments);
                let suffix = "\n[context tail truncated]";
                assert!(rendered.ends_with(suffix));
                assert_eq!(
                    rendered.len(),
                    MAX_CONTEXT_TAIL_CHARS - split + suffix.len()
                );
                assert!(rendered.trim_end_matches(suffix).ends_with('c'));
            }
        }
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
            ..Default::default()
        });
        let entries: Vec<_> = normalize_history(std::slice::from_ref(&message)).collect();
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

    fn structured_read(id: &str, content: &str) -> (ChatMessage, ChatMessage) {
        let assistant = ChatMessage::new("assistant", "reading the file").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: id.to_string(),
                name: "view_file".to_string(),
                arguments: "{}".to_string(),
            },
        ]);
        let result = ChatMessage::new("tool", content).answering(Some(id.to_string()));
        (assistant, result)
    }

    // #985: rendering the request must never mutate storage. Every retained
    // message stays byte-identical across prune passes and request builds.
    #[test]
    fn rendering_and_pruning_leave_stored_bytes_identical() {
        let (first_call, first_read) =
            structured_read("call_old", "view_file: [File: src/lib.rs]\n1: old");
        let (second_call, second_read) =
            structured_read("call_new", "view_file: [File: src/lib.rs]\n1: old");
        let history = vec![
            ChatMessage::new("user", "inspect this"),
            first_call,
            first_read,
            ChatMessage::new("assistant", "<think>scratch</think>done"),
            second_call,
            second_read,
            ChatMessage::new("user", "next"),
        ];
        let before = serde_json::to_string(&history).unwrap();

        crate::network::compaction::prune_duplicate_tool_results(&history, 1);
        crate::network::compaction::prune_historical_tool_outputs(&history, 1);
        crate::network::compaction::prune_historical_reasoning(&history, 1);
        crate::network::compaction::prune_old_tool_outputs(&history, 1);
        let first_render = to_messages(&history, "system");
        let second_render = to_messages(&history, "system");

        assert_eq!(serde_json::to_string(&history).unwrap(), before);
        assert_eq!(first_render, second_render);
    }

    // The older duplicate is excluded from the request, the newer identical
    // read is retained verbatim, and both structured pairs keep their
    // tool_call_id mapping: no result is re-attributed and no synthetic
    // "did not run" error appears for a call that ran.
    #[test]
    fn render_time_dedup_keeps_newest_and_preserves_call_mapping() {
        let (first_call, first_read) =
            structured_read("call_old", "view_file: [File: src/lib.rs]\n1: old");
        let (second_call, second_read) =
            structured_read("call_new", "view_file: [File: src/lib.rs]\n1: old");
        let mut history = vec![
            ChatMessage::new("user", "inspect this"),
            first_call,
            first_read,
        ];
        // Age the first read out of the protected recent window so the
        // render-time rule applies; the newer read stays recent.
        for i in 0..11 {
            history.push(ChatMessage::new("assistant", format!("progress note {i}")));
        }
        history.push(ChatMessage::new("assistant", "re-checking"));
        history.push(second_call);
        history.push(second_read);
        let before = serde_json::to_string(&history).unwrap();

        let messages = to_messages(&history, "system");

        assert_eq!(serde_json::to_string(&history).unwrap(), before);
        let rendered_ids: Vec<&str> = messages
            .iter()
            .filter_map(|message| message.get("tool_call_id").and_then(|id| id.as_str()))
            .collect();
        assert_eq!(rendered_ids, vec!["call_new"]);
        let assistant_ids: Vec<&str> = messages
            .iter()
            .filter_map(|message| {
                message
                    .get("tool_calls")
                    .and_then(|calls| calls.as_array())
                    .and_then(|calls| calls.first())
                    .and_then(|call| call.get("id"))
                    .and_then(|id| id.as_str())
            })
            .collect();
        assert!(assistant_ids.contains(&"call_old"));
        assert!(assistant_ids.contains(&"call_new"));
        let bodies: Vec<&str> = messages
            .iter()
            .filter_map(|message| message.get("content").and_then(|c| c.as_str()))
            .collect();
        assert!(
            !bodies.iter().any(|body| body.contains("did not run")),
            "a duplicate that ran must not be reported as never-run: {bodies:?}"
        );
        assert!(
            bodies.iter().any(|body| body.contains("1: old")),
            "the retained read must survive verbatim: {bodies:?}"
        );
    }

    // Reads with different content, errors, and truncated reads are never
    // treated as duplicates, even outside the recent window.
    #[test]
    fn render_time_dedup_requires_file_identity_and_complete_content() {
        let history = vec![
            ChatMessage::new("tool", "view_file: [File: src/a.rs]\n1: same"),
            ChatMessage::new("tool", "view_file: [File: src/a.rs]\n1: same"),
            ChatMessage::new("tool", "view_file: [File: src/b.rs]\n1: same"),
            ChatMessage::new("tool", "view_file: error: cannot read 'src/c.rs'"),
            ChatMessage::new(
                "tool",
                "view_file: [File: src/d.rs]\n1: same\n[Truncated: lines 2-2 of 2]",
            ),
            ChatMessage::new("user", "keep recent"),
        ];

        let excluded = redundant_tool_result_indices(&history, 1);

        assert_eq!(excluded, std::collections::HashSet::from([0]));
        let messages = to_messages(&history, "system");
        let rendered: String = serde_json::to_string(&messages).unwrap();
        assert!(rendered.contains("src/b.rs"));
        assert!(rendered.contains("cannot read"));
        assert!(rendered.contains("Truncated"));
    }

    // Repeated turns keep a stable prompt prefix: rendering turn two replays
    // turn one's messages byte-identically (system prompt plus history),
    // which is what keeps prefix KV-caches hot across turns.
    #[test]
    fn repeated_turns_replay_a_stable_message_prefix() {
        let turn_one = vec![
            ChatMessage::new("user", "inspect this"),
            ChatMessage::new("assistant", "reading"),
            ChatMessage::new("tool", "view_file: [File: src/lib.rs]\n1: old"),
        ];
        let first_render = to_messages(&turn_one, "system");

        let mut turn_two = turn_one.clone();
        turn_two.push(ChatMessage::new("assistant", "verified"));
        turn_two.push(ChatMessage::new("user", "next step"));
        let before = serde_json::to_string(&turn_two).unwrap();
        crate::network::compaction::prune_duplicate_tool_results(&turn_two, 12);
        crate::network::compaction::prune_historical_tool_outputs(&turn_two, 12);
        crate::network::compaction::prune_historical_reasoning(&turn_two, 12);
        crate::network::compaction::prune_old_tool_outputs(&turn_two, usize::MAX);
        let second_render = to_messages(&turn_two, "system");

        assert_eq!(serde_json::to_string(&turn_two).unwrap(), before);
        assert_eq!(&second_render[..first_render.len()], &first_render[..]);
    }
}
