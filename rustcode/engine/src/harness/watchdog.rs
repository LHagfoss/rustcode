//! A harness turn that goes silent: the harness stops sending anything and
//! the thread stays "Working" until the user presses Stop. A turn that sends
//! nothing for a while with nothing running ends as failed instead, and its
//! harness is torn down, so the next message starts a fresh one that resumes
//! the conversation (`session.rs`).
//!
//! Silence alone is normal while work runs. A tool call the harness reported
//! and has not finished (a build, a test run, another thread's `wait_session`),
//! a permission request or question waiting on the user, and the harness's own
//! background tasks ([`super::background`]) hold the deadline instead of
//! counting against it. Every message the harness sends resets it, including a
//! thought or a usage update ([`Progress::touch`]).
//!
//! A harness that never reports a tool as finished only misses the deadline,
//! which is what we want: a killed healthy turn is far worse than a hang we
//! did not catch.

use super::background::Activity;
use agent_client_protocol::schema::v1::{SessionUpdate, ToolCallStatus};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

/// How long a turn may send nothing with nothing running before it ends:
/// `[harness_defaults].silence_timeout_minutes`, 0 turning it off.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the engine knows about the turn in flight.
#[derive(Debug, Default)]
struct State {
    /// When the harness last sent something.
    last: Option<Instant>,
    /// Tool calls the harness reported and has not finished.
    tools: HashSet<String>,
    /// Permission requests and questions the user has not answered.
    waiting: usize,
    /// The harness's background work, as it last reported it.
    background: Option<Activity>,
}

/// The turn's progress, shared by the harness's output, the user's answers and
/// the thread that watches for silence.
#[derive(Debug)]
pub(crate) struct Progress {
    state: Mutex<State>,
    /// Bumped whenever `state` changes, so the watch wakes on every event
    /// without a timer of its own while work is running.
    changed: watch::Sender<u64>,
}

impl Progress {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: watch::channel(0).0,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn touch_at(state: &mut State) {
        state.last = Some(Instant::now());
    }

    pub(crate) fn touch(&self) {
        Self::touch_at(&mut self.lock());
        self.signal();
    }

    /// A turn is starting: nothing a finished turn left behind still runs.
    /// Permissions are not cleared: each answered request drops its own guard
    /// ([`Waiting`]), and zeroing the count here would let a request left open
    /// by the last turn answer for one the user has not seen.
    pub(crate) fn begin_turn(&self) {
        {
            let mut state = self.lock();
            state.tools.clear();
            Self::touch_at(&mut state);
        }
        self.signal();
    }

    /// The harness was replaced: its tools and background work went with the
    /// process. A permission the user still has open is not the new process's,
    /// so it keeps holding the deadline until it is answered.
    pub(crate) fn reset(&self) {
        {
            let mut state = self.lock();
            state.tools.clear();
            state.background = None;
            Self::touch_at(&mut state);
        }
        self.signal();
    }

    /// One update from the harness. Its tool calls also say what still runs.
    pub(crate) fn observe(&self, update: &SessionUpdate) {
        {
            let mut state = self.lock();
            match update {
                SessionUpdate::ToolCall(call) => {
                    Self::set_tool(&mut state.tools, call.tool_call_id.0.as_ref(), call.status);
                }
                SessionUpdate::ToolCallUpdate(call) => {
                    if let Some(status) = call.fields.status {
                        Self::set_tool(&mut state.tools, call.tool_call_id.0.as_ref(), status);
                    }
                }
                _ => {}
            }
            Self::touch_at(&mut state);
        }
        self.signal();
    }

    /// The harness's background work, as `harness/background.rs` reports it.
    pub(crate) fn activity(&self, activity: &Activity) {
        {
            let mut state = self.lock();
            state.background = Some(activity.clone());
            Self::touch_at(&mut state);
        }
        self.signal();
    }

    /// A permission request or question is on its way to the user; the
    /// deadline is held until the returned guard drops.
    pub(crate) fn waiting(self: &Arc<Self>) -> Waiting {
        {
            let mut state = self.lock();
            state.waiting += 1;
            Self::touch_at(&mut state);
        }
        self.signal();
        Waiting {
            progress: Arc::clone(self),
        }
    }

    /// When the harness last counted as output (tests, and a log that wants it).
    #[cfg(test)]
    pub(crate) fn last_output_at(&self) -> Option<Instant> {
        self.lock().last
    }

    /// How many permission requests and questions wait on the user (tests).
    #[cfg(test)]
    pub(crate) fn waiting_count(&self) -> usize {
        self.lock().waiting
    }

    /// Whether a permission request or a question still waits on the user. The
    /// idle reaper (`session.rs`) never stops a process that is waiting for an
    /// answer, however long the answer takes.
    pub(crate) fn awaiting_user(&self) -> bool {
        self.lock().waiting > 0
    }

    /// Whether the harness reported background work that is not idle: the idle
    /// reaper leaves a thread alone while its own tasks run.
    pub(crate) fn background_running(&self) -> bool {
        self.lock()
            .background
            .as_ref()
            .is_some_and(|activity| activity.state != "idle")
    }

    /// Whether silence is expected right now because something is running.
    pub(crate) fn blocked(&self) -> bool {
        let state = self.lock();
        !state.tools.is_empty()
            || state.waiting > 0
            || state
                .background
                .as_ref()
                .is_some_and(|activity| activity.state != "idle")
    }

    /// Resolves once the turn has sent nothing for `timeout` with nothing
    /// running; never returns while a tool, a permission or background work is
    /// open, and waits without a deadline then.
    pub(crate) async fn silence(&self, timeout: Duration) {
        if self.lock().last.is_none() {
            self.touch();
        }
        let mut changed = self.changed.subscribe();
        loop {
            let deadline = self.lock().last.unwrap_or_else(Instant::now) + timeout;
            tokio::select! {
                result = changed.changed() => {
                    // No sender left: the thread is gone and the turn ends anyway.
                    if result.is_err() {
                        return;
                    }
                    continue;
                }
                () = tokio::time::sleep_until(deadline) => {}
            }
            let now = Instant::now();
            if self.lock().last.unwrap_or(now) + timeout > now {
                // The harness answered just as the deadline passed.
                continue;
            }
            if self.blocked() {
                // Work is running, so this silence is expected: no deadline
                // until the harness says something again.
                if changed.changed().await.is_err() {
                    return;
                }
                continue;
            }
            return;
        }
    }

    fn set_tool(tools: &mut HashSet<String>, id: &str, status: ToolCallStatus) {
        match status {
            // The call has started, or is still waiting for its approval.
            ToolCallStatus::Pending | ToolCallStatus::InProgress => {
                tools.insert(id.to_owned());
            }
            // A status we do not know is read as pending by the schema, so
            // anything else here is finished.
            _ => {
                tools.remove(id);
            }
        }
    }

    fn signal(&self) {
        self.changed
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

/// One permission request or question waiting on the user; dropping it says
/// the user answered.
#[derive(Debug)]
pub(crate) struct Waiting {
    progress: Arc<Progress>,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        {
            let mut state = self.progress.lock();
            // Each guard gives back exactly the count it took, so no guard can
            // release a request another one is still holding; `saturating_sub`
            // only keeps the count from underflowing.
            state.waiting = state.waiting.saturating_sub(1);
            Progress::touch_at(&mut state);
        }
        self.progress.signal();
    }
}

#[cfg(test)]
#[path = "watchdog_tests.rs"]
mod tests;
