//! Bounded projections of a session for remote clients.
//!
//! The narrow public view of `AppState` a session owner publishes: the
//! snapshot, history pages, content chunks and per-event updates. Everything
//! here is a pure read with no disk or network access, so an owner can
//! project while it holds the state lock and send after releasing it. Every
//! projection fits [`MAX_REMOTE_FRAME_BYTES`]; what is cut is marked, never
//! silently dropped.

use crate::app::{AppState, PendingQuestion, SubAgent, SubAgentStatus};
use crate::controller::{AgentUiEvent, ApprovalAction};

use super::protocol::*;

/// Size limits for one projection. The defaults leave a snapshot well inside
/// one frame for ordinary sessions; `frame_bytes` is enforced regardless.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionLimits {
    /// Transcript messages in a snapshot, and the largest history page.
    pub transcript_items: usize,
    /// Bytes of one transcript message or tool result.
    pub text_bytes: usize,
    /// Bytes of the live response tail.
    pub live_response_bytes: usize,
    /// Bytes shared by the argument details of one approval batch.
    pub approval_bytes: usize,
    /// Bytes of a one-line label (tool detail, command, queued prompt).
    pub label_bytes: usize,
    /// Entries in each of the tool, subagent, prompt and task lists.
    pub list_items: usize,
    /// Bytes of one `get_content` chunk.
    pub chunk_bytes: usize,
    /// Encoded bytes of a whole snapshot or history page.
    pub frame_bytes: usize,
}

impl Default for ProjectionLimits {
    fn default() -> Self {
        Self {
            transcript_items: 40,
            text_bytes: 4 * 1024,
            live_response_bytes: 32 * 1024,
            approval_bytes: 32 * 1024,
            label_bytes: 512,
            list_items: 32,
            chunk_bytes: 64 * 1024,
            // Headroom for the frame envelope around the payload.
            frame_bytes: MAX_REMOTE_FRAME_BYTES - 4 * 1024,
        }
    }
}

/// What the owner knows that the session state does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionContext {
    pub registration_epoch: u64,
    /// Sequence of the last event applied to the projected state.
    pub sequence: u64,
    pub generation: u64,
    pub health: OwnerHealth,
    /// Stored session title, when the owner has one; the projection never
    /// reads the session store itself.
    pub title: Option<String>,
}

/// Cut `text` to at most `max_bytes` from its start, on a character boundary.
pub fn bound_text(text: &str, max_bytes: usize, content_id: Option<String>) -> BoundedText {
    let end = text.floor_char_boundary(max_bytes.min(text.len()));
    BoundedText {
        text: text[..end].to_owned(),
        truncated: end < text.len(),
        offset: 0,
        total_bytes: text.len() as u64,
        content_id,
    }
}

/// As [`bound_text`], keeping the end: the part of a live response a client
/// attaching mid-turn needs first.
fn bound_text_tail(text: &str, max_bytes: usize, content_id: Option<String>) -> BoundedText {
    let start = text.ceil_char_boundary(text.len().saturating_sub(max_bytes));
    BoundedText {
        text: text[start..].to_owned(),
        truncated: start > 0,
        offset: start as u64,
        total_bytes: text.len() as u64,
        content_id,
    }
}

fn label(text: &str, limits: &ProjectionLimits) -> String {
    bound_text(text, limits.label_bytes, None).text
}

fn history_revision(state: &AppState) -> String {
    format!("h{}", state.history.rewrite_revision())
}

fn history_cursor(revision: &str, before: usize) -> Option<String> {
    (before > 0).then(|| format!("{before}@{revision}"))
}

fn message_content_id(revision: &str, index: usize) -> String {
    format!("message:{revision}:{index}")
}

fn activity(state: &AppState) -> SessionActivity {
    if state.pending_tool_confirmation.is_some() {
        SessionActivity::AwaitingApproval
    } else if state.pending_question.is_some() {
        SessionActivity::AwaitingQuestion
    } else if state.has_active_turn() {
        SessionActivity::Running
    } else {
        SessionActivity::Idle
    }
}

/// The session-list row for this session.
pub fn project_session_info(state: &AppState, context: &ProjectionContext) -> RemoteSessionInfo {
    RemoteSessionInfo {
        session_id: state.active_session_id.clone(),
        registration_epoch: context.registration_epoch,
        title: context
            .title
            .clone()
            .unwrap_or_else(|| crate::config::session_title(&state.history)),
        workspace: state
            .task_working_directory
            .clone()
            .or_else(|| state.effective_workspace_root())
            .map(|path| path.display().to_string()),
        model: state.model_name.clone(),
        activity: activity(state),
        attention: RemoteAttention {
            approval: state.pending_tool_confirmation.is_some(),
            question: state.pending_question.is_some(),
        },
        health: context.health,
    }
}

fn project_question(state: &AppState, question: &PendingQuestion) -> RemoteQuestion {
    RemoteQuestion {
        question_id: question.id.clone(),
        header: question.header.clone(),
        text: question.question.clone(),
        options: question
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| RemoteQuestionOption {
                label: option.clone(),
                description: question.description(index).map(str::to_owned),
            })
            .collect(),
        multiple: question.is_multi_select,
        position: state.question_chain_position() as u32,
        chain_length: state.question_chain_len() as u32,
    }
}

fn project_approval_actions(
    batch_id: &str,
    actions: &[ApprovalAction],
    limits: &ProjectionLimits,
) -> RemoteApprovalBatch {
    // Every action is listed; the byte budget is shared between them.
    let detail_bytes = (limits.approval_bytes / actions.len().max(1)).max(256);
    RemoteApprovalBatch {
        batch_id: batch_id.to_owned(),
        actions: actions
            .iter()
            .enumerate()
            .map(|(index, action)| RemoteApprovalAction {
                tool_name: action.tool_name.clone(),
                summary: action.action_summary.clone(),
                risk: action.risk_context.clone(),
                details: bound_text(
                    &action.full_details,
                    detail_bytes,
                    Some(format!("approval:{batch_id}:{index}")),
                ),
            })
            .collect(),
    }
}

/// Pending approval actions with their complete arguments. Fails closed:
/// without a controller-owned batch ID there is nothing a client may resolve.
fn pending_approval_actions(state: &AppState) -> Option<(&str, Vec<ApprovalAction>)> {
    let batch_id = state.pending_approval_batch_id.as_deref()?;
    let confirmations = state
        .pending_tool_confirmation
        .as_ref()
        .filter(|confirmations| !confirmations.is_empty())?;
    let actions = confirmations
        .iter()
        .enumerate()
        .map(|(index, confirmation)| {
            let mut action = ApprovalAction::from_confirmation(confirmation, index);
            if let Some(full_details) = state
                .pending_approval_details
                .as_ref()
                .and_then(|details| details.get(index))
            {
                action.full_details = full_details.clone();
            }
            action
        })
        .collect();
    Some((batch_id, actions))
}

/// The question a client may answer now, with its identity.
pub(super) fn project_pending_question(state: &AppState) -> Option<RemoteQuestion> {
    state
        .pending_question
        .as_ref()
        .map(|question| project_question(state, question))
}

/// The approval batch a client may resolve now, with its identity.
pub(super) fn project_pending_approval(
    state: &AppState,
    limits: &ProjectionLimits,
) -> Option<RemoteApprovalBatch> {
    pending_approval_actions(state)
        .map(|(batch_id, actions)| project_approval_actions(batch_id, &actions, limits))
}

fn project_subagent(agent: &SubAgent, limits: &ProjectionLimits) -> RemoteSubagent {
    RemoteSubagent {
        id: agent.id,
        name: agent.name.clone(),
        task: bound_text(&agent.task, limits.label_bytes, None),
        model: agent.model.clone(),
        status: match agent.status {
            SubAgentStatus::Queued => RemoteSubagentStatus::Queued,
            SubAgentStatus::Running => RemoteSubagentStatus::Running,
            SubAgentStatus::Interrupted => RemoteSubagentStatus::Interrupted,
            SubAgentStatus::Completed => RemoteSubagentStatus::Completed,
            SubAgentStatus::Failed => RemoteSubagentStatus::Failed,
            SubAgentStatus::Cancelled => RemoteSubagentStatus::Cancelled,
        },
        active_turn: agent.active_turn,
        parent_id: agent.parent_id,
        depth: agent.depth,
        message_count: agent.history.len() as u32,
    }
}

fn project_tools(state: &AppState, limits: &ProjectionLimits) -> Vec<RemoteTool> {
    state
        .live_tool_calls
        .iter()
        .map(|call| RemoteTool {
            id: call
                .provider_call_id
                .clone()
                .unwrap_or_else(|| call.key.clone()),
            name: call.tool_name.clone(),
            detail: Some(call.target.as_str())
                .filter(|target| !target.is_empty())
                .map(|target| label(target, limits)),
            state: match (&call.finished, call.execution_started) {
                (Some(finish), _) if finish.success => RemoteToolState::Succeeded,
                (Some(_), _) => RemoteToolState::Failed,
                (None, true) => RemoteToolState::Running,
                (None, false) => RemoteToolState::Preparing,
            },
        })
        .collect()
}

fn project_messages(
    state: &AppState,
    revision: &str,
    range: std::ops::Range<usize>,
    limits: &ProjectionLimits,
) -> Vec<RemoteMessage> {
    let details = crate::controller::transcript_tool_details(state);
    state.history[range.clone()]
        .iter()
        .zip(range)
        .map(|(message, index)| RemoteMessage {
            message_id: format!("m{index}"),
            role: message.role.clone(),
            content: bound_text(
                &message.content,
                limits.text_bytes,
                Some(message_content_id(revision, index)),
            ),
            tool: message
                .tool_result
                .as_ref()
                .map(|record| RemoteMessageTool {
                    name: record.tool_name.clone(),
                    detail: details[index].clone(),
                    success: record.success,
                    pending: record.pending,
                }),
            timestamp: Some(message.timestamp.clone()).filter(|timestamp| !timestamp.is_empty()),
            response_time_ms: message.response_time_ms,
            thought_time_ms: message.thought_time_ms,
            completed_at: message.completed_at.clone(),
            turn: message.turn.as_ref().map(project_turn_timing),
        })
        .collect()
}

pub(super) fn project_turn_timing(turn: &crate::app::TurnTiming) -> RemoteTurnTiming {
    RemoteTurnTiming {
        turn_id: turn.turn_id.clone(),
        started_at: turn.started_at.clone(),
        ended_at: turn.ended_at.clone(),
        elapsed_work_ms: turn.elapsed_work_ms,
        outcome: turn.outcome.map(|outcome| match outcome {
            crate::app::TurnOutcome::Completed => RemoteTurnOutcome::Completed,
            crate::app::TurnOutcome::Cancelled => RemoteTurnOutcome::Cancelled,
            crate::app::TurnOutcome::Failed => RemoteTurnOutcome::Failed,
        }),
    }
}

pub(super) fn project_live_thought_time(state: &AppState) -> Option<u64> {
    (state.current_thought_started_at.is_some() || state.current_thought_time_ms > 0).then(|| {
        state.current_thought_time_ms.saturating_add(
            state.current_thought_started_at.map_or(0, |started| {
                started.elapsed().as_millis().min(u64::MAX as u128) as u64
            }),
        )
    })
}

pub(super) fn project_active_timing(state: &AppState) -> Option<RemoteTurnTiming> {
    state
        .measured_turn_timing()
        .as_ref()
        .map(project_turn_timing)
}

pub(super) fn project_ended_timing(state: &AppState, turn_id: &str) -> Option<RemoteTurnTiming> {
    state
        .active_turn_timing
        .as_ref()
        .filter(|turn| turn.turn_id == turn_id)
        .or_else(|| {
            state
                .history
                .iter()
                .rev()
                .filter_map(|message| message.turn.as_ref())
                .find(|turn| turn.turn_id == turn_id)
        })
        .map(project_turn_timing)
}

/// Keep at most `limit` entries, reporting how many were left out.
fn cap<T>(mut items: Vec<T>, limit: usize, omitted: &mut u32) -> Vec<T> {
    *omitted = items.len().saturating_sub(limit) as u32;
    items.truncate(limit);
    items
}

fn encoded_len<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

fn halve(text: &mut BoundedText, keep_tail: bool) -> bool {
    if text.text.is_empty() {
        return false;
    }
    let target = text.text.len() / 2;
    if keep_tail {
        let start = text.text.ceil_char_boundary(text.text.len() - target);
        text.offset += start as u64;
        text.text.drain(..start);
    } else {
        let end = text.text.floor_char_boundary(target);
        text.text.truncate(end);
    }
    text.truncated = true;
    true
}

/// Authoritative bounded state of the session at `context.sequence`.
pub fn project_snapshot(
    state: &AppState,
    context: &ProjectionContext,
    limits: &ProjectionLimits,
) -> RemoteSnapshot {
    let revision = history_revision(state);
    let mut omitted = RemoteOmitted::default();
    let first = state.history.len().saturating_sub(limits.transcript_items);
    let turn = state.active_turn_id.as_ref().map(|turn_id| RemoteTurn {
        turn_id: turn_id.clone(),
        live_response: bound_text_tail(
            &state.current_response,
            limits.live_response_bytes,
            Some(format!("response:{turn_id}")),
        ),
        tools: cap(
            project_tools(state, limits),
            limits.list_items,
            &mut omitted.tools,
        ),
        can_steer: state.can_accept_steer(),
        timing: project_active_timing(state),
        thought_time_ms: project_live_thought_time(state),
    });
    let pending_prompts = state
        .pending_steers
        .iter()
        .map(|steer| (RemotePendingPromptKind::Steer, steer.text.as_str()))
        .chain(
            state
                .pending_queue
                .iter()
                .filter(|prompt| !prompt.starts_with("__task_wakeup__:"))
                .map(|prompt| (RemotePendingPromptKind::Queue, prompt.as_str())),
        )
        .map(|(kind, text)| RemotePendingPrompt {
            kind,
            text: bound_text(text, limits.label_bytes, None),
        })
        .collect();
    let background_tasks = crate::controller::background_task_snapshots(&state.active_session_id)
        .into_iter()
        .map(|task| RemoteBackgroundTask {
            command: label(&task.command, limits),
            elapsed_ms: task.started_at.elapsed().as_millis() as u64,
            id: task.id,
        })
        .collect();
    let mut snapshot = RemoteSnapshot {
        session: project_session_info(state, context),
        sequence: context.sequence,
        generation: context.generation,
        turn,
        last_turn: state
            .history
            .iter()
            .rev()
            .filter_map(|message| message.turn.as_ref())
            .find(|turn| turn.outcome.is_some())
            .map(project_turn_timing),
        transcript: project_messages(state, &revision, first..state.history.len(), limits),
        history_cursor: history_cursor(&revision, first),
        history_revision: revision,
        pending_question: project_pending_question(state),
        pending_approval: project_pending_approval(state, limits),
        pending_prompts: cap(
            pending_prompts,
            limits.list_items,
            &mut omitted.pending_prompts,
        ),
        subagents: cap(
            state
                .subagents
                .iter()
                .map(|agent| project_subagent(agent, limits))
                .collect(),
            limits.list_items,
            &mut omitted.subagents,
        ),
        background_tasks: cap(
            background_tasks,
            limits.list_items,
            &mut omitted.background_tasks,
        ),
        omitted,
    };
    // The per-field limits bound ordinary sessions. JSON escaping can still
    // inflate text, so shed the least urgent content until the frame fits:
    // older transcript first, then the live response, then approval details
    // (which stay fetchable in full through their content IDs).
    let mut first = first;
    while encoded_len(&snapshot) > limits.frame_bytes {
        if !snapshot.transcript.is_empty() {
            let drop = snapshot.transcript.len().div_ceil(2);
            snapshot.transcript.drain(..drop);
            first += drop;
            snapshot.history_cursor = history_cursor(&snapshot.history_revision, first);
        } else if snapshot
            .turn
            .as_mut()
            .is_some_and(|turn| halve(&mut turn.live_response, true))
        {
        } else if let Some(action) = snapshot
            .pending_approval
            .as_mut()
            .and_then(|approval| {
                approval
                    .actions
                    .iter_mut()
                    .max_by_key(|action| action.details.text.len())
            })
            .filter(|action| !action.details.text.is_empty())
        {
            halve(&mut action.details, false);
        } else {
            break;
        }
    }
    snapshot
}

fn stale_cursor() -> RemoteError {
    RemoteError::new(
        RemoteErrorCode::StaleCursor,
        "the transcript changed; attach again for a current cursor",
    )
}

/// One page of transcript ending just before `cursor` (the tail when absent).
/// A cursor from a transcript that has since been rewritten is rejected: its
/// positions no longer name the same messages.
pub fn project_history_page(
    state: &AppState,
    limits: &ProjectionLimits,
    cursor: Option<&str>,
    limit: u32,
) -> Result<RemoteHistoryPage, RemoteError> {
    let revision = history_revision(state);
    let end = match cursor {
        None => state.history.len(),
        Some(cursor) => {
            let (before, cursor_revision) = cursor.split_once('@').ok_or_else(stale_cursor)?;
            let before: usize = before.parse().map_err(|_| stale_cursor())?;
            if cursor_revision != revision || before > state.history.len() {
                return Err(stale_cursor());
            }
            before
        }
    };
    let count = (limit as usize).clamp(1, limits.transcript_items);
    let mut start = end.saturating_sub(count);
    let mut page = RemoteHistoryPage {
        messages: project_messages(state, &revision, start..end, limits),
        next_cursor: history_cursor(&revision, start),
        history_revision: revision,
    };
    while encoded_len(&page) > limits.frame_bytes && page.messages.len() > 1 {
        let drop = page.messages.len() / 2;
        page.messages.drain(..drop);
        start += drop;
        page.next_cursor = history_cursor(&page.history_revision, start);
    }
    Ok(page)
}

/// Full text behind a `content_id` handed out by a projection, if it still
/// exists. IDs are opaque to clients; they only ever name content of the
/// session they were issued for.
pub fn resolve_content(state: &AppState, content_id: &str) -> Option<String> {
    let (kind, rest) = content_id.split_once(':')?;
    match kind {
        "message" => {
            let (revision, index) = rest.rsplit_once(':')?;
            (revision == history_revision(state))
                .then(|| state.history.get(index.parse::<usize>().ok()?))
                .flatten()
                .map(|message| message.content.clone())
        }
        "approval" => {
            let (batch_id, index) = rest.rsplit_once(':')?;
            let (pending_id, actions) = pending_approval_actions(state)?;
            (pending_id == batch_id)
                .then(|| actions.into_iter().nth(index.parse().ok()?))
                .flatten()
                .map(|action| action.full_details)
        }
        "response" => (state.active_turn_id.as_deref() == Some(rest))
            .then(|| state.current_response.as_ref().clone()),
        _ => None,
    }
}

/// One chunk of `content` starting at `offset`, cut on a character boundary.
/// Consecutive chunks, each requested at the previous `next_offset`, rebuild
/// the content exactly.
pub fn content_chunk(
    content_id: &str,
    content: &str,
    offset: u64,
    max_bytes: u32,
    limits: &ProjectionLimits,
) -> Result<RemoteContentChunk, RemoteError> {
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    if start > content.len() || !content.is_char_boundary(start) {
        return Err(RemoteError::new(
            RemoteErrorCode::InvalidRequest,
            "offset is not a chunk boundary of this content",
        ));
    }
    // Always make progress, even when asked for less than one character.
    let budget = (max_bytes as usize).clamp(4, limits.chunk_bytes.max(4));
    let end = content.floor_char_boundary((start + budget).min(content.len()));
    Ok(RemoteContentChunk {
        content_id: content_id.to_owned(),
        offset,
        text: content[start..end].to_owned(),
        total_bytes: content.len() as u64,
        next_offset: (end < content.len()).then_some(end as u64),
    })
}

/// Remote updates for one agent event the owner has already applied to
/// `state`. Identities come from the state, so the event names the same
/// turn, question and batch a snapshot taken now would.
pub fn project_event(
    state: &AppState,
    event: &AgentUiEvent,
    limits: &ProjectionLimits,
) -> Vec<RemoteEvent> {
    match event {
        AgentUiEvent::PromptStarted { prompt, timing } => vec![RemoteEvent::TurnStarted {
            turn_id: timing
                .as_ref()
                .map(|turn| turn.turn_id.clone())
                .or_else(|| state.active_turn_id.clone()),
            timing: timing
                .as_ref()
                .map(project_turn_timing)
                .or_else(|| project_active_timing(state)),
            prompt: bound_text(prompt, limits.text_bytes, None),
        }],
        AgentUiEvent::TextDelta { text } => vec![RemoteEvent::TextDelta {
            text: text.clone(),
            timing: project_active_timing(state),
            thought_time_ms: project_live_thought_time(state),
        }],
        AgentUiEvent::ToolStarted { name, id, detail } => vec![RemoteEvent::ToolStarted {
            tool: RemoteTool {
                id: id.clone(),
                name: name.clone(),
                detail: detail.as_deref().map(|detail| label(detail, limits)),
                state: RemoteToolState::Running,
            },
        }],
        AgentUiEvent::ToolFinished { id, result } => vec![RemoteEvent::ToolFinished {
            id: id.clone(),
            success: result.metadata.success,
            pending: result.metadata.pending,
            content: bound_text(&result.content, limits.text_bytes, None),
        }],
        AgentUiEvent::SubagentUpdated { id, .. } => state
            .subagents
            .iter()
            .find(|agent| agent.id == *id)
            .map(|agent| RemoteEvent::SubagentUpdated {
                subagent: project_subagent(agent, limits),
            })
            .into_iter()
            .collect(),
        AgentUiEvent::ApprovalRequested { batch_id, actions } => {
            vec![RemoteEvent::ApprovalRequested {
                approval: project_approval_actions(batch_id, actions, limits),
            }]
        }
        // The event carries no identity; the pending question does.
        AgentUiEvent::QuestionRequested { .. } => project_pending_question(state)
            .map(|question| RemoteEvent::QuestionRequested { question })
            .into_iter()
            .collect(),
        AgentUiEvent::TurnFinished { timing, .. } => vec![RemoteEvent::TurnFinished {
            turn_id: timing
                .as_ref()
                .map(|turn| turn.turn_id.clone())
                .or_else(|| state.active_turn_id.clone()),
            timing: timing.as_ref().map(project_turn_timing),
        }],
        AgentUiEvent::Cancelled { timing, .. } => vec![RemoteEvent::TurnCancelled {
            turn_id: timing
                .as_ref()
                .map(|turn| turn.turn_id.clone())
                .or_else(|| state.active_turn_id.clone()),
            timing: timing.as_ref().map(project_turn_timing),
        }],
        #[cfg(test)]
        AgentUiEvent::TurnRecovered { .. } | AgentUiEvent::Error { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppStatus, ChatMessage, ToolConfirmation};

    #[test]
    fn assistant_timing_survives_snapshot_and_history_projection() {
        let mut state = state_with_history(0, 0);
        let message: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant", "content": "<think>Check both paths</think>Done",
            "response_time_ms": 12000, "thought_time_ms": 4000,
            "completed_at": "2026-10-09T18:42:00+02:00",
            "turn": {"turn_id": "timed-turn", "started_at": "2026-10-09T18:41:00+02:00",
                "ended_at": "2026-10-09T18:42:00+02:00", "elapsed_work_ms": 32000,
                "outcome": "completed"}
        }))
        .unwrap();
        state.history.push(message);
        let snapshot = project_snapshot(&state, &context(), &ProjectionLimits::default());
        let page = project_history_page(&state, &ProjectionLimits::default(), None, 40).unwrap();
        for message in [&snapshot.transcript[0], &page.messages[0]] {
            let value = serde_json::to_value(message).unwrap();
            assert_eq!(value["thought_time_ms"], 4000);
            assert_eq!(value["response_time_ms"], 12000);
            assert_eq!(value["completed_at"], "2026-10-09T18:42:00+02:00");
            assert_eq!(value["turn"]["turn_id"], "timed-turn");
            assert_eq!(value["turn"]["elapsed_work_ms"], 32000);
            assert_eq!(value["turn"]["outcome"], "completed");
        }
    }

    fn context() -> ProjectionContext {
        ProjectionContext {
            registration_epoch: 3,
            sequence: 17,
            generation: 2,
            health: OwnerHealth::Live,
            title: Some("Fix the build".to_owned()),
        }
    }

    fn state_with_history(messages: usize, bytes_each: usize) -> AppState {
        let mut state = AppState::new();
        state.active_session_id = "projection-session".to_owned();
        for index in 0..messages {
            let role = if index % 2 == 0 { "user" } else { "assistant" };
            state.history.push(ChatMessage::new(
                role,
                format!("{index}:{}", "é".repeat(bytes_each / 2)),
            ));
        }
        state
    }

    #[test]
    fn snapshot_carries_the_identities_a_client_must_name() {
        let mut state = state_with_history(2, 16);
        state.status = AppStatus::AwaitingQuestion;
        let turn_id = state.begin_turn_identity();
        let question = PendingQuestion::new(
            "Which?".to_owned(),
            vec!["One".to_owned(), "Two".to_owned()],
            true,
        )
        .with_descriptions(vec!["first".to_owned()]);
        let question_id = question.id.clone();
        state.begin_question_chain(vec![question]);
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            request_id: Some("call-1".to_owned()),
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }]);
        state.pending_approval_details = Some(vec![r#"{"command":"cargo test"}"#.to_owned()]);
        state.pending_approval_batch_id = Some("controller:test:9".to_owned());

        let snapshot = project_snapshot(&state, &context(), &ProjectionLimits::default());

        assert_eq!(snapshot.sequence, 17);
        assert_eq!(snapshot.session.registration_epoch, 3);
        assert_eq!(snapshot.session.title, "Fix the build");
        assert_eq!(snapshot.session.activity, SessionActivity::AwaitingApproval);
        assert!(snapshot.session.attention.approval && snapshot.session.attention.question);
        assert_eq!(snapshot.turn.as_ref().unwrap().turn_id, turn_id);
        let question = snapshot.pending_question.unwrap();
        assert_eq!(question.question_id, question_id);
        assert_eq!((question.position, question.chain_length), (1, 1));
        assert_eq!(question.options[0].description.as_deref(), Some("first"));
        assert_eq!(question.options[1].description, None);
        let approval = snapshot.pending_approval.unwrap();
        assert_eq!(approval.batch_id, "controller:test:9");
        assert_eq!(
            approval.actions[0].details.text,
            r#"{"command":"cargo test"}"#
        );
        assert!(!approval.actions[0].details.truncated);
    }

    #[test]
    fn approval_without_a_batch_identity_is_not_projected() {
        let mut state = state_with_history(0, 0);
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            request_id: None,
            tool_name: "write_file".to_owned(),
            path: "a.txt".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }]);
        let snapshot = project_snapshot(&state, &context(), &ProjectionLimits::default());
        assert_eq!(snapshot.pending_approval, None);
    }

    #[test]
    fn snapshot_is_a_bounded_tail_with_a_cursor_to_the_rest() {
        let state = state_with_history(100, 10_000);
        let limits = ProjectionLimits::default();
        let snapshot = project_snapshot(&state, &context(), &limits);

        assert_eq!(snapshot.transcript.len(), limits.transcript_items);
        assert_eq!(snapshot.transcript.last().unwrap().message_id, "m99");
        assert_eq!(snapshot.transcript[0].message_id, "m60");
        let content = &snapshot.transcript[0].content;
        assert!(content.truncated);
        assert!(content.text.len() <= limits.text_bytes);
        assert_eq!(
            content.total_bytes as usize,
            state.history[60].content.len()
        );
        assert!(encoded_len(&snapshot) <= limits.frame_bytes);

        // The cursor continues exactly where the snapshot starts.
        let page = project_history_page(&state, &limits, snapshot.history_cursor.as_deref(), 25)
            .expect("the snapshot cursor is current");
        assert_eq!(page.messages.len(), 25);
        assert_eq!(page.messages.last().unwrap().message_id, "m59");
        assert_eq!(page.messages[0].message_id, "m35");
        assert!(page.next_cursor.is_some());
    }

    #[test]
    fn snapshot_sheds_content_until_it_fits_one_frame() {
        // Control characters escape to six bytes each: per-field limits alone
        // would let this snapshot outgrow the frame several times over.
        let mut state = AppState::new();
        for _ in 0..40 {
            state
                .history
                .push(ChatMessage::new("assistant", "\u{1}".repeat(8_000)));
        }
        let turn_id = state.begin_turn_identity();
        state.current_response = std::sync::Arc::new("\u{2}".repeat(100_000));
        let limits = ProjectionLimits {
            text_bytes: 8_000,
            live_response_bytes: 100_000,
            ..ProjectionLimits::default()
        };

        let snapshot = project_snapshot(&state, &context(), &limits);

        assert!(encoded_len(&snapshot) <= limits.frame_bytes);
        assert!(snapshot.transcript.len() < 40);
        let live = &snapshot.turn.as_ref().unwrap().live_response;
        assert!(live.truncated);
        assert_eq!(live.total_bytes, 100_000);
        assert_eq!(live.offset as usize + live.text.len(), 100_000);
        // Nothing was lost: the cut text is still fetchable in chunks.
        let full = resolve_content(&state, &format!("response:{turn_id}")).unwrap();
        assert_eq!(full.len(), 100_000);
        if let Some(first) = snapshot.transcript.first() {
            let index: usize = first.message_id[1..].parse().unwrap();
            assert_eq!(
                snapshot.history_cursor,
                history_cursor(&snapshot.history_revision, index)
            );
        }
    }

    #[test]
    fn long_lists_are_capped_and_counted() {
        let mut state = state_with_history(0, 0);
        state.status = AppStatus::Streaming;
        for index in 0..50 {
            state.pending_queue.push(format!("queued {index}"));
        }
        state
            .pending_queue
            .push("__task_wakeup__:internal".to_owned());
        let limits = ProjectionLimits::default();
        let snapshot = project_snapshot(&state, &context(), &limits);
        assert_eq!(snapshot.pending_prompts.len(), limits.list_items);
        assert_eq!(
            snapshot.omitted.pending_prompts,
            50 - limits.list_items as u32
        );
        assert!(
            snapshot
                .pending_prompts
                .iter()
                .all(|prompt| !prompt.text.text.starts_with("__task_wakeup__"))
        );
    }

    #[test]
    fn history_pages_rebuild_the_whole_transcript_and_reject_stale_cursors() {
        let mut state = state_with_history(23, 40);
        let limits = ProjectionLimits::default();
        let mut cursor = None;
        let mut ids = Vec::new();
        loop {
            let page = project_history_page(&state, &limits, cursor.as_deref(), 5).unwrap();
            ids.splice(
                0..0,
                page.messages
                    .iter()
                    .map(|message| message.message_id.clone()),
            );
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        let expected = (0..23).map(|index| format!("m{index}")).collect::<Vec<_>>();
        assert_eq!(ids, expected);

        // Appending keeps cursors valid; rewriting the transcript does not.
        let page = project_history_page(&state, &limits, None, 5).unwrap();
        state.history.push(ChatMessage::new("user", "appended"));
        project_history_page(&state, &limits, page.next_cursor.as_deref(), 5)
            .expect("appends keep positions stable");
        state.history.drain(..1);
        let error = project_history_page(&state, &limits, page.next_cursor.as_deref(), 5)
            .expect_err("a rewritten transcript invalidates the cursor");
        assert_eq!(error.code, RemoteErrorCode::StaleCursor);
        for garbage in ["", "nonsense", "999999@h0", "x@y"] {
            let error = project_history_page(&state, &limits, Some(garbage), 5).unwrap_err();
            assert_eq!(error.code, RemoteErrorCode::StaleCursor);
        }
    }

    #[test]
    fn truncated_content_is_rebuilt_exactly_from_chunks() {
        let state = state_with_history(1, 5_000);
        let limits = ProjectionLimits::default();
        let snapshot = project_snapshot(&state, &context(), &limits);
        let content = &snapshot.transcript[0].content;
        assert!(content.truncated);
        let content_id = content.content_id.clone().unwrap();
        let full = resolve_content(&state, &content_id).expect("content is still there");

        let mut rebuilt = String::new();
        let mut offset = Some(0);
        while let Some(at) = offset {
            // 1001 bytes never lands on a boundary of two-byte characters.
            let chunk = content_chunk(&content_id, &full, at, 1001, &limits).unwrap();
            assert!(chunk.text.len() <= 1001);
            assert_eq!(chunk.total_bytes as usize, full.len());
            rebuilt.push_str(&chunk.text);
            offset = chunk.next_offset;
        }
        assert_eq!(rebuilt, state.history[0].content);

        // Inside a character, or past the end.
        let inside = (full.find('é').unwrap() + 1) as u64;
        assert!(content_chunk(&content_id, &full, inside, 10, &limits).is_err());
        let past = full.len() as u64 + 1;
        assert!(content_chunk(&content_id, &full, past, 10, &limits).is_err());
        assert_eq!(resolve_content(&state, "message:h999:0"), None);
        assert_eq!(resolve_content(&state, "file:/etc/passwd"), None);
    }

    #[test]
    fn events_take_their_identities_from_the_applied_state() {
        let mut state = state_with_history(0, 0);
        let limits = ProjectionLimits::default();
        let turn_id = state.begin_turn_identity();
        let question = PendingQuestion::new("Go?".to_owned(), vec!["Yes".to_owned()], false);
        let question_id = question.id.clone();
        state.begin_question_chain(vec![question]);

        let started = project_event(
            &state,
            &AgentUiEvent::PromptStarted {
                prompt: "hello".to_owned(),
                timing: None,
            },
            &limits,
        );
        assert!(matches!(
            &started[..],
            [RemoteEvent::TurnStarted { turn_id: Some(id), .. }] if *id == turn_id
        ));

        let asked = project_event(
            &state,
            &AgentUiEvent::QuestionRequested {
                prompt: crate::controller::QuestionPrompt {
                    header: "Question".to_owned(),
                    text: "Go?".to_owned(),
                    options: vec!["Yes".to_owned()],
                    descriptions: Vec::new(),
                    multiple: false,
                },
            },
            &limits,
        );
        assert!(matches!(
            &asked[..],
            [RemoteEvent::QuestionRequested { question }] if question.question_id == question_id
        ));

        let approval = project_event(
            &state,
            &AgentUiEvent::ApprovalRequested {
                batch_id: "controller:test:4".to_owned(),
                actions: vec![ApprovalAction::new(
                    "call-4".to_owned(),
                    "write_file".to_owned(),
                    "write_file · a.txt".to_owned(),
                    "needs approval".to_owned(),
                    "x".repeat(100_000),
                )],
            },
            &limits,
        );
        let [RemoteEvent::ApprovalRequested { approval }] = &approval[..] else {
            panic!("expected one approval event");
        };
        assert_eq!(approval.batch_id, "controller:test:4");
        let details = &approval.actions[0].details;
        assert!(details.truncated, "a preview must say it is a preview");
        assert_eq!(details.total_bytes, 100_000);
        assert_eq!(
            details.content_id.as_deref(),
            Some("approval:controller:test:4:0")
        );

        // An unknown subagent produces no update rather than a made-up one.
        let unknown = project_event(
            &state,
            &AgentUiEvent::SubagentUpdated {
                id: 99,
                status: SubAgentStatus::Running,
                active_turn: true,
            },
            &limits,
        );
        assert!(unknown.is_empty());
    }
}
