pub(crate) const RESPONSE_RESERVE_TOKENS: u32 = 1024;

fn estimate_msg_tokens(msg: &serde_json::Value) -> u32 {
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => crate::network::count_tokens(s),
        Some(other) => crate::network::count_tokens(&other.to_string()),
        None => 0,
    }
}

/// Drop complete oldest conversation exchanges until the payload fits the
/// token budget. System messages and the latest exchange are preserved; an
/// assistant/tool pair is never orphaned by trimming. Returns how many
/// messages were dropped.
pub(crate) fn trim_msgs_to_budget(msgs: &mut Vec<serde_json::Value>, budget_tokens: u32) -> usize {
    let mut total: u32 = msgs.iter().map(estimate_msg_tokens).sum();
    let mut dropped = 0;
    while total > budget_tokens && msgs.len() > 3 {
        let Some(start) = msgs
            .iter()
            .position(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
        else {
            break;
        };

        // Remove from the first user turn through the message before the next
        // user turn. This keeps each assistant/tool response attached to the
        // request that produced it.
        let end = msgs
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, m)| m.get("role").and_then(|r| r.as_str()) == Some("user"))
            .map(|(i, _)| i)
            .unwrap_or(start + 1);
        let remove_end = end.min(msgs.len().saturating_sub(2));
        if remove_end <= start {
            break;
        }
        for msg in msgs.drain(start..remove_end) {
            total = total.saturating_sub(estimate_msg_tokens(&msg));
            dropped += 1;
        }
    }
    dropped
}

/// Append `text` to the content of the last message in `msgs`.
///
/// Used to place turn-varying context (environment delta, files-in-context,
/// task plan) at the *tail* of the request rather than in the system prompt.
/// Keeping the system prompt static across turns preserves the provider's
/// automatic prefix cache — dynamic content in the prefix would invalidate the
/// whole cached prefix every round and re-bill the full input.
pub(crate) fn append_to_last_message(msgs: &mut [serde_json::Value], text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last_msg) = msgs.last_mut()
        && let Some(content) = last_msg.get_mut("content")
    {
        match content {
            serde_json::Value::String(s) => {
                *s = format!("{s}\n\n{text}");
            }
            serde_json::Value::Array(arr) => {
                arr.push(serde_json::json!({
                    "type": "text",
                    "text": format!("\n\n{text}")
                }));
            }
            _ => {}
        }
    }
}

/// If the message history has grown long (e.g. >= 4 messages), inject a brief
/// system reminder right before the latest user message or tool result. This
/// prevents the model from forgetting the core guidelines and tool formats
/// due to attention dilution in long contexts.
pub(crate) fn inject_system_reminder(msgs: &mut [serde_json::Value]) {
    if msgs.len() >= 4 {
        let reminder_text = "REMINDER: Follow the configured tool protocol exactly. Use tools only when needed, inspect results before choosing the next action, and report relevant verification when finished.";

        if let Some(last_msg) = msgs.last_mut()
            && let Some(content) = last_msg.get_mut("content")
        {
            match content {
                serde_json::Value::String(s) => {
                    *s = format!("{}\n\n{}", s, reminder_text);
                }
                serde_json::Value::Array(arr) => {
                    arr.push(serde_json::json!({
                        "type": "text",
                        "text": format!("\n\n{}", reminder_text)
                    }));
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_to_last_message_string_content() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "SYS"}),
            serde_json::json!({"role": "user", "content": "hello"}),
        ];
        append_to_last_message(&mut msgs, "# Environment\nchanged");
        // System prefix must stay untouched so the cache prefix is stable.
        assert_eq!(msgs[0]["content"], "SYS");
        assert_eq!(msgs[1]["content"], "hello\n\n# Environment\nchanged");
    }

    #[test]
    fn append_to_last_message_array_content() {
        let mut msgs = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "hi"}]
        })];
        append_to_last_message(&mut msgs, "tail");
        let arr = msgs[0]["content"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["text"], "\n\ntail");
    }

    #[test]
    fn append_to_last_message_empty_is_noop() {
        let mut msgs = vec![serde_json::json!({"role": "user", "content": "hi"})];
        append_to_last_message(&mut msgs, "");
        assert_eq!(msgs[0]["content"], "hi");
    }

    #[test]
    fn trim_removes_complete_old_exchange() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({"role": "user", "content": "old request"}),
            serde_json::json!({"role": "assistant", "content": "old tool call"}),
            serde_json::json!({"role": "user", "content": "old tool result"}),
            serde_json::json!({"role": "assistant", "content": "latest answer"}),
        ];
        let dropped = trim_msgs_to_budget(&mut msgs, 8);
        assert_eq!(dropped, 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["content"], "old tool result");
        assert_eq!(msgs[2]["content"], "latest answer");
    }
}
