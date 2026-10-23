//! A session owner that every device left while it waits on the user.
//!
//! A remote session owner (`rustcode remote session-owner`) runs a turn for
//! a phone. When the turn stops on a tool approval or a question and no
//! device is attached, nobody can answer it: the owner would hold the
//! session, and its approval, for as long as it lives. After
//! [`ORPHAN_GRACE`] with no device attached it stops the turn as a cancel
//! from the phone would, saves, and exits. A device that attaches again
//! within the grace keeps the turn waiting for it.
//!
//! An owner that does exit late, or is answered after another process saved
//! the session, never overwrites that newer history: it guards the file it
//! loaded (`rustcode_session::guard_history_file`).

use crate::app::AppState;
use std::time::{Duration, Instant};

/// How long an owner waits on an approval or a question with no device
/// attached before it stops the turn and exits. Long enough for a phone to
/// come back from the background or switch networks, short enough that a
/// forgotten owner does not hold the session for hours.
pub const ORPHAN_GRACE: Duration = Duration::from_secs(10 * 60);

/// Whether the session waits on the user: a tool approval or a question.
pub fn awaiting_user(state: &AppState) -> bool {
    state.pending_tool_confirmation.is_some() || state.pending_question.is_some()
}

/// Times how long an owner has waited on the user with no device attached.
#[derive(Debug)]
pub struct OrphanWatch {
    grace: Duration,
    since: Option<Instant>,
}

impl Default for OrphanWatch {
    fn default() -> Self {
        Self::new(ORPHAN_GRACE)
    }
}

impl OrphanWatch {
    pub fn new(grace: Duration) -> Self {
        Self { grace, since: None }
    }

    /// Note what the owner looks like at `now`. Returns true once it has
    /// waited on the user with no device attached for the whole grace; an
    /// attached device or an answered wait starts the count again.
    pub fn expired(&mut self, awaiting_user: bool, device_attached: bool, now: Instant) -> bool {
        if !awaiting_user || device_attached {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.saturating_duration_since(since) >= self.grace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatMessage, ToolConfirmation};
    use rustcode_session::SessionStore;

    const GRACE: Duration = Duration::from_secs(60);

    fn awaiting_approval() -> AppState {
        let mut state = AppState::new();
        state.pending_tool_confirmation = Some(vec![ToolConfirmation {
            request_id: Some("call-1".to_owned()),
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }]);
        state
    }

    #[test]
    fn a_pending_approval_or_question_waits_on_the_user() {
        let mut state = AppState::new();
        assert!(!awaiting_user(&state));
        assert!(awaiting_user(&awaiting_approval()));
        state.pending_question = Some(crate::app::PendingQuestion::new(
            "Which one?".to_owned(),
            vec!["a".to_owned(), "b".to_owned()],
            false,
        ));
        assert!(awaiting_user(&state));
    }

    #[test]
    fn a_disconnect_during_a_pending_approval_expires_after_the_grace() {
        let state = awaiting_approval();
        let mut watch = OrphanWatch::new(GRACE);
        let start = Instant::now();
        // The phone is attached: the approval waits for it.
        assert!(!watch.expired(awaiting_user(&state), true, start));
        assert!(!watch.expired(awaiting_user(&state), true, start + GRACE * 2));
        // The phone goes away.
        let left = start + GRACE * 2;
        assert!(!watch.expired(awaiting_user(&state), false, left));
        assert!(!watch.expired(
            awaiting_user(&state),
            false,
            left + GRACE - Duration::from_secs(1)
        ));
        assert!(watch.expired(awaiting_user(&state), false, left + GRACE));
    }

    #[test]
    fn reattaching_within_the_grace_keeps_the_owner() {
        let state = awaiting_approval();
        let mut watch = OrphanWatch::new(GRACE);
        let start = Instant::now();
        assert!(!watch.expired(true, false, start));
        // The phone comes back just before the grace ends, then leaves again:
        // the count starts over from the second disconnect.
        assert!(!watch.expired(awaiting_user(&state), true, start + GRACE / 2));
        let left = start + GRACE - Duration::from_secs(1);
        assert!(!watch.expired(awaiting_user(&state), false, left));
        assert!(!watch.expired(awaiting_user(&state), false, start + GRACE));
        assert!(watch.expired(awaiting_user(&state), false, left + GRACE));
    }

    #[test]
    fn an_owner_with_nothing_to_answer_never_expires() {
        let mut watch = OrphanWatch::new(GRACE);
        let start = Instant::now();
        let idle = AppState::new();
        assert!(!watch.expired(awaiting_user(&idle), false, start));
        assert!(!watch.expired(awaiting_user(&idle), false, start + GRACE * 10));
        // An approval answered before the grace ends resets the count.
        assert!(!watch.expired(true, false, start + GRACE * 10));
        assert!(!watch.expired(false, false, start + GRACE * 11 - Duration::from_secs(1)));
        assert!(!watch.expired(true, false, start + GRACE * 11));
    }

    #[test]
    fn the_default_grace_is_the_documented_one() {
        assert_eq!(OrphanWatch::default().grace, ORPHAN_GRACE);
    }

    #[test]
    fn an_owner_never_overwrites_history_another_process_saved_after_it_loaded() {
        let config = tempfile::tempdir().unwrap();
        let store = SessionStore::new(config.path());
        let id = rustcode_session::next_session_id();
        let directory = store.ensure_session(&id);
        let path = directory.join(rustcode_session::HISTORY_FILE);
        let loaded = vec![ChatMessage::new("user", "first")];
        rustcode_session::write_history_file(&path, &loaded);

        // The owner loads the session, and saves its own progress.
        store.guard_session_history(&id);
        let mut owner = loaded.clone();
        owner.push(ChatMessage::new("assistant", "owner's answer"));
        store.save_session_history(&id, &owner);
        rustcode_session::flush_history();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("owner's answer"), "{on_disk}");

        // The app saves the session while the owner waits on an approval.
        std::thread::sleep(Duration::from_millis(20));
        let mut app = owner.clone();
        app.push(ChatMessage::new("assistant", "the app's later work"));
        std::fs::write(&path, serde_json::to_string(&app).unwrap()).unwrap();

        // The approval is answered late: the owner's stale copy stays out.
        owner.push(ChatMessage::new("tool", "approved too late"));
        store.save_session_history(&id, &owner);
        rustcode_session::flush_history();
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("the app's later work"), "{on_disk}");
        assert!(!on_disk.contains("approved too late"), "{on_disk}");
    }
}
