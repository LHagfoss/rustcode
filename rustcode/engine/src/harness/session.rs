//! One thread running on an external harness: the harness process (started
//! on demand and restarted after it dies), the turn in flight, and the
//! transcript recorded with the thread.

use super::rpc::RpcPeer;
use super::store::{self, HarnessRecord};
use super::transcript::Transcript;
use super::{HarnessKind, HarnessModel, ResolvedHarness, acp_child, codex, process};
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOption, SessionUpdate, StopReason,
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

impl Adapter {
    pub(crate) async fn start(
        harness: &ResolvedHarness,
        cwd: &Path,
        options: StartOptions,
        out: OutputSender,
    ) -> Result<Self, String> {
        let (child, stdout, stdin) = process::spawn(
            &harness.id,
            &harness.command,
            cwd,
            options.log_session.clone(),
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

    pub(crate) async fn steer(&self, text: &str) -> bool {
        match self {
            Adapter::Acp(adapter) => adapter.steer(text).await,
            Adapter::Codex(adapter) => adapter.steer(text).await,
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
                Output::Update(_) | Output::Activity(_) => {}
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
        self.save_record(|record| {
            record.model = model;
            record.effort = effort;
        });
        Ok(options)
    }

    pub(crate) async fn steer(&self, text: &str) -> bool {
        let adapter = self.adapter.lock().await.clone();
        let Some(adapter) = adapter else {
            return false;
        };
        let accepted = adapter.steer(text).await;
        if accepted {
            self.transcript
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_user(text);
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
        result
    }

    async fn run_turn(
        &self,
        prompt: Vec<ContentBlock>,
        cancel: &Notify,
    ) -> Result<StopReason, String> {
        let adapter = self.ensure_started().await?;
        self.transcript
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_user(&prompt_text(&prompt));
        self.save_history();
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
            HarnessRecord::new(&harness.id, std::env::temp_dir()),
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
