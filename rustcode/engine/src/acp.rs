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
pub(crate) mod machine_exec;
mod machine_sync;
mod machine_update;
mod mcp_app;
mod mirror;
pub(crate) mod outside;
mod permissions;
mod presence;
mod preview;
mod project_relocate;
pub(crate) mod projects;
mod prompt;
pub(crate) mod questions;
mod remote;
mod rewind;
pub(crate) mod scheduled;
mod server_status;
mod session;
mod session_load;
pub(crate) mod session_move;
mod session_repair;
mod session_replay;
mod session_transfer;
pub(crate) mod settings;
pub(crate) mod steer;
mod stop;
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
                "sessionTransfer": 1,
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
    // is no `rc server run` behind `--acp` (`server/engine_endpoint.rs`,
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
        // Metadata for `rc usage` and `rc data query`; a background
        // backfill of the existing threads runs once, on first open.
        crate::data_store::warm();
        // Links an earlier engine was running when it died are not running now.
        tokio::task::spawn_blocking(crate::data_store::interrupt_orphaned_links);
        let server = AcpServer::new(auto_approve)?;
        // Apps connected through `rc mcp-connect` find this engine by its
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
    std::pin::Pin<Box<dyn futures_util::Sink<String, Error = std::io::Error> + Send + 'static>>,
    std::pin::Pin<Box<dyn futures_util::Stream<Item = std::io::Result<String>> + Send + 'static>>,
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
    let outgoing: std::pin::Pin<
        Box<dyn futures_util::Sink<String, Error = std::io::Error> + Send + 'static>,
    > = Box::pin(outgoing);
    let incoming: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = std::io::Result<String>> + Send + 'static>,
    > = Box::pin(incoming);
    agent_client_protocol::Lines::new(outgoing, incoming)
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, agent_client_protocol::Error> {
    T::deserialize(&params).map_err(|error| invalid_params_deser(error, params))
}

fn invalid_params_deser(
    error: serde_json::Error,
    json: serde_json::Value,
) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(serde_json::json!({
        "error": error.to_string(),
        "json": json,
        "phase": "deserialization",
    }))
}

fn respond_typed<T: serde::Serialize>(
    responder: agent_client_protocol::Responder,
    response: T,
) -> Result<(), agent_client_protocol::Error> {
    let value = serde_json::to_value(response)
        .map_err(agent_client_protocol::Error::into_internal_error)?;
    responder.respond(value)
}

/// Serve one ACP client on `transport` until it disconnects. The sessions it
/// creates or loads, and their turns, belong to `server`: they outlive the
/// connection, and other connections to the same server go on.
async fn serve_connection(
    server: &AcpServer,
    transport: impl agent_client_protocol::ConnectTo<Agent> + 'static,
) -> Result<(), agent_client_protocol::Error> {
    serve_dyn_connection(server, agent_client_protocol::DynConnectTo::new(transport)).await
}

/// Non-generic entry point for serving a connection so the builder, incoming
/// protocol actor, and request dispatch table are monomorphized once across
/// stdio, socket, and test transports.
async fn serve_dyn_connection(
    server: &AcpServer,
    transport: agent_client_protocol::DynConnectTo<Agent>,
) -> Result<(), agent_client_protocol::Error> {
    let client_id = server.clients.new_id();
    let served = agent_client_protocol::Builder::new_with(
        Agent,
        AcpHandler {
            server: server.clone(),
            client_id,
        },
    )
    .name("rustcode")
    .connect_to(transport)
    .await;
    server.client_closed(client_id);
    served
}

/// Single flat handler for all incoming ACP requests and notifications,
/// avoiding a deep `ChainedHandler` / `RequestHandler` generic chain.
struct AcpHandler {
    server: AcpServer,
    client_id: clients::ClientId,
}

impl agent_client_protocol::HandleDispatchFrom<agent_client_protocol::Client> for AcpHandler {
    fn describe_chain(&self) -> impl std::fmt::Debug {
        "rustcode::acp::AcpHandler"
    }

    async fn handle_dispatch_from(
        &mut self,
        dispatch: agent_client_protocol::Dispatch,
        connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    ) -> Result<
        agent_client_protocol::Handled<agent_client_protocol::Dispatch>,
        agent_client_protocol::Error,
    > {
        match dispatch {
            agent_client_protocol::Dispatch::Request(message, responder) => {
                let mut unhandled = None;
                dispatch_request(
                    &self.server,
                    self.client_id,
                    message,
                    responder,
                    connection,
                    &mut unhandled,
                )
                .await?;
                Ok(match unhandled {
                    Some(dispatch) => agent_client_protocol::Handled::No {
                        message: dispatch,
                        retry: false,
                    },
                    None => agent_client_protocol::Handled::Yes,
                })
            }
            agent_client_protocol::Dispatch::Notification(message) => {
                let (method, params) = message.into_parts();
                if method != "session/cancel" {
                    return Ok(agent_client_protocol::Handled::No {
                        message: agent_client_protocol::Dispatch::Notification(
                            agent_client_protocol::UntypedMessage { method, params },
                        ),
                        retry: false,
                    });
                }
                let notification: CancelNotification = parse_params(params)?;
                let attempt = stop::notification_attempt(notification.meta.as_ref());
                stop::stop(
                    &self.server,
                    notification.session_id.0.as_ref(),
                    attempt.as_deref(),
                    "notification",
                )
                .await;
                Ok(agent_client_protocol::Handled::Yes)
            }
            agent_client_protocol::Dispatch::Response(..) => {
                Ok(agent_client_protocol::Handled::No {
                    message: dispatch,
                    retry: false,
                })
            }
        }
    }
}

async fn dispatch_request(
    server: &AcpServer,
    client_id: clients::ClientId,
    message: agent_client_protocol::UntypedMessage,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    let (method, params) = message.into_parts();
    match method.as_str() {
        "initialize" => {
            let request: InitializeRequest = parse_params(params)?;

            let info = clients::ClientInfo::from_capabilities(&request.client_capabilities);
            server.client_initialized(&server.clients.peer(client_id, &connection), info);
            respond_typed(
                responder,
                InitializeResponse::new(request.protocol_version)
                    .agent_capabilities(server_capabilities()),
            )
        }
        "session/new" => {
            let server = server.clone();
            let request: NewSessionRequest = parse_params(params)?;

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
            let peer = server.clients.peer(client_id, &connection);
            if let Some(asked) = harnesses::external_request(&request) {
                let external = server.external.clone();
                let remote = server.remote.clone();
                let harness = asked.harness.clone();
                let announced = server.clone();
                return connection.spawn(async move {
                    match harnesses::new_session(&external, request, asked, &peer, &remote).await {
                        Ok(response) => {
                            announced.announce_thread_created(ThreadCreatedNotification {
                                meta: None,
                                session_id: response.session_id.to_string(),
                                cwd: cwd.clone(),
                                title: None,
                                harness,
                                model: None,
                            });
                            respond_typed(
                                responder,
                                crate::worktrees::finish_new(response, worktree.as_ref()),
                            )?;
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
            respond_typed(
                responder,
                crate::worktrees::finish_new(response, worktree.as_ref()),
            )?;
            Ok(())
        }
        "session/list" => {
            let request: ListSessionsRequest = parse_params(params)?;

            respond_typed(responder, acp_list_sessions(request)?)
        }
        "_rustcode/threads/delete" => {
            let server = server.clone();
            let request: ThreadDeleteRequest = parse_params(params)?;

            server.announce_thread_deleted(ThreadDeletedNotification {
                session_id: request.session_id,
            });
            responder.respond(serde_json::json!({}))?;
            Ok(())
        }
        "_rustcode/presence" => {
            let server = server.clone();
            let request: presence::PresenceRequest = parse_params(params)?;

            responder.respond(server.presence.status(request.cwd))
        }
        "_rustcode/presence/set" => {
            let server = server.clone();
            let request: presence::SetPresenceRequest = parse_params(params)?;

            responder.respond(server.presence.set_enabled(request)?)
        }
        "_rustcode/settings/get" => {
            let request: settings::GetSettingsRequest = parse_params(params)?;

            // A client that reads settings follows a change made elsewhere.
            server
                .clients
                .subscribe(client_id, clients::Topic::Settings);
            settings::start_watching(&server.clients);
            responder.respond(settings::read(request)?)
        }
        "_rustcode/settings/set" => {
            let request: settings::SetSettingsRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::Settings);
            settings::start_watching(&server.clients);
            responder.respond(settings::write(request)?)
        }
        "_rustcode/rewind" => {
            let server = server.clone();
            let request: rewind::RewindRequest = parse_params(params)?;

            let Some((state, _)) = server.session_handle(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            let answer = {
                let mut state = state.lock().await;
                if request.first_only
                    && !crate::thread_status::has_only_first_prompt(&state.history)
                {
                    Err(agent_client_protocol::Error::invalid_params()
                        .data("the first message is no longer the entire transcript"))
                } else {
                    rewind::rewind(&mut state, request.user_index, &request.text)
                }
            };
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
        "_rustcode/session/move" => {
            let server = server.clone();
            let request: session_move::MoveSessionRequest = parse_params(params)?;

            if !valid_acp_session_id(&request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data("invalid session id"),
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
        "_rustcode/session/replay" => {
            let request: session_replay::ReplaySessionRequest = parse_params(params)?;

            match session_replay::replay_session(&request.session_id) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/export" => {
            let server = server.clone();
            let request: session_transfer::ExportSessionRequest = parse_params(params)?;

            match session_transfer::export_session(&server.external, request).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/import" => {
            let server = server.clone();
            let request: session_transfer::ImportSessionRequest = parse_params(params)?;

            match session_transfer::import_session(&server, request).await {
                Ok((answer, imported)) => {
                    if let Some(imported) = imported {
                        server.announce_thread_created(ThreadCreatedNotification {
                            meta: None,
                            session_id: imported.session_id,
                            cwd: imported.cwd,
                            title: imported.title,
                            harness: Some(imported.harness),
                            model: imported.model,
                        });
                    }
                    responder.respond(answer)
                }
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/remove" => {
            let server = server.clone();
            let request: session_transfer::RemoveSessionRequest = parse_params(params)?;

            match session_transfer::remove_session(&server, request).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/repair" => {
            let server = server.clone();
            let request: session_repair::RepairSessionRequest = parse_params(params)?;

            if !valid_acp_session_id(&request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data("invalid session id"),
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
        "_rustcode/usage/plans" => {
            let request: usage::UsagePlansRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(usage::plans(request).await)?;
                Ok(())
            })
        }
        "_rustcode/usage/shared" => {
            let request: usage::UsageSharedRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(usage::shared_plans(request).await)?;
                Ok(())
            })
        }
        "_rustcode/usage/limits" => {
            let server = server.clone();
            let request: usage::UsageLimitsRequest = parse_params(params)?;

            let Some((state, _)) = server.session_handle(&request.session_id).await else {
                // A thread on another harness has no engine session: its plan is that
                // harness's own (the Codex CLI's windows for Codex).
                let Some(harness) = usage::external_harness(&request.session_id) else {
                    return responder.respond_with_error(unknown_session(&request.session_id));
                };
                let session_id = request.session_id.clone();
                return connection.spawn(async move {
                    // A named account's own login, so the meter never
                    // shows another home's numbers.
                    let home = usage::account_home(&session_id, &harness);
                    let model = usage::external_model(&session_id);
                    responder.respond(
                        usage::harness_limits(&harness, home.as_deref(), model.as_deref()).await,
                    )?;
                    Ok(())
                });
            };
            connection.spawn(async move {
                responder.respond(usage::limits(&state).await)?;
                Ok(())
            })
        }
        "_rustcode/commands" => {
            let _request: commands::CommandsRequest = parse_params(params)?;

            connection.spawn(async move {
                match commands::catalog_update() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/signin" => {
            let request: outside::OutsideSignInRequest = parse_params(params)?;

            match outside::sign_in(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/passkey/add" => {
            let request: outside::OutsidePasskeyAddRequest = parse_params(params)?;

            match outside::passkey_add(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/passkey/remove" => {
            let request: outside::OutsidePasskeyRemoveRequest = parse_params(params)?;

            match outside::passkey_remove(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/web/sign-out" => {
            let request: outside::OutsideWebSignOutRequest = parse_params(params)?;

            match outside::web_sign_out(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/code" => {
            let request: outside::OutsideCodeRequest = parse_params(params)?;

            match outside::code(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/list" => {
            let request: outside::OutsideListRequest = parse_params(params)?;

            // A client that lists outside agents hears when one waits for it.
            server.clients.subscribe(client_id, clients::Topic::Outside);
            outside::start_publishing(&server.clients);
            connection.spawn(async move {
                match outside::list(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/create" => {
            let request: outside::OutsideCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::create(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/update" => {
            let request: outside::OutsideUpdateRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::update(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/revoke" => {
            let request: outside::OutsideRevokeRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::revoke(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/answer" => {
            let request: outside::OutsideAnswerRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::answer(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/usage" => {
            let request: data::DataUsageRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::usage(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/query" => {
            let request: data::DataQueryRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::query(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/activity" => {
            let request: data::DataActivityRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::activity(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/links/load" => {
            let _request: data::DataLinksLoadRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::links_load() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/links/save" => {
            let request: data::DataLinksSaveRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::links_save(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/receipts/get" => {
            let request: data::DataReceiptsGetRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::receipts_get(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/relocate" => {
            let server = server.clone();
            let request: project_relocate::RelocateRequest = parse_params(params)?;

            // Hold both registries through validation and writes: a closed thread
            // cannot be loaded between the check and its path being replaced.
            let sessions = server.sessions.lock().await;
            let external = server.external.lock().await;
            match project_relocate::relocate(request, |id| {
                sessions.contains_key(id) || external.get(id).is_some_and(|s| !s.is_closed())
            }) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/machine/exec" => {
            let request: machine_exec::MachineExecRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_exec::exec(request, tokio_util::sync::CancellationToken::new()).await
                {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/machine/update" => {
            let request: machine_update::MachineUpdateRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_update::handle(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/read" => {
            let request: projects::ProjectReadRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::read(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/browse" => {
            let request: projects::ProjectBrowseRequest = parse_params(params)?;

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
        }
        "_rustcode/project/create" => {
            let request: projects::ProjectCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::create(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/clone" => {
            let request: projects::ProjectCloneRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::clone(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/attachments/read" => {
            let request: attachments::AttachmentReadRequest = parse_params(params)?;

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
        }
        "_rustcode/attachments/upload" => {
            let request: attachments::AttachmentUploadRequest = parse_params(params)?;

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
        }
        "_rustcode/environment/sync" => {
            let request: machine_sync::MachineSyncRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_sync::sync(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/list" => {
            let request: loops::LoopsListRequest = parse_params(params)?;

            connection.spawn(async move {
                match {
                    let _ = request;
                    loops::list().await
                } {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/save" => {
            let request: loops::LoopsSaveRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::save(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/set_paused" => {
            let request: loops::LoopsSetPausedRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::set_paused(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/run" => {
            let request: loops::LoopsRunRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::run(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/cancel" => {
            let request: loops::LoopsCancelRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::cancel(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/delete" => {
            let request: loops::LoopsDeleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::delete(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/start_daemon" => {
            let request: loops::LoopsStartDaemonRequest = parse_params(params)?;

            connection.spawn(async move {
                match {
                    let _ = request;
                    loops::start_daemon().await
                } {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/list" => {
            let _request: scheduled::ScheduledListRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::list().await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/create" => {
            let request: scheduled::ScheduledCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::create(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/delete" => {
            let request: scheduled::ScheduledDeleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::delete(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/run" => {
            let request: scheduled::ScheduledRunRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::run(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/mirror" => {
            let request: mirror::MirrorRequest = parse_params(params)?;

            connection.spawn(async move {
                // The mirror reads and writes files; keep that off the connection's task.
                let answer = tokio::task::spawn_blocking(move || mirror::answer(request))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));
                match answer {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/config/telemetry" => {
            let _request: telemetry::TelemetryRequest = parse_params(params)?;

            connection.spawn(async move {
                match telemetry::answer() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/receipts/put" => {
            let request: data::DataReceiptsPutRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::receipts_put(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/config/telemetry/set" => {
            let request: telemetry::SetTelemetryRequest = parse_params(params)?;

            connection.spawn(async move {
                match telemetry::answer_set(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/agents" => {
            let server = server.clone();
            let request: agents::AgentsRequest = parse_params(params)?;

            let Some((state, cwd)) = server.session_handle(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            match agents::answer(&state, &cwd, &server.client, request.action).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        // A tool call to an MCP server waits for the user's approval, so it
        // runs off the dispatch loop.
        "_rustcode/mcp/app" => {
            let server = server.clone();
            let request: mcp_app::McpAppRequest = parse_params(params)?;

            let Some((state, _)) = server.session_handle(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            let builtin = harnesses::get(&server.external, &request.session_id)
                .await
                .is_none();
            let clients = server.clients.session(&request.session_id);
            let auto_approve = server.auto_approve;
            connection.spawn(async move {
                match mcp_app::handle(request, builtin, state, clients, auto_approve).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/steer" => {
            let server = server.clone();
            let request: steer::SteerRequest = parse_params(params)?;

            if let Some(external) = harnesses::get(&server.external, &request.session_id).await {
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
                        remote.note_prompt(
                            &request.session_id,
                            &request.text,
                            request
                                .meta
                                .as_ref()
                                .and_then(|v| v.get("rustcode"))
                                .and_then(|v| v.get("origin"))
                                .cloned()
                                .filter(|v| crate::message_origin::from_meta(v).is_some()),
                        );
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
                matches!(
                    block,
                    agent_client_protocol::schema::v1::ContentBlock::Image(_)
                )
            });
            if images {
                return responder.respond(serde_json::json!({ "accepted": false }));
            }
            let mut state = state.lock().await;
            let answer = steer::accept(&mut state, &request.text);
            if answer.get("accepted").and_then(serde_json::Value::as_bool) == Some(true) {
                if let Some(steer) = state.pending_steers.last_mut() {
                    steer.origin = request
                        .meta
                        .as_ref()
                        .and_then(|v| v.get("rustcode"))
                        .and_then(|v| v.get("origin"))
                        .cloned()
                        .filter(|v| crate::message_origin::from_meta(v).is_some());
                }
            }
            responder.respond(answer)
        }
        // Remote access starts a gateway, waits for a pairing answer and runs
        // launchctl: keep the dispatch loop free while each does.
        "_rustcode/remote" => {
            let server = server.clone();
            let request: remote::RemoteStatusRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.status(&request.session_id).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/start" => {
            let server = server.clone();
            let request: remote::RemoteStartRequest = parse_params(params)?;

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
                        agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {}", request.session_id)),
                    );
                };
                match server.remote.start(session, request.pair).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error.into_acp())?,
                }
                Ok(())
            })
        }
        "_rustcode/remote/pair" => {
            let server = server.clone();
            let request: remote::RemotePairRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.pair(&request.session_id).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/stop" => {
            let server = server.clone();
            let request: remote::RemoteStopRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                match server.remote.stop(&request.session_id).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/remote/answer" => {
            let server = server.clone();
            let request: remote::RemoteAnswerRequest = parse_params(params)?;

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
        "_rustcode/remote/connect_code" => {
            let request: remote::RemoteConnectCodeRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(remote::connect_code(request.cancel).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/revoke" => {
            let server = server.clone();
            let request: remote::RemoteRevokeRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.revoke(&request.device_id).await)?;
                Ok(())
            })
        }
        // Harness detection runs commands and a model listing may start
        // one, so none of these holds the dispatch loop.
        "_rustcode/harnesses" => {
            let _request: harnesses::HarnessesRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::snapshot().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/models" => {
            let request: harnesses::HarnessModelsRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::models(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/set" => {
            let request: harnesses::SetHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::set(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/remove" => {
            let request: harnesses::RemoveHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::remove(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/default" => {
            let request: harnesses::DefaultHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::set_default(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/remember" => {
            let request: harnesses::RememberHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::remember(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/add" => {
            let request: harnesses::AddHarnessAccountRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::add_account(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/remove" => {
            let server = server.clone();
            let request: harnesses::RemoveHarnessAccountRequest = parse_params(params)?;

            let external = server.external.clone();
            connection.spawn(async move {
                match harnesses::remove_account(&external, request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/check" => {
            let request: harnesses::CheckHarnessAccountsRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::check_accounts(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/session/harness" => {
            let server = server.clone();
            let request: harnesses::SwitchHarnessRequest = parse_params(params)?;

            let server = server.clone();
            let peer = server.clients.peer(client_id, &connection);
            connection.spawn(async move {
                let session_id = request.session_id.clone();
                let builtin = if harnesses::get(&server.external, &session_id)
                    .await
                    .is_some()
                {
                    None
                } else if crate::thread_status::is_running(&session_id) {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data("stop the running turn before switching harness"),
                    );
                } else {
                    close_builtin(&server, &session_id).await
                };
                match harnesses::switch(&server.external, request, builtin, &peer, &server.remote)
                    .await
                {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/onboarding/complete" => {
            let _request: harnesses::OnboardingCompleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::complete_onboarding().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities" => {
            let _request: capabilities::CapabilitiesRequest = parse_params(params)?;

            responder.respond(capabilities::snapshot_value()?)
        }
        "_rustcode/capabilities/set" => {
            let request: capabilities::SetCapabilityRequest = parse_params(params)?;

            // Starting an MCP server can take seconds; keep the dispatch
            // loop free while it does.
            connection.spawn(async move {
                match capabilities::set_capability(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/add" => {
            let request: capabilities::AddMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::add_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/remove" => {
            let request: capabilities::RemoveMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::remove_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/restart" => {
            let request: capabilities::RestartMcpServerRequest = parse_params(params)?;

            // Connecting a server again can take the same seconds a start
            // does; keep the dispatch loop free while it does.
            connection.spawn(async move {
                match capabilities::restart_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/logout" => {
            let request: capabilities::LogoutMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::logout_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/skills/add" => {
            let request: capabilities::AddSkillRequest = parse_params(params)?;

            responder.respond(capabilities::add_skill(request)?)
        }
        "_rustcode/skills/get" => {
            let request: capabilities::GetSkillRequest = parse_params(params)?;

            responder.respond(capabilities::get_skill(request)?)
        }
        // Orchestrator v2 (`docs/orchestrator.md`): worktrees, thread status
        // and checkpoints. Each runs git, so none holds the dispatch loop.
        "_rustcode/worktrees/create" => {
            let request: crate::worktrees::CreateRequest = parse_params(params)?;

            connection.spawn(async move {
                let created =
                    tokio::task::spawn_blocking(move || crate::worktrees::create(&request)).await;
                match created {
                    Ok(Ok(worktree)) => responder.respond(serde_json::json!(worktree))?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/list" => {
            let request: crate::worktrees::ListRequest = parse_params(params)?;

            connection.spawn(async move {
                let listed =
                    tokio::task::spawn_blocking(move || crate::worktrees::list(&request.repo))
                        .await;
                match listed {
                    Ok(Ok(worktrees)) => {
                        responder.respond(serde_json::json!({ "worktrees": worktrees }))?
                    }
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/remove" => {
            let request: crate::worktrees::RemoveRequest = parse_params(params)?;

            connection.spawn(async move {
                let removed = tokio::task::spawn_blocking(move || {
                    crate::worktrees::remove(&request.path, request.force)
                })
                .await;
                match removed {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/textGeneration/configure" => {
            let request: crate::text_generation::ConfigureRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::text_generation::configure(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/voice/transcribe" => {
            let request: crate::transcription::TranscribeRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::transcription::transcribe(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/textGeneration/generate" => {
            let request: crate::text_generation::GenerateRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::text_generation::generate(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/git/state" => {
            let request: crate::git_actions::StateRequest = parse_params(params)?;

            connection.spawn(async move {
                let state =
                    tokio::task::spawn_blocking(move || crate::git_actions::state(&request.path))
                        .await;
                match state {
                    Ok(Ok(state)) => responder.respond(state)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/git/run" => {
            let request: crate::git_actions::RunRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::git_actions::run(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(error.into_acp())?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/state" => {
            let request: crate::worktrees::StateRequest = parse_params(params)?;

            connection.spawn(async move {
                let states =
                    tokio::task::spawn_blocking(move || crate::worktrees::states(&request.paths))
                        .await;
                match states {
                    Ok(rows) => responder.respond(serde_json::json!({ "worktrees": rows }))?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/handoff" => {
            let request: crate::worktrees::HandoffRequest = parse_params(params)?;

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
        }
        "_rustcode/session/pin" => {
            let request: crate::thread_status::PinRequest = parse_params(params)?;

            match crate::thread_status::set_pinned(&request.session_id, request.pinned) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/permissions" => {
            let _request: permissions::PermissionsRequest = parse_params(params)?;

            // The user config is what every request's policy reads, so a
            // client lists and revokes there rather than in one session.
            let (_, _, config) = crate::config::load_config();
            responder.respond(permissions::grants_snapshot(&config))
        }
        "_rustcode/permissions/revoke" => {
            let server = server.clone();
            let request: permissions::RevokePermissionRequest = parse_params(params)?;

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
        "_rustcode/themes" => {
            let request: themes::ThemesRequest = parse_params(params)?;

            // A client that lists themes restyles when they change.
            server.clients.subscribe(client_id, clients::Topic::Themes);
            themes::start_watching(&server.clients);
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || themes::answer(request)).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/themes/set" => {
            let request: themes::SetThemeRequest = parse_params(params)?;

            server.clients.subscribe(client_id, clients::Topic::Themes);
            themes::start_watching(&server.clients);
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || themes::answer_set(request)).await {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/session/snooze" => {
            let request: crate::thread_status::SnoozeRequest = parse_params(params)?;

            match crate::thread_status::set_snoozed(&request.session_id, request.until) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/session/status" => {
            let request: crate::thread_status::SetStatusRequest = parse_params(params)?;

            match crate::thread_status::set(&request.session_id, &request.status) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/threads/search" => {
            let request: crate::thread_index::SearchRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer = tokio::task::spawn_blocking(move || {
                    crate::thread_index::answer_search(&request)
                })
                .await;
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
        }
        "_rustcode/checkpoints/list" => {
            let request: crate::checkpoints::ListRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer =
                    tokio::task::spawn_blocking(move || crate::checkpoints::answer_list(&request))
                        .await;
                match answer {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/checkpoints/diff" => {
            let request: crate::checkpoints::DiffRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer =
                    tokio::task::spawn_blocking(move || crate::checkpoints::answer_diff(&request))
                        .await;
                match answer {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/checkpoints/restore" => {
            let server = server.clone();
            let request: crate::checkpoints::RestoreRequest = parse_params(params)?;

            // Only an open built-in thread rewinds its conversation.
            let state = server
                .session_handle(&request.session_id)
                .await
                .map(|(state, _)| state);
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
        "_rustcode/server/status" => {
            let server = server.clone();
            let _request: server_status::ServerStatusRequest = parse_params(params)?;

            responder.respond(server_status::status(&server))
        }
        "_rustcode/warm" => {
            let _request: WarmRequest = parse_params(params)?;

            connection.spawn(async move {
                crate::mcp::start_deferred_servers().await;
                Ok(())
            })?;
            responder.respond(serde_json::json!({}))
        }
        // `gh` can take seconds, so neither holds the dispatch loop.
        "_rustcode/pulls" => {
            let request: PullsRequest = parse_params(params)?;

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
        }
        "_rustcode/pulls/view" => {
            let request: PullViewRequest = parse_params(params)?;

            connection.spawn(async move {
                let viewed = crate::pull_requests::view(&request.cwd, request.number).await;
                responder.respond(match viewed {
                    Ok(pull) => serde_json::json!({ "pull": pull }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/branch" => {
            let request: PullBranchRequest = parse_params(params)?;

            connection.spawn(async move {
                let prepared =
                    crate::pull_requests::prepare_branch(&request.cwd, request.number).await;
                responder.respond(match prepared {
                    Ok(branch) => serde_json::json!({ "branch": branch }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/state" => {
            let request: PullStateRequest = parse_params(params)?;

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
        }
        "_rustcode/pulls/merge/cancel" => {
            let request: PullMergeCancelRequest = parse_params(params)?;

            let cancelled = crate::pull_requests::cancel_merge(&request.merge_id);
            responder.respond(serde_json::json!({ "cancelled": cancelled }))
        }
        "_rustcode/pulls/merge" => {
            let request: PullMergeRequest = parse_params(params)?;

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
        }
        "_rustcode/worktrees/cleanup" => {
            let request: WorktreeCleanupRequest = parse_params(params)?;

            connection.spawn(async move {
                let cleaned = crate::worktrees::cleanup_resolving_rename(
                    request.path,
                    request.branch,
                    request.head_sha,
                    request.merged,
                )
                .await;
                match cleaned {
                    Ok(Ok(cleanup)) => {
                        responder.respond(serde_json::json!({ "cleanup": cleanup }))?
                    }
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/pulls/diff" => {
            let request: PullDiffRequest = parse_params(params)?;

            connection.spawn(async move {
                let diff = crate::pull_requests::diff(&request.cwd, request.number).await;
                responder.respond(match diff {
                    Ok(diff) => serde_json::to_value(diff).unwrap_or_default(),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/references" => {
            let request: PullReferencesRequest = parse_params(params)?;

            connection.spawn(async move {
                let read = crate::pull_requests::references(&request.cwd).await;
                responder.respond(match read {
                    Ok(references) => serde_json::json!({ "references": references }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/thread" => {
            let request: PullThreadRequest = parse_params(params)?;

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
        }
        "_rustcode/pulls/review" => {
            let request: PullReviewRequest = parse_params(params)?;

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
        }
        "_rustcode/pulls/watch/start" => {
            let request: PullWatchStartRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::PullWatches);
            start_pull_watching(&server.clients);
            let Some(store) = crate::pull_watch::WatchStore::default_store() else {
                return responder.respond(serde_json::json!({ "error": { "code": "failed", "message": "No config directory" } }));
            };
            let started = if request.link {
                match crate::pull_requests::watch_view_on(
                    &request.cwd,
                    &request.repo,
                    request.number,
                )
                .await
                {
                    Ok(value) => store.link(
                        &request.session_id,
                        &request.cwd,
                        &request.repo,
                        request.number,
                        &value,
                        chrono::Utc::now().timestamp(),
                    ),
                    Err(error) => Err(error.message),
                }
            } else {
                store.start(
                    &request.session_id,
                    &request.cwd,
                    request.number,
                    request.interests.unwrap_or_default(),
                    chrono::Utc::now().timestamp(),
                )
            };
            responder.respond(match started {
                Ok(watch) => serde_json::json!({ "watch": watch }),
                Err(message) => {
                    serde_json::json!({ "error": { "code": "failed", "message": message } })
                }
            })
        }
        "_rustcode/pulls/watch/stop" => {
            let request: PullWatchStopRequest = parse_params(params)?;

            let stopped = crate::pull_watch::WatchStore::default_store()
                .and_then(|store| {
                    store
                        .stop(&request.session_id, request.number, request.remove)
                        .ok()
                })
                .flatten();
            responder.respond(serde_json::json!({ "watch": stopped }))
        }
        "_rustcode/pulls/watches" => {
            let request: PullWatchesRequest = parse_params(params)?;

            // The app asks on launch: watches from before a restart
            // carry on from here.
            server
                .clients
                .subscribe(client_id, clients::Topic::PullWatches);
            start_pull_watching(&server.clients);
            if let (Some(number), Some(session_id)) = (request.take, request.session_id.as_deref())
            {
                let taken = crate::pull_watch::WatchStore::default_store()
                    .map(|store| store.take(session_id, number))
                    .transpose();
                return responder.respond(match taken {
                    Ok(Some(Some((watch, events)))) => {
                        serde_json::json!({ "watch": watch, "events": events })
                    }
                    Ok(_) => serde_json::json!({ "events": [] }),
                    Err(error) => serde_json::json!({ "error": error.to_string() }),
                });
            }
            let watches = crate::pull_watch::WatchStore::default_store()
                .map(|store| store.list(request.session_id.as_deref()))
                .unwrap_or_default();
            responder.respond(serde_json::json!({ "watches": watches }))
        }
        "_rustcode/workflows/list" => {
            let request: workflows::WorkflowListRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::Workflows);
            let clients = server.clients.clone();
            connection
                .spawn(async move { responder.respond(workflows::list(request, &clients).await) })
        }
        "_rustcode/workflows/stop" => {
            let request: workflows::WorkflowStopRequest = parse_params(params)?;

            // Stopping waits for the workers to be interrupted.
            connection.spawn(async move { responder.respond(workflows::stop(request).await) })
        }
        "_rustcode/workflows/resume" => {
            let request: workflows::WorkflowResumeRequest = parse_params(params)?;

            responder.respond(workflows::resume(request))
        }
        "_rustcode/workflows/wake" => {
            let request: workflows::WorkflowWakeRequest = parse_params(params)?;

            responder.respond(workflows::wake(&request))
        }
        // Each runs gh or the account store, so none holds the dispatch loop.
        "_rustcode/connections" => {
            let _request: connections::ConnectionsRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::snapshot_value().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/login" => {
            let _request: connections::GitHubLoginRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_login().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/wait" => {
            let _request: connections::GitHubWaitRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_wait().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/logout" => {
            let request: connections::GitHubLogoutRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_logout(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/account/refresh" => {
            let request: connections::AccountRefreshRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::account_refresh(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/account/logout" => {
            let request: connections::AccountLogoutRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::account_logout(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/cancel" => {
            let _request: connections::GitHubCancelRequest = parse_params(params)?;

            crate::connections::cancel_github_login();
            responder.respond(serde_json::Value::Null)
        }
        "session/load" => {
            let server = server.clone();
            let request: LoadSessionRequest = parse_params(params)?;

            let session_id = request.session_id.to_string();
            let peer = server.clients.peer(client_id, &connection);
            let response = acp_load_session(&server, request, &peer).await?;
            let response = crate::worktrees::finish_load(response, &session_id);
            respond_typed(responder, response)
        }
        "session/close" => {
            let server = server.clone();
            let request: CloseSessionRequest = parse_params(params)?;

            let session_id = request.session_id.to_string();
            if harnesses::close(&server.external, &session_id, &server.remote).await {
                server.clients.forget_session(&session_id);
                server.presence.session_closed(&session_id);
                return respond_typed(responder, CloseSessionResponse::new());
            }
            close_builtin(&server, &session_id).await;
            server.presence.session_closed(&session_id);
            respond_typed(responder, CloseSessionResponse::new())
        }
        "session/set_config_option" => {
            let server = server.clone();
            let request: SetSessionConfigOptionRequest = parse_params(params)?;

            let session_id = request.session_id.to_string();
            if let Some(external) = harnesses::get(&server.external, &session_id).await {
                // Setting an option may start the harness.
                return connection.spawn(async move {
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
                            respond_typed(responder, SetSessionConfigOptionResponse::new(options))?
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
            respond_typed(
                responder,
                SetSessionConfigOptionResponse::new(config_options),
            )
        }
        "_rustcode/session/stop" => {
            let server = server.clone();
            let request: stop::StopRequest = parse_params(params)?;

            let answer = stop::stop(
                &server,
                &request.session_id,
                request.attempt.as_deref(),
                "request",
            )
            .await;
            responder.respond(answer)
        }
        "session/prompt" => {
            let server = server.clone();
            let mut request: PromptRequest = parse_params(params)?;

            let session_id = request.session_id.to_string();
            // Every image is a stored one from here on: one sent by reference gets
            // its bytes back, one sent inline is kept, so the thread shows it again
            // later, here or on a paired computer.
            if let Err(error) = crate::controller::materialize_images(&mut request.prompt) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(error),
                );
            }
            let origin = request
                .meta
                .as_ref()
                .and_then(|meta| meta.get("rustcode"))
                .and_then(|v| v.get("origin"))
                .cloned()
                .filter(|v| crate::message_origin::from_meta(v).is_some());
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
                if let Some(full) = crate::harness::requested_full_access(request.meta.as_ref()) {
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
                    let update = SessionUpdate::SessionInfoUpdate(
                        agent_client_protocol::schema::v1::SessionInfoUpdate::new()
                            .title(fallback.clone()),
                    );
                    let _ = prompt::send_updates(&title_clients, &session_id, vec![update]);
                    if crate::config::load_config().2.generate_session_titles
                        && crate::text_generation::configured_for(
                            crate::config::load_session_repo(&session_id).as_deref(),
                        )
                    {
                        let title_session = session_id.clone();
                        let title_prompt = prompt_text.clone();
                        tokio::spawn(async move {
                            if let Ok(answer) = crate::text_generation::generate(
                                crate::text_generation::GenerateRequest {
                                    kind: "thread_title".into(),
                                    prompt: Some(title_prompt),
                                    path: None,
                                    files: vec![],
                                    project: crate::config::load_session_repo(&title_session),
                                },
                            )
                            .await
                            {
                                if let Some(title) = answer["text"]
                                    .as_str()
                                    .and_then(crate::app::actions::normalize_generated_title)
                                {
                                    if crate::app::actions::apply_generated_title(
                                        &title_session,
                                        Some(&fallback),
                                        &title,
                                    ) {
                                        let update = SessionUpdate::SessionInfoUpdate(agent_client_protocol::schema::v1::SessionInfoUpdate::new().title(title));
                                        let _ = prompt::send_updates(
                                            &title_clients,
                                            &title_session,
                                            vec![update],
                                        );
                                    }
                                }
                            }
                        });
                    }
                }
                server
                    .remote
                    .note_prompt(&session_id, &prompt_text, origin.clone());
                server.remote.note_prompt_applied(&session_id);
                let remote = server.remote.clone();
                let turn_session = session_id.clone();
                // The other clients that have the thread show the
                // prompt too; the one that sent it already does.
                echo_prompt(
                    &server.clients.session(&session_id),
                    client_id,
                    &request.prompt,
                    origin.clone(),
                );
                // The turn belongs to the engine, not this connection:
                // it runs on if the client goes, and only its answer
                // is lost.
                spawn_turn(async move {
                    let result = external.prompt(request.prompt).await;
                    remote.note_turn_returned(&turn_session);
                    match result {
                        Ok(stop_reason) => {
                            respond_typed(responder, PromptResponse::new(stop_reason))?
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
                    return Err(
                        agent_client_protocol::Error::invalid_params().data(message.to_string())
                    );
                }
            }
            let text = prompt_input(&request.prompt)
                .map_err(|message| agent_client_protocol::Error::invalid_params().data(message))?
                .text;
            echo_prompt(
                &server.clients.session(&session_id),
                client_id,
                &request.prompt,
                origin.clone(),
            );

            // A report only reads the session, so it answers now, beside
            // a running turn, instead of queueing at the turn gate.
            if !has_images && commands::is_report(&text) {
                let session_clients = server.clients.session(&session_id);
                connection.spawn(async move {
                    match prompt::run_report(&state, &session_id, &text, &session_clients).await {
                        Ok(stop_reason) => {
                            respond_typed(responder, PromptResponse::new(stop_reason))?
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
                        respond_typed(
                            responder,
                            PromptResponse::new(stop_reason).meta(steer::unused_meta(unused)),
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

        _ => {
            *unhandled = Some(agent_client_protocol::Dispatch::Request(
                agent_client_protocol::UntypedMessage { method, params },
                responder,
            ));
            Ok(())
        }
    }
}

#[cfg(test)]
mod dispatch_table_tests {
    use super::*;

    #[test]
    fn every_dispatched_method_matches_its_request_type() {
        assert!(
            <InitializeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "initialize"
            ),
            "method mismatch for InitializeRequest"
        );
        assert!(
            <NewSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/new"
            ),
            "method mismatch for NewSessionRequest"
        );
        assert!(
            <ListSessionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/list"
            ),
            "method mismatch for ListSessionsRequest"
        );
        assert!(
            <ThreadDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/threads/delete"
            ),
            "method mismatch for ThreadDeleteRequest"
        );
        assert!(
            <presence::PresenceRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/presence"
            ),
            "method mismatch for presence::PresenceRequest"
        );
        assert!(
            <presence::SetPresenceRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/presence/set"
            ),
            "method mismatch for presence::SetPresenceRequest"
        );
        assert!(
            <settings::GetSettingsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/settings/get"
            ),
            "method mismatch for settings::GetSettingsRequest"
        );
        assert!(
            <settings::SetSettingsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/settings/set"
            ),
            "method mismatch for settings::SetSettingsRequest"
        );
        assert!(
            <rewind::RewindRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/rewind"
            ),
            "method mismatch for rewind::RewindRequest"
        );
        assert!(<session_move::MoveSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/move"), "method mismatch for session_move::MoveSessionRequest");
        assert!(<session_replay::ReplaySessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/replay"), "method mismatch for session_replay::ReplaySessionRequest");
        assert!(<session_transfer::ExportSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/export"), "method mismatch for session_transfer::ExportSessionRequest");
        assert!(<session_transfer::ImportSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/import"), "method mismatch for session_transfer::ImportSessionRequest");
        assert!(<session_transfer::RemoveSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/remove"), "method mismatch for session_transfer::RemoveSessionRequest");
        assert!(<session_repair::RepairSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/repair"), "method mismatch for session_repair::RepairSessionRequest");
        assert!(
            <usage::UsagePlansRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/usage/plans"
            ),
            "method mismatch for usage::UsagePlansRequest"
        );
        assert!(
            <usage::UsageSharedRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/usage/shared"
            ),
            "method mismatch for usage::UsageSharedRequest"
        );
        assert!(
            <usage::UsageLimitsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/usage/limits"
            ),
            "method mismatch for usage::UsageLimitsRequest"
        );
        assert!(
            <commands::CommandsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/commands"
            ),
            "method mismatch for commands::CommandsRequest"
        );
        assert!(<outside::OutsideSignInRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/signin"), "method mismatch for outside::OutsideSignInRequest");
        assert!(<outside::OutsidePasskeyAddRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/passkey/add"), "method mismatch for outside::OutsidePasskeyAddRequest");
        assert!(<outside::OutsidePasskeyRemoveRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/passkey/remove"), "method mismatch for outside::OutsidePasskeyRemoveRequest");
        assert!(<outside::OutsideWebSignOutRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/web/sign-out"), "method mismatch for outside::OutsideWebSignOutRequest");
        assert!(
            <outside::OutsideCodeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/outside/code"
            ),
            "method mismatch for outside::OutsideCodeRequest"
        );
        assert!(
            <outside::OutsideListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/outside/list"
            ),
            "method mismatch for outside::OutsideListRequest"
        );
        assert!(<outside::OutsideCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/create"), "method mismatch for outside::OutsideCreateRequest");
        assert!(<outside::OutsideUpdateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/update"), "method mismatch for outside::OutsideUpdateRequest");
        assert!(<outside::OutsideRevokeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/revoke"), "method mismatch for outside::OutsideRevokeRequest");
        assert!(<outside::OutsideAnswerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/outside/answer"), "method mismatch for outside::OutsideAnswerRequest");
        assert!(
            <data::DataUsageRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/usage"
            ),
            "method mismatch for data::DataUsageRequest"
        );
        assert!(
            <data::DataQueryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/query"
            ),
            "method mismatch for data::DataQueryRequest"
        );
        assert!(
            <data::DataActivityRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/activity"
            ),
            "method mismatch for data::DataActivityRequest"
        );
        assert!(
            <data::DataLinksLoadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/links/load"
            ),
            "method mismatch for data::DataLinksLoadRequest"
        );
        assert!(
            <data::DataLinksSaveRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/links/save"
            ),
            "method mismatch for data::DataLinksSaveRequest"
        );
        assert!(
            <data::DataReceiptsGetRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/receipts/get"
            ),
            "method mismatch for data::DataReceiptsGetRequest"
        );
        assert!(<project_relocate::RelocateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/project/relocate"), "method mismatch for project_relocate::RelocateRequest");
        assert!(<machine_exec::MachineExecRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/machine/exec"), "method mismatch for machine_exec::MachineExecRequest");
        assert!(<machine_update::MachineUpdateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/machine/update"), "method mismatch for machine_update::MachineUpdateRequest");
        assert!(
            <projects::ProjectReadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/project/read"
            ),
            "method mismatch for projects::ProjectReadRequest"
        );
        assert!(<projects::ProjectBrowseRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/project/browse"), "method mismatch for projects::ProjectBrowseRequest");
        assert!(<projects::ProjectCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/project/create"), "method mismatch for projects::ProjectCreateRequest");
        assert!(<projects::ProjectCloneRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/project/clone"), "method mismatch for projects::ProjectCloneRequest");
        assert!(<attachments::AttachmentReadRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/attachments/read"), "method mismatch for attachments::AttachmentReadRequest");
        assert!(<attachments::AttachmentUploadRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/attachments/upload"), "method mismatch for attachments::AttachmentUploadRequest");
        assert!(<machine_sync::MachineSyncRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/environment/sync"), "method mismatch for machine_sync::MachineSyncRequest");
        assert!(
            <loops::LoopsListRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/list"
            ),
            "method mismatch for loops::LoopsListRequest"
        );
        assert!(
            <loops::LoopsSaveRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/save"
            ),
            "method mismatch for loops::LoopsSaveRequest"
        );
        assert!(
            <loops::LoopsSetPausedRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/set_paused"
            ),
            "method mismatch for loops::LoopsSetPausedRequest"
        );
        assert!(
            <loops::LoopsRunRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/run"
            ),
            "method mismatch for loops::LoopsRunRequest"
        );
        assert!(
            <loops::LoopsCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/cancel"
            ),
            "method mismatch for loops::LoopsCancelRequest"
        );
        assert!(
            <loops::LoopsDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/loops/delete"
            ),
            "method mismatch for loops::LoopsDeleteRequest"
        );
        assert!(<loops::LoopsStartDaemonRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/loops/start_daemon"), "method mismatch for loops::LoopsStartDaemonRequest");
        assert!(<scheduled::ScheduledListRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/list"), "method mismatch for scheduled::ScheduledListRequest");
        assert!(<scheduled::ScheduledCreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/create"), "method mismatch for scheduled::ScheduledCreateRequest");
        assert!(<scheduled::ScheduledDeleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/delete"), "method mismatch for scheduled::ScheduledDeleteRequest");
        assert!(<scheduled::ScheduledRunRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/scheduled/run"), "method mismatch for scheduled::ScheduledRunRequest");
        assert!(
            <mirror::MirrorRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/mirror"
            ),
            "method mismatch for mirror::MirrorRequest"
        );
        assert!(
            <telemetry::TelemetryRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/config/telemetry"
            ),
            "method mismatch for telemetry::TelemetryRequest"
        );
        assert!(
            <data::DataReceiptsPutRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/data/receipts/put"
            ),
            "method mismatch for data::DataReceiptsPutRequest"
        );
        assert!(<telemetry::SetTelemetryRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/config/telemetry/set"), "method mismatch for telemetry::SetTelemetryRequest");
        assert!(
            <agents::AgentsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/agents"
            ),
            "method mismatch for agents::AgentsRequest"
        );
        assert!(
            <mcp_app::McpAppRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/mcp/app"
            ),
            "method mismatch for mcp_app::McpAppRequest"
        );
        assert!(
            <steer::SteerRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/steer"
            ),
            "method mismatch for steer::SteerRequest"
        );
        assert!(
            <remote::RemoteStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote"
            ),
            "method mismatch for remote::RemoteStatusRequest"
        );
        assert!(
            <remote::RemoteStartRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote/start"
            ),
            "method mismatch for remote::RemoteStartRequest"
        );
        assert!(
            <remote::RemotePairRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote/pair"
            ),
            "method mismatch for remote::RemotePairRequest"
        );
        assert!(
            <remote::RemoteStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote/stop"
            ),
            "method mismatch for remote::RemoteStopRequest"
        );
        assert!(
            <remote::RemoteAnswerRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote/answer"
            ),
            "method mismatch for remote::RemoteAnswerRequest"
        );
        assert!(<remote::RemoteConnectCodeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/remote/connect_code"), "method mismatch for remote::RemoteConnectCodeRequest");
        assert!(
            <remote::RemoteRevokeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/remote/revoke"
            ),
            "method mismatch for remote::RemoteRevokeRequest"
        );
        assert!(
            <harnesses::HarnessesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/harnesses"
            ),
            "method mismatch for harnesses::HarnessesRequest"
        );
        assert!(<harnesses::HarnessModelsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/models"), "method mismatch for harnesses::HarnessModelsRequest");
        assert!(
            <harnesses::SetHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/harnesses/set"
            ),
            "method mismatch for harnesses::SetHarnessRequest"
        );
        assert!(<harnesses::RemoveHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/remove"), "method mismatch for harnesses::RemoveHarnessRequest");
        assert!(<harnesses::DefaultHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/default"), "method mismatch for harnesses::DefaultHarnessRequest");
        assert!(<harnesses::RememberHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/remember"), "method mismatch for harnesses::RememberHarnessRequest");
        assert!(<harnesses::AddHarnessAccountRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/add"), "method mismatch for harnesses::AddHarnessAccountRequest");
        assert!(<harnesses::RemoveHarnessAccountRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/remove"), "method mismatch for harnesses::RemoveHarnessAccountRequest");
        assert!(<harnesses::CheckHarnessAccountsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/harnesses/accounts/check"), "method mismatch for harnesses::CheckHarnessAccountsRequest");
        assert!(<harnesses::SwitchHarnessRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/harness"), "method mismatch for harnesses::SwitchHarnessRequest");
        assert!(<harnesses::OnboardingCompleteRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/onboarding/complete"), "method mismatch for harnesses::OnboardingCompleteRequest");
        assert!(<capabilities::CapabilitiesRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities"), "method mismatch for capabilities::CapabilitiesRequest");
        assert!(<capabilities::SetCapabilityRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/set"), "method mismatch for capabilities::SetCapabilityRequest");
        assert!(<capabilities::AddMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/add"), "method mismatch for capabilities::AddMcpServerRequest");
        assert!(<capabilities::RemoveMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/remove"), "method mismatch for capabilities::RemoveMcpServerRequest");
        assert!(<capabilities::RestartMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/restart"), "method mismatch for capabilities::RestartMcpServerRequest");
        assert!(<capabilities::LogoutMcpServerRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/capabilities/mcp/logout"), "method mismatch for capabilities::LogoutMcpServerRequest");
        assert!(<capabilities::AddSkillRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/skills/add"), "method mismatch for capabilities::AddSkillRequest");
        assert!(<capabilities::GetSkillRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/skills/get"), "method mismatch for capabilities::GetSkillRequest");
        assert!(<crate::worktrees::CreateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/worktrees/create"), "method mismatch for crate::worktrees::CreateRequest");
        assert!(<crate::worktrees::ListRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/worktrees/list"), "method mismatch for crate::worktrees::ListRequest");
        assert!(<crate::worktrees::RemoveRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/worktrees/remove"), "method mismatch for crate::worktrees::RemoveRequest");
        assert!(<crate::text_generation::ConfigureRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/textGeneration/configure"), "method mismatch for crate::text_generation::ConfigureRequest");
        assert!(<crate::transcription::TranscribeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/voice/transcribe"), "method mismatch for crate::transcription::TranscribeRequest");
        assert!(<crate::text_generation::GenerateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/textGeneration/generate"), "method mismatch for crate::text_generation::GenerateRequest");
        assert!(<crate::git_actions::StateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/git/state"), "method mismatch for crate::git_actions::StateRequest");
        assert!(<crate::git_actions::RunRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/git/run"), "method mismatch for crate::git_actions::RunRequest");
        assert!(<crate::worktrees::StateRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/worktrees/state"), "method mismatch for crate::worktrees::StateRequest");
        assert!(<crate::worktrees::HandoffRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/worktrees/handoff"), "method mismatch for crate::worktrees::HandoffRequest");
        assert!(<crate::thread_status::PinRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/pin"), "method mismatch for crate::thread_status::PinRequest");
        assert!(<permissions::PermissionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/permissions"), "method mismatch for permissions::PermissionsRequest");
        assert!(<permissions::RevokePermissionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/permissions/revoke"), "method mismatch for permissions::RevokePermissionRequest");
        assert!(
            <themes::ThemesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/themes"
            ),
            "method mismatch for themes::ThemesRequest"
        );
        assert!(
            <themes::SetThemeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/themes/set"
            ),
            "method mismatch for themes::SetThemeRequest"
        );
        assert!(<crate::thread_status::SnoozeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/snooze"), "method mismatch for crate::thread_status::SnoozeRequest");
        assert!(<crate::thread_status::SetStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/session/status"), "method mismatch for crate::thread_status::SetStatusRequest");
        assert!(<crate::thread_index::SearchRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/threads/search"), "method mismatch for crate::thread_index::SearchRequest");
        assert!(<crate::checkpoints::ListRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/checkpoints/list"), "method mismatch for crate::checkpoints::ListRequest");
        assert!(<crate::checkpoints::DiffRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/checkpoints/diff"), "method mismatch for crate::checkpoints::DiffRequest");
        assert!(<crate::checkpoints::RestoreRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/checkpoints/restore"), "method mismatch for crate::checkpoints::RestoreRequest");
        assert!(<server_status::ServerStatusRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/server/status"), "method mismatch for server_status::ServerStatusRequest");
        assert!(
            <WarmRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/warm"
            ),
            "method mismatch for WarmRequest"
        );
        assert!(
            <PullsRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls"
            ),
            "method mismatch for PullsRequest"
        );
        assert!(
            <PullViewRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/view"
            ),
            "method mismatch for PullViewRequest"
        );
        assert!(
            <PullBranchRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/branch"
            ),
            "method mismatch for PullBranchRequest"
        );
        assert!(
            <PullStateRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/state"
            ),
            "method mismatch for PullStateRequest"
        );
        assert!(
            <PullMergeCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/merge/cancel"
            ),
            "method mismatch for PullMergeCancelRequest"
        );
        assert!(
            <PullMergeRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/merge"
            ),
            "method mismatch for PullMergeRequest"
        );
        assert!(
            <WorktreeCleanupRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/worktrees/cleanup"
            ),
            "method mismatch for WorktreeCleanupRequest"
        );
        assert!(
            <PullDiffRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/diff"
            ),
            "method mismatch for PullDiffRequest"
        );
        assert!(
            <PullReferencesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/references"
            ),
            "method mismatch for PullReferencesRequest"
        );
        assert!(
            <PullThreadRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/thread"
            ),
            "method mismatch for PullThreadRequest"
        );
        assert!(
            <PullReviewRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/review"
            ),
            "method mismatch for PullReviewRequest"
        );
        assert!(
            <PullWatchStartRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/watch/start"
            ),
            "method mismatch for PullWatchStartRequest"
        );
        assert!(
            <PullWatchStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/watch/stop"
            ),
            "method mismatch for PullWatchStopRequest"
        );
        assert!(
            <PullWatchesRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/pulls/watches"
            ),
            "method mismatch for PullWatchesRequest"
        );
        assert!(<workflows::WorkflowListRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/workflows/list"), "method mismatch for workflows::WorkflowListRequest");
        assert!(<workflows::WorkflowStopRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/workflows/stop"), "method mismatch for workflows::WorkflowStopRequest");
        assert!(<workflows::WorkflowResumeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/workflows/resume"), "method mismatch for workflows::WorkflowResumeRequest");
        assert!(<workflows::WorkflowWakeRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/workflows/wake"), "method mismatch for workflows::WorkflowWakeRequest");
        assert!(<connections::ConnectionsRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections"), "method mismatch for connections::ConnectionsRequest");
        assert!(<connections::GitHubLoginRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/github/login"), "method mismatch for connections::GitHubLoginRequest");
        assert!(<connections::GitHubWaitRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/github/wait"), "method mismatch for connections::GitHubWaitRequest");
        assert!(<connections::GitHubLogoutRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/github/logout"), "method mismatch for connections::GitHubLogoutRequest");
        assert!(<connections::AccountRefreshRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/account/refresh"), "method mismatch for connections::AccountRefreshRequest");
        assert!(<connections::AccountLogoutRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/account/logout"), "method mismatch for connections::AccountLogoutRequest");
        assert!(<connections::GitHubCancelRequest as agent_client_protocol::JsonRpcMessage>::matches_method("_rustcode/connections/github/cancel"), "method mismatch for connections::GitHubCancelRequest");
        assert!(
            <LoadSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/load"
            ),
            "method mismatch for LoadSessionRequest"
        );
        assert!(
            <CloseSessionRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/close"
            ),
            "method mismatch for CloseSessionRequest"
        );
        assert!(<SetSessionConfigOptionRequest as agent_client_protocol::JsonRpcMessage>::matches_method("session/set_config_option"), "method mismatch for SetSessionConfigOptionRequest");
        assert!(
            <stop::StopRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "_rustcode/session/stop"
            ),
            "method mismatch for stop::StopRequest"
        );
        assert!(
            <PromptRequest as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/prompt"
            ),
            "method mismatch for PromptRequest"
        );
        assert!(
            <CancelNotification as agent_client_protocol::JsonRpcMessage>::matches_method(
                "session/cancel"
            )
        );
    }
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
