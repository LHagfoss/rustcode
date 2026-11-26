//! Delivery of session approval requests across client disconnects.
//!
//! When a turn asks for approval (`session/request_permission`) while no
//! client has the session attached, or when the client's connection drops
//! before it answers, the request waits for a client to attach or reconnect
//! and delivers it there instead of immediately cancelling the tool call.

use std::collections::HashSet;

use agent_client_protocol::{Client, ConnectionTo, ErrorCode, UntypedMessage};
use tokio_util::sync::CancellationToken;

use super::{ClientId, Registry, SessionClients, send_untyped_request};

/// Cancellation handle shared by every approval currently waiting in one
/// session.
pub(super) struct CancelSlot {
    id: u64,
    token: CancellationToken,
    active: usize,
}

impl CancelSlot {
    pub(super) fn cancel(self) {
        self.token.cancel();
    }
}

struct ApprovalGuard<'a> {
    session: &'a SessionClients,
    slot_id: u64,
    token: CancellationToken,
}

impl Drop for ApprovalGuard<'_> {
    fn drop(&mut self) {
        let mut registry = self.session.clients.registry();
        let remove = match registry.approval_cancels.get_mut(&self.session.session_id) {
            Some(slot) if slot.id == self.slot_id => {
                slot.active = slot.active.saturating_sub(1);
                slot.active == 0
            }
            _ => false,
        };
        if remove {
            registry.approval_cancels.remove(&self.session.session_id);
        }
    }
}

fn register_approval(session: &SessionClients) -> Option<ApprovalGuard<'_>> {
    let mut registry = session.clients.registry();
    if registry.shutting_down {
        return None;
    }
    let next_id = registry.tick();
    let slot = registry
        .approval_cancels
        .entry(session.session_id.clone())
        .or_insert_with(|| CancelSlot {
            id: next_id,
            token: CancellationToken::new(),
            active: 0,
        });
    slot.active += 1;
    Some(ApprovalGuard {
        session,
        slot_id: slot.id,
        token: slot.token.clone(),
    })
}

fn approval_cancelled(session_id: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error()
        .data(format!("approval cancelled for session {session_id}"))
}

fn requester_excluding(
    registry: &Registry,
    session_id: &str,
    excluded: &HashSet<ClientId>,
) -> Option<(ClientId, ConnectionTo<Client>)> {
    registry
        .entries
        .iter()
        .filter(|(id, _)| !excluded.contains(id))
        .filter_map(|(id, entry)| Some((entry.sessions.get(session_id)?.at, *id, entry)))
        .max_by_key(|(at, _, _)| *at)
        .map(|(_, id, entry)| (id, entry.connection.clone()))
}

fn is_connection_drop(
    session: &SessionClients,
    client_id: ClientId,
    error: &agent_client_protocol::Error,
) -> bool {
    if agent_client_protocol::is_incoming_transport_closed(error) {
        return true;
    }
    if !session.clients.registry().entries.contains_key(&client_id) {
        return true;
    }
    error.code == ErrorCode::InternalError
        && error
            .data
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .is_some_and(|detail| {
                detail.starts_with("failed to send outgoing request")
                    || detail.contains("receiver is gone")
                    || detail.contains("channel closed")
            })
}

/// Deliver `request` to an attached client of `session`, waiting for one to
/// attach or reconnect when none is present or the connection drops mid-call.
pub(super) async fn request(
    session: &SessionClients,
    request: UntypedMessage,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let Some(guard) = register_approval(session) else {
        return Err(approval_cancelled(&session.session_id));
    };
    let cancel = guard.token.clone();
    let mut watch = session.clients.attached.subscribe();
    let mut dropped = HashSet::new();

    loop {
        if cancel.is_cancelled() {
            return Err(approval_cancelled(&session.session_id));
        }
        let target = {
            let registry = session.clients.registry();
            let _ = watch.borrow_and_update();
            if registry.shutting_down {
                return Err(approval_cancelled(&session.session_id));
            }
            requester_excluding(&registry, &session.session_id, &dropped)
        };
        let Some((client_id, connection)) = target else {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(approval_cancelled(&session.session_id)),
                changed = watch.changed() => {
                    if changed.is_err() {
                        return Err(approval_cancelled(&session.session_id));
                    }
                }
            }
            continue;
        };
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(approval_cancelled(&session.session_id)),
            result = send_untyped_request(connection, request.clone()) => result,
        };
        match response {
            Ok(value) => return Ok(value),
            Err(error) if is_connection_drop(session, client_id, &error) => {
                dropped.insert(client_id);
                continue;
            }
            Err(error) => return Err(error),
        }
    }
}
