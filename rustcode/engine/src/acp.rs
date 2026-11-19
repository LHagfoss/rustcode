mod agents;
mod attachments;
mod capabilities;
pub(crate) mod clients;
mod commands;
mod config;
mod connections;
mod cursor;
pub(crate) mod data;
mod edit_diff;
mod harnesses;
mod hosted;
pub(crate) mod loops;
mod machine_sync;
mod mcp_app;
mod mirror;
pub(crate) mod outside;
mod permissions;
mod presence;
mod preview;
mod project_relocate;
mod projects;
mod prompt;
pub(crate) mod questions;
mod remote;
mod rewind;
mod scheduled;
mod server_status;
mod session;
mod session_load;
pub(crate) mod session_move;
mod session_repair;
pub(crate) mod settings;
pub(crate) mod steer;
mod streaming;
mod telemetry;
mod themes;
pub(crate) mod usage;
mod workflows;
pub(crate) use session_load::*;
mod extensions;
pub(crate) use extensions::*;

pub(crate) use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock,
    InitializeRequest, InitializeResponse, ListSessionsRequest, ListSessionsResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    PromptCapabilities, PromptRequest, PromptResponse, SessionCapabilities,
    SessionCloseCapabilities, SessionInfo, SessionInfoUpdate, SessionListCapabilities,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse,
};
#[cfg(test)]
use agent_client_protocol::schema::v1::{
    SessionConfigId, SessionConfigKind, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelectOptions, StopReason,
};
use agent_client_protocol::{Agent, Stdio};
use rustcode_tasks::TaskEvent;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

pub(crate) use config::{build_session_config_options, handle_set_config_option};
#[cfg(test)]
pub(crate) use permissions::{ApprovalRequirement, approval_requirement};
pub(crate) use prompt::prompt_input;
pub(crate) use session::{AcpSession, KnownTaskIds, SessionTurnState, Sessions, new_registry};
#[cfg(test)]
pub(crate) use streaming::{AcpEventStream, acp_stop_reason};

const ACP_SESSION_PAGE_SIZE: usize = 50;

fn valid_acp_session_id(session_id: &str) -> bool {
    rustcode_session::valid_session_id(session_id)
}

/// Highest level of the `_meta.rustcode.visualizations` capability the
/// engine knows (#2088, #2217). A client's initialize
/// `clientCapabilities._meta.rustcode.visualizations` says which
/// `rustcode-viz` widgets it renders: 1 is bar chart, table and tree; 2 adds
/// line chart, stat list and checklist. Its sessions are taught those types.
const VISUALIZATIONS_CAPABILITY_VERSION: u64 = 2;

/// The `rustcode-viz` level an ACP client advertised, capped at the highest
/// the engine knows. Absent, malformed, or zero means 0: the client would
/// only see JSON.
fn client_visualization_level(
    capabilities: &agent_client_protocol::schema::v1::ClientCapabilities,
) -> u8 {
    capabilities
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("visualizations"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |level| {
            level.min(VISUALIZATIONS_CAPABILITY_VERSION) as u8
        })
}

/// Version of the `_meta.rustcode.smartUi` capability (#2484). A client
/// advertises it while the user has Smart UI turned on, and its sessions are
/// then taught `rustcode-ui v1` blocks. `0` means the client can run them but
/// the setting is off: the agent then points the user at the setting instead
/// of inventing a format.
const SMART_UI_CAPABILITY_VERSION: u64 = 1;

fn client_smart_ui(
    capabilities: &agent_client_protocol::schema::v1::ClientCapabilities,
) -> crate::app::SmartUi {
    let version = capabilities
        .meta
        .as_ref()
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("smartUi"))
        .and_then(serde_json::Value::as_u64);
    match version {
        Some(version) if version >= SMART_UI_CAPABILITY_VERSION => crate::app::SmartUi::On,
        Some(0) => crate::app::SmartUi::Off,
        _ => crate::app::SmartUi::Unsupported,
    }
}

fn server_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(PromptCapabilities::new().image(true))
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .close(SessionCloseCapabilities::new()),
        )
        .meta(serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            serde_json::json!({
                "visualizations": VISUALIZATIONS_CAPABILITY_VERSION,
                "smartUi": SMART_UI_CAPABILITY_VERSION,
                "preview": crate::preview::PREVIEW_CAPABILITY_VERSION,
                "remote": remote::REMOTE_CAPABILITY_VERSION,
                "steer": steer::STEER_CAPABILITY_VERSION,
                "agents": agents::AGENTS_CAPABILITY_VERSION,
                "rewind": 1,
                "sessionMove": 1,
                "sessionRepair": 1,
                "sessionActivity": 1,
                "childActivity": 1,
                "pullRequests": 3,
                "worktrees": 1,
                "checkpoints": 2,
                "threadStatus": 1,
                // `_meta.rustcode.relationship` on `session/new`, and the same in
                // `session/list`: a client that opens forks and side chats checks it.
                "lineage": 1,
            }),
        )]))
}

/// `_rustcode/rewind` for a checkpoint restore that also rewinds the chat.
pub(crate) fn rewind_chat(
    state: &mut crate::app::AppState,
    user_index: usize,
    text: &str,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    rewind::rewind(state, user_index, text)
}

fn session_id_is_active(sessions: &HashMap<String, AcpSession>, session_id: &str) -> bool {
    sessions.contains_key(session_id)
}

fn workspace_cwd(
    workspace: Option<rustcode_session::SessionWorkspace>,
) -> Option<std::path::PathBuf> {
    // `canonicalize` returns the verbatim form on Windows, and a thread whose
    // folder is stored that way is a second project with the same name (#3425).
    let cwd = rustcode_session::strip_verbatim(&workspace?.cwd);
    let canonical = std::fs::canonicalize(cwd).ok()?;
    canonical
        .is_dir()
        .then(|| rustcode_session::strip_verbatim(&canonical))
}

/// The user's home folder, without the verbatim prefix a canonical Windows path
/// carries; `None` when this computer has none.
fn home_folder() -> Option<std::path::PathBuf> {
    crate::service::home_dir()
        .ok()
        .map(|home| rustcode_session::strip_verbatim(&home))
}

/// The folder a thread starts in when a request names none: the user's home
/// folder. A start with no folder comes from an orchestrator that has no
/// project on this computer, and the engine's own working directory is `/`
/// under a service manager or the app's install folder on Windows. Neither is a
/// project, and a thread that ran there said "Working directory: /" (#3425).
fn start_folder(
    requested: &std::path::Path,
    home: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if requested.as_os_str().is_empty() {
        return home.map(std::path::Path::to_path_buf);
    }
    Some(requested.to_path_buf())
}

/// The folder a listing shows for a thread: what its workspace recorded, else
/// the user's home folder, never the engine's own working directory. The bool
/// says the folder is a stand-in, not the thread's project.
fn listed_folder(
    recorded: Option<std::path::PathBuf>,
    home: Option<&std::path::Path>,
) -> (std::path::PathBuf, bool) {
    match recorded {
        Some(cwd) => (cwd, false),
        None => (
            home.map(std::path::Path::to_path_buf).unwrap_or_default(),
            true,
        ),
    }
}

const ACP_TERMINAL_LEDGER_CAPACITY: usize = 1024;
const ACP_TERMINAL_BACKLOG_CAPACITY: usize = 256;
static ACTIVE_ACP_SESSIONS: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::OnceLock::new();

pub(crate) fn is_acp_session(session_id: &str) -> bool {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .contains(session_id)
}

fn mark_acp_session(session_id: &str) {
    ACTIVE_ACP_SESSIONS
        .get_or_init(|| std::sync::Mutex::new(HashSet::new()))
        .lock()
        .expect("ACP session registry mutex poisoned")
        .insert(session_id.to_owned());
    // Other processes, the remote gateway first, see the session as open in
    // the app for as long as it is loaded here.
    if let Some(config_dir) = crate::config::get_config_dir() {
        crate::config::hold_session_open(
            &config_dir,
            session_id,
            crate::config::SessionHolder::App,
        );
    }
}

fn unmark_acp_session(session_id: &str) {
    crate::config::release_session_open(session_id, crate::config::SessionHolder::App);
    if let Some(sessions) = ACTIVE_ACP_SESSIONS.get() {
        sessions
            .lock()
            .expect("ACP session registry mutex poisoned")
            .remove(session_id);
    }
}

#[derive(Default)]
struct TerminalLedger {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl TerminalLedger {
    fn claim(&mut self, id: &str) -> bool {
        if !self.ids.insert(id.to_owned()) {
            return false;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > ACP_TERMINAL_LEDGER_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.ids.remove(&expired);
            }
        }
        true
    }
}

struct SessionTaskSink {
    state: Arc<Mutex<crate::app::AppState>>,
    /// Where the session's updates go: the clients that have it.
    clients: clients::SessionClients,
    runtime: tokio::runtime::Handle,
    terminal_ledger: std::sync::Mutex<TerminalLedger>,
    terminal_backlog: Arc<std::sync::Mutex<VecDeque<TaskEvent>>>,
    terminal_overflow: Arc<AtomicBool>,
}

impl SessionTaskSink {
    fn handle_terminal(
        &self,
        event: TaskEvent,
        prompt_sender: std::sync::mpsc::SyncSender<TaskEvent>,
    ) {
        let id = event.task_id().to_string();
        let should_resume_model = event.notify_on_complete().unwrap_or(true);
        if !self
            .terminal_ledger
            .lock()
            .expect("ACP terminal ledger mutex poisoned")
            .claim(&id)
        {
            return;
        }
        let Some((task_id, event_session_id, output)) =
            crate::tools::task_event_to_tool_output(event.clone())
        else {
            return;
        };
        let call_id = event
            .call_id()
            .map(str::to_owned)
            .unwrap_or_else(|| task_id.clone());
        let state = Arc::clone(&self.state);
        let clients = self.clients.clone();
        let terminal_backlog = Arc::clone(&self.terminal_backlog);
        let terminal_overflow = Arc::clone(&self.terminal_overflow);
        self.runtime.spawn(async move {
            {
                // A `manage_task` `wait` result already records this
                // completion; a second copy would reach the provider twice.
                if !crate::tools::background_result_answered_by_wait(&event_session_id, &task_id)
                    .await
                {
                    // The record answers the card the client already shows.
                    // That call was answered when the task went to the
                    // background, so the provider request carries it as text.
                    let mut state = state.lock().await;
                    state.history.push(crate::background_task_history_message(
                        &task_id,
                        Some(&call_id),
                        output.clone(),
                    ));
                    crate::config::save_session_history(&event_session_id, &state.history);
                }
            }
            let update = SessionUpdate::ToolCallUpdate(
                agent_client_protocol::schema::v1::ToolCallUpdate::new(
                    call_id,
                    agent_client_protocol::schema::v1::ToolCallUpdateFields::new()
                        .status(if output.success {
                            agent_client_protocol::schema::v1::ToolCallStatus::Completed
                        } else {
                            agent_client_protocol::schema::v1::ToolCallStatus::Failed
                        })
                        .raw_output(serde_json::json!({
                            "content": output.content,
                            "exitCode": output.exit_code,
                            "changedPaths": [],
                            "truncated": output.truncated,
                            "commandStatus": output.command_status,
                        })),
                ),
            );
            let _ = clients.update(update);
            // Do not expose the fallback event until its history and ACP
            // notification have been handled. This keeps a prompt that wins
            // the receive race from resuming before durable completion.
            if should_resume_model {
                {
                    let mut backlog = terminal_backlog
                        .lock()
                        .expect("ACP terminal backlog mutex poisoned");
                    backlog.push_back(event.clone());
                    while backlog.len() > ACP_TERMINAL_BACKLOG_CAPACITY {
                        backlog.pop_front();
                        terminal_overflow.store(true, Ordering::Release);
                    }
                }
                let _ = prompt_sender.try_send(event);
            }
        });
    }
}

#[derive(Clone)]
struct TaskRoute {
    sender: std::sync::mpsc::SyncSender<TaskEvent>,
    known_task_ids: Arc<std::sync::Mutex<KnownTaskIds>>,
    sink: Option<Arc<SessionTaskSink>>,
}

type TaskRoutes = Arc<std::sync::Mutex<HashMap<String, TaskRoute>>>;

fn route_task_event(routes: &TaskRoutes, event: TaskEvent) {
    let session_id = event.session_id().to_string();
    let mut routes = routes.lock().expect("ACP task routes mutex poisoned");
    let Some(route) = routes.get(&session_id) else {
        return;
    };
    route
        .known_task_ids
        .lock()
        .expect("ACP known task IDs mutex poisoned")
        .insert(event.task_id().to_string());
    if event.is_terminal()
        && let Some(sink) = &route.sink
    {
        sink.handle_terminal(event, route.sender.clone());
        return;
    }
    match route.sender.try_send(event) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
            routes.remove(&session_id);
        }
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            // Terminal events are already persisted and retained in the
            // bounded sink backlog. Keep the route alive so the prompt can
            // consume the coalesced completion once its queue drains.
        }
    }
}

#[derive(Clone)]
struct AcpServer {
    sessions: Sessions,
    client: Arc<reqwest::Client>,
    auto_approve: bool,
    task_routes: TaskRoutes,
    /// The connected clients: each one's capabilities and the sessions it
    /// created or loaded, which is where those sessions' updates go.
    clients: clients::Clients,
    /// Serializes capability registry changes with the process-wide features
    /// derived from them (presence and remote thread hosting).
    client_capability_updates: Arc<std::sync::Mutex<()>>,
    /// Discord Rich Presence, while a connected client asks for it (#2179).
    presence: presence::Presence,
    /// `/remote` shares and their pairing offers.
    remote: remote::RemoteManager,
    /// Not before this: the next attempt to share a thread by itself, after a
    /// gateway that could not be reached. Without it a gateway that is down
    /// would be retried, and logged, on every pass.
    share_retry_at: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    /// Threads running on an external harness (`docs/harnesses.md`).
    external: harnesses::ExternalSessions,
    /// The auto-share loop runs, once for the engine.
    auto_sharing: Arc<AtomicBool>,
}

/// Close a built-in thread: its turn, remote share, tasks and background
/// commands stop. Its working directory, when it was open.
async fn close_builtin(server: &AcpServer, session_id: &str) -> Option<std::path::PathBuf> {
    server
        .remote
        .stop_session(
            session_id,
            crate::remote::SessionCloseReason::SessionChanged,
        )
        .await;
    server
        .task_routes
        .lock()
        .expect("ACP task routes mutex poisoned")
        .remove(session_id);
    let session = server.sessions.lock().await.remove(session_id);
    unmark_acp_session(session_id);
    server.clients.forget_session(session_id);
    let cwd = match session {
        Some(session) => {
            session.turns.cancel_active().await;
            Some(session.cwd.clone())
        }
        None => None,
    };
    crate::tools::abort_background_starts(session_id);
    crate::tools::stop_background_tasks(session_id);
    crate::orchestration::secret::forget_thread_env(session_id);
    cwd
}

/// A built-in `session/new` honours `_meta.rustcode.model` and `effort` the
/// same way external harnesses do (docs/harnesses.md). Model first, since
/// the effort choices depend on it; an unknown value is ignored and the
/// session keeps its default.
fn apply_requested_model_and_effort(
    state: &mut crate::app::AppState,
    asked: &crate::harness::HarnessRequest,
) {
    for (id, value) in [("model", &asked.model), ("effort", &asked.effort)] {
        if let Some(value) = value {
            let _ = handle_set_config_option(
                state,
                &agent_client_protocol::schema::v1::SessionConfigId::new(id),
                &agent_client_protocol::schema::v1::SessionConfigOptionValue::value_id(
                    value.clone(),
                ),
            );
        }
    }
}

fn unknown_session(session_id: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params()
        .data(format!("unknown ACP session: {session_id}"))
}

/// One open thread auto-share may share, with how recently it was used.
struct Candidate {
    last_active: String,
    target: CandidateTarget,
}

enum CandidateTarget {
    Builtin(Arc<remote::RemoteSession>),
    Harness(String, Arc<crate::harness::session::ExternalSession>),
}

impl Candidate {
    fn session_id(&self) -> String {
        match &self.target {
            CandidateTarget::Builtin(session) => session.session_id.clone(),
            CandidateTarget::Harness(session_id, _) => session_id.clone(),
        }
    }
}

impl AcpServer {
    /// Share every open thread while remote access is switched on for this
    /// machine, and unshare them when it is switched off. A thread on the
    /// built-in harness shares from its `AppState`; one on an external
    /// harness shares the mirror of its updates. Both are the same threads
    /// the app has open, so a device sees exactly those.
    async fn auto_share(&self) {
        if !remote::enabled() {
            if self.remote.shared_count() > 0 {
                self.remote
                    .unshare_all(crate::remote::SessionCloseReason::SharingDisabled)
                    .await;
            }
            return;
        }
        if self
            .share_retry_at
            .lock()
            .ok()
            .and_then(|at| *at)
            .is_some_and(|at| std::time::Instant::now() < at)
        {
            return;
        }
        // The gateway shares at most this many threads per host, so a Mac
        // with more threads open shares the ones used most recently.
        let room = remote::MAX_SHARED_SESSIONS.saturating_sub(self.remote.shared_count());
        let mut candidates: Vec<Candidate> = Vec::new();
        {
            let sessions = self.sessions.lock().await;
            for (session_id, session) in sessions.iter() {
                if self.remote.is_shared(session_id) || self.remote.refusal(session_id).is_some() {
                    continue;
                }
                // Read without waiting: a busy thread's clock is its last
                // message, and a lock held elsewhere only costs it its place
                // in the order.
                let last_active = session
                    .state
                    .try_lock()
                    .ok()
                    .and_then(|state| {
                        state
                            .history
                            .iter()
                            .rev()
                            .map(|message| message.timestamp.clone())
                            .find(|timestamp| !timestamp.is_empty())
                    })
                    .unwrap_or_default();
                candidates.push(Candidate {
                    last_active,
                    target: CandidateTarget::Builtin(remote::RemoteSession::from_acp(
                        session_id.clone(),
                        session,
                        self.clients.session(session_id),
                        Arc::clone(&self.client),
                        self.auto_approve,
                        self.presence.clone(),
                        self.remote.clone(),
                    )),
                });
            }
        }
        {
            let sessions = self.external.lock().await;
            for (session_id, session) in sessions.iter() {
                if self.remote.is_shared(session_id)
                    || self.remote.refusal(session_id).is_some()
                    || session.is_closed()
                {
                    continue;
                }
                candidates.push(Candidate {
                    last_active: session.record().updated_at,
                    target: CandidateTarget::Harness(session_id.clone(), Arc::clone(session)),
                });
            }
        }
        // Most recently active first; a thread with no recorded activity goes
        // last. ISO 8601 strings sort by time as they are.
        candidates.sort_by(|left, right| right.last_active.cmp(&left.last_active));
        // Past the host's bound the least recently active threads stay
        // private, and their status says why.
        for extra in candidates.iter().skip(room) {
            self.remote.refuse(
                &extra.session_id(),
                "this host already shares 32 threads; close one to share this one".to_owned(),
            );
        }
        let mut failed = false;
        for candidate in candidates.into_iter().take(room) {
            let session_id = candidate.session_id();
            let started = match candidate.target {
                CandidateTarget::Builtin(session) => self.remote.start(session, false).await,
                CandidateTarget::Harness(id, session) => {
                    let clients = self.clients.session(&id);
                    self.remote
                        .start_harness(id, session, Some(clients), false)
                        .await
                }
            };
            match started {
                Ok(_) => {
                    // A thread closed while this pass connected has nothing
                    // to share: the close removed its session, so the share
                    // that just registered would outlive it.
                    if !self.thread_is_open(&session_id).await {
                        self.remote
                            .stop_session(
                                &session_id,
                                crate::remote::SessionCloseReason::SessionChanged,
                            )
                            .await;
                    }
                }
                // The gateway itself could not be reached: back off for every
                // thread, not just this one.
                Err(remote::StartError::Link(message)) => {
                    crate::dbg_log!("could not share this thread with a device: {message}");
                    failed = true;
                }
                // This thread alone cannot be shared (it closed, or another
                // process holds it): leave it, and try the others.
                Err(remote::StartError::Refused(message)) => {
                    crate::dbg_log!("thread {session_id} is not shared with a device: {message}");
                    self.remote.refuse(&session_id, message);
                }
            }
        }
        if let Ok(mut retry_at) = self.share_retry_at.lock() {
            *retry_at =
                failed.then(|| std::time::Instant::now() + std::time::Duration::from_secs(30));
        }
    }

    /// Whether `session_id` is still open in this app, under the registry's
    /// own lock: what a share started a moment ago must re-check.
    async fn thread_is_open(&self, session_id: &str) -> bool {
        if self.sessions.lock().await.contains_key(session_id) {
            return true;
        }
        self.external
            .lock()
            .await
            .get(session_id)
            .is_some_and(|session| !session.is_closed())
    }

    /// An open session's state and working directory.
    async fn session_handle(
        &self,
        session_id: &str,
    ) -> Option<(Arc<Mutex<crate::app::AppState>>, std::path::PathBuf)> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|session| (Arc::clone(&session.state), session.cwd.clone()))
    }

    fn new(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        let task_routes = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let task_subscription = crate::tools::background_task_manager().subscribe();
        let router_routes = Arc::clone(&task_routes);
        std::thread::Builder::new()
            .name("rustcode-acp-task-router".to_owned())
            .spawn(move || {
                while let Ok(event) = task_subscription.recv() {
                    route_task_event(&router_routes, event);
                }
            })
            .map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })?;
        let clients = clients::Clients::default();
        // A call that waits for the owner is shown by an app that listed
        // outside agents; with none connected, nobody can answer it.
        let approvers = clients.clone();
        crate::orchestration::outside::set_approvers(move || {
            approvers.has_topic(clients::Topic::Outside)
        });
        Ok(Self {
            sessions: new_registry(),
            client: Arc::new(client),
            auto_approve,
            task_routes,
            clients,
            client_capability_updates: Arc::new(std::sync::Mutex::new(())),
            presence: presence::Presence::default(),
            remote: remote::RemoteManager::default(),
            share_retry_at: Arc::new(std::sync::Mutex::new(None)),
            external: harnesses::ExternalSessions::default(),
            auto_sharing: Arc::new(AtomicBool::new(false)),
        })
    }

    /// `initialize` from the connection `peer`: keep what it advertised, and
    /// start what the engine runs once for all its clients.
    fn client_initialized(&self, peer: &clients::Peer, info: clients::ClientInfo) {
        {
            let _updates = self
                .client_capability_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.clients.connect(peer.id(), peer.connection(), info);
            // A thread's status is shown in every client's thread list.
            let clients = self.clients.clone();
            crate::thread_status::set_notifier(move |session_id, update| {
                clients.broadcast(SessionNotification::new(session_id, update));
            });
            crate::thread_status::start_watching();
            self.refresh_presence();
            self.refresh_thread_host();
        }
        // While remote access is on for this machine, every thread the
        // engine has open shares itself, the way a terminal does
        // (`docs/remote.md`). The flag is read again on every pass:
        // `/remote` in a terminal and Settings › Remote access here are one
        // switch.
        if !self.auto_sharing.swap(true, Ordering::AcqRel) {
            let autosharer = self.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    interval.tick().await;
                    autosharer.auto_share().await;
                }
            });
        }
    }

    /// Tell every client that lists threads about a thread just made: the
    /// machine's server owns its threads, so the app on it and every other
    /// connected app list the new row at once, whichever client asked for
    /// `session/new` (docs/desktop.md › Several machines).
    fn announce_thread_created(&self, mut notification: ThreadCreatedNotification) {
        if let Some(parent) = crate::config::load_session_parent(&notification.session_id) {
            notification.meta = Some(serde_json::json!({"rustcode": {"parent": {
                "sessionId": parent.id, "title": parent.title,
                "relationship": parent.relationship.unwrap_or(rustcode_session::SessionRelationship::Delegate).as_str()
            }}}));
        }
        self.clients
            .broadcast_to(clients::Capability::Threads, notification);
    }

    /// Tell every client that lists threads about a thread just trashed.
    fn announce_thread_deleted(&self, notification: ThreadDeletedNotification) {
        self.clients
            .broadcast_to(clients::Capability::Threads, notification);
    }

    /// Advertise thread creation only while a connected client can list the
    /// thread the engine starts for a paired device.
    fn refresh_thread_host(&self) {
        if !self.clients.any(|info| info.shows_threads) {
            self.remote.clear_thread_host();
            return;
        }
        let hosted_server = self.clone();
        self.remote.set_thread_host(move |ask| {
            let server = hosted_server.clone();
            Box::pin(async move { hosted::start(&server, ask).await })
        });
    }

    /// Presence follows whether any connected client asks for it.
    fn refresh_presence(&self) {
        self.presence
            .set_client(self.clients.any(|info| info.presence));
    }

    /// The connection `id` ended. What it started keeps running: sessions,
    /// turns and harness processes belong to the engine.
    fn client_closed(&self, id: clients::ClientId) {
        let _updates = self
            .client_capability_updates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.clients.disconnect(id);
        self.refresh_presence();
        self.refresh_thread_host();
    }

    /// The engine stops: every share, harness, turn and background task ends.
    async fn shutdown(&self) {
        {
            let _updates = self
                .client_capability_updates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.remote.clear_thread_host();
        }
        self.remote.shutdown().await;
        harnesses::close_all(&self.external).await;
        let route_ids = self
            .task_routes
            .lock()
            .expect("ACP task routes mutex poisoned")
            .drain()
            .map(|(session_id, _)| session_id)
            .collect::<Vec<_>>();
        let sessions = self
            .sessions
            .lock()
            .await
            .drain()
            .map(|(session_id, session)| (session_id, session.turns))
            .collect::<Vec<_>>();
        for (session_id, turns) in sessions {
            unmark_acp_session(&session_id);
            turns.cancel_active().await;
        }
        for session_id in route_ids {
            unmark_acp_session(&session_id);
            crate::tools::abort_background_starts(&session_id);
            crate::tools::stop_background_tasks(&session_id);
        }
    }
}

async fn unregister_acp_session(server: &AcpServer, session_id: &str) {
    server
        .remote
        .stop_session(
            session_id,
            crate::remote::SessionCloseReason::SessionChanged,
        )
        .await;
    server
        .task_routes
        .lock()
        .expect("ACP task routes mutex poisoned")
        .remove(session_id);
    let session = server.sessions.lock().await.remove(session_id);
    unmark_acp_session(session_id);
    server.clients.forget_session(session_id);
    if let Some(session) = session {
        session.turns.cancel_active().await;
    }
}

/// Whether the client lists a thread the engine starts for a paired device:
/// `_meta.rustcode.threads` in `initialize`. The desktop app does; a terminal's
/// client does not, so its shares never advertise `host_threads`.
pub(crate) fn client_shows_threads(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("threads"))
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|version| version >= 1)
}

pub async fn run_acp(auto_approve: bool) -> Result<(), Box<dyn std::error::Error>> {
    let engine = AcpEngine::start(auto_approve)?;
    // No harness process outlives the engine, however it ends.
    let _kill_harnesses = KillHarnesses;
    // Another local process — the remote gateway relaying a paired desktop —
    // reaches this engine only through a description it publishes here: there
    // is no `rustcode server run` behind `--acp` (`server/engine_endpoint.rs`,
    // #3341). stdio keeps its one client either way.
    let published = publish_endpoint(&engine);
    // Over stdio the engine has this one client and ends with it.
    let served = serve_connection(&engine.server, Stdio::new()).await;
    drop(published);
    engine.shutdown().await;
    served?;
    Ok(())
}

/// Where other local processes can reach this engine while it runs. A failure
/// here leaves the engine serving stdio: the app works, a paired device is
/// told the engine has no endpoint instead of being left without an answer.
fn publish_endpoint(engine: &AcpEngine) -> Option<crate::server::engine_endpoint::Published> {
    let run_dir = match crate::server::run_dir() {
        Ok(dir) => dir,
        Err(error) => {
            crate::dbg_log!("ACP engine: no run directory to publish an endpoint in: {error}");
            return None;
        }
    };
    match crate::server::engine_endpoint::publish(&run_dir, engine) {
        Ok(published) => {
            crate::dbg_log!("ACP engine: published {}", published.endpoint());
            Some(published)
        }
        Err(error) => {
            crate::dbg_log!("ACP engine: could not publish an endpoint: {error}");
            None
        }
    }
}

/// Ends every harness process when dropped: they live as long as the engine
/// process, whichever way it ends, and no longer.
pub struct KillHarnesses;

impl Drop for KillHarnesses {
    fn drop(&mut self) {
        crate::harness::process::kill_all();
    }
}

/// What the engine runs once per process: sessions, their turns and
/// harnesses, shares and the background work. Every ACP connection
/// ([`AcpEngine::serve`]) is a client of it; one closing ends nothing that
/// runs.
#[derive(Clone)]
pub struct AcpEngine {
    server: AcpServer,
}

impl AcpEngine {
    /// Warm what every connection reads and start the engine's own tasks.
    pub fn start(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        let startup_state = crate::app::AppState::new();
        // Connect in the background so a slow server cannot hold up startup.
        crate::mcp::defer_enabled_servers(&startup_state.config.mcp_servers);
        tokio::spawn(crate::mcp::start_deferred_servers());
        // The thread above each delegated one, recorded once for threads older than
        // the record, before the first session list is read (`thread_parent.rs`).
        // After that first run it reads one `store_meta` row and returns.
        crate::thread_parent::ensure_ready();
        crate::thread_index::warm();
        // Metadata for `rustcode usage` and `rustcode data query`; a background
        // backfill of the existing threads runs once, on first open.
        crate::data_store::warm();
        // Links an earlier engine was running when it died are not running now.
        tokio::task::spawn_blocking(crate::data_store::interrupt_orphaned_links);
        let server = AcpServer::new(auto_approve)?;
        // Apps connected through `rustcode mcp-connect` find this engine by its
        // hub, so it listens from the start when any are connected.
        tokio::spawn(crate::orchestration::outside::bind_hub_when_connected());
        // Linked pull requests from before a restart carry on from here, and
        // one made by a tool during this run is picked up on the next round
        // (#3369). Without this a link would be stored and never checked.
        start_pull_watching(&server.clients);
        Ok(Self { server })
    }

    /// An engine that starts and defers nothing (tests): the default
    /// configuration's MCP servers would otherwise start on a test's turn.
    #[cfg(test)]
    pub(crate) fn bare(auto_approve: bool) -> Result<Self, agent_client_protocol::Error> {
        Ok(Self {
            server: AcpServer::new(auto_approve)?,
        })
    }

    /// Serve one client over newline-delimited JSON-RPC on `read` and
    /// `write` until it disconnects.
    pub async fn serve<R, W>(&self, read: R, write: W) -> Result<(), agent_client_protocol::Error>
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        serve_connection(&self.server, lines_transport(read, write)).await
    }

    /// How many clients are connected.
    pub fn clients(&self) -> usize {
        self.server.clients.count()
    }

    /// The engine stops: every share, harness, turn and background task ends.
    pub async fn shutdown(&self) {
        self.server.shutdown().await;
    }
}

/// Run a turn on the runtime rather than on the connection that asked for
/// it, whose tasks end with it. Answering a client that has gone fails, and
/// the turn does not care.
///
/// A turn's future is deep to poll in a debug build, deeper than a runtime
/// worker's stack; on the connection it ran on the thread serving it (the
/// main thread over stdio). It gets a thread of its own, sized like the
/// main one's and more, and what it spawns runs on the runtime.
fn spawn_turn(
    turn: impl std::future::Future<Output = Result<(), agent_client_protocol::Error>> + Send + 'static,
) {
    spawn_on_large_stack("rustcode-turn", async move {
        if let Err(error) = turn.await {
            crate::dbg_log!("a turn's answer was not delivered: {error}");
        }
    });
}

/// Poll `future` to completion on a new thread with a large stack, as part of
/// the current runtime: what it spawns runs on the runtime's workers.
pub(crate) fn spawn_on_large_stack(
    name: &str,
    future: impl std::future::Future<Output = ()> + Send + 'static,
) {
    const STACK: usize = 64 << 20;
    let runtime = tokio::runtime::Handle::current();
    // A caller scoped to its own MCP servers (a test, `--prompt`) keeps them.
    let mcp = crate::mcp::DIRECT_MCP_REGISTRY.try_with(Clone::clone).ok();
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(STACK)
        .spawn(move || match mcp {
            Some(registry) => {
                runtime.block_on(crate::mcp::DIRECT_MCP_REGISTRY.scope(registry, future))
            }
            None => runtime.block_on(future),
        });
    if let Err(error) = spawned {
        crate::dbg_log!("could not start {name}: {error}");
    }
}

/// Show a client's prompt to the session's other clients.
fn echo_prompt(
    clients: &clients::SessionClients,
    sender: clients::ClientId,
    prompt: &[ContentBlock],
    origin: Option<serde_json::Value>,
) {
    // An image goes as its marker: the other clients read the stored file back
    // (`_rustcode/attachments/read`) instead of taking its bytes in the update.
    let text = prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            ContentBlock::Image(image) => crate::controller::stored_image_marker(image),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return;
    }
    // The other clients did not write it: they are told who did.
    clients.update_others(
        sender,
        SessionUpdate::UserMessageChunk(crate::message_origin::user_chunk(
            text,
            origin.map(|origin| {
                serde_json::Map::from_iter([(
                    "rustcode".to_owned(),
                    serde_json::json!({"origin": origin}),
                )])
            }),
        )),
    );
}

/// Newline-delimited JSON-RPC over a byte stream, as over stdio.
pub(crate) fn lines_transport<R, W>(
    read: R,
    write: W,
) -> agent_client_protocol::Lines<
    impl futures_util::Sink<String, Error = std::io::Error> + Send + 'static,
    impl futures_util::Stream<Item = std::io::Result<String>> + Send + 'static,
>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let incoming = futures_util::stream::unfold(
        tokio::io::BufReader::new(read).lines(),
        async |mut lines| match lines.next_line().await {
            Ok(Some(line)) => Some((Ok(line), lines)),
            Ok(None) => None,
            Err(error) => Some((Err(error), lines)),
        },
    );
    let outgoing = futures_util::sink::unfold(write, async |mut write, line: String| {
        write.write_all(line.as_bytes()).await?;
        write.write_all(b"\n").await?;
        write.flush().await?;
        Ok::<_, std::io::Error>(write)
    });
    agent_client_protocol::Lines::new(Box::pin(outgoing), Box::pin(incoming))
}

/// Serve one ACP client on `transport` until it disconnects. The sessions it
/// creates or loads, and their turns, belong to `server`: they outlive the
/// connection, and other connections to the same server go on.
async fn serve_connection(
    server: &AcpServer,
    transport: impl agent_client_protocol::ConnectTo<Agent> + 'static,
) -> Result<(), agent_client_protocol::Error> {
    let client_id = server.clients.new_id();
    let served = Agent
        .builder()
        .name("rustcode")
        .on_receive_request(
            async |request: InitializeRequest, responder, connection| {
                let info = clients::ClientInfo::from_capabilities(&request.client_capabilities);
                server.client_initialized(&server.clients.peer(client_id, &connection), info);
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(server_capabilities()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: NewSessionRequest, responder, _connection| {
                    // `_meta.rustcode.worktree`: the thread starts in a new worktree.
                    let (request, worktree) = match crate::worktrees::prepare_new(request) {
                        Ok(prepared) => prepared,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    // The row every other client is told about: the folder the thread runs in,
                    // which is the worktree's when it starts in one, not the checkout the caller
                    // named (it would otherwise be listed under the wrong project).
                    let cwd = request.cwd.clone();
                    // An external harness starts a process, which can take a
                    // while; it does not hold the dispatch loop.
                    let peer = server.clients.peer(client_id, &_connection);
                    if let Some(asked) = harnesses::external_request(&request) {
                        let external = server.external.clone();
                        let remote = server.remote.clone();
                        let harness = asked.harness.clone();
                        let announced = server.clone();
                        return _connection.spawn(async move {
                            match harnesses::new_session(&external, request, asked, &peer, &remote)
                                .await
                            {
                                Ok(response) => {
                                    announced.announce_thread_created(ThreadCreatedNotification {
                                        meta: None,
                                        session_id: response.session_id.to_string(),
                                        cwd: cwd.clone(),
                                        title: None,
                                        harness,
                                        model: None,
                                    });
                                    if let Some(tree) = &worktree {
                                        crate::project_hooks::attach(announced.clients.session(&response.session_id.to_string()), &tree.path);
                                    }
                                    responder.respond(crate::worktrees::finish_new(
                                        response,
                                        worktree.as_ref(),
                                    ))?;
                                }
                                Err(error) => responder.respond_with_error(error)?,
                            }
                            Ok(())
                        });
                    }
                    let response = create_builtin_session(&server, &peer, request).await?;
                    server.announce_thread_created(ThreadCreatedNotification {
                                        meta: None,
                        session_id: response.session_id.to_string(),
                        cwd,
                        title: None,
                        harness: Some(crate::harness::BUILTIN_ID.to_owned()),
                        model: None,
                    });
                    if let Some(tree) = &worktree {
                        crate::project_hooks::attach(server.clients.session(&response.session_id.to_string()), &tree.path);
                    }
                    responder.respond(crate::worktrees::finish_new(response, worktree.as_ref()))?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListSessionsRequest, responder, _connection| {
                responder.respond(acp_list_sessions(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: ThreadDeleteRequest, responder, _connection| {
                    server.announce_thread_deleted(ThreadDeletedNotification {
                        session_id: request.session_id,
                    });
                    responder.respond(serde_json::json!({}))?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: presence::PresenceRequest, responder, _connection| {
                    responder.respond(server.presence.status(request.cwd))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: presence::SetPresenceRequest, responder, _connection| {
                    responder.respond(server.presence.set_enabled(request)?)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: settings::GetSettingsRequest, responder, _connection| {
                // A client that reads settings follows a change made elsewhere.
                server.clients.subscribe(client_id, clients::Topic::Settings);
                settings::start_watching(&server.clients);
                responder.respond(settings::read(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: settings::SetSettingsRequest, responder, _connection| {
                server.clients.subscribe(client_id, clients::Topic::Settings);
                settings::start_watching(&server.clients);
                responder.respond(settings::write(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: rewind::RewindRequest, responder, _connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    let answer =
                        rewind::rewind(&mut *state.lock().await, request.user_index, &request.text);
                    match answer {
                        Ok(answer) => {
                            // No update says the chat was cut: nobody
                            // resumes from before it.
                            server.clients.invalidate_cursors(&request.session_id);
                            responder.respond(answer)
                        }
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: session_move::MoveSessionRequest, responder, _connection| {
                    if !valid_acp_session_id(&request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("invalid session id"),
                        );
                    }
                    let cwd = match canonical_acp_cwd(&request.cwd, "session/move") {
                        Ok(cwd) => cwd,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    // An open chat has a turn, tools and tasks bound to its folder.
                    if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("close the session before moving it"),
                        );
                    }
                    match session_move::move_session(&request.session_id, cwd) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: session_repair::RepairSessionRequest, responder, _connection| {
                    if !valid_acp_session_id(&request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("invalid session id"),
                        );
                    }
                    if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("close the session before repairing it"),
                        );
                    }
                    match session_repair::repair_session(&request.session_id) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: usage::UsagePlansRequest, responder, connection| {
                connection.spawn(async move {
                    responder.respond(usage::plans(request).await)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: usage::UsageSharedRequest, responder, connection| {
                connection.spawn(async move {
                    responder.respond(usage::shared_plans().await)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: usage::UsageLimitsRequest, responder, connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        // A thread on another harness has no engine session: its plan is that
                        // harness's own (the Codex CLI's windows for Codex).
                        let Some(harness) = usage::external_harness(&request.session_id) else {
                            return responder
                                .respond_with_error(unknown_session(&request.session_id));
                        };
                        let session_id = request.session_id.clone();
                        return connection.spawn(async move {
                            // A named account's own login, so the meter never
                            // shows another home's numbers.
                            let home = usage::account_home(&session_id, &harness);
                            responder
                                .respond(usage::harness_limits(&harness, home.as_deref()).await)?;
                            Ok(())
                        });
                    };
                    connection.spawn(async move {
                        responder.respond(usage::limits(&state).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: commands::CommandsRequest, responder, connection| {
                connection.spawn(async move {
                    match commands::catalog_update() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: outside::OutsideSignInRequest, responder, _connection| {
                match outside::sign_in(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder
                        .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: outside::OutsidePasskeyAddRequest, responder, _connection| {
                match outside::passkey_add(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder
                        .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: outside::OutsidePasskeyRemoveRequest, responder, _connection| {
                match outside::passkey_remove(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder
                        .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: outside::OutsideWebSignOutRequest, responder, _connection| {
                match outside::web_sign_out(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder
                        .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: outside::OutsideCodeRequest, responder, _connection| {
                match outside::code(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder
                        .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideListRequest, responder, connection| {
                // A client that lists outside agents hears when one waits for it.
                server.clients.subscribe(client_id, clients::Topic::Outside);
                outside::start_publishing(&server.clients);
                connection.spawn(async move {
                    match outside::list(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::create(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideUpdateRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::update(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideRevokeRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::revoke(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: outside::OutsideAnswerRequest, responder, connection| {
                connection.spawn(async move {
                    match outside::answer(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataUsageRequest, responder, connection| {
                connection.spawn(async move {
                    match data::usage(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataQueryRequest, responder, connection| {
                connection.spawn(async move {
                    match data::query(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataActivityRequest, responder, connection| {
                connection.spawn(async move {
                    match data::activity(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: data::DataLinksLoadRequest, responder, connection| {
                connection.spawn(async move {
                    match data::links_load() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataLinksSaveRequest, responder, connection| {
                connection.spawn(async move {
                    match data::links_save(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataReceiptsGetRequest, responder, connection| {
                connection.spawn(async move {
                    match data::receipts_get(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: project_relocate::RelocateRequest, responder, _connection| {
                    // Hold both registries through validation and writes: a closed thread
                    // cannot be loaded between the check and its path being replaced.
                    let sessions = server.sessions.lock().await;
                    let external = server.external.lock().await;
                    match project_relocate::relocate(request, |id| {
                        sessions.contains_key(id) || external.get(id).is_some_and(|s| !s.is_closed())
                    }) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error)),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectReadRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::read(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectBrowseRequest, responder, connection| {
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || projects::browse(request)).await {
                        Ok(Ok(answer)) => responder.respond(answer),
                        Ok(Err(error)) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error),
                        ),
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        ),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::create(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: projects::ProjectCloneRequest, responder, connection| {
                connection.spawn(async move {
                    match projects::clone(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: attachments::AttachmentReadRequest, responder, connection| {
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || attachments::read(request)).await {
                        Ok(Ok(answer)) => responder.respond(answer),
                        Ok(Err(error)) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error),
                        ),
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        ),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: attachments::AttachmentUploadRequest, responder, connection| {
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || attachments::upload(request)).await {
                        Ok(Ok(answer)) => responder.respond(answer),
                        Ok(Err(error)) => responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error),
                        ),
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        ),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: machine_sync::MachineSyncRequest, responder, connection| {
                connection.spawn(async move {
                    match machine_sync::sync(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: loops::LoopsListRequest, responder, connection| {
                connection.spawn(async move {
                    match { let _ = request; loops::list().await } {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: loops::LoopsSaveRequest, responder, connection| {
                connection.spawn(async move {
                    match loops::save(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: loops::LoopsSetPausedRequest, responder, connection| {
                connection.spawn(async move {
                    match loops::set_paused(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: loops::LoopsStartDaemonRequest, responder, connection| {
                connection.spawn(async move {
                    match { let _ = request; loops::start_daemon().await } {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: scheduled::ScheduledListRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::list().await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledCreateRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::create(request).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledDeleteRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::delete(request.id).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: scheduled::ScheduledRunRequest, responder, connection| {
                connection.spawn(async move {
                    match scheduled::run(request.id).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: mirror::MirrorRequest, responder, connection| {
                connection.spawn(async move {
                    // The mirror reads and writes files; keep that off the connection's task.
                    let answer = tokio::task::spawn_blocking(move || mirror::answer(request))
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()));
                    match answer {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: telemetry::TelemetryRequest, responder, connection| {
                connection.spawn(async move {
                    match telemetry::answer() {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: data::DataReceiptsPutRequest, responder, connection| {
                connection.spawn(async move {
                    match data::receipts_put(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::internal_error().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: telemetry::SetTelemetryRequest, responder, connection| {
                connection.spawn(async move {
                    match telemetry::answer_set(request) {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder
                            .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
                    }?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: agents::AgentsRequest, responder, _connection| {
                    let Some((state, cwd)) = server.session_handle(&request.session_id).await
                    else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    match agents::answer(&state, &cwd, &server.client, request.action).await {
                        Ok(answer) => responder.respond(answer),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // A tool call to an MCP server waits for the user's approval, so it
        // runs off the dispatch loop.
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: mcp_app::McpAppRequest, responder, connection| {
                    let Some((state, _)) = server.session_handle(&request.session_id).await else {
                        return responder.respond_with_error(unknown_session(&request.session_id));
                    };
                    let builtin = harnesses::get(&server.external, &request.session_id)
                        .await
                        .is_none();
                    let clients = server.clients.session(&request.session_id);
                    let auto_approve = server.auto_approve;
                    connection.spawn(async move {
                        match mcp_app::handle(request, builtin, state, clients, auto_approve).await
                        {
                            Ok(answer) => responder.respond(answer)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: steer::SteerRequest, responder, connection| {
                    if let Some(external) =
                        harnesses::get(&server.external, &request.session_id).await
                    {
                        let remote = server.remote.clone();
                        return connection.spawn(async move {
                            let mut blocks = request.prompt.clone().unwrap_or_else(|| {
                                vec![agent_client_protocol::schema::v1::ContentBlock::Text(
                                    agent_client_protocol::schema::v1::TextContent::new(
                                        request.text.clone(),
                                    ),
                                )]
                            });
                            if let Err(error) = crate::controller::materialize_images(&mut blocks) {
                                responder.respond_with_error(
                                    agent_client_protocol::Error::invalid_params().data(error),
                                )?;
                                return Ok(());
                            }
                            let accepted = external.steer(&blocks).await;
                            if accepted {
                                remote.note_prompt(&request.session_id, &request.text, request.meta.as_ref().and_then(|v| v.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some()));
                            }
                            responder.respond(serde_json::json!({ "accepted": accepted }))?;
                            Ok(())
                        });
                    }
                    let state = server
                        .sessions
                        .lock()
                        .await
                        .get(&request.session_id)
                        .map(|session| Arc::clone(&session.state));
                    let Some(state) = state else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data(format!("unknown ACP session: {}", request.session_id)),
                        );
                    };
                    // The built-in harness reads a steer as text; one with images waits for the next turn.
                    let images = request.prompt.iter().flatten().any(|block| {
                        matches!(block, agent_client_protocol::schema::v1::ContentBlock::Image(_))
                    });
                    if images {
                        return responder.respond(serde_json::json!({ "accepted": false }));
                    }
                    let mut state = state.lock().await;
                    let answer = steer::accept(&mut state, &request.text);
                    if answer.get("accepted").and_then(serde_json::Value::as_bool) == Some(true) {
                        if let Some(steer) = state.pending_steers.last_mut() {
                            steer.origin = request.meta.as_ref().and_then(|v| v.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some());
                        }
                    }
                    responder.respond(answer)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Remote access starts a gateway, waits for a pairing answer and runs
        // launchctl: keep the dispatch loop free while each does.
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStatusRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.status(&request.session_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStartRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        if request.session_id.is_empty() {
                            // Settings › Remote access: switch the machine on
                            // with no thread named. Every open thread shares
                            // itself from the next auto-share pass.
                            return responder.respond(server.remote.enable().await);
                        }
                        let session = {
                            let sessions = server.sessions.lock().await;
                            sessions.get(&request.session_id).map(|session| {
                                remote::RemoteSession::from_acp(
                                    request.session_id.clone(),
                                    session,
                                    server.clients.session(&request.session_id),
                                    Arc::clone(&server.client),
                                    server.auto_approve,
                                    server.presence.clone(),
                                    server.remote.clone(),
                                )
                            })
                        };
                        let Some(session) = session else {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params().data(format!(
                                    "unknown ACP session: {}",
                                    request.session_id
                                )),
                            );
                        };
                        match server.remote.start(session, request.pair).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error.into_acp())?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemotePairRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.pair(&request.session_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteStopRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        match server.remote.stop(&request.session_id).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteAnswerRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(
                            server
                                .remote
                                .answer_connect(&request.id, request.accept)
                                .await,
                        )?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                async move |request: remote::RemoteConnectCodeRequest, responder, connection| {
                    connection.spawn(async move {
                        responder.respond(remote::connect_code(request.cancel).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: remote::RemoteRevokeRequest, responder, connection| {
                    let server = server.clone();
                    connection.spawn(async move {
                        responder.respond(server.remote.revoke(&request.device_id).await)?;
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Harness detection runs commands and a model listing may start
        // one, so none of these holds the dispatch loop.
        .on_receive_request(
            async move |_request: harnesses::HarnessesRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::snapshot().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::HarnessModelsRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::models(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::SetHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::set(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::RemoveHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::remove(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::DefaultHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::set_default(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::RememberHarnessRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::remember(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::AddHarnessAccountRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::add_account(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: harnesses::RemoveHarnessAccountRequest, responder, connection| {
                    let external = server.external.clone();
                    connection.spawn(async move {
                        match harnesses::remove_account(&external, request).await {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: harnesses::CheckHarnessAccountsRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::check_accounts(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: harnesses::SwitchHarnessRequest, responder, connection| {
                    let server = server.clone();
                    let peer = server.clients.peer(client_id, &connection);
                    connection.spawn(async move {
                        let session_id = request.session_id.clone();
                        let builtin = if harnesses::get(&server.external, &session_id).await.is_some() {
                            None
                        } else if crate::thread_status::is_running(&session_id) {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data("stop the running turn before switching harness"),
                            );
                        } else {
                            close_builtin(&server, &session_id).await
                        };
                        match harnesses::switch(
                            &server.external,
                            request,
                            builtin,
                            &peer,
                            &server.remote,
                        )
                        .await
                        {
                            Ok(value) => responder.respond(value)?,
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: harnesses::OnboardingCompleteRequest, responder, connection| {
                connection.spawn(async move {
                    match harnesses::complete_onboarding().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: capabilities::CapabilitiesRequest, responder, _connection| {
                responder.respond(capabilities::snapshot_value()?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::SetCapabilityRequest, responder, connection| {
                // Starting an MCP server can take seconds; keep the dispatch
                // loop free while it does.
                connection.spawn(async move {
                    match capabilities::set_capability(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::AddMcpServerRequest, responder, connection| {
                connection.spawn(async move {
                    match capabilities::add_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::RemoveMcpServerRequest, responder, connection| {
                connection.spawn(async move {
                    match capabilities::remove_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::RestartMcpServerRequest, responder, connection| {
                // Connecting a server again can take the same seconds a start
                // does; keep the dispatch loop free while it does.
                connection.spawn(async move {
                    match capabilities::restart_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::LogoutMcpServerRequest, responder, connection| {
                connection.spawn(async move {
                    match capabilities::logout_mcp_server(request).await {
                        Ok(snapshot) => responder.respond(snapshot)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::AddSkillRequest, responder, _connection| {
                responder.respond(capabilities::add_skill(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: capabilities::GetSkillRequest, responder, _connection| {
                responder.respond(capabilities::get_skill(request)?)
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Orchestrator v2 (`docs/orchestrator.md`): worktrees, thread status
        // and checkpoints. Each runs git, so none holds the dispatch loop.
        .on_receive_request(
            async move |request: crate::worktrees::CreateRequest, responder, connection| {
                connection.spawn(async move {
                    let created = tokio::task::spawn_blocking(move || crate::worktrees::create(&request)).await;
                    match created {
                        Ok(Ok(worktree)) => responder.respond(serde_json::json!(worktree))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::ListRequest, responder, connection| {
                connection.spawn(async move {
                    let listed = tokio::task::spawn_blocking(move || crate::worktrees::list(&request.repo)).await;
                    match listed {
                        Ok(Ok(worktrees)) => responder.respond(serde_json::json!({ "worktrees": worktrees }))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::RemoveRequest, responder, connection| {
                connection.spawn(async move {
                    let removed = tokio::task::spawn_blocking(move || crate::worktrees::remove(&request.path, request.force)).await;
                    match removed {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::text_generation::ConfigureRequest, responder, connection| {
                connection.spawn(async move {
                    match crate::text_generation::configure(request).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::transcription::TranscribeRequest, responder, connection| {
                connection.spawn(async move {
                    match crate::transcription::transcribe(request).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::text_generation::GenerateRequest, responder, connection| {
                connection.spawn(async move {
                    match crate::text_generation::generate(request).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::git_actions::StateRequest, responder, connection| {
                connection.spawn(async move {
                    let state = tokio::task::spawn_blocking(move || crate::git_actions::state(&request.path)).await;
                    match state {
                        Ok(Ok(state)) => responder.respond(state)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::git_actions::RunRequest, responder, connection| {
                connection.spawn(async move {
                    match crate::git_actions::run(request).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(error.into_acp())?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::StateRequest, responder, connection| {
                connection.spawn(async move {
                    let states = tokio::task::spawn_blocking(move || crate::worktrees::states(&request.paths)).await;
                    match states {
                        Ok(rows) => responder.respond(serde_json::json!({ "worktrees": rows }))?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::worktrees::HandoffRequest, responder, connection| {
                connection.spawn(async move {
                    let to = crate::worktrees::Handoff::parse(request.to.trim());
                    let Some(to) = to else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data(serde_json::json!({
                                    "reason": "invalid",
                                    "message": format!("to must be checkout or worktree, not '{}'", request.to),
                                })),
                        );
                    };
                    let moved = tokio::task::spawn_blocking(move || {
                        crate::worktrees::handoff(
                            &request.session_id,
                            to,
                            request.title.as_deref(),
                            request.branch.as_deref(),
                        )
                    })
                    .await;
                    match moved {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::PinRequest, responder, _connection| {
                match crate::thread_status::set_pinned(&request.session_id, request.pinned) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: permissions::PermissionsRequest, responder, _connection| {
                // The user config is what every request's policy reads, so a
                // client lists and revokes there rather than in one session.
                let (_, _, config) = crate::config::load_config();
                responder.respond(permissions::grants_snapshot(&config))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: permissions::RevokePermissionRequest, responder, _connection| {
                    let (_, _, mut config) = crate::config::load_config();
                    let answer = permissions::revoke_grant(&mut config, &request.id);
                    // Open sessions hold their own copy of the user config: the
                    // one that answers a call must stop covering it now.
                    for session in server.sessions.lock().await.values() {
                        session.state.lock().await.config.timed_command_approvals =
                            config.timed_command_approvals.clone();
                    }
                    responder.respond(answer)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: themes::ThemesRequest, responder, connection| {
                // A client that lists themes restyles when they change.
                server.clients.subscribe(client_id, clients::Topic::Themes);
                themes::start_watching(&server.clients);
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || themes::answer(request)).await {
                        Ok(answer) => responder.respond(answer)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async |request: themes::SetThemeRequest, responder, connection| {
                server.clients.subscribe(client_id, clients::Topic::Themes);
                themes::start_watching(&server.clients);
                connection.spawn(async move {
                    match tokio::task::spawn_blocking(move || themes::answer_set(request)).await {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::SnoozeRequest, responder, _connection| {
                match crate::thread_status::set_snoozed(&request.session_id, request.until) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_status::SetStatusRequest, responder, _connection| {
                match crate::thread_status::set(&request.session_id, &request.status) {
                    Ok(()) => responder.respond(serde_json::json!({})),
                    Err(message) => responder.respond_with_error(agent_client_protocol::Error::invalid_params().data(message)),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::thread_index::SearchRequest, responder, connection| {
                connection.spawn(async move {
                    let answer =
                        tokio::task::spawn_blocking(move || crate::thread_index::answer_search(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(message)) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(message),
                        )?,
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        )?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::checkpoints::ListRequest, responder, connection| {
                connection.spawn(async move {
                    let answer = tokio::task::spawn_blocking(move || crate::checkpoints::answer_list(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: crate::checkpoints::DiffRequest, responder, connection| {
                connection.spawn(async move {
                    let answer = tokio::task::spawn_blocking(move || crate::checkpoints::answer_diff(&request)).await;
                    match answer {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error)?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: crate::checkpoints::RestoreRequest, responder, connection| {
                    // Only an open built-in thread rewinds its conversation.
                    let state = server.session_handle(&request.session_id).await.map(|(state, _)| state);
                    let clients = server.clients.clone();
                    connection.spawn(async move {
                        match crate::checkpoints::answer_restore(&request, state).await {
                            Ok(answer) => {
                                // A restore may cut the chat without an update.
                                clients.invalidate_cursors(&request.session_id);
                                responder.respond(answer)?
                            }
                            Err(error) => responder.respond_with_error(error)?,
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |_request: server_status::ServerStatusRequest, responder, _connection| {
                    responder.respond(server_status::status(&server))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: WarmRequest, responder, connection| {
                connection.spawn(async move {
                    crate::mcp::start_deferred_servers().await;
                    Ok(())
                })?;
                responder.respond(serde_json::json!({}))
            },
            agent_client_protocol::on_receive_request!(),
        )
        // `gh` can take seconds, so neither holds the dispatch loop.
        .on_receive_request(
            async move |request: PullsRequest, responder, connection| {
                connection.spawn(async move {
                    let listed = crate::pull_requests::list(
                        &request.cwd,
                        request.state.as_deref().unwrap_or("open"),
                        request.search.as_deref(),
                        request.limit.unwrap_or(30),
                    )
                    .await;
                    responder.respond(match listed {
                        Ok(list) => serde_json::json!({ "list": list }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullViewRequest, responder, connection| {
                connection.spawn(async move {
                    let viewed = crate::pull_requests::view(&request.cwd, request.number).await;
                    responder.respond(match viewed {
                        Ok(pull) => serde_json::json!({ "pull": pull }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullBranchRequest, responder, connection| {
                connection.spawn(async move {
                    let prepared = crate::pull_requests::prepare_branch(&request.cwd, request.number).await;
                    responder.respond(match prepared {
                        Ok(branch) => serde_json::json!({ "branch": branch }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullStateRequest, responder, connection| {
                connection.spawn(async move {
                    let asked = request
                        .branch
                        .filter(|branch| !branch.trim().is_empty());
                    let branch = match asked {
                        Some(branch) => Some(branch),
                        // Resolving HEAD runs git, so it does not hold an async worker.
                        None => {
                            let cwd = request.cwd.clone();
                            match tokio::task::spawn_blocking(move || {
                                crate::pull_requests::current_branch(&cwd)
                            })
                            .await
                            {
                                Ok(branch) => branch,
                                Err(error) => {
                                    return responder.respond_with_error(
                                        agent_client_protocol::Error::internal_error()
                                            .data(error.to_string()),
                                    );
                                }
                            }
                        }
                    };
                    let Some(branch) = branch else {
                        return responder.respond(serde_json::json!({
                            "branch": null,
                            "pull": null,
                            "error": { "code": "not_a_repository", "message": "the folder is not on a git branch" }
                        }));
                    };
                    let answer = match crate::pull_requests::for_branch(&request.cwd, &branch).await {
                        Ok(pull) => serde_json::json!({ "branch": branch, "pull": pull }),
                        Err(error) => serde_json::json!({ "branch": branch, "error": error }),
                    };
                    responder.respond(answer)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullMergeCancelRequest, responder, _connection| {
                let cancelled = crate::pull_requests::cancel_merge(&request.merge_id);
                responder.respond(serde_json::json!({ "cancelled": cancelled }))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullMergeRequest, responder, connection| {
                // Registered here, in the order requests arrive, so a cancel that
                // follows this request always finds the merge it is for.
                let cancel = request
                    .merge_id
                    .as_deref()
                    .map(crate::pull_requests::register_merge)
                    .unwrap_or_default();
                connection.spawn(async move {
                    let _tracked = crate::pull_requests::MergeGuard(request.merge_id.clone());
                    let wait = std::time::Duration::from_millis(
                        request
                            .timeout_ms
                            .unwrap_or(crate::pull_requests::MAX_MERGE_WAIT_SECS * 1000)
                            .max(1000),
                    );
                    let answer = match crate::pull_requests::merge_branch_when_green(
                        &request.cwd,
                        request.number,
                        request.branch.clone(),
                        wait,
                        &cancel,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            let announced = match (outcome.merged, request.session_id) {
                                (true, Some(session_id)) => {
                                    let number = outcome.number;
                                    tokio::task::spawn_blocking(move || {
                                        crate::pull_watch::WatchStore::default_store()
                                            .and_then(|store| {
                                                store.claim_merge(&session_id, number).ok()
                                            })
                                            .unwrap_or(false)
                                    })
                                    .await
                                    .unwrap_or(false)
                                }
                                _ => false,
                            };
                            serde_json::json!({ "merge": outcome, "announced": announced })
                        }
                        Err(error) => serde_json::json!({ "error": error }),
                    };
                    responder.respond(answer)?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WorktreeCleanupRequest, responder, connection| {
                connection.spawn(async move {
                    let cleaned = crate::worktrees::cleanup_resolving_rename(
                        request.path,
                        request.branch,
                        request.head_sha,
                        request.merged,
                    )
                    .await;
                    match cleaned {
                        Ok(Ok(cleanup)) => responder.respond(serde_json::json!({ "cleanup": cleanup }))?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(
                            agent_client_protocol::Error::internal_error().data(error.to_string()),
                        )?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullDiffRequest, responder, connection| {
                connection.spawn(async move {
                    let diff = crate::pull_requests::diff(&request.cwd, request.number).await;
                    responder.respond(match diff {
                        Ok(diff) => serde_json::to_value(diff).unwrap_or_default(),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullReferencesRequest, responder, connection| {
                connection.spawn(async move {
                    let read = crate::pull_requests::references(&request.cwd).await;
                    responder.respond(match read {
                        Ok(references) => serde_json::json!({ "references": references }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullThreadRequest, responder, connection| {
                connection.spawn(async move {
                    let done = crate::pull_requests::act_on_thread(
                        &request.cwd,
                        &request.repository,
                        request.number,
                        &request.action,
                    )
                    .await;
                    responder.respond(match done {
                        Ok(done) => serde_json::json!({ "done": done }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullReviewRequest, responder, connection| {
                connection.spawn(async move {
                    let posted = match crate::pull_requests::review_payload(
                        &request.commit_id,
                        &request.event,
                        &request.body,
                        &request.comments,
                    ) {
                        Ok(payload) => {
                            crate::pull_requests::post_review(
                                &request.cwd,
                                &request.repository,
                                request.number,
                                &payload,
                            )
                            .await
                        }
                        Err(error) => Err(error),
                    };
                    responder.respond(match posted {
                        Ok(review) => serde_json::json!({ "review": review }),
                        Err(error) => serde_json::json!({ "error": error }),
                    })?;
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchStartRequest, responder, _connection| {
                server.clients.subscribe(client_id, clients::Topic::PullWatches);
                start_pull_watching(&server.clients);
                let Some(store) = crate::pull_watch::WatchStore::default_store() else {
                    return responder.respond(serde_json::json!({ "error": { "code": "failed", "message": "No config directory" } }));
                };
                let started = if request.link {
                    match crate::pull_requests::watch_view_on(&request.cwd, &request.repo, request.number).await {
                        Ok(value) => store.link(&request.session_id, &request.cwd, &request.repo, request.number, &value, chrono::Utc::now().timestamp()),
                        Err(error) => Err(error.message),
                    }
                } else { store.start(
                    &request.session_id,
                    &request.cwd,
                    request.number,
                    request.interests.unwrap_or_default(),
                    chrono::Utc::now().timestamp(),
                ) };
                responder.respond(match started {
                    Ok(watch) => serde_json::json!({ "watch": watch }),
                    Err(message) => serde_json::json!({ "error": { "code": "failed", "message": message } }),
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchStopRequest, responder, _connection| {
                let stopped = crate::pull_watch::WatchStore::default_store()
                    .and_then(|store| store.stop(&request.session_id, request.number, request.remove).ok())
                    .flatten();
                responder.respond(serde_json::json!({ "watch": stopped }))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PullWatchesRequest, responder, _connection| {
                // The app asks on launch: watches from before a restart
                // carry on from here.
                server.clients.subscribe(client_id, clients::Topic::PullWatches);
                start_pull_watching(&server.clients);
                if let (Some(number), Some(session_id)) = (request.take, request.session_id.as_deref()) {
                    let taken = crate::pull_watch::WatchStore::default_store()
                        .map(|store| store.take(session_id, number))
                        .transpose();
                    return responder.respond(match taken {
                        Ok(Some(Some((watch, events)))) => serde_json::json!({ "watch": watch, "events": events }),
                        Ok(_) => serde_json::json!({ "events": [] }),
                        Err(error) => serde_json::json!({ "error": error.to_string() }),
                    });
                }
                let watches = crate::pull_watch::WatchStore::default_store()
                    .map(|store| store.list(request.session_id.as_deref()))
                    .unwrap_or_default();
                responder.respond(serde_json::json!({ "watches": watches }))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: workflows::WorkflowListRequest, responder, connection| {
                server.clients.subscribe(client_id, clients::Topic::Workflows);
                let clients = server.clients.clone();
                connection.spawn(async move {
                    responder.respond(workflows::list(request, &clients).await)
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: workflows::WorkflowStopRequest, responder, connection| {
                // Stopping waits for the workers to be interrupted.
                connection.spawn(async move { responder.respond(workflows::stop(request).await) })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: workflows::WorkflowResumeRequest, responder, _connection| {
                responder.respond(workflows::resume(request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: workflows::WorkflowWakeRequest, responder, _connection| {
                responder.respond(workflows::wake(&request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        // Each runs gh or the account store, so none holds the dispatch loop.
        .on_receive_request(
            async move |_request: connections::ConnectionsRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::snapshot_value().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubLoginRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_login().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubWaitRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_wait().await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::GitHubLogoutRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::github_logout(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::AccountRefreshRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::account_refresh(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: connections::AccountLogoutRequest, responder, connection| {
                connection.spawn(async move {
                    match connections::account_logout(request).await {
                        Ok(value) => responder.respond(value)?,
                        Err(error) => responder.respond_with_error(error)?,
                    }
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: connections::GitHubCancelRequest, responder, _connection| {
                crate::connections::cancel_github_login();
                responder.respond(serde_json::Value::Null)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: LoadSessionRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    let peer = server.clients.peer(client_id, &connection);
                    let response = acp_load_session(&server, request, &peer).await?;
                    if let Some(tree) = crate::worktrees::of_session(&session_id) {
                        crate::project_hooks::attach(server.clients.session(&session_id), &tree.path);
                    }
                    let response = crate::worktrees::finish_load(response, &session_id);
                    responder.respond(response)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: CloseSessionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    if harnesses::close(&server.external, &session_id, &server.remote).await {
                        server.clients.forget_session(&session_id);
                        server.presence.session_closed(&session_id);
                        return responder.respond(CloseSessionResponse::new());
                    }
                    close_builtin(&server, &session_id).await;
                    server.presence.session_closed(&session_id);
                    responder.respond(CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                    let session_id = request.session_id.to_string();
                    if let Some(external) = harnesses::get(&server.external, &session_id).await {
                        // Setting an option may start the harness.
                        return _connection.spawn(async move {
                            let value = harnesses::config_value(&request.value);
                            let config_id = request.config_id.0.to_string();
                            match external.set_config(&config_id, value).await {
                                Ok(options) => {
                                    // A model or effort picked in a thread is also the
                                    // harness's choice for the next new thread.
                                    if matches!(config_id.as_str(), "model" | "effort") {
                                        let record = external.record();
                                        crate::harness::remember_later(
                                            external.harness.id.clone(),
                                            record.model,
                                            record.effort,
                                        );
                                    }
                                    responder.respond(SetSessionConfigOptionResponse::new(options))?
                                }
                                Err(message) => responder.respond_with_error(
                                    agent_client_protocol::Error::invalid_params().data(message),
                                )?,
                            }
                            Ok(())
                        });
                    }
                    let state = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| Arc::clone(&session.state));
                    let Some(state) = state else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };

                    let mut state = state.lock().await;
                    let config_options =
                        handle_set_config_option(&mut state, &request.config_id, &request.value)?;
                    if matches!(request.config_id.0.as_ref(), "model" | "effort") {
                        let settings = crate::remote::projection::project_settings(&state);
                        crate::harness::remember_later(
                            crate::harness::BUILTIN_ID.to_owned(),
                            Some(settings.selected_model).filter(|model| !model.is_empty()),
                            Some(settings.reasoning_effort),
                        );
                    }
                    responder.respond(SetSessionConfigOptionResponse::new(config_options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let server = server.clone();
                async move |notification: CancelNotification, _connection| {
                    if let Some(external) =
                        harnesses::get(&server.external, notification.session_id.0.as_ref()).await
                    {
                        external.cancel();
                        return Ok(());
                    }
                    let turns = server
                        .sessions
                        .lock()
                        .await
                        .get(notification.session_id.0.as_ref())
                        .map(|session| Arc::clone(&session.turns));
                    if let Some(turns) = turns {
                        turns.cancel_active().await;
                    }
                    crate::tools::abort_background_starts(notification.session_id.0.as_ref());
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let server = server.clone();
                async move |mut request: PromptRequest, responder, connection| {
                    let session_id = request.session_id.to_string();
                    // Every image is a stored one from here on: one sent by reference gets
                    // its bytes back, one sent inline is kept, so the thread shows it again
                    // later, here or on a paired computer.
                    if let Err(error) = crate::controller::materialize_images(&mut request.prompt) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error),
                        );
                    }
                    let origin = request.meta.as_ref().and_then(|meta| meta.get("rustcode")).and_then(|v| v.get("origin")).cloned().filter(|v| crate::message_origin::from_meta(v).is_some());
                    // A wake turn (the app telling an orchestrator what its delegates did) is
                    // recorded with how many delegates it carried; its tokens and cost are on
                    // the turn's own row (issue 3649).
                    crate::message_origin::record_wake(
                        &session_id,
                        &request
                            .prompt
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text.text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                    // The access the client runs this thread at, kept where the
                    // engine reads it with no app attached (orchestration/local_threads.rs).
                    if let Some(access) = request
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get("rustcode"))
                        .and_then(|rustcode| rustcode.get("access"))
                        .and_then(serde_json::Value::as_str)
                    {
                        crate::config::save_session_access(&session_id, access);
                    }
                    if let Some(external) = harnesses::get(&server.external, &session_id).await {
                        // The access level the client runs the thread at: a harness that
                        // sandboxes its own shell leaves the sandbox for Full access.
                        if let Some(full) =
                            crate::harness::requested_full_access(request.meta.as_ref())
                        {
                            external.set_full_access(full);
                        }
                        // A device reading the shared thread sees this prompt
                        // too: a harness does not echo it back.
                        let prompt_text = request
                            .prompt
                            .iter()
                            .filter_map(|block| match block {
                                ContentBlock::Text(text) => Some(text.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        if crate::config::load_session_title(&session_id).is_none()
                            && crate::app::actions::wants_generated_title(&prompt_text)
                        {
                            let history = [crate::app::ChatMessage::new("user", prompt_text.clone())];
                            crate::config::save_session_title_if_absent(&session_id, &history[..]);
                            let fallback = crate::config::session_title(&history);
                            let title_clients = server.clients.session(&session_id);
                            let update = SessionUpdate::SessionInfoUpdate(agent_client_protocol::schema::v1::SessionInfoUpdate::new().title(fallback.clone()));
                            let _ = prompt::send_updates(&title_clients, &session_id, vec![update]);
                            if crate::config::load_config().2.generate_session_titles
                                && crate::text_generation::configured_for(crate::config::load_session_repo(&session_id).as_deref())
                            {
                                let title_session = session_id.clone();
                                let title_prompt = prompt_text.clone();
                                tokio::spawn(async move {
                                    if let Ok(answer) = crate::text_generation::generate(crate::text_generation::GenerateRequest {
                                        kind: "thread_title".into(), prompt: Some(title_prompt), path: None, files: vec![],
                                        project: crate::config::load_session_repo(&title_session),
                                    }).await {
                                        if let Some(title) = answer["text"].as_str().and_then(crate::app::actions::normalize_generated_title) {
                                            if crate::app::actions::apply_generated_title(&title_session, Some(&fallback), &title) {
                                                let update = SessionUpdate::SessionInfoUpdate(agent_client_protocol::schema::v1::SessionInfoUpdate::new().title(title));
                                                let _ = prompt::send_updates(&title_clients, &title_session, vec![update]);
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        server.remote.note_prompt(&session_id, &prompt_text, origin.clone());
                        server.remote.note_prompt_applied(&session_id);
                        let remote = server.remote.clone();
                        let turn_session = session_id.clone();
                        // The other clients that have the thread show the
                        // prompt too; the one that sent it already does.
                        echo_prompt(&server.clients.session(&session_id), client_id, &request.prompt, origin.clone());
                        // The turn belongs to the engine, not this connection:
                        // it runs on if the client goes, and only its answer
                        // is lost.
                        spawn_turn(async move {
                            // A worktree whose setup the project waits for holds the
                            // turn until it is done; a failure is the agent's first read.
                            let mut prompt = request.prompt;
                            if let Some(note) = crate::project_hooks::wait_setup(&turn_session).await {
                                prompt.insert(0, ContentBlock::Text(agent_client_protocol::schema::v1::TextContent::new(note)));
                            }
                            let result = external.prompt(prompt).await;
                            remote.note_turn_returned(&turn_session);
                            match result {
                                Ok(stop_reason) => {
                                    responder.respond(PromptResponse::new(stop_reason))?
                                }
                                Err(message) => responder.respond_with_error(
                                    agent_client_protocol::Error::internal_error().data(message),
                                )?,
                            }
                            Ok(())
                        });
                        return Ok(());
                    }
                    let session = server
                        .sessions
                        .lock()
                        .await
                        .get(&session_id)
                        .map(|session| {
                            (
                                Arc::clone(&session.state),
                                session.cwd.clone(),
                                Arc::clone(&session.turns),
                                Arc::clone(&session.task_events),
                                Arc::clone(&session.known_task_ids),
                                Arc::clone(&session.terminal_backlog),
                                Arc::clone(&session.terminal_overflow),
                            )
                        });
                    let Some((
                        state,
                        cwd,
                        turns,
                        task_events,
                        known_task_ids,
                        terminal_backlog,
                        terminal_overflow,
                    )) = session
                    else {
                        return Err(agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {session_id}")));
                    };
                    let has_images = request
                        .prompt
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Image(_)));
                    if has_images {
                        if let Some(message) = state.lock().await.image_support_error() {
                            return Err(agent_client_protocol::Error::invalid_params()
                                .data(message.to_string()));
                        }
                    }
                    let text = prompt_input(&request.prompt)
                        .map_err(|message| {
                            agent_client_protocol::Error::invalid_params().data(message)
                        })?
                        .text;
                    echo_prompt(&server.clients.session(&session_id), client_id, &request.prompt, origin.clone());

                    // A report only reads the session, so it answers now, beside
                    // a running turn, instead of queueing at the turn gate.
                    if !has_images && commands::is_report(&text) {
                        let session_clients = server.clients.session(&session_id);
                        connection.spawn(async move {
                            match prompt::run_report(&state, &session_id, &text, &session_clients)
                                .await
                            {
                                Ok(stop_reason) => {
                                    responder.respond(PromptResponse::new(stop_reason))?
                                }
                                Err(error) => responder.respond_with_error(error)?,
                            }
                            Ok(())
                        })?;
                        return Ok(());
                    }

                    // Request handlers run on ACP's dispatch loop. A model turn can
                    // perform many network and tool operations, so awaiting it here
                    // prevents the connection from processing any other messages.
                    let scheduled_turn = turns.schedule();
                    let wakeup_turns = Arc::downgrade(&turns);
                    let prompt_server = server.clone();
                    let session_clients = server.clients.session(&session_id);
                    let presence_cwd = cwd.clone();
                    server.presence.turn_started(&session_id, &presence_cwd);
                    // The turn belongs to the engine, not this connection: it
                    // runs on if the client goes, and only its answer is lost.
                    spawn_turn(async move {
                        // A worktree whose setup the project waits for holds the turn
                        // until it is done; a failure is the agent's first read.
                        let text = match crate::project_hooks::wait_setup(&session_id).await {
                            Some(note) => format!("{note}{text}"),
                            None => text,
                        };
                        let result = prompt::run_prompt(
                            Arc::clone(&state),
                            cwd,
                            scheduled_turn,
                            session_id.clone(),
                            task_events,
                            known_task_ids,
                            terminal_backlog,
                            terminal_overflow,
                            text,
                            origin,
                            session_clients,
                            prompt_server.client,
                            prompt_server.auto_approve,
                            Some(prompt_server.remote.clone()),
                            wakeup_turns,
                        )
                        .await;
                        prompt_server
                            .presence
                            .turn_finished(&session_id, &presence_cwd);

                        // Steers the turn ended before reading go back to the
                        // client, which sends them next.
                        let unused = steer::end(&mut *state.lock().await, &session_id);
                        match result {
                            Ok(stop_reason) => {
                                responder.respond(
                                    PromptResponse::new(stop_reason)
                                        .meta(steer::unused_meta(unused)),
                                )?;
                            }
                            Err(error) => {
                                responder.respond_with_error(error)?;
                            }
                        }
                        Ok(())
                    });
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(transport)
        .await;
    server.client_closed(client_id);
    served
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
