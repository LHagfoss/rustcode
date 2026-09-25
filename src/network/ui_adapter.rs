#[cfg(test)]
use super::events::{AgentEvent, FinishReason};
use super::events::{ToolResult, ToolResultMetadata};
use super::policy::TurnPolicy;
use crate::app::{AppState, ChatMessage};
use crate::tools::{ToolCall, resolve_tool_calls};
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AgentUiEvent {
    PromptStarted {
        prompt: String,
    },
    SubagentUpdated {
        id: u32,
        status: crate::app::SubAgentStatus,
        active_turn: bool,
    },
    TextDelta {
        text: String,
    },
    ToolStarted {
        name: String,
        id: String,
        detail: Option<String>,
    },
    ApprovalRequested {
        calls: Vec<ToolCall>,
    },
    QuestionRequested {
        prompt: crate::controller::QuestionPrompt,
    },
    ToolFinished {
        id: String,
        result: ToolResult,
    },
    #[cfg(test)]
    TurnRecovered {
        message: String,
    },
    TurnFinished {
        content: String,
        completed: bool,
    },
    Cancelled {
        completed_tool_ids: Vec<String>,
    },
    #[cfg(test)]
    Error {
        message: String,
        retryable: bool,
    },
}

#[derive(Clone)]
pub(crate) struct AgentUiEventSender {
    sender: mpsc::UnboundedSender<AgentUiEvent>,
}

pub(crate) type AgentUiEventReceiver = mpsc::UnboundedReceiver<AgentUiEvent>;

#[derive(Default)]
struct ResponseDeltaTracker {
    /// Address is used only as an identity check; it is never dereferenced.
    pointer: usize,
    len: usize,
    revision: u64,
    /// Latest response text already sent to the UI. Unlike the live response
    /// projection, this survives the turn's final cleanup so a terminal flush
    /// can tell whether it still needs to deliver the completed response.
    emitted_content_len: usize,
    emitted_content_hash: DefaultHasher,
}

impl AgentUiEventSender {
    pub(crate) fn channel() -> (Self, AgentUiEventReceiver) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    pub(crate) fn send(&self, event: AgentUiEvent) {
        let _ = self.sender.send(event);
    }
}

pub(crate) fn tool_display_detail(name: &str, arguments: &serde_json::Value) -> Option<String> {
    let (_, target) = crate::app::activity::summarize_tool_call(name, arguments);
    (!target.is_empty() && target != "?" && target != name).then_some(target)
}

#[cfg(test)]
pub(crate) fn map_agent_event(event: AgentEvent) -> Option<AgentUiEvent> {
    match event {
        AgentEvent::TextDelta(text) => Some(AgentUiEvent::TextDelta { text }),
        AgentEvent::ToolCall(call) => Some(AgentUiEvent::ToolStarted {
            detail: tool_display_detail(&call.name, &call.arguments),
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

fn history_tool_result_event(
    message: &ChatMessage,
    suppress_synthetic_background_completion: bool,
) -> Option<AgentUiEvent> {
    let record = message.tool_result.as_ref()?;
    // ACP's server-owned task sink emits terminal background updates directly
    // and persists this synthetic evidence for the model. Replaying the
    // synthetic history row on every continuation would send a duplicate
    // terminal update for the original provider call ID.
    if suppress_synthetic_background_completion && record.tool_name == "background_task" {
        return None;
    }
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
            pending: record.pending,
            command: record.command.clone(),
            call_id: Some(id.clone()),
            arguments_hash: record.arguments_hash.clone(),
            success: record.success,
            exit_code: record.exit_code,
            changed_paths: record.changed_paths.clone(),
            truncated: record.truncated,
            payload_truncated: record.payload_truncated,
            completeness: record.resolved_completeness(),
            full_output_artifact: record.full_output_artifact.clone(),
            replayed: record.replayed,
            error_kind: record.parsed_error_kind(),
            retryable: record.retryable,
            inspection: record.inspection.clone(),
            command_status: record.command_status.clone(),
        },
    };
    Some(AgentUiEvent::ToolFinished { id, result })
}

#[cfg(test)]
async fn publish_snapshot(
    state: &Arc<Mutex<AppState>>,
    sender: &AgentUiEventSender,
    previous_response: &mut ResponseDeltaTracker,
    previous_history_len: &mut usize,
    started_tools: &mut HashSet<String>,
    finished_tools: &mut HashSet<String>,
    approval_sent: &mut bool,
    previous_question: &mut Option<crate::controller::QuestionPrompt>,
    previous_subagents: &mut std::collections::HashMap<u32, (crate::app::SubAgentStatus, bool)>,
) {
    publish_snapshot_with_mode(
        state,
        sender,
        previous_response,
        previous_history_len,
        started_tools,
        finished_tools,
        approval_sent,
        previous_question,
        previous_subagents,
        false,
    )
    .await;
}

async fn publish_snapshot_with_mode(
    state: &Arc<Mutex<AppState>>,
    sender: &AgentUiEventSender,
    previous_response: &mut ResponseDeltaTracker,
    previous_history_len: &mut usize,
    started_tools: &mut HashSet<String>,
    finished_tools: &mut HashSet<String>,
    approval_sent: &mut bool,
    previous_question: &mut Option<crate::controller::QuestionPrompt>,
    previous_subagents: &mut std::collections::HashMap<u32, (crate::app::SubAgentStatus, bool)>,
    suppress_synthetic_background_completion: bool,
) {
    let (
        response,
        response_revision,
        response_last_rewrite_revision,
        live_tools,
        pending_approval,
        pending_question,
        protocol,
        history,
        history_len,
        subagents,
    ) = {
        let state = state.lock().await;
        (
            state.current_response.clone(),
            state.current_response_revision,
            state.current_response_last_rewrite_revision,
            Arc::clone(&state.live_tool_calls),
            state.pending_tool_confirmation.is_some(),
            state
                .pending_question
                .as_ref()
                .map(|question| crate::controller::QuestionPrompt {
                    header: question.header.clone(),
                    text: question.question.clone(),
                    options: question.options.clone(),
                    descriptions: question.descriptions.clone(),
                    multiple: question.is_multi_select,
                }),
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

    let response_pointer = Arc::as_ptr(&response) as usize;
    let extends_previous_response = response_revision != previous_response.revision
        && response_last_rewrite_revision <= previous_response.revision
        && response.len() >= previous_response.len;
    let text = if extends_previous_response {
        response[previous_response.len..].to_owned()
    } else if response_pointer != previous_response.pointer
        || response.len() != previous_response.len
        || response_revision != previous_response.revision
    {
        response.as_str().to_owned()
    } else {
        String::new()
    };
    if !text.is_empty() {
        let text_len = text.len();
        if extends_previous_response {
            previous_response.emitted_content_len = previous_response
                .emitted_content_len
                .saturating_add(text_len);
            previous_response
                .emitted_content_hash
                .write(text.as_bytes());
        } else {
            previous_response.emitted_content_len = response.len();
            previous_response.emitted_content_hash = DefaultHasher::new();
            previous_response
                .emitted_content_hash
                .write(response.as_bytes());
        }
        sender.send(AgentUiEvent::TextDelta { text });
    }
    previous_response.pointer = response_pointer;
    previous_response.len = response.len();
    previous_response.revision = response_revision;

    for call in live_tools.iter() {
        // Live keys identify presentation instances, not protocol calls. A
        // provider-less speculative call gets its ID from completed arguments
        // in assistant history below, once the call is authoritative.
        if call.execution_started
            && let Some(id) = call.provider_call_id.as_ref()
            && started_tools.insert(id.clone())
        {
            sender.send(AgentUiEvent::ToolStarted {
                id: id.clone(),
                name: call.tool_name.clone(),
                detail: (!call.target.is_empty() && call.target != "?")
                    .then(|| call.target.clone()),
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
                .map(|message| resolve_tool_calls(message, protocol))
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

    if *previous_question != pending_question {
        if let Some(prompt) = pending_question.as_ref() {
            sender.send(AgentUiEvent::QuestionRequested {
                prompt: prompt.clone(),
            });
        }
        *previous_question = pending_question;
    }

    for message in history {
        if message.role == "assistant" {
            for call in resolve_tool_calls(&message, protocol) {
                let id = call.call_id.clone().unwrap_or_else(|| {
                    format!(
                        "local_{}",
                        super::tool_exec::stable_arguments_hash(&call.arguments)
                    )
                });
                if started_tools.insert(id.clone()) {
                    sender.send(AgentUiEvent::ToolStarted {
                        id,
                        detail: tool_display_detail(&call.name, &call.arguments),
                        name: call.name,
                    });
                }
            }
        }
        if let Some(event) =
            history_tool_result_event(&message, suppress_synthetic_background_completion)
            && let AgentUiEvent::ToolFinished { id, result } = &event
            && finished_tools.insert(id.clone())
        {
            // A fast call can finish entirely between snapshot ticks.
            if started_tools.insert(id.clone()) {
                sender.send(AgentUiEvent::ToolStarted {
                    id: id.clone(),
                    name: result.tool_name.clone(),
                    detail: result.metadata.command.as_ref().and_then(|command| {
                        tool_display_detail("run_command", &serde_json::json!({"command": command}))
                    }),
                });
            }
            sender.send(event);
        }
    }
    *previous_history_len = history_len;
}

pub(crate) async fn run_agent_turn_with_events_and_context_for_session<P: TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<super::stream::StreamBuffer>>,
    prompt: String,
    sender: AgentUiEventSender,
    context: super::TurnContext,
    turn_session_id: String,
) -> super::TurnContext {
    run_agent_turn_with_events_and_context_mode(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        prompt,
        sender,
        context,
        turn_session_id,
        false,
    )
    .await
}

pub(crate) async fn run_agent_turn_with_events_for_acp<P: TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<super::stream::StreamBuffer>>,
    prompt: String,
    sender: AgentUiEventSender,
) -> super::TurnContext {
    let (max_tool_rounds, max_total_tool_rounds) = {
        let s = state.lock().await;
        (s.config.max_tool_rounds, s.config.max_total_tool_rounds)
    };
    // Keep this lock guard out of the awaited turn future. The turn locks
    // AppState while preparing each provider request, so an inline lock
    // expression here can retain the mutex for the whole async call.
    let turn_session_id = { state.lock().await.active_session_id.clone() };
    run_agent_turn_with_events_and_context_mode(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        prompt,
        sender,
        super::TurnContext::with_budgets(max_tool_rounds, max_total_tool_rounds),
        turn_session_id,
        true,
    )
    .await
}

pub(crate) async fn run_agent_turn_with_events_and_context_for_acp<P: TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<super::stream::StreamBuffer>>,
    prompt: String,
    sender: AgentUiEventSender,
    context: super::TurnContext,
) -> super::TurnContext {
    // Keep this lock guard out of the awaited turn future. The turn locks
    // AppState while preparing each provider request, so an inline lock
    // expression here can retain the mutex for the whole async call.
    let turn_session_id = { state.lock().await.active_session_id.clone() };
    run_agent_turn_with_events_and_context_mode(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        prompt,
        sender,
        context,
        turn_session_id,
        true,
    )
    .await
}

async fn run_agent_turn_with_events_and_context_mode<P: TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &CancellationToken,
    policy: &Arc<P>,
    stream_buffer: &Arc<Mutex<super::stream::StreamBuffer>>,
    prompt: String,
    sender: AgentUiEventSender,
    context: super::TurnContext,
    turn_session_id: String,
    suppress_synthetic_background_completion: bool,
) -> super::TurnContext {
    sender.send(AgentUiEvent::PromptStarted { prompt });
    let starting_history_len = state.lock().await.history.len();
    let mut turn = Box::pin(super::turn_engine::run_agent_turn_with_context_for_session(
        client,
        state,
        cancel_token,
        policy,
        stream_buffer,
        context,
        turn_session_id,
    ));
    let context = drive_turn_with_snapshots(
        turn.as_mut(),
        state,
        &sender,
        suppress_synthetic_background_completion,
        starting_history_len,
    )
    .await;

    if cancel_token.is_cancelled() {
        sender.send(AgentUiEvent::Cancelled {
            completed_tool_ids: Vec::new(),
        });
    } else {
        sender.send(AgentUiEvent::TurnFinished {
            content: context.response.final_content.clone(),
            completed: context.lifecycle.task_completed,
        });
    }
    context
}

/// Keep the turn and its UI projection advancing on the same task.
async fn drive_turn_with_snapshots<F: std::future::Future<Output = super::TurnContext>>(
    mut turn: std::pin::Pin<&mut F>,
    state: &Arc<Mutex<AppState>>,
    sender: &AgentUiEventSender,
    suppress_synthetic_background_completion: bool,
    starting_history_len: usize,
) -> F::Output {
    let mut previous_response = ResponseDeltaTracker::default();
    let mut previous_history_len = starting_history_len;
    let mut started_tools = HashSet::new();
    let mut finished_tools = HashSet::new();
    let mut approval_sent = false;
    let mut previous_question = None;
    let mut previous_subagents = std::collections::HashMap::new();

    let context = loop {
        tokio::select! {
            context = &mut turn => break context,
            // Poll the snapshot alongside the turn. Awaiting it in the branch
            // handler would suspend the turn while the snapshot waits for a
            // mutex that Tokio may already have reserved for that same turn.
            _ = async {
                tokio::time::sleep(Duration::from_millis(16)).await;
                publish_snapshot_with_mode(
                    state,
                    sender,
                    &mut previous_response,
                    &mut previous_history_len,
                    &mut started_tools,
                    &mut finished_tools,
                    &mut approval_sent,
                    &mut previous_question,
                    &mut previous_subagents,
                    suppress_synthetic_background_completion,
                ).await;
            } => {}
        }
    };

    publish_snapshot_with_mode(
        state,
        sender,
        &mut previous_response,
        &mut previous_history_len,
        &mut started_tools,
        &mut finished_tools,
        &mut approval_sent,
        &mut previous_question,
        &mut previous_subagents,
        suppress_synthetic_background_completion,
    )
    .await;
    flush_final_response_delta(
        &context.response.final_content,
        &mut previous_response,
        sender,
    );

    context
}

fn flush_final_response_delta(
    final_content: &str,
    previous_response: &mut ResponseDeltaTracker,
    sender: &AgentUiEventSender,
) {
    let emitted_prefix_matches = final_content
        .get(..previous_response.emitted_content_len)
        .is_some_and(|prefix| {
            let mut hasher = DefaultHasher::new();
            hasher.write(prefix.as_bytes());
            hasher.finish() == previous_response.emitted_content_hash.finish()
        });
    let missing_content = if emitted_prefix_matches {
        &final_content[previous_response.emitted_content_len..]
    } else {
        final_content
    };
    if !missing_content.is_empty() {
        sender.send(AgentUiEvent::TextDelta {
            text: missing_content.to_owned(),
        });
    }
    previous_response.emitted_content_len = final_content.len();
    previous_response.emitted_content_hash = DefaultHasher::new();
    previous_response
        .emitted_content_hash
        .write(final_content.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{AgentUiEvent, AgentUiEventSender, map_agent_event, publish_snapshot};
    use crate::app::{AppState, ChatMessage};
    use crate::network::events::{AgentEvent, FinishReason, ToolResult, ToolResultMetadata};
    use crate::tools::ToolCall;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn immediate_turn_completion_flushes_final_text() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let mut context = crate::network::TurnContext::new();
        context.response.final_content = "mock reply".to_owned();
        let turn_state = Arc::clone(&state);
        let turn = async move {
            let mut state = turn_state.lock().await;
            state.replace_current_response("mock reply");
            state.clear_current_response();
            context
        };
        let mut turn = Box::pin(turn);
        let (sender, mut receiver) = AgentUiEventSender::channel();

        super::drive_turn_with_snapshots(turn.as_mut(), &state, &sender, false, 0).await;

        assert!(matches!(
            receiver.try_recv(),
            Ok(AgentUiEvent::TextDelta { text }) if text == "mock reply"
        ));
    }

    #[tokio::test]
    async fn terminal_text_flush_does_not_repeat_previously_emitted_content() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let mut context = crate::network::TurnContext::new();
        context.response.final_content = "mock reply".to_owned();
        let turn_state = Arc::clone(&state);
        let turn = async move {
            turn_state
                .lock()
                .await
                .replace_current_response("mock reply");
            tokio::time::sleep(std::time::Duration::from_millis(32)).await;
            turn_state.lock().await.clear_current_response();
            context
        };
        let mut turn = Box::pin(turn);
        let (sender, mut receiver) = AgentUiEventSender::channel();

        super::drive_turn_with_snapshots(turn.as_mut(), &state, &sender, false, 0).await;

        assert!(matches!(
            receiver.try_recv(),
            Ok(AgentUiEvent::TextDelta { text }) if text == "mock reply"
        ));
        assert!(
            receiver.try_recv().is_err(),
            "terminal flush must not duplicate text"
        );
    }

    // The controller can own AppState while the turn queues for its mutex.
    // Once the snapshot timer fires, both futures must continue being polled:
    // Tokio's fair mutex otherwise reserves the next lock for the frozen turn.
    async fn contended_turn_publishes_completion(cancel: bool) {
        let state = Arc::new(Mutex::new(AppState::new()));
        let held = Arc::clone(&state).lock_owned().await;
        let token = tokio_util::sync::CancellationToken::new();
        let turn = async {
            let mut state = state.lock().await;
            state.history.push(
                crate::app::ChatMessage::new("tool", "file contents")
                    .answering(Some("call-read".to_owned()))
                    .with_tool_result(crate::app::ToolResultRecord {
                        tool_name: "view_file".to_owned(),
                        success: true,
                        ..Default::default()
                    }),
            );
            drop(state);
            if cancel {
                token.cancelled().await;
            }
            crate::network::TurnContext::new()
        };
        tokio::pin!(turn);
        let (sender, mut receiver) = AgentUiEventSender::channel();
        let driver = super::drive_turn_with_snapshots(turn.as_mut(), &state, &sender, false, 0);
        tokio::pin!(driver);
        // Queue the turn first, then explicitly poll the expired snapshot timer
        // while the mutex is still held. Release only after both have waited.
        assert!(futures_util::poll!(driver.as_mut()).is_pending());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(futures_util::poll!(driver.as_mut()).is_pending());
        drop(held);
        if cancel {
            // Poll once after release so cancellation happens while the turn
            // is active, rather than before it can acquire the state mutex.
            assert!(futures_util::poll!(driver.as_mut()).is_pending());
            token.cancel();
        }
        let result = tokio::time::timeout(std::time::Duration::from_millis(500), driver.as_mut())
            .await
            .expect("snapshot publication must not suspend the turn's mutex waiter");
        assert!(result.response.final_content.is_empty());
        assert!(matches!(
            receiver.try_recv(),
            Ok(AgentUiEvent::ToolStarted { id, .. }) if id == "call-read"
        ));
        assert!(matches!(
            receiver.try_recv(),
            Ok(AgentUiEvent::ToolFinished { id, result })
                if id == "call-read" && result.content == "file contents"
        ));
        assert!(
            receiver.try_recv().is_err(),
            "completion must be published once"
        );
    }

    #[tokio::test]
    async fn snapshot_contention_does_not_freeze_completed_tools() {
        contended_turn_publishes_completion(false).await;
    }

    #[tokio::test]
    async fn snapshot_contention_does_not_block_turn_cancellation() {
        contended_turn_publishes_completion(true).await;
    }

    #[tokio::test]
    async fn tool_snapshot_uses_matching_ids_and_skips_prior_turn_history() {
        let mut app = AppState::new();
        let completed = |id: Option<&str>, hash: &str| {
            ChatMessage::new("tool", "done")
                .answering(id.map(str::to_owned))
                .with_tool_result(crate::app::ToolResultRecord {
                    tool_name: "view_file".to_owned(),
                    arguments_hash: hash.to_owned(),
                    success: true,
                    ..Default::default()
                })
        };
        app.history.push(completed(Some("old-call"), "old"));
        let mut history_len = app.history.len();
        app.begin_live_tool_call(Some("provider-call"), "view_file", &json!({"path":"a"}));
        // This speculative projection has no protocol identity yet.
        app.begin_live_tool_call(None, "view_file", &json!({"path":"b"}));
        let state = Arc::new(Mutex::new(app));
        let (sender, mut receiver) = AgentUiEventSender::channel();
        let mut response = super::ResponseDeltaTracker::default();
        let mut started = std::collections::HashSet::new();
        let mut finished = std::collections::HashSet::new();
        let mut approval = false;
        let mut question = None;
        let mut subagents = std::collections::HashMap::new();
        publish_snapshot(
            &state,
            &sender,
            &mut response,
            &mut history_len,
            &mut started,
            &mut finished,
            &mut approval,
            &mut question,
            &mut subagents,
        )
        .await;
        assert!(
            matches!(receiver.try_recv(), Ok(AgentUiEvent::ToolStarted { id, detail, .. }) if id == "provider-call" && detail.as_deref() == Some("a"))
        );
        assert!(receiver.try_recv().is_err());
        {
            let mut state = state.lock().await;
            state
                .history
                .push(completed(Some("provider-call"), "native"));
            state.history.push(completed(None, "synthetic"));
            state.history.push(completed(Some("fast-call"), "fast"));
        }
        publish_snapshot(
            &state,
            &sender,
            &mut response,
            &mut history_len,
            &mut started,
            &mut finished,
            &mut approval,
            &mut question,
            &mut subagents,
        )
        .await;
        assert!(
            matches!(receiver.try_recv(), Ok(AgentUiEvent::ToolFinished { id, .. }) if id == "provider-call")
        );
        for expected in ["local_synthetic", "fast-call"] {
            assert!(
                matches!(receiver.try_recv(), Ok(AgentUiEvent::ToolStarted { id, .. }) if id == expected)
            );
            assert!(
                matches!(receiver.try_recv(), Ok(AgentUiEvent::ToolFinished { id, .. }) if id == expected)
            );
        }
        assert!(receiver.try_recv().is_err());
        publish_snapshot(
            &state,
            &sender,
            &mut response,
            &mut history_len,
            &mut started,
            &mut finished,
            &mut approval,
            &mut question,
            &mut subagents,
        )
        .await;
        assert!(
            receiver.try_recv().is_err(),
            "completed calls must not replay"
        );
    }

    #[test]
    fn display_detail_reuses_bounded_semantic_tool_parameters() {
        assert_eq!(
            super::tool_display_detail("view_file", &json!({"path":"src/main.rs"})),
            Some("src/main.rs".into())
        );
        assert_eq!(
            super::tool_display_detail("grep_search", &json!({"pattern":"TODO", "path":"src"})),
            Some("TODO in src".into())
        );
        assert_eq!(
            super::tool_display_detail("run_command", &json!({"command":"cargo test\n--lib"})),
            Some("cargo test --lib".into())
        );
        assert_eq!(super::tool_display_detail("view_file", &json!({})), None);
    }

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
            Some(AgentUiEvent::ToolStarted { id, name, .. }) if id == "call-1" && name == "view_file"
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

    #[tokio::test]
    async fn snapshot_delta_tracker_does_not_retain_response_and_resets_on_rewrite() {
        let state = Arc::new(Mutex::new(AppState::new()));
        state
            .lock()
            .await
            .replace_current_response("initial response");
        let (sender, mut receiver) = AgentUiEventSender::channel();
        let mut previous_response = super::ResponseDeltaTracker::default();
        let mut previous_history_len = 0;
        let mut started_tools = std::collections::HashSet::new();
        let mut finished_tools = std::collections::HashSet::new();
        let mut approval_sent = false;
        let mut previous_question = None;
        let mut previous_subagents = std::collections::HashMap::new();

        publish_snapshot(
            &state,
            &sender,
            &mut previous_response,
            &mut previous_history_len,
            &mut started_tools,
            &mut finished_tools,
            &mut approval_sent,
            &mut previous_question,
            &mut previous_subagents,
        )
        .await;

        assert_eq!(
            receiver.recv().await,
            Some(AgentUiEvent::TextDelta {
                text: "initial response".to_owned()
            })
        );
        assert_eq!(
            Arc::strong_count(&state.lock().await.current_response),
            1,
            "snapshot tracking must not retain the response Arc"
        );

        state.lock().await.append_current_response(" + more");
        publish_snapshot(
            &state,
            &sender,
            &mut previous_response,
            &mut previous_history_len,
            &mut started_tools,
            &mut finished_tools,
            &mut approval_sent,
            &mut previous_question,
            &mut previous_subagents,
        )
        .await;

        assert_eq!(
            receiver.recv().await,
            Some(AgentUiEvent::TextDelta {
                text: " + more".to_owned()
            })
        );

        {
            let mut state = state.lock().await;
            state.clear_current_response();
            state.append_current_response("replacement response");
        }

        publish_snapshot(
            &state,
            &sender,
            &mut previous_response,
            &mut previous_history_len,
            &mut started_tools,
            &mut finished_tools,
            &mut approval_sent,
            &mut previous_question,
            &mut previous_subagents,
        )
        .await;

        assert_eq!(
            receiver.recv().await,
            Some(AgentUiEvent::TextDelta {
                text: "replacement response".to_owned()
            })
        );

        state
            .lock()
            .await
            .replace_current_response("final response");
        publish_snapshot(
            &state,
            &sender,
            &mut previous_response,
            &mut previous_history_len,
            &mut started_tools,
            &mut finished_tools,
            &mut approval_sent,
            &mut previous_question,
            &mut previous_subagents,
        )
        .await;

        assert_eq!(
            receiver.recv().await,
            Some(AgentUiEvent::TextDelta {
                text: "final response".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn pending_question_is_published_while_its_response_channel_waits() {
        let state = Arc::new(Mutex::new(AppState::new()));
        let (response, mut waiting) = tokio::sync::oneshot::channel();
        {
            let mut state = state.lock().await;
            state.pending_question = Some(
                crate::app::PendingQuestion::new(
                    "Choose a direction".to_owned(),
                    vec!["Left".to_owned(), "Right".to_owned()],
                    false,
                )
                .with_header("Direction".to_owned())
                .with_descriptions(vec!["Local".to_owned(), "Remote".to_owned()]),
            );
            state.question_response = Some(response);
            state.status = crate::app::AppStatus::AwaitingQuestion;
        }
        let (sender, mut receiver) = AgentUiEventSender::channel();
        let mut previous_response = super::ResponseDeltaTracker::default();
        let mut previous_history_len = 0;
        let mut started_tools = std::collections::HashSet::new();
        let mut finished_tools = std::collections::HashSet::new();
        let mut approval_sent = false;
        let mut previous_question = None;
        let mut previous_subagents = std::collections::HashMap::new();

        publish_snapshot(
            &state,
            &sender,
            &mut previous_response,
            &mut previous_history_len,
            &mut started_tools,
            &mut finished_tools,
            &mut approval_sent,
            &mut previous_question,
            &mut previous_subagents,
        )
        .await;

        assert_eq!(
            receiver.recv().await,
            Some(AgentUiEvent::QuestionRequested {
                prompt: crate::controller::QuestionPrompt {
                    header: "Direction".to_owned(),
                    text: "Choose a direction".to_owned(),
                    options: vec!["Left".to_owned(), "Right".to_owned()],
                    descriptions: vec!["Local".to_owned(), "Remote".to_owned()],
                    multiple: false,
                },
            })
        );
        assert!(
            waiting.try_recv().is_err(),
            "the turn must still be waiting"
        );
    }

    #[test]
    fn synthetic_background_completion_history_is_not_replayed_to_acp() {
        let message = crate::background_task_history_message_with_call_id(
            "task-fast",
            crate::tools::ToolExecutionOutput::success("done".to_owned()),
            Some("call-bg".to_owned()),
        );

        assert!(
            message.tool_result.is_some(),
            "completion must remain model history"
        );
        assert!(
            matches!(
                super::history_tool_result_event(&message, false),
                Some(AgentUiEvent::ToolFinished { id, result })
                    if id == "call-bg" && result.tool_name == "background_task"
            ),
            "non-ACP consumers must continue replaying background history"
        );
        assert!(
            super::history_tool_result_event(&message, true).is_none(),
            "the sink already emitted the terminal ACP update"
        );

        let ordinary = crate::app::ChatMessage::new("tool", "file contents")
            .answering(Some("call-read".to_owned()))
            .with_tool_result(crate::app::ToolResultRecord {
                tool_name: "read_file".to_owned(),
                success: true,
                ..Default::default()
            });
        assert!(super::history_tool_result_event(&ordinary, false).is_some());
        assert!(
            super::history_tool_result_event(&ordinary, true).is_some(),
            "ACP filtering must only affect synthetic background completion records"
        );
    }
}
