//! Saving an external harness's turn while it runs, so a thread reopened
//! after the engine died mid-turn shows what the turn had done, not only the
//! prompt. The history writer debounces and coalesces, so a save per
//! boundary costs one file write at most every quarter second.

use crate::harness::transcript::Transcript;
use agent_client_protocol::schema::v1::{SessionUpdate, ToolCallStatus};
use std::time::{Duration, Instant};

/// How often streamed text alone (no tool call starting or finishing) is saved.
pub(crate) const TEXT_INTERVAL: Duration = Duration::from_secs(5);

/// When the running turn was last saved.
#[derive(Default)]
pub(crate) struct Checkpoints {
    last: Option<Instant>,
}

impl Checkpoints {
    /// Whether `update` is a point worth saving at: a tool call started or
    /// finished, or text has streamed for [`TEXT_INTERVAL`] since the last save.
    pub(crate) fn due(&mut self, update: &SessionUpdate, now: Instant) -> bool {
        let due = match update {
            SessionUpdate::ToolCall(_) => true,
            SessionUpdate::ToolCallUpdate(update) => matches!(
                update.fields.status,
                Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
            ),
            SessionUpdate::AgentMessageChunk(_)
            | SessionUpdate::AgentThoughtChunk(_)
            | SessionUpdate::Plan(_) => self
                .last
                .is_none_or(|last| now.duration_since(last) >= TEXT_INTERVAL),
            _ => false,
        };
        if due {
            self.last = Some(now);
        }
        due
    }

    /// The turn ended and was saved in full.
    pub(crate) fn reset(&mut self) {
        self.last = None;
    }
}

/// Queue the running turn's history for the thread `id`.
pub(crate) fn save(id: &str, transcript: &Transcript) {
    crate::sessions::save_session_history(id, &transcript.checkpoint());
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ToolCall, ToolCallUpdate, ToolCallUpdateFields};

    fn text(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(
            serde_json::from_value(serde_json::json!({
                "content": {"type": "text", "text": text}
            }))
            .unwrap(),
        )
    }

    fn status(id: &str, status: ToolCallStatus) -> SessionUpdate {
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            id,
            ToolCallUpdateFields::new().status(status),
        ))
    }

    #[test]
    fn tool_boundaries_are_always_due_and_text_once_per_interval() {
        let mut checkpoints = Checkpoints::default();
        let start = Instant::now();

        assert!(checkpoints.due(&text("a"), start));
        assert!(!checkpoints.due(&text("b"), start + Duration::from_secs(1)));
        assert!(checkpoints.due(
            &SessionUpdate::ToolCall(ToolCall::new("call-1", "Run")),
            start + Duration::from_secs(2)
        ));
        assert!(!checkpoints.due(
            &status("call-1", ToolCallStatus::InProgress),
            start + Duration::from_secs(3)
        ));
        assert!(checkpoints.due(
            &status("call-1", ToolCallStatus::Completed),
            start + Duration::from_secs(3)
        ));
        assert!(!checkpoints.due(&text("c"), start + Duration::from_secs(4)));
        assert!(checkpoints.due(&text("d"), start + Duration::from_secs(9)));
    }

    #[test]
    fn a_checkpoint_marks_running_calls_interrupted_and_keeps_the_live_turn() {
        let mut transcript = Transcript::new(Vec::new());
        transcript.push_user("build it");
        transcript.observe(&text("Running the build."));
        transcript.observe(&SessionUpdate::ToolCall(ToolCall::new(
            "call-1",
            "cargo build",
        )));
        transcript.observe(&text("Still going"));

        let saved = transcript.checkpoint();

        let roles: Vec<_> = saved.iter().map(|message| message.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
        assert_eq!(saved[1].content, "Running the build.");
        let record = saved[2].tool_result.as_ref().unwrap();
        assert!(!record.pending);
        assert_eq!(record.error_kind.as_deref(), Some("interrupted"));
        assert_eq!(saved[3].content, "Still going");

        let live = transcript.history[2].tool_result.as_ref().unwrap();
        assert!(live.pending);
        assert_eq!(transcript.history.len(), 3);

        transcript.observe(&status("call-1", ToolCallStatus::Completed));
        transcript.end_reply();
        let roles: Vec<_> = transcript
            .history
            .iter()
            .map(|message| message.role.as_str())
            .collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
        assert!(
            transcript.history[2]
                .tool_result
                .as_ref()
                .unwrap()
                .error_kind
                .is_none()
        );
    }

    #[test]
    fn a_saved_checkpoint_is_what_the_thread_reopens_with() {
        let id = format!("checkpoint-reopen-{}", std::process::id());
        let mut transcript = Transcript::new(Vec::new());
        transcript.push_user("run the checks");
        transcript.observe(&SessionUpdate::ToolCall(ToolCall::new(
            "call-1",
            "Run checks",
        )));
        transcript.observe(&status("call-1", ToolCallStatus::Completed));
        transcript.observe(&SessionUpdate::ToolCall(ToolCall::new("call-2", "Deploy")));

        save(&id, &transcript);
        crate::sessions::flush_history();

        let (_, reopened) =
            crate::sessions::load_session_by_id(&id).expect("history saved mid-turn");
        let tools: Vec<_> = reopened
            .iter()
            .filter_map(|message| message.tool_result.as_ref())
            .collect();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|record| !record.pending));
        assert!(tools[0].error_kind.is_none());
        assert_eq!(tools[1].error_kind.as_deref(), Some("interrupted"));
    }
}
