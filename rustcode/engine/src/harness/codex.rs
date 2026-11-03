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
use serde_json::{Value, json};
use std::collections::HashSet;
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
        let state = Arc::new(Mutex::new(State::default()));
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
        // How replies are formatted for RustCode's chat (docs/output-style.md);
        // Codex appends it to its own instructions as a developer message.
        let output_style = crate::tools::schema::harness_output_style();
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

    pub(crate) async fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<StopReason, String> {
        let (sender, receiver) = oneshot::channel();
        let (model, effort) = {
            let mut state = lock(&self.state);
            state.waiter = Some(sender);
            state.turn_id = None;
            // `cancel_requested` stays: a cancel can arrive before the turn
            // has started, and is applied as soon as it has an id.
            state.last_error = None;
            state.last_message_item = None;
            (state.model.clone(), state.effort.clone())
        };
        // `/compact` alone is Codex's own compaction, not a message for the model
        // (`thread/compact/start`, as T3 Code routes it); its `contextCompaction`
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

    pub(crate) async fn steer(&self, text: &str) -> bool {
        let Some(turn_id) = lock(&self.state).turn_id.clone() else {
            return false;
        };
        if text.trim().is_empty() {
            return false;
        }
        self.peer
            .request(
                "turn/steer",
                json!({
                    "threadId": self.thread_id,
                    "input": [{"type": "text", "text": text.trim(), "text_elements": []}],
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

/// A Codex subagent call as a step title: "Start subagent: <first line of its task>".
fn collab_title(item: &Value) -> String {
    let action = match str_field(item, "tool").unwrap_or_default() {
        "spawnAgent" => "Start subagent",
        "sendInput" | "sendMessage" | "followupTask" => "Message subagent",
        "resumeAgent" => "Resume subagent",
        "wait" => "Wait for subagents",
        "closeAgent" => "Close subagent",
        "interruptAgent" => "Interrupt subagent",
        "listAgents" => "List subagents",
        _ => "Subagent",
    };
    match str_field(item, "prompt")
        .and_then(|prompt| prompt.lines().find(|line| !line.trim().is_empty()))
    {
        Some(task) => {
            let task = task.trim();
            let short: String = task.chars().take(80).collect();
            let more = if short.len() < task.len() { "…" } else { "" };
            format!("{action}: {short}{more}")
        }
        None => action.to_owned(),
    }
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
        // Codex's own subagents: shown as a step, so the work they do is not invisible.
        "collabAgentToolCall" => ToolCall::new(id, collab_title(item))
            .kind(ToolKind::Other)
            .raw_input(json!({
                "tool": item.get("tool"),
                "prompt": item.get("prompt"),
                "model": item.get("model"),
                "agents": item.get("receiverThreadIds"),
            })),
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
        "webSearch" | "imageView" | "collabAgentToolCall" => {
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

/// Translate one Codex notification into ACP updates.
fn translate(state: &Mutex<State>, method: &str, params: &Value) -> Vec<SessionUpdate> {
    let item_id = str_field(params, "itemId").unwrap_or_default().to_owned();
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
        "item/started" => params
            .get("item")
            .and_then(tool_call_for)
            .map(SessionUpdate::ToolCall)
            .into_iter()
            .collect(),
        "item/completed" => {
            let Some(item) = params.get("item") else {
                return Vec::new();
            };
            let id = str_field(item, "id").unwrap_or_default().to_owned();
            match str_field(item, "type") {
                Some("contextCompaction") => {
                    // A compaction asked for with `/compact` runs no turn of its own
                    // that ends the wait; its completed item does.
                    let mut guard = lock(state);
                    if guard.compacting {
                        guard.compacting = false;
                        if let Some(waiter) = guard.waiter.take() {
                            let _ = waiter.send(TurnEnd::Completed);
                        }
                    }
                    drop(guard);
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
                _ => tool_update_for(item)
                    .map(SessionUpdate::ToolCallUpdate)
                    .into_iter()
                    .collect(),
            }
        }
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
                for update in translate(&state, &method, &params) {
                    let _ = out.send(Output::Update(update));
                }
                settle(&state, &method, &params);
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
