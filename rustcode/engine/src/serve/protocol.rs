//! Session message set beside the daemon protocol (issue #1443).
//!
//! Same framing style (`MAX_FRAME_BYTES`, newline-delimited JSON,
//! `#[serde(tag = "type")]` envelopes, shared `read_async_frame` /
//! `write_async_frame` helpers) with its own request/response types.
//! Snapshots and turn updates reuse the controller contract types
//! directly, so the wire can never drift from what frontends render.

use crate::controller::{ApprovalChoice, ControllerEvent};
use serde::{Deserialize, Serialize};

pub const SERVE_PROTOCOL_VERSION: u32 = 1;

/// First frame on every connection must be `Auth` with the per-launch
/// token. Anything else (or a mismatch) closes the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServeRequest {
    Auth {
        token: String,
    },
    ListSessions,
    Submit {
        prompt: String,
    },
    AnswerQuestion {
        answer: String,
    },
    Approve {
        batch_id: String,
        choice: ApprovalChoice,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServeResponse {
    Ready { version: u32 },
    Event(ControllerEvent),
    Error { code: String, message: String },
}

impl ServeResponse {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Worker errors travel as top-level error frames so a client handles
    /// one error shape whether it came from the handshake or the session.
    pub fn from_controller_error(error: &crate::controller::ControllerError) -> Self {
        let (code, message) = error.wire();
        Self::error(code, message)
    }

    /// A worker event on the wire; errors are lifted to top-level frames.
    pub fn from_event(event: ControllerEvent) -> Self {
        match &event.update {
            crate::controller::ControllerUpdate::Error(error) => Self::from_controller_error(error),
            _ => Self::Event(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_frames_use_tagged_envelopes() {
        let json = serde_json::to_value(ServeRequest::Approve {
            batch_id: "controller:1:2".into(),
            choice: ApprovalChoice::Deny,
        })
        .unwrap();
        assert_eq!(json["type"], "approve");
        assert_eq!(json["batch_id"], "controller:1:2");
        assert_eq!(json["choice"], "deny");
    }

    /// The request frames documented in `docs/mobile.md`, byte for byte.
    /// The remote protocol (`crate::remote`) is a separate wire; adding it
    /// must leave every one of these decoding and encoding exactly as before.
    #[test]
    fn legacy_request_fixtures_are_unchanged_on_the_wire() {
        let fixtures = [
            (
                r#"{"type":"auth","token":"t0k3n"}"#,
                ServeRequest::Auth {
                    token: "t0k3n".into(),
                },
            ),
            (r#"{"type":"list_sessions"}"#, ServeRequest::ListSessions),
            (
                r#"{"type":"submit","prompt":"hello"}"#,
                ServeRequest::Submit {
                    prompt: "hello".into(),
                },
            ),
            (
                r#"{"type":"answer_question","answer":"Proceed"}"#,
                ServeRequest::AnswerQuestion {
                    answer: "Proceed".into(),
                },
            ),
            (
                r#"{"type":"approve","batch_id":"controller:1:2","choice":"approve"}"#,
                ServeRequest::Approve {
                    batch_id: "controller:1:2".into(),
                    choice: ApprovalChoice::Approve,
                },
            ),
            (r#"{"type":"cancel"}"#, ServeRequest::Cancel),
        ];
        for (frame, request) in fixtures {
            assert_eq!(
                serde_json::from_str::<ServeRequest>(frame).unwrap(),
                request
            );
            assert_eq!(serde_json::to_string(&request).unwrap(), frame);
        }
        assert_eq!(SERVE_PROTOCOL_VERSION, 1);
    }

    /// The response frames a legacy client decodes, byte for byte. Question
    /// and turn identities exist in session state now but must not appear in
    /// these shapes.
    #[test]
    fn legacy_response_fixtures_are_unchanged_on_the_wire() {
        use crate::controller::{ControllerUpdate, QuestionPrompt, TurnUpdate};

        let ready = ServeResponse::Ready {
            version: SERVE_PROTOCOL_VERSION,
        };
        assert_eq!(
            serde_json::to_string(&ready).unwrap(),
            r#"{"type":"ready","version":1}"#
        );
        let question = ServeResponse::from_event(ControllerEvent {
            generation: 3,
            update: ControllerUpdate::Turn(TurnUpdate::QuestionRequested(QuestionPrompt {
                header: "Question".into(),
                text: "Continue?".into(),
                options: vec!["Proceed".into()],
                descriptions: vec![],
                multiple: false,
            })),
        });
        assert_eq!(
            serde_json::to_value(&question).unwrap(),
            serde_json::json!({
                "type": "event",
                "generation": 3,
                "update": {
                    "type": "question_requested",
                    "question": {
                        "header": "Question",
                        "text": "Continue?",
                        "options": ["Proceed"],
                        "descriptions": [],
                        "multiple": false,
                    },
                },
            })
        );

        let mut state = crate::app::AppState::new();
        state.begin_turn_identity();
        state.begin_question_chain(vec![crate::app::PendingQuestion::new(
            "Continue?".into(),
            vec!["Proceed".into()],
            false,
        )]);
        let snapshot =
            serde_json::to_value(crate::controller::ControllerSnapshot::from_state(1, &state))
                .unwrap();
        let mut keys = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "auto_approve",
                "background_tasks",
                "can_steer",
                "generation",
                "live_response",
                "models",
                "pending_approval",
                "pending_approval_batch",
                "pending_prompts",
                "pending_question",
                "queued_count",
                "selected_model",
                "session_id",
                "sessions",
                "transcript",
                "turn_active",
                "workspace",
            ]
        );
        assert_eq!(
            snapshot["pending_question"],
            serde_json::json!({
                "header": "Question",
                "text": "Continue?",
                "options": ["Proceed"],
                "descriptions": [],
                "multiple": false,
            })
        );
    }

    #[test]
    fn controller_events_ride_the_wire_unchanged() {
        let json = serde_json::to_value(ServeResponse::Error {
            code: "unauthorized".into(),
            message: "bad token".into(),
        })
        .unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "unauthorized");
    }
}
