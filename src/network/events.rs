use crate::tools::ToolCall;

/// Structured result produced by a tool execution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolResult {
    pub tool_name: String,
    pub content: String,
    pub diff: Option<String>,
    pub metadata: ToolResultMetadata,
}

/// Machine-readable execution facts kept alongside human-readable output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolResultMetadata {
    pub call_id: Option<String>,
    pub arguments_hash: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub changed_paths: Vec<String>,
    pub truncated: bool,
    pub full_output_artifact: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseSource {
    Native,
    Fenced,
    Tagged,
    RepairedJson,
    PlainText,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelResponse {
    pub raw_content: String,
    pub events: Vec<AgentEvent>,
    pub source: ResponseSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnState {
    AwaitingModel,
    AwaitingApproval,
    ExecutingTools,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnInput {
    ModelFinished { has_tool_calls: bool },
    ApprovalGranted,
    ToolsFinished,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidTransition {
    pub from: TurnState,
    pub input: TurnInput,
}

pub(crate) fn transition_turn(state: TurnState, input: TurnInput) -> Result<TurnState, InvalidTransition> {
    match (state, input) {
        (_, TurnInput::Cancelled) => Ok(TurnState::Cancelled),
        (TurnState::AwaitingModel, TurnInput::ModelFinished { has_tool_calls: true }) => Ok(TurnState::AwaitingApproval),
        (TurnState::AwaitingModel, TurnInput::ModelFinished { has_tool_calls: false }) => Ok(TurnState::Completed),
        (TurnState::AwaitingApproval, TurnInput::ApprovalGranted) => Ok(TurnState::ExecutingTools),
        (TurnState::ExecutingTools, TurnInput::ToolsFinished) => Ok(TurnState::AwaitingModel),
        _ => Err(InvalidTransition { from: state, input }),
    }
}

pub(crate) fn response_source(
    content: &str,
    protocol: crate::config::ToolProtocol,
    has_tool_calls: bool,
) -> ResponseSource {
    if !has_tool_calls {
        return ResponseSource::PlainText;
    }
    if protocol == crate::config::ToolProtocol::ApiNative {
        return ResponseSource::Native;
    }
    if content.contains("```tool") {
        return ResponseSource::Fenced;
    }
    if content.contains("[TOOL_CALLS]") {
        return ResponseSource::Tagged;
    }
    ResponseSource::RepairedJson
}

pub(crate) fn normalize_response(
    content: &str,
    provider_finish_reason: Option<&str>,
    protocol: crate::config::ToolProtocol,
) -> ModelResponse {
    let events = classify_response(content, provider_finish_reason, protocol);
    let has_tool_calls = events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolCall(_)));
    ModelResponse {
        raw_content: content.to_string(),
        source: response_source(content, protocol, has_tool_calls),
        events,
    }
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

/// Typed lifecycle for one model/tool turn. The orchestrator owns side
/// effects, while this state machine owns hand-off and terminal decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnMachine {
    state: TurnState,
}

impl TurnMachine {
    pub(crate) fn new() -> Self {
        Self { state: TurnState::AwaitingModel }
    }

    pub(crate) fn state(self) -> TurnState {
        self.state
    }

    pub(crate) fn model_finished(
        &mut self,
        cancelled: bool,
        force_final: bool,
        has_tool_calls: bool,
        task_completed: bool,
    ) -> TurnAction {
        if cancelled {
            self.state = TurnState::Cancelled;
            return TurnAction::Cancel;
        }
        debug_assert_eq!(self.state, TurnState::AwaitingModel);
        if force_final || task_completed || !has_tool_calls {
            self.state = TurnState::Completed;
            return TurnAction::FinishResponse;
        }
        self.state = TurnState::AwaitingApproval;
        TurnAction::ExecuteTools
    }

    pub(crate) fn approval_granted(&mut self) {
        debug_assert_eq!(self.state, TurnState::AwaitingApproval);
        if self.state == TurnState::AwaitingApproval {
            self.state = TurnState::ExecutingTools;
        }
    }

    pub(crate) fn approval_denied(&mut self) {
        debug_assert_eq!(self.state, TurnState::AwaitingApproval);
        if self.state == TurnState::AwaitingApproval {
            self.state = TurnState::AwaitingModel;
        }
    }

    pub(crate) fn tools_finished(&mut self) {
        debug_assert_eq!(self.state, TurnState::ExecutingTools);
        if self.state == TurnState::ExecutingTools {
            self.state = TurnState::AwaitingModel;
        }
    }

    pub(crate) fn recover_error(&mut self) -> TurnAction {
        self.state = TurnState::AwaitingModel;
        TurnAction::RecoverError
    }

    pub(crate) fn cancel(&mut self) -> TurnAction {
        self.state = TurnState::Cancelled;
        TurnAction::Cancel
    }
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
    fn api_native_responses_are_marked_native() {
        let response = normalize_response(
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"x\"}}\n```",
            Some("tool_calls"),
            crate::config::ToolProtocol::ApiNative,
        );
        assert_eq!(response.source, ResponseSource::Native);
        assert!(response.events.iter().any(|event| matches!(event, AgentEvent::ToolCall(_))));
    }

    #[test]
    fn tool_result_exposes_error_status() {
        let result = ToolResult {
            tool_name: "run_command".to_string(),
            content: "error: command failed".to_string(),
            diff: None,
            metadata: ToolResultMetadata::default(),
        };
        assert!(result.is_error());
        assert!(!ToolResult {
            tool_name: "grep".to_string(),
            content: json!("match").to_string(),
            diff: None,
            metadata: ToolResultMetadata::default(),
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
    fn records_response_source_without_changing_event_contract() {
        let response = normalize_response(
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"x\"}}\n```",
            Some("stop"),
            crate::config::ToolProtocol::Json,
        );
        assert_eq!(response.source, ResponseSource::Fenced);
        assert!(matches!(response.events[0], AgentEvent::ToolCall(_)));
        assert_eq!(response.raw_content, "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"x\"}}\n```");
    }

    #[test]
    fn turn_state_transitions_have_terminal_safety_precedence() {
        assert_eq!(
            transition_turn(
                TurnState::AwaitingModel,
                TurnInput::ModelFinished { has_tool_calls: true }
            ),
            Ok(TurnState::AwaitingApproval)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingApproval, TurnInput::ApprovalGranted),
            Ok(TurnState::ExecutingTools)
        );
        assert_eq!(
            transition_turn(TurnState::ExecutingTools, TurnInput::ToolsFinished),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::ExecutingTools, TurnInput::Cancelled),
            Ok(TurnState::Cancelled)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingModel, TurnInput::ToolsFinished),
            Err(InvalidTransition { from: TurnState::AwaitingModel, input: TurnInput::ToolsFinished })
        );
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

    #[test]
    fn turn_machine_owns_model_approval_execution_lifecycle() {
        let mut machine = TurnMachine::new();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(machine.model_finished(false, false, true, false), TurnAction::ExecuteTools);
        assert_eq!(machine.state(), TurnState::AwaitingApproval);
        machine.approval_granted();
        assert_eq!(machine.state(), TurnState::ExecutingTools);
        machine.tools_finished();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(machine.model_finished(false, false, false, false), TurnAction::FinishResponse);
        assert_eq!(machine.state(), TurnState::Completed);
    }

    #[test]
    fn turn_machine_terminal_inputs_override_tool_requests() {
        let mut machine = TurnMachine::new();
        assert_eq!(machine.model_finished(true, false, true, false), TurnAction::Cancel);
        assert_eq!(machine.state(), TurnState::Cancelled);

        let mut machine = TurnMachine::new();
        assert_eq!(machine.model_finished(false, true, true, false), TurnAction::FinishResponse);
        assert_eq!(machine.state(), TurnState::Completed);
    }
}
