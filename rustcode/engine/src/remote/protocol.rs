//! Wire types for remote protocol v1.
//!
//! One JSON object per frame. A client sends [`RemoteRequest`]; it receives
//! [`RemoteFrame`], which is a correlated response, a sequenced session event
//! or a session-list update. Every enum is a flat map tagged by `type`
//! (`kind` for the frame itself) so a `Codable` client decodes one shape.
//! Optional fields are omitted when absent, never sent as `null`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::controller::ApprovalChoice;

pub const REMOTE_PROTOCOL_VERSION: u32 = 1;

/// Upper bound for one encoded frame in either direction. Content that does
/// not fit is truncated with metadata and fetched with `get_content`.
pub const MAX_REMOTE_FRAME_BYTES: usize = 256 * 1024;

/// Longest accepted `request_id`.
pub const MAX_REQUEST_ID_BYTES: usize = 128;

/// Client → host. Device identity is derived from authentication and never
/// travels in the frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteRequest {
    pub protocol_version: u32,
    /// Client-chosen, unique per device and registration epoch. Reusing it
    /// with the same payload returns the original outcome.
    pub request_id: String,
    /// Required for every operation that targets one session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The registration the client observed for `session_id`. A session that
    /// was re-shared has a new epoch and rejects the old one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_epoch: Option<u64>,
    pub operation: RemoteOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteOperation {
    /// Live shared sessions on this host, once.
    ListSessions,
    /// As `list_sessions`, then a `sessions` frame on every change.
    SubscribeSessions,
    /// Subscribe to one session. Without `resume` (or when replay is not
    /// possible) the response carries an authoritative snapshot.
    AttachSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<ResumeCursor>,
    },
    DetachSession,
    /// Older transcript, newest page first. `cursor` comes from a snapshot
    /// or a previous page; omit it to start at the transcript tail.
    GetHistory {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        limit: u32,
    },
    /// One chunk of content that was truncated in another frame.
    GetContent {
        content_id: String,
        offset: u64,
        max_bytes: u32,
    },
    /// Start a turn. Rejected with `busy` while the session is running.
    SubmitPrompt {
        prompt: String,
    },
    /// Redirect the running turn. Rejected when it does not accept steering.
    Steer {
        prompt: String,
    },
    /// Run after the running turn. Rejected with `not_running` when idle.
    Queue {
        prompt: String,
    },
    CancelQuestion {
        question_id: String,
    },
    ExecuteCommand {
        command: String,
    },
    CancelTurn {
        turn_id: String,
    },
    AnswerQuestion {
        question_id: String,
        answer: RemoteAnswer,
    },
    ResolveApproval {
        batch_id: String,
        choice: ApprovalChoice,
    },
    /// Outcome of an earlier mutation whose response was lost. Never applies
    /// anything.
    GetRequestStatus {
        target_request_id: String,
    },
    /// Atomically select a configured profile and effort while idle.
    SetSessionSettings {
        model: String,
        reasoning_effort: String,
    },
}

impl RemoteOperation {
    /// Every `type` tag this version understands.
    pub const NAMES: [&'static str; 16] = [
        "list_sessions",
        "subscribe_sessions",
        "attach_session",
        "detach_session",
        "get_history",
        "get_content",
        "submit_prompt",
        "steer",
        "queue",
        "cancel_question",
        "execute_command",
        "cancel_turn",
        "answer_question",
        "resolve_approval",
        "get_request_status",
        "set_session_settings",
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Self::ListSessions => "list_sessions",
            Self::SubscribeSessions => "subscribe_sessions",
            Self::AttachSession { .. } => "attach_session",
            Self::DetachSession => "detach_session",
            Self::GetHistory { .. } => "get_history",
            Self::GetContent { .. } => "get_content",
            Self::SubmitPrompt { .. } => "submit_prompt",
            Self::Steer { .. } => "steer",
            Self::Queue { .. } => "queue",
            Self::CancelQuestion { .. } => "cancel_question",
            Self::ExecuteCommand { .. } => "execute_command",
            Self::CancelTurn { .. } => "cancel_turn",
            Self::AnswerQuestion { .. } => "answer_question",
            Self::ResolveApproval { .. } => "resolve_approval",
            Self::GetRequestStatus { .. } => "get_request_status",
            Self::SetSessionSettings { .. } => "set_session_settings",
        }
    }

    /// Whether the operation names one session and so needs `session_id`
    /// and `registration_epoch` on its envelope.
    pub fn targets_session(&self) -> bool {
        !matches!(
            self,
            Self::ListSessions | Self::SubscribeSessions | Self::GetRequestStatus { .. }
        )
    }

    /// Whether the operation changes session state. Mutations get a receipt
    /// and are applied at most once per registration epoch.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::SubmitPrompt { .. }
                | Self::Steer { .. }
                | Self::Queue { .. }
                | Self::CancelQuestion { .. }
                | Self::ExecuteCommand { .. }
                | Self::CancelTurn { .. }
                | Self::AnswerQuestion { .. }
                | Self::ResolveApproval { .. }
                | Self::SetSessionSettings { .. }
        )
    }
}

/// Where a reconnecting client left off. Replay is offered only for the same
/// gateway instance and registration epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResumeCursor {
    pub gateway_id: String,
    /// Identity of the last applied snapshot. Required to resume exactly at
    /// its watermark, where a newer snapshot can have the same sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// The `instance_id` the cursor was obtained under, as the handshake and
    /// the `attached`/`resumed` results report it. A cursor from another
    /// gateway instance is never replayed. When absent the gateway decides
    /// from the sequence alone, which also refuses a cursor that predates a
    /// restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Sequence of the last event the client applied.
    pub last_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteAnswer {
    /// Options of the question, verbatim. Exactly one unless the question
    /// is `multiple`.
    Selected {
        options: Vec<String>,
    },
    Custom {
        text: String,
    },
}

/// Host → client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteFrame {
    Response(RemoteResponse),
    Event(RemoteEventFrame),
    Sessions(RemoteSessionsFrame),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteResponse {
    pub protocol_version: u32,
    /// Echoes the request. Empty when the request was too malformed to
    /// carry one.
    pub request_id: String,
    /// Present for mutations only: `applied` or `rejected` once the owner
    /// has decided, `unknown` when it could not be reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ReceiptState>,
    pub result: RemoteResult,
}

impl RemoteResponse {
    pub fn new(request_id: impl Into<String>, result: RemoteResult) -> Self {
        Self {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: request_id.into(),
            receipt: None,
            result,
        }
    }

    pub fn error(request_id: impl Into<String>, error: RemoteError) -> Self {
        Self::new(request_id, RemoteResult::Error(error))
    }

    pub fn with_receipt(mut self, receipt: ReceiptState) -> Self {
        self.receipt = Some(receipt);
        self
    }
}

/// What the session owner did with a mutation. `applied` and `rejected` come
/// from the owner; `received` is only ever reported by `get_request_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    /// Accepted by the gateway, not yet decided by the owner.
    Received,
    Applied,
    Rejected,
    /// The owner is gone or holds no record. The client must not resend
    /// on its own.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteResult {
    Error(RemoteError),
    Sessions {
        gateway_id: String,
        /// The running gateway process; changes on every gateway start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<String>,
        sessions: Vec<RemoteSessionInfo>,
        /// True when `sessions` frames will follow.
        subscribed: bool,
    },
    /// Authoritative state. Events with `sequence` above
    /// `snapshot.sequence` follow.
    Attached {
        gateway_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<String>,
        /// Set when the request carried a `resume` cursor that could not be
        /// replayed: the client discards what it holds for the session and
        /// starts from `snapshot`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resync: Option<ResyncReason>,
        snapshot: Box<RemoteSnapshot>,
    },
    /// The requested replay is available: the events after
    /// `last_sequence` follow, with no snapshot.
    Resumed {
        gateway_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance_id: Option<String>,
        next_sequence: u64,
    },
    Detached,
    History(RemoteHistoryPage),
    Content(RemoteContentChunk),
    PromptAccepted {
        disposition: PromptDisposition,
    },
    TurnCancelled {
        turn_id: String,
    },
    QuestionCancelled {
        question_id: String,
    },
    CommandExecuted {
        command: String,
        title: String,
        output: String,
    },
    QuestionAnswered {
        question_id: String,
    },
    ApprovalResolved {
        batch_id: String,
        choice: ApprovalChoice,
    },
    SessionSettingsUpdated {
        settings: RemoteSessionSettings,
    },
    RequestStatus {
        target_request_id: String,
        receipt: ReceiptState,
        /// The original outcome, when the owner still holds it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Box<RemoteResult>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptDisposition {
    /// The session was idle; the prompt starts a turn.
    Started,
    Queued,
    Steered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteError {
    pub code: RemoteErrorCode,
    pub message: String,
    /// Set with `incompatible_version`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_versions: Vec<u32>,
}

impl RemoteError {
    pub fn new(code: RemoteErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            supported_versions: Vec::new(),
        }
    }

    pub fn incompatible_version(requested: u64) -> Self {
        Self {
            code: RemoteErrorCode::IncompatibleVersion,
            message: format!(
                "protocol version {requested} is not supported; this host speaks version {REMOTE_PROTOCOL_VERSION}"
            ),
            supported_versions: vec![REMOTE_PROTOCOL_VERSION],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteErrorCode {
    IncompatibleVersion,
    /// The frame is not a well-formed request for its operation.
    InvalidRequest,
    /// Unknown operation, a slash command, or steering a turn that does
    /// not accept it.
    UnsupportedOperation,
    Unauthorized,
    /// No such shared session, content or request.
    NotFound,
    /// `session_id` or `registration_epoch` no longer names the live
    /// registration.
    StaleSession,
    /// `submit_prompt` while a turn is running.
    Busy,
    /// `steer` or `queue` while no turn is running.
    NotRunning,
    StaleTurn,
    StaleQuestion,
    /// The question is pending but the answer does not fit it.
    InvalidAnswer,
    StaleApproval,
    /// A history cursor from a transcript that has since been rewritten.
    StaleCursor,
    /// `request_id` was already used with a different payload.
    RequestConflict,
    /// The owner did not answer. Sent with receipt `unknown`: the mutation
    /// may or may not have been applied.
    OwnerUnavailable,
    /// The owner cannot record another receipt; nothing was applied.
    ReceiptCapacity,
    FrameTooLarge,
    RateLimited,
    Internal,
}

/// One applied update of one registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteEventFrame {
    pub protocol_version: u32,
    pub session_id: String,
    pub registration_epoch: u64,
    /// Increases by exactly one per event within a registration. A gap means
    /// the client must attach again.
    pub sequence: u64,
    /// Controller generation the event belongs to; not an ordering key.
    pub generation: u64,
    pub event: RemoteEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteEvent {
    /// `turn_id` is absent only when the turn had already ended by the time
    /// the event was projected.
    TurnStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<RemoteTurnTiming>,
        prompt: BoundedText,
    },
    TextDelta {
        text: String,
        /// Optional current logical-turn work and assistant-segment thought timing.
        /// An empty text carries a clock update without appending content.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<RemoteTurnTiming>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_time_ms: Option<u64>,
        /// Estimated from observed reasoning text; absent when unavailable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_tokens_estimated: Option<bool>,
    },
    ToolStarted {
        tool: RemoteTool,
    },
    ToolFinished {
        id: String,
        success: bool,
        /// The tool handed off to a background task and has no result yet.
        pending: bool,
        content: BoundedText,
    },
    SubagentUpdated {
        subagent: RemoteSubagent,
    },
    ApprovalRequested {
        approval: RemoteApprovalBatch,
    },
    /// The batch left the pending state, whoever resolved it.
    ApprovalResolved {
        batch_id: String,
    },
    QuestionRequested {
        question: RemoteQuestion,
    },
    QuestionResolved {
        question_id: String,
    },
    TurnFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<RemoteTurnTiming>,
    },
    TurnCancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timing: Option<RemoteTurnTiming>,
    },
    /// Replaces all client state for the session.
    Snapshot {
        snapshot: Box<RemoteSnapshot>,
    },
    /// Events were lost for this subscriber. A `snapshot` event or a fresh
    /// `attach_session` restores state; nothing in between can be trusted.
    ResyncRequired {
        reason: ResyncReason,
    },
    /// The registration ended. The session ID is private again until it is
    /// shared anew.
    SessionClosed {
        reason: SessionCloseReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    /// The subscriber fell behind the replay ring.
    Lagged,
    /// The requested range is no longer contiguous.
    SequenceGap,
    GatewayRestarted,
    /// The transcript was rewritten; history cursors are invalid.
    HistoryChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionCloseReason {
    SharingDisabled,
    /// The terminal moved to another session.
    SessionChanged,
    OwnerExited,
    DeviceRevoked,
}

/// Pushed after `subscribe_sessions` whenever the live list changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSessionsFrame {
    pub protocol_version: u32,
    pub gateway_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    pub sessions: Vec<RemoteSessionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSessionInfo {
    pub session_id: String,
    pub registration_epoch: u64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    pub model: String,
    /// Total logical turns across the full host transcript, including an active turn.
    /// Absent when an archived prefix prevents an exact total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    pub activity: SessionActivity,
    pub attention: RemoteAttention,
    pub health: OwnerHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionActivity {
    Idle,
    Running,
    AwaitingApproval,
    AwaitingQuestion,
}

/// What is waiting on the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteAttention {
    pub approval: bool,
    pub question: bool,
}

/// Liveness of the terminal that owns the session, as the gateway sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerHealth {
    Live,
    /// Heartbeats are late; commands may not be answered.
    Unresponsive,
}

/// Text that may have been cut to fit a frame. `text` is the byte range
/// `offset..offset + text.len()` of the full content, cut on character
/// boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoundedText {
    pub text: String,
    pub truncated: bool,
    pub offset: u64,
    pub total_bytes: u64,
    /// Pass to `get_content` for the rest. Absent when the content cannot
    /// be fetched later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteContentChunk {
    pub content_id: String,
    pub offset: u64,
    pub text: String,
    pub total_bytes: u64,
    /// Offset of the next chunk; absent at the end of the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<u64>,
}

/// Safe session controls: never contains endpoint URLs or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSessionSettings {
    pub models: Vec<RemoteModelOption>,
    pub selected_model: String,
    /// `default` means omit the provider effort override, not disable thinking.
    pub reasoning_effort: String,
    pub can_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteModelOption {
    /// Exact configured profile name, including account-qualified names.
    pub id: String,
    pub model: String,
    /// `default` plus only explicitly known supported values.
    pub reasoning_efforts: Vec<String>,
}

/// Bounded view of one session. `sequence` is the exact watermark: the
/// snapshot already contains every event up to and including it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSnapshot {
    /// Optional operations implemented by this session owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<RemoteSessionSettings>,
    pub session: RemoteSessionInfo,
    /// Opaque gateway identity for this snapshot cut, including replacements
    /// at the same sequence. Store alongside the reconnect cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    pub sequence: u64,
    pub generation: u64,
    /// Present while a prompt is being run. Absent in the short gap between
    /// a prompt being accepted and its turn starting, even though
    /// `session.activity` is already `running`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<RemoteTurn>,
    /// Latest terminal logical turn, including turns with no assistant answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<RemoteTurnTiming>,
    /// Transcript tail, oldest first.
    pub transcript: Vec<RemoteMessage>,
    /// Identifies the transcript the message IDs and cursors refer to.
    pub history_revision: String,
    /// Fetches the page before `transcript`; absent when nothing is older.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<RemoteQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval: Option<RemoteApprovalBatch>,
    pub pending_prompts: Vec<RemotePendingPrompt>,
    pub subagents: Vec<RemoteSubagent>,
    pub background_tasks: Vec<RemoteBackgroundTask>,
    /// Counts of entries left out of the lists above to fit the frame.
    pub omitted: RemoteOmitted,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteOmitted {
    pub tools: u32,
    pub subagents: u32,
    pub pending_prompts: u32,
    pub background_tasks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteTurn {
    /// Name this in `cancel_turn`.
    pub turn_id: String,
    /// Tail of the response text streamed so far.
    pub live_response: BoundedText,
    pub tools: Vec<RemoteTool>,
    pub can_steer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<RemoteTurnTiming>,
    /// Aggregate across thinking blocks in the current assistant segment only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_time_ms: Option<u64>,
    /// Estimated from observed reasoning text; absent when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens_estimated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteTurnTiming {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    /// Authoritative monotonic work time; excludes user waits and gaps between runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_work_ms: Option<u64>,
    /// Absent while active or suspended. A failed turn uses `turn_finished` too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RemoteTurnOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTurnOutcome {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteTool {
    pub id: String,
    pub name: String,
    /// Short human label, e.g. the file or command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub state: RemoteToolState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteToolState {
    /// Still streaming from the model; not executing yet.
    Preparing,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteSubagent {
    pub id: u32,
    pub name: String,
    pub task: BoundedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub status: RemoteSubagentStatus,
    pub active_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub message_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSubagentStatus {
    Queued,
    Running,
    Interrupted,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteBackgroundTask {
    pub id: String,
    pub command: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteMessage {
    /// Stable within `history_revision`.
    pub message_id: String,
    pub role: String,
    pub content: BoundedText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<RemoteMessageTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_time_ms: Option<u64>,
    /// Estimated from observed reasoning text; absent when unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens_estimated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// Correlation and footer for this message's logical turn, repeated across phases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<RemoteTurnTiming>,
}

/// Set on a tool-result message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteMessageTool {
    pub name: String,
    /// Short human label, e.g. the file or command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub success: bool,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteHistoryPage {
    pub history_revision: String,
    /// Oldest first.
    pub messages: Vec<RemoteMessage>,
    /// Fetches the page before this one; absent at the start of history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteQuestion {
    /// Name this in `answer_question`.
    pub question_id: String,
    pub header: String,
    pub text: String,
    pub options: Vec<RemoteQuestionOption>,
    /// Whether more than one option may be selected.
    pub multiple: bool,
    /// 1-based position within a chain of questions from one tool call.
    pub position: u32,
    pub chain_length: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteApprovalBatch {
    /// Name this in `resolve_approval`.
    pub batch_id: String,
    pub actions: Vec<RemoteApprovalAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteApprovalAction {
    pub tool_name: String,
    pub summary: String,
    pub risk: String,
    /// The literal arguments under review. When truncated, the rest must be
    /// fetched before the batch is resolved; a preview is not the action.
    pub details: BoundedText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemotePendingPrompt {
    pub kind: RemotePendingPromptKind,
    pub text: BoundedText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemotePendingPromptKind {
    Steer,
    Queue,
}

/// Decode and validate one client frame. Every failure is already the
/// response to send back, correlated when the frame carried a request ID.
/// The version is checked before the operation is parsed, so a newer client
/// is told `incompatible_version` rather than `invalid_request`.
pub fn decode_request(frame: &str) -> Result<RemoteRequest, Box<RemoteResponse>> {
    let reject = |request_id: &str, code, message: String| {
        Box::new(RemoteResponse::error(
            request_id,
            RemoteError::new(code, message),
        ))
    };
    if frame.len() > MAX_REMOTE_FRAME_BYTES {
        return Err(reject(
            "",
            RemoteErrorCode::FrameTooLarge,
            format!("frame exceeds {MAX_REMOTE_FRAME_BYTES} bytes"),
        ));
    }
    let value: serde_json::Value = serde_json::from_str(frame).map_err(|error| {
        reject(
            "",
            RemoteErrorCode::InvalidRequest,
            format!("frame is not JSON: {error}"),
        )
    })?;
    let request_id = value
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= MAX_REQUEST_ID_BYTES)
        .unwrap_or_default()
        .to_owned();
    let Some(version) = value
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
    else {
        return Err(reject(
            &request_id,
            RemoteErrorCode::InvalidRequest,
            "protocol_version is missing".to_owned(),
        ));
    };
    if version != u64::from(REMOTE_PROTOCOL_VERSION) {
        return Err(Box::new(RemoteResponse::error(
            request_id,
            RemoteError::incompatible_version(version),
        )));
    }
    if request_id.is_empty() {
        return Err(reject(
            "",
            RemoteErrorCode::InvalidRequest,
            format!("request_id must be 1 to {MAX_REQUEST_ID_BYTES} bytes"),
        ));
    }
    if let Some(name) = value
        .get("operation")
        .and_then(|operation| operation.get("type"))
        .and_then(serde_json::Value::as_str)
        && !RemoteOperation::NAMES.contains(&name)
    {
        return Err(reject(
            &request_id,
            RemoteErrorCode::UnsupportedOperation,
            format!("operation `{name}` is not supported in protocol version {version}"),
        ));
    }
    let request: RemoteRequest = serde_json::from_value(value).map_err(|error| {
        reject(
            &request_id,
            RemoteErrorCode::InvalidRequest,
            error.to_string(),
        )
    })?;
    if request.operation.targets_session()
        && (request.session_id.is_none() || request.registration_epoch.is_none())
    {
        return Err(reject(
            &request_id,
            RemoteErrorCode::InvalidRequest,
            format!(
                "`{}` requires session_id and registration_epoch",
                request.operation.name()
            ),
        ));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(version: u32, operation: serde_json::Value) -> String {
        serde_json::json!({
            "protocol_version": version,
            "request_id": "r-1",
            "session_id": "s-1",
            "registration_epoch": 4,
            "operation": operation,
        })
        .to_string()
    }

    fn rejection(frame: &str) -> (String, RemoteError) {
        let response = decode_request(frame).expect_err("frame must be rejected");
        let RemoteResult::Error(error) = response.result else {
            panic!("rejections are error results");
        };
        (response.request_id, error)
    }

    #[test]
    fn version_mismatch_is_rejected_before_the_operation_is_parsed() {
        // The operation is one this build has never heard of: a newer client
        // must still learn that the version, not the operation, is the problem.
        let (request_id, error) = rejection(&frame(
            2,
            serde_json::json!({"type": "from_the_future", "payload": [1, 2]}),
        ));
        assert_eq!(request_id, "r-1");
        assert_eq!(error.code, RemoteErrorCode::IncompatibleVersion);
        assert_eq!(error.supported_versions, [REMOTE_PROTOCOL_VERSION]);

        let (_, older) = rejection(&frame(0, serde_json::json!({"type": "list_sessions"})));
        assert_eq!(older.code, RemoteErrorCode::IncompatibleVersion);
    }

    #[test]
    fn unknown_operations_are_unsupported_not_malformed() {
        let (request_id, error) = rejection(&frame(
            REMOTE_PROTOCOL_VERSION,
            serde_json::json!({"type": "run_slash_command", "command": "/exit"}),
        ));
        assert_eq!(request_id, "r-1");
        assert_eq!(error.code, RemoteErrorCode::UnsupportedOperation);
    }

    #[test]
    fn malformed_frames_are_invalid_requests() {
        let (request_id, error) = rejection("not json");
        assert_eq!(request_id, "");
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);

        // Known operation, missing its identity.
        let (request_id, error) = rejection(&frame(
            REMOTE_PROTOCOL_VERSION,
            serde_json::json!({"type": "cancel_turn"}),
        ));
        assert_eq!(request_id, "r-1");
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);

        let missing_version = serde_json::json!({
            "request_id": "r-1",
            "operation": {"type": "list_sessions"},
        });
        let (_, error) = rejection(&missing_version.to_string());
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);

        let oversized = " ".repeat(MAX_REMOTE_FRAME_BYTES + 1);
        assert_eq!(rejection(&oversized).1.code, RemoteErrorCode::FrameTooLarge);
    }

    #[test]
    fn session_operations_require_the_session_envelope() {
        let without_session = serde_json::json!({
            "protocol_version": REMOTE_PROTOCOL_VERSION,
            "request_id": "r-2",
            "operation": {"type": "cancel_turn", "turn_id": "turn:1:1"},
        });
        let (request_id, error) = rejection(&without_session.to_string());
        assert_eq!(request_id, "r-2");
        assert_eq!(error.code, RemoteErrorCode::InvalidRequest);

        let host_scoped = serde_json::json!({
            "protocol_version": REMOTE_PROTOCOL_VERSION,
            "request_id": "r-3",
            "operation": {"type": "list_sessions"},
        });
        let request = decode_request(&host_scoped.to_string()).expect("host operation");
        assert_eq!(request.operation, RemoteOperation::ListSessions);
        assert_eq!(request.session_id, None);
    }

    #[test]
    fn operation_names_match_the_wire_tags() {
        let operations = [
            RemoteOperation::ListSessions,
            RemoteOperation::SubscribeSessions,
            RemoteOperation::AttachSession { resume: None },
            RemoteOperation::DetachSession,
            RemoteOperation::GetHistory {
                cursor: None,
                limit: 1,
            },
            RemoteOperation::GetContent {
                content_id: String::new(),
                offset: 0,
                max_bytes: 1,
            },
            RemoteOperation::SubmitPrompt {
                prompt: String::new(),
            },
            RemoteOperation::Steer {
                prompt: String::new(),
            },
            RemoteOperation::Queue {
                prompt: String::new(),
            },
            RemoteOperation::CancelQuestion {
                question_id: String::new(),
            },
            RemoteOperation::ExecuteCommand {
                command: String::new(),
            },
            RemoteOperation::CancelTurn {
                turn_id: String::new(),
            },
            RemoteOperation::AnswerQuestion {
                question_id: String::new(),
                answer: RemoteAnswer::Custom {
                    text: String::new(),
                },
            },
            RemoteOperation::ResolveApproval {
                batch_id: String::new(),
                choice: ApprovalChoice::Deny,
            },
            RemoteOperation::GetRequestStatus {
                target_request_id: String::new(),
            },
            RemoteOperation::SetSessionSettings {
                model: String::new(),
                reasoning_effort: "default".into(),
            },
        ];
        assert_eq!(operations.len(), RemoteOperation::NAMES.len());
        for (operation, name) in operations.iter().zip(RemoteOperation::NAMES) {
            assert_eq!(operation.name(), name);
            assert_eq!(serde_json::to_value(operation).unwrap()["type"], name);
        }
    }
}
