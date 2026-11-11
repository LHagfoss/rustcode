//! One thread running on an external harness: the harness process (started
//! on demand and restarted after it dies), the turn in flight, and the
//! transcript recorded with the thread.

use super::rpc::RpcPeer;
use super::rpc_capture::RpcRecorder;
use super::store::{self, HarnessRecord};
use super::transcript::Transcript;
use super::{HarnessKind, HarnessModel, ResolvedHarness, acp_child, codex, process, watchdog};
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOption, SessionUpdate, StopReason, TextContent, ToolCallStatus, UsageUpdate,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

/// How long a harness has to stop after `session/cancel` before it is
/// killed and the turn ends as cancelled.
pub(crate) const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How long starting a harness (process, handshake, session) may take.
/// `npx` may download its package the first time.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How often a running turn's own log is re-read for the client's context
/// gauge (`follow_live_usage`). A poll that finds an unchanged log costs one
/// `stat`.
pub(crate) const LIVE_USAGE_INTERVAL: Duration = Duration::from_secs(3);
/// How long a thread keeps its harness process with no turn, no request
/// waiting on the user and no client activity before the engine stops it:
/// `[harness_defaults].idle_timeout_minutes`, 0 turning the reaper off.
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The most harness processes the engine keeps running
/// (`[harness_defaults].max_live_harnesses`); above it the least recently used
/// are stopped first.
pub(crate) const DEFAULT_MAX_LIVE: usize = 12;
/// How often the idle reaper looks. A pass costs one `try_lock` per open
/// thread and starts no work.
const REAP_INTERVAL: Duration = Duration::from_secs(60);

/// What an adapter hands the client side.
pub(crate) enum Output {
    /// Shown to the client but not part of the reply; the history already has it.
    Marker(SessionUpdate),
    Update(SessionUpdate),
    Permission {
        request: RequestPermissionRequest,
        /// The client's whole answer: a question's free-text or several
        /// options ride in its `_meta`.
        reply: oneshot::Sender<RequestPermissionResponse>,
    },
    /// Answered once everything sent before it was handled.
    Flush(oneshot::Sender<()>),
    /// The harness's background work changed (`harness/background.rs`).
    Activity(super::background::Activity),
    /// The harness sent something that became no update of its own: any event
    /// counts as output for the silence deadline (`watchdog`).
    Heartbeat,
    /// A turn with this prompt starts: what the client is sent from here on
    /// is the turn's, until [`Output::TurnEnded`].
    TurnStarted(String),
    TurnEnded,
}

pub(crate) type OutputSender = mpsc::UnboundedSender<Output>;

pub(crate) type PermissionFuture = Pin<Box<dyn Future<Output = RequestPermissionResponse> + Send>>;

/// Where a thread's updates and permission requests go: the ACP client in
/// the engine, a channel in tests.
pub(crate) trait ClientSink: Send + Sync + 'static {
    fn update(&self, update: SessionUpdate);
    fn request_permission(&self, request: RequestPermissionRequest) -> PermissionFuture;
    /// What the thread does outside a prompt: background work, or a turn the harness started itself.
    fn activity(&self, _activity: super::background::Activity) {}
    /// A turn starts with `prompt`, after `history` recorded messages: a
    /// client that opens the thread mid-turn is sent what it missed.
    fn turn_started(&self, _history: usize, _prompt: String) {}
    /// The turn [`ClientSink::turn_started`] began has ended.
    fn turn_ended(&self) {}
}

/// The settings an adapter starts with.
#[derive(Debug, Clone, Default)]
pub(crate) struct StartOptions {
    pub resume: Option<String>,
    /// The harness account this thread runs on: a config directory of its
    /// own, so another login of the same tool is used. `None` is the harness's
    /// default sign-in, exactly as before accounts existed.
    pub account: Option<super::config::HarnessAccount>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub log_session: Option<String>,
    /// What the client that opened this thread can render.
    pub client_display: super::ClientDisplay,
    /// The `rustcode mcp-orchestrator` to add to the harness's session.
    pub orchestrator: Option<crate::orchestration::hub::Launch>,
    /// The MCP servers the user configured for this workspace, so a thread on any harness has the
    /// same servers as a RustCode thread (docs/configuration.md).
    pub mcp: Vec<crate::config::McpServerConfig>,
    /// The thread this harness runs for, so the JSON-RPC traffic it speaks can
    /// be recorded against it (`harness_rpc_messages`).
    pub thread_id: Option<String>,
    /// Where this machine's skills are and which ones are turned off, for a
    /// harness with no `use_skill` (`crate::skills::harness_skills_note`).
    /// Computed when the process is started and handed in here, so a test
    /// never reads the real home directory.
    pub skills_note: Option<String>,
}

pub(crate) enum Adapter {
    Acp(acp_child::AcpAdapter),
    Codex(codex::CodexAdapter),
}

/// The variables a harness's process is started with, and the ones it must
/// not inherit. A named account is only a config directory of its own
/// (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`): the harness keeps its own sign-in
/// there. HOME stays untouched, which would hide the macOS Keychain login, and
/// the login shell's API keys are dropped so they cannot override the chosen
/// login.
fn launch_env(
    harness: &ResolvedHarness,
    options: &StartOptions,
) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut env = match (&options.orchestrator, harness.id.as_str()) {
        (Some(launch), "pi") => match super::config::config_dir() {
            Ok(config) => super::pi_mcp::environment(launch, &config.join("run").join("pi")),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    };
    // A custom harness's own variables go first, so an account's login directory wins.
    env.splice(0..0, harness.env.iter().cloned());
    let mut remove: Vec<&'static str> = Vec::new();
    let account = options
        .account
        .as_ref()
        .filter(|account| account.harness == harness.id);
    if let Some(account) = account
        && let Some(layout) = super::config::account_layout(&harness.id)
    {
        let dir = account.resolved_dir();
        match super::config::login_env(&harness.id, &dir) {
            // The Mac's own login: no variable at all. macOS keys Claude Code's
            // sign-in's Keychain item on `CLAUDE_CONFIG_DIR`, so setting it to
            // the default directory would read as signed out, and `~/.codex`
            // is what the harness reads with none.
            None => remove.push(layout.env_var),
            Some((name, value)) => env.push((name.to_owned(), value)),
        }
        remove.extend(layout.drop_env.iter().copied());
    }
    (env, remove)
}

impl Adapter {
    pub(crate) async fn start(
        harness: &ResolvedHarness,
        cwd: &Path,
        options: StartOptions,
        out: OutputSender,
    ) -> Result<Self, String> {
        let (env, remove) = launch_env(harness, &options);
        let (child, stdout, stdin) = process::spawn(
            &harness.id,
            &harness.command,
            cwd,
            options.log_session.clone(),
            &env,
            &remove,
        )?;
        let recorder = options.thread_id.as_ref().map(|thread_id| {
            let protocol = match harness.kind {
                HarnessKind::Codex => "codex-app-server",
                _ => "acp",
            };
            RpcRecorder::new(
                thread_id.clone(),
                Some(harness.id.clone()),
                Some(protocol.to_owned()),
            )
        });
        let (peer, incoming) = RpcPeer::start(stdout, stdin, recorder);
        let started = tokio::time::timeout(
            START_TIMEOUT,
            Self::connect(harness, peer, incoming, Some(child), cwd, options, out),
        )
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "{} did not start within {} s",
                harness.name,
                START_TIMEOUT.as_secs()
            ))
        });
        if let Err(error) = &started {
            super::learn(&harness.id, |learned| learned.error = Some(error.clone()));
        }
        started
    }

    /// Drive an already connected harness; tests pass in-process pipes.
    pub(crate) async fn connect(
        harness: &ResolvedHarness,
        peer: Arc<RpcPeer>,
        incoming: mpsc::UnboundedReceiver<super::rpc::Incoming>,
        child: Option<process::HarnessProcess>,
        cwd: &Path,
        options: StartOptions,
        out: OutputSender,
    ) -> Result<Self, String> {
        match harness.kind {
            HarnessKind::Codex => {
                codex::CodexAdapter::connect(harness, peer, incoming, child, cwd, options, out)
                    .await
                    .map(Adapter::Codex)
            }
            HarnessKind::Acp => {
                acp_child::AcpAdapter::connect(harness, peer, incoming, child, cwd, options, out)
                    .await
                    .map(Adapter::Acp)
            }
            HarnessKind::Builtin => Err("the built-in harness runs in the engine".to_owned()),
        }
    }

    pub(crate) fn native_session_id(&self) -> String {
        match self {
            Adapter::Acp(adapter) => adapter.native_session_id(),
            Adapter::Codex(adapter) => adapter.native_session_id(),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        match self {
            Adapter::Acp(adapter) => adapter.is_alive(),
            Adapter::Codex(adapter) => adapter.is_alive(),
        }
    }

    pub(crate) async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<StopReason, String> {
        match self {
            Adapter::Acp(adapter) => adapter.prompt(prompt).await,
            Adapter::Codex(adapter) => adapter.prompt(prompt).await,
        }
    }

    /// Tell the harness whether its thread runs with Full access. Only Codex sandboxes its own
    /// shell and takes the policy from the client; the others have no policy to change.
    pub(crate) fn apply_access(&self, full: Option<bool>) {
        if let (Adapter::Codex(adapter), Some(full)) = (self, full) {
            adapter.set_full_access(full);
        }
    }

    pub(crate) fn cancel(&self) {
        match self {
            Adapter::Acp(adapter) => adapter.cancel(),
            Adapter::Codex(adapter) => adapter.cancel(),
        }
    }

    pub(crate) async fn steer(&self, prompt: &[ContentBlock]) -> bool {
        match self {
            Adapter::Acp(adapter) => adapter.steer(prompt).await,
            Adapter::Codex(adapter) => adapter.steer(prompt).await,
        }
    }

    pub(crate) fn config_options(&self) -> Vec<SessionConfigOption> {
        match self {
            Adapter::Acp(adapter) => adapter.config_options(),
            Adapter::Codex(adapter) => adapter.config_options(),
        }
    }

    pub(crate) async fn set_config(
        &self,
        id: &str,
        value: serde_json::Value,
    ) -> Result<Vec<SessionConfigOption>, String> {
        match self {
            Adapter::Acp(adapter) => adapter.set_config(id, value).await,
            Adapter::Codex(adapter) => adapter.set_config(id, value),
        }
    }

    /// The model and effort the harness runs with now.
    pub(crate) fn model_and_effort(&self) -> (Option<String>, Option<String>) {
        match self {
            Adapter::Acp(adapter) => adapter.model_and_effort(),
            Adapter::Codex(adapter) => adapter.model_and_effort(),
        }
    }

    /// The harness's models, asking the models a person can see for the
    /// efforts they themselves offer (`AcpChild::probed_models`). `known` is
    /// what a previous listing learned, `preferred` the model a setting or a
    /// thread uses. Only an ACP harness has to be asked; Codex lists the
    /// efforts of each model itself.
    pub(crate) async fn probed_models(
        &self,
        known: &[HarnessModel],
        preferred: &[String],
    ) -> Vec<HarnessModel> {
        match self {
            Adapter::Acp(adapter) => adapter.probed_models(known, preferred).await,
            Adapter::Codex(adapter) => adapter.models(),
        }
    }

    /// Record the efforts of the model the harness runs now, for the cache.
    /// Only an ACP harness learns them here; Codex listed them from
    /// `model/list`.
    pub(crate) fn learn_efforts(&self, harness: &str) {
        if let Adapter::Acp(adapter) = self {
            adapter.learn_efforts(harness);
        }
    }

    pub(crate) fn kill(&self) {
        match self {
            Adapter::Acp(adapter) => adapter.kill(),
            Adapter::Codex(adapter) => adapter.kill(),
        }
    }
}

/// Start `harness` briefly to list its models.
pub(crate) async fn list_models(harness: &ResolvedHarness) -> Result<Vec<HarnessModel>, String> {
    let (out, mut drain) = mpsc::unbounded_channel();
    // Nothing reads a model listing's updates; answer anything it asks.
    tokio::spawn(async move {
        while let Some(output) = drain.recv().await {
            match output {
                Output::Permission { reply, .. } => {
                    let _ = reply.send(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    ));
                }
                Output::Flush(reply) => {
                    let _ = reply.send(());
                }
                Output::Update(_) | Output::Marker(_) | Output::Activity(_) => {}
                Output::Heartbeat | Output::TurnStarted(_) | Output::TurnEnded => {}
            }
        }
    });
    let cwd = std::env::temp_dir();
    let adapter = Adapter::start(harness, &cwd, StartOptions::default(), out).await?;
    // A scratch session in the temp directory, kept for this listing alone:
    // what it switches to learn a model's levels never touches a thread.
    let learned = super::learned_for(&harness.id);
    let known = if learned.probed {
        learned.models.unwrap_or_default()
    } else {
        // A thread start knows one model's efforts for all of them.
        Vec::new()
    };
    let models = adapter
        .probed_models(&known, &preferred_models(harness))
        .await;
    adapter.kill();
    Ok(models)
}

/// The models a setting names for `harness`: the engine's default, and the
/// model its entry remembers. A listing asks them for their own efforts first.
fn preferred_models(harness: &ResolvedHarness) -> Vec<String> {
    let Ok(dir) = super::config::config_dir() else {
        return Vec::new();
    };
    let (defaults, entries) = super::config::load_in(&dir);
    [
        defaults.model,
        entries
            .into_iter()
            .find(|entry| entry.id == harness.id)
            .and_then(|entry| entry.model),
    ]
    .into_iter()
    .flatten()
    .filter(|model| !model.is_empty())
    .collect()
}

/// `model` and `effort` options from a model list, without a running
/// harness.
pub(crate) fn offline_config_options(
    models: &[HarnessModel],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<SessionConfigOption> {
    use agent_client_protocol::schema::v1::{
        SessionConfigOptionCategory, SessionConfigSelectOption,
    };
    let mut choices = models
        .iter()
        .map(|model| SessionConfigSelectOption::new(model.id.clone(), model.name.clone()))
        .collect::<Vec<_>>();
    let current = model
        .map(str::to_owned)
        .or_else(|| models.first().map(|model| model.id.clone()));
    let Some(current) = current else {
        return Vec::new();
    };
    if !models.iter().any(|model| model.id == current) {
        choices.push(SessionConfigSelectOption::new(
            current.clone(),
            current.clone(),
        ));
    }
    let mut options = vec![
        SessionConfigOption::select("model", "Model", current.clone(), choices)
            .category(SessionConfigOptionCategory::Model),
    ];
    if let Some(listed) = models.iter().find(|model| model.id == current)
        && !listed.efforts.is_empty()
    {
        let chosen = effort
            .filter(|effort| listed.efforts.iter().any(|known| known == effort))
            .map(str::to_owned)
            .or_else(|| listed.default_effort.clone())
            .unwrap_or_else(|| listed.efforts[0].clone());
        let choices = listed
            .efforts
            .iter()
            .map(|effort| SessionConfigSelectOption::new(effort.clone(), effort.clone()))
            .collect::<Vec<_>>();
        options.push(
            SessionConfigOption::select("effort", "Effort", chosen, choices)
                .category(SessionConfigOptionCategory::ThoughtLevel),
        );
    }
    options
}

/// The text a recorded user message keeps of a prompt.
pub(crate) fn prompt_text(prompt: &[ContentBlock]) -> String {
    let mut parts = Vec::new();
    for block in prompt {
        match block {
            ContentBlock::Text(text) => parts.push(text.text.clone()),
            ContentBlock::Image(_) => parts.push("[image]".to_owned()),
            ContentBlock::ResourceLink(link) => parts.push(format!("@{}", link.uri)),
            ContentBlock::Resource(resource) => {
                if let Ok(value) = serde_json::to_value(&resource.resource)
                    && let Some(uri) = value.get("uri").and_then(serde_json::Value::as_str)
                {
                    parts.push(format!("@{uri}"));
                }
            }
            _ => {}
        }
    }
    parts.join("\n")
}

/// How long a silent turn waited, in the words a person reads: "10 minutes",
/// "1 minute", "45 seconds".
fn waited(timeout: Duration) -> String {
    let seconds = timeout.as_secs().max(1);
    if seconds < 60 {
        return if seconds == 1 {
            "1 second".to_owned()
        } else {
            format!("{seconds} seconds")
        };
    }
    let minutes = seconds / 60;
    if minutes == 1 {
        "1 minute".to_owned()
    } else {
        format!("{minutes} minutes")
    }
}

/// The config directory of the named account a thread runs on, when it has one:
/// where its session log lies (`crate::harness_usage`) and the `CODEX_HOME` a
/// plan read asks the Codex CLI about (`crate::acp::usage`).
pub(crate) fn account_config_dir(record: &HarnessRecord, harness: &str) -> Option<PathBuf> {
    let account = record.account.as_deref()?;
    let dir = super::config::config_dir().ok()?;
    super::config::load_accounts_in(&dir)
        .into_iter()
        .find(|known| known.id == account && known.harness == harness)
        .map(|known| known.resolved_dir())
}

/// The context window a thread's last `usage_update` reported, if any.
fn live_window(record: &HarnessRecord) -> Option<u64> {
    record
        .usage
        .as_ref()
        .and_then(|usage| usage.get("size"))
        .and_then(serde_json::Value::as_u64)
        .filter(|size| *size > 0)
}

/// A turn's poll of its harness's own log (`follow_live_usage`), stopped by
/// dropping it: when the turn returns, when it is cancelled, or on an unwind.
struct LivePoll(Arc<AtomicBool>);

impl LivePoll {
    /// The flag the poll reads, for a test that drives a turn's end.
    #[cfg(test)]
    fn handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

impl Drop for LivePoll {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The `usage_update` a poll of a running turn publishes: the context its
/// latest request took and the window the thread last reported. No token
/// totals ride in `_meta`: the turn's row is still what the harness's log says
/// when the turn ends (`record_turn_in_data_store`), never a poll of it.
fn live_usage_update(used: u64, size: u64) -> SessionUpdate {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        serde_json::json!({
            "kind": "usage",
            "version": crate::acp::usage::USAGE_META_VERSION,
            "estimated": false,
        }),
    );
    SessionUpdate::UsageUpdate(UsageUpdate::new(used, size).meta(meta))
}

/// Count one output of the harness toward the turn's silence deadline
/// (`watchdog`): its updates, including a thought or a usage report, are
/// output.
fn note_output(progress: &watchdog::Progress, output: &Output) {
    match output {
        Output::Update(update) | Output::Marker(update) => progress.observe(update),
        Output::Activity(activity) => progress.activity(activity),
        Output::Heartbeat => progress.touch(),
        // The guard a permission request takes counts it; a flush and a
        // turn's bounds are the engine's own, not the harness speaking.
        Output::Permission { .. }
        | Output::Flush(_)
        | Output::TurnStarted(_)
        | Output::TurnEnded => {}
    }
}

/// The clock the idle reaper reads: when the thread last did anything, whether
/// a turn is in flight, and whether its process was stopped to be resumed
/// later. Shared with the output pump, which sees every update, permission
/// request and answer.
#[derive(Debug)]
struct IdleState {
    last: StdMutex<tokio::time::Instant>,
    turn: AtomicBool,
    sleeping: AtomicBool,
}

/// One turn, as the idle reaper sees it: dropping it ends the turn, whichever
/// way the turn leaves (a cancel, a failure, or an unwind).
struct TurnMark(Arc<IdleState>);

impl Drop for TurnMark {
    fn drop(&mut self) {
        self.0.turn.store(false, Ordering::Release);
        self.0.touch();
    }
}

impl IdleState {
    fn share() -> Arc<Self> {
        Arc::new(Self {
            last: StdMutex::new(tokio::time::Instant::now()),
            turn: AtomicBool::new(false),
            sleeping: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, tokio::time::Instant> {
        self.last.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// The thread did something: a prompt, a load, an update, an answer.
    fn touch(&self) {
        *self.lock() = tokio::time::Instant::now();
    }

    /// When the thread last did anything; the reaper measures its idle time
    /// from here.
    fn last(&self) -> tokio::time::Instant {
        *self.lock()
    }

    fn begin_turn(self: &Arc<Self>) -> TurnMark {
        self.turn.store(true, Ordering::Release);
        self.touch();
        TurnMark(Arc::clone(self))
    }

    /// Back from a reaped process; whether it was asleep.
    fn wake(&self) -> bool {
        self.sleeping.swap(false, Ordering::Acquire)
    }
}

/// Every external thread this engine has open, so the idle reaper can find one
/// no client is using. Weak: a closed thread drops out by itself.
fn open_threads() -> &'static StdMutex<Vec<Weak<ExternalSession>>> {
    static OPEN: OnceLock<StdMutex<Vec<Weak<ExternalSession>>>> = OnceLock::new();
    OPEN.get_or_init(Default::default)
}

/// Note a thread for the idle reaper, and start the reaper the first time a
/// thread is opened.
fn register(session: &Arc<ExternalSession>) {
    let mut open = open_threads()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    open.retain(|known| known.strong_count() > 0);
    open.push(Arc::downgrade(session));
    drop(open);
    start_reaper();
}

/// The reaper itself: one pass a minute for as long as the engine runs. It is
/// started once and reads the user's settings on every pass, so turning it off
/// and on again needs no restart.
fn start_reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    // Threads are opened from async request handlers; a thread opened outside a
    // runtime (a test) simply has no reaper.
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async {
        loop {
            tokio::time::sleep(REAP_INTERVAL).await;
            reap_pass();
        }
    });
}

/// One pass with the user's settings (`[harness_defaults]`, `config.rs`).
fn reap_pass() {
    let timeout = super::config::idle_timeout();
    let cap = super::config::max_live_harnesses();
    if timeout.is_none() && cap.is_none() {
        return;
    }
    let sessions = {
        let mut open = open_threads()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        open.retain(|known| known.strong_count() > 0);
        open.iter().filter_map(Weak::upgrade).collect::<Vec<_>>()
    };
    reap_once(&sessions, timeout, cap);
}

/// Stop the processes that have been idle for `timeout`, then — while more than
/// `cap` run — the least recently used ones. Answers the threads whose process
/// was stopped, so a caller (or a test) can say which.
pub(crate) fn reap_once(
    sessions: &[Arc<ExternalSession>],
    timeout: Option<Duration>,
    cap: Option<usize>,
) -> Vec<String> {
    let now = tokio::time::Instant::now();
    let mut idle = sessions
        .iter()
        .filter_map(|session| session.idle_since().map(|since| (since, session)))
        .collect::<Vec<_>>();
    // Least recently used first, so the cap takes the oldest of them.
    idle.sort_by_key(|(since, _)| *since);
    let mut running = sessions.iter().filter(|session| session.running()).count();
    let mut reaped = Vec::new();
    for (since, session) in idle {
        let quiet = now.saturating_duration_since(since);
        let too_idle = timeout.is_some_and(|timeout| quiet >= timeout);
        let over_cap = cap.is_some_and(|cap| running > cap);
        if !too_idle && !over_cap {
            continue;
        }
        let reason = if too_idle { "idle" } else { "cap" };
        if session.reap(reason, quiet) {
            running = running.saturating_sub(1);
            reaped.push(session.id.clone());
        }
    }
    reaped
}

pub(crate) struct ExternalSession {
    pub id: String,
    pub harness: ResolvedHarness,
    pub cwd: PathBuf,
    out: OutputSender,
    adapter: Mutex<Option<Arc<Adapter>>>,
    turn: Mutex<()>,
    cancel: StdMutex<Option<Arc<Notify>>>,
    record: StdMutex<HarnessRecord>,
    transcript: Arc<StdMutex<Transcript>>,
    closed: AtomicBool,
    persist: bool,
    client_display: StdMutex<super::ClientDisplay>,
    /// Whether the client runs this thread with Full access, as its last prompt said
    /// (`harness::requested_full_access`). `None` until a prompt says: the harness keeps the
    /// policy it started with.
    full_access: StdMutex<Option<bool>>,
    orchestrator: StdMutex<Option<crate::orchestration::hub::Launch>>,
    /// The turn in flight's progress, for the silence deadline.
    progress: Arc<watchdog::Progress>,
    /// How long a turn may send nothing with nothing running. `None` reads
    /// `[harness_defaults].silence_timeout_minutes`; `Some(Duration::ZERO)`
    /// turns the deadline off (tests, which cannot rely on the user's config).
    silence: StdMutex<Option<Duration>>,
    /// When the thread last did anything, whether a turn runs, and whether the
    /// idle reaper stopped its process (`IdleState`).
    idle: Arc<IdleState>,
}

impl ExternalSession {
    /// A thread on `harness`. `history` is what it recorded before, for a
    /// loaded thread. `persist` is false in tests that need no store.
    pub(crate) fn new(
        id: String,
        harness: ResolvedHarness,
        record: HarnessRecord,
        history: Vec<crate::app::ChatMessage>,
        sink: Arc<dyn ClientSink>,
        persist: bool,
    ) -> Arc<Self> {
        let (out, mut outputs) = mpsc::unbounded_channel::<Output>();
        let transcript = Arc::new(StdMutex::new(Transcript::new(history)));
        let progress = watchdog::Progress::new();
        let idle = IdleState::share();
        let thread_id = id.clone();
        let harness_id = harness.id.clone();
        {
            let transcript = Arc::clone(&transcript);
            let progress = Arc::clone(&progress);
            let idle = Arc::clone(&idle);
            tokio::spawn(async move {
                while let Some(output) = outputs.recv().await {
                    // Anything the thread does renews the idle timer: an update,
                    // a question, a heartbeat (`reap_once`).
                    idle.touch();
                    note_output(&progress, &output);
                    match output {
                        Output::Update(update) => {
                            transcript
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .observe(&update);
                            if let Some(row) =
                                Self::harness_compaction(&update, &thread_id, &harness_id)
                            {
                                crate::data_store::record_compaction(&row);
                            }
                            sink.update(update);
                        }
                        Output::Marker(update) => sink.update(update),
                        Output::Permission { request, reply } => {
                            let waiting = progress.waiting();
                            let asked = sink.request_permission(request);
                            let answered = Arc::clone(&idle);
                            tokio::spawn(async move {
                                let _ = reply.send(asked.await);
                                // The user answered: the thread is in use again.
                                answered.touch();
                                drop(waiting);
                            });
                        }
                        Output::Flush(reply) => {
                            let _ = reply.send(());
                        }
                        Output::Activity(activity) => sink.activity(activity),
                        Output::Heartbeat => {}
                        Output::TurnStarted(prompt) => {
                            // Counted here, after every update before it.
                            let history = transcript
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .history
                                .len();
                            sink.turn_started(history, prompt);
                        }
                        Output::TurnEnded => sink.turn_ended(),
                    }
                }
            });
        }
        let session = Arc::new(Self {
            id,
            cwd: record.cwd.clone(),
            harness,
            out,
            adapter: Mutex::new(None),
            turn: Mutex::new(()),
            cancel: StdMutex::new(None),
            record: StdMutex::new(record),
            transcript,
            closed: AtomicBool::new(false),
            persist,
            client_display: StdMutex::new(super::ClientDisplay::default()),
            full_access: StdMutex::new(None),
            orchestrator: StdMutex::new(None),
            progress,
            silence: StdMutex::new(None),
            idle,
        });
        // Only a thread the engine keeps on disk is reaped: tests and scratch
        // threads have no record to resume.
        if persist {
            register(&session);
        }
        session
    }

    /// Whether the harness gets the orchestration tools.
    pub(crate) fn orchestrated(&self) -> bool {
        self.orchestrator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// Give the harness the orchestration tools from its next start.
    pub(crate) fn set_orchestrator(&self, launch: Option<crate::orchestration::hub::Launch>) {
        *self
            .orchestrator
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = launch;
    }

    pub(crate) fn set_client_display(&self, display: super::ClientDisplay) {
        *self
            .client_display
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = display;
    }

    /// Record the access level the client runs this thread at, from its next turn on. The access
    /// level is the client's (`Ask`, `Auto-edit`, `Full access`); a harness that sandboxes its own
    /// shell needs to know it, or Full access still runs in the sandbox.
    pub(crate) fn set_full_access(&self, full: bool) {
        *self
            .full_access
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(full);
    }

    pub(crate) fn record(&self) -> HarnessRecord {
        self.record
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn save_record(&self, edit: impl FnOnce(&mut HarnessRecord)) {
        let record = {
            let mut record = self
                .record
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            edit(&mut record);
            record.updated_at = chrono::Utc::now().to_rfc3339();
            record.clone()
        };
        if self.persist {
            store::save(&self.id, &record);
        }
    }

    fn save_history(&self) {
        let (history, usage) = {
            let transcript = self
                .transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (transcript.history.clone(), transcript.usage.clone())
        };
        if usage.is_some() && usage != self.record().usage {
            self.save_record(|record| record.usage = usage);
        }
        if !self.persist || history.is_empty() {
            return;
        }
        crate::config::save_session_history(&self.id, &history);
        // Written now rather than queued, so `session/list` right after the
        // turn already shows it.
        crate::config::flush_history();
        crate::config::save_session_title_if_absent(&self.id, &history);
        if let Some(title) = crate::config::load_session_title(&self.id) {
            self.save_record(|record| record.title = Some(title));
        }
    }

    /// Write the history now: a thread just switched to this harness.
    pub(crate) fn save_now(&self) {
        self.save_history();
    }

    /// Wire an adapter in directly (tests).
    #[cfg(test)]
    pub(crate) async fn attach(&self, adapter: Adapter) {
        *self.adapter.lock().await = Some(Arc::new(adapter));
    }

    #[cfg(test)]
    pub(crate) fn output(&self) -> OutputSender {
        self.out.clone()
    }

    /// How many permission requests and questions wait on the user (tests).
    #[cfg(test)]
    pub(crate) fn waiting_requests(&self) -> usize {
        self.progress.waiting_count()
    }

    /// When the harness last counted as output (tests).
    #[cfg(test)]
    pub(crate) fn last_output_at(&self) -> Option<tokio::time::Instant> {
        self.progress.last_output_at()
    }

    /// Whether a harness is attached (tests).
    #[cfg(test)]
    pub(crate) async fn has_adapter(&self) -> bool {
        self.adapter.lock().await.is_some()
    }

    /// The transcript recorded so far. A share of this thread starts its
    /// mirror from it, so nothing recorded before the share is missing.
    pub(crate) fn recorded_history(&self) -> Vec<crate::app::ChatMessage> {
        self.transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .history
            .clone()
    }

    /// The transcript recorded so far (tests).
    #[cfg(test)]
    pub(crate) fn history(&self) -> Vec<crate::app::ChatMessage> {
        self.transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .history
            .clone()
    }

    /// The running adapter, started (or restarted, resuming the harness's
    /// own session) when there is none.
    pub(crate) async fn ensure_started(&self) -> Result<Arc<Adapter>, String> {
        if self.closed.load(Ordering::Acquire) {
            return Err("the thread is closed".to_owned());
        }
        let mut slot = self.adapter.lock().await;
        if let Some(adapter) = slot.as_ref()
            && adapter.is_alive()
        {
            return Ok(Arc::clone(adapter));
        }
        if let Some(dead) = slot.take() {
            dead.kill();
        }
        let record = self.record();
        let mut options = self.resume_options(&record)?;
        // The workspace's own servers, with the user config under them: the same list the built-in
        // harness runs, so Settings › MCP is one place for every harness.
        let mcp = crate::config::get_config_dir()
            .map(|dir| crate::config::workspace_mcp_servers_in(&dir, &self.cwd))
            .unwrap_or_default()
            .into_iter()
            .filter(|server| server.enabled)
            .collect();
        // Signed in to in RustCode (Settings › MCP, `rustcode mcp login`): the harness gets the token.
        options.mcp = crate::mcp::with_saved_logins(mcp).await;
        let adapter = Adapter::start(&self.harness, &self.cwd, options, self.out.clone()).await?;
        let native = adapter.native_session_id();
        let (model, effort) = adapter.model_and_effort();
        self.save_record(|record| {
            record.native_session_id = Some(native);
            record.model = model.or(record.model.take());
            record.effort = effort.or(record.effort.take());
        });
        let adapter = Arc::new(adapter);
        // A fresh process: nothing the old one left open still runs.
        self.progress.reset();
        self.idle.touch();
        if self.idle.wake() {
            // Woken from the idle reaper: the thread resumes the harness's own
            // session exactly as it was, so nothing went wrong while it slept.
            crate::logger::operational_event(
                "harness.resumed",
                serde_json::json!({
                    "session_id": self.id,
                    "harness": self.harness.id,
                    "native_session_id": self.record().native_session_id,
                }),
            );
        }
        *slot = Some(Arc::clone(&adapter));
        Ok(adapter)
    }

    /// What starting (or restarting) this thread's harness is asked for: the
    /// harness's own session to resume, the account it ran on, the model and
    /// effort it ran with (`harness.json`). Nothing is started here, and the
    /// record is the only source, so a process the idle reaper stopped resumes
    /// exactly where the thread left off.
    fn resume_options(&self, record: &HarnessRecord) -> Result<StartOptions, String> {
        // The record names the account; a removed one is an error, never a
        // quiet fall back to the default login.
        let account = match record.account.as_deref() {
            Some(id) => {
                let dir = super::config::config_dir()?;
                let accounts = super::config::load_accounts_in(&dir);
                Some(super::config::find_account(
                    &accounts,
                    &self.harness.id,
                    id,
                )?)
            }
            None => None,
        };
        let orchestrator = self
            .orchestrator
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let client_display = *self
            .client_display
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Ok(StartOptions {
            resume: record.native_session_id.clone(),
            account,
            model: record.model.clone(),
            effort: record.effort.clone(),
            log_session: self.persist.then(|| self.id.clone()),
            client_display,
            orchestrator,
            // The caller adds the workspace's servers; a test wants none.
            mcp: Vec::new(),
            thread_id: Some(self.id.clone()),
            // Read when the process starts, so a thread the idle reaper stopped
            // picks up a toggle made in the meantime; a running process keeps
            // the note it got (`docs/harnesses.md`).
            skills_note: crate::skills::harness_skills_note(&self.harness.id),
        })
    }

    /// Whether a harness process is running for this thread. A start in
    /// progress holds the slot, and is counted as running: the reaper never
    /// races one.
    pub(crate) fn running(&self) -> bool {
        match self.adapter.try_lock() {
            Ok(slot) => slot.as_ref().is_some_and(|adapter| adapter.is_alive()),
            Err(_) => true,
        }
    }

    /// Since when nothing happened on this thread, when the idle reaper may stop
    /// its process: no turn and no request waiting on the user, no background
    /// work, and a process there is to stop.
    ///
    /// A tool call the last turn left open is not counted: the turn ended, so
    /// nothing of it runs any more, and a harness's own work reports itself as
    /// background activity instead.
    fn idle_since(&self) -> Option<tokio::time::Instant> {
        if self.closed.load(Ordering::Acquire) || self.idle.turn.load(Ordering::Acquire) {
            return None;
        }
        if self.progress.awaiting_user() || self.progress.background_running() {
            return None;
        }
        // Only a thread the next prompt can resume: without the harness's own
        // session id, stopping the process would lose the conversation.
        if self
            .record()
            .native_session_id
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return None;
        }
        self.running().then(|| self.idle.last())
    }

    /// Whether the idle reaper stopped this thread's process and the next
    /// prompt starts a new one (`ensure_started`).
    #[cfg(test)]
    pub(crate) fn sleeping(&self) -> bool {
        self.idle.sleeping.load(Ordering::Acquire)
    }

    /// Stop this thread's harness process the way closing the thread does, and
    /// keep everything the next prompt needs to resume the harness's own
    /// session: `harness.json` still names its native session id, account,
    /// model and effort. The thread is asleep, not crashed — nothing is sent to
    /// the client and no turn fails.
    fn reap(&self, reason: &str, quiet: Duration) -> bool {
        // A running turn holds `turn` for as long as it runs, so this can never
        // cut one off; a turn that starts now simply gets a new process.
        let Ok(_turn) = self.turn.try_lock() else {
            return false;
        };
        if self.closed.load(Ordering::Acquire) || self.idle.turn.load(Ordering::Acquire) {
            return false;
        }
        if self.progress.awaiting_user() || self.progress.background_running() {
            return false;
        }
        let Some(adapter) = self
            .adapter
            .try_lock()
            .ok()
            .and_then(|mut slot| slot.take())
        else {
            return false;
        };
        // A process that had already gone needs no reaping; the next prompt
        // restarts it anyway.
        let alive = adapter.is_alive();
        adapter.kill();
        self.progress.reset();
        if !alive {
            return false;
        }
        self.idle.sleeping.store(true, Ordering::Release);
        crate::logger::operational_event(
            "harness.reaped",
            serde_json::json!({
                "session_id": self.id,
                "harness": self.harness.id,
                "native_session_id": self.record().native_session_id,
                "reason": reason,
                "idle_ms": quiet.as_millis() as u64,
            }),
        );
        crate::dbg_log_for_session!(
            self.id,
            "harness {} idle for {}; process stopped, the next message resumes its session",
            self.harness.id,
            waited(quiet)
        );
        true
    }

    /// The running harness's options, or for a thread whose harness has not
    /// started yet the recorded model and effort among the models the
    /// harness listed before.
    pub(crate) async fn config_options(&self) -> Vec<SessionConfigOption> {
        if let Some(adapter) = self.adapter.lock().await.as_ref() {
            return adapter.config_options();
        }
        let record = self.record();
        offline_config_options(
            &super::learned_for(&self.harness.id)
                .models
                .unwrap_or_default(),
            record.model.as_deref(),
            record.effort.as_deref(),
        )
    }

    pub(crate) async fn set_config(
        &self,
        id: &str,
        value: serde_json::Value,
    ) -> Result<Vec<SessionConfigOption>, String> {
        let adapter = self.ensure_started().await?;
        let options = adapter.set_config(id, value).await?;
        // Another model answers with its own thought levels: keep them, so the
        // picker knows them the next time without a listing (`docs/harnesses.md`).
        if id == "model" {
            adapter.learn_efforts(&self.harness.id);
        }
        let (model, effort) = adapter.model_and_effort();
        let before = {
            let record = self.record();
            (record.model, record.effort)
        };
        let divider = super::switch::note(
            &mut self
                .transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .history,
            &before,
            &(model.clone(), effort.clone()),
        );
        self.save_record(|record| {
            record.model = model;
            record.effort = effort;
        });
        if let Some(divider) = divider {
            self.save_history();
            let _ = self.out.send(Output::Marker(divider));
        }
        Ok(options)
    }

    pub(crate) async fn steer(&self, prompt: &[ContentBlock]) -> bool {
        let adapter = self.adapter.lock().await.clone();
        let Some(adapter) = adapter else {
            return false;
        };
        let accepted = adapter.steer(prompt).await;
        if accepted {
            self.transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_user(&prompt_text(prompt));
        }
        accepted
    }

    /// Wait until every update sent so far reached the client.
    async fn flush(&self) {
        let (sender, receiver) = oneshot::channel();
        if self.out.send(Output::Flush(sender)).is_ok() {
            let _ = receiver.await;
        }
    }

    /// Run one turn.
    ///
    /// Taken on the `Arc` so the turn can follow itself in the client's
    /// context gauge for as long as it runs (`follow_live_usage`).
    pub(crate) async fn prompt(
        self: &Arc<Self>,
        prompt: Vec<ContentBlock>,
    ) -> Result<StopReason, String> {
        let _turn = self.turn.lock().await;
        // Seen by the idle reaper: no process goes while a turn runs.
        let _mark = self.idle.begin_turn();
        // Bounds the turn for a client that opens the thread mid-turn,
        // however this function returns.
        let _ = self.out.send(Output::TurnStarted(prompt_text(&prompt)));
        struct TurnBound(OutputSender);
        impl Drop for TurnBound {
            fn drop(&mut self) {
                let _ = self.0.send(Output::TurnEnded);
            }
        }
        let _bound = TurnBound(self.out.clone());
        let _status = crate::thread_status::turn(&self.id);
        if self.persist {
            crate::checkpoints::before_turn(&self.id, &self.cwd, &prompt_text(&prompt), None).await;
        }
        let cancel = Arc::new(Notify::new());
        *self
            .cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Arc::clone(&cancel));
        let started_at = chrono::Utc::now();
        let history_from = self
            .transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .history
            .len();
        // A harness that keeps its token counts in its own log says nothing
        // about them until the turn ends, so the gauge is fed from the log.
        // Dropped when this function returns, whichever way the turn leaves.
        let _poll = self.spawn_live_usage();
        let result = self.run_turn(prompt, &cancel).await;
        *self
            .cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        self.flush().await;
        self.transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .end_reply();
        self.save_history();
        self.record_turn_in_data_store(started_at, history_from, &result)
            .await;
        result
    }

    /// Follow a running turn in the client's context gauge.
    ///
    /// A harness whose token counts live only in its own log
    /// (`harness_usage::log_based`) reports nothing over ACP until the turn
    /// ends, so the context ring would sit on the previous turn's numbers for
    /// the whole turn. The log is polled instead; dropping the `LivePoll`
    /// stops it.
    fn spawn_live_usage(self: &Arc<Self>) -> LivePoll {
        let running = Arc::new(AtomicBool::new(false));
        let poll = LivePoll(Arc::clone(&running));
        if !crate::harness_usage::log_based(&self.harness.id) {
            return poll;
        }
        running.store(true, Ordering::Release);
        let session = Arc::clone(self);
        tokio::spawn(async move { session.follow_live_usage(&running).await });
        poll
    }

    /// Poll the harness's own log for as long as the turn runs, and publish
    /// what the latest request took when it changes.
    async fn follow_live_usage(self: Arc<Self>, running: &AtomicBool) {
        let harness = self.harness.id.clone();
        // The log lies in the account's own config directory when the thread
        // runs on a named login.
        let account_dir = account_config_dir(&self.record(), &harness);
        let cwd = self.cwd.clone();
        let mut stamp: Option<crate::harness_usage::LogStamp> = None;
        let mut published: Option<u64> = None;
        while running.load(Ordering::Acquire) && !self.closed.load(Ordering::Acquire) {
            tokio::time::sleep(LIVE_USAGE_INTERVAL).await;
            if !running.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
                break;
            }
            let record = self.record();
            let Some(native) = record.native_session_id.clone() else {
                continue;
            };
            // Without a window there is nothing to draw a gauge against: the
            // size is what the harness sent with its last turn-end usage.
            let Some(size) = live_window(&record) else {
                continue;
            };
            let (harness, cwd, account_dir) = (harness.clone(), cwd.clone(), account_dir.clone());
            let read = tokio::task::spawn_blocking(move || {
                let paths = crate::harness_usage::session_logs(
                    &harness,
                    Some(&native),
                    Some(cwd.as_path()),
                    account_dir.as_deref(),
                );
                crate::harness_usage::poll_logs(&harness, &paths, stamp)
            })
            .await
            .unwrap_or_default();
            stamp = read.stamp;
            let Some(used) = read.used.filter(|used| *used > 0) else {
                continue;
            };
            if published == Some(used) {
                continue;
            }
            published = Some(used);
            let _ = self.out.send(Output::Update(live_usage_update(used, size)));
        }
    }

    /// A compaction an ACP harness reported. Claude Code's adapter presents one as
    /// the synthetic tool call `Compact conversation` (`_meta.claudeCode.toolName
    /// = "compact"`), with the trigger, tokens and duration in `rawOutput` on its
    /// terminal update. RustCode does not advertise the newer `compaction_update`
    /// capability, so this is the shape a Claude Code thread sends. `None` for any
    /// other update, and for a compaction still running: only the terminal state
    /// is a row.
    fn harness_compaction(
        update: &SessionUpdate,
        thread_id: &str,
        harness: &str,
    ) -> Option<crate::data_store::CompactionRow> {
        let (id, status, facts, meta, title) = match update {
            SessionUpdate::ToolCall(call) => (
                call.tool_call_id.0.as_ref(),
                Some(call.status),
                call.raw_output.as_ref(),
                call.meta.as_ref(),
                Some(call.title.as_str()),
            ),
            SessionUpdate::ToolCallUpdate(update) => (
                update.tool_call_id.0.as_ref(),
                update.fields.status,
                update.fields.raw_output.as_ref(),
                update.meta.as_ref(),
                update.fields.title.as_deref(),
            ),
            _ => return None,
        };
        let completed = matches!(
            status,
            Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
        );
        if !completed {
            return None;
        }
        let named_compaction = meta
            .and_then(|meta| meta.get("claudeCode"))
            .and_then(|claude| claude.get("toolName"))
            .and_then(serde_json::Value::as_str)
            == Some("compact");
        if !named_compaction && title != Some("Compact conversation") {
            return None;
        }
        let number = |name: &str| {
            facts
                .and_then(|facts| facts.get(name))
                .and_then(serde_json::Value::as_i64)
        };
        let trigger = match facts
            .and_then(|facts| facts.get("trigger"))
            .and_then(serde_json::Value::as_str)
        {
            Some("manual") => "manual",
            Some("idle") => "cache_expiry",
            // Claude Code's own automatic compaction.
            _ => "harness_auto",
        };
        Some(crate::data_store::CompactionRow {
            thread_id: thread_id.to_owned(),
            turn_id: None,
            harness: Some(harness.to_owned()),
            model: None,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            duration_ms: number("durationMs"),
            trigger: trigger.to_owned(),
            mode: Some("harness_summary".to_owned()),
            tokens_before: number("preTokens"),
            tokens_after: number("postTokens"),
            messages_before: None,
            messages_after: None,
            idle_secs_before: None,
            count: None,
            success: status != Some(ToolCallStatus::Failed),
            error_kind: (status == Some(ToolCallStatus::Failed))
                .then(|| "harness_failed".to_owned()),
            external_id: Some(id.to_owned()),
            source: Some(harness.to_owned()),
        })
    }

    /// One row per turn in `data.sqlite` (`crate::data_store`): what the
    /// session spent, its outcome and its tool calls. Metadata only; a failure
    /// is logged there and never fails the turn.
    ///
    /// The harness reports cumulative session usage, so the row keeps the
    /// growth over the last total the same session series reported. When the
    /// `usage_update` carries no token totals (the context gauge is not one),
    /// the harness's own log is read for them, off the async runtime.
    async fn record_turn_in_data_store(
        &self,
        started_at: chrono::DateTime<chrono::Utc>,
        history_from: usize,
        result: &Result<StopReason, String>,
    ) {
        if !self.persist {
            return;
        }
        let (usage, tool_calls) = {
            let transcript = self
                .transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let usage = transcript.usage.clone();
            let calls = transcript
                .history
                .get(history_from.min(transcript.history.len())..)
                .unwrap_or_default()
                .iter()
                .filter_map(|message| {
                    let recorded = message.tool_result.as_ref()?;
                    if recorded.pending {
                        return None;
                    }
                    Some(crate::data_store::ToolCallRow {
                        tool_name: recorded.tool_name.clone(),
                        success: Some(recorded.success),
                        exit_code: recorded.exit_code.map(i64::from),
                        at: (!message.timestamp.is_empty()).then(|| message.timestamp.clone()),
                        started_at: None,
                        duration_ms: None,
                        output_bytes: Some(message.content.len() as i64),
                        truncated: Some(recorded.truncated || recorded.payload_truncated),
                        mcp_server: None,
                        command_name: recorded
                            .command
                            .as_deref()
                            .and_then(|command| command.split_whitespace().next())
                            .map(str::to_owned),
                        error_kind: recorded.error_kind.clone(),
                    })
                })
                .collect::<Vec<_>>();
            (usage, calls)
        };
        let record = self.record();
        // The totals below are one harness session's, so the series they belong
        // to is that session (and the account it ran as): a restart, a switch
        // or another account starts a series whose totals are not comparable
        // with the previous one's.
        let series = record
            .native_session_id
            .as_deref()
            .filter(|native| !native.trim().is_empty())
            .map(|native| match record.account.as_deref() {
                Some(account) => format!("{account}@{native}"),
                None => native.to_owned(),
            });
        let mut cumulative = crate::data_store::SessionUsage {
            tokens: usage
                .as_ref()
                .and_then(crate::data_store::totals)
                .unwrap_or_default(),
            cost_usd: usage.as_ref().and_then(crate::data_store::cost_usd),
            at: None,
        };
        if cumulative.tokens.is_empty() {
            let harness = self.harness.id.clone();
            let native = record.native_session_id.clone();
            let cwd = self.cwd.clone();
            let account_dir = account_config_dir(&record, &harness);
            let scanned = tokio::task::spawn_blocking(move || {
                crate::harness_usage::session_usage(
                    &harness,
                    native.as_deref(),
                    Some(cwd.as_path()),
                    account_dir.as_deref(),
                )
            })
            .await
            .ok()
            .flatten();
            if let Some(logged) = scanned {
                cumulative.tokens = logged.tokens;
                if cumulative.cost_usd.is_none() {
                    cumulative.cost_usd = logged.cost_usd;
                }
                cumulative.at = logged.at;
            }
        }
        let finished_at = chrono::Utc::now();
        let context_tokens = usage
            .as_ref()
            .and_then(|usage| usage.get("used"))
            .and_then(serde_json::Value::as_u64)
            .map(|used| used as i64);
        let context_window = usage
            .as_ref()
            .and_then(|usage| usage.get("size"))
            .and_then(serde_json::Value::as_u64)
            .filter(|size| *size > 0)
            .map(|size| size as i64);
        let stop_reason = match result {
            Ok(StopReason::EndTurn) => "end_turn",
            Ok(StopReason::Cancelled) => "cancelled",
            Ok(StopReason::MaxTokens) => "max_tokens",
            Ok(StopReason::MaxTurnRequests) => "max_turn_requests",
            Ok(StopReason::Refusal) => "refused",
            Ok(_) => "completed",
            Err(_) => "failed",
        };
        crate::data_store::record_cumulative_turn(
            &crate::data_store::TurnRow {
                thread_id: self.id.clone(),
                started_at: started_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                finished_at: Some(finished_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                harness: Some(self.harness.id.clone()),
                model: record.model.clone(),
                input_tokens: None,
                output_tokens: None,
                cached_tokens: None,
                cache_write_tokens: None,
                cost_usd: None,
                status: match result {
                    Ok(StopReason::EndTurn) => "completed".to_owned(),
                    Ok(StopReason::Cancelled) => "cancelled".to_owned(),
                    Ok(StopReason::MaxTokens) => "max_tokens".to_owned(),
                    Ok(StopReason::MaxTurnRequests) => "max_turn_requests".to_owned(),
                    Ok(StopReason::Refusal) => "refused".to_owned(),
                    Ok(_) => "completed".to_owned(),
                    Err(_) => "failed".to_owned(),
                },
                duration_ms: Some((finished_at - started_at).num_milliseconds().max(0)),
                time_to_first_token_ms: None,
                model_time_ms: None,
                tool_time_ms: None,
                context_tokens,
                context_window,
                stop_reason: Some(stop_reason.to_owned()),
                tool_calls,
            },
            cumulative,
            series.as_deref(),
        );
    }

    async fn run_turn(
        &self,
        prompt: Vec<ContentBlock>,
        cancel: &Notify,
    ) -> Result<StopReason, String> {
        let adapter = self.ensure_started().await?;
        // The first prompt after a switch carries the conversation so far.
        let handoff = self.record().handoff_from.and_then(|from| {
            let transcript = self
                .transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            super::handoff::context(&transcript.history, &from, &self.harness.id)
        });
        self.transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_user(&prompt_text(&prompt));
        self.save_history();
        let mut prompt = prompt;
        let handed = handoff.is_some();
        if let Some(context) = handoff {
            prompt.insert(0, ContentBlock::Text(TextContent::new(context)));
        }
        // Nothing a finished turn left running still counts against this one.
        self.progress.begin_turn();
        let silence = self.silence_timeout();
        adapter.apply_access(
            *self
                .full_access
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let turn = adapter.prompt(prompt);
        tokio::pin!(turn);
        let silent = async {
            match silence {
                Some(timeout) => self.progress.silence(timeout).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(silent);
        // The turn is polled first, so its request is on the wire before a
        // cancel that is already waiting goes out.
        tokio::select! {
            biased;
            result = &mut turn => {
                if !adapter.is_alive() {
                    self.forget_adapter(&adapter).await;
                }
                if handed && result.is_ok() {
                    self.save_record(|record| record.handoff_from = None);
                }
                result
            }
            () = cancel.notified() => {
                adapter.cancel();
                match tokio::time::timeout(CANCEL_GRACE, &mut turn).await {
                    // A harness that answers a cancel with an error has still
                    // stopped.
                    Ok(Ok(stop)) => Ok(stop),
                    Ok(Err(_)) => Ok(StopReason::Cancelled),
                    Err(_) => {
                        crate::dbg_log!(
                            "harness {} ignored cancel for {} s; killing it",
                            self.harness.id,
                            CANCEL_GRACE.as_secs()
                        );
                        adapter.kill();
                        self.forget_adapter(&adapter).await;
                        Ok(StopReason::Cancelled)
                    }
                }
            }
            () = &mut silent => {
                // Unreachable when the deadline is off: that future never resolves.
                let timeout = silence.unwrap_or(watchdog::DEFAULT_TIMEOUT);
                crate::logger::operational_event(
                    "harness.turn_silent",
                    serde_json::json!({
                        "session_id": self.id,
                        "harness": self.harness.id,
                        "native_session_id": self.record().native_session_id,
                        "timeout_ms": timeout.as_millis() as u64,
                    }),
                );
                crate::dbg_log!(
                    "harness {} sent nothing for {}; killing it",
                    self.harness.id,
                    waited(timeout)
                );
                // The same teardown a harness that ignores a cancel gets: the
                // process goes, and the next message starts a new one from the
                // recorded native session.
                adapter.kill();
                self.forget_adapter(&adapter).await;
                Err(format!(
                    "No output from {} for {}. Send a message to try again.",
                    self.harness.name,
                    waited(timeout)
                ))
            }
        }
    }

    /// The turn's silence deadline: what a test picked, else
    /// `[harness_defaults].silence_timeout_minutes`.
    fn silence_timeout(&self) -> Option<Duration> {
        match *self
            .silence
            .lock()
            .unwrap_or_else(|error| error.into_inner())
        {
            Some(picked) => (picked > Duration::ZERO).then_some(picked),
            None => super::config::silence_timeout(),
        }
    }

    /// Pick the silence deadline instead of reading the user's config (tests).
    #[cfg(test)]
    pub(crate) fn set_silence_timeout(&self, timeout: Option<Duration>) {
        *self
            .silence
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(timeout.unwrap_or(Duration::ZERO));
    }

    async fn forget_adapter(&self, adapter: &Arc<Adapter>) {
        let mut slot = self.adapter.lock().await;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, adapter))
        {
            *slot = None;
            // Its tools, permissions and background work went with it.
            self.progress.reset();
        }
    }

    /// Ask the running turn to stop.
    pub(crate) fn cancel(&self) {
        if let Some(cancel) = self
            .cancel
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            cancel.notify_one();
        }
    }

    /// Whether this thread was closed: the app's registry dropped it, so a
    /// share of it must end.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Stop the harness for good: `session/close`, connection close.
    pub(crate) async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancel();
        if let Some(adapter) = self.adapter.lock().await.take() {
            adapter.kill();
        }
    }

    pub(crate) fn kill_now(&self) {
        self.closed.store(true, Ordering::Release);
        if let Ok(slot) = self.adapter.try_lock()
            && let Some(adapter) = slot.as_ref()
        {
            adapter.kill();
        }
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    use crate::harness::config::HarnessAccount;

    fn account(harness: &str, dir: &str) -> HarnessAccount {
        HarnessAccount {
            id: "paral-2".to_owned(),
            harness: harness.to_owned(),
            name: Some("Paral 2".to_owned()),
            config_dir: dir.to_owned(),
        }
    }

    fn session_update(value: serde_json::Value) -> SessionUpdate {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_claude_compaction_tool_call_becomes_a_compaction_row() {
        let update = session_update(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "compact-1",
            "status": "completed",
            "rawOutput": {
                "trigger": "manual",
                "preTokens": 120_000,
                "postTokens": 18_000,
                "durationMs": 2_400,
            },
            "_meta": {"claudeCode": {"toolName": "compact"}},
        }));
        let row = ExternalSession::harness_compaction(&update, "thread-1", "claude-code")
            .expect("a terminal compaction is recorded");
        assert_eq!(row.thread_id, "thread-1");
        assert_eq!(row.harness.as_deref(), Some("claude-code"));
        assert_eq!(row.trigger, "manual");
        assert_eq!(row.mode.as_deref(), Some("harness_summary"));
        assert_eq!(row.tokens_before, Some(120_000));
        assert_eq!(row.tokens_after, Some(18_000));
        assert_eq!(row.duration_ms, Some(2_400));
        assert_eq!(row.external_id.as_deref(), Some("compact-1"));
        assert!(row.success);

        // Claude's automatic pass and the adapter's idle compaction keep
        // their distinct triggers.
        let automatic = session_update(serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "compact-2",
            "title": "Compact conversation",
            "status": "completed",
            "rawOutput": {"trigger": "automatic"},
        }));
        let row = ExternalSession::harness_compaction(&automatic, "thread-1", "claude-code")
            .expect("the title is enough");
        assert_eq!(row.trigger, "harness_auto");
        let idle = session_update(serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "compact-3",
            "status": "completed",
            "rawOutput": {"trigger": "idle"},
            "_meta": {"claudeCode": {"toolName": "compact"}},
        }));
        assert_eq!(
            ExternalSession::harness_compaction(&idle, "thread-1", "claude-code")
                .unwrap()
                .trigger,
            "cache_expiry"
        );
    }

    #[test]
    fn a_compaction_still_running_or_another_tool_is_not_a_row() {
        let running = session_update(serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "compact-1",
            "title": "Compact conversation",
            "status": "in_progress",
        }));
        assert!(ExternalSession::harness_compaction(&running, "thread-1", "claude-code").is_none());
        let other = session_update(serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-1",
            "title": "Read file",
            "status": "completed",
        }));
        assert!(ExternalSession::harness_compaction(&other, "thread-1", "claude-code").is_none());
        let text = session_update(serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "done"},
        }));
        assert!(ExternalSession::harness_compaction(&text, "thread-1", "claude-code").is_none());
    }

    #[test]
    fn an_account_launch_sets_its_directory_and_drops_ambient_keys() {
        let harness = test_support::harness("claude-code", HarnessKind::Acp);
        let options = StartOptions {
            account: Some(account("claude-code", "/tmp/claude-paral-2")),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                "/tmp/claude-paral-2".to_owned()
            )]
        );
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(remove.contains(&name), "{name} must not reach the child");
        }
        assert!(
            remove.iter().all(|name| *name != "HOME"),
            "HOME stays: the macOS Keychain login needs it"
        );
    }

    #[test]
    fn the_default_login_account_sets_no_config_dir() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let harness = test_support::harness("claude-code", HarnessKind::Acp);
        let options = StartOptions {
            account: Some(account(
                "claude-code",
                &home.join(".claude").to_string_lossy(),
            )),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert!(
            env.is_empty(),
            "the Mac's own login keeps CLAUDE_CONFIG_DIR unset"
        );
        assert!(
            remove.contains(&"CLAUDE_CONFIG_DIR"),
            "an inherited CLAUDE_CONFIG_DIR must be dropped"
        );
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            assert!(remove.contains(&name), "{name} must not reach the child");
        }
    }

    #[test]
    fn a_codex_account_launch_sets_its_home_and_drops_ambient_keys() {
        let harness = test_support::harness("codex", HarnessKind::Codex);
        let options = StartOptions {
            account: Some(account("codex", "/tmp/codex-paral")),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert_eq!(
            env,
            vec![("CODEX_HOME".to_owned(), "/tmp/codex-paral".to_owned())]
        );
        for name in ["OPENAI_API_KEY", "CODEX_API_KEY"] {
            assert!(remove.contains(&name), "{name} must not reach the child");
        }
        assert!(
            !remove.contains(&"CLAUDE_CONFIG_DIR"),
            "a Codex account says nothing about Claude Code's variable"
        );
        assert!(
            remove.iter().all(|name| *name != "HOME"),
            "HOME stays: a login may still read it"
        );
    }

    #[test]
    fn a_codex_account_on_the_default_home_sets_no_variable() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let harness = test_support::harness("codex", HarnessKind::Codex);
        let options = StartOptions {
            account: Some(account("codex", &home.join(".codex").to_string_lossy())),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert!(env.is_empty(), "~/.codex runs with no CODEX_HOME at all");
        assert!(
            remove.contains(&"CODEX_HOME"),
            "an inherited CODEX_HOME must be dropped"
        );
        for name in ["OPENAI_API_KEY", "CODEX_API_KEY"] {
            assert!(remove.contains(&name), "{name} must not reach the child");
        }
    }

    #[test]
    fn a_claude_account_never_sets_the_codex_home() {
        let harness = test_support::harness("claude-code", HarnessKind::Acp);
        let options = StartOptions {
            account: Some(account("claude-code", "/tmp/claude-paral-2")),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert!(
            env.iter().all(|(name, _)| name != "CODEX_HOME"),
            "one harness's variable never points at another's login"
        );
        assert!(!remove.contains(&"CODEX_HOME") && !remove.contains(&"OPENAI_API_KEY"));
    }

    #[test]
    fn no_account_launch_keeps_todays_environment() {
        let harness = test_support::harness("claude-code", HarnessKind::Acp);
        let (env, remove) = launch_env(&harness, &StartOptions::default());
        assert!(env.is_empty(), "no env change without an account");
        assert!(remove.is_empty(), "nothing removed without an account");
    }

    #[test]
    fn an_account_of_another_harness_is_ignored() {
        let harness = test_support::harness("codex", HarnessKind::Codex);
        let options = StartOptions {
            account: Some(account("claude-code", "/tmp/claude-paral-2")),
            ..Default::default()
        };
        let (env, remove) = launch_env(&harness, &options);
        assert!(env.is_empty());
        assert!(remove.is_empty());
        assert_eq!(harness.id, "codex");
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::harness::rpc::Incoming;

    /// Two connected peers: ours, and the fake harness's.
    pub(crate) fn pipe() -> (
        (Arc<RpcPeer>, mpsc::UnboundedReceiver<Incoming>),
        (Arc<RpcPeer>, mpsc::UnboundedReceiver<Incoming>),
    ) {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (our_read, our_write) = tokio::io::split(ours);
        let (their_read, their_write) = tokio::io::split(theirs);
        (
            RpcPeer::start(our_read, our_write, None),
            RpcPeer::start(their_read, their_write, None),
        )
    }

    pub(crate) fn harness(id: &str, kind: HarnessKind) -> ResolvedHarness {
        ResolvedHarness {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
            command: vec![id.to_owned()],
            env: Vec::new(),
            enabled: true,
            known: None,
        }
    }

    pub(crate) struct Thread {
        pub session: Arc<ExternalSession>,
        pub updates: mpsc::UnboundedReceiver<SessionUpdate>,
        pub permissions: mpsc::UnboundedReceiver<RequestPermissionRequest>,
    }

    /// A thread on `harness` with no store, its permission requests answered
    /// with `choice`.
    pub(crate) fn thread(harness: &ResolvedHarness, choice: Option<&str>) -> Thread {
        with_record(
            harness,
            HarnessRecord::new(&harness.id, std::env::temp_dir()),
            choice,
        )
    }

    /// A thread on `harness` with its own record (an account, a model) and no store.
    pub(crate) fn with_record(
        harness: &ResolvedHarness,
        record: HarnessRecord,
        choice: Option<&str>,
    ) -> Thread {
        with_record_named(harness, "test-thread", record, choice)
    }

    /// The same, under a session id of its own: a test that reads the thread's
    /// status must not see a test running beside it on the shared id.
    pub(crate) fn with_record_named(
        harness: &ResolvedHarness,
        session_id: &str,
        record: HarnessRecord,
        choice: Option<&str>,
    ) -> Thread {
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (permissions_tx, permissions) = mpsc::unbounded_channel();
        let sink = Arc::new(ChannelSink {
            updates: updates_tx,
            permissions: permissions_tx,
            choice: choice.map(str::to_owned),
        });
        let session = ExternalSession::new(
            session_id.to_owned(),
            harness.clone(),
            record,
            Vec::new(),
            sink,
            false,
        );
        Thread {
            session,
            updates,
            permissions,
        }
    }

    /// A thread whose client is `wrap`ped around a sink that never answers a
    /// permission request by itself: only whoever the wrapper adds can. Used
    /// where a test answers from somewhere other than the client, as a paired
    /// device does.
    pub(crate) fn thread_with_pending_client(
        harness: &ResolvedHarness,
        session_id: &str,
        wrap: impl FnOnce(Arc<dyn ClientSink>) -> Arc<dyn ClientSink>,
    ) -> Thread {
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (permissions_tx, permissions) = mpsc::unbounded_channel();
        let recording: Arc<dyn ClientSink> = Arc::new(PendingSink {
            updates: updates_tx,
            permissions: permissions_tx,
        });
        let sink = wrap(recording);
        let record = HarnessRecord::new(&harness.id, std::env::temp_dir());
        let session = ExternalSession::new(
            session_id.to_owned(),
            harness.clone(),
            record,
            Vec::new(),
            sink,
            false,
        );
        Thread {
            session,
            updates,
            permissions,
        }
    }

    /// A sink that records what it is sent and never answers a permission
    /// request on its own.
    pub(crate) struct PendingSink {
        pub updates: mpsc::UnboundedSender<SessionUpdate>,
        pub permissions: mpsc::UnboundedSender<RequestPermissionRequest>,
    }

    impl ClientSink for PendingSink {
        fn update(&self, update: SessionUpdate) {
            let _ = self.updates.send(update);
        }

        fn request_permission(&self, request: RequestPermissionRequest) -> PermissionFuture {
            let _ = self.permissions.send(request);
            Box::pin(std::future::pending())
        }
    }

    /// The `sessionUpdate` kinds received so far, with each update's JSON.
    pub(crate) fn drain(
        updates: &mut mpsc::UnboundedReceiver<SessionUpdate>,
    ) -> Vec<serde_json::Value> {
        let mut seen = Vec::new();
        while let Ok(update) = updates.try_recv() {
            seen.push(serde_json::to_value(update).unwrap());
        }
        seen
    }

    pub(crate) fn kinds(updates: &[serde_json::Value]) -> Vec<String> {
        updates
            .iter()
            .map(|update| {
                update["sessionUpdate"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    }

    /// A sink that records updates and answers every permission request
    /// with the option `choice` names.
    pub(crate) struct ChannelSink {
        pub updates: mpsc::UnboundedSender<SessionUpdate>,
        pub permissions: mpsc::UnboundedSender<RequestPermissionRequest>,
        pub choice: Option<String>,
    }

    impl ClientSink for ChannelSink {
        fn update(&self, update: SessionUpdate) {
            let _ = self.updates.send(update);
        }

        fn request_permission(&self, request: RequestPermissionRequest) -> PermissionFuture {
            let _ = self.permissions.send(request);
            let choice = self.choice.clone();
            Box::pin(async move {
                RequestPermissionResponse::new(match choice {
                    Some(choice) => RequestPermissionOutcome::Selected(
                        agent_client_protocol::schema::v1::SelectedPermissionOutcome::new(choice),
                    ),
                    None => RequestPermissionOutcome::Cancelled,
                })
            })
        }
    }
}

#[cfg(test)]
mod removed_account_tests {
    use super::*;
    use crate::harness::store::HarnessRecord;

    #[tokio::test]
    async fn a_prompt_on_a_removed_account_errors_without_starting() {
        let harness = super::test_support::harness("claude-code", HarnessKind::Acp);
        let mut record = HarnessRecord::new("claude-code", std::env::temp_dir());
        record.account = Some("removed-login".to_owned());
        let thread = super::test_support::with_record(&harness, record, None);
        let error = thread
            .session
            .prompt(vec![ContentBlock::Text(TextContent::new(
                "hello".to_owned(),
            ))])
            .await
            .unwrap_err();
        assert!(
            error.contains("unknown claude-code account 'removed-login'"),
            "{error}"
        );
        assert!(
            thread.session.history().is_empty(),
            "the failed start recorded nothing and started no process"
        );
        assert!(
            thread.session.record().account.as_deref() == Some("removed-login"),
            "the record keeps the account, never a quiet fall back"
        );
    }
}

#[cfg(test)]
#[path = "reap_tests.rs"]
mod reap_tests;

#[cfg(test)]
mod live_usage_tests {
    use super::*;

    #[test]
    fn a_live_usage_update_carries_the_context_and_no_totals() {
        let value = serde_json::to_value(live_usage_update(12_345, 200_000)).unwrap();
        assert_eq!(value["sessionUpdate"], "usage_update");
        assert_eq!(value["used"], 12_345);
        assert_eq!(value["size"], 200_000);
        let meta = &value["_meta"]["rustcode"];
        assert_eq!(meta["kind"], "usage");
        assert_eq!(meta["version"], crate::acp::usage::USAGE_META_VERSION);
        assert_eq!(meta["estimated"], false);
        assert_eq!(
            crate::data_store::totals(&value),
            None,
            "no token totals ride along: the turn row still reads them from the log"
        );
    }

    #[tokio::test]
    async fn only_a_harness_whose_usage_lives_in_its_log_is_polled() {
        let log_based = crate::harness_usage::log_based;
        assert!(log_based("pi") && log_based("claude-code"));
        assert!(!log_based("codex"), "codex reports its usage over ACP");
        let harness = super::test_support::harness("codex", HarnessKind::Codex);
        let thread = super::test_support::thread(&harness, None);
        let poll = thread.session.spawn_live_usage();
        assert!(
            !poll.handle().load(Ordering::Acquire),
            "nothing to poll, so nothing is spawned"
        );
    }

    #[tokio::test]
    async fn the_poll_stops_when_the_turn_ends() {
        let harness = super::test_support::harness("claude-code", HarnessKind::Acp);
        let thread = super::test_support::thread(&harness, None);
        let poll = thread.session.spawn_live_usage();
        let flag = poll.handle();
        assert!(flag.load(Ordering::Acquire), "a log-based turn is polled");
        // What a finished, cancelled or failed turn does to it.
        drop(poll);
        assert!(
            !flag.load(Ordering::Acquire),
            "the poll stops when the turn leaves"
        );
    }
}
