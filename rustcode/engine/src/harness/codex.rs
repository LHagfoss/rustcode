//! Kind `codex`: drives `codex app-server` (newline JSON-RPC over stdio,
//! protocol v2 as `codex app-server generate-ts` describes it).
//!
//! | Codex | ACP |
//! | --- | --- |
//! | `thread/start`, `thread/resume` | the thread's harness session |
//! | `turn/start` … `turn/completed` | `session/prompt` |
//! | `turn/interrupt` | `session/cancel` |
//! | `turn/steer` | `_rustcode/steer` (live) |
//! | `model/list` | `model` and `effort` config options |
//! | `item/agentMessage/delta` | `agent_message_chunk` |
//! | `item/reasoning/*Delta` | `agent_thought_chunk` |
//! | command, file change, MCP and web items | `tool_call`, `tool_call_update` |
//! | `turn/plan/updated` | `plan` |
//! | `item/*/requestApproval` | `session/request_permission` |
//! | `item/tool/requestUserInput` | a RustCode question (`acp/questions.rs`) |
//! | `thread/tokenUsage/updated` | `usage_update` |
//! | `collabAgentToolCall` | one subagent row per agent it starts (`collab_updates`) |
//! | `mcpServer/elicitation/request` | MCP tool approval: accepted for `rustcode`, else `session/request_permission`; forms with fields declined |

use super::rpc::{Incoming, RpcError, RpcPeer};
use super::session::{Output, OutputSender, StartOptions};
use super::{HarnessModel, ResolvedHarness, process::HarnessProcess};
use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, PermissionOption, PermissionOptionKind, Plan, PlanEntry,
    PlanEntryPriority, PlanEntryStatus, RequestPermissionOutcome, RequestPermissionRequest,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionUpdate,
    StopReason, TextContent, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind, UsageUpdate,
};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// Tool output kept on a tool card; the harness has the full text.
const TOOL_OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexModel {
    id: String,
    name: String,
    efforts: Vec<String>,
    default_effort: Option<String>,
    is_default: bool,
}

#[derive(Debug)]
enum TurnEnd {
    Completed,
    Interrupted,
    Failed(String),
}

#[derive(Default)]
struct State {
    /// The RustCode thread this Codex thread runs for, so a compaction it
    /// reports can be stored against it.
    thread_id: Option<String>,
    /// Codex’s thread id, distinct from RustCode’s id; child events name another thread.
    codex_thread_id: Option<String>,
    models: Vec<CodexModel>,
    model: Option<String>,
    effort: Option<String>,
    turn_id: Option<String>,
    waiter: Option<oneshot::Sender<TurnEnd>>,
    cancel_requested: bool,
    /// Items whose text streamed as deltas, so their completion is not sent
    /// again.
    streamed: HashSet<String>,
    summarized: HashSet<String>,
    last_message_item: Option<String>,
    last_error: Option<String>,
    /// A `/compact` is running: its completed `contextCompaction` item ends the
    /// wait. Codex's own auto-compaction inside a turn does not.
    compacting: bool,
    /// Whether the client runs this thread with Full access, from the last prompt that said so.
    /// Full access runs Codex without its sandbox; anything else keeps the workspace sandbox.
    full_access: Option<bool>,
    /// Which policy Codex holds for this thread now, so a turn sends it only when it changes.
    sent_full_access: Option<bool>,
    /// Codex's own subagents, by the Codex thread each runs in, which is how
    /// every notification about it names it.
    collab: HashMap<String, CollabAgent>,
}

/// One of Codex's own subagents (`collabAgentToolCall`) as the transcript's
/// child row shows it: what it was asked, and the state it last reported, so a
/// state repeated by a later call is not sent again.
#[derive(Debug)]
struct CollabAgent {
    /// The row's tool call id: the call that started the agent, its thread id.
    call_id: String,
    /// The first line of its task, for the row and for the titles of `wait` and
    /// `closeAgent` steps.
    task: String,
    /// Codex's own name for its state (`running`, `completed`, …).
    status: Option<String>,
    /// The message that came with that state: usually how the agent finished.
    message: Option<String>,
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|error| error.into_inner())
}

pub(crate) struct CodexAdapter {
    peer: Arc<RpcPeer>,
    process: Option<HarnessProcess>,
    thread_id: String,
    state: Arc<Mutex<State>>,
    killed: AtomicBool,
}

fn text_chunk(text: impl Into<String>) -> ContentChunk {
    ContentChunk::new(ContentBlock::Text(TextContent::new(text.into())))
}

fn str_field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

fn parse_models(answer: &Value) -> Vec<CodexModel> {
    answer
        .get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter(|model| {
                    !model
                        .get("hidden")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|model| {
                    let id = str_field(model, "id").or_else(|| str_field(model, "model"))?;
                    Some(CodexModel {
                        id: id.to_owned(),
                        name: str_field(model, "displayName").unwrap_or(id).to_owned(),
                        efforts: model
                            .get("supportedReasoningEfforts")
                            .and_then(Value::as_array)
                            .map(|efforts| {
                                efforts
                                    .iter()
                                    .filter_map(|effort| str_field(effort, "reasoningEffort"))
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        default_effort: str_field(model, "defaultReasoningEffort")
                            .map(str::to_owned),
                        is_default: model
                            .get("isDefault")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The sandbox and approval policy for `thread/start` and `thread/resume`. RustCode answers
/// approvals itself by the thread's access level (Ask, Auto-edit, Full access), so Codex writes
/// in the workspace and asks for anything more. Left to Codex, a folder it does not trust runs
/// read-only, and the agent cannot edit the project the thread was opened in.
fn set_policy(params: &mut Value) {
    params["sandbox"] = json!("workspace-write");
    params["approvalPolicy"] = json!("on-request");
}

/// The policy a turn runs under when the client's access level is known. A turn's policy stays
/// with the thread, so each access level is sent explicitly: Full access leaves the sandbox
/// (`dangerFullAccess`, nothing to ask for), anything else is `set_policy`'s again.
fn access_policy(full_access: bool) -> (Value, &'static str) {
    if full_access {
        (json!({"type": "dangerFullAccess"}), "never")
    } else {
        (json!({"type": "workspaceWrite"}), "on-request")
    }
}

/// The `config` overrides for `thread/start` and `thread/resume`: the orchestrator when the thread
/// has one, and the workspace's own MCP servers, so a Codex thread runs the servers Settings · MCP
/// shows. Codex takes a stdio server as `command`/`args`/`env` and a remote one as
/// `url`/`http_headers` (docs/configuration.md).
fn codex_config(options: &StartOptions) -> Option<Value> {
    let mut config = match &options.orchestrator {
        Some(launch) => launch.codex_config(),
        None => serde_json::Map::new().into(),
    };
    for server in &options.mcp {
        let entry = match &server.url {
            Some(url) => serde_json::json!({ "url": url, "http_headers": server.headers }),
            None => serde_json::json!({
                "command": server.command,
                "args": server.args,
                "env": server.env,
            }),
        };
        config[format!("mcp_servers.{}", server.name)] = entry;
    }
    // Nothing to override: leave the key out rather than send an empty object.
    let empty = config.as_object().is_none_or(|map| map.is_empty());
    (!empty).then_some(config)
}

/// ACP prompt blocks as Codex input items.
fn input_items(prompt: &[ContentBlock]) -> Vec<Value> {
    let mut items = Vec::new();
    for block in prompt {
        match block {
            ContentBlock::Text(text) => {
                items.push(json!({"type": "text", "text": text.text, "text_elements": []}));
            }
            ContentBlock::Image(image) => items.push(json!({
                "type": "image",
                "url": format!("data:{};base64,{}", image.mime_type, image.data),
            })),
            ContentBlock::ResourceLink(link) => {
                let text = match link.uri.strip_prefix("file://") {
                    Some(path) => path.to_owned(),
                    None => link.uri.clone(),
                };
                items.push(json!({"type": "text", "text": text, "text_elements": []}));
            }
            other => {
                let text = super::session::prompt_text(std::slice::from_ref(other));
                if !text.is_empty() {
                    items.push(json!({"type": "text", "text": text, "text_elements": []}));
                }
            }
        }
    }
    items
}

impl CodexAdapter {
    pub(crate) async fn connect(
        harness: &ResolvedHarness,
        peer: Arc<RpcPeer>,
        incoming: mpsc::UnboundedReceiver<Incoming>,
        process: Option<HarnessProcess>,
        cwd: &Path,
        options: StartOptions,
        out: OutputSender,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State {
            thread_id: options.thread_id.clone(),
            ..State::default()
        }));
        tokio::spawn(relay(incoming, Arc::clone(&peer), Arc::clone(&state), out));
        let describe = |error: RpcError| format!("{}: {}", harness.name, error.describe());
        peer.request_within(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "rustcode",
                    "title": "RustCode",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": false, "requestAttestation": false },
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await
        .map_err(describe)?;
        peer.notify("initialized", Value::Null);

        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let answer = peer
                .request_within("model/list", json!({"cursor": cursor}), HANDSHAKE_TIMEOUT)
                .await
                .map_err(describe)?;
            models.extend(parse_models(&answer));
            cursor = str_field(&answer, "nextCursor").map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }

        let cwd_text = cwd.to_string_lossy().into_owned();
        // Where it runs and how replies are formatted for RustCode's chat
        // (docs/output-style.md), where the skills are, and what Codex's own
        // sandbox denies on this platform: Codex appends it to its own
        // instructions as a developer message. The note is an input, computed
        // when the thread started this process.
        let output_style = crate::tools::schema::harness_instructions(
            &harness.name,
            options.client_display.visualization_level,
            options.client_display.smart_ui,
            options.skills_note.as_deref(),
            crate::tools::schema::codex_windows_sandbox_note(),
        );
        let mut thread = None;
        if let Some(resume) = &options.resume {
            let mut params = json!({
                "threadId": resume,
                "cwd": cwd_text,
                "excludeTurns": true,
                "developerInstructions": output_style,
            });
            set_policy(&mut params);
            if let Some(model) = &options.model {
                params["model"] = json!(model);
            }
            if let Some(config) = codex_config(&options) {
                params["config"] = config;
            }
            match peer
                .request_within("thread/resume", params, HANDSHAKE_TIMEOUT)
                .await
            {
                Ok(answer) => thread = Some(answer),
                Err(error) => crate::dbg_log!(
                    "harness {}: thread/resume {resume} failed ({}); starting a new thread",
                    harness.id,
                    error.describe()
                ),
            }
        }
        let thread = match thread {
            Some(thread) => thread,
            None => {
                let mut params = json!({"cwd": cwd_text, "developerInstructions": output_style});
                set_policy(&mut params);
                if let Some(model) = &options.model {
                    params["model"] = json!(model);
                }
                if let Some(config) = codex_config(&options) {
                    params["config"] = config;
                }
                peer.request_within("thread/start", params, HANDSHAKE_TIMEOUT)
                    .await
                    .map_err(describe)?
            }
        };
        let thread_id = thread
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{} returned no thread id", harness.name))?
            .to_owned();
        {
            let mut state = lock(&state);
            state.codex_thread_id = Some(thread_id.clone());
            state.models = models;
            state.model = options
                .model
                .clone()
                .or_else(|| str_field(&thread, "model").map(str::to_owned));
            state.effort = options
                .effort
                .clone()
                .or_else(|| str_field(&thread, "reasoningEffort").map(str::to_owned));
        }
        let adapter = Self {
            peer,
            process,
            thread_id,
            state,
            killed: AtomicBool::new(false),
        };
        let models = adapter.models();
        super::learn(&harness.id, |learned| {
            learned.error = None;
            if !models.is_empty() {
                learned.listed(&harness.id, models);
            }
        });
        Ok(adapter)
    }

    pub(crate) fn native_session_id(&self) -> String {
        self.thread_id.clone()
    }

    pub(crate) fn is_alive(&self) -> bool {
        !self.killed.load(Ordering::Acquire) && !self.peer.is_closed()
    }

    /// The client's access level for this thread, applied from its next turn.
    pub(crate) fn set_full_access(&self, full: bool) {
        lock(&self.state).full_access = Some(full);
    }

    pub(crate) async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<StopReason, String> {
        let (sender, receiver) = oneshot::channel();
        let (model, effort, access) = {
            let mut state = lock(&self.state);
            state.waiter = Some(sender);
            state.turn_id = None;
            // `cancel_requested` stays: a cancel can arrive before the turn
            // has started, and is applied as soon as it has an id.
            state.last_error = None;
            state.last_message_item = None;
            // The policy goes out when the client's access level is known and is not the one
            // Codex already holds (a thread starts in `set_policy`'s workspace sandbox).
            let access = state
                .full_access
                .filter(|full| state.sent_full_access.unwrap_or(false) != *full);
            (state.model.clone(), state.effort.clone(), access)
        };
        // `/compact` alone is Codex's own compaction, not a message for the model
        // (`thread/compact/start`); its `contextCompaction`
        // item becomes the chat's compaction card.
        let compact = is_compact_command(&prompt);
        lock(&self.state).compacting = compact;
        let (method, params) = if compact {
            ("thread/compact/start", json!({"threadId": self.thread_id}))
        } else {
            let mut params = json!({"threadId": self.thread_id, "input": input_items(&prompt)});
            if let Some(model) = model {
                params["model"] = json!(model);
            }
            if let Some(effort) = effort {
                params["effort"] = json!(effort);
            }
            if let Some(full) = access {
                let (sandbox, approval) = access_policy(full);
                params["sandboxPolicy"] = sandbox;
                params["approvalPolicy"] = json!(approval);
            }
            ("turn/start", params)
        };
        let started = match self.peer.request(method, params).await {
            Ok(started) => started,
            Err(error) => {
                let mut state = lock(&self.state);
                state.waiter = None;
                state.cancel_requested = false;
                return Err(format!("Codex: {}", error.describe()));
            }
        };
        if let (Some(full), false) = (access, compact) {
            lock(&self.state).sent_full_access = Some(full);
        }
        let turn_id = started
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let interrupt = {
            let mut state = lock(&self.state);
            state.turn_id = turn_id.clone();
            state.cancel_requested
        };
        if interrupt {
            self.interrupt();
        }
        match receiver.await {
            Ok(TurnEnd::Completed) => Ok(StopReason::EndTurn),
            Ok(TurnEnd::Interrupted) => Ok(StopReason::Cancelled),
            Ok(TurnEnd::Failed(message)) => Err(message),
            Err(_) => Err("Codex exited during the turn".to_owned()),
        }
    }

    fn interrupt(&self) {
        let Some(turn_id) = lock(&self.state).turn_id.clone() else {
            return;
        };
        let peer = Arc::clone(&self.peer);
        let thread_id = self.thread_id.clone();
        tokio::spawn(async move {
            if let Err(error) = peer
                .request(
                    "turn/interrupt",
                    json!({"threadId": thread_id, "turnId": turn_id}),
                )
                .await
            {
                crate::dbg_log!("Codex turn/interrupt failed: {}", error.describe());
            }
        });
    }

    pub(crate) fn cancel(&self) {
        let known = {
            let mut state = lock(&self.state);
            state.cancel_requested = true;
            state.turn_id.is_some()
        };
        if known {
            self.interrupt();
        }
    }

    pub(crate) async fn steer(&self, prompt: &[ContentBlock]) -> bool {
        let Some(turn_id) = lock(&self.state).turn_id.clone() else {
            return false;
        };
        let input = input_items(prompt);
        if input.is_empty() {
            return false;
        }
        self.peer
            .request(
                "turn/steer",
                json!({
                    "threadId": self.thread_id,
                    "input": input,
                    "expectedTurnId": turn_id,
                }),
            )
            .await
            .is_ok()
    }

    pub(crate) fn config_options(&self) -> Vec<SessionConfigOption> {
        config_options(&lock(&self.state))
    }

    pub(crate) fn set_config(
        &self,
        id: &str,
        value: Value,
    ) -> Result<Vec<SessionConfigOption>, String> {
        let value = value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("expected a value id for '{id}'"))?;
        let mut state = lock(&self.state);
        match id {
            "model" => {
                let model = state.models.iter().find(|model| model.id == value).cloned();
                if model.is_none() && !state.models.is_empty() {
                    return Err(format!("unknown Codex model '{value}'"));
                }
                if let (Some(model), Some(effort)) = (&model, &state.effort)
                    && !model.efforts.contains(effort)
                {
                    state.effort = model.default_effort.clone();
                }
                state.model = Some(value);
            }
            "effort" => {
                let efforts = current_model(&state)
                    .map(|model| model.efforts.clone())
                    .unwrap_or_default();
                if !efforts.is_empty() && !efforts.contains(&value) {
                    return Err(format!("unknown effort '{value}'"));
                }
                state.effort = Some(value);
            }
            other => return Err(format!("unknown config option '{other}'")),
        }
        Ok(config_options(&state))
    }

    pub(crate) fn model_and_effort(&self) -> (Option<String>, Option<String>) {
        let state = lock(&self.state);
        (state.model.clone(), state.effort.clone())
    }

    pub(crate) fn models(&self) -> Vec<HarnessModel> {
        lock(&self.state)
            .models
            .iter()
            .map(|model| HarnessModel {
                id: model.id.clone(),
                name: model.name.clone(),
                efforts: model.efforts.clone(),
                default_effort: model.default_effort.clone(),
                efforts_unknown: false,
            })
            .collect()
    }

    pub(crate) fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        if let Some(waiter) = lock(&self.state).waiter.take() {
            let _ = waiter.send(TurnEnd::Interrupted);
        }
        if let Some(process) = &self.process {
            process.kill();
        }
    }
}

fn current_model(state: &State) -> Option<&CodexModel> {
    match &state.model {
        Some(id) => state.models.iter().find(|model| &model.id == id),
        None => state.models.iter().find(|model| model.is_default),
    }
}

fn config_options(state: &State) -> Vec<SessionConfigOption> {
    let current = state
        .model
        .clone()
        .or_else(|| current_model(state).map(|model| model.id.clone()))
        .unwrap_or_default();
    let mut choices = state
        .models
        .iter()
        .map(|model| SessionConfigSelectOption::new(model.id.clone(), model.name.clone()))
        .collect::<Vec<_>>();
    if !current.is_empty() && !state.models.iter().any(|model| model.id == current) {
        choices.push(SessionConfigSelectOption::new(
            current.clone(),
            current.clone(),
        ));
    }
    let mut options = Vec::new();
    if !choices.is_empty() {
        options.push(
            SessionConfigOption::select("model", "Model", current, choices)
                .category(SessionConfigOptionCategory::Model)
                .description("Codex model"),
        );
    }
    if let Some(model) = current_model(state)
        && !model.efforts.is_empty()
    {
        let effort = state
            .effort
            .clone()
            .filter(|effort| model.efforts.contains(effort))
            .or_else(|| model.default_effort.clone())
            .unwrap_or_else(|| model.efforts[0].clone());
        let choices = model
            .efforts
            .iter()
            .map(|effort| SessionConfigSelectOption::new(effort.clone(), effort.clone()))
            .collect::<Vec<_>>();
        options.push(
            SessionConfigOption::select("effort", "Effort", effort, choices)
                .category(SessionConfigOptionCategory::ThoughtLevel)
                .description("Reasoning effort"),
        );
    }
    options
}

/// Whether a prompt is exactly `/compact`: a request to compact the thread.
fn is_compact_command(prompt: &[ContentBlock]) -> bool {
    let text: String = prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    prompt
        .iter()
        .all(|block| matches!(block, ContentBlock::Text(_)))
        && text.trim() == "/compact"
}

fn truncate(text: &str) -> String {
    if text.len() <= TOOL_OUTPUT_LIMIT {
        return text.to_owned();
    }
    let mut end = TOOL_OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (truncated)", &text[..end])
}

fn file_paths(item: &Value) -> Vec<String> {
    item.get("changes")
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .filter_map(|change| str_field(change, "path"))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The first line of a subagent's task, short enough for a row title.
fn task_line(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let short: String = line.chars().take(80).collect();
    if short.len() < line.len() {
        format!("{short}…")
    } else {
        short
    }
}

/// The agents a collab call names, by the Codex thread each runs in.
fn receivers(item: &Value) -> Vec<String> {
    item.get("receiverThreadIds")
        .and_then(Value::as_array)
        .map(|threads| {
            threads
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The state Codex reports for the agent in `thread`, as it names it, with the
/// message it gave for it.
fn agent_state(item: &Value, thread: &str) -> Option<(String, Option<String>)> {
    let state = item.get("agentsStates")?.get(thread)?;
    let status = str_field(state, "status")?.to_owned();
    let message = str_field(state, "message")
        .filter(|message| !message.trim().is_empty())
        .map(str::to_owned);
    Some((status, message))
}

/// Codex's state for an agent as the tool status a client draws, and the word
/// its row's title adds. `pendingInit` and `running` are the only live states.
fn agent_status(status: &str) -> (ToolCallStatus, &'static str) {
    match status {
        "pendingInit" => (ToolCallStatus::InProgress, "starting"),
        "running" => (ToolCallStatus::InProgress, "running"),
        "completed" => (ToolCallStatus::Completed, "finished"),
        "interrupted" => (ToolCallStatus::Completed, "interrupted"),
        "shutdown" => (ToolCallStatus::Completed, "closed"),
        "errored" | "notFound" => (ToolCallStatus::Failed, "failed"),
        _ => (ToolCallStatus::InProgress, "running"),
    }
}

/// Whether an agent's state is one it does not leave on its own.
fn agent_finished(agent: &CollabAgent) -> bool {
    agent
        .status
        .as_deref()
        .is_some_and(|status| status != "running" && status != "pendingInit")
}

/// The `_meta` a client reads to draw a call as a subagent row: the adapter's
/// tool name for one (`Agent`), and the row of the agent a call was made from.
/// Claude Code's adapter sends both fields, so a client that draws its
/// subagents draws these too; Codex has no adapter of its own, so the fields
/// are filled here.
fn agent_meta(parent: Option<&str>) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("toolName".to_owned(), json!("Agent"));
    if let Some(parent) = parent {
        fields.insert("parentToolUseId".to_owned(), json!(parent));
    }
    let mut meta = Map::new();
    meta.insert("claudeCode".to_owned(), Value::Object(fields));
    meta
}

/// The `_meta` that folds a call under the row of the agent that made it.
fn step_meta(parent: &str) -> Map<String, Value> {
    let mut meta = Map::new();
    meta.insert("claudeCode".to_owned(), json!({"parentToolUseId": parent}));
    meta
}

/// The transcript row of one of Codex's subagents: a tool call the client
/// draws as a subagent, named for its task, with the task and the model it
/// runs on as its input.
fn agent_row(
    agent: &CollabAgent,
    prompt: &str,
    model: Option<&str>,
    parent: Option<&str>,
) -> ToolCall {
    let (status, _) = agent
        .status
        .as_deref()
        .map(agent_status)
        .unwrap_or((ToolCallStatus::InProgress, "starting"));
    let title = if agent.task.is_empty() {
        "Started agent".to_owned()
    } else {
        format!("Started agent: {}", agent.task)
    };
    ToolCall::new(agent.call_id.clone(), title)
        .kind(ToolKind::Other)
        .status(status)
        .raw_input(json!({
            "subagent_type": "Agent",
            "description": agent.task,
            "prompt": prompt,
            "model": model,
        }))
        .meta(agent_meta(parent))
}

/// The update an agent's row takes from the state Codex reports for it.
fn agent_update(agent: &CollabAgent, status: &str, message: Option<&str>) -> ToolCallUpdate {
    let (tool_status, word) = agent_status(status);
    let mut fields = ToolCallUpdateFields::new().status(tool_status);
    // A finished agent says how it ended; a running one keeps its title.
    if tool_status != ToolCallStatus::InProgress {
        fields = fields.title(format!("Agent {word}: {}", agent.task));
    }
    // Its last message is what it has to show for the turn.
    if let Some(message) = message {
        fields = fields.content(vec![ToolCallContent::from(ContentBlock::Text(
            TextContent::new(truncate(message)),
        ))]);
    }
    ToolCallUpdate::new(agent.call_id.clone(), fields)
}

/// The call's own step in the transcript, for the calls that are not an
/// agent's own row: `Waiting for 2 agents`, `Closed agent: <task>`.
fn step_call(item: &Value, id: &str, title: String) -> ToolCall {
    ToolCall::new(id.to_owned(), title)
        .kind(ToolKind::Other)
        .status(ToolCallStatus::InProgress)
        .raw_input(json!({
            "tool": item.get("tool"),
            "prompt": item.get("prompt"),
            "model": item.get("model"),
            "agents": item.get("receiverThreadIds"),
        }))
}

/// One agent's title (`Closed agent: <task>`) or a count (`Closed 2 agents`).
fn one_or_many(one: &str, many: &str, count: usize, task: &str) -> String {
    if count > 1 {
        format!("{many} {count} agents")
    } else if task.is_empty() {
        one.to_owned()
    } else {
        format!("{one}: {task}")
    }
}

/// What a collab call did, as its step's title.
fn collab_step_title(
    tool: &str,
    receivers: &[String],
    agents: &HashMap<String, CollabAgent>,
    completed: bool,
) -> String {
    // A wait or a close that returns says who it finished, not what it asked.
    if completed && matches!(tool, "wait" | "closeAgent" | "interruptAgent") {
        let ended = receivers
            .iter()
            .filter_map(|thread| agents.get(thread))
            .filter(|agent| agent_finished(agent))
            .collect::<Vec<_>>();
        match ended.len() {
            1 => {
                let agent = ended[0];
                let (_, word) = agent_status(agent.status.as_deref().unwrap_or_default());
                return format!("Agent {word}: {}", agent.task);
            }
            count if count > 1 => return format!("{count} agents finished"),
            _ => {}
        }
    }
    let count = receivers.len();
    let task = receivers
        .iter()
        .find_map(|thread| agents.get(thread))
        .map(|agent| agent.task.as_str())
        .unwrap_or_default();
    match tool {
        "spawnAgent" => one_or_many("Started agent", "Started", count, task),
        "sendInput" | "sendMessage" | "followupTask" => {
            one_or_many("Messaged agent", "Messaged", count, task)
        }
        "resumeAgent" => one_or_many("Resumed agent", "Resumed", count, task),
        "wait" if count == 0 => "Waiting for agents".to_owned(),
        "wait" => one_or_many("Waiting for agent", "Waiting for", count, task),
        "closeAgent" => one_or_many("Closed agent", "Closed", count, task),
        "interruptAgent" => one_or_many("Interrupted agent", "Interrupted", count, task),
        "listAgents" => "Listed agents".to_owned(),
        _ => "Subagent".to_owned(),
    }
}

/// The row of the Codex subagent whose thread a notification names, when the
/// notification is one of its own.
fn agent_row_of_call(state: &Mutex<State>, params: &Value) -> Option<String> {
    let thread = str_field(params, "threadId")?;
    lock(state)
        .collab
        .get(thread)
        .map(|agent| agent.call_id.clone())
}

/// Whether a notification belongs to one of Codex's own subagents rather than
/// the thread: its turn ending is not the thread's turn ending.
fn is_agent_thread(state: &Mutex<State>, params: &Value) -> bool {
    str_field(params, "threadId").is_some_and(|thread| {
        let state = lock(state);
        state.collab.contains_key(thread)
            || state
                .codex_thread_id
                .as_deref()
                .is_some_and(|root| root != thread)
    })
}

/// A tool call an agent made, marked with that agent's row so the client folds
/// its steps under it. Any other call is left as it is.
fn fold_under_agent(state: &Mutex<State>, params: &Value, call: ToolCall) -> ToolCall {
    match agent_row_of_call(state, params) {
        Some(parent) => call.meta(step_meta(&parent)),
        None => call,
    }
}

fn fold_under_agent_update(
    state: &Mutex<State>,
    params: &Value,
    update: ToolCallUpdate,
) -> ToolCallUpdate {
    match agent_row_of_call(state, params) {
        Some(parent) => update.meta(step_meta(&parent)),
        None => update,
    }
}

/// The updates one `collabAgentToolCall` item makes: a row for each agent a
/// `spawnAgent` started, the state every call reports for the agents it names,
/// and the call's own step.
fn collab_updates(
    state: &Mutex<State>,
    params: &Value,
    item: &Value,
    completed: bool,
) -> Vec<SessionUpdate> {
    let tool = str_field(item, "tool").unwrap_or_default();
    let item_id = str_field(item, "id").unwrap_or_default().to_owned();
    let receivers = receivers(item);
    let mut guard = lock(state);
    // An agent that starts its own subagents: their rows fold under its row.
    let parent = str_field(params, "threadId")
        .and_then(|thread| guard.collab.get(thread))
        .map(|agent| agent.call_id.clone());
    let mut updates = Vec::new();
    if tool == "spawnAgent" {
        let prompt = str_field(item, "prompt").unwrap_or_default().to_owned();
        let task = task_line(&prompt);
        let model = str_field(item, "model").map(str::to_owned);
        for thread in &receivers {
            if guard.collab.contains_key(thread) {
                continue;
            }
            let (status, message) = match agent_state(item, thread) {
                Some((status, message)) => (Some(status), message),
                None => (None, None),
            };
            let agent = CollabAgent {
                call_id: format!("{item_id}:{thread}"),
                task: task.clone(),
                status,
                message,
            };
            updates.push(SessionUpdate::ToolCall(agent_row(
                &agent,
                &prompt,
                model.as_deref(),
                parent.as_deref(),
            )));
            guard.collab.insert(thread.clone(), agent);
        }
    }
    // Every call reports the state of the agents it names, so their rows move
    // on a wait, a message or a close as well as on the agent's own events.
    for thread in &receivers {
        let Some(agent) = guard.collab.get_mut(thread) else {
            continue;
        };
        let Some((status, message)) = agent_state(item, thread) else {
            continue;
        };
        if agent.status.as_deref() == Some(status.as_str()) && agent.message == message {
            continue;
        }
        agent.status = Some(status.clone());
        agent.message = message.clone();
        updates.push(SessionUpdate::ToolCallUpdate(agent_update(
            agent,
            &status,
            message.as_deref(),
        )));
    }
    // The call's own step. A spawn that started agents draws their rows instead,
    // so its step would say the same thing twice.
    if !(tool == "spawnAgent" && !receivers.is_empty()) {
        let title = collab_step_title(tool, &receivers, &guard.collab, completed);
        updates.push(if completed {
            let status = match str_field(item, "status").unwrap_or("completed") {
                "failed" => ToolCallStatus::Failed,
                _ => ToolCallStatus::Completed,
            };
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                item_id,
                ToolCallUpdateFields::new().status(status).title(title),
            ))
        } else {
            SessionUpdate::ToolCall(step_call(item, &item_id, title))
        });
    }
    updates
}

/// One of Codex's own subagents finishing its turn: its row ends with it. The
/// thread's own turn is not over, so the turn does not settle (`settle`).
fn agent_turn_end(state: &Mutex<State>, params: &Value) -> Vec<SessionUpdate> {
    let Some(thread) = str_field(params, "threadId") else {
        return Vec::new();
    };
    let (status, message) = match params.pointer("/turn/status").and_then(Value::as_str) {
        Some("failed") => (
            "errored",
            params
                .pointer("/turn/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        Some("interrupted") => ("interrupted", None),
        _ => ("completed", None),
    };
    let mut guard = lock(state);
    let Some(agent) = guard.collab.get_mut(thread) else {
        return Vec::new();
    };
    if agent.status.as_deref() == Some(status) && agent.message == message {
        return Vec::new();
    }
    agent.status = Some(status.to_owned());
    agent.message = message.clone();
    vec![SessionUpdate::ToolCallUpdate(agent_update(
        agent,
        status,
        message.as_deref(),
    ))]
}

/// The tool card a Codex item starts, if it is a tool.
fn tool_call_for(item: &Value) -> Option<ToolCall> {
    let id = str_field(item, "id")?.to_owned();
    let call = match str_field(item, "type")? {
        "commandExecution" => {
            let command = str_field(item, "command").unwrap_or("command");
            ToolCall::new(id, command)
                .kind(ToolKind::Execute)
                .raw_input(json!({"command": command, "cwd": item.get("cwd")}))
        }
        "fileChange" => {
            let paths = file_paths(item);
            let title = if paths.is_empty() {
                "Edit files".to_owned()
            } else {
                format!("Edit {}", paths.join(", "))
            };
            ToolCall::new(id, title)
                .kind(ToolKind::Edit)
                .locations(paths.into_iter().map(ToolCallLocation::new).collect())
                .raw_input(json!({"changes": item.get("changes")}))
        }
        "mcpToolCall" => {
            let title = format!(
                "{}.{}",
                str_field(item, "server").unwrap_or("mcp"),
                str_field(item, "tool").unwrap_or("tool")
            );
            ToolCall::new(id, title)
                .kind(ToolKind::Other)
                .raw_input(item.get("arguments").cloned().unwrap_or(Value::Null))
        }
        "dynamicToolCall" => ToolCall::new(id, str_field(item, "tool").unwrap_or("tool"))
            .kind(ToolKind::Other)
            .raw_input(item.get("arguments").cloned().unwrap_or(Value::Null)),
        "webSearch" => {
            let query = str_field(item, "query").unwrap_or_default();
            ToolCall::new(id, format!("Web search {query}").trim().to_owned())
                .kind(ToolKind::Fetch)
                .raw_input(item.clone())
        }
        "imageView" => ToolCall::new(
            id,
            format!("View {}", str_field(item, "path").unwrap_or("image")),
        )
        .kind(ToolKind::Read),
        _ => return None,
    };
    Some(call.status(ToolCallStatus::InProgress))
}

/// The update a finished Codex tool item makes to its card.
fn tool_update_for(item: &Value) -> Option<ToolCallUpdate> {
    let id = str_field(item, "id")?.to_owned();
    let status = str_field(item, "status").unwrap_or("completed");
    let failed = matches!(status, "failed" | "declined");
    let mut fields = ToolCallUpdateFields::new();
    match str_field(item, "type")? {
        "commandExecution" => {
            let exit_code = item.get("exitCode").and_then(Value::as_i64);
            let output = truncate(str_field(item, "aggregatedOutput").unwrap_or_default());
            let failed = failed || exit_code.is_some_and(|code| code != 0);
            fields = fields
                .status(if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                })
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new(output.clone()),
                ))])
                .raw_output(json!({"exitCode": exit_code, "output": output, "status": status}));
        }
        "fileChange" => {
            let diffs = item
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(|change| {
                            let path = str_field(change, "path")?;
                            let diff = str_field(change, "diff").unwrap_or_default();
                            Some(format!("{path}\n```diff\n{}\n```", diff.trim_end()))
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .unwrap_or_default();
            fields = fields
                .status(if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                })
                .content(vec![ToolCallContent::from(ContentBlock::Text(
                    TextContent::new(truncate(&diffs)),
                ))])
                .raw_output(json!({"status": status, "changes": item.get("changes")}));
        }
        "mcpToolCall" | "dynamicToolCall" => {
            let failed = failed
                || item.get("error").is_some_and(|error| !error.is_null())
                || item.get("success").and_then(Value::as_bool) == Some(false);
            let result = item
                .get("result")
                .or_else(|| item.get("contentItems"))
                .or_else(|| item.get("error"))
                .cloned()
                .unwrap_or(Value::Null);
            fields = fields
                .status(if failed {
                    ToolCallStatus::Failed
                } else {
                    ToolCallStatus::Completed
                })
                .raw_output(result);
        }
        "webSearch" | "imageView" => {
            fields = fields.status(if failed {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            });
        }
        _ => return None,
    }
    Some(ToolCallUpdate::new(id, fields))
}

fn plan_for(params: &Value) -> Option<Plan> {
    let steps = params.get("plan")?.as_array()?;
    let entries = steps
        .iter()
        .filter_map(|step| {
            let content = str_field(step, "step")?;
            let status = match str_field(step, "status") {
                Some("completed") => PlanEntryStatus::Completed,
                Some("inProgress") => PlanEntryStatus::InProgress,
                _ => PlanEntryStatus::Pending,
            };
            Some(PlanEntry::new(content, PlanEntryPriority::Medium, status))
        })
        .collect();
    Some(Plan::new(entries))
}

/// Codex's token usage as the `usage_update` the built-in harness sends:
/// `used` is the latest request's input, `size` the model's window (0 when
/// Codex does not say), and the thread's totals ride in `_meta.rustcode`.
fn usage_for(params: &Value) -> Option<UsageUpdate> {
    let usage = params.get("tokenUsage")?;
    let count = |breakdown: &str, field: &str| {
        usage
            .get(breakdown)
            .and_then(|breakdown| breakdown.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let size = usage
        .get("modelContextWindow")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        json!({
            "kind": "usage",
            "version": crate::acp::usage::USAGE_META_VERSION,
            "estimated": false,
            "inputTokens": count("total", "inputTokens"),
            "outputTokens": count("total", "outputTokens"),
            "cachedTokens": count("total", "cachedInputTokens"),
        }),
    );
    Some(UsageUpdate::new(count("last", "inputTokens"), size).meta(meta))
}

/// The row for one completed `contextCompaction` item. Codex's own item id is
/// the `external_id`, so a replayed `item/completed` (a reconnect, a server
/// that repeats itself) updates its row instead of adding a second one.
fn compaction_row(
    thread_id: String,
    model: Option<String>,
    manual: bool,
    item_id: &str,
) -> crate::data_store::CompactionRow {
    crate::data_store::CompactionRow {
        thread_id,
        turn_id: None,
        harness: Some("codex".to_owned()),
        model,
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        duration_ms: None,
        trigger: if manual { "manual" } else { "harness_auto" }.to_owned(),
        mode: Some("harness_summary".to_owned()),
        tokens_before: None,
        tokens_after: None,
        messages_before: None,
        messages_after: None,
        idle_secs_before: None,
        count: None,
        success: true,
        error_kind: None,
        external_id: (!item_id.trim().is_empty()).then(|| item_id.to_owned()),
        source: Some("codex".to_owned()),
    }
}

/// Translate one Codex notification into ACP updates.
fn translate(state: &Mutex<State>, method: &str, params: &Value) -> Vec<SessionUpdate> {
    let item_id = str_field(params, "itemId").unwrap_or_default().to_owned();
    // What a subagent says, plans and spends is its own, not the thread's: only
    // its tool calls stay, folded under its row.
    let subagent_only = matches!(
        method,
        "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/reasoning/textDelta"
            | "turn/plan/updated"
            | "thread/tokenUsage/updated"
    );
    if subagent_only && is_agent_thread(state, params) {
        return Vec::new();
    }
    match method {
        "item/agentMessage/delta" => {
            let delta = str_field(params, "delta").unwrap_or_default();
            if delta.is_empty() {
                return Vec::new();
            }
            let mut state = lock(state);
            let separate = state
                .last_message_item
                .as_ref()
                .is_some_and(|last| *last != item_id);
            state.streamed.insert(item_id.clone());
            state.last_message_item = Some(item_id);
            let text = if separate {
                format!("\n\n{delta}")
            } else {
                delta.to_owned()
            };
            vec![SessionUpdate::AgentMessageChunk(text_chunk(text))]
        }
        "item/reasoning/summaryTextDelta" => {
            lock(state).summarized.insert(item_id);
            let delta = str_field(params, "delta").unwrap_or_default();
            if delta.is_empty() {
                return Vec::new();
            }
            vec![SessionUpdate::AgentThoughtChunk(text_chunk(delta))]
        }
        "item/reasoning/summaryPartAdded" => {
            let first = lock(state).summarized.insert(item_id);
            if first {
                Vec::new()
            } else {
                vec![SessionUpdate::AgentThoughtChunk(text_chunk("\n\n"))]
            }
        }
        "item/reasoning/textDelta" => {
            let delta = str_field(params, "delta").unwrap_or_default();
            if delta.is_empty() || lock(state).summarized.contains(&item_id) {
                return Vec::new();
            }
            lock(state).streamed.insert(item_id);
            vec![SessionUpdate::AgentThoughtChunk(text_chunk(delta))]
        }
        "item/started" => {
            let Some(item) = params.get("item") else {
                return Vec::new();
            };
            if str_field(item, "type") == Some("collabAgentToolCall") {
                return collab_updates(state, params, item, false);
            }
            // Codex compacts on its own inside a turn, and says when it begins:
            // the chat shows it until the completed item replaces the line.
            if str_field(item, "type") == Some("contextCompaction") {
                if is_agent_thread(state, params) {
                    return Vec::new();
                }
                return vec![crate::acp::usage::compaction_progress_card(
                    crate::acp::usage::CompactionPhase::Started,
                    "Compacting context…".to_owned(),
                )];
            }
            tool_call_for(item)
                .map(|call| SessionUpdate::ToolCall(fold_under_agent(state, params, call)))
                .into_iter()
                .collect()
        }
        "item/completed" => {
            let Some(item) = params.get("item") else {
                return Vec::new();
            };
            let id = str_field(item, "id").unwrap_or_default().to_owned();
            let own_text = matches!(
                str_field(item, "type"),
                Some("agentMessage" | "reasoning" | "contextCompaction")
            );
            if own_text && is_agent_thread(state, params) {
                return Vec::new();
            }
            match str_field(item, "type") {
                Some("contextCompaction") => {
                    // A compaction asked for with `/compact` runs no turn of its own
                    // that ends the wait; its completed item does.
                    let mut guard = lock(state);
                    let manual = guard.compacting;
                    let thread_id = guard.thread_id.clone();
                    let model = guard.model.clone();
                    if guard.compacting {
                        guard.compacting = false;
                        if let Some(waiter) = guard.waiter.take() {
                            let _ = waiter.send(TurnEnd::Completed);
                        }
                    }
                    drop(guard);
                    if let Some(thread_id) = thread_id {
                        crate::data_store::record_compaction(&compaction_row(
                            thread_id, model, manual, &id,
                        ));
                    }
                    vec![crate::acp::usage::compaction_card(
                        None,
                        None,
                        Some("Context compacted: Codex summarized the conversation to free the context window.".to_owned()),
                    )]
                }
                Some("agentMessage") => {
                    let mut state = lock(state);
                    if state.streamed.contains(&id) {
                        return Vec::new();
                    }
                    let text = str_field(item, "text").unwrap_or_default();
                    if text.is_empty() {
                        return Vec::new();
                    }
                    let separate = state.last_message_item.is_some();
                    state.last_message_item = Some(id);
                    let text = if separate {
                        format!("\n\n{text}")
                    } else {
                        text.to_owned()
                    };
                    vec![SessionUpdate::AgentMessageChunk(text_chunk(text))]
                }
                Some("reasoning") => {
                    let state = lock(state);
                    if state.summarized.contains(&id) || state.streamed.contains(&id) {
                        return Vec::new();
                    }
                    let summary = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default();
                    if summary.is_empty() {
                        Vec::new()
                    } else {
                        vec![SessionUpdate::AgentThoughtChunk(text_chunk(summary))]
                    }
                }
                Some("collabAgentToolCall") => collab_updates(state, params, item, true),
                _ => tool_update_for(item)
                    .map(|update| {
                        SessionUpdate::ToolCallUpdate(fold_under_agent_update(
                            state, params, update,
                        ))
                    })
                    .into_iter()
                    .collect(),
            }
        }
        "turn/completed" => agent_turn_end(state, params),
        "turn/plan/updated" => plan_for(params)
            .map(SessionUpdate::Plan)
            .into_iter()
            .collect(),
        "thread/tokenUsage/updated" => usage_for(params)
            .map(SessionUpdate::UsageUpdate)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// Note the end of a turn or a failure that will not be retried.
fn settle(state: &Mutex<State>, method: &str, params: &Value) {
    // A subagent's own turn, and a failure it reports, are not the thread's.
    if matches!(method, "error" | "turn/completed") && is_agent_thread(state, params) {
        return;
    }
    match method {
        "error" => {
            if !params
                .get("willRetry")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && let Some(message) = params.pointer("/error/message").and_then(Value::as_str)
            {
                lock(state).last_error = Some(message.to_owned());
            }
        }
        "turn/completed" => {
            let mut state = lock(state);
            let end = match params.pointer("/turn/status").and_then(Value::as_str) {
                Some("interrupted") => TurnEnd::Interrupted,
                Some("failed") => TurnEnd::Failed(
                    params
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| state.last_error.clone())
                        .map(|message| format!("Codex: {message}"))
                        .unwrap_or_else(|| "Codex: the turn failed".to_owned()),
                ),
                _ => TurnEnd::Completed,
            };
            state.turn_id = None;
            state.cancel_requested = false;
            state.streamed.clear();
            state.summarized.clear();
            if let Some(waiter) = state.waiter.take() {
                let _ = waiter.send(end);
            }
        }
        _ => {}
    }
}

/// Ask the user Codex's `request_user_input` questions, one at a time, as
/// RustCode questions (`acp/questions.rs`), and answer with every reply.
/// Dismissing any question fails the request, so Codex hears the user did
/// not answer.
async fn ask_user(peer: Arc<RpcPeer>, out: OutputSender, id: Value, params: Value) {
    let item = str_field(&params, "itemId").unwrap_or("input").to_owned();
    let questions = params
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut answers = serde_json::Map::new();
    for question in &questions {
        let question_id = str_field(question, "id").unwrap_or_default().to_owned();
        let options = question
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let prompt = crate::controller::QuestionPrompt {
            header: str_field(question, "header").unwrap_or_default().to_owned(),
            text: str_field(question, "question")
                .unwrap_or_default()
                .to_owned(),
            options: options
                .iter()
                .filter_map(|option| str_field(option, "label"))
                .map(str::to_owned)
                .collect(),
            descriptions: options
                .iter()
                .map(|option| {
                    str_field(option, "description")
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect(),
            multiple: false,
        };
        let request = crate::acp::questions::question_permission_request(
            "",
            &format!("{item}:{question_id}"),
            &prompt,
        );
        let (reply, answer) = oneshot::channel();
        let reply = if out.send(Output::Permission { request, reply }).is_ok() {
            answer
                .await
                .ok()
                .and_then(|response| crate::acp::questions::question_reply(&response, &prompt))
        } else {
            None
        };
        let Some(reply) = reply else {
            peer.respond(
                id,
                Err(RpcError {
                    code: -32000,
                    message: "the user dismissed the question".to_owned(),
                    data: None,
                }),
            );
            return;
        };
        let given = match reply {
            crate::controller::QuestionReply::Options(labels) => labels,
            crate::controller::QuestionReply::Custom(text) => vec![text],
        };
        answers.insert(question_id, json!({"answers": given}));
    }
    peer.respond(id, Ok(json!({"answers": answers})));
}

/// What to do with a Codex `mcpServer/elicitation/request`.
#[derive(Debug)]
pub(super) enum Elicitation {
    /// Our own orchestrator server: RustCode asks the user itself when a
    /// thread starts or messages another, so a second prompt adds nothing.
    Accept,
    /// A yes/no confirmation, such as "Allow GitHub to run tool …?".
    Ask(RequestPermissionRequest),
    /// A form with fields, or a URL to open: no UI for it.
    Decline,
}

/// Codex asks before an MCP tool runs through an elicitation with an empty
/// form. Accept it for our server, ask the user for others.
pub(super) fn elicitation(params: &Value) -> Elicitation {
    let mode = str_field(params, "mode").unwrap_or("form");
    let empty_form = mode != "url"
        && params
            .get("requestedSchema")
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
            .is_none_or(serde_json::Map::is_empty);
    if !empty_form {
        return Elicitation::Decline;
    }
    let server = str_field(params, "serverName").unwrap_or_default();
    if server == crate::orchestration::hub::SERVER_NAME {
        return Elicitation::Accept;
    }
    let message = str_field(params, "message")
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Allow {server} to run a tool?"));
    let options = vec![
        PermissionOption::new("accept", "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            "acceptForSession",
            "Allow for this thread",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("decline", "Deny", PermissionOptionKind::RejectOnce),
    ];
    let tool_call = ToolCallUpdate::new(
        "elicitation",
        ToolCallUpdateFields::new()
            .title(message)
            .kind(ToolKind::Other)
            .raw_input(params.clone()),
    );
    Elicitation::Ask(RequestPermissionRequest::new(
        String::new(),
        tool_call,
        options,
    ))
}

/// Codex's answer for the option the user picked; anything else declines.
/// "For this thread" rides in `_meta.persist`, as Codex's own UI sends it.
pub(super) fn elicitation_reply(option: &str) -> Value {
    match option {
        "accept" => json!({"action": "accept", "content": {}}),
        "acceptForSession" => {
            json!({"action": "accept", "content": {}, "_meta": {"persist": "session"}})
        }
        _ => json!({"action": "decline", "content": null}),
    }
}

/// The ACP permission request for a Codex approval request, with the
/// decision each option answers.
fn approval_request(method: &str, params: &Value) -> Option<(RequestPermissionRequest, bool)> {
    let legacy = matches!(method, "execCommandApproval" | "applyPatchApproval");
    let item_id = str_field(params, "itemId")
        .or_else(|| str_field(params, "callId"))
        .unwrap_or("approval")
        .to_owned();
    let reason = str_field(params, "reason");
    let (title, kind) = match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => {
            let command = match params.get("command") {
                Some(Value::String(command)) => command.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => "Run a command".to_owned(),
            };
            (command, ToolKind::Execute)
        }
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            ("Apply file changes".to_owned(), ToolKind::Edit)
        }
        _ => return None,
    };
    let title = match reason {
        Some(reason) if !reason.is_empty() => format!("{title} ({reason})"),
        _ => title,
    };
    let (allow, always, reject) = if legacy {
        ("approved", "approved_for_session", "abort")
    } else {
        ("accept", "acceptForSession", "decline")
    };
    let options = vec![
        PermissionOption::new(allow, "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            always,
            "Allow for this thread",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new(reject, "Deny", PermissionOptionKind::RejectOnce),
    ];
    let tool_call = ToolCallUpdate::new(
        item_id,
        ToolCallUpdateFields::new()
            .title(title)
            .kind(kind)
            .raw_input(params.clone()),
    );
    Some((
        RequestPermissionRequest::new(String::new(), tool_call, options),
        legacy,
    ))
}

async fn relay(
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    peer: Arc<RpcPeer>,
    state: Arc<Mutex<State>>,
    out: OutputSender,
) {
    while let Some(message) = incoming.recv().await {
        match message {
            Incoming::Notification { method, params } => {
                let updates = translate(&state, &method, &params);
                // Not every notification becomes an update, but each one says
                // Codex is alive (`watchdog`).
                let spoke = !updates.is_empty();
                for update in updates {
                    let _ = out.send(Output::Update(update));
                }
                settle(&state, &method, &params);
                if !spoke {
                    let _ = out.send(Output::Heartbeat);
                }
            }
            Incoming::Barrier(done) => {
                let _ = done.send(());
            }
            Incoming::Request { id, method, params } if method == "item/tool/requestUserInput" => {
                tokio::spawn(ask_user(Arc::clone(&peer), out.clone(), id, params));
            }
            Incoming::Request { id, method, params }
                if method == "mcpServer/elicitation/request" =>
            {
                match elicitation(&params) {
                    Elicitation::Accept => {
                        peer.respond(id, Ok(json!({"action": "accept", "content": {}})));
                    }
                    Elicitation::Decline => {
                        // A form with fields of its own: RustCode has no form
                        // UI, so the MCP server hears a decline, not an error.
                        crate::dbg_log!(
                            "codex elicitation declined: {}",
                            str_field(&params, "message").unwrap_or_default()
                        );
                        peer.respond(id, Ok(json!({"action": "decline", "content": null})));
                    }
                    Elicitation::Ask(request) => {
                        let (reply, answer) = oneshot::channel();
                        let asked = out.send(Output::Permission { request, reply }).is_ok();
                        let peer = Arc::clone(&peer);
                        tokio::spawn(async move {
                            let selected = match answer.await.map(|response| response.outcome) {
                                Ok(RequestPermissionOutcome::Selected(selected)) if asked => {
                                    selected.option_id.0.to_string()
                                }
                                _ => String::new(),
                            };
                            peer.respond(id, Ok(elicitation_reply(&selected)));
                        });
                    }
                }
            }
            Incoming::Request { id, method, params } => {
                let Some((request, legacy)) = approval_request(&method, &params) else {
                    peer.respond(id, Err(RpcError::method_not_found(&method)));
                    continue;
                };
                let (reply, answer) = oneshot::channel();
                let asked = out.send(Output::Permission { request, reply }).is_ok();
                let peer = Arc::clone(&peer);
                tokio::spawn(async move {
                    let outcome = if asked {
                        answer
                            .await
                            .map(|response| response.outcome)
                            .unwrap_or(RequestPermissionOutcome::Cancelled)
                    } else {
                        RequestPermissionOutcome::Cancelled
                    };
                    let decision = match outcome {
                        RequestPermissionOutcome::Selected(selected) => {
                            selected.option_id.0.to_string()
                        }
                        _ if legacy => "abort".to_owned(),
                        _ => "cancel".to_owned(),
                    };
                    peer.respond(id, Ok(json!({"decision": decision})));
                });
            }
        }
    }
    // The app-server is gone: a turn waiting on it will never complete.
    if let Some(waiter) = lock(&state).waiter.take() {
        let _ = waiter.send(TurnEnd::Failed("Codex exited during the turn".to_owned()));
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
