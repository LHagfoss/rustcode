//! One thread running on an external harness: the harness process (started
//! on demand and restarted after it dies), the turn in flight, and the
//! transcript recorded with the thread.

use super::rpc::RpcPeer;
use super::store::{self, HarnessRecord};
use super::transcript::Transcript;
use super::{HarnessKind, HarnessModel, ResolvedHarness, acp_child, codex, process};
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOption, SessionUpdate, StopReason, TextContent,
};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};

/// How long a harness has to stop after `session/cancel` before it is
/// killed and the turn ends as cancelled.
pub(crate) const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// How long starting a harness (process, handshake, session) may take.
/// `npx` may download its package the first time.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(120);

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
    /// The `rustcode mcp-orchestrator` to add to the harness's session.
    pub orchestrator: Option<crate::orchestration::hub::Launch>,
    /// The MCP servers the user configured for this workspace, so a thread on any harness has the
    /// same servers as a RustCode thread (docs/configuration.md).
    pub mcp: Vec<crate::config::McpServerConfig>,
}

pub(crate) enum Adapter {
    Acp(acp_child::AcpAdapter),
    Codex(codex::CodexAdapter),
}

/// The variables a harness's process is started with, and the ones it must
/// not inherit. A named account is only a `CLAUDE_CONFIG_DIR`: the harness
/// keeps its own sign-in there. HOME stays untouched, which would hide the
/// macOS Keychain login, and the login shell's API keys are dropped so they
/// cannot override the chosen login.
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
    let mut remove: Vec<&'static str> = Vec::new();
    let account = options
        .account
        .as_ref()
        .filter(|account| account.harness == harness.id);
    if let Some(account) = account {
        env.push((
            "CLAUDE_CONFIG_DIR".to_owned(),
            account.resolved_dir().to_string_lossy().into_owned(),
        ));
        remove.extend([
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ]);
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
        let (peer, incoming) = RpcPeer::start(stdout, stdin);
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

    pub(crate) fn models(&self) -> Vec<HarnessModel> {
        match self {
            Adapter::Acp(adapter) => adapter.models(),
            Adapter::Codex(adapter) => adapter.models(),
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
            }
        }
    });
    let cwd = std::env::temp_dir();
    let adapter = Adapter::start(harness, &cwd, StartOptions::default(), out).await?;
    let models = adapter.models();
    adapter.kill();
    Ok(models)
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
    orchestrator: StdMutex<Option<crate::orchestration::hub::Launch>>,
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
        {
            let transcript = Arc::clone(&transcript);
            tokio::spawn(async move {
                while let Some(output) = outputs.recv().await {
                    match output {
                        Output::Update(update) => {
                            transcript
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .observe(&update);
                            sink.update(update);
                        }
                        Output::Marker(update) => sink.update(update),
                        Output::Permission { request, reply } => {
                            let asked = sink.request_permission(request);
                            tokio::spawn(async move {
                                let _ = reply.send(asked.await);
                            });
                        }
                        Output::Flush(reply) => {
                            let _ = reply.send(());
                        }
                        Output::Activity(activity) => sink.activity(activity),
                    }
                }
            });
        }
        Arc::new(Self {
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
            orchestrator: StdMutex::new(None),
        })
    }

    /// Give the harness the orchestration tools from its next start.
    pub(crate) fn set_orchestrator(&self, launch: Option<crate::orchestration::hub::Launch>) {
        *self
            .orchestrator
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = launch;
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
        // The workspace's own servers, with the user config under them: the same list the built-in
        // harness runs, so Settings › MCP is one place for every harness.
        let mcp = crate::config::get_config_dir()
            .map(|dir| crate::config::workspace_mcp_servers_in(&dir, &self.cwd))
            .unwrap_or_default()
            .into_iter()
            .filter(|server| server.enabled)
            .collect();
        // Signed in to in RustCode (Settings › MCP, `rustcode mcp login`): the harness gets the token.
        let mcp = crate::mcp::with_saved_logins(mcp).await;
        let adapter = Adapter::start(
            &self.harness,
            &self.cwd,
            StartOptions {
                resume: record.native_session_id.clone(),
                account,
                model: record.model.clone(),
                effort: record.effort.clone(),
                log_session: self.persist.then(|| self.id.clone()),
                orchestrator,
                mcp,
            },
            self.out.clone(),
        )
        .await?;
        let native = adapter.native_session_id();
        let (model, effort) = adapter.model_and_effort();
        self.save_record(|record| {
            record.native_session_id = Some(native);
            record.model = model.or(record.model.take());
            record.effort = effort.or(record.effort.take());
        });
        let adapter = Arc::new(adapter);
        *slot = Some(Arc::clone(&adapter));
        Ok(adapter)
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
    pub(crate) async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<StopReason, String> {
        let _turn = self.turn.lock().await;
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

    /// One row per turn in `data.sqlite` (`crate::data_store`): what the
    /// session spent, its outcome and its tool calls. Metadata only; a failure
    /// is logged there and never fails the turn.
    ///
    /// The harness reports cumulative session usage, so the row keeps the
    /// growth over what this thread already has for the harness. When the
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
                    })
                })
                .collect::<Vec<_>>();
            (usage, calls)
        };
        let record = self.record();
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
            // The log lies in the account's own config directory when the
            // thread runs on a named login.
            let account_dir = record.account.as_deref().and_then(|account| {
                let dir = super::config::config_dir().ok()?;
                let accounts = super::config::load_accounts_in(&dir);
                accounts
                    .into_iter()
                    .find(|known| known.id == account && known.harness == self.harness.id)
                    .map(|known| known.resolved_dir())
            });
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
                tool_calls,
            },
            cumulative,
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
        let turn = adapter.prompt(prompt);
        tokio::pin!(turn);
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
        }
    }

    async fn forget_adapter(&self, adapter: &Arc<Adapter>) {
        let mut slot = self.adapter.lock().await;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, adapter))
        {
            *slot = None;
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
            RpcPeer::start(our_read, our_write),
            RpcPeer::start(their_read, their_write),
        )
    }

    pub(crate) fn harness(id: &str, kind: HarnessKind) -> ResolvedHarness {
        ResolvedHarness {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
            command: vec![id.to_owned()],
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
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (permissions_tx, permissions) = mpsc::unbounded_channel();
        let sink = Arc::new(ChannelSink {
            updates: updates_tx,
            permissions: permissions_tx,
            choice: choice.map(str::to_owned),
        });
        let session = ExternalSession::new(
            "test-thread".to_owned(),
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
