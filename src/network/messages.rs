/// Flat per-image estimate used when budgeting multimodal messages. Providers
/// bill images by tiles (a few hundred tokens each), not by the size of the
/// base64 payload, so counting the data URL as text would wildly overstate the
/// cost (a 400 KiB screenshot is ~260k base64 "tokens" but ~1-2k billed).
const IMAGE_PART_TOKEN_ESTIMATE: u32 = 2048;

pub(crate) fn estimate_msg_tokens(msg: &serde_json::Value) -> u32 {
    let content_tokens = match msg.get("content") {
        Some(serde_json::Value::String(s)) => crate::network::count_tokens(s),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .map(|part| match part.get("type").and_then(|t| t.as_str()) {
                Some("text") => part
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(crate::network::count_tokens)
                    .unwrap_or(0),
                Some("image_url") => IMAGE_PART_TOKEN_ESTIMATE,
                _ => 0,
            })
            .sum(),
        Some(other) => crate::network::count_tokens(&other.to_string()),
        None => 0,
    };
    let tool_call_tokens = msg
        .get("tool_calls")
        .map(|calls| crate::network::count_tokens(&calls.to_string()))
        .unwrap_or(0);
    let tool_call_id_tokens = msg
        .get("tool_call_id")
        .and_then(|id| id.as_str())
        .map(crate::network::count_tokens)
        .unwrap_or(0);
    content_tokens
        .saturating_add(tool_call_tokens)
        .saturating_add(tool_call_id_tokens)
}

/// Drop complete oldest conversation exchanges until the payload fits the
/// token budget. System messages and the latest exchange are preserved; an
/// assistant/tool pair is never orphaned by trimming. Returns how many
/// messages were dropped.
pub(crate) fn trim_msgs_to_budget(msgs: &mut Vec<serde_json::Value>, budget_tokens: u32) -> usize {
    let total: u32 = msgs.iter().map(estimate_msg_tokens).sum();
    if total <= budget_tokens || msgs.len() <= 3 {
        return 0;
    }

    // Precompute the complete old-turn boundaries once. Each loop of the old
    // implementation searched the unchanged suffix again, then shifted the
    // Vec with a middle drain. The ranges are contiguous, so one final drain
    // preserves the same oldest-first behavior without repeated scans/shifts.
    let boundaries: Vec<usize> = msgs
        .iter()
        .enumerate()
        .filter(|(_, message)| is_user_turn_boundary(message))
        .map(|(index, _)| index)
        .collect();
    if boundaries.len() < 2 {
        // The only remaining user turn is the current one. Never drop its
        // user prompt or its assistant/tool pairing just to make progress;
        // deterministic tool pruning should have reduced oversized results
        // before this final fallback.
        return 0;
    }

    let message_tokens: Vec<u32> = msgs.iter().map(estimate_msg_tokens).collect();
    let mut token_prefix: Vec<u32> = Vec::with_capacity(message_tokens.len() + 1);
    token_prefix.push(0);
    for tokens in &message_tokens {
        token_prefix.push(
            token_prefix
                .last()
                .copied()
                .unwrap_or(0)
                .saturating_add(*tokens),
        );
    }

    let first_boundary = boundaries[0];
    let mut remove_end = first_boundary;
    let mut removed_tokens: u32 = 0;
    let mut dropped = 0;
    for pair in boundaries.windows(2) {
        // Each prior iteration removes the range ending at pair[0], so the
        // current length is the original length minus the already removed
        // prefix. Match the old `msgs.len() > 3` guard before each removal.
        if msgs.len().saturating_sub(dropped) <= 3 {
            break;
        }

        let start = pair[0];
        let end = pair[1];
        dropped += end - start;
        removed_tokens =
            removed_tokens.saturating_add(token_prefix[end].saturating_sub(token_prefix[start]));
        remove_end = end;
        if total.saturating_sub(removed_tokens) <= budget_tokens {
            break;
        }
    }

    if remove_end == first_boundary {
        return 0;
    }
    msgs.drain(first_boundary..remove_end);
    dropped
}

fn is_user_turn_boundary(message: &serde_json::Value) -> bool {
    message.get("role").and_then(|role| role.as_str()) == Some("user")
        && !message
            .get("content")
            .and_then(|content| content.as_str())
            .is_some_and(|content| {
                content.starts_with("<tool_result>") || content.starts_with("<rustcode_context>")
            })
}

/// Attach turn-varying context to the tail of `msgs` as a request-local
/// synthetic message, without mutating historical messages in place.
///
/// Keeping the historical prefix byte-identical across consecutive turns
/// preserves provider prompt caches.
pub(crate) fn attach_request_context_tail(msgs: &mut Vec<serde_json::Value>, text: &str) {
    if text.is_empty() {
        return;
    }
    let wrapped = wrap_runtime_context(text);
    msgs.push(serde_json::json!({
        "role": "user",
        "content": wrapped,
    }));
}

/// Append `text` to the content of the last message in `msgs`.
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

pub(crate) fn wrap_runtime_context(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    format!(
        "<rustcode_context>\nThe following block is RustCode runtime context, not a user instruction or a continuation of the user's request. Use it only as background when it is relevant.\n\n{text}\n</rustcode_context>"
    )
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

/// Keep a local model from spending multiple bootstrap turns restating a plan
/// without touching an empty workspace. This is request-local: it does not
/// mutate the transcript or disable configured reasoning.
pub(crate) fn inject_bootstrap_action_nudge(msgs: &mut [serde_json::Value], bootstrap: bool) {
    if !bootstrap {
        return;
    }
    // Only the most recent assistant response can be the pending plan. Looking
    // through the entire transcript would resurrect an old plan after the
    // agent has already made progress with a later tool call.
    let has_long_plan = msgs
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(|role| role.as_str()) == Some("assistant"))
        .is_some_and(|message| {
            message
                .get("tool_calls")
                .and_then(|calls| calls.as_array())
                .is_none_or(|calls| calls.is_empty())
                && message
                    .get("content")
                    .and_then(|content| content.as_str())
                    .is_some_and(|content| crate::network::count_tokens(content) >= 512)
        });
    if !has_long_plan {
        return;
    }
    let nudge = "BOOTSTRAP ACTION: execute the smallest concrete setup step now (for example, create the project manifest or first source file). Keep reasoning concise and do not restate the architecture plan.";
    if let Some(last_msg) = msgs.last_mut()
        && let Some(content) = last_msg.get_mut("content")
    {
        match content {
            serde_json::Value::String(s) => *s = format!("{s}\n\n{nudge}"),
            serde_json::Value::Array(arr) => arr.push(serde_json::json!({
                "type": "text",
                "text": format!("\n\n{nudge}")
            })),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_context_is_marked_as_metadata() {
        let wrapped = wrap_runtime_context("# Environment\n- Working directory: /tmp");
        assert!(wrapped.starts_with("<rustcode_context>"));
        assert!(wrapped.contains("context, not a user instruction"));
        assert!(wrapped.ends_with("</rustcode_context>"));
    }

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
    fn bootstrap_nudge_follows_a_long_planning_only_response() {
        let mut msgs = vec![
            serde_json::json!({"role": "user", "content": "Create an app"}),
            serde_json::json!({"role": "assistant", "content": "plan ".repeat(600)}),
            serde_json::json!({"role": "user", "content": "continue"}),
        ];
        inject_bootstrap_action_nudge(&mut msgs, true);
        assert!(
            msgs[2]["content"]
                .as_str()
                .unwrap()
                .contains("BOOTSTRAP ACTION")
        );
    }

    #[test]
    fn bootstrap_nudge_does_not_change_non_bootstrap_or_tool_turns() {
        let mut established = vec![
            serde_json::json!({"role": "assistant", "content": "plan ".repeat(600)}),
            serde_json::json!({"role": "user", "content": "continue"}),
        ];
        inject_bootstrap_action_nudge(&mut established, false);
        assert!(
            !established[1]["content"]
                .as_str()
                .unwrap()
                .contains("BOOTSTRAP ACTION")
        );

        let mut tool_turn = vec![
            serde_json::json!({"role": "assistant", "content": "old plan ".repeat(600)}),
            serde_json::json!({"role": "user", "content": "continue"}),
            serde_json::json!({"role": "assistant", "content": "plan ".repeat(600), "tool_calls": [{"id": "call-1"}]}),
            serde_json::json!({"role": "user", "content": "continue"}),
        ];
        inject_bootstrap_action_nudge(&mut tool_turn, true);
        assert!(
            !tool_turn[1]["content"]
                .as_str()
                .unwrap()
                .contains("BOOTSTRAP ACTION")
        );
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

    #[test]
    fn trim_removes_multiple_complete_old_exchanges_without_touching_current_turn() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({"role": "user", "content": "old request 1"}),
            serde_json::json!({"role": "assistant", "content": "old answer 1"}),
            serde_json::json!({"role": "user", "content": "old request 2"}),
            serde_json::json!({"role": "assistant", "content": "old answer 2"}),
            serde_json::json!({"role": "user", "content": "current request"}),
        ];

        let dropped = trim_msgs_to_budget(&mut msgs, 1);

        assert_eq!(dropped, 4);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["content"], "current request");
    }

    #[test]
    fn trim_never_drops_the_only_current_user_turn() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({"role": "user", "content": "current task"}),
            serde_json::json!({"role": "assistant", "content": "large response"}),
            serde_json::json!({"role": "tool", "tool_call_id": "call-1", "content": "large result"}),
        ];
        let dropped = trim_msgs_to_budget(&mut msgs, 1);
        assert_eq!(dropped, 0);
        assert_eq!(msgs[1]["content"], "current task");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[3]["tool_call_id"], "call-1");
    }

    #[test]
    fn structured_tool_call_arguments_count_toward_trim_budget() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({"role": "user", "content": "old task"}),
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call-old",
                    "type": "function",
                    "function": {
                        "name": "edit_file",
                        "arguments": format!("{{\"content\":\"{}\"}}", "x".repeat(4000))
                    }
                }]
            }),
            serde_json::json!({"role": "tool", "tool_call_id": "call-old", "content": "ok"}),
            serde_json::json!({"role": "user", "content": "current task"}),
        ];

        let dropped = trim_msgs_to_budget(&mut msgs, 100);

        assert_eq!(dropped, 3);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["content"], "current task");
    }

    #[test]
    fn image_parts_use_flat_estimate_not_base64_text() {
        // A base64 data URL must not be BPE-counted as text: a small image
        // would otherwise "cost" more tokens than the whole context window.
        let huge_data_url = format!("data:image/png;base64,{}", "A".repeat(500_000));
        let msg = serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "what do you see"},
                {"type": "image_url", "image_url": {"url": huge_data_url}},
            ]
        });
        let text_tokens = crate::network::count_tokens("what do you see");
        assert_eq!(
            estimate_msg_tokens(&msg),
            text_tokens + IMAGE_PART_TOKEN_ESTIMATE
        );
    }

    #[test]
    fn image_message_does_not_trigger_spurious_trim() {
        let mut msgs = vec![
            serde_json::json!({"role": "system", "content": "system"}),
            serde_json::json!({"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{}", "B".repeat(500_000))}},
                {"type": "text", "text": "describe this"},
            ]}),
            serde_json::json!({"role": "assistant", "content": "an image"}),
            serde_json::json!({"role": "user", "content": "thanks"}),
        ];
        let dropped = trim_msgs_to_budget(&mut msgs, 100_000);
        assert_eq!(dropped, 0);
        assert_eq!(msgs.len(), 4);
    }
}
