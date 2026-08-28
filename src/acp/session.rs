use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

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
}

pub(crate) type Sessions = Arc<Mutex<HashMap<String, AcpSession>>>;

pub(crate) fn new_registry() -> Sessions {
    Arc::new(Mutex::new(HashMap::new()))
}
