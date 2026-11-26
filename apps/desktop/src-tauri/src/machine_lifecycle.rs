//! Starting and ending one machine's supervisor, for [`super::Machines`].

use super::{Live, MachineState};

/// Whether a window attaching to `entry` shares the supervisor that runs:
/// one is running, and this window is not the one that asked before.
pub(super) fn joins(entry: &Live, window: &str) -> bool {
    entry.task.is_some()
        && !matches!(
            entry.state,
            MachineState::NeedsPairing | MachineState::Stopped
        )
        && !entry.link.has(window)
}

/// End a machine's supervisor and connection.
pub(super) fn stop(entry: &mut Live) {
    if let Some(task) = entry.task.take() {
        task.abort();
    }
    entry.link.end();
    entry.outbox = None;
    entry.state = MachineState::Stopped;
    entry.route = None;
    entry.answered = None;
    entry.cause = None;
    entry.detail = None;
    entry.hint = None;
    entry.retry_in_secs = None;
}
