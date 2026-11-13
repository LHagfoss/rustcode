//! Sharing a thread that runs on an external harness (`docs/harnesses.md`).
//!
//! The built-in harness shares straight from its `AppState`
//! ([`super::publisher`]). A thread on Codex, Claude Code, Pi, OpenCode or any
//! other external harness has no `AppState`: its updates arrive as ACP
//! [`SessionUpdate`]s. This module keeps a mirror of that stream — the
//! recorded transcript, the turn in flight and its tools, the pending
//! approval or question — and publishes a fresh snapshot whenever it changes.
//!
//! Reads (`get_history`, `get_content`, `get_changes`) are answered from the
//! mirror. Mutations go to the harness: a prompt through
//! [`ExternalSession::prompt`](crate::harness::session::ExternalSession::prompt),
//! a cancel through `cancel`, a steer through `steer`, and an approval or
//! question answer through the permission request the harness is waiting on.
//! That is the same path the desktop app's own prompt takes, so a prompt sent
//! from a device shows up in the app and one sent in the app shows up on the
//! device.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agent_client_protocol::schema::v1::{
    ContentBlock, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionUpdate, ToolCall, ToolCallStatus, ToolCallUpdate,
};
use serde_json::Value;
use tokio::sync::oneshot;

use crate::app::ChatMessage;
use crate::controller::QuestionPrompt;
use rustcode_core::{ToolCallRef, ToolResultRecord};

use super::ops::SessionRegistration;
use super::projection::{ProjectionLimits, bound_text};
use crate::controller::ApprovalChoice;

use super::protocol::{
    OwnerHealth, REMOTE_PROTOCOL_VERSION, RemoteAnswer, RemoteApprovalAction, RemoteApprovalBatch,
    RemoteAttention, RemoteError, RemoteErrorCode, RemoteHistoryPage, RemoteMessage,
    RemoteMessageTool, RemoteOmitted, RemoteOperation, RemotePendingPrompt,
    RemotePendingPromptKind, RemoteQuestion, RemoteQuestionOption, RemoteRequest, RemoteResult,
    RemoteSessionInfo, RemoteSnapshot, RemoteThreadParent, RemoteTool, RemoteToolState, RemoteTurn,
    RemoteTurnTiming, SessionActivity,
};

/// Prompts a device may have queued on one thread. Past this the owner says
/// so (`rate_limited`) instead of growing without bound.
pub(crate) const MAX_QUEUED_PROMPTS: usize = 16;

/// Which pending permission request a `bridge_permission` future owns, so
/// answering one in the app does not clear another device's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingKey {
    Approval(String),
    Question(String),
}

/// Where one recorded tool call lives in the mirror's history.
#[derive(Debug, Clone)]
struct ToolSlot {
    /// The `tool` message holding its result.
    result: usize,
    title: String,
    kind: String,
    input: Option<Value>,
    output: String,
    status: ToolCallStatus,
}

impl ToolSlot {
    fn finished(&self) -> bool {
        matches!(
            self.status,
            ToolCallStatus::Completed | ToolCallStatus::Failed
        )
    }

    fn success(&self) -> bool {
        self.status == ToolCallStatus::Completed
    }
}

/// The turn in flight, as the mirror knows it.
#[derive(Debug, Clone)]
struct LiveTurn {
    timing: RemoteTurnTiming,
}

/// A question the harness asked through a permission request and still waits
/// on. `reply` answers the harness; the device's answer goes here.
struct PendingQuestion {
    question_id: String,
    prompt: QuestionPrompt,
    options: Vec<(String, String)>, // permission option id, label
    reply: oneshot::Sender<RequestPermissionResponse>,
}

/// An approval the harness asked for through a permission request.
struct PendingApproval {
    batch_id: String,
    tool_name: String,
    summary: String,
    details: String,
    options: Vec<(String, PermissionOptionKind)>,
    reply: oneshot::Sender<RequestPermissionResponse>,
}

#[derive(Default)]
struct Mirror {
    history: Vec<ChatMessage>,
    /// Assistant text of the current segment, `<think>` included.
    live: String,
    /// Reasoning streamed since the last flush, kept as a `<think>` span.
    thought: String,
    tools: HashMap<String, ToolSlot>,
    turn: Option<LiveTurn>,
    /// Turns finished on this thread, as the session list counts them.
    turns: u64,
    last_turn: Option<RemoteTurnTiming>,
    queue: VecDeque<String>,
    /// Pending permission requests, by the harness's own identities: two can
    /// wait at once, and answering one must not drop the other.
    approvals: Vec<PendingApproval>,
    questions: Vec<PendingQuestion>,
    /// Whether this harness takes a message mid-turn (`_meta.rustcode.steer`).
    can_steer: bool,
    /// Bumped whenever a message is rewritten in place, so a client can tell
    /// that its history cursor no longer names the same messages.
    revision: u64,
    dirty: bool,
    last_active_at: Option<String>,
}

impl Mirror {
    fn touch(&mut self) {
        self.dirty = true;
    }

    fn push(&mut self, message: ChatMessage) {
        self.last_active_at = Some(message.timestamp.clone());
        self.history.push(message);
        self.touch();
    }

    /// Close the assistant text so far into a message of its own, if there is
    /// any.
    fn flush_reply(&mut self) {
        let text = self.take_reply();
        let Some(text) = text else { return };
        let mut message = ChatMessage::new("assistant", text);
        message.timestamp = now();
        self.push(message);
    }

    fn take_reply(&mut self) -> Option<String> {
        let thought = std::mem::take(&mut self.thought);
        let live = std::mem::take(&mut self.live);
        let mut text = String::new();
        if !thought.trim().is_empty() {
            text.push_str("<think>");
            text.push_str(&thought);
            text.push_str("</think>");
        }
        text.push_str(&live);
        (!text.trim().is_empty()).then_some(text)
    }

    fn begin_turn(&mut self) -> bool {
        if self.turn.is_some() {
            return false;
        }
        // Text that arrived while no turn was live belongs to a turn whose
        // start the mirror missed; keep it as a message rather than dropping
        // it into this one.
        self.flush_reply();
        self.tools.clear();
        self.turn = Some(LiveTurn {
            timing: RemoteTurnTiming {
                turn_id: format!("harness-turn-{}", self.turns + 1),
                started_at: Some(now()),
                ended_at: None,
                elapsed_work_ms: None,
                outcome: None,
            },
        });
        self.touch();
        true
    }

    fn end_turn(&mut self) -> bool {
        let Some(turn) = self.turn.take() else {
            return false;
        };
        self.flush_reply();
        let unfinished = self
            .tools
            .values()
            .filter(|slot| !slot.finished())
            .map(|slot| (slot.result, slot.output.clone()))
            .collect::<Vec<_>>();
        for (index, output) in unfinished {
            self.finish_tool_message(index, false, &output);
        }
        self.tools.clear();
        self.turns += 1;
        let mut timing = turn.timing;
        timing.ended_at = Some(now());
        self.last_turn = Some(timing);
        self.touch();
        true
    }

    fn finish_tool_message(&mut self, index: usize, success: bool, output: &str) {
        let Some(message) = self.history.get_mut(index) else {
            return;
        };
        let mut changed = false;
        if !output.is_empty() && message.content != output {
            message.content = output.to_owned();
            changed = true;
        }
        if let Some(record) = message.tool_result.as_mut() {
            if record.success != success || record.pending {
                record.success = success;
                record.pending = false;
                changed = true;
            }
        }
        if changed {
            // A tool message rewritten in place moves every cursor taken
            // before it: a client re-attaches instead of paging a transcript
            // it no longer names.
            self.revision += 1;
        }
    }

    fn tool(&mut self, call: &ToolCall) {
        let id = call.tool_call_id.0.to_string();
        let title = call.title.clone();
        let kind = tool_kind(call.kind);
        if !self.tools.contains_key(&id) {
            self.start_tool(&id, &title, &kind, call.raw_input.clone());
        }
        let mut fields = agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
            .title(title)
            .kind(call.kind)
            .status(call.status)
            .content(call.content.clone());
        if let Some(input) = &call.raw_input {
            fields = fields.raw_input(input.clone());
        }
        if let Some(output) = &call.raw_output {
            fields = fields.raw_output(output.clone());
        }
        self.apply_tool(&id, &fields);
    }

    fn tool_update(&mut self, update: &ToolCallUpdate) {
        let id = update.tool_call_id.0.to_string();
        if !self.tools.contains_key(&id) {
            let Some(title) = update.fields.title.clone() else {
                return;
            };
            let kind = update.fields.kind.map(tool_kind).unwrap_or_default();
            self.start_tool(&id, &title, &kind, update.fields.raw_input.clone());
        }
        self.apply_tool(&id, &update.fields);
    }

    /// Record a call: an assistant message naming it, then its `tool`
    /// message, exactly as a replayed transcript carries them.
    fn start_tool(&mut self, id: &str, title: &str, kind: &str, input: Option<Value>) {
        let mut message = self
            .take_reply()
            .map(|text| ChatMessage::new("assistant", text))
            .unwrap_or_else(|| ChatMessage::new("assistant", ""));
        message.timestamp = now();
        message.tool_calls.push(ToolCallRef {
            id: id.to_owned(),
            name: title.to_owned(),
            arguments: input.clone().unwrap_or(Value::Null).to_string(),
        });
        self.push(message);

        let mut recorded = ChatMessage::new("tool", "");
        recorded.timestamp = now();
        recorded.tool_call_id = Some(id.to_owned());
        recorded.tool_result = Some(ToolResultRecord {
            tool_name: title.to_owned(),
            arguments_hash: arguments_hash(input.as_ref()),
            success: false,
            pending: true,
            ..ToolResultRecord::default()
        });
        self.push(recorded);
        let result = self.history.len() - 1;
        self.tools.insert(
            id.to_owned(),
            ToolSlot {
                result,
                title: title.to_owned(),
                kind: kind.to_owned(),
                input,
                output: String::new(),
                status: ToolCallStatus::Pending,
            },
        );
    }

    fn apply_tool(
        &mut self,
        id: &str,
        fields: &agent_client_protocol::schema::v1::ToolCallUpdateFields,
    ) {
        let extra_output = tool_output(fields);
        let app = tool_app(fields);
        let Some(slot) = self.tools.get_mut(id) else {
            return;
        };
        let app_result = slot.result;
        if let Some(title) = &fields.title {
            slot.title = title.clone();
        }
        if let Some(input) = &fields.raw_input {
            slot.input = Some(input.clone());
        }
        if let Some(status) = fields.status {
            slot.status = status;
        }
        if !extra_output.is_empty() {
            if !slot.output.ends_with('\n') && !slot.output.is_empty() {
                slot.output.push('\n');
            }
            slot.output.push_str(&extra_output);
        }
        let finished = slot.finished();
        let success = slot.success();
        let output = slot.output.clone();
        let result = slot.result;
        if let Some(app) = app {
            self.attach_app(app_result, app);
        }
        if finished {
            self.finish_tool_message(result, success, &output);
        }
        self.touch();
    }

    /// Keep the interactive view a result offered on its record, next to
    /// the text, so a client that renders MCP Apps finds it.
    fn attach_app(&mut self, index: usize, app: rustcode_core::mcp_app::McpAppResource) {
        let Some(record) = self
            .history
            .get_mut(index)
            .and_then(|message| message.tool_result.as_mut())
        else {
            return;
        };
        if record.mcp_app.as_ref() != Some(&app) {
            record.mcp_app = Some(app);
            self.revision += 1;
        }
    }

    fn plan(&mut self, plan: &agent_client_protocol::schema::v1::Plan) {
        let mut text = String::new();
        for entry in &plan.entries {
            let mark = match entry.status {
                agent_client_protocol::schema::v1::PlanEntryStatus::Completed => "x",
                agent_client_protocol::schema::v1::PlanEntryStatus::InProgress => "-",
                _ => " ",
            };
            text.push_str(&format!("- [{mark}] {}\n", entry.content));
        }
        let mut message = ChatMessage::new("tool", text.trim_end().to_owned());
        message.timestamp = now();
        message.tool_calls.push(ToolCallRef {
            id: "plan".to_owned(),
            name: "plan".to_owned(),
            arguments: "{}".to_owned(),
        });
        message.tool_result = Some(ToolResultRecord {
            tool_name: "plan".to_owned(),
            success: true,
            ..ToolResultRecord::default()
        });
        self.push(message);
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// A harness tool kind as the short name a tool row carries.
fn tool_kind(kind: agent_client_protocol::schema::v1::ToolKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn arguments_hash(input: Option<&Value>) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input
        .map(std::string::ToString::to_string)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The MCP App a tool call's content embeds: a `ui://` HTML resource. Its
/// HTML is never part of the text the phone shows.
fn tool_app(
    fields: &agent_client_protocol::schema::v1::ToolCallUpdateFields,
) -> Option<rustcode_core::mcp_app::McpAppResource> {
    use agent_client_protocol::schema::v1::{
        ContentBlock, EmbeddedResourceResource, ToolCallContent,
    };
    fields.content.as_ref()?.iter().find_map(|block| {
        let ToolCallContent::Content(content) = block else {
            return None;
        };
        let ContentBlock::Resource(resource) = &content.content else {
            return None;
        };
        let EmbeddedResourceResource::TextResourceContents(text) = &resource.resource else {
            return None;
        };
        rustcode_core::mcp_app::mcp_app_resource(&text.uri, text.mime_type.as_deref(), &text.text)
    })
}

/// The text a tool call's content carries. The adapters put a command's
/// output in `content`, in `raw_output` or in the terminal deltas they convert
/// to `content`, so one of them is what the phone shows.
fn tool_output(fields: &agent_client_protocol::schema::v1::ToolCallUpdateFields) -> String {
    use agent_client_protocol::schema::v1::{ContentBlock, ToolCallContent};
    let mut text = String::new();
    if let Some(content) = &fields.content {
        for block in content {
            match block {
                ToolCallContent::Content(content) => {
                    if let ContentBlock::Text(chunk) = &content.content {
                        text.push_str(&chunk.text);
                    }
                }
                ToolCallContent::Diff(diff) => {
                    text.push_str(&format!("{} (+/-)\n", diff.path.display()));
                }
                _ => {}
            }
        }
    }
    if let Some(output) = &fields.raw_output {
        match output {
            Value::String(string) => text.push_str(string),
            Value::Null => {}
            other => {
                if text.is_empty() {
                    match other.get("output").and_then(Value::as_str) {
                        Some(output) => text.push_str(output),
                        None => text.push_str(&other.to_string()),
                    }
                }
            }
        }
    }
    text
}

/// The mirror of one external-harness thread, shared by the tee sink (which
/// fills it) and the share loop in [`crate::acp::remote`] (which publishes it).
pub(crate) struct HarnessMirror {
    session_id: String,
    harness: String,
    cwd: PathBuf,
    model: String,
    title: String,
    /// The thread this one was started from, read when the mirror is created:
    /// it never changes, and the snapshot builds stay free of the store.
    parent: Option<RemoteThreadParent>,
    state: StdMutex<Mirror>,
    /// Prompts that were applied to the harness and whose turn has not
    /// returned yet. A turn the harness is between phases of still counts.
    busy: AtomicUsize,
    /// The desktop app closed the thread: the share ends.
    closed: AtomicBool,
}

impl HarnessMirror {
    pub(crate) fn new(
        session_id: String,
        harness: String,
        cwd: PathBuf,
        model: String,
        title: String,
        history: Vec<ChatMessage>,
        can_steer: bool,
    ) -> Self {
        let last_active_at = history
            .iter()
            .rev()
            .map(|message| message.timestamp.clone())
            .find(|timestamp| !timestamp.is_empty());
        let turns = history
            .iter()
            .filter_map(|message| message.turn.as_ref())
            .filter(|turn| turn.outcome.is_some())
            .map(|turn| turn.turn_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len() as u64;
        let parent = super::session_parent(&session_id);
        Self {
            session_id,
            harness,
            cwd,
            model,
            title,
            parent,
            state: StdMutex::new(Mirror {
                history,
                turns,
                last_active_at,
                can_steer,
                ..Mirror::default()
            }),
            busy: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    /// The harness the thread runs on, as `docs/harnesses.md` names it.
    pub(crate) fn harness_id(&self) -> &str {
        &self.harness
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Mirror> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Sharing ended: a prompt a device queued must not run afterwards, and a
    /// harness waiting on a device's answer hears the cancellation.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let mut state = self.lock();
        if !state.queue.is_empty() || !state.approvals.is_empty() || !state.questions.is_empty() {
            state.queue.clear();
            // Dropping the senders is what tells each waiting harness (and
            // `bridge_permission`) that no answer is coming.
            state.approvals.clear();
            state.questions.clear();
            state.touch();
        }
    }

    /// Note one update the harness sent, as the tee sink does before the app
    /// sees it.
    pub(crate) fn observe(&self, update: &SessionUpdate) {
        let mut state = self.lock();
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    state.live.push_str(&text.text);
                    state.touch();
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    state.thought.push_str(&text.text);
                    state.touch();
                }
            }
            SessionUpdate::UserMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    // Some harnesses echo the prompt; keep it once.
                    let echoed = state.history.last().is_some_and(|message| {
                        message.role == "user" && message.content == text.text
                    });
                    if !echoed && !text.text.trim().is_empty() {
                        let mut message = ChatMessage::new("user", text.text.clone());
                        message.timestamp = now();
                        state.push(message);
                    }
                }
            }
            SessionUpdate::ToolCall(call) => state.tool(call),
            SessionUpdate::ToolCallUpdate(update) => state.tool_update(update),
            SessionUpdate::Plan(plan) => state.plan(plan),
            _ => {}
        }
    }

    /// Note a prompt the app or a device is about to send, so the device sees
    /// it in the transcript.
    pub(crate) fn note_prompt(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let mut state = self.lock();
        let echoed = state
            .history
            .last()
            .is_some_and(|message| message.role == "user" && message.content == text);
        if !echoed {
            let mut message = ChatMessage::new("user", text.to_owned());
            message.timestamp = now();
            state.push(message);
        }
        state.touch();
    }

    pub(crate) fn note_prompt_applied(&self) {
        self.busy.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn note_turn_returned(&self) {
        self.busy
            .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }

    /// Bring the mirror's turn in line with whether the thread is running:
    /// a turn the app or the harness started is picked up here, and one that
    /// ended is closed. Returns whether anything changed.
    pub(crate) fn sync_turn(&self, running: bool) -> bool {
        let busy = self.busy.load(Ordering::Acquire) > 0;
        let mut state = self.lock();
        if running || busy {
            state.begin_turn()
        } else {
            state.end_turn()
        }
    }

    pub(crate) fn dirty(&self) -> bool {
        self.lock().dirty
    }

    pub(crate) fn take_queue(&self) -> Option<String> {
        self.lock().queue.pop_front()
    }

    /// Queue a prompt for the end of the running turn. `false` when the
    /// device already has as many queued as one thread takes.
    pub(crate) fn queue(&self, text: String) -> bool {
        let mut state = self.lock();
        if state.queue.len() >= MAX_QUEUED_PROMPTS || self.closed.load(Ordering::Acquire) {
            return false;
        }
        state.queue.push_back(text);
        state.touch();
        true
    }

    /// Register the permission request the harness is waiting on, if sharing
    /// can answer it. The receiver completes when a device answers; the
    /// caller races it with the app's own answer. Several requests can wait
    /// at once (a harness that asks two tools in a row), so each is kept
    /// under its own identity and answered on its own.
    pub(crate) fn begin_permission(
        &self,
        request: &RequestPermissionRequest,
    ) -> (PendingKey, oneshot::Receiver<RequestPermissionResponse>) {
        let (sender, receiver) = oneshot::channel();
        let mut state = self.lock();
        let key = if let Some(prompt) = question_prompt(request) {
            let question_id = format!("harness-question-{}", request.tool_call.tool_call_id.0);
            state
                .questions
                .retain(|question| question.question_id != question_id);
            state.questions.push(PendingQuestion {
                question_id: question_id.clone(),
                prompt,
                options: request
                    .options
                    .iter()
                    .map(|option| (option.option_id.0.to_string(), option.name.clone()))
                    .collect(),
                reply: sender,
            });
            PendingKey::Question(question_id)
        } else {
            let batch_id = format!("harness-approval-{}", request.tool_call.tool_call_id.0);
            let title = request
                .tool_call
                .fields
                .title
                .clone()
                .unwrap_or_else(|| "Approval".to_owned());
            let details = request
                .tool_call
                .fields
                .raw_input
                .as_ref()
                .map(std::string::ToString::to_string)
                .unwrap_or_default();
            state
                .approvals
                .retain(|approval| approval.batch_id != batch_id);
            state.approvals.push(PendingApproval {
                batch_id: batch_id.clone(),
                tool_name: request
                    .tool_call
                    .fields
                    .kind
                    .map(tool_kind)
                    .unwrap_or_else(|| "tool".to_owned()),
                summary: title,
                details,
                options: request
                    .options
                    .iter()
                    .map(|option| (option.option_id.0.to_string(), option.kind))
                    .collect(),
                reply: sender,
            });
            PendingKey::Approval(batch_id)
        };
        state.touch();
        (key, receiver)
    }

    /// The app answered one request first: stop offering that one to devices
    /// and leave any other request waiting.
    pub(crate) fn end_permission(&self, key: &PendingKey) {
        let mut state = self.lock();
        let before = state.approvals.len() + state.questions.len();
        match key {
            PendingKey::Approval(batch_id) => {
                state
                    .approvals
                    .retain(|approval| approval.batch_id != *batch_id);
            }
            PendingKey::Question(question_id) => {
                state
                    .questions
                    .retain(|question| question.question_id != *question_id);
            }
        }
        if state.approvals.len() + state.questions.len() != before {
            state.touch();
        }
    }

    /// Resolve an approval the device named, if it is still pending.
    pub(crate) fn resolve_approval(
        &self,
        batch_id: &str,
        choice: ApprovalChoice,
    ) -> Result<(), RemoteError> {
        let mut state = self.lock();
        let Some(index) = state
            .approvals
            .iter()
            .position(|approval| approval.batch_id == batch_id)
        else {
            return Err(stale(
                RemoteErrorCode::StaleApproval,
                "the named approval batch is no longer pending",
            ));
        };
        let approval = state.approvals.remove(index);
        let Some(option) = pick_option(&approval.options, choice) else {
            state.approvals.insert(index, approval);
            return Err(stale(
                RemoteErrorCode::UnsupportedOperation,
                "the harness offers no option for that answer",
            ));
        };
        state.touch();
        let _ = approval
            .reply
            .send(RequestPermissionResponse::new(selected(option)));
        Ok(())
    }

    /// Answer a question the device named, if it is still pending.
    pub(crate) fn answer_question(
        &self,
        question_id: &str,
        answer: &RemoteAnswer,
    ) -> Result<(), RemoteError> {
        let mut state = self.lock();
        let Some(index) = state
            .questions
            .iter()
            .position(|question| question.question_id == question_id)
        else {
            return Err(stale(
                RemoteErrorCode::StaleQuestion,
                "the named question is no longer pending",
            ));
        };
        let question = &state.questions[index];
        let option = match answer {
            RemoteAnswer::Selected { options } => {
                let label = options.first().cloned().unwrap_or_default();
                question
                    .options
                    .iter()
                    .find(|(_, name)| *name == label)
                    .map(|(id, _)| id.clone())
            }
            RemoteAnswer::Custom { .. } => question.options.first().map(|(id, _)| id.clone()),
        };
        let Some(option) = option else {
            return Err(stale(
                RemoteErrorCode::InvalidAnswer,
                "the answer is not one of this question's options",
            ));
        };
        let question = state.questions.remove(index);
        state.touch();
        let mut reply = RequestPermissionResponse::new(selected(option));
        if let RemoteAnswer::Custom { text } = answer {
            reply = reply.meta(question_meta(text));
        }
        let _ = question.reply.send(reply);
        Ok(())
    }

    /// Drop the pending question; the harness hears that the user cancelled.
    pub(crate) fn cancel_question(&self, question_id: &str) -> Result<(), RemoteError> {
        let mut state = self.lock();
        let Some(index) = state
            .questions
            .iter()
            .position(|question| question.question_id == question_id)
        else {
            return Err(stale(
                RemoteErrorCode::StaleQuestion,
                "the named question is no longer pending",
            ));
        };
        let question = state.questions.remove(index);
        state.touch();
        let _ = question.reply.send(RequestPermissionResponse::new(
            RequestPermissionOutcome::Cancelled,
        ));
        Ok(())
    }

    pub(crate) fn wants_prompt(&self) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        let state = self.lock();
        !state.queue.is_empty() && state.turn.is_none()
    }

    /// Whether this harness takes a message mid-turn.
    pub(crate) fn can_steer(&self) -> bool {
        self.lock().can_steer
    }

    /// Whether sharing this thread ended.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Ids of the approvals waiting on a device, oldest first (tests, and a
    /// log line that names more than one).
    #[cfg(test)]
    pub(crate) fn pending_approval_ids(&self) -> Vec<String> {
        self.lock()
            .approvals
            .iter()
            .map(|approval| approval.batch_id.clone())
            .collect()
    }

    /// The snapshot a device attaches to, at `sequence`.
    pub(crate) fn snapshot(
        &self,
        registration: &SessionRegistration,
        sequence: u64,
        limits: &ProjectionLimits,
    ) -> RemoteSnapshot {
        let mut state = self.lock();
        state.dirty = false;
        let mut omitted = RemoteOmitted::default();
        let revision = format!("h{}", state.revision);
        let mut first = state.history.len().saturating_sub(limits.transcript_items);
        let tools = super::projection::cap(
            state
                .tools
                .iter()
                .filter(|(_, slot)| !slot.finished())
                .map(|(id, slot)| RemoteTool {
                    id: id.clone(),
                    name: slot.title.clone(),
                    detail: (!slot.kind.is_empty()).then(|| slot.kind.clone()),
                    state: match slot.status {
                        ToolCallStatus::Pending => RemoteToolState::Preparing,
                        ToolCallStatus::Failed => RemoteToolState::Failed,
                        ToolCallStatus::Completed => RemoteToolState::Succeeded,
                        _ => RemoteToolState::Running,
                    },
                })
                .collect::<Vec<_>>(),
            limits.list_items,
            &mut omitted.tools,
        );
        let turn = state.turn.clone().map(|turn| RemoteTurn {
            turn_id: turn.timing.turn_id.clone(),
            live_response: bound_text(&state.live, limits.live_response_bytes, None),
            tools,
            can_steer: state.can_steer,
            timing: Some(turn.timing),
            thought_time_ms: None,
            thought_tokens: None,
            thought_tokens_estimated: None,
        });
        let pending_prompts = super::projection::cap(
            state
                .queue
                .iter()
                .map(|text| RemotePendingPrompt {
                    kind: RemotePendingPromptKind::Queue,
                    text: bound_text(text, limits.label_bytes, None),
                })
                .collect::<Vec<_>>(),
            limits.list_items,
            &mut omitted.pending_prompts,
        );
        let awaiting_approval = !state.approvals.is_empty();
        let awaiting_question = !state.questions.is_empty();
        let mut snapshot = RemoteSnapshot {
            commands: Vec::new(),
            capabilities: Vec::new(),
            settings: None,
            session: RemoteSessionInfo {
                session_id: self.session_id.clone(),
                registration_epoch: registration.registration_epoch,
                title: self.title.clone(),
                workspace: Some(self.cwd.display().to_string()),
                model: self.model.clone(),
                harness: Some(self.harness.clone()),
                turn_count: Some(state.turns),
                last_active_at: state.last_active_at.clone(),
                parent: self.parent.clone(),
                activity: if awaiting_approval {
                    SessionActivity::AwaitingApproval
                } else if awaiting_question {
                    SessionActivity::AwaitingQuestion
                } else if state.turn.is_some() {
                    SessionActivity::Running
                } else {
                    SessionActivity::Idle
                },
                attention: RemoteAttention {
                    approval: awaiting_approval,
                    question: awaiting_question,
                },
                health: OwnerHealth::Live,
            },
            snapshot_id: None,
            sequence,
            generation: 0,
            turn,
            last_turn: state.last_turn.clone(),
            transcript: project_messages(
                &state.history,
                &revision,
                first..state.history.len(),
                limits,
            ),
            history_revision: revision.clone(),
            history_cursor: (first > 0).then(|| format!("{first}@{revision}")),
            // The device answers the first of each; the rest stay pending and
            // are offered by the next snapshot.
            pending_question: state.questions.first().map(|question| RemoteQuestion {
                question_id: question.question_id.clone(),
                header: question.prompt.header.clone(),
                text: question.prompt.text.clone(),
                options: question
                    .options
                    .iter()
                    .map(|(_, label)| RemoteQuestionOption {
                        label: label.clone(),
                        description: None,
                    })
                    .collect(),
                multiple: question.prompt.multiple,
                position: 1,
                chain_length: state.questions.len() as u32,
            }),
            pending_approval: state.approvals.first().map(|approval| RemoteApprovalBatch {
                batch_id: approval.batch_id.clone(),
                actions: vec![RemoteApprovalAction {
                    tool_name: approval.tool_name.clone(),
                    summary: approval.summary.clone(),
                    risk: String::new(),
                    details: bound_text(&approval.details, limits.approval_bytes, None),
                }],
            }),
            pending_prompts,
            subagents: Vec::new(),
            background_tasks: Vec::new(),
            omitted,
        };
        // The per-field limits bound an ordinary thread; JSON escaping can
        // still inflate text. Shed the least urgent content until the frame
        // fits, exactly as `project_snapshot` does: older transcript first,
        // then the live response, then the approval's arguments.
        while super::projection::encoded_len(&snapshot) > limits.frame_bytes {
            if !snapshot.transcript.is_empty() {
                let drop = snapshot.transcript.len().div_ceil(2);
                snapshot.transcript.drain(..drop);
                first += drop;
                snapshot.history_cursor = (first > 0).then(|| format!("{first}@{revision}"));
            } else if snapshot
                .turn
                .as_mut()
                .is_some_and(|turn| super::projection::halve(&mut turn.live_response, true))
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
                super::projection::halve(&mut action.details, false);
            } else {
                break;
            }
        }
        snapshot
    }

    /// The turn a device may cancel now.
    pub(crate) fn turn_id(&self) -> Option<String> {
        self.lock()
            .turn
            .as_ref()
            .map(|turn| turn.timing.turn_id.clone())
    }

    /// A message the owner produced on its own: the harness failed the turn.
    pub(crate) fn note_error(&self, message: &str) {
        let mut state = self.lock();
        state.flush_reply();
        let mut recorded = ChatMessage::new("assistant", message.to_owned());
        recorded.timestamp = now();
        state.push(recorded);
    }

    /// Validate a request's identity against this registration, before
    /// anything is read or applied.
    pub(crate) fn check(
        &self,
        registration: &SessionRegistration,
        request: &RemoteRequest,
    ) -> Result<(), RemoteError> {
        if request.protocol_version != REMOTE_PROTOCOL_VERSION {
            return Err(RemoteError::incompatible_version(u64::from(
                request.protocol_version,
            )));
        }
        if request.session_id.as_deref() != Some(registration.session_id.as_str())
            || request.registration_epoch != Some(registration.registration_epoch)
            || registration.session_id != self.session_id
        {
            return Err(stale(
                RemoteErrorCode::StaleSession,
                "the session is no longer shared under this registration",
            ));
        }
        Ok(())
    }

    /// Answer a read that only the owner can serve.
    pub(crate) fn read(
        &self,
        registration: &SessionRegistration,
        request: &RemoteRequest,
        limits: &ProjectionLimits,
    ) -> Result<RemoteResult, RemoteError> {
        self.check(registration, request)?;
        let state = self.lock();
        let revision = format!("h{}", state.revision);
        match &request.operation {
            RemoteOperation::GetHistory { cursor, limit } => {
                let end = match cursor {
                    None => state.history.len(),
                    Some(cursor) => {
                        let (before, cursor_revision) =
                            cursor.split_once('@').ok_or_else(stale_cursor)?;
                        let before: usize = before.parse().map_err(|_| stale_cursor())?;
                        if cursor_revision != revision || before > state.history.len() {
                            return Err(stale_cursor());
                        }
                        before
                    }
                };
                let count = (*limit as usize).clamp(1, limits.transcript_items);
                let start = end.saturating_sub(count);
                Ok(RemoteResult::History(RemoteHistoryPage {
                    messages: project_messages(&state.history, &revision, start..end, limits),
                    next_cursor: (start > 0).then(|| format!("{start}@{revision}")),
                    history_revision: revision,
                }))
            }
            RemoteOperation::GetContent {
                content_id,
                offset,
                max_bytes,
            } => {
                let content = resolve_content(&state, &revision, content_id)
                    .ok_or_else(|| stale(RemoteErrorCode::NotFound, "no such content"))?;
                super::projection::content_chunk(content_id, &content, *offset, *max_bytes, limits)
                    .map(RemoteResult::Content)
            }
            _ => Err(stale(
                RemoteErrorCode::UnsupportedOperation,
                "the session owner does not answer this operation",
            )),
        }
    }
}

fn stale(code: RemoteErrorCode, message: &str) -> RemoteError {
    RemoteError::new(code, message)
}

fn stale_cursor() -> RemoteError {
    stale(
        RemoteErrorCode::StaleCursor,
        "the transcript changed; attach again for a current cursor",
    )
}

fn selected(option: String) -> RequestPermissionOutcome {
    RequestPermissionOutcome::Selected(
        agent_client_protocol::schema::v1::SelectedPermissionOutcome::new(option),
    )
}

fn question_meta(text: &str) -> agent_client_protocol::schema::v1::Meta {
    let mut meta = agent_client_protocol::schema::v1::Meta::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({ "answer": { "text": text } }),
    );
    meta
}

/// Approve or deny with the option the harness offers for it, preferring the
/// one-shot kind so a denial does not remember the tool.
fn pick_option(
    options: &[(String, PermissionOptionKind)],
    choice: ApprovalChoice,
) -> Option<String> {
    let wanted: &[PermissionOptionKind] = match choice {
        ApprovalChoice::Approve => &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ],
        ApprovalChoice::Deny => &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
    };
    for kind in wanted {
        if let Some((id, _)) = options.iter().find(|(_, offered)| offered == kind) {
            return Some(id.clone());
        }
    }
    None
}

/// The question behind a permission request, when it is one. Harnesses ask a
/// question through `session/request_permission` with the `_meta.rustcode`
/// question payload and the question itself in `raw_input` (see
/// [`crate::acp::questions`]).
fn question_prompt(request: &RequestPermissionRequest) -> Option<QuestionPrompt> {
    let kind = request
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("kind"))
        .and_then(Value::as_str)?;
    if kind != "question" {
        return None;
    }
    let input = request.tool_call.fields.raw_input.as_ref()?;
    let options = input
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let descriptions = input
        .get("descriptions")
        .and_then(Value::as_array)
        .map(|descriptions| {
            descriptions
                .iter()
                .map(|description| description.as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(QuestionPrompt {
        header: input
            .get("header")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        text: input
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        options: if options.is_empty() {
            request
                .options
                .iter()
                .map(|option| option.name.clone())
                .collect()
        } else {
            options
        },
        descriptions,
        multiple: input
            .get("multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// One page of the mirror's transcript, shaped like
/// [`super::projection`] shapes the built-in harness's.
fn project_messages(
    history: &[ChatMessage],
    revision: &str,
    range: std::ops::Range<usize>,
    limits: &ProjectionLimits,
) -> Vec<RemoteMessage> {
    history[range.clone()]
        .iter()
        .zip(range)
        .map(|(message, index)| RemoteMessage {
            message_id: format!("m{index}"),
            role: message.role.clone(),
            content: bound_text(
                &message.content,
                limits.text_bytes,
                Some(format!("message:{revision}:{index}")),
            ),
            tool: message
                .tool_result
                .as_ref()
                .map(|record| RemoteMessageTool {
                    call_id: message.tool_call_id.clone(),
                    name: record.tool_name.clone(),
                    detail: record.command.clone().or_else(|| {
                        message
                            .tool_calls
                            .first()
                            .map(|call| call.name.clone())
                            .filter(|name| !name.is_empty())
                    }),
                    success: record.success,
                    pending: record.pending,
                    mcp_app: record.mcp_app.as_ref().map(Into::into),
                }),
            timestamp: chrono::DateTime::parse_from_rfc3339(&message.timestamp)
                .ok()
                .map(|_| message.timestamp.clone()),
            response_time_ms: message.response_time_ms,
            thought_time_ms: message.thought_time_ms,
            thought_tokens: message.thought_tokens,
            thought_tokens_estimated: message.thought_tokens.map(|_| true),
            completed_at: message.completed_at.clone(),
            turn: message.turn.as_ref().map(|turn| RemoteTurnTiming {
                turn_id: turn.turn_id.clone(),
                started_at: turn.started_at.clone(),
                ended_at: turn.ended_at.clone(),
                elapsed_work_ms: turn.elapsed_work_ms,
                outcome: None,
            }),
            origin: super::projection::project_origin(message),
        })
        .collect()
}

/// Full text behind a `content_id` this mirror handed out.
fn resolve_content(state: &Mirror, revision: &str, content_id: &str) -> Option<String> {
    let (kind, rest) = content_id.split_once(':')?;
    match kind {
        "message" => {
            let (message_revision, index) = rest.rsplit_once(':')?;
            (message_revision == revision)
                .then(|| state.history.get(index.parse::<usize>().ok()?))
                .flatten()
                .map(|message| message.content.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror() -> HarnessMirror {
        HarnessMirror::new(
            "session-1".to_owned(),
            "claude-code".to_owned(),
            PathBuf::from("/tmp/workspace"),
            "opus".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            true,
        )
    }

    fn registration() -> SessionRegistration {
        SessionRegistration {
            session_id: "session-1".to_owned(),
            registration_epoch: 3,
        }
    }

    fn text_chunk(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(agent_client_protocol::schema::v1::ContentChunk::new(
            ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(text)),
        ))
    }

    #[test]
    fn a_tool_result_keeps_its_mcp_app_next_to_the_text() {
        use agent_client_protocol::schema::v1::{
            EmbeddedResource, EmbeddedResourceResource, TextContent, TextResourceContents,
            ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
        };
        let mirror = mirror();
        mirror.note_prompt("weather");
        assert!(mirror.sync_turn(true));
        let app = TextResourceContents::new("<p>Sunny</p>", "ui://weather/view")
            .mime_type("text/html;profile=mcp-app".to_owned());
        mirror.observe(&SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new()
                .title("mcp__weather__show")
                .status(ToolCallStatus::Completed)
                .content(vec![
                    ToolCallContent::from(ContentBlock::Text(TextContent::new("Sunny, 21C"))),
                    ToolCallContent::from(ContentBlock::Resource(EmbeddedResource::new(
                        EmbeddedResourceResource::TextResourceContents(app),
                    ))),
                ]),
        )));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        let result = snapshot
            .transcript
            .iter()
            .find(|message| message.role == "tool")
            .expect("the result was kept");
        assert_eq!(
            result.content.text, "Sunny, 21C",
            "the HTML is not the text"
        );
        let tool = result.tool.as_ref().expect("a tool record");
        let app = tool.mcp_app.as_ref().expect("the app was carried");
        assert_eq!(app.uri, "ui://weather/view");
        assert!(app.available);
        assert_eq!(app.total_bytes, "<p>Sunny</p>".len() as u64);
    }

    #[test]
    fn a_turn_publishes_the_prompt_the_reply_and_its_end() {
        let mirror = mirror();
        mirror.note_prompt("hello");
        assert!(mirror.sync_turn(true));
        mirror.observe(&text_chunk("hi there"));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        assert_eq!(snapshot.transcript.len(), 1);
        assert_eq!(snapshot.transcript[0].content.text, "hello");
        let turn = snapshot.turn.expect("a turn is running");
        assert_eq!(turn.live_response.text, "hi there");
        assert_eq!(snapshot.session.activity, SessionActivity::Running);

        assert!(mirror.sync_turn(false));
        let snapshot = mirror.snapshot(&registration(), 2, &ProjectionLimits::default());
        assert!(snapshot.turn.is_none(), "the turn ended");
        let last = snapshot.transcript.last().expect("the reply was kept");
        assert_eq!(last.role, "assistant");
        assert_eq!(last.content.text, "hi there");
    }

    #[test]
    fn a_snapshot_names_the_harness_and_model_of_the_thread() {
        let mirror = HarnessMirror::new(
            "session-1".to_owned(),
            "claude-code".to_owned(),
            PathBuf::from("/tmp/workspace"),
            "opus".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            true,
        );
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        assert_eq!(snapshot.session.harness.as_deref(), Some("claude-code"));
        assert_eq!(snapshot.session.model, "opus");
    }

    #[test]
    fn an_approval_round_trips_to_the_harness() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };
        let mirror = mirror();
        let request = RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new(
                "call-1",
                ToolCallUpdateFields::new()
                    .title("Run `rm -rf build`")
                    .raw_input(serde_json::json!({"command": "rm -rf build"})),
            ),
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        );
        let (_, mut reply) = mirror.begin_permission(&request);
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        let approval = snapshot.pending_approval.expect("an approval is pending");
        assert_eq!(snapshot.session.attention.approval, true);
        assert_eq!(approval.actions[0].summary, "Run `rm -rf build`");

        mirror
            .resolve_approval(&approval.batch_id, ApprovalChoice::Approve)
            .expect("the batch resolves");
        let answered = reply.try_recv().expect("the harness hears the answer");
        assert!(matches!(
            answered.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "allow"
        ));
        let snapshot = mirror.snapshot(&registration(), 2, &ProjectionLimits::default());
        assert!(snapshot.pending_approval.is_none());
    }

    #[test]
    fn a_stale_approval_is_refused_and_the_pending_one_kept() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };
        let mirror = mirror();
        let request = RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new("call-1", ToolCallUpdateFields::new().title("Edit")),
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        );
        let (_key, _reply) = mirror.begin_permission(&request);
        let error = mirror
            .resolve_approval("another-batch", ApprovalChoice::Deny)
            .expect_err("a different batch is refused");
        assert_eq!(error.code, RemoteErrorCode::StaleApproval);
        assert!(
            mirror
                .snapshot(&registration(), 1, &ProjectionLimits::default())
                .pending_approval
                .is_some()
        );
    }

    /// Two harness requests can wait at once; answering one leaves the other
    /// pending, and each reply reaches its own request.
    #[test]
    fn two_pending_approvals_are_answered_independently() {
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };
        let mirror = mirror();
        let ask = |id: &'static str, title: &str| {
            RequestPermissionRequest::new(
                "session-1",
                ToolCallUpdate::new(id, ToolCallUpdateFields::new().title(title)),
                vec![
                    PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                    PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
                ],
            )
        };
        let (first_key, mut first) = mirror.begin_permission(&ask("call-1", "Run the tests"));
        let (second_key, mut second) = mirror.begin_permission(&ask("call-2", "Write the file"));
        assert_eq!(
            mirror.pending_approval_ids(),
            vec![
                "harness-approval-call-1".to_owned(),
                "harness-approval-call-2".to_owned()
            ]
        );

        // The app answers the first; the second is still offered to devices.
        mirror.end_permission(&first_key);
        assert_eq!(
            mirror.pending_approval_ids(),
            vec!["harness-approval-call-2".to_owned()],
            "answering in the app must not clear another request"
        );
        assert!(
            first.try_recv().is_err(),
            "the app's answer is the harness's answer for that request"
        );
        assert!(
            second.try_recv().is_err(),
            "the untouched request is still waiting"
        );

        mirror
            .resolve_approval("harness-approval-call-2", ApprovalChoice::Deny)
            .expect("the second batch resolves");
        let answered = second.blocking_recv().expect("the harness hears it");
        assert!(matches!(
            answered.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "deny"
        ));
        assert!(second_key == PendingKey::Approval("harness-approval-call-2".to_owned()));
    }

    /// Text that arrived before the turn was known grew no turn: it is kept as
    /// a message, not cleared when the turn begins.
    #[test]
    fn text_that_arrives_before_a_turn_is_kept() {
        let mirror = mirror();
        mirror.observe(&text_chunk("an early word"));
        assert!(mirror.sync_turn(true));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        assert!(
            snapshot
                .transcript
                .iter()
                .any(|message| message.role == "assistant"
                    && message.content.text.contains("an early word")),
            "the early text is a message of its own: {:?}",
            snapshot.transcript
        );
    }

    /// A device that queues more prompts than a thread takes is refused, and
    /// a share that ends drops what was queued rather than running it later.
    #[test]
    fn the_queue_is_bounded_and_dropped_when_sharing_ends() {
        let mirror = mirror();
        for index in 0..MAX_QUEUED_PROMPTS {
            assert!(mirror.queue(format!("queued {index}")));
        }
        assert!(!mirror.queue("one too many".to_owned()));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        assert_eq!(snapshot.pending_prompts.len(), MAX_QUEUED_PROMPTS);
        assert_eq!(snapshot.omitted.pending_prompts, 0);

        mirror.close();
        assert!(!mirror.wants_prompt(), "nothing runs after sharing ended");
        assert!(mirror.take_queue().is_none());
        let snapshot = mirror.snapshot(&registration(), 2, &ProjectionLimits::default());
        assert!(snapshot.pending_prompts.is_empty());
    }

    /// The snapshot sheds content until it fits one frame, and caps the lists
    /// it carries, exactly as the built-in harness projects.
    #[test]
    fn an_oversized_snapshot_sheds_until_it_fits() {
        let mirror = mirror();
        for index in 0..20 {
            mirror.note_prompt(&format!("prompt {index} {}", "x".repeat(2_000)));
        }
        for index in 0..40 {
            mirror.queue(format!("queued {index}"));
        }
        let limits = ProjectionLimits {
            transcript_items: 40,
            text_bytes: 4_000,
            frame_bytes: 8_000,
            list_items: 32,
            ..ProjectionLimits::default()
        };
        let snapshot = mirror.snapshot(&registration(), 1, &limits);
        let encoded = serde_json::to_vec(&snapshot).expect("a snapshot serializes");
        assert!(
            encoded.len() <= limits.frame_bytes,
            "{} bytes is more than one frame",
            encoded.len()
        );
        assert!(
            snapshot.transcript.len() < 20,
            "older transcript was shed first"
        );
        // A history cursor that points at the tail survives the shedding.
        let RemoteResult::History(page) = mirror
            .read(
                &registration(),
                &RemoteRequest {
                    authenticated_device_id: None,
                    protocol_version: REMOTE_PROTOCOL_VERSION,
                    request_id: "r1".to_owned(),
                    session_id: Some("session-1".to_owned()),
                    registration_epoch: Some(3),
                    operation: RemoteOperation::GetHistory {
                        cursor: None,
                        limit: 2,
                    },
                },
                &limits,
            )
            .expect("the tail page is served")
        else {
            panic!("a history page");
        };
        assert_eq!(page.messages.len(), 2);
    }

    #[test]
    fn a_snapshot_says_whether_the_harness_takes_a_steer() {
        let mirror = HarnessMirror::new(
            "session-1".to_owned(),
            "codex".to_owned(),
            PathBuf::from("/tmp/workspace"),
            "gpt-5.5".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            false,
        );
        assert!(mirror.sync_turn(true));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        let turn = snapshot.turn.expect("a turn is running");
        assert!(
            !turn.can_steer,
            "a harness that does not steer is not offered"
        );
    }

    // #3516: a harness thread keeps only the text of a prompt, and the mirror
    // says who sent it all the same.
    #[test]
    fn a_mirrored_prompt_says_who_sent_it() {
        let mirror = mirror();
        mirror.note_prompt("From thread “Ship the tree” (agent):\n\nReview the rules.");
        mirror.note_prompt(
            "Delegate failed: Fix the build\n\nFinished:\n\n- link_id session-1/4 · failed · “Fix the build” · Codex · session child-9\n  answer: (nothing it said came back)\n\n<!-- rustcode:wake -->",
        );
        mirror.note_prompt("thanks");
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        let origins = snapshot
            .transcript
            .iter()
            .map(|message| {
                message
                    .origin
                    .clone()
                    .expect("a user message has an origin")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            origins
                .iter()
                .map(|origin| origin.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["agent", "delegate", "user"]
        );
        assert_eq!(
            origins[0].threads[0].title.as_deref(),
            Some("Ship the tree")
        );
        assert_eq!(
            origins[1].threads,
            vec![crate::remote::RemoteOriginThread {
                session_id: Some("child-9".to_owned()),
                title: Some("Fix the build".to_owned()),
                status: Some("failed".to_owned()),
                link_id: Some("session-1/4".to_owned()),
            }]
        );
        assert!(origins[2].threads.is_empty());
    }

    #[test]
    fn history_pages_walk_backwards_through_the_mirror() {
        let mirror = mirror();
        for index in 0..5 {
            mirror.note_prompt(&format!("prompt {index}"));
        }
        let limits = ProjectionLimits::default();
        let page = mirror
            .read(
                &registration(),
                &RemoteRequest {
                    authenticated_device_id: None,
                    protocol_version: REMOTE_PROTOCOL_VERSION,
                    request_id: "r1".to_owned(),
                    session_id: Some("session-1".to_owned()),
                    registration_epoch: Some(3),
                    operation: RemoteOperation::GetHistory {
                        cursor: None,
                        limit: 2,
                    },
                },
                &limits,
            )
            .expect("the tail page is served");
        let RemoteResult::History(page) = page else {
            panic!("a history page");
        };
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[1].content.text, "prompt 4");
        let cursor = page.next_cursor.expect("more before this page");
        assert!(cursor.starts_with("3@"));
    }

    #[test]
    fn a_queued_prompt_waits_for_the_running_turn() {
        let mirror = mirror();
        assert!(mirror.sync_turn(true));
        mirror.queue("next".to_owned());
        assert!(!mirror.wants_prompt(), "the turn is still running");
        assert!(mirror.sync_turn(false));
        assert!(mirror.wants_prompt());
        assert_eq!(mirror.take_queue().as_deref(), Some("next"));
        let snapshot = mirror.snapshot(&registration(), 1, &ProjectionLimits::default());
        assert!(snapshot.pending_prompts.is_empty());
    }
}
