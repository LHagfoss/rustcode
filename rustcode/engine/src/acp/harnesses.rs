//! The ACP side of external harnesses (`docs/harnesses.md`): the
//! `_rustcode/harnesses*` and `_rustcode/onboarding/complete` requests, and
//! threads whose `session/new` named a harness. Those threads live in their
//! own registry beside the built-in sessions; every ACP request that takes a
//! session id looks there first.

use crate::harness::session::{ClientSink, ExternalSession, PermissionFuture};
use crate::harness::{self, HarnessKind, store};
use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigOptionValue, SessionInfoUpdate, SessionNotification, SessionUpdate,
};
use agent_client_protocol::{Client, ConnectionTo};
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
    let models = harness::models(request.harness.trim())
        .await
        .map_err(failure)?;
    Ok(serde_json::json!({ "models": models }))
}

pub(crate) async fn set(request: SetHarnessRequest) -> Result<Value, agent_client_protocol::Error> {
    harness::set(harness::config::HarnessChange {
        id: request.id,
        enabled: request.enabled,
        command: request.command,
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

pub(crate) async fn complete_onboarding() -> Result<Value, agent_client_protocol::Error> {
    harness::complete_onboarding().await.map_err(failure)
}

/// Updates and permission requests of one external thread, sent to the ACP
/// client under the thread's own session id.
struct AcpClientSink {
    connection: ConnectionTo<Client>,
    session_id: String,
}

impl ClientSink for AcpClientSink {
    fn update(&self, update: SessionUpdate) {
        let _ = self
            .connection
            .send_notification(SessionNotification::new(self.session_id.clone(), update));
    }

    fn request_permission(&self, mut request: RequestPermissionRequest) -> PermissionFuture {
        request.session_id = self.session_id.clone().into();
        let connection = self.connection.clone();
        Box::pin(async move {
            match connection.send_request(request).block_task().await {
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
    connection: &ConnectionTo<Client>,
    orchestration: u8,
) -> Result<NewSessionResponse, agent_client_protocol::Error> {
    let cwd = super::canonical_acp_cwd(&request.cwd, "session/new")?;
    let id = asked.harness.clone().unwrap_or_default();
    let dir = harness::config::config_dir().map_err(failure)?;
    let (defaults, entries) = harness::config::load_in(&dir);
    let resolved = harness::resolve(&entries, &id)
        .ok_or_else(|| failure(format!("unknown harness '{id}'")))?;
    let is_default = defaults.harness.as_deref() == Some(id.as_str());
    let model = asked
        .model
        .clone()
        .or_else(|| defaults.model.clone().filter(|_| is_default));
    let effort = asked
        .effort
        .clone()
        .or_else(|| defaults.effort.clone().filter(|_| is_default));

    let session_id = crate::config::create_detached_session();
    if session_id.is_empty() {
        return Err(failure("session storage is unavailable".to_owned()));
    }
    let mut record = store::HarnessRecord::new(&resolved.id, cwd.clone());
    record.model = model;
    record.effort = effort;
    store::save(&session_id, &record);
    let _ = crate::config::save_session_workspace(
        &session_id,
        &super::session_workspace(cwd, request.additional_directories.clone()),
    );
    let sink = Arc::new(AcpClientSink {
        connection: connection.clone(),
        session_id: session_id.clone(),
    });
    let session = ExternalSession::new(
        session_id.clone(),
        resolved.clone(),
        record,
        Vec::new(),
        sink,
        true,
    );
    let orchestrated = attach_orchestrator(&session, orchestration, connection).await;
    if let Err(message) = session.ensure_started().await {
        crate::orchestration::hub::release(&session_id);
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
        .meta(harness::session_meta(&resolved, orchestrated)))
}

/// Give an external thread the orchestration tools when the client hosts
/// version 2 of them: register it with the engine's orchestrator socket and
/// have the harness start `rustcode mcp-orchestrator`. Whether it did.
async fn attach_orchestrator(
    session: &ExternalSession,
    orchestration: u8,
    connection: &ConnectionTo<Client>,
) -> bool {
    if orchestration < 2 || !harness::capabilities(&session.harness).orchestration {
        return false;
    }
    let Some(client) = super::preview::orchestration_client_for(true, connection, &session.id)
    else {
        return false;
    };
    let launch = crate::orchestration::hub::launch_for(&session.id, client).await;
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
    connection: &ConnectionTo<Client>,
    orchestration: u8,
) -> Result<LoadSessionResponse, agent_client_protocol::Error> {
    let session_id = request.session_id.to_string();
    let cwd = super::canonical_acp_cwd(&request.cwd, "session/load")?;
    if std::fs::canonicalize(&record.cwd).ok().as_ref() != Some(&cwd) {
        return Err(failure(
            "session/load cwd does not match the recorded workspace".to_owned(),
        ));
    }
    let dir = harness::config::config_dir().map_err(failure)?;
    let (_, entries) = harness::config::load_in(&dir);
    let resolved = harness::resolve(&entries, &record.harness).ok_or_else(|| {
        failure(format!(
            "this thread ran on '{}', which is no longer configured",
            record.harness
        ))
    })?;
    let history = crate::config::load_session_by_id(&session_id)
        .map(|(_, history)| history)
        .unwrap_or_default();
    let title = record.title.clone();
    let usage = record
        .usage
        .clone()
        .and_then(|usage| serde_json::from_value::<SessionUpdate>(usage).ok());
    let session = {
        let mut open = sessions.lock().await;
        if open.contains_key(&session_id) {
            return Err(failure("session is already active".to_owned()));
        }
        let sink = Arc::new(AcpClientSink {
            connection: connection.clone(),
            session_id: session_id.clone(),
        });
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
    let orchestrated = attach_orchestrator(&session, orchestration, connection).await;
    super::mark_acp_session(&session_id);
    let mut updates = harness::transcript::replay(&history, Some(&cwd));
    updates.extend(usage);
    if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
        updates.push(SessionUpdate::SessionInfoUpdate(
            SessionInfoUpdate::new().title(title),
        ));
    }
    for update in updates {
        if let Err(error) =
            connection.send_notification(SessionNotification::new(session_id.clone(), update))
        {
            close(sessions, &session_id).await;
            return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
        }
    }
    Ok(LoadSessionResponse::new()
        .config_options(session.config_options().await)
        .meta(harness::session_meta(&resolved, orchestrated)))
}

pub(crate) async fn get(
    sessions: &ExternalSessions,
    session_id: &str,
) -> Option<Arc<ExternalSession>> {
    sessions.lock().await.get(session_id).cloned()
}

/// Close one external thread; whether it was one.
pub(crate) async fn close(sessions: &ExternalSessions, session_id: &str) -> bool {
    let session = sessions.lock().await.remove(session_id);
    let Some(session) = session else {
        return false;
    };
    session.close().await;
    crate::orchestration::hub::release(session_id);
    super::unmark_acp_session(session_id);
    true
}

/// The connection closed: stop every harness.
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
/// harness.
pub(crate) fn list_meta(session_id: &str) -> Option<(Value, Option<String>)> {
    let record = store::load(session_id)?;
    Some((
        serde_json::json!({ "harness": { "id": record.harness } }),
        record.model,
    ))
}
