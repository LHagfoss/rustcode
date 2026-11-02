//! Kind `acp`: RustCode is the ACP client of a harness process (OpenCode,
//! Gemini CLI, Claude Code through `claude-agent-acp`, any ACP agent).
//!
//! The child's `session/update` notifications are relayed as they are; its
//! `session/request_permission` requests go to RustCode's client and the
//! answer comes back. RustCode advertises no file system or terminal client
//! capabilities, so the child uses its own tools; any `fs/*` or `terminal/*`
//! request is refused. The child's `model` and thought-level config options
//! are offered as the thread's `model` and `effort` options.

use super::rpc::{Incoming, RpcError, RpcPeer};
use super::session::{Output, OutputSender, StartOptions};
use super::{HarnessModel, ResolvedHarness, process::HarnessProcess};
use agent_client_protocol::schema::v1::{
    ContentBlock, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions, SessionUpdate, StopReason,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Default)]
struct State {
    /// The child's own config options, ids as the child spells them.
    options: Vec<SessionConfigOption>,
    /// `models` from an agent that predates config options.
    legacy_models: Option<Value>,
    /// Updates are dropped while the child replays history on load: the
    /// client already has the thread's recorded transcript.
    muted: bool,
}

pub(crate) struct AcpAdapter {
    peer: Arc<RpcPeer>,
    process: Option<HarnessProcess>,
    native_id: String,
    state: Arc<Mutex<State>>,
    killed: AtomicBool,
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|error| error.into_inner())
}

/// Our id for one of the child's options: its model choice is `model`, its
/// thought level `effort`, others keep their own id.
fn our_id(options: &[SessionConfigOption], index: usize) -> String {
    let option = &options[index];
    let id = option.id.0.as_ref();
    let is_model = |option: &SessionConfigOption| {
        option.category == Some(SessionConfigOptionCategory::Model)
            || option.id.0.as_ref() == "model"
    };
    let is_effort = |option: &SessionConfigOption| {
        option.category == Some(SessionConfigOptionCategory::ThoughtLevel)
            || matches!(
                option.id.0.as_ref(),
                "effort" | "reasoning_effort" | "thought_level"
            )
    };
    if is_model(option) && options.iter().position(is_model) == Some(index) {
        return "model".to_owned();
    }
    if is_effort(option) && options.iter().position(is_effort) == Some(index) {
        return "effort".to_owned();
    }
    if id == "model" || id == "effort" {
        return format!("harness.{id}");
    }
    id.to_owned()
}

/// The child's id for our option `id`.
fn their_id(options: &[SessionConfigOption], id: &str) -> Option<String> {
    (0..options.len())
        .find(|index| our_id(options, *index) == id)
        .map(|index| options[index].id.0.to_string())
}

fn mapped_options(state: &State) -> Vec<SessionConfigOption> {
    if state.options.is_empty() {
        return legacy_model_option(state.legacy_models.as_ref())
            .into_iter()
            .collect();
    }
    (0..state.options.len())
        .map(|index| {
            let mut option = state.options[index].clone();
            option.id = our_id(&state.options, index).into();
            option
        })
        .collect()
}

/// A `model` select built from the unstable `models` state some agents
/// send instead of config options.
fn legacy_model_option(models: Option<&Value>) -> Option<SessionConfigOption> {
    let models = models?;
    let current = models.get("currentModelId")?.as_str()?.to_owned();
    let choices = models
        .get("availableModels")?
        .as_array()?
        .iter()
        .filter_map(|model| {
            let id = model.get("modelId")?.as_str()?.to_owned();
            let name = model
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();
            let mut choice =
                agent_client_protocol::schema::v1::SessionConfigSelectOption::new(id, name);
            if let Some(description) = model.get("description").and_then(Value::as_str) {
                choice = choice.description(description);
            }
            Some(choice)
        })
        .collect::<Vec<_>>();
    Some(
        SessionConfigOption::select("model", "Model", current, choices)
            .category(SessionConfigOptionCategory::Model),
    )
}

fn select_values(option: &SessionConfigOption) -> (String, Vec<(String, String)>) {
    match &option.kind {
        SessionConfigKind::Select(select) => {
            let choices = match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => options
                    .iter()
                    .map(|choice| (choice.value.0.to_string(), choice.name.clone()))
                    .collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|group| group.options.iter())
                    .map(|choice| (choice.value.0.to_string(), choice.name.clone()))
                    .collect(),
                #[allow(unreachable_patterns)]
                _ => Vec::new(),
            };
            (select.current_value.0.to_string(), choices)
        }
        #[allow(unreachable_patterns)]
        _ => (String::new(), Vec::new()),
    }
}

fn parse_options(value: Option<&Value>) -> Vec<SessionConfigOption> {
    value
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| serde_json::from_value(option.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

impl AcpAdapter {
    pub(crate) async fn connect(
        harness: &ResolvedHarness,
        peer: Arc<RpcPeer>,
        incoming: mpsc::UnboundedReceiver<Incoming>,
        process: Option<HarnessProcess>,
        cwd: &Path,
        options: StartOptions,
        out: OutputSender,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State::default()));
        tokio::spawn(relay(
            incoming,
            Arc::clone(&peer),
            Arc::clone(&state),
            out.clone(),
        ));
        let describe = |error: RpcError| format!("{}: {}", harness.name, error.describe());
        let initialized = peer
            .request_within(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": { "readTextFile": false, "writeTextFile": false },
                        "terminal": false,
                    },
                    "clientInfo": { "name": "rustcode", "title": "RustCode", "version": env!("CARGO_PKG_VERSION") },
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await
            .map_err(describe)?;
        let capabilities = initialized.get("agentCapabilities");
        let load_session = capabilities
            .and_then(|capabilities| capabilities.get("loadSession"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let images = capabilities
            .and_then(|capabilities| capabilities.pointer("/promptCapabilities/image"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let cwd_text = cwd.to_string_lossy().into_owned();

        let mut native = None;
        let mut session = Value::Null;
        if let Some(resume) = options.resume.as_ref().filter(|_| load_session) {
            lock(&state).muted = true;
            let loaded = peer
                .request_within(
                    "session/load",
                    json!({"sessionId": resume, "cwd": cwd_text, "mcpServers": []}),
                    HANDSHAKE_TIMEOUT,
                )
                .await;
            // Let the replay drain before updates count again.
            peer.barrier().await;
            lock(&state).muted = false;
            match loaded {
                Ok(value) => {
                    native = Some(resume.clone());
                    session = value;
                }
                Err(error) => crate::dbg_log!(
                    "harness {}: session/load {resume} failed ({}); starting a new session",
                    harness.id,
                    error.describe()
                ),
            }
        }
        if native.is_none() {
            session = peer
                .request_within(
                    "session/new",
                    json!({"cwd": cwd_text, "mcpServers": []}),
                    HANDSHAKE_TIMEOUT,
                )
                .await
                .map_err(|error| {
                    if error.code == -32000 {
                        format!(
                            "{} needs you to sign in first ({})",
                            harness.name,
                            error.describe()
                        )
                    } else {
                        describe(error)
                    }
                })?;
            native = session
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        let native_id = native.ok_or_else(|| format!("{} returned no session id", harness.name))?;
        {
            let mut state = lock(&state);
            state.options = parse_options(session.get("configOptions"));
            state.legacy_models = session
                .get("models")
                .cloned()
                .filter(|models| !models.is_null());
        }
        let adapter = Self {
            peer,
            process,
            native_id,
            state,
            killed: AtomicBool::new(false),
        };
        if let Some(model) = options.model {
            let _ = adapter.set_config("model", json!(model)).await;
        }
        if let Some(effort) = options.effort {
            let _ = adapter.set_config("effort", json!(effort)).await;
        }
        let has_effort = adapter
            .config_options()
            .iter()
            .any(|option| option.id.0.as_ref() == "effort");
        let models = adapter.models();
        super::learn(&harness.id, |learned| {
            learned.resume = Some(load_session);
            learned.images = Some(images);
            learned.effort = Some(has_effort);
            learned.error = None;
            if !models.is_empty() {
                learned.models = Some(models);
            }
        });
        Ok(adapter)
    }

    pub(crate) fn native_session_id(&self) -> String {
        self.native_id.clone()
    }

    pub(crate) fn is_alive(&self) -> bool {
        !self.killed.load(Ordering::Acquire) && !self.peer.is_closed()
    }

    pub(crate) async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<StopReason, String> {
        let answer = self
            .peer
            .request(
                "session/prompt",
                json!({"sessionId": self.native_id, "prompt": prompt}),
            )
            .await
            .map_err(|error| error.describe())?;
        // The turn's last updates arrived before its answer; pass them on
        // first.
        self.peer.barrier().await;
        Ok(answer
            .get("stopReason")
            .cloned()
            .and_then(|reason| serde_json::from_value(reason).ok())
            .unwrap_or(StopReason::EndTurn))
    }

    pub(crate) fn cancel(&self) {
        self.peer
            .notify("session/cancel", json!({"sessionId": self.native_id}));
    }

    pub(crate) fn config_options(&self) -> Vec<SessionConfigOption> {
        mapped_options(&lock(&self.state))
    }

    pub(crate) async fn set_config(
        &self,
        id: &str,
        value: Value,
    ) -> Result<Vec<SessionConfigOption>, String> {
        let (theirs, legacy) = {
            let state = lock(&self.state);
            (
                their_id(&state.options, id),
                state.options.is_empty() && state.legacy_models.is_some(),
            )
        };
        if let Some(theirs) = theirs {
            let mut params =
                json!({"sessionId": self.native_id, "configId": theirs, "value": value});
            if value.is_boolean() {
                params["type"] = json!("boolean");
            }
            let answer = self
                .peer
                .request("session/set_config_option", params)
                .await
                .map_err(|error| error.describe())?;
            let options = parse_options(answer.get("configOptions"));
            if !options.is_empty() {
                lock(&self.state).options = options;
            }
            return Ok(self.config_options());
        }
        if legacy && id == "model" {
            let model = value.as_str().unwrap_or_default().to_owned();
            self.peer
                .request(
                    "session/set_model",
                    json!({"sessionId": self.native_id, "modelId": model}),
                )
                .await
                .map_err(|error| error.describe())?;
            if let Some(models) = lock(&self.state).legacy_models.as_mut() {
                models["currentModelId"] = json!(model);
            }
            return Ok(self.config_options());
        }
        Err(format!("unknown config option '{id}'"))
    }

    pub(crate) fn model_and_effort(&self) -> (Option<String>, Option<String>) {
        let options = self.config_options();
        let current = |id: &str| {
            options
                .iter()
                .find(|option| option.id.0.as_ref() == id)
                .map(|option| select_values(option).0)
                .filter(|value| !value.is_empty())
        };
        (current("model"), current("effort"))
    }

    pub(crate) fn models(&self) -> Vec<HarnessModel> {
        let options = self.config_options();
        let find = |id: &str| options.iter().find(|option| option.id.0.as_ref() == id);
        let (default_effort, efforts) = find("effort")
            .map(|option| {
                let (current, choices) = select_values(option);
                (
                    Some(current).filter(|current| !current.is_empty()),
                    choices.into_iter().map(|(value, _)| value).collect(),
                )
            })
            .unwrap_or((None, Vec::new()));
        find("model")
            .map(|option| {
                select_values(option)
                    .1
                    .into_iter()
                    .map(|(id, name)| HarnessModel {
                        id,
                        name,
                        efforts: efforts.clone(),
                        default_effort: default_effort.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        if let Some(process) = &self.process {
            process.kill();
        }
    }
}

/// Handle what the child sends: updates go out, permission requests are
/// asked, anything else is refused.
async fn relay(
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    peer: Arc<RpcPeer>,
    state: Arc<Mutex<State>>,
    out: OutputSender,
) {
    while let Some(message) = incoming.recv().await {
        match message {
            Incoming::Notification { method, params } if method == "session/update" => {
                if lock(&state).muted {
                    continue;
                }
                let Some(update) = params.get("update").cloned() else {
                    continue;
                };
                let update = match serde_json::from_value::<SessionUpdate>(update) {
                    Ok(update) => update,
                    Err(error) => {
                        crate::dbg_log!("harness sent an update RustCode does not know: {error}");
                        continue;
                    }
                };
                let update = match update {
                    SessionUpdate::ConfigOptionUpdate(changed) => {
                        let mut state = lock(&state);
                        state.options = changed.config_options;
                        SessionUpdate::ConfigOptionUpdate(
                            agent_client_protocol::schema::v1::ConfigOptionUpdate::new(
                                mapped_options(&state),
                            ),
                        )
                    }
                    other => other,
                };
                let _ = out.send(Output::Update(update));
            }
            Incoming::Notification { .. } => {}
            Incoming::Barrier(done) => {
                let _ = done.send(());
            }
            Incoming::Request { id, method, params } if method == "session/request_permission" => {
                let request = match serde_json::from_value::<RequestPermissionRequest>(params) {
                    Ok(request) => request,
                    Err(error) => {
                        peer.respond(
                            id,
                            Err(RpcError {
                                code: -32602,
                                message: format!("invalid permission request: {error}"),
                                data: None,
                            }),
                        );
                        continue;
                    }
                };
                let (reply, answer) = oneshot::channel();
                if out.send(Output::Permission { request, reply }).is_err() {
                    let response =
                        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
                    peer.respond(
                        id,
                        Ok(serde_json::to_value(response).unwrap_or(Value::Null)),
                    );
                    continue;
                }
                let peer = Arc::clone(&peer);
                tokio::spawn(async move {
                    let outcome = answer.await.unwrap_or(RequestPermissionOutcome::Cancelled);
                    let response = RequestPermissionResponse::new(outcome);
                    peer.respond(
                        id,
                        Ok(serde_json::to_value(response).unwrap_or(Value::Null)),
                    );
                });
            }
            Incoming::Request { id, method, .. } => {
                peer.respond(id, Err(RpcError::method_not_found(&method)));
            }
        }
    }
}

#[cfg(test)]
#[path = "acp_child_tests.rs"]
mod tests;
