//! Switching a thread to another harness: the new harness
//! starts its own session, and its first prompt carries the conversation so
//! far, newest first back to a budget, inside `<context>` before the user's
//! message. The thread shows a divider where the switch happened; the divider
//! is a `handoff` message in the thread's history, so a reload shows it too,
//! and nothing replays it to a model.

use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
use rustcode_core::ChatMessage;
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

/// The history message that records a move from `from_machine` to `to_machine`
/// (and any harness/account change that accompanied it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn marker_machine(
    from_harness: &str,
    from_account: Option<&str>,
    to_harness: &str,
    to_account: Option<&str>,
    from_machine: &str,
    to_machine: &str,
    model: Option<&str>,
    resumed_native: bool,
) -> ChatMessage {
    let mut message = ChatMessage::new(
        ROLE,
        json!({
            "from": from_harness,
            "to": to_harness,
            "from_account": from_account,
            "to_account": to_account,
            "from_machine": from_machine,
            "to_machine": to_machine,
            "model": model,
            "resumed_native": resumed_native,
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

/// The divider the client draws for a recorded switch or machine move.
pub(crate) fn divider(marker: &ChatMessage) -> Option<SessionUpdate> {
    let value = serde_json::from_str::<Value>(&marker.content).ok()?;
    let from = value.get("from").and_then(Value::as_str)?;
    let to = value.get("to").and_then(Value::as_str)?;
    let model = value.get("model").and_then(Value::as_str);
    let from_account = value.get("from_account").and_then(Value::as_str);
    let to_account = value.get("to_account").and_then(Value::as_str);
    let from_machine = value.get("from_machine").and_then(Value::as_str);
    let to_machine = value.get("to_machine").and_then(Value::as_str);
    let resumed_native = value
        .get("resumed_native")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = match (from_machine, to_machine) {
        (Some(src), Some(dst)) if from == to && from_account == to_account => {
            format!("Moved from {src} to {dst}")
        }
        (Some(src), Some(dst)) => format!(
            "Moved from {src} ({}) to {dst} ({})",
            label(from, from_account),
            label(to, to_account)
        ),
        _ => format!(
            "Switched from {} to {}",
            label(from, from_account),
            label(to, to_account)
        ),
    };
    let mut rustcode = serde_json::Map::new();
    rustcode.insert("kind".to_owned(), json!("handoff"));
    rustcode.insert("from".to_owned(), json!(from));
    rustcode.insert("to".to_owned(), json!(to));
    rustcode.insert("from_account".to_owned(), json!(from_account));
    rustcode.insert("to_account".to_owned(), json!(to_account));
    rustcode.insert("model".to_owned(), json!(model));
    if let Some(src) = from_machine {
        rustcode.insert("from_machine".to_owned(), json!(src));
    }
    if let Some(dst) = to_machine {
        rustcode.insert("to_machine".to_owned(), json!(dst));
    }
    if resumed_native {
        rustcode.insert("resumed_native".to_owned(), json!(true));
    }
    let mut meta = serde_json::Map::new();
    meta.insert("rustcode".to_owned(), Value::Object(rustcode));
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
            // The model's reasoning stays behind: replaying it into another
            // model's prompt reads as copying reasoning to it, and Anthropic's
            // safeguards then refuse every turn (`reasoning_extraction`).
            let answer = rustcode_tool_protocol::text::strip_think_blocks(text);
            let answer = answer.trim();
            let mut parts = Vec::new();
            if !answer.is_empty() && !message.conversation_recap {
                parts.push(format!("[assistant]\n{answer}"));
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
    fn the_models_reasoning_is_not_handed_over() {
        let mut call = ChatMessage::new("assistant", "<think>Plan the rounding.\n\n</think>");
        call.tool_calls.push(rustcode_core::ToolCallRef {
            id: "t1".to_owned(),
            name: "Round the caps".to_owned(),
            arguments: "{}".to_owned(),
        });
        let history = vec![
            ChatMessage::new("user", "Cap the files"),
            call,
            ChatMessage::new("assistant", "<think>Check both paths</think>Done."),
        ];
        let text = context(&history, "claude-code", "claude-code").unwrap();
        assert!(!text.contains("<think>"));
        assert!(!text.contains("Plan the rounding"));
        assert!(!text.contains("Check both paths"));
        assert!(text.contains("[tool call] Round the caps"));
        assert!(text.contains("[assistant]\nDone."));
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
    fn an_old_harness_id_and_a_new_account_label_both_read() {
        let history = vec![ChatMessage::new("user", "hello")];
        // Records written before accounts kept the harness id.
        let old = context(&history, "claude-code", "codex").unwrap();
        assert!(
            old.starts_with(
                "<context>\nThis conversation started on Claude Code and continues here on Codex"
            ),
            "{old}"
        );
        // Since accounts, the value may be a display label; it is not looked
        // up as a harness id and reads as written.
        let label = "Claude Code · Paral 2";
        let new = context(&history, label, "claude-code").unwrap();
        assert!(
            new.starts_with(
                "<context>\nThis conversation started on Claude Code · Paral 2 and continues here on Claude Code"
            ),
            "{new}"
        );
        let update = divider(&marker_with(
            "claude-code",
            Some("Paral 2"),
            "claude-code",
            Some("Paral 3"),
            None,
        ))
        .unwrap();
        let value = serde_json::to_value(&update).unwrap();
        assert_eq!(
            value["content"]["text"],
            "Switched from Claude Code · Paral 2 to Claude Code · Paral 3"
        );
        assert_eq!(value["_meta"]["rustcode"]["from_account"], "Paral 2");
        assert_eq!(value["_meta"]["rustcode"]["to_account"], "Paral 3");
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

    #[test]
    fn machine_move_marker_replays_with_machine_names_and_stays_out_of_context() {
        let same = marker_machine(
            "claude-code",
            None,
            "claude-code",
            None,
            "MacBook",
            "VPS",
            Some("claude-sonnet-5"),
            false,
        );
        let value = serde_json::to_value(divider(&same).unwrap()).unwrap();
        assert_eq!(value["_meta"]["rustcode"]["kind"], "handoff");
        assert_eq!(value["_meta"]["rustcode"]["from_machine"], "MacBook");
        assert_eq!(value["_meta"]["rustcode"]["to_machine"], "VPS");
        assert_eq!(value["content"]["text"], "Moved from MacBook to VPS");

        let switched = marker_machine(
            "claude-code",
            None,
            "codex",
            None,
            "MacBook",
            "VPS",
            Some("gpt-6"),
            false,
        );
        let switched_val = serde_json::to_value(divider(&switched).unwrap()).unwrap();
        assert_eq!(
            switched_val["content"]["text"],
            "Moved from MacBook (Claude Code) to VPS (Codex)"
        );

        let resumed = marker_machine(
            "claude-code",
            None,
            "claude-code",
            None,
            "VPS",
            "MacBook",
            None,
            true,
        );
        let resumed_val = serde_json::to_value(divider(&resumed).unwrap()).unwrap();
        assert_eq!(resumed_val["_meta"]["rustcode"]["resumed_native"], true);

        let ctx = context(
            &[ChatMessage::new("user", "Hi"), same],
            "MacBook",
            "claude-code",
        )
        .unwrap();
        assert!(!ctx.contains("from_machine"));
    }
}
