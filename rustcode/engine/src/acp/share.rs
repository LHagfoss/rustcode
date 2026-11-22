//! The built-in agent's side of a remote share: it publishes the thread's
//! state to the gateway and answers device reads and mutations from it.

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::builtin::{AgentFuture, BuiltinPublisher, Published};
use crate::app::AppState;
use crate::controller::AgentUiEvent;
use crate::remote::owner::OwnerMessage;
use crate::remote::{
    ProjectionLimits, RemoteError, RemoteRequest, RemoteResult, SessionMutation, SessionPublisher,
    SessionRegistration,
};

struct StatePublisher {
    state: Arc<Mutex<AppState>>,
    inner: SessionPublisher,
}

impl BuiltinPublisher for StatePublisher {
    fn observe(&mut self, event: &AgentUiEvent) {
        self.inner.observe(event);
    }

    fn request_snapshot(&mut self) {
        self.inner.request_snapshot();
    }

    fn mark_lagging(&mut self) {
        self.inner.mark_lagging();
    }

    fn sequence(&self) -> u64 {
        self.inner.sequence()
    }

    fn publish(&mut self, room_for_resync: bool) -> AgentFuture<'_, Option<Published>> {
        Box::pin(async move {
            let state = self.state.lock().await;
            if !self.inner.owns(&state) {
                return None;
            }
            let messages = self.inner.publish(&state, room_for_resync);
            Some(Published {
                messages,
                queued_while_idle: !state.has_active_turn() && !state.pending_queue.is_empty(),
            })
        })
    }
}

pub(super) async fn share(
    state: &Arc<Mutex<AppState>>,
    session_id: &str,
    registration: SessionRegistration,
) -> Option<(Box<dyn BuiltinPublisher>, OwnerMessage)> {
    let guard = state.lock().await;
    if guard.active_session_id != session_id {
        return None;
    }
    let (inner, register) =
        SessionPublisher::register(&guard, registration, guard.has_active_turn());
    drop(guard);
    let publisher = StatePublisher {
        state: Arc::clone(state),
        inner,
    };
    Some((Box::new(publisher), register))
}

pub(super) async fn mutate(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    registration: &SessionRegistration,
    request: &RemoteRequest,
) -> Result<SessionMutation, RemoteError> {
    crate::remote::apply_session_mutation(state, cancel_token, registration, request).await
}

pub(super) async fn read(
    state: &Mutex<AppState>,
    registration: &SessionRegistration,
    request: &RemoteRequest,
) -> Result<RemoteResult, RemoteError> {
    let state = state.lock().await;
    crate::remote::read_session(&state, registration, request, &ProjectionLimits::default())
}

pub(super) async fn changes_workspace(
    state: &Mutex<AppState>,
    registration: &SessionRegistration,
    request: &RemoteRequest,
) -> Result<std::path::PathBuf, RemoteError> {
    let state = state.lock().await;
    crate::remote::ops::changes_workspace(&state, registration, request)
}

pub(super) fn selected_model(state: &Mutex<AppState>) -> Option<String> {
    let state = state.try_lock().ok()?;
    Some(crate::remote::projection::project_settings(&state).selected_model)
}
