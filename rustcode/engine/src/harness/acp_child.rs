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
    SessionConfigSelectOptions, SessionUpdate, StopReason, TextContent,
};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(90);
/// The client waits this long (10 s) for `_rustcode/steer`; answer before it.
const STEER_TIMEOUT: Duration = Duration::from_secs(8);
/// How many models a listing asks for their own thought levels: the models a
/// person can see, the one a setting or a thread uses, and no more. Each is one
/// round trip, so this is what bounds a listing's cost.
pub(crate) const PROBE_LIMIT: usize = 30;
/// How long a listing spends asking models; the rest of the catalog keeps the
/// union of the levels learned so far (`HarnessModel::efforts_unknown`).
pub(crate) const PROBE_BUDGET: Duration = Duration::from_secs(5);

/// What is left of a `user_message_chunk` once RustCode's own setup block is
/// taken out of it; `None` drops the chunk. Some agents (pi-acp, OpenCode) echo
/// the prompt they were given back as user content, and the block at the head
/// of a first prompt is RustCode's, not the user's. `reminder` is that exact
/// block, `pending` holds a chunk's tail that could still become its start, so
/// an agent that echoes in pieces never leaks half a tag into the thread.
pub(crate) fn strip_runtime_echo(
    text: &str,
    pending: &mut String,
    reminder: &str,
) -> Option<String> {
    let mut seen = std::mem::take(pending);
    seen.push_str(text);
    let kept = seen.replace(reminder, "");
    // Hold back the longest tail that is still the beginning of the block: the
    // next chunk may complete it. Only whole characters can be a tag's start.
    let mut hold = 0;
    for (at, _) in seen.char_indices() {
        let tail = &seen[at..];
        if tail.len() < reminder.len() && reminder.starts_with(tail) {
            hold = tail.len();
        }
    }
    let mut kept = kept;
    if hold > 0 {
        pending.push_str(&seen[seen.len() - hold..]);
        // The held tail is what is left at the end of the text.
        kept.truncate(kept.len().saturating_sub(hold));
    }
    (!kept.trim().is_empty()).then_some(kept)
}

#[derive(Default)]
struct State {
    /// A prompt has gone out in this process.
    prompted: bool,
    /// The child's own config options, ids as the child spells them.
    options: Vec<SessionConfigOption>,
    /// `models` from an agent that predates config options.
    legacy_models: Option<Value>,
    /// Updates are dropped while the child replays history on load: the
    /// client already has the thread's recorded transcript.
    muted: bool,
    /// Claude Code's async tasks that still run.
    background: super::background::Background,
    /// The setup text a first prompt still owes the agent, for one with no
    /// `sessionCapabilities.systemPrompt` (`docs/harnesses.md`).
    reminder: Option<String>,
    /// The exact setup block this child was sent: its echo is stripped
    /// (`strip_runtime_echo`). `None` until a first prompt carries one.
    echo_reminder: Option<String>,
    /// A tail of the echo that may still become the start of that block.
    echo_pending: String,
}

pub(crate) struct AcpAdapter {
    peer: Arc<RpcPeer>,
    process: Option<HarnessProcess>,
    native_id: String,
    state: Arc<Mutex<State>>,
    killed: AtomicBool,
    /// The agent takes `_session/steering`: a message added to the running turn.
    steering: bool,
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

/// `params` with the client system prompt, when the agent takes one.
fn with_system_prompt(mut params: Value, system_prompt: Option<&Value>) -> Value {
    if let Some(prompt) = system_prompt {
        params["systemPrompt"] = prompt.clone();
    }
    params
}

/// Claude Code's own subagent tool, under the name it had (`Task`) and the one
/// it has now (`Agent`). A Claude Code thread RustCode runs delegates through
/// the orchestrator's MCP tools (`start_session`, `fan_out`) instead, so its
/// native subagents do not bypass the thread's delegate UI (#3176).
const CLAUDE_CODE_SUBAGENT_TOOLS: [&str; 2] = ["Task", "Agent"];

/// Add Claude Code's subagent tool to a session's `_meta` as disallowed. Only
/// the keys this names are set, so another `_meta` key a caller carries stays,
/// and a `disallowedTools` list already there is extended rather than replaced.
fn disallow_subagent_tool(meta: &mut Value) {
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    let Some(claude) = meta
        .entry("claudeCode")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    else {
        return;
    };
    let Some(options) = claude
        .entry("options")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    else {
        return;
    };
    let disallowed = options
        .entry("disallowedTools")
        .or_insert_with(|| json!([]));
    let Some(list) = disallowed.as_array_mut() else {
        return;
    };
    for tool in CLAUDE_CODE_SUBAGENT_TOOLS {
        if !list.iter().any(|held| held == tool) {
            list.push(json!(tool));
        }
    }
}

/// `params` with the disallow its harness's session takes, if any. Only
/// `claude-agent-acp` takes one this way: it forwards
/// `_meta.claudeCode.options` to the Claude Agent SDK and merges
/// `disallowedTools` into its own list, which leaves the rustcode MCP server
/// (and with it `start_session` and `fan_out`) alone.
///
/// Without the orchestrator (orchestration off in Settings, or a platform where
/// its socket is not built) there is nothing to delegate through, so Claude
/// Code keeps its own subagent tool rather than losing delegation entirely.
fn with_session_meta(
    mut params: Value,
    harness: &ResolvedHarness,
    options: &StartOptions,
) -> Value {
    if harness.id != "claude-code" || options.orchestrator.is_none() {
        return params;
    }
    if !params.get("_meta").is_some_and(Value::is_object) {
        params["_meta"] = json!({});
    }
    if let Some(meta) = params.get_mut("_meta") {
        disallow_subagent_tool(meta);
    }
    params
}

/// The MCP servers the child's session gets: the orchestrator when the thread has one, then the
/// workspace's own servers, so a Claude Code or OpenCode thread has the servers Settings · MCP
/// shows (docs/configuration.md). A stdio server is `name`/`command`/`args`/`env`, a remote one
/// `type: "http"`/`url`/`headers`, as ACP's `session/new` takes them. A remote server goes only
/// to a child that advertised `mcpCapabilities.http`, so one that cannot take it still starts; a
/// workspace server named like the orchestrator gives way to it.
fn mcp_servers(options: &StartOptions, http: bool) -> Value {
    let mut servers: Vec<Value> = options
        .orchestrator
        .iter()
        .map(crate::orchestration::hub::Launch::acp_server)
        .collect();
    for server in &options.mcp {
        if options.orchestrator.is_some() && server.name == crate::orchestration::hub::SERVER_NAME {
            crate::dbg_log!(
                "harness MCP: '{}' is the orchestrator's name; skipped",
                server.name
            );
            continue;
        }
        if server.url.is_some() && !http {
            crate::dbg_log!(
                "harness MCP: '{}' is remote and the harness takes no HTTP servers; skipped",
                server.name
            );
            continue;
        }
        servers.push(config_server(server));
    }
    Value::Array(servers)
}

fn config_server(server: &crate::config::McpServerConfig) -> Value {
    let pairs = |map: &std::collections::HashMap<String, String>| {
        Value::Array(
            map.iter()
                .map(|(name, value)| serde_json::json!({"name": name, "value": value}))
                .collect(),
        )
    };
    match &server.url {
        Some(url) => serde_json::json!({
            "type": "http",
            "name": server.name,
            "url": url,
            "headers": pairs(&server.headers),
        }),
        None => serde_json::json!({
            "name": server.name,
            "command": server.command,
            "args": server.args,
            "env": pairs(&server.env),
        }),
    }
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
                        // Claude Code's adapter then reports its background work.
                        "_meta": super::background::capabilities_meta(),
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
        let steering = initialized
            .pointer("/_meta/steering/supported")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let cwd_text = cwd.to_string_lossy().into_owned();
        let http = capabilities
            .and_then(|capabilities| capabilities.pointer("/mcpCapabilities/http"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mcp_servers = mcp_servers(&options, http);
        // This harness has no `use_skill`, so it is told where the skills are
        // and which ones the user turned off (#2898). The note is an input:
        // the thread computed it when it started this process.
        let instructions = crate::tools::schema::harness_instructions(
            &harness.name,
            super::client_visualization_level(),
            super::client_smart_ui(),
            options.skills_note.as_deref(),
        );
        // An agent that takes a client system prompt (Claude Code's adapter)
        // gets where it runs and how RustCode draws replies appended to its own.
        let takes_system_prompt = capabilities
            .and_then(|capabilities| capabilities.pointer("/sessionCapabilities/systemPrompt"))
            .is_some_and(|value| !value.is_null());
        let system_prompt = takes_system_prompt.then(|| {
            json!({
                "mode": "append",
                "content": [{ "type": "text", "text": instructions }],
            })
        });

        let mut native = None;
        // Whether this process created the session. A loaded one already holds
        // the setup text in the agent's own history.
        let mut created = false;
        let mut session = Value::Null;
        if let Some(resume) = options.resume.as_ref().filter(|_| load_session) {
            lock(&state).muted = true;
            let loaded = peer
                .request_within(
                    "session/load",
                    with_system_prompt(
                        with_session_meta(
                            json!({"sessionId": resume, "cwd": cwd_text, "mcpServers": mcp_servers}),
                            harness,
                            &options,
                        ),
                        system_prompt.as_ref(),
                    ),
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
            created = true;
            session = peer
                .request_within(
                    "session/new",
                    with_system_prompt(
                        with_session_meta(
                            json!({"cwd": cwd_text, "mcpServers": mcp_servers}),
                            harness,
                            &options,
                        ),
                        system_prompt.as_ref(),
                    ),
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
            // No `systemPrompt` field on `session/new`: the same text rides at
            // the head of the first prompt instead, once, on this new session.
            if created && !takes_system_prompt {
                let reminder = crate::tools::schema::runtime_reminder(&instructions);
                state.echo_reminder = Some(reminder.clone());
                state.reminder = Some(reminder);
            }
        }
        let adapter = Self {
            peer,
            process,
            native_id,
            state,
            killed: AtomicBool::new(false),
            steering,
        };
        // Read before the thread's own choice is applied, so each model's
        // default effort is the harness's and not this thread's.
        let has_effort = adapter
            .config_options()
            .iter()
            .any(|option| option.id.0.as_ref() == "effort");
        let models = adapter.models();
        if let Some(model) = options.model {
            let _ = adapter.set_config("model", json!(model)).await;
        }
        // The model the thread runs now answers with its own thought levels:
        // clamped to them, and kept for the next listing.
        let running = adapter.effort_levels();
        if let Some(effort) = options.effort.as_deref() {
            let offered = running.as_ref().map(|(_, efforts, _)| efforts.as_slice());
            let effort = super::clamp_effort(effort, offered.unwrap_or_default())
                .unwrap_or_else(|| effort.to_owned());
            let _ = adapter.set_config("effort", json!(effort)).await;
        }
        super::learn(&harness.id, |learned| {
            learned.resume = Some(load_session);
            learned.images = Some(images);
            learned.steer = Some(steering);
            learned.effort = Some(has_effort);
            learned.error = None;
            // Only the model this thread runs has its own efforts here, so a
            // list from a listing is left alone; one learned this way is
            // listed again for the others'.
            if learned.models.is_none() && !models.is_empty() {
                learned.listed_for(&harness.id, models, false);
            }
            if let Some((model, efforts, default_effort)) = running {
                learned.set_model_efforts(&model, efforts, default_effort);
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
        let mut prompt = prompt;
        {
            let mut state = lock(&self.state);
            state.prompted = true;
            // An agent with no client system prompt reads RustCode's setup text
            // here instead: once, at the head of the first prompt. The user's
            // own text is already recorded, so this never reaches history.
            if let Some(reminder) = state.reminder.take() {
                prompt.insert(0, ContentBlock::Text(TextContent::new(reminder)));
            }
        }
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
        // A half-read echo does not outlive the turn that carried it.
        lock(&self.state).echo_pending.clear();
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

    /// Add `text` to the running turn through the agent's `_session/steering`
    /// extension. False when the agent has none or no turn is running (it
    /// answers `promptRequired`), so the client sends it as the next prompt.
    pub(crate) async fn steer(&self, prompt: &[ContentBlock]) -> bool {
        if !self.steering {
            return false;
        }
        let answer = self
            .peer
            .request_within(
                "_session/steering",
                json!({
                    "sessionId": self.native_id,
                    "prompt": prompt,
                    "_meta": {"steering": {"idleBehavior": "promptRequired"}},
                }),
                STEER_TIMEOUT,
            )
            .await;
        matches!(answer, Ok(answer) if answer.get("outcome").and_then(Value::as_str) == Some("injected"))
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
        // The levels this model really takes: a level it does not offer (it
        // was picked from the union while the model was still unknown) goes
        // out as the closest one it does.
        let value = match (id, value.as_str()) {
            ("effort", Some(effort)) => {
                let offered = Self::effort_choices(&self.config_options())
                    .map(|(_, offered)| offered)
                    .unwrap_or_default();
                match super::clamp_effort(effort, &offered) {
                    Some(clamped) => json!(clamped),
                    None => value,
                }
            }
            _ => value,
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

    /// Keep the efforts of the model this session runs (`docs/harnesses.md`):
    /// a model a thread started or switched to knows its own levels from now
    /// on, without the listing asking for it.
    pub(crate) fn learn_efforts(&self, harness: &str) {
        if let Some((model, efforts, default_effort)) = self.effort_levels() {
            super::learn_model_efforts(harness, &model, efforts, default_effort);
        }
    }

    /// The effort option's own id, the level the session runs with and the
    /// levels the session's model offers.
    fn effort_choices(options: &[SessionConfigOption]) -> Option<(String, Vec<String>)> {
        let index = (0..options.len()).find(|index| our_id(options, *index) == "effort")?;
        let (current, choices) = select_values(&options[index]);
        Some((
            current,
            choices.into_iter().map(|(value, _)| value).collect(),
        ))
    }

    /// The model the session runs and the efforts it itself offers, as the
    /// harness just reported them: an empty list means it takes no effort.
    fn effort_levels(&self) -> Option<(String, Vec<String>, Option<String>)> {
        let options = self.config_options();
        let model = self.model_and_effort().0?;
        let (efforts, default_effort) = match Self::effort_choices(&options) {
            Some((current, efforts)) => (
                super::order_efforts(efforts),
                Some(current).filter(|current| !current.is_empty()),
            ),
            None => (Vec::new(), None),
        };
        Some((model, efforts, default_effort))
    }

    /// Note `efforts` as the ones `model` offers.
    fn mark_efforts(
        models: &mut [HarnessModel],
        model: &str,
        efforts: Vec<String>,
        default_effort: Option<String>,
    ) {
        if let Some(entry) = models.iter_mut().find(|entry| entry.id == model) {
            entry.efforts = super::order_efforts(efforts);
            entry.default_effort = default_effort;
            entry.efforts_unknown = false;
        }
    }

    /// The catalog as a session that has just started or switched model
    /// describes it: the model it runs with its own efforts, the others with
    /// the union of what the harness is known to offer (`efforts_unknown`).
    pub(crate) fn models(&self) -> Vec<HarnessModel> {
        let options = self.config_options();
        let Some(index) = (0..options.len()).find(|index| our_id(&options, *index) == "model")
        else {
            return Vec::new();
        };
        let (current, choices) = select_values(&options[index]);
        let mut models = choices
            .into_iter()
            .map(|(id, name)| HarnessModel {
                id,
                name,
                efforts: Vec::new(),
                default_effort: None,
                efforts_unknown: true,
            })
            .collect::<Vec<_>>();
        match self.effort_levels() {
            Some((model, efforts, default_effort)) => {
                Self::mark_efforts(&mut models, &model, efforts, default_effort);
            }
            // The model runs with no thought level beside it: it takes none.
            None => Self::mark_efforts(&mut models, &current, Vec::new(), None),
        }
        super::refresh_unknown_efforts(&mut models, self.model_and_effort().1.as_deref());
        models
    }

    /// The catalog with the models the app shows asked for their own efforts.
    ///
    /// An ACP agent reports the thought level option of the model the session
    /// runs, so a model is selected to read its own; the session's model goes
    /// back afterwards. Probing is bounded ([`PROBE_LIMIT`] models,
    /// [`PROBE_BUDGET`]): the model the session runs, then `preferred` (the
    /// model a setting or a thread uses), then the catalog in the order the
    /// picker shows it, skipping what `known` already knows. A model left
    /// unprobed says `efforts_unknown` and carries the union of the known
    /// levels; a thread that picks it learns its own then. The session is the
    /// engine's scratch listing session ([`super::session::list_models`]),
    /// never a user's thread.
    pub(crate) async fn probed_models(
        &self,
        known: &[HarnessModel],
        preferred: &[String],
    ) -> Vec<HarnessModel> {
        self.probe_models(known, preferred, PROBE_LIMIT, PROBE_BUDGET)
            .await
    }

    async fn probe_models(
        &self,
        known: &[HarnessModel],
        preferred: &[String],
        limit: usize,
        budget: std::time::Duration,
    ) -> Vec<HarnessModel> {
        let options = self.config_options();
        let Some(index) = (0..options.len()).find(|index| our_id(&options, *index) == "model")
        else {
            return Vec::new();
        };
        let (current, choices) = select_values(&options[index]);
        let mut models = choices
            .into_iter()
            .map(|(id, name)| HarnessModel {
                id,
                name,
                efforts: Vec::new(),
                default_effort: None,
                efforts_unknown: true,
            })
            .collect::<Vec<_>>();
        // What a listing or a model a thread ran already knows is kept.
        for entry in models.iter_mut() {
            let previous = known
                .iter()
                .find(|previous| previous.id == entry.id && !previous.efforts_unknown);
            if let Some(previous) = previous {
                entry.efforts = super::order_efforts(previous.efforts.clone());
                entry.default_effort = previous.default_effort.clone();
                entry.efforts_unknown = false;
            }
        }
        if let Some((model, efforts, default_effort)) = self.effort_levels() {
            Self::mark_efforts(&mut models, &model, efforts, default_effort);
        }
        let mut want = vec![current.clone()];
        want.extend(preferred.iter().cloned());
        want.extend(models.iter().map(|entry| entry.id.clone()));
        let started = std::time::Instant::now();
        let mut probed = 0;
        for model in want {
            if probed >= limit || started.elapsed() >= budget {
                break;
            }
            let unknown = models
                .iter()
                .find(|entry| entry.id == model)
                .is_some_and(|entry| entry.efforts_unknown && entry.id != current);
            if !unknown {
                continue;
            }
            probed += 1;
            let Ok(options) = self.set_config("model", json!(model)).await else {
                continue;
            };
            let (efforts, default_effort) = match Self::effort_choices(&options) {
                Some((current, efforts)) => (efforts, Some(current)),
                None => (Vec::new(), None),
            };
            Self::mark_efforts(
                &mut models,
                &model,
                efforts,
                default_effort.filter(|effort| !effort.is_empty()),
            );
        }
        if probed > 0 && !current.is_empty() {
            let _ = self.set_config("model", json!(current)).await;
        }
        // The level the session runs with now, for the models still unknown.
        super::refresh_unknown_efforts(&mut models, self.model_and_effort().1.as_deref());
        models
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
                // Anything the agent sends means it is alive, even when it
                // becomes no update of its own (`watchdog`).
                if lock(&state).muted {
                    let _ = out.send(Output::Heartbeat);
                    continue;
                }
                let Some(update) = params.get("update").cloned() else {
                    let _ = out.send(Output::Heartbeat);
                    continue;
                };
                if super::background::Background::is_task_update(&update) {
                    let effects = lock(&state).background.apply(&update);
                    let mut spoke = false;
                    if let Some(update) = effects.update {
                        let _ = out.send(Output::Update(update));
                        spoke = true;
                    }
                    if let Some(activity) = effects.activity {
                        let _ = out.send(Output::Activity(activity));
                        spoke = true;
                    }
                    if !spoke {
                        let _ = out.send(Output::Heartbeat);
                    }
                    continue;
                }
                // Text before any prompt is the agent's startup banner, not a reply: Pi lists its
                // context files and every skill here. It would open the thread and fill handoffs.
                if !lock(&state).prompted
                    && update.get("sessionUpdate").and_then(Value::as_str)
                        == Some("agent_message_chunk")
                {
                    let _ = out.send(Output::Heartbeat);
                    continue;
                }
                let update = match serde_json::from_value::<SessionUpdate>(update) {
                    Ok(update) => update,
                    Err(error) => {
                        crate::dbg_log!("harness sent an update RustCode does not know: {error}");
                        let _ = out.send(Output::Heartbeat);
                        continue;
                    }
                };
                let update = match update {
                    SessionUpdate::UserMessageChunk(mut chunk)
                        if lock(&state).echo_reminder.is_some() =>
                    {
                        let ContentBlock::Text(text) = &mut chunk.content else {
                            let _ =
                                out.send(Output::Update(SessionUpdate::UserMessageChunk(chunk)));
                            continue;
                        };
                        let mut state = lock(&state);
                        let reminder = state.echo_reminder.clone().unwrap_or_default();
                        let kept =
                            strip_runtime_echo(&text.text, &mut state.echo_pending, &reminder);
                        drop(state);
                        match kept {
                            Some(kept) => {
                                text.text = kept;
                                SessionUpdate::UserMessageChunk(chunk)
                            }
                            // All of it was RustCode's block: no user message.
                            None => {
                                let _ = out.send(Output::Heartbeat);
                                continue;
                            }
                        }
                    }
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
                    let response = answer.await.unwrap_or_else(|_| {
                        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
                    });
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
