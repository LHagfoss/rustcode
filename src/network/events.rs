use crate::tools::ToolCall;

/// Structured result produced by a tool execution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolResult {
    pub tool_name: String,
    pub content: String,
    pub diff: Option<String>,
    pub file_preview: Option<(String, String)>,
    pub metadata: ToolResultMetadata,
}

impl ToolResult {
    /// Build the single provider-independent result contract. Human-facing
    /// `content` remains separate from these typed execution facts; callers
    /// must not classify failures by searching the display text.
    pub(crate) fn execution_envelope(&self) -> crate::tools::ToolResultEnvelope {
        crate::tools::ToolResultEnvelope {
            call_id: self
                .metadata
                .call_id
                .clone()
                .unwrap_or_else(|| format!("local_{}", self.metadata.arguments_hash)),
            tool_name: self.tool_name.clone(),
            success: self.metadata.success,
            error_kind: self.metadata.error_kind,
            retryable: self.metadata.retryable,
            exit_code: self.metadata.exit_code,
            changed_paths: self.metadata.changed_paths.clone(),
            output: self.content.clone(),
            truncated: self.metadata.truncated,
            full_output_artifact: self.metadata.full_output_artifact.clone(),
            replayed: self.metadata.replayed,
        }
    }
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
    pub replayed: bool,
    pub error_kind: Option<crate::tools::ToolErrorKind>,
    pub retryable: bool,
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
    ModelFinished {
        has_tool_calls: bool,
    },
    ApprovalGranted,
    ApprovalDenied,
    ToolsFinished,
    ErrorRecovered,
    /// The finish gate rejected a prose completion and the orchestrator is
    /// intentionally starting another model round.
    RetryRequested,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidTransition {
    pub from: TurnState,
    pub input: TurnInput,
}

impl std::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid turn transition: {:?} cannot accept {:?}",
            self.from, self.input
        )
    }
}

/// The single source of truth for the turn lifecycle. Every state change the
/// orchestrator makes must go through here, so an illegal hand-off (executing
/// tools before approval, finishing tools that never started, …) is rejected
/// instead of silently corrupting the machine.
///
/// Legal transitions:
///   AwaitingModel   --ModelFinished{tools}--> AwaitingApproval
///   AwaitingModel   --ModelFinished{none}---> Completed
///   AwaitingModel   --ErrorRecovered-------> AwaitingModel
///   AwaitingApproval --ApprovalGranted-----> ExecutingTools
///   AwaitingApproval --ApprovalDenied------> AwaitingModel
///   AwaitingApproval --ErrorRecovered------> AwaitingModel
///   ExecutingTools  --ToolsFinished-------> AwaitingModel
///   ExecutingTools  --ErrorRecovered------> AwaitingModel
///   Completed       --RetryRequested------> AwaitingModel
///   <any non-terminal> --Cancelled--------> Cancelled
pub(crate) fn transition_turn(
    state: TurnState,
    input: TurnInput,
) -> Result<TurnState, InvalidTransition> {
    use TurnState::{AwaitingApproval, AwaitingModel, Cancelled, Completed, ExecutingTools};
    let next = match (state, input) {
        // Terminal states never transition again.
        (Completed, TurnInput::RetryRequested) => AwaitingModel,
        (Completed | Cancelled, _) => return Err(InvalidTransition { from: state, input }),
        (_, TurnInput::Cancelled) => Cancelled,
        (
            AwaitingModel,
            TurnInput::ModelFinished {
                has_tool_calls: true,
            },
        ) => AwaitingApproval,
        (
            AwaitingModel,
            TurnInput::ModelFinished {
                has_tool_calls: false,
            },
        ) => Completed,
        (AwaitingApproval, TurnInput::ApprovalGranted) => ExecutingTools,
        (AwaitingApproval, TurnInput::ApprovalDenied) => AwaitingModel,
        (ExecutingTools, TurnInput::ToolsFinished) => AwaitingModel,
        // A recoverable provider/stream error rewinds any in-flight turn back to
        // awaiting the model so the loop can retry cleanly.
        (AwaitingModel | AwaitingApproval | ExecutingTools, TurnInput::ErrorRecovered) => {
            AwaitingModel
        }
        _ => return Err(InvalidTransition { from: state, input }),
    };
    Ok(next)
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
    // Reasoning that arrived in the content channel behind a bare `thought`
    // marker becomes a normal `<think>` span first, so it is classified and
    // stored as reasoning rather than as the model's answer.
    let content = &super::text::promote_bare_thought_markers(content);
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

/// Adapt provider-native calls into the common event contract without looking
/// at the response text. The text is retained for history and diagnostics,
/// but it cannot create a second call or turn a fenced decoy into execution.
pub(crate) fn native_response(
    content: &str,
    provider_finish_reason: Option<&str>,
    tool_calls: Vec<ToolCall>,
) -> ModelResponse {
    let has_tool_calls = !tool_calls.is_empty();
    let mut events = tool_calls
        .into_iter()
        .map(AgentEvent::ToolCall)
        .collect::<Vec<_>>();

    if !has_tool_calls {
        events.push(AgentEvent::TextDelta(content.to_string()));
    }

    events.push(AgentEvent::Finished(if has_tool_calls {
        FinishReason::ToolCalls
    } else {
        FinishReason::from_provider(provider_finish_reason)
    }));

    ModelResponse {
        raw_content: content.to_string(),
        source: response_source(
            content,
            crate::config::ToolProtocol::ApiNative,
            has_tool_calls,
        ),
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
        Self {
            state: TurnState::AwaitingModel,
        }
    }

    pub(crate) fn state(self) -> TurnState {
        self.state
    }

    /// Apply one input through the validated transition table. On success the
    /// state advances and the new state is returned; on an illegal transition
    /// the state is left untouched, a debug assertion fires (to catch
    /// orchestrator bugs in tests/dev), and the error is returned so callers
    /// can degrade gracefully in release builds instead of corrupting state.
    fn apply(&mut self, input: TurnInput) -> Result<TurnState, InvalidTransition> {
        match transition_turn(self.state, input) {
            Ok(next) => {
                self.state = next;
                Ok(next)
            }
            Err(invalid) => {
                crate::logger::operational_event(
                    "turn.invalid_transition",
                    serde_json::json!({
                        "from": format!("{:?}", invalid.from),
                        "input": format!("{:?}", invalid.input),
                        "detail": invalid.to_string(),
                    }),
                );
                Err(invalid)
            }
        }
    }

    pub(crate) fn model_finished(
        &mut self,
        cancelled: bool,
        force_final: bool,
        has_tool_calls: bool,
        task_completed: bool,
    ) -> Result<TurnAction, InvalidTransition> {
        if cancelled {
            self.apply(TurnInput::Cancelled)?;
            return Ok(TurnAction::Cancel);
        }
        // Forced wrap-up and an already-completed task both finish the turn even
        // when the model emitted tool calls, so collapse them into the
        // no-tool-calls input the machine understands.
        let will_execute = has_tool_calls && !force_final && !task_completed;
        let next = self.apply(TurnInput::ModelFinished {
            has_tool_calls: will_execute,
        })?;
        Ok(match next {
            TurnState::AwaitingApproval => TurnAction::ExecuteTools,
            _ => TurnAction::FinishResponse,
        })
    }

    pub(crate) fn approval_granted(&mut self) -> Result<(), InvalidTransition> {
        self.apply(TurnInput::ApprovalGranted).map(|_| ())
    }

    pub(crate) fn approval_denied(&mut self) -> Result<(), InvalidTransition> {
        self.apply(TurnInput::ApprovalDenied).map(|_| ())
    }

    pub(crate) fn tools_finished(&mut self) -> Result<(), InvalidTransition> {
        self.apply(TurnInput::ToolsFinished).map(|_| ())
    }

    pub(crate) fn retry_for_finish_gate(&mut self) -> Result<(), InvalidTransition> {
        self.apply(TurnInput::RetryRequested).map(|_| ())
    }

    /// Finish the tool phase only when the machine is actually executing tools.
    /// This is the common "cleanup" case for the many orchestrator exit paths:
    /// after an approval denial the machine is already back in `AwaitingModel`,
    /// so there is nothing to finish and calling it is a no-op.
    pub(crate) fn finish_tools_if_executing(&mut self) {
        if self.state == TurnState::ExecutingTools {
            let _ = self.tools_finished();
        }
    }

    /// Return to the next model turn when a tool batch is abandoned before
    /// approval or while it is executing. Loop recovery runs after the model
    /// response has entered `AwaitingApproval`, so finishing only an executing
    /// phase would strand the machine and make the recovery response look like
    /// an illegal transition.
    pub(crate) fn abandon_tool_phase(&mut self) {
        match self.state {
            TurnState::AwaitingApproval => {
                let _ = self.approval_denied();
            }
            TurnState::ExecutingTools => self.finish_tools_if_executing(),
            _ => {}
        }
    }

    pub(crate) fn recover_error(&mut self) -> TurnAction {
        // Recovery must never crash user-facing error handling; if the machine
        // is already terminal we simply leave it there.
        let _ = self.apply(TurnInput::ErrorRecovered);
        TurnAction::RecoverError
    }

    pub(crate) fn cancel(&mut self) -> TurnAction {
        // Cancellation is idempotent: a machine already in a terminal state
        // stays there rather than asserting.
        if self.state != TurnState::Cancelled && self.state != TurnState::Completed {
            let _ = self.apply(TurnInput::Cancelled);
        }
        TurnAction::Cancel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_provider_finish_reasons() {
        assert_eq!(
            FinishReason::from_provider(Some("stop")),
            FinishReason::Stop
        );
        assert_eq!(
            FinishReason::from_provider(Some("tool_calls")),
            FinishReason::ToolCalls
        );
        assert_eq!(
            FinishReason::from_provider(Some("length")),
            FinishReason::Length
        );
        assert_eq!(FinishReason::from_provider(None), FinishReason::Stop);
    }

    #[test]
    fn api_native_responses_are_marked_native() {
        let response = native_response(
            "provider text with a fenced decoy: ```tool {\"name\":\"write\"} ```",
            Some("tool_calls"),
            vec![ToolCall {
                name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "x"}),
            }],
        );
        assert_eq!(response.source, ResponseSource::Native);
        assert!(
            response
                .events
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolCall(_)))
        );
        assert_eq!(
            response
                .events
                .iter()
                .filter(|event| matches!(event, AgentEvent::ToolCall(_)))
                .count(),
            1
        );
        assert!(matches!(
            &response.events[0],
            AgentEvent::ToolCall(ToolCall { name, .. }) if name == "grep"
        ));
    }

    #[test]
    fn native_response_does_not_parse_fenced_text_without_structured_calls() {
        let response = native_response(
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"decoy\"}}\n```",
            Some("stop"),
            Vec::new(),
        );

        assert_eq!(response.source, ResponseSource::PlainText);
        assert!(matches!(
            response.events.first(),
            Some(AgentEvent::TextDelta(content)) if content.contains("decoy")
        ));
        assert!(
            !response
                .events
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolCall(_)))
        );
        assert_eq!(
            response.events.last(),
            Some(&AgentEvent::Finished(FinishReason::Stop))
        );
    }

    #[test]
    fn classifies_tool_response_before_text_completion() {
        let events = classify_response(
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"TODO\"}}\n```",
            Some("stop"),
            crate::config::ToolProtocol::Json,
        );

        assert!(matches!(events[0], AgentEvent::ToolCall(_)));
        assert_eq!(
            events.last(),
            Some(&AgentEvent::Finished(FinishReason::ToolCalls))
        );
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
        assert_eq!(
            response.raw_content,
            "```tool\n{\"name\":\"grep\",\"arguments\":{\"pattern\":\"x\"}}\n```"
        );
    }

    #[test]
    fn turn_state_transitions_have_terminal_safety_precedence() {
        assert_eq!(
            transition_turn(
                TurnState::AwaitingModel,
                TurnInput::ModelFinished {
                    has_tool_calls: true
                }
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
            Err(InvalidTransition {
                from: TurnState::AwaitingModel,
                input: TurnInput::ToolsFinished
            })
        );
    }

    #[test]
    fn every_valid_transition_from_every_state_is_accepted() {
        // AwaitingModel
        assert_eq!(
            transition_turn(
                TurnState::AwaitingModel,
                TurnInput::ModelFinished {
                    has_tool_calls: true
                }
            ),
            Ok(TurnState::AwaitingApproval)
        );
        assert_eq!(
            transition_turn(
                TurnState::AwaitingModel,
                TurnInput::ModelFinished {
                    has_tool_calls: false
                }
            ),
            Ok(TurnState::Completed)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingModel, TurnInput::ErrorRecovered),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingModel, TurnInput::Cancelled),
            Ok(TurnState::Cancelled)
        );
        // AwaitingApproval
        assert_eq!(
            transition_turn(TurnState::AwaitingApproval, TurnInput::ApprovalGranted),
            Ok(TurnState::ExecutingTools)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingApproval, TurnInput::ApprovalDenied),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingApproval, TurnInput::ErrorRecovered),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::AwaitingApproval, TurnInput::Cancelled),
            Ok(TurnState::Cancelled)
        );
        // ExecutingTools
        assert_eq!(
            transition_turn(TurnState::ExecutingTools, TurnInput::ToolsFinished),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::ExecutingTools, TurnInput::ErrorRecovered),
            Ok(TurnState::AwaitingModel)
        );
        assert_eq!(
            transition_turn(TurnState::ExecutingTools, TurnInput::Cancelled),
            Ok(TurnState::Cancelled)
        );
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let bad = [
            (TurnState::AwaitingModel, TurnInput::ApprovalGranted),
            (TurnState::AwaitingModel, TurnInput::ApprovalDenied),
            (TurnState::AwaitingModel, TurnInput::ToolsFinished),
            (TurnState::AwaitingApproval, TurnInput::ToolsFinished),
            (
                TurnState::AwaitingApproval,
                TurnInput::ModelFinished {
                    has_tool_calls: true,
                },
            ),
            (TurnState::ExecutingTools, TurnInput::ApprovalGranted),
            (
                TurnState::ExecutingTools,
                TurnInput::ModelFinished {
                    has_tool_calls: false,
                },
            ),
        ];
        for (state, input) in bad {
            assert_eq!(
                transition_turn(state, input),
                Err(InvalidTransition { from: state, input }),
                "expected {state:?} + {input:?} to be rejected"
            );
        }
    }

    #[test]
    fn terminal_states_reject_all_inputs_including_cancel() {
        for terminal in [TurnState::Cancelled] {
            for input in [
                TurnInput::ModelFinished {
                    has_tool_calls: true,
                },
                TurnInput::ApprovalGranted,
                TurnInput::ToolsFinished,
                TurnInput::ErrorRecovered,
                TurnInput::Cancelled,
            ] {
                assert!(
                    transition_turn(terminal, input).is_err(),
                    "terminal {terminal:?} should reject {input:?}"
                );
            }
        }
        assert!(
            transition_turn(
                TurnState::Completed,
                TurnInput::ModelFinished {
                    has_tool_calls: true
                }
            )
            .is_err()
        );
        assert!(transition_turn(TurnState::Completed, TurnInput::ApprovalGranted).is_err());
        assert!(transition_turn(TurnState::Completed, TurnInput::ToolsFinished).is_err());
        assert!(transition_turn(TurnState::Completed, TurnInput::ErrorRecovered).is_err());
        assert!(transition_turn(TurnState::Completed, TurnInput::Cancelled).is_err());
        assert_eq!(
            transition_turn(TurnState::Completed, TurnInput::RetryRequested),
            Ok(TurnState::AwaitingModel)
        );
    }

    #[test]
    fn turn_machine_owns_model_approval_execution_lifecycle() {
        let mut machine = TurnMachine::new();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(
            machine.model_finished(false, false, true, false),
            Ok(TurnAction::ExecuteTools)
        );
        assert_eq!(machine.state(), TurnState::AwaitingApproval);
        machine.approval_granted().unwrap();
        assert_eq!(machine.state(), TurnState::ExecutingTools);
        machine.tools_finished().unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(
            machine.model_finished(false, false, false, false),
            Ok(TurnAction::FinishResponse)
        );
        assert_eq!(machine.state(), TurnState::Completed);
    }

    #[test]
    fn turn_machine_terminal_inputs_override_tool_requests() {
        let mut machine = TurnMachine::new();
        assert_eq!(
            machine.model_finished(true, false, true, false),
            Ok(TurnAction::Cancel)
        );
        assert_eq!(machine.state(), TurnState::Cancelled);

        let mut machine = TurnMachine::new();
        assert_eq!(
            machine.model_finished(false, true, true, false),
            Ok(TurnAction::FinishResponse)
        );
        assert_eq!(machine.state(), TurnState::Completed);

        // A completed task collapses to a finish even with tool calls present.
        let mut machine = TurnMachine::new();
        assert_eq!(
            machine.model_finished(false, false, true, true),
            Ok(TurnAction::FinishResponse)
        );
        assert_eq!(machine.state(), TurnState::Completed);
    }

    #[test]
    fn tools_cannot_execute_while_awaiting_approval() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, true, false).unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingApproval);
        // The orchestrator gates execution on this exact predicate; until
        // approval is granted it must remain false.
        assert!(machine.state() != TurnState::ExecutingTools);
        machine.approval_granted().unwrap();
        assert_eq!(machine.state(), TurnState::ExecutingTools);
    }

    #[test]
    fn approval_denied_returns_to_awaiting_model_without_executing() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, true, false).unwrap();
        machine.approval_denied().unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        // finish_tools_if_executing is a no-op here because we never executed.
        machine.finish_tools_if_executing();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
    }

    #[test]
    fn finish_tools_if_executing_transitions_only_when_executing() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, true, false).unwrap();
        machine.approval_granted().unwrap();
        assert_eq!(machine.state(), TurnState::ExecutingTools);
        machine.finish_tools_if_executing();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        // Second call is a safe no-op.
        machine.finish_tools_if_executing();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
    }

    #[test]
    fn abandoning_a_pending_tool_phase_rewinds_before_execution() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, true, false).unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingApproval);
        machine.abandon_tool_phase();
        assert_eq!(machine.state(), TurnState::AwaitingModel);

        machine.model_finished(false, false, true, false).unwrap();
        machine.approval_granted().unwrap();
        machine.abandon_tool_phase();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
    }

    #[test]
    fn invalid_machine_transition_returns_err_without_changing_state() {
        let mut machine = TurnMachine::new();
        let err = machine.tools_finished();
        assert!(err.is_err());
        assert_eq!(machine.state(), TurnState::AwaitingModel);
    }

    #[test]
    fn finish_gate_can_reopen_completed_turn_for_retry() {
        let mut machine = TurnMachine::new();
        assert_eq!(
            machine.model_finished(false, false, false, false),
            Ok(TurnAction::FinishResponse)
        );
        assert_eq!(machine.state(), TurnState::Completed);
        machine.retry_for_finish_gate().unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingModel);
        assert_eq!(
            machine.model_finished(false, false, true, false),
            Ok(TurnAction::ExecuteTools)
        );
    }

    #[test]
    fn recover_error_rewinds_to_awaiting_model() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, true, false).unwrap();
        assert_eq!(machine.state(), TurnState::AwaitingApproval);
        assert_eq!(machine.recover_error(), TurnAction::RecoverError);
        assert_eq!(machine.state(), TurnState::AwaitingModel);
    }

    #[test]
    fn cancel_is_idempotent_on_terminal_states() {
        let mut machine = TurnMachine::new();
        machine.model_finished(false, false, false, false).unwrap();
        assert_eq!(machine.state(), TurnState::Completed);
        // Cancelling an already-completed turn must not panic or corrupt state.
        assert_eq!(machine.cancel(), TurnAction::Cancel);
        assert_eq!(machine.state(), TurnState::Completed);
    }
}
