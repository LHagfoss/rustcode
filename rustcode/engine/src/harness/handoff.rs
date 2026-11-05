//! Switching a thread to another harness: the new harness
//! starts its own session, and its first prompt carries the conversation so
//! far, newest first back to a budget, inside `<context>` before the user's
//! message. The thread shows a divider where the switch happened; the divider
//! is a `handoff` message in the thread's history, so a reload shows it too,
//! and nothing replays it to a model.

use crate::app::ChatMessage;
use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
use serde_json::{Value, json};

/// The history role of a switch.
pub(crate) const ROLE: &str = "handoff";

/// How much of the conversation the new harness gets, in characters (about
/// 16k tokens).
const BUDGET: usize = 64 * 1024;
/// A tool call's result is cut to this much.
const TOOL_LIMIT: usize = 600;

/// How a thread's origin or destination is named in a divider: the harness,
/// with the account's name in brackets when it runs on a named login.
pub(crate) fn label(harness: &str, account: Option<&str>) -> String {
    let harness = super::display_name(harness);
    match account {
        Some(account) => format!("{harness} · {account}"),
        None => harness,
    }
}

/// The history message that records a switch from `from` to `to`, and the
/// named accounts either side runs on.
pub(crate) fn marker_with(
    from: &str,
    from_account: Option<&str>,
    to: &str,
    to_account: Option<&str>,
    model: Option<&str>,
) -> ChatMessage {
    let mut message = ChatMessage::new(
        ROLE,
        json!({
            "from": from, "to": to,
            "from_account": from_account, "to_account": to_account,
            "model": model,
        })
        .to_string(),
    );
    message.timestamp = chrono::Local::now().to_rfc3339();
    message
}

/// The history message that records a switch from `from` to `to`.
#[cfg(test)]
pub(crate) fn marker(from: &str, to: &str, model: Option<&str>) -> ChatMessage {
    marker_with(from, None, to, None, model)
}

/// The divider the client draws for a recorded switch.
pub(crate) fn divider(marker: &ChatMessage) -> Option<SessionUpdate> {
    let value = serde_json::from_str::<Value>(&marker.content).ok()?;
    let from = value.get("from").and_then(Value::as_str)?;
    let to = value.get("to").and_then(Value::as_str)?;
    let model = value.get("model").and_then(Value::as_str);
    let from_account = value.get("from_account").and_then(Value::as_str);
    let to_account = value.get("to_account").and_then(Value::as_str);
    let text = format!(
        "Switched from {} to {}",
        label(from, from_account),
        label(to, to_account)
    );
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        json!({
            "kind": "handoff", "from": from, "to": to,
            "from_account": from_account, "to_account": to_account, "model": model,
        }),
    );
    Some(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::Text(TextContent::new(text))).meta(meta),
    ))
}

/// The preamble of the first prompt after a switch: the conversation so far
/// in `<context>`, then `User message:` for the prompt that follows. `None`
/// when there is nothing to hand over.
pub(crate) fn context(history: &[ChatMessage], from: &str, to: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut used = 0;
    let mut omitted = 0;
    for message in super::transcript::without_startup_banner(history)
        .iter()
        .rev()
    {
        let Some(line) = render(message) else {
            continue;
        };
        if used + line.len() > BUDGET {
            omitted += 1;
            continue;
        }
        used += line.len();
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let mut out = format!(
        "<context>\nThis conversation started on {} and continues here on {}. \
         It is the conversation so far, oldest first",
        super::display_name(from),
        super::display_name(to)
    );
    if omitted > 0 {
        out.push_str(&format!(
            "; {omitted} earlier messages did not fit and are left out"
        ));
    }
    out.push_str(". Tool results are shortened.\n\n");
    out.push_str(&lines.join("\n\n"));
    out.push_str("\n</context>\n\nUser message:\n");
    Some(out)
}

fn render(message: &ChatMessage) -> Option<String> {
    let text = message.content.trim();
    match message.role.as_str() {
        "user" if !text.is_empty() => Some(format!("[user]\n{text}")),
        "assistant" => {
            let mut parts = Vec::new();
            if !text.is_empty() && !message.conversation_recap {
                parts.push(format!("[assistant]\n{text}"));
            }
            for call in &message.tool_calls {
                parts.push(format!("[tool call] {}", cut(&call.name, 200)));
            }
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
        "tool" if !text.is_empty() => Some(format!("[tool result]\n{}", cut(text, TOOL_LIMIT))),
        "system" if message.compaction_boundary.is_some() && !text.is_empty() => {
            Some(format!("[summary of the earlier conversation]\n{text}"))
        }
        _ => None,
    }
}

fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_holds_the_conversation_and_ends_before_the_message() {
        let mut call = ChatMessage::new("assistant", "Looking.");
        call.tool_calls.push(rustcode_core::ToolCallRef {
            id: "t1".to_owned(),
            name: "Read src/main.rs".to_owned(),
            arguments: "{}".to_owned(),
        });
        let history = vec![
            ChatMessage::new("user", "Fix the bug"),
            call,
            ChatMessage::new("tool", "x".repeat(2000)),
            ChatMessage::new("assistant", "Fixed it."),
            marker("claude-code", "codex", None),
        ];
        let text = context(&history, "claude-code", "codex").unwrap();
        assert!(text.starts_with("<context>\nThis conversation started on Claude Code"));
        assert!(text.contains("[user]\nFix the bug"));
        assert!(text.contains("[tool call] Read src/main.rs"));
        assert!(text.contains(&format!("{}…", "x".repeat(TOOL_LIMIT))));
        assert!(text.ends_with("</context>\n\nUser message:\n"));
        assert!(
            !text.contains("\"from\""),
            "the switch itself is not context"
        );
    }

    #[test]
    fn a_long_conversation_keeps_its_newest_part() {
        let history: Vec<_> = (0..200)
            .map(|i| ChatMessage::new("user", format!("message {i} {}", "y".repeat(1000))))
            .collect();
        let text = context(&history, "codex", "pi").unwrap();
        assert!(text.contains("message 199"));
        assert!(!text.contains("message 0 "));
        assert!(text.contains("earlier messages did not fit"));
    }

    #[test]
    fn a_stored_startup_banner_is_not_handed_over() {
        let history = vec![
            ChatMessage::new(
                "assistant",
                "pi v1.5.0\n---\n\n## Skills\n- /a/SKILL.md".to_owned(),
            ),
            ChatMessage::new("user", "hello".to_owned()),
        ];
        let text = context(&history, "pi", "codex").unwrap();
        assert!(!text.contains("SKILL.md"));
        assert!(text.contains("hello"));
        assert_eq!(super::super::transcript::replay(&history, None).len(), 1);
    }

    #[test]
    fn nothing_to_hand_over_is_none() {
        assert!(context(&[], "codex", "pi").is_none());
    }

    #[test]
    fn the_marker_replays_as_a_divider() {
        let update = divider(&marker("claude-code", "codex", Some("gpt-6-luna"))).unwrap();
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(value["_meta"]["rustcode"]["kind"], "handoff");
        assert_eq!(value["_meta"]["rustcode"]["model"], "gpt-6-luna");
        assert_eq!(
            value["content"]["text"],
            "Switched from Claude Code to Codex"
        );
    }
}
