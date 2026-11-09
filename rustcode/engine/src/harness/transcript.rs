//! The transcript of a thread on an external harness, recorded as the
//! thread's `history.json` so the thread list, search and `session/load`
//! work for every harness.
//!
//! | Live update | Recorded as |
//! | --- | --- |
//! | message and thought chunks | an `assistant` message (`<think>` for thoughts) |
//! | `tool_call` | an `assistant` message naming the call, then a `tool` message |
//! | `tool_call_update` | changes that `tool` message in place: status, output |
//! | `plan` | one `tool` message per turn (`tool_name` `plan`), a checklist |
//! | `usage_update` | kept as the thread's latest usage |
//!
//! A tool message's `tool_result.tool_name` is the ACP tool kind. Its content
//! is the output, wherever the harness put it: `content` text and diffs, the
//! `terminal_output` deltas Pi sends for a shell command (with its exit code
//! in `terminal_exit`), or the structured `rawOutput` Codex and Claude Code
//! use for MCP and other non-terminal tools. Output is bounded exactly like a
//! built-in tool result — one message, with the full text in
//! `tool_output/` when it does not fit — so a replay or a post-mortem has the
//! same evidence as a native turn. Images and other binary payloads stay out.

use crate::app::ChatMessage;
use agent_client_protocol::schema::v1::{
    ContentBlock, Meta, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate,
    TextContent, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields,
    ToolKind,
};
use rustcode_core::{ToolCallRef, ToolResultRecord};
use serde_json::Value;
use std::collections::HashMap;

const PLAN_TOOL: &str = "plan";
const PLAN_ID_PREFIX: &str = "plan:";

/// While a turn runs, one tool's output is accumulated up to this much before
/// it is bounded for the message: enough that the live card keeps streaming,
/// small enough that a runaway command cannot grow history without bound. The
/// final message keeps the built-in limit (50 KiB or 1000 lines, the rest in
/// `tool_output/`), applied when the turn ends.
const TOOL_OUTPUT_ACCUMULATION_LIMIT: usize = 256 * 1024;

/// Where one recorded tool call lives in `history`.
#[derive(Debug, Clone, Copy)]
struct ToolSlot {
    /// The `assistant` message naming the call.
    call: usize,
    /// The `tool` message holding its result.
    result: usize,
}

#[derive(Default)]
pub(crate) struct Transcript {
    pub history: Vec<ChatMessage>,
    reply: String,
    thought: String,
    /// Tool call id → where its two messages are.
    tools: HashMap<String, ToolSlot>,
    /// This turn's plan message.
    plan: Option<usize>,
    /// The latest `usage_update`, as JSON.
    pub usage: Option<serde_json::Value>,
}

/// Whether a message chunk is a compaction that is running or failed, shown
/// while it happens and never kept (`acp/usage.rs` › `compaction_progress_card`).
fn live_compaction_line(meta: Option<&Meta>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .filter(|rustcode| rustcode.get("kind").and_then(Value::as_str) == Some("compaction"))
        .and_then(|rustcode| rustcode.get("phase")?.as_str())
        .is_some_and(|phase| matches!(phase, "started" | "failed"))
}

impl Transcript {
    pub(crate) fn new(history: Vec<ChatMessage>) -> Self {
        Self {
            history,
            ..Self::default()
        }
    }

    pub(crate) fn observe(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                // "Compacting context…" and a compaction failure are live lines
                // (`_meta.rustcode.phase`), not part of the reply.
                if live_compaction_line(chunk.meta.as_ref()) {
                    return;
                }
                if let ContentBlock::Text(text) = &chunk.content {
                    self.reply.push_str(&text.text);
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    self.thought.push_str(&text.text);
                }
            }
            SessionUpdate::ToolCall(call) => self.tool_call(call),
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                if !self.tools.contains_key(&id) {
                    // An update for a call never announced: record it once it
                    // says what it is.
                    let Some(title) = update.fields.title.clone() else {
                        return;
                    };
                    self.start_tool(&id, &title);
                }
                self.apply(&id, &update.fields, update.meta.as_ref());
            }
            SessionUpdate::Plan(plan) => self.plan(plan),
            SessionUpdate::UsageUpdate(_) => {
                self.usage = serde_json::to_value(update).ok();
            }
            _ => {}
        }
    }

    fn tool_call(&mut self, call: &ToolCall) {
        let id = call.tool_call_id.0.to_string();
        if !self.tools.contains_key(&id) {
            self.start_tool(&id, &call.title);
        }
        let mut fields = ToolCallUpdateFields::new()
            .title(call.title.clone())
            .kind(call.kind)
            .status(call.status)
            .content(call.content.clone())
            .locations(call.locations.clone());
        if let Some(input) = &call.raw_input {
            fields = fields.raw_input(input.clone());
        }
        if let Some(output) = &call.raw_output {
            fields = fields.raw_output(output.clone());
        }
        self.apply(&id, &fields, call.meta.as_ref());
    }

    /// Close the text so far into an `assistant` message that names the
    /// call, then add its `tool` message.
    fn start_tool(&mut self, id: &str, title: &str) {
        let mut message = self
            .take_reply()
            .unwrap_or_else(|| ChatMessage::new("assistant", ""));
        message.tool_calls.push(ToolCallRef {
            id: id.to_owned(),
            name: title.to_owned(),
            arguments: "{}".to_owned(),
        });
        self.history.push(message);
        let call = self.history.len() - 1;
        let mut result = ChatMessage::new("tool", "");
        result.tool_call_id = Some(id.to_owned());
        result.tool_result = Some(ToolResultRecord {
            tool_name: kind_name(ToolKind::Other),
            pending: true,
            ..ToolResultRecord::default()
        });
        self.history.push(result);
        self.tools.insert(
            id.to_owned(),
            ToolSlot {
                call,
                result: self.history.len() - 1,
            },
        );
    }

    fn apply(&mut self, id: &str, fields: &ToolCallUpdateFields, meta: Option<&Meta>) {
        let Some(slot) = self.tools.get(id).copied() else {
            return;
        };
        if let Some(reference) = self.history[slot.call]
            .tool_calls
            .iter_mut()
            .find(|reference| reference.id == id)
        {
            if let Some(title) = &fields.title {
                reference.name = title.clone();
            }
            if let Some(input) = &fields.raw_input {
                reference.arguments = input.to_string();
            }
        }
        // The output, from wherever this harness delivers it.
        let snapshot = fields
            .content
            .as_deref()
            .map(summary)
            .filter(|text| !text.is_empty());
        let terminal_delta = meta.and_then(terminal_output);
        let raw = fields
            .raw_output
            .as_ref()
            .and_then(raw_output_text)
            .filter(|text| !text.is_empty());
        let exit_code = meta
            .and_then(terminal_exit)
            .or_else(|| fields.raw_output.as_ref().and_then(raw_exit_code));

        let message = &mut self.history[slot.result];
        if let Some(text) = snapshot {
            // A harness sends the whole output each time for a non-terminal
            // tool, so this replaces rather than appends.
            message.content = text;
        } else if let Some(delta) = terminal_delta {
            // A terminal streams deltas: append, bounded while the turn runs.
            append_capped(&mut message.content, &delta, TOOL_OUTPUT_ACCUMULATION_LIMIT);
        } else if let Some(text) = raw
            && message.content.is_empty()
        {
            // Structured output for an MCP or dynamic tool, which carries no
            // card text of its own.
            message.content = text;
        }
        let record = message.tool_result.get_or_insert_with(Default::default);
        if let Some(kind) = fields.kind {
            record.tool_name = kind_name(kind);
        }
        if let Some(status) = fields.status {
            record.pending = matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress);
            record.success = matches!(status, ToolCallStatus::Completed);
        }
        if let Some(code) = exit_code {
            record.exit_code = Some(code as i32);
        }
        if let Some(locations) = &fields.locations {
            record.changed_paths = locations
                .iter()
                .map(|location| location.path.display().to_string())
                .collect();
        }
    }

    fn plan(&mut self, plan: &Plan) {
        let content = plan_checklist(plan);
        if let Some(index) = self.plan {
            self.history[index].content = content;
            return;
        }
        if let Some(reply) = self.take_reply() {
            self.history.push(reply);
        }
        let mut message = ChatMessage::new("tool", content);
        message.tool_call_id = Some(format!("{PLAN_ID_PREFIX}{}", self.history.len()));
        message.tool_result = Some(ToolResultRecord {
            tool_name: PLAN_TOOL.to_owned(),
            success: true,
            ..ToolResultRecord::default()
        });
        self.history.push(message);
        self.plan = Some(self.history.len() - 1);
    }

    pub(crate) fn push_user(&mut self, text: &str) {
        self.end_reply();
        self.plan = None;
        self.history.push(ChatMessage::new("user", text.to_owned()));
    }

    /// The turn ended: record its text, and a call that never finished as
    /// failed, so a replay shows no card still running.
    pub(crate) fn end_reply(&mut self) {
        if let Some(reply) = self.take_reply() {
            self.history.push(reply);
        }
        for (_, slot) in self.tools.drain() {
            finalize_tool_message(&mut self.history[slot.result]);
        }
    }

    fn take_reply(&mut self) -> Option<ChatMessage> {
        if self.reply.is_empty() && self.thought.is_empty() {
            return None;
        }
        let content = if self.thought.trim().is_empty() {
            std::mem::take(&mut self.reply)
        } else {
            format!(
                "<think>{}</think>{}",
                std::mem::take(&mut self.thought),
                std::mem::take(&mut self.reply)
            )
        };
        self.thought.clear();
        Some(ChatMessage::new("assistant", content))
    }
}

fn kind_name(kind: ToolKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

fn parse_kind(name: &str) -> ToolKind {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).unwrap_or(ToolKind::Other)
}

/// The text a tool card showed. Not capped here: the message is bounded once,
/// when the turn ends, so the full text can reach its artifact.
fn summary(content: &[ToolCallContent]) -> String {
    let mut parts = Vec::new();
    for item in content {
        match item {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(text) => parts.push(text.text.clone()),
                ContentBlock::Image(_) => parts.push("[image]".to_owned()),
                ContentBlock::ResourceLink(link) => parts.push(link.uri.clone()),
                _ => {}
            },
            ToolCallContent::Diff(diff) => parts.push(format!("Edited {}", diff.path.display())),
            _ => {}
        }
    }
    parts.join("\n")
}

/// `_meta.terminal_output.data`: the next chunk of a shell command's output,
/// as the harness streams it.
fn terminal_output(meta: &Meta) -> Option<String> {
    meta.get("terminal_output")?
        .get("data")?
        .as_str()
        .map(str::to_owned)
        .filter(|data| !data.is_empty())
}

/// `_meta.terminal_exit.exit_code`: how the command ended.
fn terminal_exit(meta: &Meta) -> Option<i64> {
    meta.get("terminal_exit")?
        .get("exit_code")
        .and_then(Value::as_i64)
}

/// The exit code a structured `rawOutput` carries (`exitCode` or
/// `exit_code`), for a harness that does not use terminal meta.
fn raw_exit_code(raw: &Value) -> Option<i64> {
    ["exitCode", "exit_code", "exit"]
        .iter()
        .find_map(|key| raw.get(*key).and_then(Value::as_i64))
}

/// The text inside a structured `rawOutput`.
///
/// Codex sends an MCP tool's result there, Claude Code sends a tool result's
/// content blocks, and Pi sends a tool's structured result. Only text is
/// taken: images, audio and other binary blocks are skipped rather than
/// stringified.
fn raw_output_text(raw: &Value) -> Option<String> {
    let mut parts = Vec::new();
    let mut binary = false;
    collect_raw_text(raw, &mut parts, 0, &mut binary);
    let text = parts.join("\n");
    if !text.trim().is_empty() {
        return Some(text);
    }
    // A payload that was only an image or a blob has no text to keep.
    if binary {
        return None;
    }
    // A small status object with no text of its own still tells the reader
    // what happened; anything larger is left out.
    let compact = serde_json::to_string(raw).ok()?;
    (compact.len() <= 4 * 1024).then_some(compact)
}

fn collect_raw_text(value: &Value, parts: &mut Vec<String>, depth: usize, binary: &mut bool) {
    if depth > 6 || parts.len() >= 64 {
        return;
    }
    match value {
        Value::String(text) => {
            if looks_binary(text) {
                *binary = true;
            } else if !text.trim().is_empty() {
                parts.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_raw_text(item, parts, depth + 1, binary);
            }
        }
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str).unwrap_or("");
            if matches!(kind, "image" | "audio" | "blob") {
                *binary = true;
                return;
            }
            for key in [
                "text",
                "content",
                "output",
                "stdout",
                "stderr",
                "aggregatedOutput",
                "result",
                "data",
                "message",
                "error",
            ] {
                if let Some(inner) = map.get(key) {
                    collect_raw_text(inner, parts, depth + 1, binary);
                }
            }
        }
        _ => {}
    }
}

/// A string that is mostly base64 or a data URL is binary payload, not text.
fn looks_binary(text: &str) -> bool {
    if text.starts_with("data:") && text.contains(";base64,") {
        return true;
    }
    let sample: String = text.chars().take(4_096).collect();
    !sample.is_empty() && sample.bytes().filter(|byte| *byte == b'=').count() > sample.len() / 8
}

/// Appends `text`, stopping at `limit` with one truncation marker.
fn append_capped(target: &mut String, text: &str, limit: usize) {
    if target.len() >= limit {
        return;
    }
    if target.len() + text.len() <= limit {
        target.push_str(text);
        return;
    }
    let mut room = limit - target.len();
    while room > 0 && !text.is_char_boundary(room) {
        room -= 1;
    }
    target.push_str(&text[..room]);
    target.push_str("\n… (truncated)");
}

/// Bounds a finished tool message the way a built-in tool result is bounded:
/// one message, long output cut to the shared limits with the full text saved
/// under `tool_output/`, so a replay and the search index stay the same size
/// as for a native turn.
fn finalize_tool_message(message: &mut ChatMessage) {
    if let Some(record) = message.tool_result.as_mut()
        && record.pending
    {
        record.pending = false;
        record.success = false;
    }
    if message.content.is_empty() {
        return;
    }
    let name = message
        .tool_result
        .as_ref()
        .map(|record| record.tool_name.as_str())
        .unwrap_or("tool");
    let bounded = crate::network::output::truncate_tool_output_for_message(
        name,
        std::mem::take(&mut message.content),
        "",
    );
    message.content = bounded.content;
    if let Some(record) = message.tool_result.as_mut() {
        record.truncated |= bounded.truncated;
        record.payload_truncated |= bounded.truncated;
        if record.full_output_artifact.is_none() {
            record.full_output_artifact = bounded.full_output_artifact;
        }
    }
}

fn plan_checklist(plan: &Plan) -> String {
    plan.entries
        .iter()
        .map(|entry| {
            let mark = match entry.status {
                PlanEntryStatus::Completed => "[x]",
                PlanEntryStatus::InProgress => "[~]",
                _ => "[ ]",
            };
            format!("- {mark} {}", entry.content.replace('\n', " "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_checklist(text: &str) -> Plan {
    let entries = text
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("- ")?;
            let (status, content) = if let Some(rest) = line.strip_prefix("[x] ") {
                (PlanEntryStatus::Completed, rest)
            } else if let Some(rest) = line.strip_prefix("[~] ") {
                (PlanEntryStatus::InProgress, rest)
            } else {
                (PlanEntryStatus::Pending, line.strip_prefix("[ ] ")?)
            };
            Some(PlanEntry::new(content, PlanEntryPriority::Medium, status))
        })
        .collect();
    Plan::new(entries)
}

/// The history without the startup banner older Pi threads recorded as their
/// first reply: its version, context files and every skill path.
pub(crate) fn without_startup_banner(history: &[ChatMessage]) -> &[ChatMessage] {
    match history.first() {
        Some(first)
            if first.role == "assistant"
                && first.content.starts_with("pi v")
                && first.content.contains("\n## Skills\n") =>
        {
            &history[1..]
        }
        _ => history,
    }
}

/// The updates that show a recorded external transcript again: text as the
/// built-in replay sends it, each tool call as one finished card, each plan
/// as a `plan`.
pub(crate) fn replay(history: &[ChatMessage], cwd: Option<&std::path::Path>) -> Vec<SessionUpdate> {
    let titles = history
        .iter()
        .flat_map(|message| &message.tool_calls)
        .map(|call| (call.id.as_str(), call))
        .collect::<HashMap<_, _>>();
    let mut updates = Vec::new();
    for message in without_startup_banner(history) {
        match (
            message.role.as_str(),
            &message.tool_result,
            &message.tool_call_id,
        ) {
            (super::handoff::ROLE, _, _) => updates.extend(super::handoff::divider(message)),
            (super::switch::ROLE, _, _) => updates.extend(super::switch::divider(message)),
            ("tool", Some(record), Some(_)) if record.tool_name == PLAN_TOOL => {
                updates.push(SessionUpdate::Plan(parse_checklist(&message.content)));
            }
            ("tool", Some(record), Some(id)) => {
                let reference = titles.get(id.as_str());
                let mut call = ToolCall::new(
                    id.clone(),
                    reference.map_or_else(|| "Tool".to_owned(), |call| call.name.clone()),
                )
                .kind(parse_kind(&record.tool_name))
                .status(if record.success && !record.pending {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                })
                .locations(
                    record
                        .changed_paths
                        .iter()
                        .map(ToolCallLocation::new)
                        .collect(),
                );
                if !message.content.is_empty() {
                    call = call.content(vec![ToolCallContent::from(ContentBlock::Text(
                        TextContent::new(message.content.clone()),
                    ))]);
                }
                if let Some(input) = reference
                    .and_then(|call| {
                        serde_json::from_str::<serde_json::Value>(&call.arguments).ok()
                    })
                    .filter(|input| input.as_object().is_none_or(|object| !object.is_empty()))
                {
                    call = call.raw_input(input);
                }
                updates.push(SessionUpdate::ToolCall(call));
            }
            ("assistant", _, _) if !message.tool_calls.is_empty() => {
                if !message.content.is_empty() {
                    let mut text = message.clone();
                    text.tool_calls.clear();
                    updates.extend(crate::acp::replay_history_updates(&[text], cwd));
                }
            }
            _ => updates.extend(crate::acp::replay_history_updates(
                std::slice::from_ref(message),
                cwd,
            )),
        }
    }
    updates
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ToolCallUpdate;

    #[test]
    fn a_call_still_running_when_the_turn_ends_replays_as_failed() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "Run tests")
                .kind(ToolKind::Execute)
                .status(ToolCallStatus::InProgress),
        ));
        transcript.end_reply();
        let replayed = serde_json::to_value(replay(&transcript.history, None)).unwrap();
        assert_eq!(replayed[1]["sessionUpdate"], "tool_call");
        assert_eq!(replayed[1]["title"], "Run tests");
        assert_eq!(replayed[1]["status"], "failed");
    }

    #[test]
    fn plans_round_trip_through_their_checklist() {
        let plan = Plan::new(vec![
            PlanEntry::new(
                "Read",
                PlanEntryPriority::Medium,
                PlanEntryStatus::Completed,
            ),
            PlanEntry::new(
                "Fix\nit",
                PlanEntryPriority::Medium,
                PlanEntryStatus::InProgress,
            ),
            PlanEntry::new("Ship", PlanEntryPriority::Medium, PlanEntryStatus::Pending),
        ]);
        let text = plan_checklist(&plan);
        assert_eq!(text, "- [x] Read\n- [~] Fix it\n- [ ] Ship");
        let back = parse_checklist(&text);
        assert_eq!(back.entries.len(), 3);
        assert_eq!(back.entries[1].status, PlanEntryStatus::InProgress);
        assert_eq!(back.entries[2].content, "Ship");
    }

    #[test]
    fn a_compaction_shown_while_it_runs_is_not_recorded_in_the_reply() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&crate::acp::usage::compaction_progress_card(
            crate::acp::usage::CompactionPhase::Started,
            "Compacting context…".to_owned(),
        ));
        transcript.observe(&crate::acp::usage::compaction_progress_card(
            crate::acp::usage::CompactionPhase::Failed,
            "Context compaction failed: x".to_owned(),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history.len(), 1, "{:?}", transcript.history);
    }

    fn tool_update(fields: ToolCallUpdateFields) -> SessionUpdate {
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new("t1", fields))
    }

    /// Pi's shell command: the output only ever arrives as terminal deltas,
    /// with the exit code in its own meta object.
    #[test]
    fn pi_terminal_deltas_become_the_tool_row_output() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "echo hello-from-tool")
                .kind(ToolKind::Execute)
                .status(ToolCallStatus::Pending)
                .content(vec![ToolCallContent::Terminal(
                    agent_client_protocol::schema::v1::Terminal::new("t1"),
                )])
                .meta(Meta::from_iter([(
                    "terminal_info".to_owned(),
                    serde_json::json!({"terminal_id": "t1", "cwd": "/tmp"}),
                )])),
        ));
        let mut delta = |data: &str| {
            transcript.observe(&SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(
                    "t1",
                    ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
                )
                .meta(Meta::from_iter([(
                    "terminal_output".to_owned(),
                    serde_json::json!({"terminal_id": "t1", "data": data}),
                )])),
            ));
        };
        delta("hello-");
        delta("from-tool\n");
        transcript.observe(&SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(
                "t1",
                ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            )
            .meta(Meta::from_iter([(
                "terminal_exit".to_owned(),
                serde_json::json!({"terminal_id": "t1", "exit_code": 0, "signal": null}),
            )])),
        ));
        transcript.end_reply();

        let message = &transcript.history[2];
        assert_eq!(message.role, "tool");
        assert_eq!(message.content, "hello-from-tool\n");
        let record = message.tool_result.as_ref().unwrap();
        assert_eq!(record.exit_code, Some(0));
        assert!(record.success);

        let replayed = serde_json::to_value(replay(&transcript.history, None)).unwrap();
        let card = replayed
            .as_array()
            .unwrap()
            .iter()
            .find(|update| update["sessionUpdate"] == "tool_call")
            .expect("the call replays");
        assert_eq!(card["content"][0]["content"]["text"], "hello-from-tool\n");
    }

    /// Pi's non-shell tools send the full output as card content each time.
    #[test]
    fn pi_content_snapshot_replaces_the_earlier_output() {
        let text = |text: &str| {
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new(text),
                ))])
        };
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "read").kind(ToolKind::Read),
        ));
        transcript.observe(&tool_update(text("partial")));
        transcript.observe(&tool_update(text("the whole file")));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "the whole file");
    }

    /// Codex's `mcpToolCall`: no card content, the result in `rawOutput`.
    #[test]
    fn codex_mcp_raw_output_becomes_the_tool_row_output() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "search_sessions").kind(ToolKind::Other),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .raw_output(serde_json::json!({
                    "content": [{"type": "text", "text": "found two threads"}],
                    "isError": false,
                })),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "found two threads");
    }

    /// Codex's `commandExecution`: content text plus a `rawOutput` with the
    /// exit code. The text wins; the code is kept.
    #[test]
    fn codex_command_output_and_exit_code_are_kept() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "cargo test").kind(ToolKind::Execute),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Failed)
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new("test failed"),
                ))])
                .raw_output(serde_json::json!({"exitCode": 101, "output": "test failed"})),
        ));
        transcript.end_reply();
        let record = transcript.history[2].tool_result.as_ref().unwrap();
        assert_eq!(transcript.history[2].content, "test failed");
        assert_eq!(record.exit_code, Some(101));
        assert!(!record.success);
    }

    /// Claude Code's adapter puts a tool result's blocks in `rawOutput`, with
    /// no card content, for calls its renderer does not describe.
    #[test]
    fn claude_raw_output_blocks_are_kept_as_text() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "Read /repo/AGENTS.md").kind(ToolKind::Read),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .raw_output(serde_json::json!([
                    {"type": "text", "text": "# Rules\nLine one."},
                    {"type": "image", "data": "aGVsbG8=", "mimeType": "image/png"},
                ])),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "# Rules\nLine one.");
    }

    /// Opencode sends its output as card content, which already worked; the
    /// row must keep it and not gain a raw dump on top.
    #[test]
    fn opencode_content_is_kept_without_a_raw_dump() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "bash").kind(ToolKind::Execute),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new("done"),
                ))])
                .raw_output(serde_json::json!({"output": "done", "exitCode": 0})),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "done");
        assert_eq!(
            transcript.history[2]
                .tool_result
                .as_ref()
                .unwrap()
                .exit_code,
            Some(0)
        );
    }

    #[test]
    fn an_image_only_raw_output_stores_no_text() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "screenshot").kind(ToolKind::Other),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .raw_output(serde_json::json!({"content": [
                    {"type": "image", "data": "aGVsbG8=", "mimeType": "image/png"}
                ]})),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "");
    }

    /// A command that floods the terminal cannot grow history without bound:
    /// the row keeps the built-in limit and the full text goes to a file.
    #[test]
    fn a_flood_of_terminal_output_is_bounded_with_an_artifact() {
        const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
        // Artifacts go to the config directory, which a test build keeps to
        // itself; nothing here touches a real one.
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "yes").kind(ToolKind::Execute),
        ));
        for _ in 0..8 {
            transcript.observe(&SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(
                    "t1",
                    ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
                )
                .meta(Meta::from_iter([(
                    "terminal_output".to_owned(),
                    serde_json::json!({"data": "x".repeat(30 * 1024)}),
                )])),
            ));
        }
        transcript.end_reply();

        let message = &transcript.history[2];
        let record = message.tool_result.as_ref().unwrap();
        assert!(
            message.content.len() <= MAX_TOOL_OUTPUT_BYTES,
            "message stayed {} bytes",
            message.content.len()
        );
        assert!(record.truncated && record.payload_truncated);
        let artifact = record.full_output_artifact.as_ref().expect("artifact");
        let saved = std::fs::read_to_string(artifact).unwrap();
        assert_eq!(saved.len(), 8 * 30 * 1024, "the whole flood was saved");
        assert!(message.content.contains("Full output saved to"));
    }

    #[test]
    fn a_turn_with_no_output_keeps_an_empty_row() {
        let mut transcript = Transcript::default();
        transcript.push_user("go");
        transcript.observe(&SessionUpdate::ToolCall(
            ToolCall::new("t1", "sleep").kind(ToolKind::Other),
        ));
        transcript.observe(&tool_update(
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        ));
        transcript.end_reply();
        assert_eq!(transcript.history[2].content, "");
        assert!(transcript.history[2].tool_result.as_ref().unwrap().success);
    }
}
