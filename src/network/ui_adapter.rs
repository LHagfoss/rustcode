use super::events::{AgentEvent, FinishReason, ToolResult, ToolResultMetadata};
use super::policy::TurnPolicy;
use crate::app::{AppState, ChatMessage};
use crate::tools::ToolCall;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentUiEvent {
    PromptStarted { prompt: String },
    SubagentUpdated {
        id: u32,
        status: crate::app::SubAgentStatus,
        active_turn: bool,
    },
    TextDelta { text: String },
    ToolStarted { name: String, id: String },
    ApprovalRequested { calls: Vec<ToolCall> },
    ToolFinished { id: String, result: ToolResult },
    TurnRecovered { message: String },
    TurnFinished { content: String, completed: bool },
    Cancelled { completed_tool_ids: Vec<String> },
    Error { message: String, retryable: bool },
}

#[derive(Clone)]
pub(crate) struct AgentUiEventSender {
    sender: mpsc::UnboundedSender<AgentUiEvent>,
}

pub(crate) type AgentUiEventReceiver = mpsc::UnboundedReceiver<AgentUiEvent>;

impl AgentUiEventSender {
    pub(crate) fn channel() -> (Self, AgentUiEventReceiver) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub(crate) fn send(&self, event: AgentUiEvent) {
        let _ = self.sender.send(event);
    }
}

pub(crate) fn map_agent_event(event: AgentEvent) -> Option<AgentUiEvent> {
    match event {
        AgentEvent::TextDelta(text) => Some(AgentUiEvent::TextDelta { text }),
        AgentEvent::ToolCall(call) => Some(AgentUiEvent::ToolStarted {
            id: call
                .call_id
                .clone()
                .unwrap_or_else(|| format!("local:{}", call.name)),
            name: call.name,
        }),
        AgentEvent::ToolResult(result) => Some(AgentUiEvent::ToolFinished {
            id: result
                .metadata
                .call_id
                .clone()
                .unwrap_or_else(|| format!("local_{}", result.metadata.arguments_hash)),
            result,
        }),
        AgentEvent::Finished(FinishReason::Stop | FinishReason::Length) => {
            Some(AgentUiEvent::TurnFinished {
                content: String::new(),
                completed: true,
            })
        }
        AgentEvent::Finished(FinishReason::ToolCalls | FinishReason::Unknown(_)) => None,
        AgentEvent::Finished(FinishReason::Cancelled) | AgentEvent::Cancelled => {
            Some(AgentUiEvent::Cancelled {
                completed_tool_ids: Vec::new(),
            })
        }
        AgentEvent::Finished(FinishReason::Error(message)) | AgentEvent::Error(message) => {
            Some(AgentUiEvent::Error {
                message,
                retryable: false,
            })
        }
        AgentEvent::ContextLimit => Some(AgentUiEvent::Error {
            message: "context limit reached".to_owned(),
            retryable: true,
        }),
    }
}

fn history_tool_result_event(message: &ChatMessage) -> Option<AgentUiEvent> {
    let record = message.tool_result.as_ref()?;
    let id = message
        .tool_call_id
        .clone()
        .unwrap_or_else(|| format!("local_{}", record.arguments_hash));
    let result = ToolResult {
        tool_name: record.tool_name.clone(),
        content: message.content.clone(),
        diff: message.diff.clone(),
        file_preview: message.file_preview.clone(),
        metadata: ToolResultMetadata {
            call_id: Some(id.clone()),
            arguments_hash: record.arguments_hash.clone(),
            success: record.success,
            exit_code: record.exit_code,
            changed_paths: record.changed_paths.clone(),
            truncated: record.truncated,
            full_output_artifact: record.full_output_artifact.clone(),
            replayed: record.replayed,
            error_kind: record.parsed_error_kind(),
            retryable: record.retryable,
        },
    };
    Some(AgentUiEvent::ToolFinished { id, result })
}

async fn publish_snapshot(
    state: &Arc<Mutex<AppState>>,
    sender: &AgentUiEventSender,
    previous_response: &mut Arc<String>,
    previous_history_len: &mut usize,
    started_tools: &mut HashSet<String>,
    finished_tools: &mut HashSet<String>,
    approval_sent: &mut bool,
    previous_subagents: &mut std::collections::HashMap<u32, (crate::app::SubAgentStatus, bool)>,
) {
    let (response, live_tools, pending_approval, protocol, history, history_len, subagents) = {
        let state = state.lock().await;
        (
            state.current_response.clone(),
            state.live_tool_calls.clone(),
            state.pending_tool_confirmation.is_some(),
            state.active_tool_protocol(),
            state
                .history
                .iter()
                .skip((*previous_history_len).min(state.history.len()))
                .cloned()
                .collect::<Vec<_>>(),
            state.history.len(),
            state
                .subagents
                .iter()
                .map(|agent| (agent.id, (agent.status, agent.active_turn)))
                .collect::<Vec<_>>(),
        )
    };

    for (id, snapshot) in subagents {
        if previous_subagents.get(&id) != Some(&snapshot) {
            sender.send(AgentUiEvent::SubagentUpdated {
                id,
                status: snapshot.0,
                active_turn: snapshot.1,
            });
            previous_subagents.insert(id, snapshot);
        }
    }

    if response.as_str() != previous_response.as_str() {
        let text = response
            .as_str()
            .strip_prefix(previous_response.as_str())
            .unwrap_or(response.as_str())
            .to_owned();
        if !text.is_empty() {
            sender.send(AgentUiEvent::TextDelta { text });
        }
        *previous_response = Arc::clone(&response);
    }

    for call in live_tools {
        if started_tools.insert(call.key.clone()) {
            sender.send(AgentUiEvent::ToolStarted {
                id: call.key,
                name: call.tool_name,
            });
        }
    }

    if pending_approval && !*approval_sent {
        let calls = {
            let state = state.lock().await;
            state
                .history
                .iter()
                .rev()
                .find(|message| message.role == "assistant")
                .map(|message| message.resolved_tool_calls(protocol))
                .filter(|calls| !calls.is_empty())
                .unwrap_or_else(|| crate::tools::parse_tool_calls(&response, protocol))
        };
        if !calls.is_empty() {
            sender.send(AgentUiEvent::ApprovalRequested { calls });
            *approval_sent = true;
        }
    } else if !pending_approval {
        *approval_sent = false;
    }

    for message in history {
        if let Some(event) = history_tool_result_event(&message)
            && let AgentUiEvent::ToolFinished { id, .. } = &event
            && finished_tools.insert(id.clone())
        {
            sender.send(event);
        }
    }
    *previous_history_len = history_len;
}

pub(crate) async fn run_agent_turn_with_events<P: TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<super::stream::StreamBuffer>>,
    prompt: String,
    sender: AgentUiEventSender,
) -> super::TurnContext {
    sender.send(AgentUiEvent::PromptStarted { prompt });
    let mut turn = Box::pin(super::turn_engine::run_agent_turn(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
    ));
    let mut previous_response = Arc::new(String::new());
    let mut previous_history_len = 0;
    let mut started_tools = HashSet::new();
    let mut finished_tools = HashSet::new();
    let mut approval_sent = false;
    let mut previous_subagents = std::collections::HashMap::new();

    let context = loop {
        tokio::select! {
            context = &mut turn => break context,
            _ = tokio::time::sleep(Duration::from_millis(16)) => {
                publish_snapshot(
                    state,
                    &sender,
                    &mut previous_response,
                    &mut previous_history_len,
                    &mut started_tools,
                    &mut finished_tools,
                    &mut approval_sent,
                    &mut previous_subagents,
                ).await;
            }
        }
    };

    publish_snapshot(
        state,
        &sender,
        &mut previous_response,
        &mut previous_history_len,
        &mut started_tools,
        &mut finished_tools,
        &mut approval_sent,
        &mut previous_subagents,
    )
    .await;

    if cancel_token.is_cancelled() {
        sender.send(AgentUiEvent::Cancelled {
            completed_tool_ids: Vec::new(),
        });
    } else {
        sender.send(AgentUiEvent::TurnFinished {
            content: context.final_content.clone(),
            completed: context.task_completed,
        });
    }
    context
}

#[cfg(test)]
mod tests {
    use super::{AgentUiEvent, AgentUiEventSender, map_agent_event};
    use crate::network::events::{AgentEvent, FinishReason, ToolResult, ToolResultMetadata};
    use crate::tools::ToolCall;
    use serde_json::json;

    #[test]
    fn maps_text_tool_completion_cancellation_and_errors() {
        assert!(matches!(
            map_agent_event(AgentEvent::TextDelta("hello".to_owned())),
            Some(AgentUiEvent::TextDelta { text }) if text == "hello"
        ));

        let call = ToolCall {
            name: "view_file".to_owned(),
            arguments: json!({"path": "src/main.rs"}),
            call_id: Some("call-1".to_owned()),
        };
        assert!(matches!(
            map_agent_event(AgentEvent::ToolCall(call)),
            Some(AgentUiEvent::ToolStarted { id, name }) if id == "call-1" && name == "view_file"
        ));

        let result = ToolResult {
            tool_name: "view_file".to_owned(),
            content: "ok".to_owned(),
            diff: None,
            file_preview: None,
            metadata: ToolResultMetadata {
                call_id: Some("call-1".to_owned()),
                success: true,
                ..ToolResultMetadata::default()
            },
        };
        assert!(matches!(
            map_agent_event(AgentEvent::ToolResult(result)),
            Some(AgentUiEvent::ToolFinished { id, .. }) if id == "call-1"
        ));
        assert!(matches!(
            map_agent_event(AgentEvent::Finished(FinishReason::Stop)),
            Some(AgentUiEvent::TurnFinished {
                completed: true,
                ..
            })
        ));
        assert!(matches!(
            map_agent_event(AgentEvent::Cancelled),
            Some(AgentUiEvent::Cancelled { .. })
        ));
        assert!(matches!(
            map_agent_event(AgentEvent::Error("retry".to_owned())),
            Some(AgentUiEvent::Error { message, retryable: false }) if message == "retry"
        ));
    }

    #[tokio::test]
    async fn sender_round_trips_approval_and_recovery_events() {
        let (sender, mut receiver) = AgentUiEventSender::channel();
        sender.send(AgentUiEvent::SubagentUpdated {
            id: 7,
            status: crate::app::SubAgentStatus::Running,
            active_turn: true,
        });
        sender.send(AgentUiEvent::ApprovalRequested { calls: Vec::new() });
        sender.send(AgentUiEvent::TurnRecovered {
            message: "retrying".to_owned(),
        });

        assert!(matches!(
            receiver.recv().await,
            Some(AgentUiEvent::SubagentUpdated {
                id: 7,
                status: crate::app::SubAgentStatus::Running,
                active_turn: true
            })
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(AgentUiEvent::ApprovalRequested { calls }) if calls.is_empty()
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(AgentUiEvent::TurnRecovered { message }) if message == "retrying"
        ));
    }
}
