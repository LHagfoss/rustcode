use crate::app::{AppState, AppStatus, ChatMessage};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Pid, System};
use tokio::sync::Mutex;

const CHANGELOG_CONTENT: &str = include_str!("../../CHANGELOG.md");
pub(crate) const CTRL_C_EXIT_CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);

pub fn build_latest_changelog() -> String {
    let mut out = String::new();
    let mut version_count = 0;

    for line in CHANGELOG_CONTENT.lines() {
        if line.starts_with("## [") {
            version_count += 1;
            if version_count > 2 {
                break;
            }
        }
        if version_count > 0 {
            out.push_str(line);
            out.push('\n');
        }
    }

    if out.trim().is_empty() {
        CHANGELOG_CONTENT
            .lines()
            .take(30)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        out.trim().to_string()
    }
}

pub async fn handle_escape(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut tokio_util::sync::CancellationToken,
) {
    let mut s = state.lock().await;
    let active_session_id = s.active_session_id.clone();
    s.promote_pending_steers_to_queue(&active_session_id);
    s.clear_ctrl_c_exit_arming();
    s.reset_suggestion_cycle();
    s.input_buffer.clear();
    s.cursor_position = 0;

    cancel_token.cancel();
    *cancel_token = tokio_util::sync::CancellationToken::new();
    s.clear_active_turn_projection();

    if s.status == AppStatus::Streaming || s.orchestrator_running {
        s.enter_idle();
        // Let the current orchestrator unwind and release its lease at the
        // cancellation boundary. User prompts already accepted while the
        // stream was active must survive and will be submitted in FIFO order
        // once that lease is released. Background wakeups are internal
        // continuations, not user input, and must not restart after Esc.
        s.consume_observed_background_wakeups();
    } else if !s.pending_queue.is_empty() {
        s.pending_queue.remove(0);
        s.note_pending_prompt_removed(0);
        if s.pending_queue.is_empty() {
            s.enter_idle();
        }
    }
    s.background_turn_context = None;
}

/// Arm the exit confirmation on the first Ctrl+C and exit on the second.
/// Ctrl+C deliberately does not cancel drafts, overlays, or active work;
/// Esc remains the cancellation key for those interactions.
pub async fn handle_ctrl_c(state: &Arc<Mutex<AppState>>) -> bool {
    let mut s = state.lock().await;
    let now = Instant::now();
    if s.ctrl_c_exit_deadline
        .is_some_and(|deadline| deadline > now)
    {
        s.ctrl_c_exit_deadline = None;
        true
    } else {
        s.ctrl_c_exit_deadline = Some(now + CTRL_C_EXIT_CONFIRMATION_WINDOW);
        s.request_redraw();
        false
    }
}

#[path = "actions/commands.rs"]
mod commands;
#[path = "actions/enter.rs"]
mod enter;
#[path = "actions/session.rs"]
mod session;
#[path = "actions/submit.rs"]
mod submit;

#[cfg(test)]
#[path = "actions/tests.rs"]
mod tests;

pub use commands::*;
#[allow(unused_imports)]
pub use enter::handle_enter;
pub(crate) use enter::handle_enter_with_ui_events;
pub use session::*;
pub(crate) use submit::{SubmitOutcome, submit_plain_prompt};

#[cfg(test)]
use commands::append_codex_rate_limits;
use commands::{background_terminal_list, stop_background_terminals};
use session::{history_matches_snapshot, report_stale_compaction, try_merge_compacted_history};
