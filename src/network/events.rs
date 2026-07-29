use crate::tools::ToolCall;

/// Structured result produced by a tool execution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolResult {
    pub tool_name: String,
    pub content: String,
    pub diff: Option<String>,
}

impl ToolResult {
    pub fn is_error(&self) -> bool {
        self.content
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("error")
    }
}

/// Provider-independent reason that a model response stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    Cancelled,
    Error(String),
    Unknown(String),
}

impl FinishReason {
    pub fn from_provider(value: Option<&str>) -> Self {
        match value {
            Some("stop") | None => Self::Stop,
            Some("tool_calls") | Some("function_call") => Self::ToolCalls,
            Some("length") => Self::Length,
            Some(other) => Self::Unknown(other.to_string()),
        }
    }
}

/// Convert one completed provider response into the events consumed by the
/// turn loop. Tool-call detection lives here so every caller applies the same
/// precedence: structured tool work first, otherwise final text.
pub(crate) fn classify_response(
    content: &str,
    provider_finish_reason: Option<&str>,
    protocol: crate::config::ToolProtocol,
) -> Vec<AgentEvent> {
    let tool_calls = crate::tools::parse_tool_calls(content, protocol);
    let mut events = tool_calls
        .into_iter()
        .map(AgentEvent::ToolCall)
        .collect::<Vec<_>>();

    if events.is_empty() {
        events.push(AgentEvent::TextDelta(content.to_string()));
    }

    let finish_reason = if events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCall(_)))
    {
        FinishReason::ToolCalls
    } else {
        FinishReason::from_provider(provider_finish_reason)
    };
    events.push(AgentEvent::Finished(finish_reason));
    events
}

/// Events exchanged between response handling, tool execution, and the turn
/// state machine. The current loop still consumes some legacy return values;
/// this type is the migration seam for the event-driven loop.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum AgentEvent {
    TextDelta(String),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Finished(FinishReason),
    ContextLimit,
    Cancelled,
    Error(String),
}

/// Decision returned by the turn state machine after a response is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnAction {
    ExecuteTools,
    FinishResponse,
    Cancel,
    RecoverError,
}

pub(crate) fn next_turn_action(
    cancelled: bool,
    stream_failed: bool,
    force_final: bool,
    has_tool_calls: bool,
    task_completed: bool,
) -> TurnAction {
    if cancelled {
        return TurnAction::Cancel;
    }
    if stream_failed {
        return TurnAction::RecoverError;
    }
    if force_final || task_completed {
        return TurnAction::FinishResponse;
    }
    if has_tool_calls {
        TurnAction::ExecuteTools
    } else {
        TurnAction::FinishResponse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_provider_finish_reasons() {
        assert_eq!(FinishReason::from_provider(Some("stop")), FinishReason::Stop);
        assert_eq!(
            FinishReason::from_provider(Some("tool_calls")),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_provider(Some("length")), FinishReason::Length);
        assert_eq!(FinishReason::from_provider(None), FinishReason::Stop);
    }

    #[test]
    fn tool_result_exposes_error_status() {
        let result = ToolResult {
            tool_name: "run_command".to_string(),
            content: "error: command failed".to_string(),
            diff: None,
        };
        assert!(result.is_error());
        assert!(!ToolResult {
            tool_name: "grep".to_string(),
            content: json!("match").to_string(),
            diff: None,
        }
        .is_error());
    }

    #[test]
    fn classifies_tool_response_before_text_completion() {
        let events = classify_response(
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"TODO\"}}\n```",
            Some("stop"),
            crate::config::ToolProtocol::Json,
        );

        assert!(matches!(events[0], AgentEvent::ToolCall(_)));
        assert_eq!(events.last(), Some(&AgentEvent::Finished(FinishReason::ToolCalls)));
    }

    #[test]
    fn classifies_plain_response_as_text_then_finish() {
        let events = classify_response("done", Some("stop"), crate::config::ToolProtocol::Json);

        assert_eq!(events[0], AgentEvent::TextDelta("done".to_string()));
        assert_eq!(events[1], AgentEvent::Finished(FinishReason::Stop));
    }

    #[test]
    fn prioritizes_safety_and_terminal_actions() {
        assert_eq!(
            next_turn_action(true, false, false, true, false),
            TurnAction::Cancel
        );
        assert_eq!(
            next_turn_action(false, true, false, true, false),
            TurnAction::RecoverError
        );
        assert_eq!(
            next_turn_action(false, false, true, true, false),
            TurnAction::FinishResponse
        );
        assert_eq!(
            next_turn_action(false, false, false, true, false),
            TurnAction::ExecuteTools
        );
        assert_eq!(
            next_turn_action(false, false, false, false, false),
            TurnAction::FinishResponse
        );
    }
}
