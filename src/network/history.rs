use crate::app::{ChatMessage, ToolResultRecord};
use crate::tools::{ToolCall, resolve_tool_calls};

pub const MAX_CONTEXT_FRAGMENT_CHARS: usize = 16 * 1024;
pub const MAX_CONTEXT_TAIL_CHARS: usize = 48 * 1024;
const MAX_TOOL_CONTINUITY_CHARS: usize = 512;

/// Immutable instruction inputs assembled fresh for every provider request.
/// They are deliberately not persisted in conversation history: resume,
/// retry, and compaction all reconstruct them from current configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestInstructions<'a> {
    pub(crate) base: &'a str,
    pub(crate) developer: Option<&'a str>,
}

impl<'a> RequestInstructions<'a> {
    pub(crate) const fn new(base: &'a str, developer: Option<&'a str>) -> Self {
        Self { base, developer }
    }
}

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
            // Native tool calls are owned by the structured renderer, which
            // supports batches. Never resurrect them as a single text fence
            // here: a fully-deduped assistant message must fall back to plain
            // prose, and multi-call messages must not be truncated to one.
            if !message.tool_calls.is_empty() {
                HistoryEntry::Assistant(&message.content)
            } else {
                let calls = resolve_tool_calls(message, crate::config::ToolProtocol::Json);
                if calls.len() == 1 {
                    HistoryEntry::ToolCall(calls.into_iter().next().expect("one call"))
                } else {
                    HistoryEntry::Assistant(&message.content)
                }
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

/// Persisted tool records are intentionally rich for replay, UI, and
/// diagnostics. The model only needs the small execution contract that
/// explains whether the result is usable and how to recover from it. Keeping
/// this projection at the provider boundary avoids repeating tool names and
/// other durable bookkeeping in every subsequent request. The compact
/// arguments hash remains because replay and duplicate-call recovery use it as
/// the stable identity of the tool invocation.
fn compact_tool_result_metadata(metadata: &ToolResultRecord) -> String {
    let mut compact = serde_json::Map::new();
    compact.insert("success".into(), serde_json::json!(metadata.success));
    compact.insert(
        "completeness".into(),
        serde_json::json!(metadata.resolved_completeness().as_str()),
    );
    if !metadata.arguments_hash.is_empty() {
        compact.insert(
            "arguments_hash".into(),
            serde_json::json!(metadata.arguments_hash),
        );
    }
    if metadata.pending {
        compact.insert("pending".into(), serde_json::json!(true));
    }
    if metadata.payload_truncated {
        compact.insert("payload_truncated".into(), serde_json::json!(true));
    }
    if let Some(exit_code) = metadata.exit_code {
        compact.insert("exit_code".into(), serde_json::json!(exit_code));
    }
    if !metadata.changed_paths.is_empty() {
        compact.insert(
            "changed_paths".into(),
            serde_json::json!(metadata.changed_paths),
        );
    }
    if let Some(error_kind) = metadata.error_kind.as_deref() {
        compact.insert("error_kind".into(), serde_json::json!(error_kind));
    }
    if metadata.retryable {
        compact.insert("retryable".into(), serde_json::json!(true));
    }
    if metadata.replayed {
        compact.insert("replayed".into(), serde_json::json!(true));
    }
    if let Some(artifact) = metadata.full_output_artifact.as_deref() {
        compact.insert("full_output_artifact".into(), serde_json::json!(artifact));
    }
    if let Some(inspection) = metadata.inspection.as_ref() {
        compact.insert(
            "inspection".into(),
            serde_json::to_value(inspection).unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::to_string(&compact).unwrap_or_else(|_| "{}".to_string())
}

/// Indices of older exact-duplicate file reads excluded from the request.
///
/// Pure, non-mutating selection (#985): storage keeps every message verbatim
/// and only the rendered request drops the redundant older copy, while the
/// newer identical read is retained verbatim. Reads with different content
/// remain intact, as do errors, truncated reads, and recent raw context.
///
/// A structured duplicate is removed as a complete call/result exchange from
/// the provider projection. The persisted messages remain lossless, and an
/// answer from another scope must not satisfy a reused id in the active
/// request.
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
        || body.contains("[Unchanged read replay:")
        || body.starts_with("[Unchanged since")
    {
        return None;
    }
    Some(format!("{name}\0{header}\0{body}"))
}

fn announcing_call_index(
    history: &[ChatMessage],
    result_index: usize,
    call_id: &str,
) -> Option<usize> {
    history[..result_index]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.tool_calls.iter().any(|call| call.id == call_id))
        .map(|(index, _)| index)
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
    let system_prompt = system_prompt.into();
    to_messages_with_instructions(history, RequestInstructions::new(&system_prompt, None))
}

pub(crate) fn to_messages_with_instructions(
    history: &[ChatMessage],
    instructions: RequestInstructions<'_>,
) -> Vec<serde_json::Value> {
    to_messages_with_scope(history, instructions, HistoryRenderScope::Full)
}

/// Render the request-local history projection used for an active model turn.
/// Persisted history remains complete, but old tool mechanics and lifecycle
/// notices are not useful conversation context forever. Keep durable dialogue
/// from older turns, the complete previous turn for continuity, and the
/// complete active turn for progressive reads, edits, and recovery.
pub(crate) fn to_messages_for_request(
    history: &[ChatMessage],
    instructions: RequestInstructions<'_>,
) -> Vec<serde_json::Value> {
    to_messages_with_scope(history, instructions, HistoryRenderScope::RecentTurns)
}

#[derive(Clone, Copy)]
enum HistoryRenderScope {
    Full,
    RecentTurns,
}

fn to_messages_with_scope(
    history: &[ChatMessage],
    instructions: RequestInstructions<'_>,
    scope: HistoryRenderScope,
) -> Vec<serde_json::Value> {
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": instructions.base,
    })];
    if let Some(developer) = instructions.developer.filter(|value| !value.is_empty()) {
        messages.push(serde_json::json!({
            "role": "developer",
            "content": developer,
        }));
    }
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
    let redundant = redundant_tool_result_indices(history, super::compaction::KEEP_RECENT_TURNS);
    let turn_starts = request_turn_starts(history);
    let included: std::collections::HashSet<usize> = history
        .iter()
        .enumerate()
        .filter(|(index, message)| {
            should_include_request_message(*index, message, scope, turn_starts)
                && !redundant.contains(index)
        })
        .map(|(index, _)| index)
        .collect();
    // Render-time deduplication must remove the announcing call together with
    // its result. Otherwise the provider receives an assistant tool_calls
    // announcement with no matching role=tool message. Keep this keyed by the
    // announcement index as well as the id: an id can be reused by a broken or
    // recovered transcript, and one old duplicate must not hide a newer call.
    let redundant_calls: std::collections::HashSet<(usize, String)> = redundant
        .iter()
        .filter_map(|result_index| {
            let call_id = history[*result_index].tool_call_id.as_ref()?;
            let announcement = announcing_call_index(history, *result_index, call_id)?;
            Some((announcement, call_id.clone()))
        })
        .collect();
    let rendered_calls: std::collections::HashMap<usize, Vec<crate::app::ToolCallRef>> = included
        .iter()
        .filter_map(|index| {
            let message = &history[*index];
            if message.role != "assistant" || message.tool_calls.is_empty() {
                return None;
            }
            let calls = message
                .tool_calls
                .iter()
                .filter(|call| !redundant_calls.contains(&(*index, call.id.clone())))
                .cloned()
                .collect::<Vec<_>>();
            Some((*index, calls))
        })
        .collect();
    let answered: std::collections::HashSet<(usize, String)> = history
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let call_id = message.tool_call_id.as_deref()?;
            let announcement = announcing_call_index(history, index, call_id)?;
            (included.contains(&announcement)
                && included.contains(&index)
                && rendered_calls
                    .get(&announcement)
                    .is_some_and(|calls| calls.iter().any(|call| call.id == call_id)))
            .then_some((announcement, call_id.to_owned()))
        })
        .collect();
    // Compaction can drop the assistant message that announced a call while
    // keeping its result. An answer to a call the request never mentions is just
    // as invalid as an unanswered call, so those fall back to the text form.
    // Keyed by (announcement, id): ids can be reused by recovery, and a result
    // for a dropped announcement must not satisfy a newer call with the same id.
    let announced: std::collections::HashSet<(usize, String)> = rendered_calls
        .iter()
        .flat_map(|(index, calls)| calls.iter().map(|call| (*index, call.id.clone())))
        .collect();

    for (index, message) in history.iter().enumerate() {
        if !included.contains(&index) {
            continue;
        }
        if message.conversation_recap {
            continue;
        }
        let orphan_result = message.tool_call_id.as_deref().is_some_and(|id| {
            match announcing_call_index(history, index, id) {
                Some(announcement) => !announced.contains(&(announcement, id.to_owned())),
                None => true,
            }
        });
        let projected_message = rendered_calls.get(&index).map(|calls| {
            let mut projected = message.clone();
            projected.tool_calls = calls.clone();
            projected
        });
        let message_for_render = projected_message.as_ref().unwrap_or(message);
        if let Some(structured) = (!orphan_result)
            .then(|| structured_message(message_for_render))
            .flatten()
        {
            messages.push(structured);
            // Speak for the calls nothing else answered, in the order they were
            // made, so the model sees which of them never ran.
            for call in rendered_calls
                .get(&index)
                .into_iter()
                .flat_map(|calls| calls.iter())
                .filter(|call| !answered.contains(&(index, call.id.clone())))
            {
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": "error: this call did not run — the turn ended before it could",
                }));
            }
            continue;
        }
        // Normalize the projected message so render-time dedup applies:
        // a fully-deduped assistant message falls back to prose instead of
        // resurrecting its removed calls as text.
        let entry = normalize_message(message_for_render);
        messages.push(match entry {
        HistoryEntry::ToolResult { tool_name, content, metadata } => {
            let metadata_line = metadata
                .map(|value| format!("\nmetadata: {}", compact_tool_result_metadata(value)))
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
        HistoryEntry::System(content) => runtime_notice("historical_system", content),
        HistoryEntry::CompactionSummary(content) => runtime_notice("compaction", content),
        HistoryEntry::Lifecycle(content) => runtime_notice("lifecycle", content),
        });
    }

    messages
}

fn request_turn_starts(history: &[ChatMessage]) -> Option<(usize, Option<usize>)> {
    let mut users = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == "user")
        .map(|(index, _)| index);
    let active = users.next_back()?;
    let previous = users.next_back();
    Some((active, previous))
}

fn is_compaction_summary(message: &ChatMessage) -> bool {
    message.role == "system"
        && message
            .content
            .starts_with(crate::network::compaction::SUMMARY_MARKER)
}

fn is_lifecycle_notice(message: &ChatMessage) -> bool {
    message.role == "system" && !is_compaction_summary(message)
}

fn is_durable_older_message(message: &ChatMessage) -> bool {
    if message.conversation_recap || is_lifecycle_notice(message) {
        return is_compaction_summary(message);
    }
    match message.role.as_str() {
        "tool" => false,
        "assistant" => message.tool_calls.is_empty(),
        _ => true,
    }
}

fn should_include_request_message(
    index: usize,
    message: &ChatMessage,
    scope: HistoryRenderScope,
    turn_starts: Option<(usize, Option<usize>)>,
) -> bool {
    if message.conversation_recap {
        return false;
    }
    let HistoryRenderScope::RecentTurns = scope else {
        return true;
    };
    let Some((active_start, previous_start)) = turn_starts else {
        return true;
    };
    if index >= active_start {
        return true;
    }
    if previous_start.is_some_and(|start| index >= start) {
        return !is_lifecycle_notice(message);
    }
    is_durable_older_message(message)
}

fn runtime_notice(kind: &str, content: &str) -> serde_json::Value {
    let mut bounded = content.to_string();
    if bounded.len() > MAX_CONTEXT_FRAGMENT_CHARS {
        bounded.truncate(bounded.floor_char_boundary(MAX_CONTEXT_FRAGMENT_CHARS));
        bounded.push_str("\n[runtime notice truncated]");
    }
    serde_json::json!({
        "role": "user",
        "content": format!(
            "<rustcode_runtime_notice provenance=\"{kind}\">\n{bounded}\n</rustcode_runtime_notice>"
        ),
    })
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
                serde_json::Value::String(tool_continuity_note(message))
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
                format!(
                    "{content}\n[result_metadata: {}]",
                    compact_tool_result_metadata(metadata)
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

/// Keep a compact action breadcrumb when an assistant tool turn contained only
/// private reasoning. The structured call remains the source of truth for its
/// arguments; this note only makes the attempted action and resume intent
/// explicit without replaying the reasoning itself.
fn tool_continuity_note(message: &ChatMessage) -> String {
    let mut names = String::new();
    for call in &message.tool_calls {
        if !names.is_empty() {
            names.push_str(", ");
        }
        names.push('`');
        names.push_str(&call.name);
        names.push('`');
    }
    let mut note = format!(
        "[Continuity: attempted {names}; use the tool result to continue the pending task without repeating this call unless needed.]"
    );
    if note.len() > MAX_TOOL_CONTINUITY_CHARS {
        const SUFFIX: &str = "...; continue from the tool result.]";
        let keep = MAX_TOOL_CONTINUITY_CHARS.saturating_sub(SUFFIX.len());
        note.truncate(note.floor_char_boundary(keep));
        note.push_str(SUFFIX);
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToolProtocol;

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
        assert!(
            msgs[2]["content"]
                .as_str()
                .unwrap()
                .contains("attempted `grep`")
        );
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
    fn think_only_tool_turns_get_safe_cross_round_continuity() {
        let history = vec![
            ChatMessage::new("user", "fix the parser"),
            ChatMessage::new(
                "assistant",
                "<think>I found the bug. After this read I must edit the parser and test it.</think>",
            )
            .with_tool_calls(vec![crate::app::ToolCallRef {
                id: "call_read".to_string(),
                name: "view_file".to_string(),
                arguments: r#"{"path":"src/parser.rs"}"#.to_string(),
            }]),
            ChatMessage::new("tool", "view_file: parser source")
                .answering(Some("call_read".to_string())),
        ];

        let msgs = to_messages(&history, "sys");
        let content = msgs[2]["content"].as_str().expect("continuity note");
        assert!(content.contains("attempted `view_file`"), "got: {content}");
        assert!(
            content.contains("continue the pending task"),
            "got: {content}"
        );
        assert!(!content.contains("I found the bug"), "got: {content}");
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_read");
        assert_eq!(msgs[3]["tool_call_id"], "call_read");
    }

    #[test]
    fn think_only_tool_continuity_is_bounded_on_utf8_boundaries() {
        let history = vec![
            ChatMessage::new("user", "continue"),
            ChatMessage::new("assistant", "<think>private plan</think>").with_tool_calls(vec![
                crate::app::ToolCallRef {
                    id: "call_large".to_string(),
                    name: format!("inspect_{}", "🦀".repeat(MAX_TOOL_CONTINUITY_CHARS)),
                    arguments: "{}".to_string(),
                },
            ]),
            ChatMessage::new("tool", "inspect: done").answering(Some("call_large".to_string())),
        ];

        let msgs = to_messages(&history, "sys");
        let content = msgs[2]["content"].as_str().expect("continuity note");
        assert!(content.len() <= MAX_TOOL_CONTINUITY_CHARS);
        assert!(content.ends_with("continue from the tool result.]"));
        assert_eq!(msgs[2]["tool_calls"][0]["id"], "call_large");
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
    fn immutable_instructions_are_typed_once_and_runtime_notices_keep_order() {
        let history = vec![
            ChatMessage::new("user", "inspect"),
            ChatMessage::new("system", "[Loop warning: use existing evidence]"),
            ChatMessage::new(
                "tool",
                "run_command: verified\n[Output truncated: 4 bytes total]",
            ),
            ChatMessage::new(
                "system",
                format!(
                    "{}\ncompleted earlier work",
                    crate::network::compaction::SUMMARY_MARKER
                ),
            ),
        ];

        let messages = to_messages_with_instructions(
            &history,
            RequestInstructions::new("base", Some("developer")),
        );

        assert_eq!(
            messages[0],
            serde_json::json!({"role": "system", "content": "base"})
        );
        assert_eq!(
            messages[1],
            serde_json::json!({"role": "developer", "content": "developer"})
        );
        assert_eq!(
            messages.iter().filter(|m| m["content"] == "base").count(),
            1
        );
        assert_eq!(
            messages
                .iter()
                .filter(|m| m["content"] == "developer")
                .count(),
            1
        );
        assert_eq!(messages[2]["content"], "inspect");
        assert_eq!(messages[3]["role"], "user");
        assert!(
            messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("Loop warning")
        );
        assert!(
            messages[4]["content"]
                .as_str()
                .unwrap()
                .contains("verified")
        );
        assert!(
            messages[4]["content"]
                .as_str()
                .unwrap()
                .contains("Output truncated")
        );
        assert_eq!(messages[5]["role"], "user");
        assert!(
            messages[5]["content"]
                .as_str()
                .unwrap()
                .contains("completed earlier work")
        );
    }

    #[test]
    fn retry_and_resume_reconstruct_the_same_single_instruction_prefix() {
        let stored = vec![
            ChatMessage::new("system", "[Session History Summary]\nprior work"),
            ChatMessage::new("user", "continue"),
            ChatMessage::new("system", "[Recovery: answer from the evidence]"),
        ];
        let serialized = serde_json::to_string(&stored).unwrap();
        let resumed: Vec<ChatMessage> = serde_json::from_str(&serialized).unwrap();
        let instructions = RequestInstructions::new("base", Some("project rules"));

        let first = to_messages_with_instructions(&stored, instructions);
        let retry = to_messages_with_instructions(&stored, instructions);
        let resume = to_messages_with_instructions(&resumed, instructions);

        assert_eq!(first, retry);
        assert_eq!(first, resume);
        assert_eq!(first.iter().filter(|m| m["role"] == "system").count(), 1);
        assert_eq!(first.iter().filter(|m| m["role"] == "developer").count(), 1);
        assert_eq!(
            first
                .iter()
                .filter(|m| m["content"]
                    .as_str()
                    .is_some_and(|c| c.contains("Recovery:")))
                .count(),
            1
        );
    }

    #[test]
    fn immutable_instruction_inputs_render_exactly_once() {
        let history = vec![
            ChatMessage::new("system", "[Loop warning: inspect a different range]"),
            ChatMessage::new("user", "continue"),
        ];
        let messages = to_messages_with_instructions(
            &history,
            RequestInstructions::new("BASE-RULE", Some("DEVELOPER-RULE")),
        );
        let rendered = serde_json::to_string(&messages).unwrap();

        assert_eq!(rendered.matches("BASE-RULE").count(), 1);
        assert_eq!(rendered.matches("DEVELOPER-RULE").count(), 1);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "developer");
        assert_eq!(messages[1]["content"], "DEVELOPER-RULE");
        assert_eq!(messages[2]["role"], "user");
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .contains("provenance=\"lifecycle\"")
        );
    }

    #[test]
    fn request_projection_keeps_recent_evidence_and_drops_stale_mechanics() {
        let old_call = ChatMessage::new("assistant", "old tool call").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "old-call".into(),
                name: "run_command".into(),
                arguments: "{\"command\":\"old\"}".into(),
            },
        ]);
        let old_result = ChatMessage::new("tool", "run_command: OLD RAW OUTPUT")
            .answering(Some("old-call".into()));
        let previous_call =
            ChatMessage::new("assistant", "previous tool call").with_tool_calls(vec![
                crate::app::ToolCallRef {
                    id: "previous-call".into(),
                    name: "view_file".into(),
                    arguments: "{\"path\":\"src/lib.rs\"}".into(),
                },
            ]);
        let previous_result = ChatMessage::new("tool", "view_file: PREVIOUS INSPECTION EVIDENCE")
            .answering(Some("previous-call".into()));
        let history = vec![
            ChatMessage::new("user", "old task"),
            old_call,
            old_result,
            ChatMessage::new("system", "[old recovery: use another step]"),
            ChatMessage::new("assistant", "old task finished"),
            ChatMessage::new("user", "previous task"),
            previous_call,
            previous_result,
            ChatMessage::new("system", "[previous recovery: retry]"),
            ChatMessage::new("assistant", "previous task inspected the file"),
            ChatMessage::new("user", "current task"),
            ChatMessage::new("system", "[current recovery: inspect the actual error]"),
        ];
        let stored = serde_json::to_string(&history).expect("serialize history");

        let messages = to_messages_for_request(
            &history,
            RequestInstructions::new("base", Some("developer")),
        );
        let rendered = serde_json::to_string(&messages).expect("render request");

        assert!(rendered.contains("old task finished"));
        assert!(!rendered.contains("OLD RAW OUTPUT"));
        assert!(!rendered.contains("old-call"));
        assert!(rendered.contains("PREVIOUS INSPECTION EVIDENCE"));
        assert!(rendered.contains("previous-call"));
        assert!(!rendered.contains("previous recovery"));
        assert!(rendered.contains("current recovery"));
        assert_eq!(serde_json::to_string(&history).unwrap(), stored);
    }

    #[test]
    fn request_projection_does_not_use_an_out_of_scope_result_to_answer_a_call() {
        let call_id = "reused-view-call";
        let current_call =
            ChatMessage::new("assistant", "read the current file").with_tool_calls(vec![
                crate::app::ToolCallRef {
                    id: call_id.to_string(),
                    name: "view_file".to_string(),
                    arguments: r#"{"path":"src/current.rs"}"#.to_string(),
                },
            ]);
        let history = vec![
            ChatMessage::new("user", "old task"),
            ChatMessage::new("tool", "view_file: old result").answering(Some(call_id.into())),
            ChatMessage::new("assistant", "old task complete"),
            ChatMessage::new("user", "previous task"),
            ChatMessage::new("assistant", "previous task complete"),
            ChatMessage::new("user", "current task"),
            current_call,
        ];

        let messages = to_messages_for_request(&history, RequestInstructions::new("base", None));
        let assistant_index = messages
            .iter()
            .position(|message| message.get("tool_calls").is_some())
            .expect("current structured call is retained");
        let result = messages
            .get(assistant_index + 1)
            .expect("retained call has a following result");

        assert_eq!(result["role"], "tool");
        assert_eq!(result["tool_call_id"], call_id);
        assert!(
            result["content"]
                .as_str()
                .is_some_and(|content| content.contains("did not run"))
        );
        assert!(!messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("old result"))
        }));
    }

    #[test]
    fn request_projection_bounds_a_growing_tool_loop_without_losing_dialogue() {
        let mut history = Vec::new();
        for turn in 0..8 {
            history.push(ChatMessage::new("user", format!("task turn {turn}")));
            if turn == 0 {
                history.push(ChatMessage::new(
                    "system",
                    "[Evidence-based recovery: stale instruction from turn zero]",
                ));
            }
            history.push(
                ChatMessage::new(
                    "assistant",
                    format!("planning turn {turn}\n```tool\n{{}}\n```"),
                )
                .with_tool_calls(vec![crate::app::ToolCallRef {
                    id: format!("loop-call-{turn}"),
                    name: "view_file".to_string(),
                    arguments: format!(r###"{{"path":"src/loop_{turn}.rs"}}"###),
                }]),
            );
            history.push(
                ChatMessage::new(
                    "tool",
                    format!(
                        "view_file: repeated raw loop output {turn} {}",
                        "x".repeat(512)
                    ),
                )
                .answering(Some(format!("loop-call-{turn}"))),
            );
            history.push(ChatMessage::new(
                "assistant",
                format!("turn {turn} is understood"),
            ));
        }
        history.push(ChatMessage::new("user", "continue the current task"));
        history.push(ChatMessage::new(
            "system",
            "[Evidence-based recovery: take one different action]",
        ));

        let rendered = serde_json::to_string(&to_messages_for_request(
            &history,
            RequestInstructions::new("base", None),
        ))
        .expect("render request");

        assert!(rendered.contains("task turn 0"));
        assert!(rendered.contains("turn 7 is understood"));
        assert!(rendered.contains("continue the current task"));
        assert!(rendered.contains("repeated raw loop output 7"));
        assert!(!rendered.contains("repeated raw loop output 0"));
        assert!(!rendered.contains("loop-call-0"));
        assert!(rendered.contains("Evidence-based recovery: take one different action"));
        assert!(!rendered.contains("stale instruction from turn zero"));
        assert!(rendered.len() < 10_000);
    }

    #[test]
    fn compaction_and_truncation_guidance_are_bounded_runtime_context() {
        let history = vec![
            ChatMessage::new(
                "system",
                format!(
                    "{}\n{}",
                    crate::network::compaction::SUMMARY_MARKER,
                    "x".repeat(MAX_CONTEXT_FRAGMENT_CHARS + 100)
                ),
            ),
            ChatMessage::new("system", "[tool_result_incomplete: request the next range]"),
            ChatMessage::new("tool", "view_file: successful source evidence"),
        ];
        let messages = to_messages_with_instructions(
            &history,
            RequestInstructions::new("base", Some("developer")),
        );
        let rendered = serde_json::to_string(&messages).unwrap();

        assert!(rendered.contains("provenance=\\\"compaction\\\""));
        assert!(rendered.contains("runtime notice truncated"));
        assert!(rendered.contains("tool_result_incomplete"));
        assert!(rendered.contains("successful source evidence"));
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
    }

    #[test]
    fn reconstructed_requests_do_not_accumulate_instruction_history() {
        let history = vec![
            ChatMessage::new("system", "[Session resumed]"),
            ChatMessage::new("user", "finish the review"),
        ];
        let instructions = RequestInstructions::new("base", Some("developer"));

        let retry = to_messages_with_instructions(&history, instructions);
        let after_resume = to_messages_with_instructions(&history, instructions);

        assert_eq!(retry, after_resume);
        assert_eq!(
            retry
                .iter()
                .filter(|message| message["role"] == "system")
                .count(),
            1
        );
        assert_eq!(
            retry
                .iter()
                .filter(|message| message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("Session resumed")))
                .count(),
            1
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
    fn native_tool_calls_never_resurrect_as_text_fences() {
        let batch = ChatMessage::new("assistant", "reading two files").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call-a".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
            crate::app::ToolCallRef {
                id: "call-b".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
        ]);
        let entries: Vec<_> = normalize_history(std::slice::from_ref(&batch)).collect();
        assert!(
            matches!(entries[0], HistoryEntry::Assistant(_)),
            "batched native calls must not truncate to one text fence"
        );

        let single = ChatMessage::new("assistant", "reading one file").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call-a".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
        ]);
        let entries: Vec<_> = normalize_history(std::slice::from_ref(&single)).collect();
        assert!(
            matches!(entries[0], HistoryEntry::Assistant(_)),
            "structured calls are owned by the structured renderer"
        );
    }

    #[test]
    fn failed_textual_tool_checkpoint_replays_as_unexecuted_assistant_text() {
        let partial = "[TOOL_CALLS]write_to_file[ARGS]{\"path\":\"x\",\"content\":\"partial";
        let history = vec![
            ChatMessage::new("user", "write the file"),
            ChatMessage::new("assistant", partial).as_unexecuted_tool_call_checkpoint(),
        ];

        let entries: Vec<_> = normalize_history(&history).collect();
        assert!(matches!(entries[1], HistoryEntry::Assistant(_)));

        let messages = to_messages(&history, "system");
        assert_eq!(messages[2]["role"], "assistant");
        assert!(messages[2]["tool_calls"].is_null());
        assert_eq!(messages[2]["content"], partial);
    }

    #[test]
    fn failed_native_tool_checkpoint_replays_as_plain_diagnostic_text() {
        let checkpoint = "[Partial ApiNative tool-call checkpoint: call id=call-write; tool=write_to_file; arguments incomplete; no tool was executed.]";
        let history = vec![
            ChatMessage::new("user", "write the file"),
            ChatMessage::new("assistant", checkpoint).as_unexecuted_tool_call_checkpoint(),
        ];

        let messages = to_messages(&history, "system");
        assert_eq!(messages[2]["role"], "assistant");
        assert!(messages[2]["tool_calls"].is_null());
        assert!(
            messages[2]["content"]
                .as_str()
                .unwrap()
                .contains("call-write")
        );
        assert!(resolve_tool_calls(&history[1], ToolProtocol::ApiNative).is_empty());
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
        let content = messages[1]["content"].as_str().unwrap();
        assert!(content.contains("metadata:"));
        assert!(content.contains("completeness"));
        assert!(content.contains("arguments_hash"));
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

    fn assert_native_tool_call_results(messages: &[serde_json::Value]) {
        let mut calls = std::collections::BTreeMap::new();
        let mut results = std::collections::BTreeMap::new();
        for message in messages {
            if message["role"] == "assistant" {
                for call in message["tool_calls"].as_array().into_iter().flatten() {
                    let id = call["id"].as_str().expect("native call id");
                    *calls.entry(id.to_owned()).or_insert(0usize) += 1;
                }
            } else if message["role"] == "tool" {
                let id = message["tool_call_id"]
                    .as_str()
                    .expect("provider result id");
                *results.entry(id.to_owned()).or_insert(0usize) += 1;
            }
        }
        assert_eq!(calls, results, "native tool calls and results diverged");
        assert!(
            calls.values().all(|count| *count == 1),
            "duplicate call ids: {calls:?}"
        );
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

    // The older duplicate exchange is excluded from the request, the newer
    // identical read is retained verbatim, and the retained structured pair
    // keeps its tool_call_id mapping.
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
        assert_eq!(assistant_ids, vec!["call_new"]);
        assert_native_tool_call_results(&messages);
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

    #[test]
    fn compaction_projection_closes_only_the_retained_native_calls() {
        let (old_call, old_result) =
            structured_read("call_compacted", "view_file: [File: src/old.rs]\n1: old");
        let current_call = ChatMessage::new("assistant", "inspect current").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call_current".into(),
                name: "view_file".into(),
                arguments: r#"{"path":"src/current.rs"}"#.into(),
            },
        ]);
        let history = vec![
            ChatMessage::new("user", "old task"),
            old_call,
            old_result,
            ChatMessage::new("assistant", "old task finished"),
            ChatMessage::new("user", "middle task"),
            ChatMessage::new("assistant", "middle task finished"),
            ChatMessage::new("user", "current task"),
            current_call,
        ];

        let messages = to_messages_for_request(&history, RequestInstructions::new("system", None));

        assert_native_tool_call_results(&messages);
        assert!(!messages.iter().any(|message| {
            message["tool_calls"]
                .as_array()
                .is_some_and(|calls| calls.iter().any(|call| call["id"] == "call_compacted"))
        }));
        assert!(messages.iter().any(|message| {
            message["tool_call_id"] == "call_current"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("did not run"))
        }));
    }

    #[test]
    fn deferred_native_calls_get_one_provider_result_each() {
        let calls = vec![
            crate::app::ToolCallRef {
                id: "call_executed".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
            crate::app::ToolCallRef {
                id: "call_deferred".into(),
                name: "edit_file".into(),
                arguments: "{}".into(),
            },
        ];
        let history = vec![
            ChatMessage::new("user", "do the work"),
            ChatMessage::new("assistant", "run the first call").with_tool_calls(calls),
            ChatMessage::new("tool", "view_file: completed")
                .answering(Some("call_executed".into())),
            ChatMessage::new("user", "continue"),
        ];

        let messages = to_messages(&history, "system");

        assert_native_tool_call_results(&messages);
        assert!(messages.iter().any(|message| {
            message["tool_call_id"] == "call_deferred"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("did not run"))
        }));
    }

    #[test]
    fn reused_call_id_pairs_with_nearest_announcement() {
        let old_call = ChatMessage::new("assistant", "first attempt").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call-reused".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
        ]);
        let new_call = ChatMessage::new("assistant", "retry attempt").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call-reused".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
        ]);
        let new_result = ChatMessage::new("tool", "view_file: fresh content")
            .answering(Some("call-reused".into()));
        let history = vec![
            ChatMessage::new("user", "inspect"),
            old_call,
            ChatMessage::new("assistant", "interrupted"),
            new_call,
            new_result,
            ChatMessage::new("user", "continue"),
        ];

        let messages = to_messages(&history, "system");

        // Both announcements are retained; the stale one is closed
        // synthetically while the newer one keeps its real result. A global
        // id set would mark both answered and drop the synthetic close.
        let tool_contents: Vec<&str> = messages
            .iter()
            .filter(|message| message["tool_call_id"] == "call-reused")
            .filter_map(|message| message["content"].as_str())
            .collect();
        assert_eq!(tool_contents.len(), 2);
        assert!(
            tool_contents
                .iter()
                .any(|body| body.contains("did not run")),
            "stale reused announcement must be closed: {tool_contents:?}"
        );
        assert!(
            tool_contents
                .iter()
                .any(|body| body.contains("fresh content")),
            "newest result must survive verbatim: {tool_contents:?}"
        );
    }

    #[test]
    fn interrupted_native_calls_keep_their_recorded_results_paired() {
        let calls = vec![
            crate::app::ToolCallRef {
                id: "call_finished_before_interrupt".into(),
                name: "view_file".into(),
                arguments: "{}".into(),
            },
            crate::app::ToolCallRef {
                id: "call_interrupted".into(),
                name: "run_command".into(),
                arguments: "{}".into(),
            },
        ];
        let history = vec![
            ChatMessage::new("user", "inspect and run"),
            ChatMessage::new("assistant", "the provider was interrupted")
                .with_tool_calls(calls)
                .as_unexecuted_tool_call_checkpoint(),
            ChatMessage::new("tool", "view_file: completed")
                .answering(Some("call_finished_before_interrupt".into())),
            ChatMessage::new("tool", "run_command: error: interrupted")
                .answering(Some("call_interrupted".into())),
        ];

        let messages = to_messages(&history, "system");

        assert_native_tool_call_results(&messages);
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

    #[test]
    fn compact_replayed_read_does_not_replace_the_original_result() {
        let history = vec![
            ChatMessage::new(
                "tool",
                "view_file: [File: src/lib.rs, Lines 1 to 2 of 2]\n1: original\n2: result",
            ),
            ChatMessage::new(
                "tool",
                "view_file: [File: src/lib.rs, Lines 1 to 2 of 2]\n[Unchanged read replay: fingerprint=abc; range=src/lib.rs lines 1 to 2 of 2. The earlier result contains this unchanged output.]",
            ),
        ];

        assert!(redundant_tool_result_indices(&history, 0).is_empty());
        let messages = to_messages(&history, "system");
        let rendered = serde_json::to_string(&messages).expect("render history");
        assert!(rendered.contains("1: original"));
        assert!(rendered.contains("Unchanged read replay"));
    }
}
