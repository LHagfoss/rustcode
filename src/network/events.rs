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
}
