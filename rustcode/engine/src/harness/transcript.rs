//! The transcript of a thread on an external harness, recorded as the
//! thread's `history.json` so the thread list, search and `session/load`
//! work for every harness.
//!
//! | Live update | Recorded as |
//! | --- | --- |
//! | message and thought chunks | an `assistant` message (`<think>` for thoughts) |
//! | `tool_call` | an `assistant` message naming the call, then a `tool` message |
//! | `tool_call_update` | changes that `tool` message in place: status, output summary |
//! | `plan` | one `tool` message per turn (`tool_name` `plan`), a checklist |
//! | `usage_update` | kept as the thread's latest usage |
//!
//! A tool message's `tool_result.tool_name` is the ACP tool kind, its
//! content the text the card showed (capped at [`TOOL_SUMMARY_LIMIT`]).

use crate::app::ChatMessage;
use agent_client_protocol::schema::v1::{
    ContentBlock, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, SessionUpdate, TextContent,
    ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields, ToolKind,
};
use rustcode_core::{ToolCallRef, ToolResultRecord};
use std::collections::HashMap;

/// Output text a recorded tool call keeps; the harness has the rest.
pub(crate) const TOOL_SUMMARY_LIMIT: usize = 4 * 1024;
const PLAN_TOOL: &str = "plan";
const PLAN_ID_PREFIX: &str = "plan:";

#[derive(Default)]
pub(crate) struct Transcript {
    pub history: Vec<ChatMessage>,
    reply: String,
    thought: String,
    /// Tool call id → (its `assistant` message, its `tool` message).
    tools: HashMap<String, (usize, usize)>,
    /// This turn's plan message.
    plan: Option<usize>,
    /// The latest `usage_update`, as JSON.
    pub usage: Option<serde_json::Value>,
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
                self.apply(&id, &update.fields);
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
        self.apply(&id, &fields);
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
        self.tools
            .insert(id.to_owned(), (call, self.history.len() - 1));
    }

    fn apply(&mut self, id: &str, fields: &ToolCallUpdateFields) {
        let Some(&(call, result)) = self.tools.get(id) else {
            return;
        };
        if let Some(reference) = self.history[call]
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
        let message = &mut self.history[result];
        if let Some(content) = &fields.content {
            message.content = summary(content);
        }
        let record = message.tool_result.get_or_insert_with(Default::default);
        if let Some(kind) = fields.kind {
            record.tool_name = kind_name(kind);
        }
        if let Some(status) = fields.status {
            record.pending = matches!(status, ToolCallStatus::Pending | ToolCallStatus::InProgress);
            record.success = matches!(status, ToolCallStatus::Completed);
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
        for (_, (_, result)) in self.tools.drain() {
            if let Some(record) = self.history[result].tool_result.as_mut()
                && record.pending
            {
                record.pending = false;
                record.success = false;
            }
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

/// The text a tool card showed, capped.
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
    cap(&parts.join("\n"), TOOL_SUMMARY_LIMIT)
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (truncated)", &text[..end])
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
    for message in history {
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
    fn long_output_is_capped() {
        let long = "x".repeat(TOOL_SUMMARY_LIMIT + 10);
        let content = vec![ToolCallContent::from(ContentBlock::Text(TextContent::new(
            long,
        )))];
        assert!(summary(&content).ends_with("… (truncated)"));
    }
}
