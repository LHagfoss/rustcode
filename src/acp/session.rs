use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use tokio::sync::Mutex;

const KNOWN_TASK_ID_CAPACITY: usize = 1024;

/// Bounded set of task IDs seen by a session's router.
///
/// IDs are retained only to distinguish events that predate a prompt from
/// tasks created during it. A bounded FIFO keeps an idle, long-lived session
/// from retaining every task it has ever run.
#[derive(Default)]
pub(crate) struct KnownTaskIds {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl KnownTaskIds {
    pub(crate) fn insert(&mut self, id: String) {
        if !self.ids.insert(id.clone()) {
            return;
        }
        self.order.push_back(id);
        while self.order.len() > KNOWN_TASK_ID_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.ids.remove(id);
        self.order.retain(|known| known != id);
    }

    pub(crate) fn snapshot(&self) -> impl Iterator<Item = String> + '_ {
        self.ids.iter().cloned()
    }
}

pub(crate) struct SessionTurnState {
    gate: Arc<Mutex<()>>,
    next_id: std::sync::atomic::AtomicU64,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
}

impl SessionTurnState {
    pub(crate) fn new() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            next_id: std::sync::atomic::AtomicU64::new(1),
            accepted: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn schedule(&self) -> ScheduledSessionTurn {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((id, cancel_token.clone()));
        ScheduledSessionTurn {
            id,
            gate: Arc::clone(&self.gate),
            cancel_token,
            accepted: Arc::clone(&self.accepted),
            registered: true,
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn begin(&self) -> SessionTurnGuard {
        self.schedule().begin().await
    }

    pub(crate) async fn cancel_active(&self) -> bool {
        let accepted = self
            .accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (_, token) in accepted.iter() {
            token.cancel();
        }
        !accepted.is_empty()
    }
}

pub(crate) struct ScheduledSessionTurn {
    id: u64,
    gate: Arc<Mutex<()>>,
    cancel_token: tokio_util::sync::CancellationToken,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
    registered: bool,
}

impl ScheduledSessionTurn {
    pub(crate) async fn begin(mut self) -> SessionTurnGuard {
        let gate = Arc::clone(&self.gate).lock_owned().await;
        self.registered = false;
        SessionTurnGuard {
            id: self.id,
            _gate: gate,
            cancel_token: self.cancel_token.clone(),
            accepted: Arc::clone(&self.accepted),
        }
    }
}

impl Drop for ScheduledSessionTurn {
    fn drop(&mut self) {
        if self.registered {
            self.accepted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(id, _)| *id != self.id);
        }
    }
}

pub(crate) struct SessionTurnGuard {
    id: u64,
    _gate: tokio::sync::OwnedMutexGuard<()>,
    cancel_token: tokio_util::sync::CancellationToken,
    accepted: Arc<std::sync::Mutex<Vec<(u64, tokio_util::sync::CancellationToken)>>>,
}

impl SessionTurnGuard {
    pub(crate) fn cancel_token(&self) -> &tokio_util::sync::CancellationToken {
        &self.cancel_token
    }
}

impl Drop for SessionTurnGuard {
    fn drop(&mut self) {
        self.accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| *id != self.id);
    }
}

pub(crate) struct AcpSession {
    pub(crate) state: Arc<Mutex<crate::app::AppState>>,
    pub(crate) cwd: PathBuf,
    pub(crate) turns: Arc<SessionTurnState>,
    pub(crate) task_events: Arc<std::sync::Mutex<Receiver<rustcode_tasks::TaskEvent>>>,
    /// Task IDs observed by the ACP router but not yet consumed by a prompt.
    /// Keeping this alongside the inbox prevents a completion that races a
    /// new prompt from being mistaken for work created by that prompt.
    pub(crate) known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    pub(crate) terminal_backlog: Arc<std::sync::Mutex<VecDeque<rustcode_tasks::TaskEvent>>>,
    pub(crate) terminal_overflow: Arc<AtomicBool>,
}

pub(crate) type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

pub(crate) fn new_registry() -> Sessions {
    Arc::new(Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::{KNOWN_TASK_ID_CAPACITY, KnownTaskIds};

    #[test]
    fn known_task_ids_are_bounded_and_fifo() {
        let mut ids = KnownTaskIds::default();
        for index in 0..=KNOWN_TASK_ID_CAPACITY {
            ids.insert(format!("task-{index}"));
        }

        assert_eq!(ids.ids.len(), KNOWN_TASK_ID_CAPACITY);
        assert!(!ids.ids.contains("task-0"));
        assert!(ids.ids.contains(&format!("task-{KNOWN_TASK_ID_CAPACITY}")));
    }
}
