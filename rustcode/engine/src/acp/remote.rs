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

use super::clients::SessionClients;
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SessionUpdate, TextContent,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::AcpSession;
use super::session::{KnownTaskIds, SessionTurnState};
use crate::app::AppState;
use crate::controller::AgentUiEvent;
use crate::harness::session::PermissionFuture;
use crate::harness::session::{ClientSink, ExternalSession};
use crate::remote::SessionCloseReason;
use crate::remote::harness::HarnessMirror;
// The thread-start seam is the same on every host; only the gateway behind it
// is unix-only.
use crate::remote::{RemoteError, RemoteSessionInfo};
use rustcode_tasks::TaskEvent;
use std::pin::Pin;

// Shares and pairing offers are driven by the remote gateway, which the
// workspace builds only on unix; everywhere else the manager below is a
// stub with the same surface that reports the feature as unavailable.
use crate::remote::owner::{
    GatewayLinkStatus, OwnerCommand, OwnerConnector, OwnerLink, OwnerMessage, OwnerReply,
    default_connector, next_registration_epoch,
};
use crate::remote::{
    OwnerFollowUp, ProjectionLimits, PromptDisposition, ReceiptState, RemoteErrorCode,
    RemoteOperation, RemoteRequest, RemoteResponse, RemoteResult, RemoteSnapshot, SessionPublisher,
    SessionRegistration, apply_session_mutation, read_session,
};
use crate::remote_gateway::control::{
    DeviceSummary, MachineOfferDetails, MachineOfferStatus, OfferDetails,
};
use crate::remote_gateway::lifecycle::RemoteLifecycle;
use crate::remote_gateway::service;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(crate) const REMOTE_CAPABILITY_VERSION: u64 = 1;

/// Shared threads the gateway holds per host (`SessionHub`'s own bound). A
/// Mac with more threads open shares the ones used most recently.
pub(crate) const MAX_SHARED_SESSIONS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote", response = serde_json::Value)]
pub(crate) struct RemoteStatusRequest {
    /// The thread to report on; empty for the machine itself (Settings ›
    /// Remote access).
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/start", response = serde_json::Value)]
pub(crate) struct RemoteStartRequest {
    /// The thread to share; empty switches remote access on for the machine.
    #[serde(default)]
    pub session_id: String,
    /// Ask for a pairing offer as soon as the gateway is connected.
    #[serde(default)]
    pub pair: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/pair", response = serde_json::Value)]
pub(crate) struct RemotePairRequest {
    /// The thread whose share shows the code; empty pairs the machine.
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/stop", response = serde_json::Value)]
pub(crate) struct RemoteStopRequest {
    #[serde(default)]
    pub session_id: String,
}

/// Accept or deny a device's connection request (`connectRequests` in the
/// machine's status). The answer is the machine's status plus
/// `connectAnswer`; on acceptance it carries the code to show, the only copy.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/answer", response = serde_json::Value)]
pub(crate) struct RemoteAnswerRequest {
    pub id: String,
    pub accept: bool,
}

/// Create a connect code for another computer (Settings › Machines › Create
/// connect code), or withdraw the open one with `cancel`. The answer is the
/// machine's status plus `connectCode` (shown once) or `connectCodeError`.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/connect_code", response = serde_json::Value)]
pub(crate) struct RemoteConnectCodeRequest {
    #[serde(default)]
    pub cancel: bool,
}

/// Forget a paired device and close its connections. The answer is the
/// machine's status plus `revoked`, or `revokeError`.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/revoke", response = serde_json::Value)]
pub(crate) struct RemoteRevokeRequest {
    pub device_id: String,
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
    /// The clients that have the thread: a device's turn streams to them too.
    pub clients: SessionClients,
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
        clients: SessionClients,
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
            clients,
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
            let update = SessionUpdate::UserMessageChunk(crate::message_origin::user_chunk(
                prompt.clone(),
                None,
            ));
            let _ = session.clients.update(update);
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
            None,
            session.clients.clone(),
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
            let _ = session.clients.update(update);
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

/// Why a thread could not be shared. `Link` means the gateway could not be
/// reached at all (a machine-wide problem worth backing off from); `Refused`
/// is about this thread alone (it closed, or the app already shares as many
/// as it takes), so every other thread is still tried.
#[derive(Debug)]
pub(crate) enum StartError {
    Link(String),
    Refused(String),
}

impl StartError {
    pub(crate) fn into_acp(self) -> agent_client_protocol::Error {
        match self {
            StartError::Link(message) | StartError::Refused(message) => {
                agent_client_protocol::Error::internal_error().data(message)
            }
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct RemoteManager {
    inner: Arc<std::sync::Mutex<Inner>>,
}

/// How long a refused thread is left alone before it is tried again: the
/// reason may pass (another thread closed, the terminal that held it stopped).
const REFUSAL_TTL: Duration = Duration::from_secs(60);

struct Refusal {
    reason: String,
    until: Instant,
}

#[derive(Default)]
struct Inner {
    shares: std::collections::HashMap<String, Arc<Share>>,
    /// Threads the gateway would not take, and why, for as long as the reason
    /// may hold. Auto-share skips them and their status says so.
    refused: std::collections::HashMap<String, Refusal>,
    /// Starts a thread a device asked for. Set by an engine whose client shows
    /// threads (the desktop app); absent everywhere else, and then the shares
    /// do not advertise [`crate::remote::session_capability::HOST_THREADS`].
    thread_host: Option<ThreadHost>,
    /// The gateway link factory, replaced in tests that have no gateway: a
    /// share is otherwise opened against the machine's own gateway.
    #[cfg(test)]
    connector: Option<Arc<dyn Fn() -> Box<dyn OwnerConnector> + Send + Sync>>,
}

/// What a device asked for when it created a thread on this host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ThreadStart {
    /// The folder the gateway resolved, inside the host's allowed roots.
    pub path: Option<String>,
    /// The harness to run it on; absent is this host's default.
    pub harness: Option<String>,
    /// The model on that harness; absent is the harness's own choice.
    pub model: Option<String>,
}

type ThreadHost = Arc<
    dyn Fn(
            ThreadStart,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<RemoteSessionInfo, RemoteError>> + Send>,
        > + Send
        + Sync,
>;

struct Share {
    target: ShareTarget,
    registration: SessionRegistration,
    state: Mutex<ShareState>,
    events: mpsc::UnboundedSender<AgentUiEvent>,
    stop: CancellationToken,
    pairing_in_flight: AtomicBool,
}

struct ShareState {
    publisher: Publisher,
    link: OwnerLink,
    events: mpsc::UnboundedReceiver<AgentUiEvent>,
    gateway: Option<GatewayLinkStatus>,
    lifecycle: Option<RemoteLifecycle>,
    pairing_due: bool,
    offer: Option<PairingOffer>,
    pairing_error: Option<String>,
    closed: Option<String>,
}

/// What a share publishes. The built-in harness publishes events projected
/// from its state; a thread on an external harness publishes a snapshot
/// whenever its mirror changed.
enum Publisher {
    Acp(SessionPublisher),
    Harness {
        sequence: u64,
        /// Asked for by the gateway, or owed after a lagging link.
        snapshot_due: bool,
        lagging: bool,
    },
}

impl Publisher {
    fn observe(&mut self, event: &AgentUiEvent) {
        if let Publisher::Acp(publisher) = self {
            publisher.observe(event);
        }
    }

    fn request_snapshot(&mut self) {
        match self {
            Publisher::Acp(publisher) => publisher.request_snapshot(),
            Publisher::Harness { snapshot_due, .. } => *snapshot_due = true,
        }
    }

    fn mark_lagging(&mut self) {
        match self {
            Publisher::Acp(publisher) => publisher.mark_lagging(),
            Publisher::Harness { lagging, .. } => *lagging = true,
        }
    }

    fn owns(&self, state: &AppState) -> bool {
        match self {
            Publisher::Acp(publisher) => publisher.owns(state),
            Publisher::Harness { .. } => true,
        }
    }

    fn publish(&mut self, state: &AppState, room_for_resync: bool) -> Vec<OwnerMessage> {
        match self {
            Publisher::Acp(publisher) => publisher.publish(state, room_for_resync),
            Publisher::Harness { .. } => Vec::new(),
        }
    }

    fn sequence(&self) -> u64 {
        match self {
            Publisher::Acp(publisher) => publisher.sequence(),
            Publisher::Harness { sequence, .. } => *sequence,
        }
    }

    /// For a harness share: the snapshot owed this pass, with the reason a
    /// subscriber must discard what it holds (`None` for a plain update).
    /// `dirty` says the mirror changed since the last one.
    fn harness_snapshot(
        &mut self,
        dirty: bool,
        room_for_resync: bool,
    ) -> Option<Option<crate::remote::ResyncReason>> {
        let Publisher::Harness {
            sequence,
            snapshot_due,
            lagging,
        } = self
        else {
            return None;
        };
        if *lagging {
            if !room_for_resync {
                return None;
            }
            *lagging = false;
            *snapshot_due = false;
            *sequence += 1;
            return Some(Some(crate::remote::ResyncReason::Lagged));
        }
        if !*snapshot_due && !dirty {
            return None;
        }
        *snapshot_due = false;
        *sequence += 1;
        Some(None)
    }
}

/// One shared thread: on the built-in harness, or on an external one.
enum ShareTarget {
    Acp(Arc<RemoteSession>),
    Harness(Arc<HarnessShare>),
}

impl ShareTarget {
    /// Which harness the shared thread runs on: `rustcode` for the built-in
    /// one, the harness id otherwise.
    fn kind(&self) -> &str {
        match self {
            ShareTarget::Acp(_) => crate::harness::BUILTIN_ID,
            ShareTarget::Harness(share) => share.mirror.harness_id(),
        }
    }

    fn session_id(&self) -> &str {
        match self {
            ShareTarget::Acp(session) => &session.session_id,
            ShareTarget::Harness(share) => &share.session_id,
        }
    }

    fn stop_sharing(&self) {
        match self {
            ShareTarget::Acp(session) => session.sharing.store(false, Ordering::Release),
            ShareTarget::Harness(share) => share.mirror.close(),
        }
    }
}

/// A thread on an external harness (`docs/harnesses.md`), shared from the
/// mirror of the updates it sends. Prompts, cancels, steers, approvals and
/// question answers are applied to the harness itself, through the same
/// [`ExternalSession`] the desktop app's own prompt goes through.
struct HarnessShare {
    session_id: String,
    external: Arc<ExternalSession>,
    mirror: Arc<HarnessMirror>,
    /// The app's clients that have the thread: a prompt a device sends is
    /// echoed to them as a `user_message_chunk`, so the chat shows what the
    /// phone did. `None` in a test that has no ACP connection.
    clients: Option<SessionClients>,
}

struct PairingOffer {
    qr_payload: String,
    code: String,
    address: String,
    /// Other addresses the listener answers on, also in the QR payload.
    alternates: Vec<String>,
    loopback_only: bool,
    gateway_id: String,
    instance_id: Option<String>,
    challenge: Option<u64>,
    locked_for_secs: Option<u64>,
    expires_at: Instant,
}

impl PairingOffer {
    fn new(offer: OfferDetails) -> Self {
        let lifetime = Duration::from_secs(offer.expires_in_secs);
        Self {
            qr_payload: offer.qr_payload.expose().to_owned(),
            code: offer.code.expose().to_owned(),
            address: offer.advertised_address,
            alternates: offer.alternates,
            loopback_only: offer.loopback_only,
            gateway_id: offer.gateway_id,
            instance_id: offer.instance_id,
            challenge: offer.challenge,
            locked_for_secs: offer.locked_for_secs,
            expires_at: Instant::now() + lifetime,
        }
    }
}

impl RemoteManager {
    fn connector(&self) -> Box<dyn OwnerConnector> {
        #[cfg(test)]
        if let Ok(inner) = self.inner.lock()
            && let Some(make) = &inner.connector
        {
            return make();
        }
        default_connector()
    }

    /// The manager with a link factory of its own (tests).
    #[cfg(test)]
    fn with_connector(make: impl Fn() -> Box<dyn OwnerConnector> + Send + Sync + 'static) -> Self {
        let manager = Self::default();
        if let Ok(mut inner) = manager.inner.lock() {
            inner.connector = Some(Arc::new(make));
        }
        manager
    }

    /// Give a manager that already serves threads a link factory of its own (tests of the
    /// engine's request path, which cannot reach a gateway).
    #[cfg(test)]
    pub(crate) fn use_connector(
        &self,
        make: impl Fn() -> Box<dyn OwnerConnector> + Send + Sync + 'static,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.connector = Some(Arc::new(make));
        }
    }

    fn get(&self, session_id: &str) -> Option<Arc<Share>> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.shares.get(session_id).cloned())
    }

    fn forget(&self, share: &Arc<Share>) {
        let session_id = share.target.session_id();
        if let Ok(mut inner) = self.inner.lock()
            && inner
                .shares
                .get(session_id)
                .is_some_and(|current| Arc::ptr_eq(current, share))
        {
            inner.shares.remove(session_id);
        }
        share.target.stop_sharing();
    }

    /// Whether `session_id` is shared right now.
    pub(crate) fn is_shared(&self, session_id: &str) -> bool {
        self.get(session_id).is_some()
    }

    /// Say how a thread a device asks for is started: the desktop app's engine
    /// sets this, and only then do its shares advertise
    /// [`crate::remote::session_capability::HOST_THREADS`] and the gateway
    /// route `create_session` here instead of starting a session owner.
    pub(crate) fn set_thread_host(
        &self,
        start: impl Fn(
            ThreadStart,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<RemoteSessionInfo, RemoteError>> + Send>,
        > + Send
        + Sync
        + 'static,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.thread_host = Some(Arc::new(start));
        }
    }

    /// Stop advertising thread creation when no connected client can list the
    /// thread that would be started.
    pub(crate) fn clear_thread_host(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.thread_host = None;
        }
    }

    /// Whether this engine starts threads for devices.
    pub(crate) fn can_host_threads(&self) -> bool {
        self.inner
            .lock()
            .is_ok_and(|inner| inner.thread_host.is_some())
    }

    fn thread_host(&self) -> Option<ThreadHost> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.thread_host.clone())
    }

    /// Answer a device's `create_session`: the client starts the thread, the
    /// share of it follows, and the row is what the device attaches to. The
    /// owner's own receipt store (`remote_gateway::owner_client`) records the
    /// request before it reaches this share, so a retry is answered from it
    /// and nothing is started twice.
    async fn host_thread(&self, request: RemoteRequest, reply: OwnerReply) {
        let request_id = request.request_id.clone();
        let Some(host) = self.thread_host() else {
            reply.send(
                RemoteResponse::error(
                    request_id,
                    RemoteError::new(
                        RemoteErrorCode::UnsupportedOperation,
                        "this host does not start threads for devices",
                    ),
                )
                .with_receipt(ReceiptState::Rejected),
            );
            return;
        };
        let RemoteOperation::CreateSession {
            path,
            harness,
            model,
        } = request.operation
        else {
            reply.send(
                RemoteResponse::error(
                    request_id,
                    RemoteError::new(
                        RemoteErrorCode::UnsupportedOperation,
                        "only `create_session` is answered here",
                    ),
                )
                .with_receipt(ReceiptState::Rejected),
            );
            return;
        };
        let started = host(ThreadStart {
            path,
            harness,
            model,
        })
        .await;
        reply.send(match started {
            Ok(session) => {
                RemoteResponse::new(request_id, RemoteResult::SessionCreated { session })
                    .with_receipt(ReceiptState::Applied)
            }
            Err(error) => {
                RemoteResponse::error(request_id, error).with_receipt(ReceiptState::Rejected)
            }
        });
    }

    /// The row of a thread this engine just started, for the answer to
    /// `create_session`: the share's own view when it has one, else what the
    /// session is known to be at its first moment.
    pub(crate) fn thread_info(&self, session_id: &str) -> Option<RemoteSessionInfo> {
        let share = self.get(session_id)?;
        let epoch = share.registration.registration_epoch;
        if let Some(harness) = share.harness() {
            return Some(
                harness
                    .mirror
                    .snapshot(&share.registration, 0, &ProjectionLimits::default())
                    .session,
            );
        }
        let session = share.acp()?;
        let model = session
            .state
            .try_lock()
            .ok()
            .map(|state| crate::remote::projection::project_settings(&state).selected_model)
            .unwrap_or_default();
        Some(RemoteSessionInfo {
            session_id: session_id.to_owned(),
            registration_epoch: epoch,
            title: crate::config::load_session_title(session_id).unwrap_or_default(),
            workspace: Some(session.cwd.display().to_string()),
            model,
            harness: Some(crate::harness::BUILTIN_ID.to_owned()),
            turn_count: Some(0),
            last_active_at: None,
            parent: crate::remote::session_parent(session_id),
            activity: crate::remote::SessionActivity::Idle,
            attention: crate::remote::RemoteAttention {
                approval: false,
                question: false,
            },
            health: crate::remote::OwnerHealth::Live,
        })
    }

    /// Note that this thread could not be shared, and why, so the next pass
    /// does not retry it at once and the thread's status can say why.
    pub(crate) fn refuse(&self, session_id: &str, reason: String) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.refused.insert(
                session_id.to_owned(),
                Refusal {
                    reason,
                    until: Instant::now() + REFUSAL_TTL,
                },
            );
        }
    }

    /// What the machine's status adds from this manager: how many open
    /// threads it holds back, and why (Settings shows the count).
    fn host_extras(&self, mut status: Value) -> Value {
        if let Some(object) = status.as_object_mut() {
            object.insert("notShared".to_owned(), json!(self.refused_count()));
        }
        status
    }

    /// Why this thread is not shared, while the reason still stands.
    pub(crate) fn refusal(&self, session_id: &str) -> Option<String> {
        let Ok(mut inner) = self.inner.lock() else {
            return None;
        };
        match inner.refused.get(session_id) {
            Some(refusal) if refusal.until > Instant::now() => Some(refusal.reason.clone()),
            Some(_) => {
                inner.refused.remove(session_id);
                None
            }
            None => None,
        }
    }

    /// How many threads are held back with a reason (the machine's status).
    pub(crate) fn refused_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| {
                let now = Instant::now();
                inner
                    .refused
                    .values()
                    .filter(|refusal| refusal.until > now)
                    .count()
            })
            .unwrap_or(0)
    }

    /// The mirror of a shared thread on an external harness, for the sink that
    /// feeds it and for the ACP handlers that note what the app sends.
    pub(crate) fn mirror(&self, session_id: &str) -> Option<Arc<HarnessMirror>> {
        match &self.get(session_id)?.target {
            ShareTarget::Harness(share) => Some(Arc::clone(&share.mirror)),
            ShareTarget::Acp(_) => None,
        }
    }

    /// Note a prompt the app sent to a shared thread, so a device reading the
    /// transcript sees it (a harness does not echo the prompt back).
    pub(crate) fn note_prompt(
        &self,
        session_id: &str,
        text: &str,
        origin: Option<serde_json::Value>,
    ) {
        if let Some(mirror) = self.mirror(session_id) {
            mirror.note_prompt_with_origin(text, origin);
        }
    }

    /// Note that a prompt was handed to a harness and its turn has not
    /// returned yet, so the mirror does not close the turn early.
    pub(crate) fn note_prompt_applied(&self, session_id: &str) {
        if let Some(mirror) = self.mirror(session_id) {
            mirror.note_prompt_applied();
        }
    }

    pub(crate) fn note_turn_returned(&self, session_id: &str) {
        if let Some(mirror) = self.mirror(session_id) {
            mirror.note_turn_returned();
        }
    }

    /// Whether any thread is shared. Used to know whether switching remote
    /// access off has anything to end.
    pub(crate) fn shared_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.shares.len())
            .unwrap_or(0)
    }

    /// The sink a thread on an external harness sends its updates through:
    /// the client's own sink, with every update fed to the mirror first and
    /// permission requests offered to paired devices.
    pub(crate) fn wrap_sink(
        &self,
        inner: Arc<dyn ClientSink>,
        session_id: &str,
    ) -> Arc<dyn ClientSink> {
        Arc::new(TeeSink {
            inner,
            session_id: session_id.to_owned(),
            remote: self.clone(),
        })
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
    ) -> Result<Value, StartError> {
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
        let mut connector = self.connector();
        let directory = connector.gateway_directory();
        let link = connector
            .connect(&registration)
            .map_err(|error| StartError::Link(error.to_string()))?;
        let lifecycle = directory.map(RemoteLifecycle::new);

        let (events, receiver) = mpsc::unbounded_channel();
        let share_state = {
            let state = session.state.lock().await;
            if state.active_session_id != session.session_id {
                return Err(StartError::Refused(
                    "the ACP session is no longer active".to_owned(),
                ));
            }
            let (publisher, register) =
                SessionPublisher::register(&state, registration.clone(), state.has_active_turn());
            let publisher = Publisher::Acp(publisher);
            drop(state);
            if let Err(error) = link.outbound.try_send(register) {
                return Err(StartError::Link(format!(
                    "could not register the remote session: {error}"
                )));
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
            target: ShareTarget::Acp(Arc::clone(&session)),
            registration,
            state: Mutex::new(share_state),
            events,
            stop: CancellationToken::new(),
            pairing_in_flight: AtomicBool::new(false),
        });
        session.sharing.store(true, Ordering::Release);
        if self.inner.lock().is_err() {
            return Err(StartError::Link("remote manager is unavailable".to_owned()));
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .shares
                .insert(session.session_id.clone(), Arc::clone(&share));
            inner.refused.remove(&session.session_id);
        }
        tokio::spawn(share_loop(self.clone(), share));
        Ok(self.status(&session.session_id).await)
    }

    /// Share a thread that runs on an external harness (`docs/harnesses.md`).
    /// Its updates are mirrored; a prompt, cancel or answer from a device is
    /// applied through the same [`ExternalSession`] the app's own prompt uses.
    pub(crate) async fn start_harness(
        &self,
        session_id: String,
        external: Arc<ExternalSession>,
        clients: Option<SessionClients>,
        pair: bool,
    ) -> Result<Value, StartError> {
        if let Some(share) = self.get(&session_id) {
            if pair {
                share.request_pair().await;
            }
            return Ok(self.status(&session_id).await);
        }
        // A thread the app closed while this pass was connecting has nothing
        // to share: sharing it now would leave a registration no one removes.
        if external.is_closed() {
            return Err(StartError::Refused(
                "the thread was closed before it could be shared".to_owned(),
            ));
        }

        let record = external.record();
        let title = record
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .or_else(|| crate::config::load_session_title(&session_id))
            .unwrap_or_default();
        // What the device shows as the thread's model: the harness's own id
        // for the model, absent when the record has none (a thread that has
        // not prompted yet).
        let model = record.model.clone().unwrap_or_default();
        let mirror = Arc::new(HarnessMirror::new(
            session_id.clone(),
            external.harness.id.clone(),
            external.cwd.clone(),
            model,
            title,
            external.recorded_history(),
            // What the harness advertises: a steer it does not take is refused
            // rather than offered in the snapshot.
            crate::harness::capabilities(&external.harness)
                .steering
                .live,
        ));
        let registration = SessionRegistration {
            session_id: session_id.clone(),
            registration_epoch: next_registration_epoch(),
        };
        let mut connector = self.connector();
        let directory = connector.gateway_directory();
        let link = connector
            .connect(&registration)
            .map_err(|error| StartError::Link(error.to_string()))?;
        let lifecycle = directory.map(RemoteLifecycle::new);
        let mut snapshot = mirror.snapshot(&registration, 0, &ProjectionLimits::default());
        if self.can_host_threads() {
            advertise_host_threads(&mut snapshot);
        }
        let register = OwnerMessage::Register {
            registration: registration.clone(),
            snapshot: Box::new(snapshot),
        };
        if let Err(error) = link.outbound.try_send(register) {
            return Err(StartError::Link(format!(
                "could not register the remote session: {error}"
            )));
        }
        let (events, receiver) = mpsc::unbounded_channel();
        let share = Arc::new(Share {
            target: ShareTarget::Harness(Arc::new(HarnessShare {
                session_id: session_id.clone(),
                external,
                mirror,
                clients,
            })),
            registration,
            state: Mutex::new(ShareState {
                publisher: Publisher::Harness {
                    sequence: 0,
                    snapshot_due: false,
                    lagging: false,
                },
                link,
                events: receiver,
                gateway: None,
                lifecycle,
                pairing_due: pair,
                offer: None,
                pairing_error: None,
                closed: None,
            }),
            events,
            stop: CancellationToken::new(),
            pairing_in_flight: AtomicBool::new(false),
        });
        if self.inner.lock().is_err() {
            return Err(StartError::Link("remote manager is unavailable".to_owned()));
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.shares.insert(session_id.clone(), Arc::clone(&share));
            inner.refused.remove(&session_id);
        }
        tokio::spawn(share_loop(self.clone(), share));
        Ok(self.status(&session_id).await)
    }

    pub(crate) async fn status(&self, session_id: &str) -> Value {
        let Some(share) = self.get(session_id) else {
            let mut status = self.host_extras(host_status().await);
            if let Some(object) = status.as_object_mut() {
                object.insert("sessionId".to_owned(), json!(session_id));
                object.insert("shared".to_owned(), json!(false));
                object.insert("notShared".to_owned(), json!(self.refusal(session_id)));
            }
            return status;
        };
        share.pair_if_due().await;
        share.status_value().await
    }

    /// Ask for a pairing challenge. With no thread named (Settings › Remote
    /// access) the machine's gateway is switched on first, so a device can
    /// pair before any thread is shared.
    pub(crate) async fn pair(&self, session_id: &str) -> Value {
        let Some(share) = self.get(session_id) else {
            return self.host_extras(pair_host().await);
        };
        share.request_pair().await;
        share.pair_if_due().await;
        share.status_value().await
    }

    /// Switch remote access on for the machine with no thread to share yet:
    /// the gateway starts, and every open thread shares itself from the next
    /// pass of the ACP frontend's auto-share loop.
    pub(crate) async fn enable(&self) -> Value {
        match service::enable_for_host().await {
            Ok(()) => self.host_extras(host_status().await),
            Err(error) => json!({
                "available": true,
                "enabled": remote_enabled(),
                "error": format!("{error:#}"),
            }),
        }
    }

    /// Accept or deny a device that asked to connect.
    pub(crate) async fn answer_connect(&self, id: &str, accept: bool) -> Value {
        self.host_extras(answer_connect_host(id, accept).await)
    }

    /// Forget a paired device.
    pub(crate) async fn revoke(&self, device_id: &str) -> Value {
        self.host_extras(revoke_host(device_id).await)
    }

    /// End every share, because remote access was switched off.
    pub(crate) async fn unshare_all(&self, reason: SessionCloseReason) {
        let shares = self
            .inner
            .lock()
            .map(|mut inner| {
                inner.refused.clear();
                inner
                    .shares
                    .drain()
                    .map(|(_, share)| share)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for share in shares {
            share.close(reason).await;
        }
    }

    /// Turn remote access off for this machine, ending every share. It is
    /// authoritative: the gateway is stopped and the machine-wide switch is
    /// written off even when this app had nothing shared, so the next pass
    /// cannot share a thread again two seconds later.
    pub(crate) async fn stop(
        &self,
        session_id: &str,
    ) -> Result<Value, agent_client_protocol::Error> {
        // Threads first (their registrations leave the gateway's list), then
        // the gateway itself.
        let shares = {
            let mut inner = self.inner.lock().map_err(|_| {
                agent_client_protocol::Error::internal_error().data("remote manager is unavailable")
            })?;
            inner.refused.clear();
            inner
                .shares
                .drain()
                .map(|(_, share)| share)
                .collect::<Vec<_>>()
        };
        let directory = crate::config::get_config_dir()
            .or_else(|| shares.first().and_then(|share| share.directory()));
        for share in &shares {
            share.close(SessionCloseReason::SharingDisabled).await;
        }
        let Some(directory) = directory else {
            return Err(agent_client_protocol::Error::internal_error()
                .data("no configuration directory: remote access could not be switched off"));
        };
        service::disable(&directory).await.map_err(|error| {
            agent_client_protocol::Error::internal_error().data(error.to_string())
        })?;
        Ok(self.status(session_id).await)
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

/// Whether remote access is switched on for this machine. While it is, every
/// open ACP thread shares itself, like a terminal does.
pub(crate) fn enabled() -> bool {
    remote_enabled()
}

fn remote_enabled() -> bool {
    crate::config::get_config_dir()
        .as_deref()
        .is_some_and(service::enabled)
}

/// What Settings › Remote access shows without naming a thread: whether
/// remote access is on, the gateway's address and state, the paired devices
/// and where pairing stands. Reads the gateway's own status, so it is the
/// same answer `rustcode remote status` gives.
async fn host_status() -> Value {
    let Some(directory) = crate::config::get_config_dir() else {
        return json!({
            "available": true,
            "enabled": false,
            "shared": false,
            "pairedDevices": 0,
            "devices": [],
            "error": "no configuration directory",
        });
    };
    let lifecycle = RemoteLifecycle::new(&directory);
    let enabled = service::enabled(&directory);
    let status = lifecycle.status().await.ok().flatten();
    let devices = status
        .as_ref()
        .map(|status| status.devices.clone())
        .unwrap_or_else(|| {
            lifecycle
                .devices()
                .unwrap_or_default()
                .iter()
                .map(DeviceSummary::from)
                .collect()
        });
    let connected = |id: &str| {
        status.as_ref().is_some_and(|status| {
            status
                .connections
                .iter()
                .any(|connection| connection.device_id == id)
        })
    };
    let gateway = status.as_ref().map(|status| {
        json!({
            "connected": true,
            "instanceId": status.instance_id,
            "advertisedAddress": status.advertised_address,
            "listenAddress": status.listen_address,
            // Every address the gateway answers on, with its interface: an
            // `auto` bind opens the LAN and any overlay, and a machine that
            // cannot reach one of them needs to see the other.
            "listenAddresses": status.listening,
            "loopbackOnly": status.loopback_only,
            "routes": status.routes,
            "connectedDevices": status.connections.iter().map(|connection| connection.device_name.clone()).collect::<Vec<_>>(),
            "attachedDevices": Vec::<String>::new(),
            "pairedDevices": status.paired_devices,
            "pairingOpen": status.pairing_open,
            // The gateway's own status is snake_case; this answer is
            // camelCase like every other field the apps read.
            "pairing": status.pairing.as_ref().map(|pairing| json!({
                "expiresInSecs": pairing.expires_in_secs,
                "attemptsLeft": pairing.attempts_left,
                "lockedForSecs": pairing.locked_for_secs,
                "challenge": pairing.challenge,
                "lastChallenge": pairing.last_challenge,
            })),
            "sessions": status.sessions.as_ref().map(|sessions| sessions.len()).unwrap_or(0),
            // The connect code for another computer, apart from the
            // challenge: open, and how the last one ended.
            "machineOffer": status.machine_offer.as_ref().map(machine_offer_status_json),
        })
    });
    let connect_requests = status
        .as_ref()
        .map(|status| {
            status
                .connect_requests
                .iter()
                .map(|request| {
                    json!({
                        "id": request.id,
                        "deviceName": request.device_name,
                        "deviceKind": request.device_kind,
                        "platform": request.platform,
                        "remoteAddr": request.remote_addr,
                        "ageSecs": request.age_secs,
                        "expiresInSecs": request.expires_in_secs,
                        "state": request.state,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ended_connect_requests = status
        .as_ref()
        .map(|status| {
            status
                .ended_connect_requests
                .iter()
                .map(|ended| {
                    json!({
                        "id": ended.id,
                        "deviceName": ended.device_name,
                        "end": ended.end,
                        "deviceId": ended.device_id,
                        "endedSecsAgo": ended.ended_secs_ago,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Why a device did not get in, newest first. This is what lets the page
    // say "a phone tried to connect with an older app version" instead of
    // showing nothing while the phone only says it cannot connect.
    let refused_connections = status
        .as_ref()
        .map(|status| {
            status
                .refused_connections
                .iter()
                .map(|refused| {
                    json!({
                        "reason": refused.reason,
                        "attempts": refused.attempts,
                        "deviceName": refused.device_name,
                        "deviceKind": refused.device_kind,
                        "platform": refused.platform,
                        "protocolVersion": refused.protocol_version,
                        "supportedVersions": refused.supported_versions,
                        "remoteAddr": refused.remote_addr,
                        "refusedSecsAgo": refused.refused_secs_ago,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "available": true,
        "enabled": enabled,
        "shared": false,
        "gateway": gateway,
        "connectRequests": connect_requests,
        "endedConnectRequests": ended_connect_requests,
        "refusedConnections": refused_connections,
        "pairedDevices": devices.len(),
        "devices": devices
            .iter()
            .map(|device| device_json(device, Some(connected(&device.id))))
            .collect::<Vec<_>>(),
    })
}

/// A paired device as the apps read it: what it is, where it runs and when
/// it was last connected. Never a token or its digest. `connected` is
/// `None` where the answer cannot tell.
fn device_json(device: &DeviceSummary, connected: Option<bool>) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "kind": device.kind,
        "platform": device.platform,
        "hostId": device.host_id,
        "lastSeenAt": device.last_seen_at,
        "connected": connected,
    })
}

/// A pairing challenge for the machine, with no thread named: Settings ›
/// Remote access › Pair a device. Switches remote access on when it is off,
/// exactly as `/remote` does in a terminal.
async fn pair_host() -> Value {
    if let Err(error) = service::enable_for_host().await {
        return json!({
            "available": true,
            "enabled": remote_enabled(),
            "shared": false,
            "pairingError": format!("{error:#}"),
        });
    }
    let Some(directory) = crate::config::get_config_dir() else {
        return json!({
            "available": true,
            "enabled": false,
            "shared": false,
            "pairingError": "no configuration directory",
        });
    };
    let lifecycle = RemoteLifecycle::new(&directory);
    let result = tokio::time::timeout(Duration::from_secs(15), lifecycle.pair()).await;
    let mut status = host_status().await;
    let Some(object) = status.as_object_mut() else {
        return status;
    };
    object.insert("shared".to_owned(), json!(false));
    object.insert("pairingPending".to_owned(), json!(false));
    object.insert("offer".to_owned(), Value::Null);
    match result {
        Ok(Ok(offer)) => {
            let offer = PairingOffer::new(offer);
            object.insert("offer".to_owned(), offer_json(&offer, Instant::now()));
            object.insert("pairingError".to_owned(), Value::Null);
        }
        Ok(Err(error)) => {
            object.insert("pairingError".to_owned(), json!(error.to_string()));
        }
        Err(_) => {
            object.insert(
                "pairingError".to_owned(),
                json!("the remote gateway did not answer in time"),
            );
        }
    }
    status
}

fn machine_offer_status_json(offer: &MachineOfferStatus) -> Value {
    json!({
        "open": offer.open.map(|open| json!({
            "serial": open.serial,
            "expiresInSecs": open.expires_in_secs,
            "attemptsLeft": open.attempts_left,
        })),
        "lastEnded": offer.last_ended.as_ref().map(|ended| json!({
            "serial": ended.serial,
            "end": ended.end,
            "deviceId": ended.device_id,
            "deviceName": ended.device_name,
        })),
    })
}

/// A connect code for another computer, or the open one withdrawn. Creating
/// one switches remote access on when it is off, as pairing a phone does: the
/// other computer reaches this one through the gateway.
pub(crate) async fn connect_code(cancel: bool) -> Value {
    let Some(directory) = crate::config::get_config_dir() else {
        return json!({
            "available": true,
            "enabled": false,
            "connectCodeError": "no configuration directory",
        });
    };
    let lifecycle = RemoteLifecycle::new(&directory);
    if cancel {
        let cancelled = lifecycle.cancel_machine_offer().await;
        let mut status = host_status().await;
        if let Some(object) = status.as_object_mut() {
            match cancelled {
                Ok(was_open) => object.insert("connectCodeCancelled".to_owned(), json!(was_open)),
                Err(error) => {
                    object.insert("connectCodeError".to_owned(), json!(format!("{error:#}")))
                }
            };
        }
        return status;
    }
    if let Err(error) = service::enable_for_host().await {
        let mut status = host_status().await;
        if let Some(object) = status.as_object_mut() {
            object.insert("connectCodeError".to_owned(), json!(format!("{error:#}")));
        }
        return status;
    }
    let result = tokio::time::timeout(Duration::from_secs(15), lifecycle.machine_offer()).await;
    let mut status = host_status().await;
    let Some(object) = status.as_object_mut() else {
        return status;
    };
    match result {
        Ok(Ok(offer)) => {
            object.insert("connectCode".to_owned(), connect_code_json(&offer));
        }
        Ok(Err(error)) => {
            object.insert("connectCodeError".to_owned(), json!(format!("{error:#}")));
        }
        Err(_) => {
            object.insert(
                "connectCodeError".to_owned(),
                json!("the remote gateway did not answer in time"),
            );
        }
    }
    status
}

fn connect_code_json(offer: &MachineOfferDetails) -> Value {
    json!({
        "code": offer.code.expose(),
        "link": offer.link.expose(),
        "address": offer.address,
        "addressInCode": offer.address_in_code,
        "gatewayId": offer.gateway_id,
        "instanceId": offer.instance_id,
        "hostName": offer.host_name,
        "routes": offer.routes,
        "loopbackOnly": offer.loopback_only,
        "expiresInSecs": offer.expires_in_secs,
        "serial": offer.serial,
        "lockedForSecs": offer.locked_for_secs,
    })
}

/// Settings › Remote access, or the prompt anywhere in the app: the owner's
/// answer to a device asking to connect, with the machine's status.
async fn answer_connect_host(id: &str, accept: bool) -> Value {
    let result = match crate::config::get_config_dir() {
        Some(directory) => RemoteLifecycle::new(&directory)
            .answer_connect_request(id, accept)
            .await
            .map_err(|error| format!("{error:#}")),
        None => Err("no configuration directory".to_owned()),
    };
    let mut status = host_status().await;
    if let Some(object) = status.as_object_mut() {
        match result {
            Ok(answer) => {
                object.insert(
                    "connectAnswer".to_owned(),
                    json!({
                        "id": answer.id,
                        "deviceName": answer.device_name,
                        "instanceId": answer.instance_id,
                        "accepted": answer.accepted,
                        "code": answer.code.as_ref().map(|code| code.expose()),
                        "expiresInSecs": answer.expires_in_secs,
                        "lockedForSecs": answer.locked_for_secs,
                    }),
                );
            }
            Err(error) => {
                object.insert("connectError".to_owned(), json!(error));
            }
        }
    }
    status
}

/// Settings › Remote access › Revoke: forget a device, as `rustcode remote
/// revoke` does, and answer with the machine's status.
async fn revoke_host(device_id: &str) -> Value {
    let result = match crate::config::get_config_dir() {
        Some(directory) => RemoteLifecycle::new(&directory)
            .revoke(device_id)
            .await
            .map_err(|error| format!("{error:#}")),
        None => Err("no configuration directory".to_owned()),
    };
    let mut status = host_status().await;
    if let Some(object) = status.as_object_mut() {
        match result {
            Ok(revocation) => {
                object.insert(
                    "revoked".to_owned(),
                    json!({
                        "deviceId": revocation.device_id,
                        "deviceName": revocation.device_name,
                    }),
                );
            }
            Err(error) => {
                object.insert("revokeError".to_owned(), json!(error));
            }
        }
    }
    status
}

/// One pairing offer as the apps render it. `qrRows` is the code itself, one
/// string per row of `0` (light) and `1` (dark) modules with the four-module
/// quiet zone included, so a client draws it without a QR encoder.
fn offer_json(offer: &PairingOffer, now: Instant) -> Value {
    json!({
        "qrPayload": offer.qr_payload,
        "qrRows": crate::remote_gateway::qr::bitmap(&offer.qr_payload),
        "code": offer.code,
        "address": offer.address,
        "alternates": offer.alternates,
        "expiresInSecs": offer.expires_at.saturating_duration_since(now).as_secs(),
        "loopbackOnly": offer.loopback_only,
        "gatewayId": offer.gateway_id,
        "instanceId": offer.instance_id,
        "challenge": offer.challenge,
        "lockedForSecs": offer.locked_for_secs,
    })
}

/// Say on a share's snapshot that its owner starts threads a device asks for
/// (`create_session`): what makes the gateway offer `thread_create_options`
/// and route new threads here instead of starting a session owner.
fn advertise_host_threads(snapshot: &mut RemoteSnapshot) {
    let capability = crate::remote::session_capability::HOST_THREADS;
    if !snapshot
        .capabilities
        .iter()
        .any(|known| known == capability)
    {
        snapshot.capabilities.push(capability.to_owned());
    }
}

async fn share_loop(manager: RemoteManager, share: Arc<Share>) {
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = share.stop.cancelled() => break,
            _ = interval.tick() => {}
        }
        if !share.tick(&manager).await {
            break;
        }
    }
    // A share the gateway closed (the host already shares as many threads as
    // it takes, or another owner holds this one) is refused, not broken: the
    // reason is kept so the thread's status can say it.
    if !share.stop.is_cancelled()
        && let Some(reason) = share.state.lock().await.closed.clone()
    {
        manager.refuse(share.target.session_id(), reason);
    }
    manager.forget(&share);
}

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
            .map(|offer| offer_json(offer, now));
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
            "target": self.target.kind(),
            "sessionId": self.target.session_id(),
            "registrationEpoch": self.registration.registration_epoch,
            "gateway": gateway,
            "offer": offer,
            "pairingPending": pairing_pending,
            "pairingExpired": expired,
            "pairingError": state.pairing_error,
            "error": state.closed,
            "pairedDevices": devices.len(),
            "devices": devices
                .iter()
                .map(|device| device_json(&DeviceSummary::from(device), None))
                .collect::<Vec<_>>(),
        })
    }

    async fn close(&self, reason: SessionCloseReason) {
        self.target.stop_sharing();
        self.stop.cancel();
        let state = self.state.lock().await;
        let _ = state
            .link
            .outbound
            .try_send(OwnerMessage::Unregister { reason });
    }

    /// One pass of the owner loop: apply device commands, publish state, and
    /// start a turn when a device queued a prompt.
    async fn tick(&self, manager: &RemoteManager) -> bool {
        if let Some(share) = self.harness() {
            let share = Arc::clone(share);
            return self.tick_harness(&share, manager).await;
        }
        let Some(session) = self.acp().cloned() else {
            return false;
        };
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
                        // A thread the device asked for is started by the
                        // client, not by this share: it answers on its own
                        // once the thread exists.
                        if matches!(request.operation, RemoteOperation::CreateSession { .. }) {
                            let manager = manager.clone();
                            tokio::spawn(async move { manager.host_thread(request, reply).await });
                            continue;
                        }
                        let request_id = request.request_id.clone();
                        let mut cancel_token = session
                            .turns
                            .active_token()
                            .unwrap_or_else(CancellationToken::new);
                        if request.operation.is_mutation() {
                            match apply_session_mutation(
                                &session.state,
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
                        } else if matches!(
                            request.operation,
                            crate::remote::protocol::RemoteOperation::GetChanges
                        ) {
                            // Git may take a moment: read it off the owner's
                            // loop, with no lock held.
                            let app = session.state.lock().await;
                            let workspace = crate::remote::ops::changes_workspace(
                                &app,
                                &self.registration,
                                &request,
                            );
                            drop(app);
                            match workspace {
                                Ok(root) => {
                                    tokio::spawn(async move {
                                        let changes = crate::remote::changes::read(
                                            &root,
                                            chrono::Utc::now().to_rfc3339(),
                                        )
                                        .await;
                                        reply.send(RemoteResponse::new(
                                            request_id,
                                            crate::remote::protocol::RemoteResult::Changes(changes),
                                        ));
                                    });
                                }
                                Err(error) => reply.send(RemoteResponse::error(request_id, error)),
                            }
                        } else {
                            let app = session.state.lock().await;
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

            let app = session.state.lock().await;
            if !state.publisher.owns(&app) {
                return false;
            }
            let room_for_resync = state.link.outbound.capacity() > 1;
            let mut messages = state.publisher.publish(&app, room_for_resync);
            if manager.can_host_threads() {
                for message in &mut messages {
                    if let OwnerMessage::Snapshot { snapshot, .. } = message {
                        advertise_host_threads(snapshot);
                    }
                }
            }
            start_runner |= !session.runner.load(Ordering::Acquire)
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
            session.ensure_runner();
        }
        true
    }

    fn acp(&self) -> Option<&Arc<RemoteSession>> {
        match &self.target {
            ShareTarget::Acp(session) => Some(session),
            ShareTarget::Harness(_) => None,
        }
    }

    fn harness(&self) -> Option<&Arc<HarnessShare>> {
        match &self.target {
            ShareTarget::Harness(share) => Some(share),
            ShareTarget::Acp(_) => None,
        }
    }

    /// One pass for a thread on an external harness: apply device commands,
    /// keep the mirror's turn in step with the harness, publish a snapshot
    /// when it changed, and start a queued prompt the harness is free for.
    async fn tick_harness(&self, share: &Arc<HarnessShare>, manager: &RemoteManager) -> bool {
        // The app closed the thread: its share ends, even if the close raced
        // the pass that started it.
        if share.external.is_closed() {
            return false;
        }
        let mut should_pair = false;
        let mut requests = Vec::new();
        {
            let mut state = self.state.lock().await;
            if let Some(reason) = state.closed.take() {
                state.closed = Some(reason);
                return false;
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
                        if matches!(request.operation, RemoteOperation::CreateSession { .. }) {
                            let manager = manager.clone();
                            tokio::spawn(async move { manager.host_thread(request, reply).await });
                            continue;
                        }
                        requests.push((request, reply))
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
        }

        // Requests are applied without the share's lock: a steer and a queued
        // prompt reach the harness without stalling a status read or pairing.
        for (request, reply) in requests {
            let request_id = request.request_id.clone();
            // A steer is handed to the harness like a prompt: the answer may
            // take a moment, and this loop publishes the thread's turn.
            if matches!(request.operation, RemoteOperation::Steer { .. }) {
                let share = Arc::clone(share);
                let registration = self.registration.clone();
                tokio::spawn(async move {
                    let response = match harness_mutation(&share, &registration, &request).await {
                        Ok(result) => RemoteResponse::new(request.request_id.clone(), result)
                            .with_receipt(ReceiptState::Applied),
                        Err(error) => RemoteResponse::error(request.request_id.clone(), error)
                            .with_receipt(ReceiptState::Rejected),
                    };
                    reply.send(response);
                });
                continue;
            }
            if request.operation.is_mutation() {
                match harness_mutation(share, &self.registration, &request).await {
                    Ok(result) => {
                        reply.send(
                            RemoteResponse::new(request_id, result)
                                .with_receipt(ReceiptState::Applied),
                        );
                    }
                    Err(error) => {
                        reply.send(
                            RemoteResponse::error(request_id, error)
                                .with_receipt(ReceiptState::Rejected),
                        );
                    }
                }
            } else if matches!(
                request.operation,
                crate::remote::protocol::RemoteOperation::GetChanges
            ) {
                match share.mirror.check(&self.registration, &request) {
                    Ok(()) => {
                        let root = share.external.cwd.clone();
                        tokio::spawn(async move {
                            let changes = crate::remote::changes::read(
                                &root,
                                chrono::Utc::now().to_rfc3339(),
                            )
                            .await;
                            reply.send(RemoteResponse::new(
                                request_id,
                                crate::remote::protocol::RemoteResult::Changes(changes),
                            ));
                        });
                    }
                    Err(error) => reply.send(RemoteResponse::error(request_id, error)),
                }
            } else {
                match share
                    .mirror
                    .read(&self.registration, &request, &ProjectionLimits::default())
                {
                    Ok(result) => reply.send(RemoteResponse::new(request_id, result)),
                    Err(error) => reply.send(RemoteResponse::error(request_id, error)),
                }
            }
        }

        let start_next = {
            let mut state = self.state.lock().await;
            if let Some(reason) = state.closed.take() {
                state.closed = Some(reason);
                return false;
            }
            // A turn the app or a device started is picked up here, and one
            // that ended is closed, so the transcript keeps both prompts and
            // the reply the device saw streaming.
            share
                .mirror
                .sync_turn(crate::thread_status::is_active(&share.session_id));
            let room_for_resync = state.link.outbound.capacity() > 1;
            let dirty = share.mirror.dirty();
            if let Some(resync) = state.publisher.harness_snapshot(dirty, room_for_resync) {
                let sequence = state.publisher.sequence();
                let mut snapshot = share.mirror.snapshot(
                    &self.registration,
                    sequence,
                    &ProjectionLimits::default(),
                );
                if manager.can_host_threads() {
                    advertise_host_threads(&mut snapshot);
                }
                let message = OwnerMessage::Snapshot {
                    snapshot: Box::new(snapshot),
                    resync,
                };
                match state.link.outbound.try_send(message) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        state.publisher.mark_lagging();
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
            share.mirror.wants_prompt()
        };

        if should_pair {
            self.pair_if_due().await;
        }
        if start_next {
            share.start_queued();
        }
        true
    }
}

impl HarnessShare {
    /// Show the app a prompt a device sent, as if it had been typed in the
    /// chat: ACP has no other way to tell a client about a message it did not
    /// send.
    fn echo_prompt(&self, text: &str) {
        let Some(clients) = &self.clients else {
            return;
        };
        let _ = clients.update(SessionUpdate::UserMessageChunk(
            crate::message_origin::user_chunk(text.to_owned(), None),
        ));
    }

    /// Run one prompt on the harness, as the app's own prompt does, and then
    /// the next one a device queued while it ran.
    fn start_prompt(self: &Arc<Self>, text: String) {
        if self.mirror.is_closed() {
            // Remote access ended while this prompt waited.
            return;
        }
        self.echo_prompt(&text);
        self.mirror.note_prompt(&text);
        // The turn guard is taken inside `prompt`; until it is, the mirror
        // must not believe the thread went idle.
        self.mirror.note_prompt_applied();
        let share = Arc::clone(self);
        tokio::spawn(async move {
            let blocks = vec![ContentBlock::Text(TextContent::new(text))];
            let result = share.external.prompt(blocks).await;
            share.mirror.note_turn_returned();
            if let Err(error) = result {
                share.mirror.note_error(&format!(
                    "**{} turn failed**\n\n{error}",
                    share.external.harness.name
                ));
            }
            share.start_queued();
        });
    }

    /// Start a prompt a device queued, if the harness is idle.
    fn start_queued(self: &Arc<Self>) {
        if crate::thread_status::is_active(&self.session_id) {
            return;
        }
        let Some(text) = self.mirror.take_queue() else {
            return;
        };
        self.start_prompt(text);
    }
}

/// Apply one mutating request to a thread on an external harness, or say why
/// not. Everything a request names is checked under the lock that applies it.
async fn harness_mutation(
    share: &Arc<HarnessShare>,
    registration: &SessionRegistration,
    request: &RemoteRequest,
) -> Result<RemoteResult, RemoteError> {
    share.mirror.check(registration, request)?;
    match &request.operation {
        RemoteOperation::SubmitPrompt { prompt } => {
            if crate::thread_status::is_active(&share.session_id) {
                return Err(RemoteError::new(
                    RemoteErrorCode::Busy,
                    "a turn is already running on this session",
                ));
            }
            share.start_prompt(prompt.clone());
            Ok(RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started,
            })
        }
        RemoteOperation::Queue { prompt } => {
            if !crate::thread_status::is_active(&share.session_id) {
                return Err(RemoteError::new(
                    RemoteErrorCode::NotRunning,
                    "no turn is running on this session",
                ));
            }
            if !share.mirror.queue(prompt.clone()) {
                return Err(RemoteError::new(
                    RemoteErrorCode::RateLimited,
                    "too many prompts are already queued on this thread",
                ));
            }
            Ok(RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Queued,
            })
        }
        RemoteOperation::Steer { prompt } => {
            if !share.mirror.can_steer() {
                return Err(RemoteError::new(
                    RemoteErrorCode::UnsupportedOperation,
                    "this harness does not accept steering a running turn",
                ));
            }
            let blocks = vec![ContentBlock::Text(TextContent::new(prompt.clone()))];
            if share.external.steer(&blocks).await {
                share.echo_prompt(prompt);
                share.mirror.note_prompt(prompt);
                Ok(RemoteResult::PromptAccepted {
                    disposition: PromptDisposition::Steered,
                })
            } else {
                Err(RemoteError::new(
                    RemoteErrorCode::UnsupportedOperation,
                    "this harness does not accept steering a running turn",
                ))
            }
        }
        RemoteOperation::CancelTurn { turn_id } => {
            if share.mirror.turn_id().as_deref() != Some(turn_id.as_str()) {
                return Err(RemoteError::new(
                    RemoteErrorCode::StaleTurn,
                    "the named turn is no longer running",
                ));
            }
            share.external.cancel();
            Ok(RemoteResult::TurnCancelled {
                turn_id: turn_id.clone(),
            })
        }
        RemoteOperation::ResolveApproval { batch_id, choice } => {
            share.mirror.resolve_approval(batch_id, *choice)?;
            Ok(RemoteResult::ApprovalResolved {
                batch_id: batch_id.clone(),
                choice: *choice,
            })
        }
        RemoteOperation::AnswerQuestion {
            question_id,
            answer,
        } => {
            share.mirror.answer_question(question_id, answer)?;
            Ok(RemoteResult::QuestionAnswered {
                question_id: question_id.clone(),
            })
        }
        RemoteOperation::CancelQuestion { question_id } => {
            share.mirror.cancel_question(question_id)?;
            Ok(RemoteResult::QuestionCancelled {
                question_id: question_id.clone(),
            })
        }
        other => Err(RemoteError::new(
            RemoteErrorCode::UnsupportedOperation,
            format!(
                "a thread on an external harness does not support `{}` from a device",
                other.name()
            ),
        )),
    }
}

/// A thread's sink, with the mirror fed first: every update the harness sends
/// is what a device sees, and a permission request is offered to devices
/// while the app still gets it.
struct TeeSink {
    inner: Arc<dyn ClientSink>,
    session_id: String,
    remote: RemoteManager,
}

impl ClientSink for TeeSink {
    fn update(&self, update: SessionUpdate) {
        if let Some(mirror) = self.remote.mirror(&self.session_id) {
            mirror.observe(&update);
        }
        self.inner.update(update);
    }

    fn request_permission(&self, request: RequestPermissionRequest) -> PermissionFuture {
        bridge_permission(
            self.remote.mirror(&self.session_id),
            self.inner.clone(),
            request,
        )
    }

    fn turn_started(&self, history: usize, prompt: String) {
        self.inner.turn_started(history, prompt);
    }

    fn turn_ended(&self) {
        self.inner.turn_ended();
    }

    fn child_activity(&self, activity: crate::harness::child_activity::ChildActivity) {
        self.inner.child_activity(activity);
    }
}

/// Offer a harness's permission request to paired devices while the app is
/// still asked: whichever answers first ends it.
pub(crate) fn bridge_permission(
    mirror: Option<Arc<HarnessMirror>>,
    inner: Arc<dyn ClientSink>,
    request: RequestPermissionRequest,
) -> PermissionFuture {
    let Some(mirror) = mirror else {
        return inner.request_permission(request);
    };
    // Registered before the future is polled: a device can see the request
    // even if whoever awaits this one is busy for a moment.
    let (key, mut from_device) = mirror.begin_permission(&request);
    Box::pin(async move {
        let mut from_app = inner.request_permission(request);
        tokio::select! {
            answer = &mut from_app => {
                // Only this request leaves the offer; another harness request
                // waiting beside it keeps its own card and its own answer.
                mirror.end_permission(&key);
                answer
            }
            // A device answered first. The app's card stays until the turn
            // ends, when the app drops its pending permissions.
            answer = &mut from_device => answer.unwrap_or_else(|_| {
                RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
            }),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_devices_reach_the_apps_with_kind_platform_and_last_seen() {
        use crate::remote_gateway::devices::{DeviceKind, Devices};
        let summary = DeviceSummary {
            id: "0011223344556677".into(),
            name: "Lars's PC".into(),
            last_seen_at: Some("2026-11-08T09:30:00Z".parse().unwrap()),
            kind: DeviceKind::Desktop,
            platform: "windows".into(),
            host_id: Some("host-0123456789abcdef".into()),
        };
        assert_eq!(
            device_json(&summary, Some(true)),
            json!({
                "id": "0011223344556677",
                "name": "Lars's PC",
                "kind": "desktop",
                "platform": "windows",
                "hostId": "host-0123456789abcdef",
                "lastSeenAt": "2026-11-08T09:30:00Z",
                "connected": true,
            })
        );
        // A record from before devices said what they are reads as a phone.
        let old: Devices = serde_json::from_str(
            r#"{"version":1,"devices":[{"id":"aa","name":"iPhone","token_sha256":"00","paired_at":"2026-10-01T10:00:00Z"}]}"#,
        )
        .unwrap();
        let value = device_json(&DeviceSummary::from(&old.list()[0]), None);
        assert_eq!(value["kind"], "phone");
        assert_eq!(value["platform"], "unknown");
        assert!(value["lastSeenAt"].is_null() && value["connected"].is_null());
        assert!(value.get("token_sha256").is_none());
    }

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

    /// Settings › Remote access asks without naming a thread: the machine's
    /// own switch, the gateway and the paired devices.
    #[test]
    fn a_status_request_may_name_no_thread() {
        let host: super::RemoteStatusRequest =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(host.session_id, "");
        let start: super::RemoteStartRequest =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(start.session_id, "");
        assert!(!start.pair);
        let stop: super::RemoteStopRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(stop.session_id, "");
        let answer: super::RemoteAnswerRequest =
            serde_json::from_value(serde_json::json!({"id": "5be1c4a07d2e9f63", "accept": true}))
                .unwrap();
        assert_eq!(
            (answer.id.as_str(), answer.accept),
            ("5be1c4a07d2e9f63", true)
        );
        let revoke: super::RemoteRevokeRequest =
            serde_json::from_value(serde_json::json!({"deviceId": "0011223344556677"})).unwrap();
        assert_eq!(revoke.device_id, "0011223344556677");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_host_status_names_the_switch_and_the_devices() {
        let value = super::host_status().await;
        assert_eq!(value["available"], true);
        assert!(value["enabled"].is_boolean());
        assert_eq!(value["shared"], false);
        assert!(value["devices"].is_array());
        assert!(value["connectRequests"].is_array());
        assert!(value["endedConnectRequests"].is_array());
        assert!(value["refusedConnections"].is_array());
    }

    /// A prompt a device sends on a thread that runs on an external harness
    /// reaches the harness, and the mirror a device reads grows with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_prompt_from_a_device_runs_on_the_harness_and_shows_in_the_mirror() {
        use crate::harness::acp_child::tests::start_thread;
        // A session id of this test's own: the turn status is process-wide,
        // and tests run beside this one.
        let session_id = "remote-device-prompt";
        let (thread, _seen) = start_thread(
            "fake-acp-prompt",
            session_id,
            Default::default(),
            Some("allow"),
            Default::default(),
        )
        .await;
        let mirror = Arc::new(crate::remote::harness::HarnessMirror::new(
            session_id.to_owned(),
            "fake-acp".to_owned(),
            thread.session.cwd.clone(),
            "sonnet".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            true,
        ));
        // What the tee sink does in the app: every update the harness sends
        // is what a device sees.
        let mut updates = thread.updates;
        let sink_mirror = Arc::clone(&mirror);
        tokio::spawn(async move {
            while let Some(update) = updates.recv().await {
                sink_mirror.observe(&update);
            }
        });
        let share = Arc::new(HarnessShare {
            session_id: session_id.to_owned(),
            external: Arc::clone(&thread.session),
            mirror: Arc::clone(&mirror),
            clients: None,
        });
        let registration = SessionRegistration {
            session_id: session_id.to_owned(),
            registration_epoch: 1,
        };
        let request = RemoteRequest {
            authenticated_device_id: None,
            protocol_version: crate::remote::REMOTE_PROTOCOL_VERSION,
            request_id: "r1".to_owned(),
            session_id: Some(session_id.to_owned()),
            registration_epoch: Some(1),
            operation: RemoteOperation::SubmitPrompt {
                prompt: "write notes".to_owned(),
            },
        };
        let applied = harness_mutation(&share, &registration, &request)
            .await
            .expect("the device's prompt is accepted");
        assert!(matches!(
            applied,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started
            }
        ));
        // What the share loop does every pass: follow the turn, then publish.
        let mut texts = Vec::new();
        for _ in 0..400 {
            mirror.sync_turn(crate::thread_status::is_active(session_id));
            let snapshot = mirror.snapshot(&registration, 1, &ProjectionLimits::default());
            texts = snapshot
                .transcript
                .iter()
                .map(|message| (message.role.clone(), message.content.text.clone()))
                .collect();
            if texts
                .iter()
                .any(|(role, text)| role == "assistant" && text.contains("Wrote it."))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        mirror.sync_turn(false);
        assert!(
            texts
                .iter()
                .any(|(role, text)| role == "user" && text == "write notes"),
            "the device's prompt is in the shared transcript: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|(role, text)| role == "assistant" && text.contains("Wrote it.")),
            "the harness's reply is in the shared transcript: {texts:?}"
        );
    }

    /// A question a device answers while the app still shows its card.
    /// A shared thread on an external harness registers under its own id, and
    /// switching remote access off ends the share.
    /// A share connected to a gateway this test holds: it keeps both ends so
    /// it can read the registration and answer requests itself.
    struct TestGateway {
        slot: Arc<std::sync::Mutex<Option<crate::remote::owner::GatewayLink>>>,
    }

    impl crate::remote::owner::OwnerConnector for TestGateway {
        fn connect(
            &mut self,
            _registration: &SessionRegistration,
        ) -> Result<crate::remote::owner::OwnerLink, crate::remote::owner::OwnerLinkError> {
            let (owner, gateway) = crate::remote::owner::owner_link_pair(16, 16);
            if let Ok(mut slot) = self.slot.lock() {
                *slot = Some(gateway);
            }
            Ok(owner)
        }
    }

    fn registered_thread(session_id: &str) -> RemoteSessionInfo {
        RemoteSessionInfo {
            session_id: session_id.to_owned(),
            registration_epoch: 3,
            title: String::new(),
            workspace: Some("/tmp/project".to_owned()),
            model: "sonnet".to_owned(),
            harness: Some("claude-code".to_owned()),
            turn_count: Some(0),
            last_active_at: None,
            parent: None,
            activity: crate::remote::SessionActivity::Idle,
            attention: crate::remote::RemoteAttention {
                approval: false,
                question: false,
            },
            health: crate::remote::OwnerHealth::Live,
        }
    }

    /// Share `test-thread` with the gateway `gateway` hands back, and start a
    /// thread on request.
    async fn hosted_share(manager: &super::RemoteManager) {
        manager.set_thread_host(|ask| {
            Box::pin(async move {
                assert_eq!(ask.path.as_deref(), Some("/tmp/project"));
                Ok(registered_thread(match ask.harness.as_deref() {
                    Some("claude-code") => "thread-new",
                    _ => "thread-other",
                }))
            })
        });
        let harness = crate::harness::session::test_support::harness(
            "fake-acp",
            crate::harness::HarnessKind::Acp,
        );
        let thread = crate::harness::session::test_support::thread(&harness, None);
        manager
            .start_harness(
                "test-thread".to_owned(),
                Arc::clone(&thread.session),
                None,
                false,
            )
            .await
            .expect("the thread shares itself");
    }

    fn create_request(request_id: &str, harness: &str) -> RemoteRequest {
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: crate::remote::REMOTE_PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            session_id: None,
            registration_epoch: None,
            operation: RemoteOperation::CreateSession {
                path: Some("/tmp/project".to_owned()),
                harness: Some(harness.to_owned()),
                model: None,
            },
        }
    }

    #[tokio::test]
    async fn a_share_that_hosts_threads_advertises_it_and_answers_the_create() {
        use crate::remote::owner::OwnerMessage;

        let slot = Arc::new(std::sync::Mutex::new(None));
        let for_connector = Arc::clone(&slot);
        let manager = super::RemoteManager::with_connector(move || {
            Box::new(TestGateway {
                slot: Arc::clone(&for_connector),
            })
        });
        hosted_share(&manager).await;
        let mut gateway = slot.lock().unwrap().take().expect("a gateway connection");
        let registered = gateway.messages.try_recv().expect("a registration");
        let OwnerMessage::Register { snapshot, .. } = registered else {
            panic!("the first message is the registration");
        };
        assert!(
            snapshot
                .capabilities
                .iter()
                .any(|capability| capability == crate::remote::session_capability::HOST_THREADS),
            "{:?}",
            snapshot.capabilities
        );

        // The gateway forwards a device's `create_session`; the share answers
        // with the row the app's new thread has.
        let (reply, response) = crate::remote::owner::OwnerReply::channel();
        gateway
            .commands
            .send(crate::remote::owner::OwnerCommand::Request {
                request: create_request("create-1", "claude-code"),
                reply,
            })
            .await
            .expect("the share takes the request");
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), response)
            .await
            .expect("the share answers")
            .expect("the reply handle is not dropped");
        assert_eq!(response.receipt, Some(ReceiptState::Applied));
        match response.result {
            RemoteResult::SessionCreated { session } => {
                assert_eq!(session.session_id, "thread-new");
                assert_eq!(session.harness.as_deref(), Some("claude-code"));
            }
            other => panic!("a thread was created: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_share_without_a_thread_host_neither_advertises_nor_answers_it() {
        use crate::remote::owner::{
            GatewayLink, OwnerConnector, OwnerLink, OwnerLinkError, OwnerMessage,
        };

        struct QuietGateway;
        impl OwnerConnector for QuietGateway {
            fn connect(
                &mut self,
                _registration: &SessionRegistration,
            ) -> Result<OwnerLink, OwnerLinkError> {
                let (owner, gateway) = crate::remote::owner::owner_link_pair(16, 16);
                tokio::spawn(async move {
                    let GatewayLink {
                        mut messages,
                        commands,
                    } = gateway;
                    let register = messages.recv().await.expect("a registration");
                    assert!(matches!(&register, OwnerMessage::Register { snapshot, .. }
                            if !snapshot.capabilities.iter().any(|capability| capability
                                == crate::remote::session_capability::HOST_THREADS)));
                    drop(commands);
                });
                Ok(owner)
            }
        }

        let manager = super::RemoteManager::with_connector(|| Box::new(QuietGateway));
        assert!(!manager.can_host_threads());
        let harness = crate::harness::session::test_support::harness(
            "fake-acp",
            crate::harness::HarnessKind::Acp,
        );
        let thread = crate::harness::session::test_support::thread(&harness, None);
        manager
            .start_harness(
                "test-thread".to_owned(),
                Arc::clone(&thread.session),
                None,
                false,
            )
            .await
            .expect("the thread shares itself");
        manager
            .unshare_all(crate::remote::SessionCloseReason::SharingDisabled)
            .await;
    }

    #[tokio::test]
    async fn a_harness_share_registers_and_unshares() {
        use crate::remote::owner::{GatewayLink, OwnerConnector, OwnerLink, OwnerLinkError};

        struct MemoryGateway;
        impl OwnerConnector for MemoryGateway {
            fn connect(
                &mut self,
                _registration: &SessionRegistration,
            ) -> Result<OwnerLink, OwnerLinkError> {
                let (owner, gateway) = crate::remote::owner::owner_link_pair(16, 16);
                // A gateway that answers every command: what a share needs to
                // stay registered.
                tokio::spawn(async move {
                    let GatewayLink {
                        mut messages,
                        commands,
                    } = gateway;
                    while let Some(message) = messages.recv().await {
                        if matches!(
                            message,
                            crate::remote::owner::OwnerMessage::Unregister { .. }
                        ) {
                            break;
                        }
                    }
                    drop(commands);
                });
                Ok(owner)
            }
        }

        let manager = super::RemoteManager::with_connector(|| Box::new(MemoryGateway));
        let harness = crate::harness::session::test_support::harness(
            "fake-acp",
            crate::harness::HarnessKind::Acp,
        );
        let thread = crate::harness::session::test_support::thread(&harness, None);
        manager
            .start_harness(
                "test-thread".to_owned(),
                Arc::clone(&thread.session),
                None,
                false,
            )
            .await
            .expect("the thread shares itself");
        assert!(manager.is_shared("test-thread"));
        assert!(manager.mirror("test-thread").is_some());

        manager
            .unshare_all(crate::remote::SessionCloseReason::SharingDisabled)
            .await;
        assert!(!manager.is_shared("test-thread"), "remote access is off");
    }

    /// Two harness requests wait at once: answering one leaves the other on
    /// screen, and each `bridge_permission` future gets its own answer.
    #[cfg(unix)]
    #[tokio::test]
    async fn two_bridged_permissions_are_answered_independently() {
        use crate::harness::session::{ClientSink, PermissionFuture};
        use agent_client_protocol::schema::v1::{
            PermissionOption, PermissionOptionKind, ToolCallUpdate, ToolCallUpdateFields,
        };

        struct NeverAnswers;
        impl ClientSink for NeverAnswers {
            fn update(&self, _update: SessionUpdate) {}
            fn request_permission(&self, _request: RequestPermissionRequest) -> PermissionFuture {
                Box::pin(std::future::pending())
            }
        }

        let mirror = Arc::new(crate::remote::harness::HarnessMirror::new(
            "session-1".to_owned(),
            "claude-code".to_owned(),
            std::env::temp_dir(),
            "opus".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            true,
        ));
        let ask = |id: &'static str| {
            RequestPermissionRequest::new(
                "session-1",
                ToolCallUpdate::new(id, ToolCallUpdateFields::new().title("Edit a file")),
                vec![
                    PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                    PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
                ],
            )
        };
        let mut first = Box::pin(super::bridge_permission(
            Some(Arc::clone(&mirror)),
            Arc::new(NeverAnswers),
            ask("call-1"),
        ));
        let mut second = Box::pin(super::bridge_permission(
            Some(Arc::clone(&mirror)),
            Arc::new(NeverAnswers),
            ask("call-2"),
        ));
        assert_eq!(mirror.pending_approval_ids().len(), 2);

        // A device denies the second; the first keeps waiting.
        mirror
            .resolve_approval(
                "harness-approval-call-2",
                crate::controller::ApprovalChoice::Deny,
            )
            .expect("the second batch resolves");
        let answered = tokio::time::timeout(std::time::Duration::from_secs(1), &mut second)
            .await
            .expect("the second request answers");
        assert!(matches!(
            answered.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "deny"
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut first)
                .await
                .is_err(),
            "the first request is still waiting, not cancelled"
        );

        mirror
            .resolve_approval(
                "harness-approval-call-1",
                crate::controller::ApprovalChoice::Approve,
            )
            .expect("the first batch resolves");
        let answered = tokio::time::timeout(std::time::Duration::from_secs(1), &mut first)
            .await
            .expect("the first request answers");
        assert!(matches!(
            answered.outcome,
            RequestPermissionOutcome::Selected(selected) if selected.option_id.0.as_ref() == "allow"
        ));
    }

    /// A share the gateway turned down says why, and auto-share leaves that
    /// thread alone for a while instead of retrying it every pass.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_refused_share_reports_its_reason() {
        use crate::remote::owner::{GatewayLink, OwnerConnector, OwnerLink, OwnerLinkError};

        struct RefusingGateway;
        impl OwnerConnector for RefusingGateway {
            fn connect(
                &mut self,
                _registration: &SessionRegistration,
            ) -> Result<OwnerLink, OwnerLinkError> {
                let (owner, gateway) = crate::remote::owner::owner_link_pair(8, 8);
                tokio::spawn(async move {
                    let GatewayLink {
                        mut messages,
                        commands,
                    } = gateway;
                    // The registration arrives, then the hub turns it down:
                    // the host already shares as many threads as it takes.
                    let _ = messages.recv().await;
                    let _ = commands
                        .send(OwnerCommand::Closed {
                            reason: "the host already shares 32 sessions".to_owned(),
                        })
                        .await;
                });
                Ok(owner)
            }
        }

        let manager = super::RemoteManager::with_connector(|| Box::new(RefusingGateway));
        let harness = crate::harness::session::test_support::harness(
            "fake-acp",
            crate::harness::HarnessKind::Acp,
        );
        let thread = crate::harness::session::test_support::thread(&harness, None);
        manager
            .start_harness(
                "test-thread".to_owned(),
                Arc::clone(&thread.session),
                None,
                false,
            )
            .await
            .expect("the share registers");
        for _ in 0..200 {
            if !manager.is_shared("test-thread") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!manager.is_shared("test-thread"), "the share ended");
        let status = manager.status("test-thread").await;
        assert_eq!(status["shared"], false);
        assert_eq!(status["notShared"], "the host already shares 32 sessions");
        assert_eq!(manager.refused_count(), 1);
    }

    /// A device answers an approval while the app still shows its card: the
    /// harness hears the device's answer and the turn finishes.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_device_answer_resolves_the_harness_permission_request() {
        use crate::harness::acp_child::tests::{Fake, start_shared};
        use crate::harness::session::{ClientSink, PermissionFuture};

        struct Tee {
            inner: Arc<dyn ClientSink>,
            mirror: Arc<crate::remote::harness::HarnessMirror>,
        }

        impl ClientSink for Tee {
            fn update(&self, update: SessionUpdate) {
                self.mirror.observe(&update);
                self.inner.update(update);
            }

            fn request_permission(&self, request: RequestPermissionRequest) -> PermissionFuture {
                super::bridge_permission(
                    Some(Arc::clone(&self.mirror)),
                    Arc::clone(&self.inner),
                    request,
                )
            }
        }

        let session_id = "remote-device-answer";
        let mirror = Arc::new(crate::remote::harness::HarnessMirror::new(
            session_id.to_owned(),
            "fake-acp-answer".to_owned(),
            std::env::temp_dir(),
            "sonnet".to_owned(),
            "A thread".to_owned(),
            Vec::new(),
            false,
        ));
        let tee_mirror = Arc::clone(&mirror);
        // The app never answers: only the device does.
        let thread = start_shared(
            "fake-acp-answer",
            session_id,
            Fake::echo(false),
            move |inner| {
                Arc::new(Tee {
                    inner,
                    mirror: tee_mirror,
                }) as Arc<dyn ClientSink>
            },
        )
        .await;

        let session = Arc::clone(&thread.session);
        let prompting = tokio::spawn(async move {
            session
                .prompt(vec![ContentBlock::Text(TextContent::new("write notes"))])
                .await
        });
        let registration = SessionRegistration {
            session_id: session_id.to_owned(),
            registration_epoch: 1,
        };
        let mut batch = None;
        for _ in 0..200 {
            if let Some(approval) = mirror
                .snapshot(&registration, 1, &ProjectionLimits::default())
                .pending_approval
            {
                batch = Some(approval);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let batch = batch.expect("the device is offered the approval");
        mirror
            .resolve_approval(&batch.batch_id, crate::controller::ApprovalChoice::Approve)
            .expect("the device's answer is applied");
        let stop = tokio::time::timeout(std::time::Duration::from_secs(5), prompting)
            .await
            .expect("the turn ends")
            .expect("the prompt task runs");
        assert_eq!(
            stop,
            Ok(agent_client_protocol::schema::v1::StopReason::EndTurn),
            "the harness took the device's answer as permission"
        );
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
            instance_id: Some("instance".to_owned()),
            credential: Secret::new("credential"),
            code: Secret::new("1234-5678"),
            expires_in_secs: 300,
            challenge: Some(7),
            locked_for_secs: None,
            alternates: vec!["100.64.0.4:8787".to_owned()],
            qr_payload: Secret::new("payload"),
        });
        assert_eq!(offer.code, "1234-5678");
        assert_eq!(offer.qr_payload, "payload");
        assert_eq!(offer.address, "192.168.1.4:8787");
        assert_eq!(offer.alternates, ["100.64.0.4:8787"]);
        assert!(offer.expires_at > Instant::now());
    }
}
