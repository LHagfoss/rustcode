//! `/remote` for ACP clients such as the macOS app.
//!
//! The engine owns the sharing bridge: it registers the session with the local
//! gateway, publishes the same snapshots and events as the terminal, applies
//! commands from paired devices, and runs prompts that arrive there. The
//! client only asks for status, a fresh pairing offer, or to turn sharing off,
//! and renders the answer.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::{Client, ConnectionTo};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::AcpSession;
use super::session::{KnownTaskIds, SessionTurnState};
use crate::app::AppState;
use crate::controller::AgentUiEvent;
use crate::remote::SessionCloseReason;
use rustcode_tasks::TaskEvent;

// Shares and pairing offers are driven by the remote gateway, which the
// workspace builds only on unix; everywhere else the manager below is a
// stub with the same surface that reports the feature as unavailable.
#[cfg(unix)]
use crate::remote::owner::{
    GatewayLinkStatus, OwnerCommand, OwnerLink, OwnerMessage, default_connector,
    next_registration_epoch,
};
#[cfg(unix)]
use crate::remote::{
    OwnerFollowUp, ProjectionLimits, ReceiptState, RemoteResponse, SessionPublisher,
    SessionRegistration, apply_session_mutation, read_session,
};
#[cfg(unix)]
use crate::remote_gateway::control::OfferDetails;
#[cfg(unix)]
use crate::remote_gateway::lifecycle::RemoteLifecycle;
#[cfg(unix)]
use crate::remote_gateway::service;
#[cfg(unix)]
use std::time::{Duration, Instant};
#[cfg(unix)]
use tokio::sync::mpsc;
#[cfg(unix)]
use tokio_util::sync::CancellationToken;

pub(crate) const REMOTE_CAPABILITY_VERSION: u64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote", response = serde_json::Value)]
pub(crate) struct RemoteStatusRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/start", response = serde_json::Value)]
pub(crate) struct RemoteStartRequest {
    pub session_id: String,
    /// Ask for a pairing offer as soon as the gateway is connected.
    #[serde(default)]
    pub pair: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/pair", response = serde_json::Value)]
pub(crate) struct RemotePairRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/stop", response = serde_json::Value)]
pub(crate) struct RemoteStopRequest {
    pub session_id: String,
}

/// The runtime a remote turn needs. It is built from the ACP session when
/// `/remote` starts and stays valid while the share exists.
pub(crate) struct RemoteSession {
    pub session_id: String,
    pub state: Arc<Mutex<AppState>>,
    pub cwd: PathBuf,
    pub turns: Arc<SessionTurnState>,
    pub task_events: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<TaskEvent>>>,
    pub known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    pub terminal_backlog: Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
    pub terminal_overflow: Arc<AtomicBool>,
    pub connection: ConnectionTo<Client>,
    pub client: Arc<reqwest::Client>,
    pub auto_approve: bool,
    pub presence: super::presence::Presence,
    pub remote: RemoteManager,
    runner: AtomicBool,
    sharing: AtomicBool,
}

impl RemoteSession {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_acp(
        session_id: String,
        session: &AcpSession,
        connection: ConnectionTo<Client>,
        client: Arc<reqwest::Client>,
        auto_approve: bool,
        presence: super::presence::Presence,
        remote: RemoteManager,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            state: Arc::clone(&session.state),
            cwd: session.cwd.clone(),
            turns: Arc::clone(&session.turns),
            task_events: Arc::clone(&session.task_events),
            known_task_ids: Arc::clone(&session.known_task_ids),
            terminal_backlog: Arc::clone(&session.terminal_backlog),
            terminal_overflow: Arc::clone(&session.terminal_overflow),
            connection,
            client,
            auto_approve,
            presence,
            remote,
            runner: AtomicBool::new(false),
            sharing: AtomicBool::new(false),
        })
    }

    /// Start the turn loop if a queued prompt is waiting and no other turn is
    /// running. Safe to call from several places: only one runner exists.
    fn ensure_runner(self: &Arc<Self>) {
        if !self.sharing.load(Ordering::Acquire) {
            return;
        }
        if self
            .runner
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let session = Arc::clone(self);
        tokio::spawn(async move {
            run_remote_turns(session).await;
        });
    }
}

async fn run_remote_turns(session: Arc<RemoteSession>) {
    loop {
        if !session.sharing.load(Ordering::Acquire) {
            break;
        }
        let prompt = {
            let mut state = session.state.lock().await;
            if state.active_session_id != session.session_id
                || state.pending_queue.is_empty()
                || state.has_active_turn()
            {
                None
            } else {
                Some(state.pending_queue.remove(0))
            }
        };
        let Some(prompt) = prompt else {
            break;
        };
        if prompt.starts_with("__task_wakeup__:") {
            // Background wakeups are internal markers, not user prompts.
            continue;
        }
        if !prompt.trim_start().starts_with('/') {
            let update = SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(prompt.clone()),
            )));
            let _ = session
                .connection
                .send_notification(SessionNotification::new(session.session_id.clone(), update));
        }
        let scheduled_turn = session.turns.schedule();
        session
            .presence
            .turn_started(&session.session_id, &session.cwd);
        let result = super::prompt::run_prompt(
            Arc::clone(&session.state),
            session.cwd.clone(),
            scheduled_turn,
            session.session_id.clone(),
            Arc::clone(&session.task_events),
            Arc::clone(&session.known_task_ids),
            Arc::clone(&session.terminal_backlog),
            Arc::clone(&session.terminal_overflow),
            prompt,
            session.connection.clone(),
            Arc::clone(&session.client),
            session.auto_approve,
            Some(session.remote.clone()),
            Arc::downgrade(&session.turns),
        )
        .await;
        session
            .presence
            .turn_finished(&session.session_id, &session.cwd);
        if let Err(error) = result {
            let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new(format!("**Remote turn failed**\n\n{error}")),
            )));
            let _ = session
                .connection
                .send_notification(SessionNotification::new(session.session_id.clone(), update));
        }
        {
            let mut state = session.state.lock().await;
            state.promote_pending_steers_to_queue(&session.session_id);
        }
    }
    session.runner.store(false, Ordering::Release);
    // A prompt queued between the last check and the flag clear would
    // otherwise sit until the next status poll.
    if session.sharing.load(Ordering::Acquire) {
        let has_work = {
            let state = session.state.lock().await;
            !state.pending_queue.is_empty() && !state.has_active_turn()
        };
        if has_work {
            session.ensure_runner();
        }
    }
}

#[cfg(unix)]
#[derive(Clone, Default)]
pub(crate) struct RemoteManager {
    inner: Arc<std::sync::Mutex<Inner>>,
}

#[cfg(unix)]
#[derive(Default)]
struct Inner {
    shares: std::collections::HashMap<String, Arc<Share>>,
}

#[cfg(unix)]
struct Share {
    session: Arc<RemoteSession>,
    registration: SessionRegistration,
    state: Mutex<ShareState>,
    events: mpsc::UnboundedSender<AgentUiEvent>,
    stop: CancellationToken,
    pairing_in_flight: AtomicBool,
}

#[cfg(unix)]
struct ShareState {
    publisher: SessionPublisher,
    link: OwnerLink,
    events: mpsc::UnboundedReceiver<AgentUiEvent>,
    gateway: Option<GatewayLinkStatus>,
    lifecycle: Option<RemoteLifecycle>,
    pairing_due: bool,
    offer: Option<PairingOffer>,
    pairing_error: Option<String>,
    closed: Option<String>,
}

#[cfg(unix)]
struct PairingOffer {
    qr_payload: String,
    code: String,
    address: String,
    loopback_only: bool,
    gateway_id: String,
    challenge: Option<u64>,
    locked_for_secs: Option<u64>,
    expires_at: Instant,
}

#[cfg(unix)]
impl PairingOffer {
    fn new(offer: OfferDetails) -> Self {
        let lifetime = Duration::from_secs(offer.expires_in_secs);
        Self {
            qr_payload: offer.qr_payload.expose().to_owned(),
            code: offer.code.expose().to_owned(),
            address: offer.advertised_address,
            loopback_only: offer.loopback_only,
            gateway_id: offer.gateway_id,
            challenge: offer.challenge,
            locked_for_secs: offer.locked_for_secs,
            expires_at: Instant::now() + lifetime,
        }
    }
}

#[cfg(unix)]
impl RemoteManager {
    fn get(&self, session_id: &str) -> Option<Arc<Share>> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.shares.get(session_id).cloned())
    }

    fn forget(&self, share: &Arc<Share>) {
        if let Ok(mut inner) = self.inner.lock()
            && inner
                .shares
                .get(&share.session.session_id)
                .is_some_and(|current| Arc::ptr_eq(current, share))
        {
            inner.shares.remove(&share.session.session_id);
        }
        share.session.sharing.store(false, Ordering::Release);
    }

    /// Note an agent event for the share that owns `session_id`. Called from
    /// the prompt loop for local and remote turns alike.
    pub(crate) fn observe(&self, session_id: &str, event: &AgentUiEvent) {
        if let Some(share) = self.get(session_id) {
            let _ = share.events.send(event.clone());
        }
    }

    pub(crate) async fn start(
        &self,
        session: Arc<RemoteSession>,
        pair: bool,
    ) -> Result<Value, agent_client_protocol::Error> {
        if let Some(share) = self.get(&session.session_id) {
            session.sharing.store(true, Ordering::Release);
            if pair {
                share.request_pair().await;
            }
            return Ok(self.status(&session.session_id).await);
        }

        let registration = SessionRegistration {
            session_id: session.session_id.clone(),
            registration_epoch: next_registration_epoch(),
        };
        let mut connector = default_connector();
        let directory = connector.gateway_directory();
        let link = connector.connect(&registration).map_err(|error| {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        })?;
        let lifecycle = directory.map(RemoteLifecycle::new);

        let (events, receiver) = mpsc::unbounded_channel();
        let share_state = {
            let state = session.state.lock().await;
            if state.active_session_id != session.session_id {
                return Err(agent_client_protocol::Error::invalid_params()
                    .data("the ACP session is no longer active"));
            }
            let (publisher, register) =
                SessionPublisher::register(&state, registration.clone(), state.has_active_turn());
            drop(state);
            if let Err(error) = link.outbound.try_send(register) {
                return Err(agent_client_protocol::Error::internal_error()
                    .data(format!("could not register the remote session: {error}")));
            }
            ShareState {
                publisher,
                link,
                events: receiver,
                gateway: None,
                lifecycle,
                pairing_due: pair,
                offer: None,
                pairing_error: None,
                closed: None,
            }
        };

        let share = Arc::new(Share {
            session: Arc::clone(&session),
            registration,
            state: Mutex::new(share_state),
            events,
            stop: CancellationToken::new(),
            pairing_in_flight: AtomicBool::new(false),
        });
        session.sharing.store(true, Ordering::Release);
        self.inner
            .lock()
            .map_err(|_| {
                agent_client_protocol::Error::internal_error().data("remote manager is unavailable")
            })?
            .shares
            .insert(session.session_id.clone(), Arc::clone(&share));
        tokio::spawn(share_loop(self.clone(), share));
        Ok(self.status(&session.session_id).await)
    }

    pub(crate) async fn status(&self, session_id: &str) -> Value {
        let Some(share) = self.get(session_id) else {
            return idle_status(session_id);
        };
        share.pair_if_due().await;
        share.status_value().await
    }

    pub(crate) async fn pair(&self, session_id: &str) -> Value {
        let Some(share) = self.get(session_id) else {
            return json!({
                "available": true,
                "shared": false,
                "error": "this session is not shared",
            });
        };
        share.request_pair().await;
        share.pair_if_due().await;
        share.status_value().await
    }

    /// Turn remote access off for this machine, ending every share.
    pub(crate) async fn stop(
        &self,
        session_id: &str,
    ) -> Result<Value, agent_client_protocol::Error> {
        let shares = {
            let mut inner = self.inner.lock().map_err(|_| {
                agent_client_protocol::Error::internal_error().data("remote manager is unavailable")
            })?;
            inner
                .shares
                .drain()
                .map(|(_, share)| share)
                .collect::<Vec<_>>()
        };
        let directory = shares
            .first()
            .and_then(|share| share.directory())
            .or_else(crate::config::get_config_dir);
        for share in &shares {
            share.close(SessionCloseReason::SharingDisabled).await;
        }
        if let Some(directory) = &directory {
            service::disable(directory).await.map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        }
        Ok(idle_status(session_id))
    }

    /// Stop sharing one session without changing the machine-wide setting.
    pub(crate) async fn stop_session(&self, session_id: &str, reason: SessionCloseReason) {
        let share = self
            .inner
            .lock()
            .ok()
            .and_then(|mut inner| inner.shares.remove(session_id));
        if let Some(share) = share {
            share.close(reason).await;
        }
    }

    /// End all shares because the ACP connection is gone.
    pub(crate) async fn shutdown(&self) {
        let shares = self
            .inner
            .lock()
            .map(|mut inner| {
                inner
                    .shares
                    .drain()
                    .map(|(_, share)| share)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for share in shares {
            share.close(SessionCloseReason::OwnerExited).await;
        }
    }
}

/// On hosts without the gateway, the manager keeps the same surface so the
/// ACP handlers compile everywhere, and reports sharing as unavailable.
#[cfg(not(unix))]
#[derive(Clone, Default)]
pub(crate) struct RemoteManager;

#[cfg(not(unix))]
impl RemoteManager {
    /// Shares cannot exist on this host, so there is nothing to publish.
    pub(crate) fn observe(&self, _session_id: &str, _event: &AgentUiEvent) {}

    pub(crate) async fn start(
        &self,
        _session: Arc<RemoteSession>,
        _pair: bool,
    ) -> Result<Value, agent_client_protocol::Error> {
        Err(self.unsupported())
    }

    pub(crate) async fn status(&self, session_id: &str) -> Value {
        idle_status(session_id)
    }

    pub(crate) async fn pair(&self, session_id: &str) -> Value {
        json!({
            "available": false,
            "shared": false,
            "sessionId": session_id,
            "error": "remote access is not available on this host",
        })
    }

    pub(crate) async fn stop(
        &self,
        _session_id: &str,
    ) -> Result<Value, agent_client_protocol::Error> {
        Err(self.unsupported())
    }

    pub(crate) async fn stop_session(&self, _session_id: &str, _reason: SessionCloseReason) {}

    pub(crate) async fn shutdown(&self) {}

    fn unsupported(&self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::invalid_params().data(
            "remote access needs the remote gateway, which the workspace builds only on unix hosts",
        )
    }
}

fn idle_status(session_id: &str) -> Value {
    let devices = idle_devices();
    json!({
        "available": true,
        "enabled": remote_enabled(),
        "shared": false,
        "sessionId": session_id,
        "pairedDevices": devices.len(),
        "devices": devices,
    })
}

#[cfg(unix)]
fn remote_enabled() -> bool {
    crate::config::get_config_dir()
        .as_deref()
        .is_some_and(service::enabled)
}

#[cfg(not(unix))]
fn remote_enabled() -> bool {
    false
}

#[cfg(unix)]
fn idle_devices() -> Vec<Value> {
    let directory = crate::config::get_config_dir();
    directory
        .as_deref()
        .map(RemoteLifecycle::new)
        .and_then(|lifecycle| lifecycle.devices().ok())
        .unwrap_or_default()
        .iter()
        .map(|device| {
            json!({
                "id": device.id,
                "name": device.name,
            })
        })
        .collect()
}

#[cfg(not(unix))]
fn idle_devices() -> Vec<Value> {
    Vec::new()
}

#[cfg(unix)]
async fn share_loop(manager: RemoteManager, share: Arc<Share>) {
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = share.stop.cancelled() => break,
            _ = interval.tick() => {}
        }
        if !share.tick().await {
            break;
        }
    }
    manager.forget(&share);
}

#[cfg(unix)]
impl Share {
    fn directory(&self) -> Option<PathBuf> {
        self.state.try_lock().ok().and_then(|state| {
            state
                .lifecycle
                .as_ref()
                .map(RemoteLifecycle::config_directory)
        })
    }

    async fn request_pair(&self) {
        let mut state = self.state.lock().await;
        state.pairing_due = true;
        state.pairing_error = None;
        state.offer = None;
    }

    async fn pair_if_due(&self) {
        if self.pairing_in_flight.swap(true, Ordering::AcqRel) {
            return;
        }
        let lifecycle = {
            let mut state = self.state.lock().await;
            let connected = state
                .gateway
                .as_ref()
                .is_some_and(|gateway| gateway.connected);
            if !state.pairing_due || !connected {
                drop(state);
                self.pairing_in_flight.store(false, Ordering::Release);
                return;
            }
            // Clear the request before waiting so a `request_pair` that
            // arrives while the gateway is answering is not lost: it sets
            // this back to true and the loop issues another offer.
            state.pairing_due = false;
            state.offer = None;
            state.pairing_error = None;
            state.lifecycle.clone()
        };
        let Some(lifecycle) = lifecycle else {
            let mut state = self.state.lock().await;
            state.pairing_error = Some("remote access is unavailable on this host".to_owned());
            self.pairing_in_flight.store(false, Ordering::Release);
            return;
        };
        let result = tokio::time::timeout(Duration::from_secs(15), lifecycle.pair()).await;
        let mut state = self.state.lock().await;
        match result {
            Ok(Ok(offer)) => {
                state.offer = Some(PairingOffer::new(offer));
                state.pairing_error = None;
            }
            Ok(Err(error)) => {
                state.pairing_error = Some(error.to_string());
            }
            Err(_) => {
                state.pairing_error = Some("the remote gateway did not answer in time".to_owned());
            }
        }
        self.pairing_in_flight.store(false, Ordering::Release);
    }

    async fn status_value(&self) -> Value {
        let state = self.state.lock().await;
        let now = Instant::now();
        let offer = state
            .offer
            .as_ref()
            .filter(|offer| offer.expires_at > now)
            .map(|offer| {
                json!({
                    "qrPayload": offer.qr_payload,
                    "code": offer.code,
                    "address": offer.address,
                    "expiresInSecs": offer.expires_at.saturating_duration_since(now).as_secs(),
                    "loopbackOnly": offer.loopback_only,
                    "gatewayId": offer.gateway_id,
                    "challenge": offer.challenge,
                    "lockedForSecs": offer.locked_for_secs,
                })
            });
        let expired = state
            .offer
            .as_ref()
            .is_some_and(|offer| offer.expires_at <= now);
        let gateway = state.gateway.as_ref().map(|gateway| {
            json!({
                "connected": gateway.connected,
                "advertisedAddress": gateway.advertised_address,
                "loopbackOnly": gateway.loopback_only,
                "attachedDevices": gateway.attached_devices,
                "connectedDevices": gateway.connected_devices,
            })
        });
        let directory = state
            .lifecycle
            .as_ref()
            .map(RemoteLifecycle::config_directory)
            .or_else(crate::config::get_config_dir);
        let devices = state
            .lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.devices().ok())
            .unwrap_or_default();
        let enabled = directory.as_deref().is_some_and(service::enabled);
        let pairing_pending = state.pairing_due
            || (self.pairing_in_flight.load(Ordering::Acquire) && state.offer.is_none());
        json!({
            "available": true,
            "enabled": enabled,
            "shared": true,
            "sessionId": self.session.session_id,
            "registrationEpoch": self.registration.registration_epoch,
            "gateway": gateway,
            "offer": offer,
            "pairingPending": pairing_pending,
            "pairingExpired": expired,
            "pairingError": state.pairing_error,
            "error": state.closed,
            "pairedDevices": devices.len(),
            "devices": devices.iter().map(|device| json!({
                "id": device.id,
                "name": device.name,
            })).collect::<Vec<_>>(),
        })
    }

    async fn close(&self, reason: SessionCloseReason) {
        self.session.sharing.store(false, Ordering::Release);
        self.stop.cancel();
        let state = self.state.lock().await;
        let _ = state
            .link
            .outbound
            .try_send(OwnerMessage::Unregister { reason });
    }

    /// One pass of the owner loop: apply device commands, publish state, and
    /// start a turn when a device queued a prompt.
    async fn tick(&self) -> bool {
        let mut should_pair = false;
        let mut start_runner = false;
        let alive = {
            let mut state = self.state.lock().await;
            if let Some(reason) = state.closed.take() {
                state.closed = Some(reason);
                return false;
            }

            while let Ok(event) = state.events.try_recv() {
                state.publisher.observe(&event);
            }

            for _ in 0..8 {
                let command = match state.link.commands.try_recv() {
                    Ok(command) => command,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        state.closed = Some("the remote gateway closed the link".to_owned());
                        return false;
                    }
                };
                match command {
                    OwnerCommand::Request { request, reply } => {
                        let request_id = request.request_id.clone();
                        let mut cancel_token = self
                            .session
                            .turns
                            .active_token()
                            .unwrap_or_else(CancellationToken::new);
                        if request.operation.is_mutation() {
                            match apply_session_mutation(
                                &self.session.state,
                                &mut cancel_token,
                                &self.registration,
                                &request,
                            )
                            .await
                            {
                                Ok(mutation) => {
                                    let follow_up = mutation.follow_up;
                                    reply.send(
                                        RemoteResponse::new(request_id, mutation.result)
                                            .with_receipt(ReceiptState::Applied),
                                    );
                                    if follow_up == OwnerFollowUp::StartTurn {
                                        start_runner = true;
                                    }
                                }
                                Err(error) => {
                                    reply.send(
                                        RemoteResponse::error(request_id, error)
                                            .with_receipt(ReceiptState::Rejected),
                                    );
                                }
                            }
                        } else {
                            let app = self.session.state.lock().await;
                            let result = read_session(
                                &app,
                                &self.registration,
                                &request,
                                &ProjectionLimits::default(),
                            );
                            drop(app);
                            match result {
                                Ok(result) => reply.send(RemoteResponse::new(request_id, result)),
                                Err(error) => reply.send(RemoteResponse::error(request_id, error)),
                            }
                        }
                    }
                    OwnerCommand::SnapshotRequested => state.publisher.request_snapshot(),
                    OwnerCommand::Status(status) => {
                        should_pair |= status.connected && state.pairing_due;
                        state.gateway = Some(status);
                    }
                    OwnerCommand::Closed { reason } => {
                        state.closed = Some(reason);
                        return false;
                    }
                }
            }

            let app = self.session.state.lock().await;
            if !state.publisher.owns(&app) {
                return false;
            }
            let room_for_resync = state.link.outbound.capacity() > 1;
            let messages = state.publisher.publish(&app, room_for_resync);
            start_runner |= !self.session.runner.load(Ordering::Acquire)
                && !app.has_active_turn()
                && !app.pending_queue.is_empty();
            drop(app);

            for message in messages {
                match state.link.outbound.try_send(message) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        state.publisher.mark_lagging();
                        break;
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        state.closed = Some("the remote gateway closed the link".to_owned());
                        return false;
                    }
                }
            }
            should_pair |= state.pairing_due
                && state
                    .gateway
                    .as_ref()
                    .is_some_and(|gateway| gateway.connected);
            true
        };

        if !alive {
            return false;
        }
        if should_pair {
            self.pair_if_due().await;
        }
        if start_runner {
            self.session.ensure_runner();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::idle_status;

    #[test]
    fn requests_use_the_camel_case_acp_wire_shape() {
        let start: super::RemoteStartRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "session-1",
            "pair": true,
        }))
        .unwrap();
        assert_eq!(start.session_id, "session-1");
        assert!(start.pair);
        let status: super::RemoteStatusRequest =
            serde_json::from_value(serde_json::json!({"sessionId": "session-1"})).unwrap();
        assert_eq!(status.session_id, "session-1");
    }

    #[test]
    fn idle_status_is_explicit_about_not_sharing() {
        let value = idle_status("session-1");
        assert_eq!(value["available"], true);
        assert_eq!(value["shared"], false);
        assert_eq!(value["sessionId"], "session-1");
    }

    #[cfg(unix)]
    #[test]
    fn offer_exposes_only_what_the_popup_needs() {
        use super::PairingOffer;
        use crate::remote_gateway::control::OfferDetails;
        use crate::remote_gateway::handshake::Secret;
        use std::time::Instant;
        let offer = PairingOffer::new(OfferDetails {
            advertised_address: "192.168.1.4:8787".to_owned(),
            loopback_only: false,
            gateway_id: "gateway".to_owned(),
            credential: Secret::new("credential"),
            code: Secret::new("1234-5678"),
            expires_in_secs: 300,
            challenge: Some(7),
            locked_for_secs: None,
            qr_payload: Secret::new("payload"),
        });
        assert_eq!(offer.code, "1234-5678");
        assert_eq!(offer.qr_payload, "payload");
        assert_eq!(offer.address, "192.168.1.4:8787");
        assert!(offer.expires_at > Instant::now());
    }
}
