use crate::app::ChatMessage;
use crate::network::events::{ToolResult, ToolResultMetadata};
use crate::network::tool_exec::result::stable_arguments_hash;
use crate::tools::{ToolCall, ToolSafety};

/// Whether equivalent successful calls may execute again within one logical
/// user turn. Unknown tools are conservative because MCP calls can perform
/// external side effects; MCP tools may opt into repeatable reads with the
/// standard `readOnlyHint` annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolReplayPolicy {
    Repeatable,
    OncePerUserTurn,
}

pub(crate) fn tool_replay_policy(name: &str) -> ToolReplayPolicy {
    match crate::tools::tool_safety(name) {
        ToolSafety::ReadOnly | ToolSafety::ControlPlane | ToolSafety::Interactive => {
            ToolReplayPolicy::Repeatable
        }
        ToolSafety::WorkspaceMutation | ToolSafety::ProcessControl | ToolSafety::Delegation => {
            ToolReplayPolicy::OncePerUserTurn
        }
        ToolSafety::Unknown => crate::tools::mcp_tool_read_only_hint(name)
            .then_some(ToolReplayPolicy::Repeatable)
            .unwrap_or(ToolReplayPolicy::OncePerUserTurn),
    }
}

/// Return a synthetic successful replay when durable history proves the same
/// side effect already completed after the latest explicit user message.
pub(crate) fn successful_side_effect_replay(
    history: &[ChatMessage],
    call: &ToolCall,
) -> Option<ToolResult> {
    if tool_replay_policy(&call.name) == ToolReplayPolicy::Repeatable {
        return None;
    }
    let turn_start = history
        .iter()
        .rposition(|message| message.role == "user" && !message.content.starts_with('/'))?;
    let arguments_hash = stable_arguments_hash(&call.arguments);
    let completed = history[turn_start + 1..].iter().any(|message| {
        message.tool_result.as_ref().is_some_and(|result| {
            result.success
                && !result.pending
                && result.tool_name.eq_ignore_ascii_case(&call.name)
                && result.arguments_hash == arguments_hash
        })
    });
    completed.then(|| ToolResult {
        tool_name: call.name.clone(),
        content: "[Side-effect replay suppressed: an equivalent call already succeeded in this user turn.]"
            .to_string(),
        diff: None,
        file_preview: None,
        metadata: ToolResultMetadata {
            arguments_hash,
            success: true,
            replayed: true,
            ..Default::default()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ToolResultRecord;

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            name: name.to_string(),
            arguments,
            call_id: None,
        }
    }

    fn successful_result(call: &ToolCall) -> ChatMessage {
        ChatMessage::new("tool", "sent").with_tool_result(ToolResultRecord {
            tool_name: call.name.clone(),
            arguments_hash: stable_arguments_hash(&call.arguments),
            success: true,
            ..Default::default()
        })
    }

    #[test]
    fn equivalent_side_effect_is_suppressed_across_mode_and_continuity_notes() {
        let original = call(
            "send_email",
            serde_json::json!({"to":"person@example.test","subject":"Hello"}),
        );
        let history = vec![
            ChatMessage::new("user", "Send the email"),
            successful_result(&original),
            ChatMessage::new("system", "Switched mode"),
            ChatMessage::new("assistant", "Continuing after reset"),
        ];
        let reordered = call(
            "SEND_EMAIL",
            serde_json::json!({"subject":"Hello","to":"person@example.test"}),
        );

        let replay = successful_side_effect_replay(&history, &reordered)
            .expect("equivalent successful side effect must not execute again");
        assert!(replay.metadata.success);
        assert!(replay.metadata.replayed);
    }

    #[test]
    fn distinct_explicit_user_turn_allows_the_same_side_effect() {
        let send = call("send_email", serde_json::json!({"message":"hello"}));
        let history = vec![
            ChatMessage::new("user", "Send it once"),
            successful_result(&send),
            ChatMessage::new("user", "Send it again"),
        ];
        assert!(successful_side_effect_replay(&history, &send).is_none());
    }

    #[test]
    fn reads_and_failed_side_effects_remain_executable() {
        let read = call("view_file", serde_json::json!({"path":"src/lib.rs"}));
        let send = call("send_email", serde_json::json!({"message":"hello"}));
        let failed = ChatMessage::new("tool", "failed").with_tool_result(ToolResultRecord {
            tool_name: send.name.clone(),
            arguments_hash: stable_arguments_hash(&send.arguments),
            success: false,
            ..Default::default()
        });
        let history = vec![ChatMessage::new("user", "Try it"), failed];

        assert_eq!(tool_replay_policy(&read.name), ToolReplayPolicy::Repeatable);
        assert!(successful_side_effect_replay(&history, &read).is_none());
        assert!(successful_side_effect_replay(&history, &send).is_none());
    }
}
