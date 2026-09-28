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
