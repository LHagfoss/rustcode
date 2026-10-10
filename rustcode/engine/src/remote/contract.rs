//! The committed contract: JSON Schema and golden frames.
//!
//! Both are generated from the wire types in [`super::protocol`] and
//! committed under `docs/remote-protocol/v1/`, where a client in another
//! repository builds its own types against the schema and decodes the golden
//! frames in its tests. The tests below regenerate everything and fail on any
//! difference, so the wire cannot change without the contract changing in
//! the same commit. To update the committed files after an intended change:
//!
//! ```text
//! RUSTCODE_UPDATE_REMOTE_PROTOCOL=1 cargo test --lib remote::contract
//! ```

use crate::controller::ApprovalChoice;

use crate::remote_gateway::handshake::{
    ErrorCode, HANDSHAKE_PROTOCOL_VERSION, HandshakeRequest, HandshakeResponse, PairingMethod,
    Secret,
};
use crate::remote_gateway::pairing::QrPayload;

use super::protocol::*;

/// Directory of the committed contract, relative to the repository root.
pub const CONTRACT_DIR: &str = "docs/remote-protocol/v1";

/// Pretty JSON with object keys sorted, so the committed schema does not
/// depend on how `serde_json` happens to order maps in a given build.
fn sorted_json(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, depth: usize, out: &mut String) {
        let indent = "  ".repeat(depth + 1);
        let close = "  ".repeat(depth);
        match value {
            serde_json::Value::Object(map) if !map.is_empty() => {
                let mut keys = map.keys().collect::<Vec<_>>();
                keys.sort();
                out.push_str("{\n");
                for (index, key) in keys.iter().enumerate() {
                    let separator = if index + 1 < keys.len() { "," } else { "" };
                    out.push_str(&indent);
                    out.push_str(&serde_json::Value::from(key.as_str()).to_string());
                    out.push_str(": ");
                    write(&map[*key], depth + 1, out);
                    out.push_str(separator);
                    out.push('\n');
                }
                out.push_str(&close);
                out.push('}');
            }
            serde_json::Value::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    let separator = if index + 1 < items.len() { "," } else { "" };
                    out.push_str(&indent);
                    write(item, depth + 1, out);
                    out.push_str(separator);
                    out.push('\n');
                }
                out.push_str(&close);
                out.push(']');
            }
            scalar_or_empty => out.push_str(&scalar_or_empty.to_string()),
        }
    }
    let mut out = String::new();
    write(value, 0, &mut out);
    out.push('\n');
    out
}

/// `(file name, contents)` of the schema for each direction.
pub fn schema_documents() -> Vec<(&'static str, String)> {
    let document = |schema: schemars::Schema| sorted_json(schema.as_value());
    vec![
        (
            "request.schema.json",
            document(schemars::schema_for!(RemoteRequest)),
        ),
        (
            "frame.schema.json",
            document(schemars::schema_for!(RemoteFrame)),
        ),
        (
            "handshake-request.schema.json",
            document(schemars::schema_for!(HandshakeRequest)),
        ),
        (
            "handshake-response.schema.json",
            document(schemars::schema_for!(HandshakeResponse)),
        ),
        (
            "pairing-qr.schema.json",
            document(schemars::schema_for!(QrPayload)),
        ),
    ]
}

/// The first frame a device sends, one example per shape. The secrets are
/// examples; no gateway ever issued them.
pub fn golden_handshake_requests() -> Vec<(&'static str, HandshakeRequest)> {
    vec![
        (
            "pair_credential",
            HandshakeRequest::Pair {
                protocol_version: HANDSHAKE_PROTOCOL_VERSION,
                method: PairingMethod::Credential,
                secret: Secret::new("q0lYc1o3bXJ3T2d5d0l2Rk5kU2tqeTZqZ0Z2ZUo0V2s"),
                device_name: "Lars's iPhone".to_owned(),
            },
        ),
        (
            "pair_code",
            HandshakeRequest::Pair {
                protocol_version: HANDSHAKE_PROTOCOL_VERSION,
                method: PairingMethod::Code,
                secret: Secret::new("4821-9034"),
                device_name: "Lars's iPhone".to_owned(),
            },
        ),
        (
            "authenticate",
            HandshakeRequest::Authenticate {
                protocol_version: HANDSHAKE_PROTOCOL_VERSION,
                device_id: "0011223344556677".to_owned(),
                token: Secret::new("ZXhhbXBsZS1kZXZpY2UtdG9rZW4tbm90LXJlYWw"),
            },
        ),
    ]
}

/// The handshake answers and one bare error frame per code.
pub fn golden_handshake_responses() -> Vec<(String, HandshakeResponse)> {
    let mut frames = vec![
        (
            "paired".to_owned(),
            HandshakeResponse::Paired {
                protocol_version: HANDSHAKE_PROTOCOL_VERSION,
                gateway_id: GATEWAY.to_owned(),
                instance_id: INSTANCE.to_owned(),
                device_id: "0011223344556677".to_owned(),
                device_name: "Lars's iPhone".to_owned(),
                token: Secret::new("ZXhhbXBsZS1kZXZpY2UtdG9rZW4tbm90LXJlYWw"),
                host_name: Some("studio".to_owned()),
            },
        ),
        (
            "authenticated".to_owned(),
            HandshakeResponse::Authenticated {
                protocol_version: HANDSHAKE_PROTOCOL_VERSION,
                gateway_id: GATEWAY.to_owned(),
                instance_id: INSTANCE.to_owned(),
                device_id: "0011223344556677".to_owned(),
                device_name: "Lars's iPhone".to_owned(),
                host_name: Some("studio".to_owned()),
            },
        ),
    ];
    for code in [
        ErrorCode::InvalidFrame,
        ErrorCode::FrameTooLarge,
        ErrorCode::UnsupportedVersion,
        ErrorCode::HandshakeTimeout,
        ErrorCode::PairingFailed,
        ErrorCode::RateLimited,
        ErrorCode::Unauthorized,
        ErrorCode::Busy,
        ErrorCode::Revoked,
        ErrorCode::SlowConsumer,
        ErrorCode::IdleTimeout,
        ErrorCode::ShuttingDown,
        ErrorCode::NotImplemented,
        ErrorCode::Internal,
    ] {
        let name = serde_json::to_value(code).expect("codes serialize");
        frames.push((
            format!("error_{}", name.as_str().expect("codes are strings")),
            HandshakeResponse::Error {
                code,
                message: "why the connection is being closed".to_owned(),
                retry_after_secs: (code == ErrorCode::RateLimited).then_some(240),
            },
        ));
    }
    frames
}

/// What a pairing QR code encodes.
pub fn golden_qr_payload() -> QrPayload {
    QrPayload {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        address: "100.92.13.44:17879".to_owned(),
        gateway_id: GATEWAY.to_owned(),
        credential: Secret::new("q0lYc1o3bXJ3T2d5d0l2Rk5kU2tqeTZqZ0Z2ZUo0V2s"),
        host_name: Some("studio".to_owned()),
    }
}

fn request(
    request_id: &str,
    session: Option<(&str, u64)>,
    operation: RemoteOperation,
) -> RemoteRequest {
    RemoteRequest {
        protocol_version: REMOTE_PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        session_id: session.map(|(session_id, _)| session_id.to_owned()),
        registration_epoch: session.map(|(_, epoch)| epoch),
        operation,
    }
}

const SESSION: (&str, u64) = ("5f0c2d9e-7a41-4c1b-9d55-0b6c1f2a3e44", 3);
const GATEWAY: &str = "gw-8e1f4b7a";
const INSTANCE: &str = "inst-41c07d92";
const TURN: &str = "turn:48213:12";
const QUESTION: &str = "question:48213:4";
const BATCH: &str = "controller:48213:7";

/// One example of every request shape, as `(name, request)`.
pub fn golden_requests() -> Vec<(&'static str, RemoteRequest)> {
    let session = Some(SESSION);
    vec![
        (
            "set_session_settings",
            request(
                "req-0016",
                session,
                RemoteOperation::SetSessionSettings {
                    model: "example-profile".into(),
                    reasoning_effort: "high".into(),
                },
            ),
        ),
        (
            "list_sessions",
            request("req-0001", None, RemoteOperation::ListSessions),
        ),
        (
            "subscribe_sessions",
            request("req-0002", None, RemoteOperation::SubscribeSessions),
        ),
        (
            "attach_session",
            request(
                "req-0003",
                session,
                RemoteOperation::AttachSession { resume: None },
            ),
        ),
        (
            "attach_session_resume",
            request(
                "req-0004",
                session,
                RemoteOperation::AttachSession {
                    resume: Some(ResumeCursor {
                        gateway_id: GATEWAY.to_owned(),
                        snapshot_id: Some("snapshot-example".to_owned()),
                        instance_id: Some(INSTANCE.to_owned()),
                        last_sequence: 412,
                    }),
                },
            ),
        ),
        (
            "detach_session",
            request("req-0005", session, RemoteOperation::DetachSession),
        ),
        (
            "get_history",
            request(
                "req-0006",
                session,
                RemoteOperation::GetHistory {
                    cursor: Some("60@h2".to_owned()),
                    limit: 25,
                },
            ),
        ),
        (
            "get_content",
            request(
                "req-0007",
                session,
                RemoteOperation::GetContent {
                    content_id: format!("approval:{BATCH}:0"),
                    offset: 16384,
                    max_bytes: 65536,
                },
            ),
        ),
        (
            "submit_prompt",
            request(
                "req-0008",
                session,
                RemoteOperation::SubmitPrompt {
                    prompt: "Run the test suite and fix what fails".to_owned(),
                },
            ),
        ),
        (
            "steer",
            request(
                "req-0009",
                session,
                RemoteOperation::Steer {
                    prompt: "Skip the flaky network test".to_owned(),
                },
            ),
        ),
        (
            "queue",
            request(
                "req-0010",
                session,
                RemoteOperation::Queue {
                    prompt: "Then open a pull request".to_owned(),
                },
            ),
        ),
        (
            "cancel_turn",
            request(
                "req-0011",
                session,
                RemoteOperation::CancelTurn {
                    turn_id: TURN.to_owned(),
                },
            ),
        ),
        (
            "answer_question_selected",
            request(
                "req-0012",
                session,
                RemoteOperation::AnswerQuestion {
                    question_id: QUESTION.to_owned(),
                    answer: RemoteAnswer::Selected {
                        options: vec!["Rebase onto main".to_owned()],
                    },
                },
            ),
        ),
        (
            "answer_question_custom",
            request(
                "req-0013",
                session,
                RemoteOperation::AnswerQuestion {
                    question_id: QUESTION.to_owned(),
                    answer: RemoteAnswer::Custom {
                        text: "Leave the branch as it is".to_owned(),
                    },
                },
            ),
        ),
        (
            "resolve_approval",
            request(
                "req-0014",
                session,
                RemoteOperation::ResolveApproval {
                    batch_id: BATCH.to_owned(),
                    choice: ApprovalChoice::Approve,
                },
            ),
        ),
        (
            "get_request_status",
            request(
                "req-0015",
                None,
                RemoteOperation::GetRequestStatus {
                    target_request_id: "req-0008".to_owned(),
                },
            ),
        ),
    ]
}

fn whole(text: &str, content_id: Option<&str>) -> BoundedText {
    BoundedText {
        text: text.to_owned(),
        truncated: false,
        offset: 0,
        total_bytes: text.len() as u64,
        content_id: content_id.map(str::to_owned),
    }
}

fn golden_settings() -> RemoteSessionSettings {
    RemoteSessionSettings {
        models: vec![RemoteModelOption {
            id: "example-profile".into(),
            model: "gpt-5.5".into(),
            reasoning_efforts: vec!["default".into(), "high".into()],
        }],
        selected_model: "example-profile".into(),
        reasoning_effort: "high".into(),
        can_change: false,
    }
}

fn golden_session_info() -> RemoteSessionInfo {
    RemoteSessionInfo {
        session_id: SESSION.0.to_owned(),
        registration_epoch: SESSION.1,
        title: "Fix the failing parser tests".to_owned(),
        workspace: Some("/Users/dev/code/parser".to_owned()),
        model: "gpt-5.5".to_owned(),
        turn_count: Some(2),
        activity: SessionActivity::AwaitingApproval,
        attention: RemoteAttention {
            approval: true,
            question: false,
        },
        health: OwnerHealth::Live,
    }
}

fn golden_question() -> RemoteQuestion {
    RemoteQuestion {
        question_id: QUESTION.to_owned(),
        header: "Branch".to_owned(),
        text: "The branch is behind main. How should I continue?".to_owned(),
        options: vec![
            RemoteQuestionOption {
                label: "Rebase onto main".to_owned(),
                description: Some("Replays the two local commits".to_owned()),
            },
            RemoteQuestionOption {
                label: "Merge main".to_owned(),
                description: None,
            },
        ],
        multiple: false,
        position: 1,
        chain_length: 2,
    }
}

fn golden_approval() -> RemoteApprovalBatch {
    RemoteApprovalBatch {
        batch_id: BATCH.to_owned(),
        actions: vec![
            RemoteApprovalAction {
                tool_name: "run_command".to_owned(),
                summary: "run_command · cargo test --workspace".to_owned(),
                risk: "This action requires your approval before it can continue.".to_owned(),
                details: whole(
                    r#"{"command":"cargo test --workspace"}"#,
                    Some("approval:controller:48213:7:0"),
                ),
            },
            RemoteApprovalAction {
                tool_name: "write_to_file".to_owned(),
                summary: "write_to_file · src/parser.rs".to_owned(),
                risk: "This action requires your approval before it can continue.".to_owned(),
                details: BoundedText {
                    text: r#"{"path":"src/parser.rs","content":"use std::collections::HashMap;\n"#
                        .to_owned(),
                    truncated: true,
                    offset: 0,
                    total_bytes: 48211,
                    content_id: Some("approval:controller:48213:7:1".to_owned()),
                },
            },
        ],
    }
}

fn golden_subagent() -> RemoteSubagent {
    RemoteSubagent {
        id: 2,
        name: "explorer".to_owned(),
        task: whole("Find every caller of parse_expression", None),
        model: Some("gpt-5.5-mini".to_owned()),
        status: RemoteSubagentStatus::Running,
        active_turn: true,
        parent_id: None,
        depth: 1,
        message_count: 6,
    }
}

fn golden_tool() -> RemoteTool {
    RemoteTool {
        id: "call_Jq3".to_owned(),
        name: "view_file".to_owned(),
        detail: Some("src/parser.rs (lines 1-80)".to_owned()),
        state: RemoteToolState::Running,
    }
}

fn golden_messages() -> Vec<RemoteMessage> {
    vec![
        RemoteMessage {
            message_id: "m59".to_owned(),
            role: "assistant".to_owned(),
            content: whole("<think>Check both paths</think>Done", Some("message:h2:59")),
            tool: None,
            timestamp: Some("2026-10-09T18:40:25+02:00".to_owned()),
            response_time_ms: Some(12000),
            thought_time_ms: Some(4000),
            thought_tokens: Some(800),
            thought_tokens_estimated: Some(true),
            completed_at: Some("2026-10-09T18:40:32+02:00".to_owned()),
            turn: Some(golden_previous_timing()),
        },
        RemoteMessage {
            message_id: "m60".to_owned(),
            role: "user".to_owned(),
            content: whole("Why does the parser test fail?", Some("message:h2:60")),
            tool: None,
            timestamp: Some("2026-10-09T18:41:00+02:00".to_owned()),
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            thought_tokens_estimated: None,
            completed_at: None,
            turn: None,
        },
        RemoteMessage {
            message_id: "m61".to_owned(),
            role: "tool".to_owned(),
            content: BoundedText {
                text: "running 42 tests\ntest parse_empty ... ok\n".to_owned(),
                truncated: true,
                offset: 0,
                total_bytes: 9120,
                content_id: Some("message:h2:61".to_owned()),
            },
            tool: Some(RemoteMessageTool {
                name: "run_command".to_owned(),
                detail: Some("cargo test parser".to_owned()),
                success: false,
                pending: false,
            }),
            timestamp: Some("2026-10-09T18:41:10+02:00".to_owned()),
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            thought_tokens_estimated: None,
            completed_at: None,
            turn: None,
        },
    ]
}

fn golden_timing(outcome: Option<RemoteTurnOutcome>) -> RemoteTurnTiming {
    RemoteTurnTiming {
        turn_id: TURN.to_owned(),
        started_at: Some("2026-10-09T18:41:00+02:00".to_owned()),
        ended_at: outcome.map(|_| "2026-10-09T18:42:00+02:00".to_owned()),
        elapsed_work_ms: Some(if outcome.is_some() { 32000 } else { 12000 }),
        outcome,
    }
}

fn golden_previous_timing() -> RemoteTurnTiming {
    let mut timing = golden_timing(Some(RemoteTurnOutcome::Completed));
    timing.turn_id = "turn:previous".to_owned();
    timing.started_at = Some("2026-10-09T18:40:00+02:00".to_owned());
    timing.ended_at = Some("2026-10-09T18:40:32+02:00".to_owned());
    timing
}

fn golden_snapshot() -> RemoteSnapshot {
    RemoteSnapshot {
        settings: Some(golden_settings()),
        snapshot_id: Some("snapshot-example".to_owned()),
        session: golden_session_info(),
        sequence: 412,
        generation: 1,
        turn: Some(RemoteTurn {
            turn_id: TURN.to_owned(),
            live_response: BoundedText {
                text: "The failure comes from the tokenizer, not the parser.".to_owned(),
                truncated: true,
                offset: 40960,
                total_bytes: 41013,
                content_id: Some(format!("response:{TURN}")),
            },
            tools: vec![golden_tool()],
            can_steer: false,
            timing: Some(golden_timing(None)),
            thought_time_ms: Some(4000),
            thought_tokens: Some(800),
            thought_tokens_estimated: Some(true),
        }),
        transcript: golden_messages(),
        history_revision: "h2".to_owned(),
        history_cursor: Some("60@h2".to_owned()),
        pending_question: None,
        pending_approval: Some(golden_approval()),
        pending_prompts: vec![RemotePendingPrompt {
            kind: RemotePendingPromptKind::Queue,
            text: whole("Then open a pull request", None),
        }],
        subagents: vec![golden_subagent()],
        background_tasks: vec![RemoteBackgroundTask {
            id: "task-3".to_owned(),
            command: "cargo watch -x check".to_owned(),
            elapsed_ms: 93400,
        }],
        omitted: RemoteOmitted::default(),
        last_turn: Some(golden_previous_timing()),
    }
}

/// One example of every host frame shape, as `(name, frame)`.
pub fn golden_frames() -> Vec<(String, RemoteFrame)> {
    let response = |name: &str, request_id: &str, receipt, result| {
        let mut response = RemoteResponse::new(request_id, result);
        response.receipt = receipt;
        (format!("response_{name}"), RemoteFrame::Response(response))
    };
    let error = |code: RemoteErrorCode, receipt, message: &str| {
        let name = serde_json::to_value(code).expect("codes serialize");
        response(
            &format!("error_{}", name.as_str().expect("codes are strings")),
            "req-0100",
            receipt,
            RemoteResult::Error(RemoteError::new(code, message)),
        )
    };
    let event = |name: &str, sequence, event| {
        (
            format!("event_{name}"),
            RemoteFrame::Event(RemoteEventFrame {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                session_id: SESSION.0.to_owned(),
                registration_epoch: SESSION.1,
                sequence,
                generation: 1,
                event,
            }),
        )
    };
    let rejected = Some(ReceiptState::Rejected);
    let applied = Some(ReceiptState::Applied);
    vec![
        response(
            "session_settings_updated",
            "req-0016",
            Some(ReceiptState::Applied),
            RemoteResult::SessionSettingsUpdated {
                settings: RemoteSessionSettings {
                    can_change: true,
                    ..golden_settings()
                },
            },
        ),
        response(
            "sessions",
            "req-0001",
            None,
            RemoteResult::Sessions {
                gateway_id: GATEWAY.to_owned(),
                instance_id: Some(INSTANCE.to_owned()),
                sessions: vec![golden_session_info()],
                subscribed: false,
            },
        ),
        response(
            "attached",
            "req-0003",
            None,
            RemoteResult::Attached {
                gateway_id: GATEWAY.to_owned(),
                instance_id: Some(INSTANCE.to_owned()),
                resync: None,
                snapshot: Box::new(golden_snapshot()),
            },
        ),
        // A `resume` cursor that could not be replayed.
        response(
            "attached_resync",
            "req-0003",
            None,
            RemoteResult::Attached {
                gateway_id: GATEWAY.to_owned(),
                instance_id: Some(INSTANCE.to_owned()),
                resync: Some(ResyncReason::GatewayRestarted),
                snapshot: Box::new(golden_snapshot()),
            },
        ),
        response(
            "resumed",
            "req-0004",
            None,
            RemoteResult::Resumed {
                gateway_id: GATEWAY.to_owned(),
                instance_id: Some(INSTANCE.to_owned()),
                next_sequence: 413,
            },
        ),
        response("detached", "req-0005", None, RemoteResult::Detached),
        response(
            "history",
            "req-0006",
            None,
            RemoteResult::History(RemoteHistoryPage {
                history_revision: "h2".to_owned(),
                messages: golden_messages(),
                next_cursor: Some("35@h2".to_owned()),
            }),
        ),
        response(
            "content",
            "req-0007",
            None,
            RemoteResult::Content(RemoteContentChunk {
                content_id: format!("approval:{BATCH}:1"),
                offset: 16384,
                text: "    let mut table = HashMap::new();\n".to_owned(),
                total_bytes: 48211,
                next_offset: Some(16420),
            }),
        ),
        response(
            "prompt_accepted",
            "req-0008",
            applied,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started,
            },
        ),
        response(
            "turn_cancelled",
            "req-0011",
            applied,
            RemoteResult::TurnCancelled {
                turn_id: TURN.to_owned(),
            },
        ),
        response(
            "question_answered",
            "req-0012",
            applied,
            RemoteResult::QuestionAnswered {
                question_id: QUESTION.to_owned(),
            },
        ),
        response(
            "approval_resolved",
            "req-0014",
            applied,
            RemoteResult::ApprovalResolved {
                batch_id: BATCH.to_owned(),
                choice: ApprovalChoice::Approve,
            },
        ),
        response(
            "request_status",
            "req-0015",
            None,
            RemoteResult::RequestStatus {
                target_request_id: "req-0008".to_owned(),
                receipt: ReceiptState::Applied,
                result: Some(Box::new(RemoteResult::PromptAccepted {
                    disposition: PromptDisposition::Started,
                })),
            },
        ),
        response(
            "request_status_unknown",
            "req-0016",
            None,
            RemoteResult::RequestStatus {
                target_request_id: "req-0009".to_owned(),
                receipt: ReceiptState::Unknown,
                result: None,
            },
        ),
        response(
            "request_status_received",
            "req-0017",
            None,
            RemoteResult::RequestStatus {
                target_request_id: "req-0010".to_owned(),
                receipt: ReceiptState::Received,
                result: None,
            },
        ),
        response(
            "error_incompatible_version",
            "req-0100",
            None,
            RemoteResult::Error(RemoteError::incompatible_version(2)),
        ),
        error(
            RemoteErrorCode::UnsupportedOperation,
            rejected,
            "slash commands are not available remotely",
        ),
        error(
            RemoteErrorCode::Busy,
            rejected,
            "the session is running a turn; steer or queue instead",
        ),
        error(
            RemoteErrorCode::NotRunning,
            rejected,
            "no turn is running; submit a prompt instead",
        ),
        error(
            RemoteErrorCode::StaleSession,
            rejected,
            "the session is no longer shared under this registration",
        ),
        error(
            RemoteErrorCode::StaleTurn,
            rejected,
            "the named turn is no longer running",
        ),
        error(
            RemoteErrorCode::StaleQuestion,
            rejected,
            "the named question is no longer pending",
        ),
        error(
            RemoteErrorCode::InvalidAnswer,
            rejected,
            "`Squash` is not an option of this question",
        ),
        error(
            RemoteErrorCode::StaleApproval,
            rejected,
            "the named approval batch is no longer pending",
        ),
        error(
            RemoteErrorCode::StaleCursor,
            None,
            "the transcript changed; attach again for a current cursor",
        ),
        error(
            RemoteErrorCode::RequestConflict,
            rejected,
            "request_id was already used with a different payload",
        ),
        error(
            RemoteErrorCode::OwnerUnavailable,
            Some(ReceiptState::Unknown),
            "the terminal did not answer; check the transcript before sending again",
        ),
        error(
            RemoteErrorCode::InvalidRequest,
            None,
            "`cancel_turn` requires session_id and registration_epoch",
        ),
        event(
            "turn_started",
            413,
            RemoteEvent::TurnStarted {
                turn_id: Some(TURN.to_owned()),
                prompt: whole("Run the test suite and fix what fails", None),
                timing: Some(golden_timing(None)),
            },
        ),
        event(
            "text_delta",
            414,
            RemoteEvent::TextDelta {
                text: "The failure comes from ".to_owned(),
                timing: Some(golden_timing(None)),
                thought_time_ms: Some(4000),
                thought_tokens: Some(800),
                thought_tokens_estimated: Some(true),
            },
        ),
        event(
            "tool_started",
            415,
            RemoteEvent::ToolStarted {
                tool: golden_tool(),
            },
        ),
        event(
            "tool_finished",
            416,
            RemoteEvent::ToolFinished {
                id: "call_Jq3".to_owned(),
                success: true,
                pending: false,
                content: BoundedText {
                    text: "use std::collections::HashMap;\n".to_owned(),
                    truncated: true,
                    offset: 0,
                    total_bytes: 2890,
                    content_id: None,
                },
            },
        ),
        event(
            "subagent_updated",
            417,
            RemoteEvent::SubagentUpdated {
                subagent: golden_subagent(),
            },
        ),
        event(
            "approval_requested",
            418,
            RemoteEvent::ApprovalRequested {
                approval: golden_approval(),
            },
        ),
        event(
            "approval_resolved",
            419,
            RemoteEvent::ApprovalResolved {
                batch_id: BATCH.to_owned(),
            },
        ),
        event(
            "question_requested",
            420,
            RemoteEvent::QuestionRequested {
                question: golden_question(),
            },
        ),
        event(
            "question_resolved",
            421,
            RemoteEvent::QuestionResolved {
                question_id: QUESTION.to_owned(),
            },
        ),
        event(
            "turn_finished",
            422,
            RemoteEvent::TurnFinished {
                turn_id: Some(TURN.to_owned()),
                timing: Some(golden_timing(Some(RemoteTurnOutcome::Completed))),
            },
        ),
        event(
            "turn_cancelled",
            423,
            RemoteEvent::TurnCancelled {
                turn_id: Some(TURN.to_owned()),
                timing: Some(golden_timing(Some(RemoteTurnOutcome::Cancelled))),
            },
        ),
        event(
            "snapshot",
            424,
            RemoteEvent::Snapshot {
                snapshot: Box::new(RemoteSnapshot {
                    sequence: 424,
                    turn: None,
                    pending_approval: None,
                    pending_question: Some(golden_question()),
                    ..golden_snapshot()
                }),
            },
        ),
        event(
            "turn_failed",
            427,
            RemoteEvent::TurnFinished {
                turn_id: Some(TURN.to_owned()),
                timing: Some(golden_timing(Some(RemoteTurnOutcome::Failed))),
            },
        ),
        event(
            "clock_update",
            428,
            RemoteEvent::TextDelta {
                text: String::new(),
                timing: Some(golden_timing(None)),
                thought_time_ms: Some(4000),
                thought_tokens: Some(800),
                thought_tokens_estimated: Some(true),
            },
        ),
        event(
            "resync_required",
            425,
            RemoteEvent::ResyncRequired {
                reason: ResyncReason::Lagged,
            },
        ),
        event(
            "session_closed",
            426,
            RemoteEvent::SessionClosed {
                reason: SessionCloseReason::SharingDisabled,
            },
        ),
        (
            "sessions".to_owned(),
            RemoteFrame::Sessions(RemoteSessionsFrame {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                gateway_id: GATEWAY.to_owned(),
                instance_id: Some(INSTANCE.to_owned()),
                sessions: vec![
                    golden_session_info(),
                    RemoteSessionInfo {
                        session_id: "b3a9e6f0-1c2d-4e5f-8a7b-9c0d1e2f3a4b".to_owned(),
                        registration_epoch: 1,
                        title: "New session".to_owned(),
                        workspace: None,
                        model: "gpt-5.5".to_owned(),
                        turn_count: Some(0),
                        activity: SessionActivity::Idle,
                        attention: RemoteAttention {
                            approval: false,
                            question: false,
                        },
                        health: OwnerHealth::Unresponsive,
                    },
                ],
            }),
        ),
    ]
}

/// Every committed file as `(path relative to CONTRACT_DIR, contents)`.
pub fn contract_files() -> Vec<(String, String)> {
    fn pretty<T: serde::Serialize>(value: &T) -> String {
        let mut text = serde_json::to_string_pretty(value).expect("wire types serialize");
        text.push('\n');
        text
    }
    let mut files = schema_documents()
        .into_iter()
        .map(|(name, contents)| (name.to_owned(), contents))
        .collect::<Vec<_>>();
    files.extend(
        golden_requests()
            .iter()
            .map(|(name, request)| (format!("golden/requests/{name}.json"), pretty(request))),
    );
    files.extend(
        golden_frames()
            .iter()
            .map(|(name, frame)| (format!("golden/frames/{name}.json"), pretty(frame))),
    );
    files.extend(golden_handshake_requests().iter().map(|(name, frame)| {
        (
            format!("golden/handshake/requests/{name}.json"),
            pretty(frame),
        )
    }));
    files.extend(golden_handshake_responses().iter().map(|(name, frame)| {
        (
            format!("golden/handshake/responses/{name}.json"),
            pretty(frame),
        )
    }));
    files.push((
        "golden/handshake/pairing_qr.json".to_owned(),
        pretty(&golden_qr_payload()),
    ));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_v1_frames_decode_with_timing_absent() {
        let old: RemoteEvent =
            serde_json::from_str(r#"{"type":"turn_finished","turn_id":"old"}"#).unwrap();
        assert!(matches!(
            old,
            RemoteEvent::TurnFinished { timing: None, .. }
        ));
        let old: RemoteEvent =
            serde_json::from_str(r#"{"type":"text_delta","text":"hello"}"#).unwrap();
        assert!(matches!(
            old,
            RemoteEvent::TextDelta {
                timing: None,
                thought_time_ms: None,
                thought_tokens: None,
                thought_tokens_estimated: None,
                ..
            }
        ));
        let old: RemoteMessage = serde_json::from_value(serde_json::json!({
            "message_id": "m1", "role": "assistant", "content": {
                "text": "old answer", "truncated": false, "offset": 0, "total_bytes": 10
            }, "timestamp": "2026-01-01T10:00:00Z"
        }))
        .unwrap();
        let value = serde_json::to_value(old).unwrap();
        for key in [
            "turn",
            "response_time_ms",
            "thought_time_ms",
            "completed_at",
        ] {
            assert!(value.get(key).is_none());
        }
        let mut snapshot = serde_json::to_value(golden_snapshot()).unwrap();
        snapshot.as_object_mut().unwrap().remove("last_turn");
        snapshot.as_object_mut().unwrap().remove("settings");
        snapshot["session"]
            .as_object_mut()
            .unwrap()
            .remove("turn_count");
        let turn = snapshot["turn"].as_object_mut().unwrap();
        turn.remove("timing");
        turn.remove("thought_time_ms");
        let decoded: RemoteSnapshot = serde_json::from_value(snapshot).unwrap();
        assert!(decoded.last_turn.is_none());
        assert!(decoded.settings.is_none());
        assert!(decoded.session.turn_count.is_none());
        assert!(decoded.turn.unwrap().timing.is_none());
    }
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    const GOLDEN_DIRECTORIES: [&str; 4] = [
        "golden/requests",
        "golden/frames",
        "golden/handshake/requests",
        "golden/handshake/responses",
    ];

    fn contract_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_DIR)
    }

    fn committed(relative: &str) -> String {
        let path = contract_dir().join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn committed_json(directory: &str) -> Vec<(String, Value)> {
        let mut files = std::fs::read_dir(contract_dir().join(directory))
            .expect("golden directory is committed")
            .map(|entry| entry.expect("directory entry").file_name())
            .map(|name| format!("{directory}/{}", name.to_string_lossy()))
            .collect::<Vec<_>>();
        files.sort();
        files
            .into_iter()
            .map(|relative| {
                let value = serde_json::from_str(&committed(&relative))
                    .unwrap_or_else(|error| panic!("{relative}: {error}"));
                (relative, value)
            })
            .collect()
    }

    /// The drift check. Regenerates the schema and every golden frame from
    /// the Rust types and compares them with what is committed.
    #[test]
    fn committed_contract_matches_the_wire_types() {
        let files = contract_files();
        if std::env::var_os("RUSTCODE_UPDATE_REMOTE_PROTOCOL").is_some() {
            for directory in GOLDEN_DIRECTORIES {
                let directory = contract_dir().join(directory);
                let _ = std::fs::remove_dir_all(&directory);
                std::fs::create_dir_all(&directory).expect("create golden directory");
            }
            for (relative, contents) in &files {
                std::fs::write(contract_dir().join(relative), contents).expect("write contract");
            }
            return;
        }
        let hint = "run `RUSTCODE_UPDATE_REMOTE_PROTOCOL=1 cargo test --lib remote::contract` \
                    and commit docs/remote-protocol";
        for (relative, contents) in &files {
            let path = contract_dir().join(relative);
            let on_disk = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{relative} is not committed ({error}); {hint}"));
            // Compared as JSON so line endings and key order cannot produce
            // a false failure, while any change of content still does.
            let expected: Value = serde_json::from_str(contents).expect("generated JSON");
            let actual: Value = serde_json::from_str(&on_disk)
                .unwrap_or_else(|error| panic!("{relative} is not JSON ({error}); {hint}"));
            assert!(
                expected == actual,
                "{relative} no longer matches the wire types; {hint}"
            );
        }
        // A golden frame whose shape was removed must not linger.
        let generated = files
            .iter()
            .map(|(relative, _)| relative.clone())
            .collect::<BTreeSet<_>>();
        for directory in GOLDEN_DIRECTORIES {
            for (relative, _) in committed_json(directory) {
                assert!(
                    generated.contains(&relative),
                    "{relative} is committed but no longer generated; {hint}"
                );
            }
        }
    }

    #[test]
    fn every_golden_request_round_trips_and_decodes() {
        let requests = committed_json("golden/requests");
        assert!(!requests.is_empty());
        for (relative, value) in requests {
            let request: RemoteRequest = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{relative}: {error}"));
            assert_eq!(
                serde_json::to_value(&request).unwrap(),
                value,
                "{relative} does not survive a round trip"
            );
            let decoded = decode_request(&value.to_string())
                .unwrap_or_else(|rejection| panic!("{relative} was rejected: {rejection:?}"));
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn every_golden_frame_round_trips() {
        let frames = committed_json("golden/frames");
        assert!(!frames.is_empty());
        for (relative, value) in frames {
            let frame: RemoteFrame = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{relative}: {error}"));
            let encoded = serde_json::to_string(&frame).unwrap();
            assert!(encoded.len() <= MAX_REMOTE_FRAME_BYTES);
            assert_eq!(
                serde_json::from_str::<Value>(&encoded).unwrap(),
                value,
                "{relative} does not survive a round trip"
            );
        }
    }

    /// Validates `value` against the subset of JSON Schema the generator
    /// emits. An unknown keyword fails the test instead of being skipped, so
    /// a schema this cannot check is never reported as satisfied.
    fn validate(schema: &Value, value: &Value, root: &Value, at: &str) -> Result<(), String> {
        let schema = match schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return Err(format!("{at}: nothing is allowed here")),
            Value::Object(schema) => schema,
            other => panic!("schema is not an object: {other}"),
        };
        for (keyword, rule) in schema {
            match keyword.as_str() {
                "$schema" | "$defs" | "title" | "description" | "format" | "default" => {}
                "$ref" => {
                    let name = rule
                        .as_str()
                        .and_then(|reference| reference.strip_prefix("#/$defs/"))
                        .unwrap_or_else(|| panic!("unsupported $ref {rule}"));
                    validate(&root["$defs"][name], value, root, at)?;
                }
                "type" => {
                    let matches = |name: &str| match name {
                        "object" => value.is_object(),
                        "array" => value.is_array(),
                        "string" => value.is_string(),
                        "boolean" => value.is_boolean(),
                        "null" => value.is_null(),
                        "integer" => value.is_i64() || value.is_u64(),
                        "number" => value.is_number(),
                        other => panic!("unsupported type {other}"),
                    };
                    let allowed = match rule {
                        Value::String(name) => matches(name),
                        Value::Array(names) => names
                            .iter()
                            .any(|name| matches(name.as_str().expect("type names are strings"))),
                        other => panic!("unsupported type rule {other}"),
                    };
                    if !allowed {
                        return Err(format!("{at}: expected {rule}, found {value}"));
                    }
                }
                "const" => {
                    if value != rule {
                        return Err(format!("{at}: expected {rule}, found {value}"));
                    }
                }
                "enum" => {
                    if !rule.as_array().expect("enum is a list").contains(value) {
                        return Err(format!("{at}: {value} is not one of {rule}"));
                    }
                }
                "minimum" => {
                    if value.as_f64() < rule.as_f64() {
                        return Err(format!("{at}: {value} is below {rule}"));
                    }
                }
                "required" => {
                    for field in rule.as_array().expect("required is a list") {
                        let field = field.as_str().expect("field names are strings");
                        if value.get(field).is_none() {
                            return Err(format!("{at}: `{field}` is missing"));
                        }
                    }
                }
                "properties" => {
                    for (field, property) in rule.as_object().expect("properties is a map") {
                        if let Some(inner) = value.get(field) {
                            validate(property, inner, root, &format!("{at}.{field}"))?;
                        }
                    }
                }
                "additionalProperties" => {
                    let known = schema.get("properties").and_then(Value::as_object);
                    for (field, inner) in value.as_object().into_iter().flatten() {
                        if known.is_none_or(|known| !known.contains_key(field)) {
                            validate(rule, inner, root, &format!("{at}.{field}"))?;
                        }
                    }
                }
                "items" => {
                    for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                        validate(rule, item, root, &format!("{at}[{index}]"))?;
                    }
                }
                "anyOf" | "oneOf" => {
                    let branches = rule.as_array().expect("branches are a list");
                    let outcomes = branches
                        .iter()
                        .map(|branch| validate(branch, value, root, at))
                        .collect::<Vec<_>>();
                    let matched = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
                    if matched == 0 || (keyword == "oneOf" && matched != 1) {
                        return Err(format!(
                            "{at}: matched {matched} of {} branches: {outcomes:?}",
                            branches.len()
                        ));
                    }
                }
                other => panic!("the contract test cannot check schema keyword `{other}`"),
            }
        }
        Ok(())
    }

    fn schema(name: &str) -> Value {
        serde_json::from_str(&committed(name)).expect("schema is JSON")
    }

    #[test]
    fn every_golden_frame_satisfies_the_committed_schema() {
        for (schema_name, directory) in [
            ("request.schema.json", "golden/requests"),
            ("frame.schema.json", "golden/frames"),
            ("handshake-request.schema.json", "golden/handshake/requests"),
            (
                "handshake-response.schema.json",
                "golden/handshake/responses",
            ),
        ] {
            let schema = schema(schema_name);
            for (relative, value) in committed_json(directory) {
                if let Err(error) = validate(&schema, &value, &schema, "$") {
                    panic!("{relative} violates {schema_name}: {error}");
                }
            }
        }
    }

    #[test]
    fn the_schema_rejects_frames_that_are_not_on_the_wire() {
        let schema = schema("request.schema.json");
        let mut request = serde_json::to_value(
            &golden_requests()
                .into_iter()
                .find(|(name, _)| *name == "cancel_turn")
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(request["operation"]["type"], "cancel_turn");
        assert!(validate(&schema, &request, &schema, "$").is_ok());
        request["operation"]["turn_id"] = Value::Null;
        assert!(validate(&schema, &request, &schema, "$").is_err());
        request["operation"] = serde_json::json!({"type": "run_slash_command"});
        assert!(validate(&schema, &request, &schema, "$").is_err());
        let mut versionless = serde_json::to_value(&golden_requests()[0].1).unwrap();
        versionless
            .as_object_mut()
            .unwrap()
            .remove("protocol_version");
        assert!(validate(&schema, &versionless, &schema, "$").is_err());
    }

    /// Tag values of an internally tagged enum, read from the schema.
    fn tags(schema: &Value, definition: Option<&str>, tag: &str) -> BTreeSet<String> {
        let node = definition.map_or(schema, |name| &schema["$defs"][name]);
        node["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{definition:?} is not a tagged enum"))
            .iter()
            .map(|variant| {
                variant["properties"][tag]["const"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{definition:?} variant has no `{tag}` tag"))
                    .to_owned()
            })
            .collect()
    }

    /// Values of a unit-only enum, read from the schema.
    fn tags_of_enum(schema: &Value, definition: &str) -> BTreeSet<String> {
        let node = &schema["$defs"][definition];
        let plain = node["enum"].as_array().into_iter().flatten();
        let documented = node["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|variant| {
                variant["enum"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(variant.get("const"))
            });
        plain
            .chain(documented)
            .map(|value| value.as_str().expect("enum values are strings").to_owned())
            .collect()
    }

    fn strings(values: &[(String, Value)], pointer: &str) -> BTreeSet<String> {
        values
            .iter()
            .filter_map(|(_, value)| value.pointer(pointer)?.as_str().map(str::to_owned))
            .collect()
    }

    #[test]
    fn handshake_golden_frames_round_trip_and_cover_every_shape() {
        let requests = committed_json("golden/handshake/requests");
        for (relative, value) in &requests {
            let frame = HandshakeRequest::parse(&value.to_string())
                .unwrap_or_else(|rejection| panic!("{relative} was rejected: {rejection:?}"));
            assert_eq!(&serde_json::to_value(&frame).unwrap(), value, "{relative}");
        }
        let responses = committed_json("golden/handshake/responses");
        for (relative, value) in &responses {
            let frame: HandshakeResponse = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{relative}: {error}"));
            assert_eq!(&serde_json::to_value(&frame).unwrap(), value, "{relative}");
        }
        let request_schema = schema("handshake-request.schema.json");
        let response_schema = schema("handshake-response.schema.json");
        assert_eq!(
            strings(&requests, "/type"),
            tags(&request_schema, None, "type")
        );
        assert_eq!(
            strings(&responses, "/type"),
            tags(&response_schema, None, "type")
        );
        assert_eq!(
            strings(&responses, "/code"),
            tags_of_enum(&response_schema, "ErrorCode")
        );

        let qr: Value = serde_json::from_str(&committed("golden/handshake/pairing_qr.json"))
            .expect("QR payload is JSON");
        let qr_schema = schema("pairing-qr.schema.json");
        validate(&qr_schema, &qr, &qr_schema, "$").expect("QR payload satisfies its schema");
        let decoded: QrPayload = serde_json::from_value(qr.clone()).expect("QR payload decodes");
        assert_eq!(serde_json::to_value(&decoded).unwrap(), qr);
    }

    /// A shape added to the protocol must come with a golden frame.
    #[test]
    fn golden_frames_cover_every_shape_in_the_schema() {
        let request_schema = schema("request.schema.json");
        let frame_schema = schema("frame.schema.json");
        let requests = committed_json("golden/requests");
        let frames = committed_json("golden/frames");

        let operations = tags(&request_schema, Some("RemoteOperation"), "type");
        assert_eq!(
            operations,
            RemoteOperation::NAMES
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        );
        assert_eq!(strings(&requests, "/operation/type"), operations);
        assert_eq!(
            strings(&requests, "/operation/answer/type"),
            tags(&request_schema, Some("RemoteAnswer"), "type")
        );
        assert_eq!(strings(&frames, "/kind"), tags(&frame_schema, None, "kind"));
        assert_eq!(
            strings(&frames, "/result/type"),
            tags(&frame_schema, Some("RemoteResult"), "type")
        );
        assert_eq!(
            strings(&frames, "/event/type"),
            tags(&frame_schema, Some("RemoteEvent"), "type")
        );
        let receipts = tags_of_enum(&frame_schema, "ReceiptState");
        let mut seen = strings(&frames, "/receipt");
        seen.extend(strings(&frames, "/result/receipt"));
        assert_eq!(seen, receipts);
        let codes = strings(&frames, "/result/code");
        for code in [
            "busy",
            "stale_turn",
            "stale_question",
            "stale_approval",
            "unsupported_operation",
            "incompatible_version",
        ] {
            assert!(codes.contains(code), "no golden frame carries `{code}`");
        }
    }
}
