//! The ACP side of external harnesses (`docs/harnesses.md`): the
//! `_rustcode/harnesses*` and `_rustcode/onboarding/complete` requests, and
//! threads whose `session/new` named a harness. Those threads live in their
//! own registry beside the built-in sessions; every ACP request that takes a
//! session id looks there first.

use super::clients::{Peer, SessionClients};
use crate::harness::session::{ClientSink, ExternalSession, PermissionFuture};
use crate::harness::{self, HarnessKind, store};
use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOptionValue, SessionInfoUpdate, SessionUpdate,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) type ExternalSessions = Arc<Mutex<HashMap<String, Arc<ExternalSession>>>>;

/// List every harness, its detection and the defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses", response = serde_json::Value)]
pub(crate) struct HarnessesRequest {}

/// List a harness's models, starting it briefly if needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/models", response = serde_json::Value)]
pub(crate) struct HarnessModelsRequest {
    pub harness: String,
    /// Start the harness again even when its models are known: a Check.
    #[serde(default)]
    pub refresh: bool,
}

/// Turn a harness on or off, change its command, or add a custom one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/set", response = serde_json::Value)]
pub(crate) struct SetHarnessRequest {
    pub id: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// The custom harness's environment; replaces the old one.
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<HarnessKind>,
}

/// Remove a custom harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/remove", response = serde_json::Value)]
pub(crate) struct RemoveHarnessRequest {
    pub id: String,
}

/// Choose the harness, model and effort new threads start with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/default", response = serde_json::Value)]
pub(crate) struct DefaultHarnessRequest {
    pub harness: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// Keep the model and effort last chosen for new threads on a harness.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/remember", response = serde_json::Value)]
pub(crate) struct RememberHarnessRequest {
    pub harness: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

/// Move a thread to another harness (`harness/handoff.rs`). The new harness
/// starts with the thread's next prompt, which carries the conversation so far.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/harness", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwitchHarnessRequest {
    pub session_id: String,
    pub harness: String,
    /// The named account to run on; omitted is the harness's default login.
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Why the client moves the thread, for its activity: `limit` when it
    /// continues a delegate on another provider at a usage limit (#3392).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Add a named account: a config directory of its own.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/accounts/add", response = serde_json::Value)]
pub(crate) struct AddHarnessAccountRequest {
    pub harness: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub config_dir: Option<String>,
}

/// Forget a named account; its directory stays on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/accounts/remove", response = serde_json::Value)]
pub(crate) struct RemoveHarnessAccountRequest {
    pub harness: String,
    pub id: String,
}

/// Re-read each account's sign-in and answer the snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/harnesses/accounts/check", response = serde_json::Value)]
pub(crate) struct CheckHarnessAccountsRequest {
    /// Harnesses whose install is detected again rather than read from the last minute's answer.
    #[serde(default)]
    pub redetect: Vec<String>,
}

/// Mark first-run onboarding done.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/onboarding/complete", response = serde_json::Value)]
pub(crate) struct OnboardingCompleteRequest {}

fn failure(message: String) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message)
}

pub(crate) async fn snapshot() -> Result<Value, agent_client_protocol::Error> {
    harness::snapshot().await.map_err(failure)
}

pub(crate) async fn models(
    request: HarnessModelsRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let models = harness::models(request.harness.trim(), request.refresh)
        .await
        .map_err(failure)?;
    Ok(serde_json::json!({ "models": models }))
}

pub(crate) async fn set(request: SetHarnessRequest) -> Result<Value, agent_client_protocol::Error> {
    harness::set(harness::config::HarnessChange {
        id: request.id,
        enabled: request.enabled,
        command: request.command,
        env: request.env,
        name: request.name,
        kind: request.kind,
    })
    .await
    .map_err(failure)
}

pub(crate) async fn remove(
    request: RemoveHarnessRequest,
) -> Result<Value, agent_client_protocol::Error> {
    harness::remove(request.id.trim()).await.map_err(failure)
}

pub(crate) async fn set_default(
    request: DefaultHarnessRequest,
) -> Result<Value, agent_client_protocol::Error> {
    harness::set_default(request.harness, request.model, request.effort)
        .await
        .map_err(failure)
}

pub(crate) async fn remember(
    request: RememberHarnessRequest,
) -> Result<Value, agent_client_protocol::Error> {
    tokio::task::spawn_blocking(move || {
        harness::remember(request.harness.trim(), request.model, request.effort)
    })
    .await
    .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?
    .map_err(failure)
}

/// `_rustcode/harnesses/accounts/add`: validate, write the user config and
/// create the directory, then answer the fresh snapshot.
pub(crate) async fn add_account(
    request: AddHarnessAccountRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let dir = harness::config::config_dir().map_err(failure)?;
    tokio::task::spawn_blocking(move || {
        harness::add_account(harness::config::AccountChange {
            harness: request.harness,
            id: request.id,
            name: Some(request.name),
            config_dir: request.config_dir,
        })
    })
    .await
    .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?
    .map_err(failure)?;
    harness::snapshot_in(&dir).await.map_err(failure)
}

/// `_rustcode/harnesses/accounts/remove`: only the config entry goes; the
/// directory and the login in it stay. Refused while a live thread uses it.
pub(crate) async fn remove_account(
    sessions: &ExternalSessions,
    request: RemoveHarnessAccountRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let harness_id = request.harness.trim().to_owned();
    let account = request.id.trim().to_owned();
    let live = sessions.lock().await.values().any(|session| {
        let record = session.record();
        record.harness == harness_id && record.account.as_deref() == Some(account.as_str())
    });
    if live {
        return Err(failure(format!(
            "a thread is running on the account '{account}'; close it first"
        )));
    }
    let dir = harness::config::config_dir().map_err(failure)?;
    tokio::task::spawn_blocking({
        let harness_id = harness_id.clone();
        let account = account.clone();
        move || harness::remove_account(&harness_id, &account)
    })
    .await
    .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?
    .map_err(failure)?;
    harness::snapshot_in(&dir).await.map_err(failure)
}

/// `_rustcode/harnesses/accounts/check`: look each account's sign-in up
/// again (only its credentials, nothing is started).
pub(crate) async fn check_accounts(
    request: CheckHarnessAccountsRequest,
) -> Result<Value, agent_client_protocol::Error> {
    harness::check_accounts(&request.redetect)
        .await
        .map_err(failure)
}

pub(crate) async fn complete_onboarding() -> Result<Value, agent_client_protocol::Error> {
    harness::complete_onboarding().await.map_err(failure)
}

/// Updates and permission requests of one external thread, sent to the ACP
/// clients that have it, under the thread's own session id.
struct AcpClientSink {
    clients: SessionClients,
    /// The turn running, kept for a client that opens the thread mid-turn.
    turn: std::sync::Mutex<Option<super::clients::LiveTurn>>,
}

impl AcpClientSink {
    fn new(clients: SessionClients) -> Self {
        Self {
            clients,
            turn: std::sync::Mutex::new(None),
        }
    }
}

impl ClientSink for AcpClientSink {
    fn update(&self, update: SessionUpdate) {
        let _ = self.clients.update(update);
    }

    fn turn_started(&self, history: usize, prompt: String) {
        let turn = self.clients.begin_turn(history, Some(prompt));
        *self.turn.lock().unwrap_or_else(|error| error.into_inner()) = Some(turn);
    }

    fn turn_ended(&self) {
        self.turn
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }

    fn activity(&self, activity: harness::background::Activity) {
        let _ = self
            .clients
            .notify(super::prompt::SessionActivityNotification {
                session_id: self.clients.session_id().to_owned(),
                state: activity.state.to_owned(),
                background: activity.background,
            });
    }

    fn request_permission(&self, mut request: RequestPermissionRequest) -> PermissionFuture {
        request.session_id = self.clients.session_id().to_owned().into();
        let clients = self.clients.clone();
        let waiting = crate::thread_status::waiting(clients.session_id());
        // What kind of tool is asking and what each option means, never the
        // call's title or arguments, which can hold a command or a path.
        let tool_kind = request
            .tool_call
            .fields
            .kind
            .as_ref()
            .map(|kind| format!("{kind:?}").to_lowercase());
        let option_kinds: Vec<(String, String)> = request
            .options
            .iter()
            .map(|option| {
                (
                    option.option_id.0.to_string(),
                    format!("{:?}", option.kind).to_lowercase(),
                )
            })
            .collect();
        Box::pin(async move {
            let _waiting = waiting;
            let started = std::time::Instant::now();
            let asked = chrono::Utc::now();
            let answer = clients.request(None, request).await;
            let decision = match &answer {
                Ok(response) => match &response.outcome {
                    RequestPermissionOutcome::Selected(selected) => option_kinds
                        .iter()
                        .find(|(id, _)| id.as_str() == selected.option_id.0.as_ref())
                        .map_or_else(|| "selected".to_owned(), |(_, kind)| kind.clone()),
                    _ => "cancelled".to_owned(),
                },
                Err(_) => "cancelled".to_owned(),
            };
            let stamp = |at: chrono::DateTime<chrono::Utc>| {
                at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            };
            crate::data_store::record_approval(&crate::data_store::ApprovalRow {
                thread_id: clients.session_id().to_owned(),
                source: "harness".to_owned(),
                tool_name: tool_kind,
                requested_at: stamp(asked),
                decided_at: Some(stamp(chrono::Utc::now())),
                wait_ms: Some(started.elapsed().as_millis() as i64),
                decision,
            });
            match answer {
                Ok(response) => response,
                Err(error) => {
                    crate::dbg_log!("harness permission request failed: {error}");
                    RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                }
            }
        })
    }
}

/// The harness `session/new` asked for, when it is an external one.
pub(crate) fn external_request(request: &NewSessionRequest) -> Option<harness::HarnessRequest> {
    let asked = harness::requested(request.meta.as_ref());
    asked
        .harness
        .as_deref()
        .is_some_and(|id| id != harness::BUILTIN_ID)
        .then_some(asked)
}

/// `session/new` on an external harness: record the thread, start the
/// harness in the thread's directory, and answer with its options.
pub(crate) async fn new_session(
    sessions: &ExternalSessions,
    request: NewSessionRequest,
    asked: harness::HarnessRequest,
    peer: &Peer,
    remote: &super::remote::RemoteManager,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    let cwd = super::session_new_cwd(&request.cwd)?;
    let id = asked.harness.clone().unwrap_or_default();
    let dir = harness::config::config_dir().map_err(failure)?;
    let (defaults, entries) = harness::config::load_in(&dir);
    let resolved = harness::resolve(&entries, &id)
        .ok_or_else(|| failure(format!("unknown harness '{id}'")))?;
    let accounts = harness::config::load_accounts_in(&dir);
    let account = match asked
        .account
        .as_deref()
        .map(str::trim)
        .filter(|account| !account.is_empty())
    {
        Some(account) => {
            Some(harness::config::find_account(&accounts, &id, account).map_err(failure)?)
        }
        None => None,
    };
    let (model, effort) = harness::starting_choice(
        &id,
        &asked,
        &defaults,
        &entries,
        &harness::learned_for(&id).models.unwrap_or_default(),
    );

    let session_id = crate::config::create_detached_session();
    if session_id.is_empty() {
        return Err(failure("session storage is unavailable".to_owned()));
    }
    // The name the thread was made with, from the app that made it (issue 3524).
    crate::config::save_requested_title(&session_id, request.meta.as_ref());
    // The thread that started this one, kept in its metadata so a pruned link
    // cannot turn it into a top-level thread (`thread_parent.rs`).
    if let Some((parent, relationship)) = crate::thread_parent::requested(request.meta.as_ref()) {
        crate::thread_parent::record(&session_id, &parent, relationship);
    }
    let mut record = store::HarnessRecord::new(&resolved.id, cwd.clone());
    record.account = account.as_ref().map(|account| account.id.clone());
    record.model = model;
    record.effort = effort;
    store::save(&session_id, &record);
    let _ = crate::config::save_session_workspace(
        &session_id,
        &super::session_workspace(cwd, request.additional_directories.clone()),
    );
    let clients = peer.attach(&session_id);
    let sink = remote.wrap_sink(Arc::new(AcpClientSink::new(clients.clone())), &session_id);
    let session = ExternalSession::new(
        session_id.clone(),
        resolved.clone(),
        record,
        Vec::new(),
        sink,
        true,
    );
    let info = peer.info();
    session.set_client_display(info.display());
    let orchestrated =
        attach_orchestrator(&session, info.orchestration, info.smart_ui, &clients).await;
    if let Err(message) = session.ensure_started().await {
        crate::orchestration::hub::release(&session_id);
        peer.clients().forget_session(&session_id);
        crate::dbg_log!("harness {} failed to start: {message}", resolved.id);
        return Err(agent_client_protocol::Error::internal_error().data(message));
    }
    let options = session.config_options().await;
    super::mark_acp_session(&session_id);
    sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&session));
    Ok(NewSessionResponse::new(session_id)
        .config_options(options)
        .meta(harness::session_meta_for(
            &resolved,
            account.as_ref(),
            orchestrated,
        )))
}

/// Give an external thread the orchestration tools when the client hosts
/// version 2 of them: register it with the engine's orchestrator socket and
/// have the harness start `rustcode mcp-orchestrator`. Whether it did.
async fn attach_orchestrator(
    session: &ExternalSession,
    orchestration: u8,
    smart_ui: crate::app::SmartUi,
    clients: &SessionClients,
) -> bool {
    if !harness::capabilities(&session.harness).orchestration {
        return false;
    }
    // A harness without its own question tool also gets `ask_user_question`,
    // asked through this thread's permission channel (#3393).
    let ask_user = harness::needs_question_tool(&session.harness);
    // No app hosts the tools: this engine answers the reads itself and names
    // the app for the calls that need it (#3422). An app that turned them off
    // still turns them off, so nothing is attached then.
    if orchestration == 0 {
        if clients.app_attached() {
            return false;
        }
        let launch = crate::orchestration::hub::launch_local_for(
            &session.id,
            crate::orchestration::VERSION,
            smart_ui,
        )
        .await;
        return set_launch(session, launch, ask_user).await;
    }
    if orchestration < 2 {
        return false;
    }
    let Some(client) = super::preview::orchestration_client_for(true, clients) else {
        return false;
    };
    let launch =
        crate::orchestration::hub::launch_for(&session.id, client, orchestration, smart_ui).await;
    set_launch(session, launch, ask_user).await
}

/// Start the thread's orchestration server with `launch`, offering
/// `ask_user_question` when `ask_user`. Whether there is a server.
async fn set_launch(
    session: &ExternalSession,
    launch: Option<crate::orchestration::hub::Launch>,
    ask_user: bool,
) -> bool {
    let ask_user = ask_user
        && launch.is_some()
        && crate::orchestration::hub::set_questions(&session.id, session.question_asker()).await;
    let launch = launch.map(|launch| launch.with_ask_user(ask_user));
    let attached = launch.is_some();
    session.set_orchestrator(launch);
    attached
}

/// `session/load` of a thread that ran on an external harness: replay its
/// recorded transcript. The harness itself starts with the next prompt and
/// resumes its own session when it can.
pub(crate) async fn load_session(
    sessions: &ExternalSessions,
    request: &LoadSessionRequest,
    record: store::HarnessRecord,
    peer: &Peer,
    remote: &super::remote::RemoteManager,
) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
    let session_id = request.session_id.to_string();
    let cwd = super::canonical_acp_cwd(&request.cwd, "session/load")?;
    if !super::opens_in(&record.cwd, &cwd) {
        return Err(failure(
            "session/load cwd does not match the recorded workspace".to_owned(),
        ));
    }
    let dir = harness::config::config_dir().map_err(failure)?;
    let (_, entries) = harness::config::load_in(&dir);
    let accounts = harness::config::load_accounts_in(&dir);
    // A removed account must not lock a thread out of its own history. The
    // thread opens and replays what it recorded; nothing is started, and its
    // next prompt says the account is gone (`ensure_started`). The meta still
    // names the account, so the client can show it was removed.
    let account = harness::config::account_for_load(&accounts, &record);
    let resolved = harness::resolve(&entries, &record.harness).ok_or_else(|| {
        failure(format!(
            "this thread ran on '{}', which is no longer configured",
            record.harness
        ))
    })?;
    // Where the thread's log is before its transcript is read: whatever it
    // logs after this is not in the transcript.
    let base = peer.clients().load_mark(&session_id);
    // Validate before replay: a failed read must not create an empty writable
    // thread whose next prompt overwrites the transcript.
    let _ = handoff_history(&session_id)?;
    let history = crate::config::load_session_history_direct(&session_id);
    let title = record.title.clone();
    let info = peer.info();
    let usage = record
        .usage
        .clone()
        .and_then(|usage| serde_json::from_value::<SessionUpdate>(usage).ok());
    let session = {
        let mut open = sessions.lock().await;
        if open.contains_key(&session_id) {
            return Err(failure("session is already active".to_owned()));
        }
        // Not attached yet: the replay below attaches the loading client in
        // the same step that queues the transcript to it, so no live update
        // can land between its chunks.
        let sink = remote.wrap_sink(
            Arc::new(AcpClientSink::new(peer.clients().session(&session_id))),
            &session_id,
        );
        let session = ExternalSession::new(
            session_id.clone(),
            resolved.clone(),
            record,
            history.clone(),
            sink,
            true,
        );
        open.insert(session_id.clone(), Arc::clone(&session));
        session
    };
    session.set_client_display(info.display());
    super::mark_acp_session(&session_id);
    let mut updates = harness::transcript::replay(&history, Some(&cwd));
    updates.extend(usage);
    if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
        updates.push(SessionUpdate::SessionInfoUpdate(
            SessionInfoUpdate::new().title(title),
        ));
    }
    // It asked to resume a thread that is not open here (the engine
    // restarted, or it was closed): the transcript replaces what it has.
    let reset = super::cursor::requested_cursor(request).is_some();
    super::load_pause::here(&session_id).await;
    // Queued and attached under one lock (see `Peer::attach_closed`): the
    // transcript, then what the thread logged since `base`.
    let failed = match peer.attach_closed(&session_id, base, updates, reset) {
        Ok(Some(_)) => None,
        Ok(None) => Some("session changed while loading; retry session/load".to_owned()),
        Err(error) => Some(error.to_string()),
    };
    if let Some(error) = failed {
        close(sessions, &session_id, remote).await;
        peer.clients().forget_session(&session_id);
        return Err(agent_client_protocol::Error::internal_error().data(error));
    }
    // The orchestrator is hosted by the client that most recently attached.
    let orchestrated = attach_orchestrator(
        &session,
        info.orchestration,
        info.smart_ui,
        &peer.clients().session(&session_id),
    )
    .await;
    Ok(peer.position(&session_id, reset).finish(
        LoadSessionResponse::new()
            .config_options(session.config_options().await)
            .meta(harness::session_meta_for(
                &resolved,
                account.as_ref(),
                orchestrated,
            )),
    ))
}

/// `session/load` of a thread already open in this engine, perhaps mid-turn:
/// `peer` joins the clients that have it. It is sent the transcript from
/// before the running turn, what the turn sent so far, then the rest live.
pub(crate) async fn attach_open(
    session: &Arc<ExternalSession>,
    request: &LoadSessionRequest,
    peer: &Peer,
) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
    let session_id = session.id.clone();
    let cwd = super::canonical_acp_cwd(&request.cwd, "session/load")?;
    if std::fs::canonicalize(&session.cwd).ok().as_ref() != Some(&cwd) {
        return Err(failure(
            "session/load cwd does not match the open thread".to_owned(),
        ));
    }
    let position = super::attach_with_replay(
        peer,
        &session_id,
        super::cursor::requested_cursor(request),
        || {
            let history = rustcode_core::History::from(session.recorded_history());
            async move { history }
        },
        |history| harness::transcript::replay(history, Some(&cwd)),
    )
    .await?;
    let record = session.record();
    let mut updates = Vec::new();
    // A resumed client has these already: changes to them were updates.
    if !position.resumed
        && let Some(usage) = record
            .usage
            .clone()
            .and_then(|usage| serde_json::from_value::<SessionUpdate>(usage).ok())
    {
        updates.push(usage);
    }
    if !position.resumed
        && let Some(title) = record
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
    {
        updates.push(SessionUpdate::SessionInfoUpdate(
            SessionInfoUpdate::new().title(title),
        ));
    }
    for update in updates {
        let _ = peer.send_snapshot(&session_id, update);
    }
    let position = peer.position(&session_id, position.reset);
    let account = harness::config::config_dir().ok().and_then(|dir| {
        harness::config::account_for_load(&harness::config::load_accounts_in(&dir), &record)
    });
    // The app that loads this thread hosts its orchestration from now on. A
    // thread registered locally while no app was attached keeps its running
    // `mcp-orchestrator` and its token; only the hub's client is swapped, in
    // place (#3422).
    let info = peer.info();
    let session_clients = peer.clients().session(&session_id);
    let orchestrated = if !session.orchestrated() {
        attach_orchestrator(session, info.orchestration, info.smart_ui, &session_clients).await
    } else if info.orchestration >= 1 {
        if let Some(client) = super::preview::orchestration_client_for(true, &session_clients) {
            let _ = crate::orchestration::hub::swap_client(&session_id, client, info.orchestration)
                .await;
        }
        true
    } else {
        true
    };
    Ok(position.finish(
        LoadSessionResponse::new()
            .config_options(session.config_options().await)
            .meta(harness::session_meta_for(
                &session.harness,
                account.as_ref(),
                orchestrated,
            )),
    ))
}

/// Where a switch starts from: the open session, a built-in thread's folder,
/// or the record of a thread that is not open (for example one whose account
/// was removed). Never starts a harness.
fn switch_origin(
    current: Option<&ExternalSession>,
    builtin_cwd: Option<std::path::PathBuf>,
    to: &str,
    session_id: &str,
) -> Option<(String, store::HarnessRecord)> {
    match (current, builtin_cwd) {
        (Some(current), _) => Some((current.harness.id.clone(), current.record())),
        (None, Some(cwd)) => Some((
            harness::BUILTIN_ID.to_owned(),
            store::HarnessRecord::new(to, cwd),
        )),
        (None, None) => store::load(session_id).map(|record| (record.harness.clone(), record)),
    }
}

/// A handoff must read the transcript independently of listing/resume filters.
/// Refuse a damaged file before changing the harness record or closing a thread.
fn handoff_history(
    session_id: &str,
) -> Result<Vec<crate::app::ChatMessage>, agent_client_protocol::Error> {
    crate::config::flush_history();
    let directory = crate::config::get_active_session_dir(session_id)
        .ok_or_else(|| failure("session storage is unavailable".into()))?;
    let path = crate::config::session_meta_by_id(session_id)
        .map(|meta| meta.path)
        .unwrap_or_else(|| directory.join("history.json"));
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| failure(format!("cannot read thread history: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(failure(format!("cannot read thread history: {error}"))),
    }
}

/// Why a thread cannot change harness or account now: its turn is running.
fn switch_refusal(session_id: &str) -> Option<&'static str> {
    crate::thread_status::is_running(session_id)
        .then_some("stop the running turn before switching harness")
}

pub(crate) async fn get(
    sessions: &ExternalSessions,
    session_id: &str,
) -> Option<Arc<ExternalSession>> {
    sessions.lock().await.get(session_id).cloned()
}

/// `_rustcode/session/harness`: the thread `request.session_id` goes on with
/// `request.harness`. It was an external thread in `sessions`, or a built-in
/// one in `builtin_cwd` that the caller already closed. Answers with the
/// thread's new options and `_meta` (as `session/load` would).
pub(crate) async fn switch(
    sessions: &ExternalSessions,
    request: SwitchHarnessRequest,
    builtin_cwd: Option<std::path::PathBuf>,
    peer: &Peer,
    remote: &super::remote::RemoteManager,
) -> Result<Value, agent_client_protocol::Error> {
    let session_id = request.session_id.clone();
    let reason = request.reason.clone();
    let to = request.harness.trim().to_owned();
    if to == harness::BUILTIN_ID {
        return Err(failure(
            "RustCode cannot take over a thread from another harness yet; start a new thread on it"
                .to_owned(),
        ));
    }
    if let Some(reason) = switch_refusal(&session_id) {
        return Err(failure(reason.to_owned()));
    }
    let dir = harness::config::config_dir().map_err(failure)?;
    let (defaults, entries) = harness::config::load_in(&dir);
    let accounts = harness::config::load_accounts_in(&dir);
    let resolved = harness::resolve(&entries, &to)
        .ok_or_else(|| failure(format!("unknown harness '{to}'")))?;
    // The account asked for, when it is one that harness really has.
    let account = match request
        .account
        .as_deref()
        .map(str::trim)
        .filter(|account| !account.is_empty())
    {
        Some(account) => {
            Some(harness::config::find_account(&accounts, &to, account).map_err(failure)?)
        }
        None => None,
    };
    let current = get(sessions, &session_id).await;
    let (from, mut record) = switch_origin(current.as_deref(), builtin_cwd, &to, &session_id)
        .ok_or_else(|| failure(format!("unknown session '{session_id}'")))?;
    let account_id = account.as_ref().map(|account| account.id.as_str());
    if from == to && record.account.as_deref() == account_id {
        return Err(failure(format!(
            "the thread already runs on {}",
            account.as_ref().map_or_else(
                || resolved.name.clone(),
                |account| format!("{} as {}", resolved.name, account.display_name())
            )
        )));
    }
    // What the old thread was, for the divider and the handoff preamble.
    let from_account_name = record.account.as_deref().and_then(|id| {
        accounts
            .iter()
            .find(|known| known.harness == from && known.id == id)
            .map(|known| known.display_name().to_owned())
    });
    let from_label = harness::handoff::label(&from, from_account_name.as_deref());
    let mut history = match &current {
        Some(session) => session.recorded_history(),
        None => handoff_history(&session_id)?,
    };
    if current.is_some() {
        close(sessions, &session_id, remote).await;
    }
    let asked = harness::HarnessRequest {
        harness: Some(to.clone()),
        account: request.account.clone(),
        model: request.model,
        effort: request.effort,
    };
    let (model, effort) = harness::starting_choice(
        &to,
        &asked,
        &defaults,
        &entries,
        &harness::learned_for(&to).models.unwrap_or_default(),
    );
    let from_model = record.model.clone();
    record.harness = to.clone();
    record.account = account.as_ref().map(|account| account.id.clone());
    record.native_session_id = None;
    record.model = model;
    record.effort = effort;
    record.usage = None;
    // A thread switched twice before it prompted still owes the first harness's context.
    record.handoff_from = record.handoff_from.take().or(Some(from_label.clone()));
    record.updated_at = chrono::Utc::now().to_rfc3339();
    store::save(&session_id, &record);

    let to_account_name = account
        .as_ref()
        .map(harness::config::HarnessAccount::display_name);
    let marker = harness::handoff::marker_with(
        &from,
        from_account_name.as_deref(),
        &to,
        to_account_name,
        record.model.as_deref(),
    );
    let divider = harness::handoff::divider(&marker);
    history.push(marker);
    if reason.as_deref() == Some("limit") {
        crate::logger::operational_event(
            "thread.limit_switch",
            serde_json::json!({
                "session_id": session_id,
                "from_harness": from,
                "from_model": from_model,
                "to_harness": to,
                "to_model": record.model,
                "reason": "limit",
            }),
        );
    }

    let clients = peer.attach(&session_id);
    let sink = remote.wrap_sink(Arc::new(AcpClientSink::new(clients.clone())), &session_id);
    let session = ExternalSession::new(
        session_id.clone(),
        resolved.clone(),
        record,
        history,
        sink,
        true,
    );
    let info = peer.info();
    session.set_client_display(info.display());
    session.save_now();
    let orchestrated =
        attach_orchestrator(&session, info.orchestration, info.smart_ui, &clients).await;
    super::mark_acp_session(&session_id);
    sessions
        .lock()
        .await
        .insert(session_id.clone(), Arc::clone(&session));
    if let Some(divider) = divider {
        let _ = clients.update(divider);
    }
    Ok(serde_json::json!({
        "configOptions": session.config_options().await,
        "_meta": harness::session_meta_for(&resolved, account.as_ref(), orchestrated),
    }))
}

/// Close one external thread; whether it was one.
pub(crate) async fn close(
    sessions: &ExternalSessions,
    session_id: &str,
    remote: &super::remote::RemoteManager,
) -> bool {
    let session = sessions.lock().await.remove(session_id);
    let Some(session) = session else {
        return false;
    };
    remote
        .stop_session(
            session_id,
            crate::remote::SessionCloseReason::SessionChanged,
        )
        .await;
    session.close().await;
    crate::orchestration::hub::release(session_id);
    super::unmark_acp_session(session_id);
    true
}

/// The engine stops: stop every harness.
pub(crate) async fn close_all(sessions: &ExternalSessions) {
    let open = sessions.lock().await.drain().collect::<Vec<_>>();
    for (session_id, session) in open {
        session.kill_now();
        crate::orchestration::hub::release(&session_id);
        super::unmark_acp_session(&session_id);
    }
    harness::process::kill_all();
}

/// A `session/set_config_option` value as the harness takes it.
pub(crate) fn config_value(value: &SessionConfigOptionValue) -> Value {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.get("value").cloned())
        .unwrap_or(Value::Null)
}

/// `_meta.rustcode.harness` of a listed thread, for one on an external
/// harness, and the account it runs on (so a client can count a login's
/// threads without opening each one).
pub(crate) fn list_meta(session_id: &str) -> Option<(Value, Option<String>)> {
    let record = store::load(session_id)?;
    Some((
        serde_json::json!({
            "harness": { "id": record.harness },
            "account": record.account,
        }),
        record.model,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::config::{HarnessAccount, find_account};

    #[test]
    fn handoff_preserves_an_interrupted_transcript_and_refuses_corruption() {
        let id = crate::config::create_detached_session();
        let history = vec![crate::app::ChatMessage::new("user", "Still working")];
        crate::config::save_session_history(&id, &history);
        crate::config::flush_history();
        let loaded = handoff_history(&id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].content, "Still working");
        let path = crate::config::get_active_session_dir(&id)
            .unwrap()
            .join("history.json");
        std::fs::write(&path, "broken json").unwrap();
        assert!(handoff_history(&id).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken json");
    }

    #[test]
    fn an_unknown_account_is_refused_instead_of_falling_back() {
        let accounts = vec![HarnessAccount {
            id: "paral-2".into(),
            harness: "claude-code".into(),
            name: Some("Paral 2".into()),
            config_dir: "~/.claude-paral-2".into(),
        }];
        let error = find_account(&accounts, "claude-code", "paral-9").unwrap_err();
        assert!(
            error.contains("unknown claude-code account 'paral-9'"),
            "{error}"
        );
        assert!(find_account(&accounts, "claude-code", "paral-2").is_ok());
    }

    #[tokio::test]
    async fn a_thread_loads_with_its_removed_account_and_its_history() {
        let record = store::HarnessRecord {
            harness: "claude-code".into(),
            account: Some("gone".into()),
            ..store::HarnessRecord::new("claude-code", std::env::temp_dir())
        };
        // Loading resolves the record's account without the config entry.
        let account = harness::config::account_for_load(&[], &record).expect("account named");
        assert_eq!(account.id, "gone");
        assert_eq!(account.display_name(), "gone");
        let resolved = harness::resolve(&[], "claude-code").unwrap();
        let meta = harness::session_meta_for(&resolved, Some(&account), false);
        assert_eq!(
            meta["rustcode"]["account"]["id"], "gone",
            "the client still learns which account the record names"
        );

        // The history the load replays, with no harness started.
        let history = vec![
            crate::app::ChatMessage::new("user", "hello"),
            crate::app::ChatMessage::new("assistant", "hi"),
        ];
        let updates = harness::transcript::replay(&history, None);
        assert!(updates.len() >= 2, "the recorded conversation replays");
        let thread = crate::harness::session::test_support::with_record(&resolved, record, None);
        // Offline options: reading them never starts the harness.
        let _ = thread.session.config_options().await;
        assert_eq!(
            thread.session.record().account.as_deref(),
            Some("gone"),
            "no quiet fall back to the default login"
        );
    }

    #[tokio::test]
    async fn a_thread_with_a_removed_account_can_switch() {
        let session_id = crate::config::create_detached_session();
        let mut record = store::HarnessRecord::new("claude-code", std::env::temp_dir());
        record.account = Some("gone".into());
        store::save(&session_id, &record);
        // Not open in this engine: its own record is the starting point.
        let (from, origin) = switch_origin(None, None, "claude-code", &session_id)
            .expect("a closed thread still switches");
        assert_eq!(from, "claude-code");
        assert_eq!(origin.account.as_deref(), Some("gone"));

        // An open thread is used as it is.
        let harness = crate::harness::session::test_support::harness(
            "claude-code",
            harness::HarnessKind::Acp,
        );
        let thread = crate::harness::session::test_support::with_record(&harness, origin, None);
        let (from, origin) =
            switch_origin(Some(&thread.session), None, "claude-code", "ignored").unwrap();
        assert_eq!(from, "claude-code");
        assert_eq!(origin.account.as_deref(), Some("gone"));
    }

    #[test]
    fn switching_is_refused_while_a_turn_runs() {
        let id = "switch-refusal-test";
        assert_eq!(switch_refusal(id), None);
        {
            let _turn = crate::thread_status::turn(id);
            assert_eq!(
                switch_refusal(id),
                Some("stop the running turn before switching harness")
            );
        }
        assert_eq!(switch_refusal(id), None, "an idle thread may switch");
    }
}
