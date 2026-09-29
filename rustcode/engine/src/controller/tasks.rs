//! Background-task and subagent observation for frontends (issues #1439, #1442).
//!
//! The task and subagent systems live in `crate::tools` / `crate::app`
//! (the latter backed by `rustcode-tasks`); these modules are the
//! frontend-facing contract over them. Every frontend — TUI, desktop, and the
//! future `serve` transport — observes and stops background tasks, and drives
//! subagents, through these items instead of reaching into the engine
//! directly.

/// Subagent process manager frontends spawn through.
pub use crate::app::SubagentController;
/// Per-session turn context a frontend attaches while background work is
/// running, so the engine drives those turns with the same limits as the
/// foreground session.
pub use crate::network::TurnContext;

/// Owned display data for one live background task.
///
/// Field-for-field this mirrors the engine's internal snapshot shape, but it
/// is owned by the controller contract so frontends never name engine
/// internals. Timestamps stay as `Instant` (elapsed/sort use only); anything
/// crossing a process boundary (see #1443) must render them first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDisplay {
    pub id: String,
    pub command: String,
    pub started_at: std::time::Instant,
    pub child_pid: Option<u32>,
}

impl TaskDisplay {
    /// Single-line label for the task's command, whitespace-collapsed and
    /// bounded to `max_chars` (ellipsis on overflow).
    pub fn label(&self, max_chars: usize) -> String {
        background_command_label(&self.command, max_chars)
    }
}

/// Wire form: `Instant` has no portable encoding, so snapshots carry how
/// long the task has been running instead of when it started.
impl serde::Serialize for TaskDisplay {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut task = serializer.serialize_struct("TaskDisplay", 4)?;
        task.serialize_field("id", &self.id)?;
        task.serialize_field("command", &self.command)?;
        task.serialize_field("elapsed_secs", &self.started_at.elapsed().as_secs())?;
        task.serialize_field("child_pid", &self.child_pid)?;
        task.end()
    }
}

impl From<crate::tools::BackgroundTaskSnapshot> for TaskDisplay {
    fn from(snapshot: crate::tools::BackgroundTaskSnapshot) -> Self {
        Self {
            id: snapshot.id,
            command: snapshot.command,
            started_at: snapshot.start_time,
            child_pid: snapshot.child_pid,
        }
    }
}

/// Live background tasks for `session_id`, oldest first (manager order).
pub fn background_task_snapshots(session_id: &str) -> Vec<TaskDisplay> {
    crate::tools::background_task_snapshots(session_id)
        .into_iter()
        .map(TaskDisplay::from)
        .collect()
}

/// Whether any background task is still running in `session_id`.
pub fn has_background_tasks(session_id: &str) -> bool {
    crate::tools::has_background_tasks(session_id)
}

/// Collapse whitespace and bound a command to one display line.
pub fn background_command_label(command: &str, max_chars: usize) -> String {
    crate::tools::background_command_label(command, max_chars)
}

/// Stop background tasks in `session_id`: just `task_id` when given,
/// the whole session otherwise. Idempotent — unknown ids stop nothing.
pub fn stop_background_tasks(
    session_id: &str,
    task_id: Option<&str>,
) -> crate::tools::BackgroundStopResult {
    match task_id {
        Some(id) => single_stop_result(crate::tools::background_task_manager().cancel(id)),
        None => crate::tools::stop_background_tasks(session_id),
    }
}

fn single_stop_result(result: rustcode_tasks::CancelResult) -> crate::tools::BackgroundStopResult {
    use rustcode_tasks::CancelResult;
    let mut summary = crate::tools::BackgroundStopResult::default();
    match result {
        CancelResult::Cancelled => summary.stopped = 1,
        CancelResult::Requested => summary.requested = 1,
        CancelResult::Failed => summary.failed = 1,
        CancelResult::AlreadyFinished | CancelResult::NotFound => {}
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_label_collapses_whitespace_and_bounds_length() {
        assert_eq!(
            background_command_label("cargo\n test\t--locked", 80),
            "cargo test --locked"
        );
        assert_eq!(background_command_label("sleep 30", 80), "sleep 30");
        let bounded = background_command_label("cargo test --locked", 10);
        assert_eq!(bounded.chars().count(), 10);
        assert!(bounded.ends_with('…'));
    }

    #[test]
    fn snapshots_expose_live_tasks_through_the_contract() {
        let session_id = "controller-tasks-snapshot";
        crate::tools::spawn_background_task_for_test(
            "controller-tasks-snapshot-task",
            session_id,
            "sleep 30",
        )
        .expect("spawn background task");
        // PID publication is asynchronous; wait for Running before asserting.
        for _ in 0..100 {
            if background_task_snapshots(session_id)
                .first()
                .is_some_and(|task| task.child_pid.is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let snapshots = background_task_snapshots(session_id);
        assert!(has_background_tasks(session_id));
        stop_background_tasks(session_id, None);
        // Cancellation of a just-spawned task can complete asynchronously.
        for _ in 0..100 {
            if background_task_snapshots(session_id).is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "controller-tasks-snapshot-task");
        assert_eq!(snapshots[0].command, "sleep 30");
        assert_eq!(snapshots[0].label(80), "sleep 30");
        assert!(snapshots[0].started_at.elapsed().as_secs() < 60);
        assert!(!has_background_tasks(session_id));
        assert!(background_task_snapshots(session_id).is_empty());
    }

    #[test]
    fn single_task_stop_reports_per_task_outcome() {
        let session_id = "controller-tasks-single-stop";
        crate::tools::spawn_background_task_for_test(
            "controller-tasks-single-stop-task",
            session_id,
            "sleep 30",
        )
        .expect("spawn background task");

        let missing = stop_background_tasks(session_id, Some("no-such-task"));
        assert_eq!(missing, crate::tools::BackgroundStopResult::default());

        let stopped = stop_background_tasks(session_id, Some("controller-tasks-single-stop-task"));
        assert_eq!(stopped.stopped + stopped.requested, 1);
        assert_eq!(stopped.failed, 0);
    }
}
