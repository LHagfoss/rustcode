//! Claude Code CLI transport.
//!
//! A CLI-bound profile has no HTTP endpoint. Each conversation is served by a
//! long-lived headless `claude` child that keeps its own context and runs its
//! own model loop. RustCode stays in charge of everything else:
//!
//! - The child gets none of its built-in tools. RustCode's tool schemas are
//!   offered through an in-process MCP server tunnelled over the child's
//!   stdin/stdout, so every call is executed (and authorized) by RustCode.
//! - One provider "request" is one assistant message. When the model stops to
//!   call tools, the child's `tools/call` requests are held open; the next
//!   request delivers the results RustCode produced and resumes the stream.
//! - The child's Messages stream events are forwarded as SSE lines, so the
//!   existing Messages parser in `stream_request` consumes them unchanged.
//!
//! A conversation is matched to a live child by continuity (the tool results
//! it is waiting for, or the user message it last answered). Anything else —
//! a rewind, a compaction, a restart, a model switch — starts a new child and
//! restores the earlier history as a transcript.

use crate::provider_auth::claude_cli as cli;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::bytes::Bytes;

/// MCP server name the child sees. Kept short: the child prefixes every tool
/// with it and provider tool names are length-limited.
const MCP_SERVER: &str = "rc";
const TOOL_PREFIX: &str = "mcp__rc__";
/// Where the child reports which model tool call an MCP request belongs to
/// (`_meta["claudecode/toolUseId"]`, as a JSON pointer).
const TOOL_USE_ID_POINTER: &str = "/params/_meta/claudecode~1toolUseId";
const MAX_LIVE_CONVERSATIONS: usize = 4;
const IDLE_REAP: Duration = Duration::from_secs(30 * 60);
/// Linux caps a single argument at 128 KiB; stay well inside it.
const MAX_SYSTEM_ARG_BYTES: usize = 96 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 400_000;
const MAX_TRANSCRIPT_ITEM_BYTES: usize = 24_000;
const MAX_TRANSCRIPT_TOOL_BYTES: usize = 8_000;
const MISSING_RESULT: &str = "No result was recorded for this call. It did not run.";

pub(crate) type ByteStream = futures_util::stream::BoxStream<'static, std::io::Result<Bytes>>;

pub(crate) struct RoundRequest<'a> {
    pub session_id: &'a str,
    pub model: &'a str,
    pub effort: Option<&'a str>,
    /// Provider-shaped history (system, user, assistant, tool messages).
    pub messages: &'a [Value],
    /// OpenAI-style function schemas for the tools RustCode offers.
    pub tool_schemas: &'a [Value],
    pub allow_tools: bool,
}

/// Identifies the user message a child last answered: its text and how many
/// assistant messages precede it. Position keeps a repeated "yes" or
/// "continue" from matching an earlier point after a rewind.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Anchor {
    assistants_before: usize,
    text_hash: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    /// The child finished its turn and is waiting for user input.
    Idle,
    /// The child announced these tool calls and is waiting for their results.
    AwaitingTools(Vec<String>),
}

struct Parked {
    request_id: Value,
    rpc_id: Value,
}

struct Shared {
    phase: Phase,
    anchors: Vec<Anchor>,
    /// Results RustCode produced that the child has not asked for yet.
    results: HashMap<String, String>,
    /// `tools/call` requests the child sent before RustCode had a result.
    parked: HashMap<String, Parked>,
    /// User turns written to the child.
    turn: u64,
    /// Turn-end markers consumed from the child.
    results_seen: u64,
    dead: bool,
    last_used: Instant,
    stderr_tail: String,
}

impl Shared {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            anchors: Vec::new(),
            results: HashMap::new(),
            parked: HashMap::new(),
            turn: 0,
            results_seen: 0,
            dead: false,
            last_used: Instant::now(),
            stderr_tail: String::new(),
        }
    }
}

/// State the stdout reader shares with request handling.
struct Core {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    shared: StdMutex<Shared>,
    /// Tool definitions in MCP `tools/list` form.
    tools: Vec<Value>,
}

impl Core {
    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn write(&self, value: &Value) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(format!("{value}\n").as_bytes()).await?;
        stdin.flush().await
    }
}

enum Event {
    /// One Messages stream event.
    Stream(Value),
    Limits(Value),
    /// The child finished a user turn.
    Result {
        ordinal: u64,
        error: Option<String>,
    },
    Closed,
}

struct Conversation {
    core: Arc<Core>,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<Event>>,
    child: StdMutex<Option<tokio::process::Child>>,
    session_id: String,
    model: String,
    effort: Option<String>,
    tool_names: HashSet<String>,
    /// One-shot, tool-less requests (recaps, summaries) get a child that is
    /// discarded with its response.
    ephemeral: bool,
    busy: AtomicBool,
}

impl Conversation {
    fn kill(&self) {
        self.core.shared().dead = true;
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = child.start_kill();
        }
    }
}

fn registry() -> std::sync::MutexGuard<'static, Vec<Arc<Conversation>>> {
    static REGISTRY: OnceLock<StdMutex<Vec<Arc<Conversation>>>> = OnceLock::new();
    REGISTRY
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn retire(conversation: &Arc<Conversation>) {
    registry().retain(|live| !Arc::ptr_eq(live, conversation));
    conversation.kill();
}

/// Stop every live child, e.g. when the account is signed out.
pub(crate) fn shutdown_all() {
    let live = std::mem::take(&mut *registry());
    for conversation in live {
        conversation.kill();
    }
}

fn role(message: &Value) -> &str {
    message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user")
}

fn is_system(message: &Value) -> bool {
    matches!(role(message), "system" | "developer")
}

fn is_user(message: &Value) -> bool {
    !matches!(role(message), "assistant" | "tool" | "system" | "developer")
}

fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn anchor_at(conversation: &[&Value], index: usize) -> Anchor {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    message_text(conversation[index]).hash(&mut hasher);
    Anchor {
        assistants_before: conversation[..index]
            .iter()
            .filter(|message| role(message) == "assistant")
            .count(),
        text_hash: hasher.finish(),
    }
}

fn user_blocks(message: &Value) -> Vec<Value> {
    super::anthropic_messages::content_blocks(message.get("content")).unwrap_or_else(|_| {
        let text = message_text(message);
        if text.is_empty() {
            Vec::new()
        } else {
            vec![json!({"type": "text", "text": text})]
        }
    })
}

#[derive(Debug)]
enum Plan {
    /// Answer the tool calls the child is waiting on.
    Continue {
        results: Vec<(String, String)>,
        anchor: Option<Anchor>,
    },
    /// Send the next user turn.
    NewTurn {
        content: Vec<Value>,
        anchor: Option<Anchor>,
    },
}

fn new_turn(conversation: &[&Value], start: usize) -> Plan {
    let mut content = Vec::new();
    let mut anchor = None;
    for (index, message) in conversation.iter().enumerate().skip(start) {
        let blocks = user_blocks(message);
        if !blocks.is_empty() {
            content.extend(blocks);
            anchor = Some(anchor_at(conversation, index));
        }
    }
    if content.is_empty() {
        content.push(json!({"type": "text", "text": "Continue."}));
    }
    Plan::NewTurn { content, anchor }
}

/// Decide whether `conversation` (system messages removed) continues what a
/// child in `phase` has seen, and if so what to send it. `None` means the
/// histories diverged and the child cannot serve this request.
fn plan(phase: &Phase, anchors: &[Anchor], conversation: &[&Value]) -> Option<Plan> {
    match phase {
        Phase::AwaitingTools(ids) => {
            let mut results: Vec<(String, String)> = Vec::new();
            let mut last_match = None;
            for (index, message) in conversation.iter().enumerate() {
                if role(message) != "tool" {
                    continue;
                }
                let Some(id) = message.get("tool_call_id").and_then(Value::as_str) else {
                    continue;
                };
                if ids.iter().any(|expected| expected == id)
                    && !results.iter().any(|(seen, _)| seen == id)
                {
                    results.push((id.to_owned(), message_text(message)));
                    last_match = Some(index);
                }
            }
            let last = last_match?;
            for id in ids {
                if !results.iter().any(|(seen, _)| seen == id) {
                    results.push((id.clone(), MISSING_RESULT.to_owned()));
                }
            }
            // The child only accepts tool results mid-turn. A user message
            // that arrived while the tools ran rides along with the last one.
            let mut anchor = None;
            let mut extra = String::new();
            for (index, message) in conversation.iter().enumerate().skip(last + 1) {
                let text = message_text(message);
                if is_user(message) && !text.trim().is_empty() {
                    extra.push_str("\n\n[User message sent while these tools ran]\n");
                    extra.push_str(&text);
                    anchor = Some(anchor_at(conversation, index));
                }
            }
            if !extra.is_empty() {
                let last_id = conversation[last]["tool_call_id"].as_str().unwrap_or("");
                if let Some((_, text)) = results.iter_mut().find(|(id, _)| id == last_id) {
                    text.push_str(&extra);
                }
            }
            Some(Plan::Continue { results, anchor })
        }
        Phase::Idle => {
            let last_assistant = conversation
                .iter()
                .rposition(|message| role(message) == "assistant")?;
            let prior_user = conversation[..last_assistant]
                .iter()
                .rposition(|message| is_user(message))?;
            if !anchors.contains(&anchor_at(conversation, prior_user)) {
                return None;
            }
            if conversation[last_assistant + 1..]
                .iter()
                .any(|message| !is_user(message))
            {
                return None;
            }
            Some(new_turn(conversation, last_assistant + 1))
        }
    }
}

fn truncated(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!("{}\n[truncated]", &text[..text.floor_char_boundary(max)])
}

/// Render earlier history for a child that has not seen it. Lossy by design:
/// tool calls become text, so it is context to read, not a turn to replay.
fn render_transcript(history: &[&Value]) -> Option<String> {
    let mut items = Vec::new();
    for message in history {
        let text = message_text(message);
        let item = match role(message) {
            "assistant" => {
                let mut body = truncated(text.trim(), MAX_TRANSCRIPT_ITEM_BYTES);
                for call in message
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let name = call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    let arguments = call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    body.push_str(&format!(
                        "\n[called tool {name} with {}]",
                        truncated(arguments, MAX_TRANSCRIPT_TOOL_BYTES)
                    ));
                }
                format!("<assistant>\n{}\n</assistant>", body.trim())
            }
            "tool" => format!(
                "<tool_result>\n{}\n</tool_result>",
                truncated(text.trim(), MAX_TRANSCRIPT_TOOL_BYTES)
            ),
            _ => format!(
                "<user>\n{}\n</user>",
                truncated(text.trim(), MAX_TRANSCRIPT_ITEM_BYTES)
            ),
        };
        items.push(item);
    }
    if items.is_empty() {
        return None;
    }
    // Keep the most recent history when the whole of it does not fit.
    let mut kept = Vec::new();
    let mut size = 0;
    for item in items.into_iter().rev() {
        size += item.len();
        if size > MAX_TRANSCRIPT_BYTES && !kept.is_empty() {
            kept.push("[earlier messages omitted]".to_owned());
            break;
        }
        kept.push(item);
    }
    kept.reverse();
    Some(format!(
        "The conversation below took place earlier in this session and is restored here as context. Tool calls in it already ran; do not repeat them or answer it again. Continue from the message that follows it.\n<earlier_conversation>\n{}\n</earlier_conversation>",
        kept.join("\n")
    ))
}

/// First turn for a new child: restored history, then the live user input.
fn first_turn(conversation: &[&Value], system_overflow: Option<&str>) -> (Vec<Value>, Vec<Anchor>) {
    let start = conversation
        .iter()
        .rposition(|message| !is_user(message))
        .map_or(0, |index| index + 1);
    let mut content = Vec::new();
    if let Some(overflow) = system_overflow {
        content.push(json!({
            "type": "text",
            "text": format!("<system_instructions_continued>\n{overflow}\n</system_instructions_continued>"),
        }));
    }
    if let Some(transcript) = render_transcript(&conversation[..start]) {
        content.push(json!({"type": "text", "text": transcript}));
    }
    let Plan::NewTurn {
        content: live,
        anchor,
    } = new_turn(conversation, start)
    else {
        unreachable!("new_turn always plans a user turn")
    };
    content.extend(live);
    // With no live user input the child answers the restored history, so the
    // last user message in it is what the next request will be anchored to.
    let anchor = anchor.or_else(|| {
        conversation
            .iter()
            .rposition(|message| is_user(message))
            .map(|index| anchor_at(conversation, index))
    });
    (content, anchor.into_iter().collect())
}

fn system_prompt(messages: &[Value]) -> String {
    messages
        .iter()
        .filter(|message| is_system(message))
        .map(message_text)
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn mcp_tools(schemas: &[Value]) -> Vec<Value> {
    schemas
        .iter()
        .filter_map(|schema| {
            let function = schema.get("function")?;
            Some(json!({
                "name": function.get("name")?.as_str()?,
                "description": function.get("description").and_then(Value::as_str).unwrap_or(""),
                "inputSchema": function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            }))
        })
        .collect()
}

fn tool_names(tools: &[Value]) -> HashSet<String> {
    tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

fn control_success(request_id: &Value, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": request_id, "response": response},
    })
}

fn tool_response(parked: &Parked, text: &str) -> Value {
    control_success(
        &parked.request_id,
        json!({"mcp_response": {
            "jsonrpc": "2.0",
            "id": parked.rpc_id,
            "result": {"content": [{"type": "text", "text": text}]},
        }}),
    )
}

/// Answer a control request from the child. `None` means the answer is
/// deferred: a tool call RustCode has not produced a result for yet.
fn control_reply(tools: &[Value], shared: &mut Shared, line: &Value) -> Option<Value> {
    let request_id = &line["request_id"];
    let request = &line["request"];
    match request["subtype"].as_str() {
        // RustCode authorizes every call itself when it executes the tool.
        Some("can_use_tool") => Some(control_success(
            request_id,
            json!({
                "behavior": "allow",
                "updatedInput": request.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
        )),
        Some("mcp_message") => {
            let message = &request["message"];
            let rpc_id = message.get("id").cloned().unwrap_or(Value::Null);
            let result = match message["method"].as_str() {
                Some("initialize") => json!({
                    "protocolVersion": message
                        .pointer("/params/protocolVersion")
                        .cloned()
                        .unwrap_or_else(|| json!("2025-06-18")),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "rustcode", "version": env!("CARGO_PKG_VERSION")},
                }),
                Some("tools/list") => json!({"tools": tools}),
                Some("ping") => json!({}),
                Some("tools/call") => {
                    let parked = Parked {
                        request_id: request_id.clone(),
                        rpc_id,
                    };
                    let Some(id) = message.pointer(TOOL_USE_ID_POINTER).and_then(Value::as_str)
                    else {
                        return Some(tool_response(
                            &parked,
                            "RustCode could not match this call to a model tool call.",
                        ));
                    };
                    return match shared.results.remove(id) {
                        Some(text) => Some(tool_response(&parked, &text)),
                        None => {
                            shared.parked.insert(id.to_owned(), parked);
                            None
                        }
                    };
                }
                _ if rpc_id.is_null() => {
                    return Some(control_success(request_id, json!({"mcp_response": {}})));
                }
                _ => {
                    return Some(control_success(
                        request_id,
                        json!({"mcp_response": {
                            "jsonrpc": "2.0",
                            "id": rpc_id,
                            "error": {"code": -32601, "message": "method not found"},
                        }}),
                    ));
                }
            };
            Some(control_success(
                request_id,
                json!({"mcp_response": {"jsonrpc": "2.0", "id": rpc_id, "result": result}}),
            ))
        }
        _ => Some(json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": "RustCode does not handle this request",
            },
        })),
    }
}

fn result_error(line: &Value) -> String {
    line.get("result")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let errors = line.get("errors")?.as_array()?;
            let text = errors
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ");
            (!text.is_empty()).then_some(text)
        })
        .unwrap_or_else(|| {
            format!(
                "the Claude Code CLI ended the turn with {}",
                line["subtype"].as_str().unwrap_or("an error")
            )
        })
}

async fn read_stdout(
    core: Arc<Core>,
    stdout: tokio::process::ChildStdout,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut lines = BufReader::new(stdout).lines();
    let mut ordinal = 0;
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(mut value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let sent = match value["type"].as_str() {
            Some("stream_event") => {
                // Only the main conversation's stream is a provider response.
                if value
                    .get("parent_tool_use_id")
                    .is_some_and(|parent| !parent.is_null())
                {
                    continue;
                }
                events.send(Event::Stream(value["event"].take()))
            }
            Some("rate_limit_event") => events.send(Event::Limits(value["rate_limit_info"].take())),
            Some("result") => {
                ordinal += 1;
                let failed = value["is_error"] == true || value["subtype"] != "success";
                events.send(Event::Result {
                    ordinal,
                    error: failed.then(|| result_error(&value)),
                })
            }
            Some("control_request") => {
                let reply = control_reply(&core.tools, &mut core.shared(), &value);
                if let Some(reply) = reply
                    && core.write(&reply).await.is_err()
                {
                    break;
                }
                continue;
            }
            _ => continue,
        };
        if sent.is_err() {
            break;
        }
    }
    core.shared().dead = true;
    let _ = events.send(Event::Closed);
}

async fn read_stderr(core: Arc<Core>, stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut shared = core.shared();
        shared.stderr_tail.push_str(line.trim_end());
        shared.stderr_tail.push('\n');
        let excess = shared.stderr_tail.len().saturating_sub(2000);
        if excess > 0 {
            let cut = shared.stderr_tail.ceil_char_boundary(excess);
            shared.stderr_tail.drain(..cut);
        }
    }
}

fn runtime_dir() -> std::path::PathBuf {
    // A neutral directory: the child must not pick up a project's CLAUDE.md or
    // settings, and RustCode's tools carry their own workspace paths.
    let dir = crate::provider_auth::config_dir()
        .map(|dir| dir.join("claude-cli"))
        .unwrap_or_else(|_| std::env::temp_dir().join("rustcode-claude-cli"));
    if std::fs::create_dir_all(&dir).is_ok() {
        dir
    } else {
        std::env::temp_dir()
    }
}

/// Split a system prompt into the part passed as an argument and any overflow
/// that must travel in the first user message instead.
fn split_system(system: &str) -> (&str, Option<&str>) {
    if system.len() <= MAX_SYSTEM_ARG_BYTES {
        return (system, None);
    }
    let (head, tail) = system.split_at(system.floor_char_boundary(MAX_SYSTEM_ARG_BYTES));
    (head, Some(tail))
}

async fn spawn(
    mut command: tokio::process::Command,
    request: &RoundRequest<'_>,
    system: &str,
    ephemeral: bool,
) -> Result<Arc<Conversation>, String> {
    if request.model.is_empty() || request.model.starts_with('-') {
        return Err("the Claude Code CLI profile has an invalid model name".into());
    }
    let tools = if ephemeral {
        Vec::new()
    } else {
        mcp_tools(request.tool_schemas)
    };
    command
        .args(cli::headless_args())
        .args(["--model", request.model])
        .args([
            "--system-prompt",
            if system.trim().is_empty() {
                "You are a helpful assistant."
            } else {
                system
            },
        ]);
    if let Some(effort) = request.effort {
        command.args(["--effort", effort]);
    }
    if !tools.is_empty() {
        let servers = json!({"mcpServers": {MCP_SERVER: {"type": "sdk", "name": MCP_SERVER}}});
        command
            .args(["--mcp-config", &servers.to_string()])
            .args(["--permission-prompt-tool", "stdio"]);
    }
    let mut child = command
        // RustCode may hold a tool call open for as long as the user takes to
        // approve it, and bounds tool output itself.
        .env("MCP_TOOL_TIMEOUT", "86400000")
        .env("MAX_MCP_OUTPUT_TOKENS", "400000")
        .current_dir(runtime_dir())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!(
                "could not start the Claude Code CLI ({error}); install it and sign in with `claude auth login`"
            )
        })?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("the Claude Code CLI started without its input or output streams".into());
    };
    let core = Arc::new(Core {
        stdin: tokio::sync::Mutex::new(stdin),
        shared: StdMutex::new(Shared::new()),
        tools,
    });
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(read_stdout(Arc::clone(&core), stdout, sender));
    tokio::spawn(read_stderr(Arc::clone(&core), stderr));
    Ok(Arc::new(Conversation {
        tool_names: tool_names(&core.tools),
        core,
        events: tokio::sync::Mutex::new(receiver),
        child: StdMutex::new(Some(child)),
        session_id: request.session_id.to_owned(),
        model: request.model.to_owned(),
        effort: request.effort.map(str::to_owned),
        ephemeral,
        busy: AtomicBool::new(true),
    }))
}

/// Find a live child this request continues, retiring idle or stale ones on
/// the way.
fn claim(request: &RoundRequest<'_>, conversation: &[&Value]) -> Option<(Arc<Conversation>, Plan)> {
    let wanted = tool_names(&mcp_tools(request.tool_schemas));
    let mut live = registry();
    let mut claimed = None;
    live.retain(|candidate| {
        if candidate.busy.load(Ordering::Acquire) {
            return true;
        }
        let shared = candidate.core.shared();
        if shared.dead || shared.last_used.elapsed() > IDLE_REAP {
            drop(shared);
            candidate.kill();
            return false;
        }
        if claimed.is_some()
            || candidate.session_id != request.session_id
            || candidate.model != request.model
            || candidate.effort.as_deref() != request.effort
        {
            return true;
        }
        let Some(plan) = plan(&shared.phase, &shared.anchors, conversation) else {
            return true;
        };
        // A child learns its tools once. If RustCode now offers ones it has
        // never seen (a mode switch, a newly connected MCP server), replace it
        // at the turn boundary rather than hide them from the model.
        if matches!(plan, Plan::NewTurn { .. })
            && request.allow_tools
            && !wanted.is_subset(&candidate.tool_names)
        {
            drop(shared);
            candidate.kill();
            return false;
        }
        drop(shared);
        candidate.busy.store(true, Ordering::Release);
        claimed = Some((Arc::clone(candidate), plan));
        true
    });
    claimed
}

fn register(conversation: &Arc<Conversation>) {
    let mut live = registry();
    while live.len() >= MAX_LIVE_CONVERSATIONS {
        let oldest = live
            .iter()
            .enumerate()
            .filter(|(_, candidate)| !candidate.busy.load(Ordering::Acquire))
            .min_by_key(|(_, candidate)| candidate.core.shared().last_used)
            .map(|(index, _)| index);
        let Some(index) = oldest else {
            break;
        };
        live.remove(index).kill();
    }
    live.push(Arc::clone(conversation));
}

fn user_message(content: Vec<Value>) -> Value {
    json!({"type": "user", "message": {"role": "user", "content": content}})
}

/// Send this request to a `claude` child and return the response as a
/// Messages SSE byte stream that ends after one assistant message.
pub(crate) async fn start_round(request: RoundRequest<'_>) -> Result<ByteStream, String> {
    start_round_with(cli::command(), request).await
}

async fn start_round_with(
    command: tokio::process::Command,
    request: RoundRequest<'_>,
) -> Result<ByteStream, String> {
    let conversation: Vec<&Value> = request
        .messages
        .iter()
        .filter(|message| !is_system(message))
        .collect();
    let (live, writes) = match claim(&request, &conversation) {
        Some((live, Plan::Continue { results, anchor })) => {
            let mut writes = Vec::new();
            let mut shared = live.core.shared();
            for (id, text) in results {
                match shared.parked.remove(&id) {
                    Some(parked) => writes.push(tool_response(&parked, &text)),
                    None => {
                        shared.results.insert(id, text);
                    }
                }
            }
            shared.anchors.extend(anchor);
            drop(shared);
            (live, writes)
        }
        Some((live, Plan::NewTurn { content, anchor })) => {
            let mut shared = live.core.shared();
            shared.results.clear();
            shared.turn += 1;
            if let Some(anchor) = anchor {
                shared.anchors = vec![anchor];
            }
            drop(shared);
            (live, vec![user_message(content)])
        }
        None => {
            let system = system_prompt(request.messages);
            let (system, overflow) = split_system(&system);
            let ephemeral = !request.allow_tools;
            let live = spawn(command, &request, system, ephemeral).await?;
            let (content, anchors) = first_turn(&conversation, overflow);
            {
                let mut shared = live.core.shared();
                shared.turn = 1;
                shared.anchors = anchors;
            }
            if !ephemeral {
                register(&live);
            }
            let initialize = json!({
                "type": "control_request",
                "request_id": "rustcode-init",
                "request": {"subtype": "initialize"},
            });
            (live, vec![initialize, user_message(content)])
        }
    };
    let turn = {
        let mut shared = live.core.shared();
        shared.last_used = Instant::now();
        shared.turn
    };
    // From here the round owns the child: dropping it unfinished retires it.
    let round = Round {
        live,
        turn,
        tool_ids: Vec::new(),
        stop_reason: None,
        done: false,
        complete: false,
    };
    for write in &writes {
        if let Err(error) = round.live.core.write(write).await {
            return Err(round.failure(&format!("could not reach the Claude Code CLI ({error})")));
        }
    }
    Ok(futures_util::stream::unfold(round, |mut round| async move {
        if round.done {
            return None;
        }
        let line = round.next().await;
        let chunk = Bytes::from(format!("data: {line}\n\n"));
        Some((Ok::<_, std::io::Error>(chunk), round))
    })
    .boxed())
}

/// One provider response: the events of a single assistant message.
struct Round {
    live: Arc<Conversation>,
    turn: u64,
    tool_ids: Vec<String>,
    stop_reason: Option<String>,
    done: bool,
    complete: bool,
}

impl Drop for Round {
    fn drop(&mut self) {
        // A response abandoned mid-message (cancel, timeout, error) leaves the
        // child's context out of step with RustCode's history.
        if !self.complete || self.live.ephemeral {
            retire(&self.live);
        }
        self.live.busy.store(false, Ordering::Release);
    }
}

impl Round {
    fn failure(&self, summary: &str) -> String {
        let shared = self.live.core.shared();
        let detail = shared.stderr_tail.trim();
        if detail.is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}: {detail}")
        }
    }

    fn fail(&mut self, summary: &str) -> Value {
        self.done = true;
        json!({"type": "error", "error": {"message": self.failure(summary)}})
    }

    async fn next(&mut self) -> Value {
        loop {
            let event = self.live.events.lock().await.recv().await;
            match event {
                None | Some(Event::Closed) => {
                    return self.fail("the Claude Code CLI exited before finishing its response");
                }
                Some(Event::Limits(info)) => {
                    if let Some(limits) = limits_event(&info) {
                        return limits;
                    }
                }
                Some(Event::Result { ordinal, error }) => {
                    {
                        let mut shared = self.live.core.shared();
                        shared.results_seen = shared.results_seen.max(ordinal);
                    }
                    if ordinal < self.turn {
                        continue;
                    }
                    // This turn ended without the assistant message we are
                    // waiting for: a usage limit, an API error, a refusal.
                    let summary = error.unwrap_or_else(|| {
                        "the Claude Code CLI ended the turn without a model response".into()
                    });
                    return self.fail(&summary);
                }
                Some(Event::Stream(mut event)) => {
                    // Anything still arriving from an earlier turn is not the
                    // answer to this one.
                    if self.live.core.shared().results_seen + 1 < self.turn {
                        continue;
                    }
                    // Redacted reasoning arrives as empty deltas; forwarding
                    // them would open an empty thinking block in the UI.
                    if event["delta"]["type"] == "thinking_delta"
                        && event["delta"]["thinking"]
                            .as_str()
                            .is_none_or(str::is_empty)
                    {
                        continue;
                    }
                    match event["type"].as_str() {
                        Some("message_start") => {
                            self.tool_ids.clear();
                            self.stop_reason = None;
                        }
                        Some("content_block_start")
                            if event["content_block"]["type"] == "tool_use" =>
                        {
                            let block = &mut event["content_block"];
                            if let Some(id) = block["id"].as_str() {
                                self.tool_ids.push(id.to_owned());
                            }
                            if let Some(name) = block["name"]
                                .as_str()
                                .and_then(|name| name.strip_prefix(TOOL_PREFIX))
                                .map(str::to_owned)
                            {
                                block["name"] = Value::String(name);
                            }
                        }
                        Some("message_delta") => {
                            self.stop_reason = event
                                .pointer("/delta/stop_reason")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                        Some("message_stop") => {
                            let mut shared = self.live.core.shared();
                            shared.phase = if self.stop_reason.as_deref() == Some("tool_use")
                                && !self.tool_ids.is_empty()
                            {
                                Phase::AwaitingTools(std::mem::take(&mut self.tool_ids))
                            } else {
                                Phase::Idle
                            };
                            shared.last_used = Instant::now();
                            self.done = true;
                            self.complete = true;
                        }
                        _ => {}
                    }
                    return event;
                }
            }
        }
    }
}

/// Translate the child's plan-limit report into the `rate_limits` shape the
/// stream loop already records for subscription accounts.
fn limits_event(info: &Value) -> Option<Value> {
    let window = |key: &str, minutes: u64| {
        let window = info.pointer(&format!("/unifiedWindows/{key}"))?;
        let used = window.get("utilization")?.as_f64()?;
        Some(json!({
            "used_percent": (used * 100.0).clamp(0.0, 100.0),
            "window_minutes": minutes,
            "resets_at": window.get("resetsAt").and_then(Value::as_i64),
        }))
    };
    let primary = window("five_hour", 5 * 60);
    let secondary = window("seven_day", 7 * 24 * 60);
    (primary.is_some() || secondary.is_some()).then(|| {
        json!({
            "type": "rate_limits",
            "rate_limits": {"primary": primary, "secondary": secondary},
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Value {
        json!({"role": "user", "content": text})
    }

    fn assistant(text: &str) -> Value {
        json!({"role": "assistant", "content": text})
    }

    fn calling(ids: &[&str]) -> Value {
        let calls: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id, "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}}))
            .collect();
        json!({"role": "assistant", "content": "", "tool_calls": calls})
    }

    fn tool(id: &str, text: &str) -> Value {
        json!({"role": "tool", "tool_call_id": id, "content": text})
    }

    fn refs(messages: &[Value]) -> Vec<&Value> {
        messages.iter().collect()
    }

    #[test]
    fn tool_results_continue_the_waiting_child_and_fill_gaps() {
        let history = [
            user("fix it"),
            calling(&["t1", "t2"]),
            tool("t1", "contents"),
            user("also check b.rs"),
        ];
        let phase = Phase::AwaitingTools(vec!["t1".into(), "t2".into()]);
        let Some(Plan::Continue { results, anchor }) = plan(&phase, &[], &refs(&history)) else {
            panic!("expected a continuation");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "t1");
        assert!(results[0].1.starts_with("contents"));
        assert!(results[0].1.contains("also check b.rs"));
        assert_eq!(results[1], ("t2".into(), MISSING_RESULT.into()));
        assert_eq!(anchor, Some(anchor_at(&refs(&history), 3)));

        // None of the awaited results are present: the histories diverged.
        let other = [user("fix it"), calling(&["x"]), tool("x", "contents")];
        assert!(plan(&phase, &[], &refs(&other)).is_none());
    }

    #[test]
    fn an_idle_child_only_takes_the_turn_after_the_one_it_answered() {
        let history = [
            user("first"),
            calling(&["t1"]),
            tool("t1", "ok"),
            assistant("done"),
            user("second"),
        ];
        let conversation = refs(&history);
        let anchors = [anchor_at(&conversation, 0)];
        let Some(Plan::NewTurn { content, anchor }) = plan(&Phase::Idle, &anchors, &conversation)
        else {
            panic!("expected a new turn");
        };
        assert_eq!(content, [json!({"type": "text", "text": "second"})]);
        assert_eq!(anchor, Some(anchor_at(&conversation, 4)));

        // Rewound to before the answered turn and asked something else.
        let rewound = [user("zero"), assistant("hi"), user("first, differently")];
        assert!(plan(&Phase::Idle, &anchors, &refs(&rewound)).is_none());
        // Same text at a different point in the conversation is not a match.
        let shifted = [
            user("zero"),
            assistant("hi"),
            user("first"),
            assistant("done"),
            user("second"),
        ];
        assert!(plan(&Phase::Idle, &anchors, &refs(&shifted)).is_none());
        // Compacted: no assistant message left to continue from.
        assert!(plan(&Phase::Idle, &anchors, &refs(&[user("summary")])).is_none());
        // A continuation request with no new user input still gets a turn.
        let Some(Plan::NewTurn { content, anchor }) =
            plan(&Phase::Idle, &anchors, &refs(&history[..4]))
        else {
            panic!("expected a new turn");
        };
        assert_eq!(content[0]["text"], "Continue.");
        assert_eq!(anchor, None);
    }

    #[test]
    fn a_new_child_gets_earlier_history_as_a_bounded_transcript() {
        let big = "x".repeat(MAX_TRANSCRIPT_TOOL_BYTES * 2);
        let history = [
            user("first"),
            calling(&["t1"]),
            tool("t1", &big),
            assistant("done"),
            user("second"),
            json!({"role": "user", "content": [{"type": "text", "text": "third"}]}),
        ];
        let conversation = refs(&history);
        let (content, anchors) = first_turn(&conversation, Some("overflow rules"));
        assert_eq!(content.len(), 4);
        assert!(
            content[0]["text"]
                .as_str()
                .unwrap()
                .contains("overflow rules")
        );
        let transcript = content[1]["text"].as_str().unwrap();
        assert!(transcript.contains("<user>\nfirst\n</user>"));
        assert!(transcript.contains("[called tool read_file with {\"path\":\"a.rs\"}]"));
        assert!(transcript.contains("[truncated]"));
        assert!(transcript.len() < big.len());
        assert!(!transcript.contains("second"));
        assert_eq!(content[2]["text"], "second");
        assert_eq!(content[3]["text"], "third");
        assert_eq!(anchors, [anchor_at(&conversation, 5)]);

        // A first message needs no transcript at all.
        let (content, anchors) = first_turn(&refs(&[user("hello")]), None);
        assert_eq!(content, [json!({"type": "text", "text": "hello"})]);
        assert_eq!(anchors.len(), 1);

        // Resumed mid tool round: nothing live to send, anchor to history.
        let (content, anchors) = first_turn(&refs(&history[..3]), None);
        assert_eq!(content.last().unwrap()["text"], "Continue.");
        assert_eq!(anchors, [anchor_at(&conversation, 0)]);
    }

    #[test]
    fn system_prompt_overflow_splits_on_a_character_boundary() {
        assert_eq!(split_system("short"), ("short", None));
        let long = "é".repeat(MAX_SYSTEM_ARG_BYTES);
        let (head, tail) = split_system(&long);
        assert!(head.len() <= MAX_SYSTEM_ARG_BYTES);
        assert_eq!(head.len() + tail.unwrap().len(), long.len());
    }

    #[test]
    fn tool_calls_are_parked_until_rustcode_has_a_result() {
        let schema = json!({"type": "function", "function": {
            "name": "read_file",
            "description": "Read a file",
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}},
        }});
        let tools = mcp_tools(&[schema]);
        let mut shared = Shared::new();
        let mcp = |id: u64, method: &str, params: Value| {
            json!({"type": "control_request", "request_id": format!("r{id}"), "request": {
                "subtype": "mcp_message",
                "server_name": MCP_SERVER,
                "message": {"jsonrpc": "2.0", "id": id, "method": method, "params": params},
            }})
        };
        let listed = control_reply(&tools, &mut shared, &mcp(1, "tools/list", json!({}))).unwrap();
        let listed = &listed["response"]["response"]["mcp_response"]["result"]["tools"];
        assert_eq!(listed[0]["name"], "read_file");
        assert_eq!(
            listed[0]["inputSchema"]["properties"]["path"]["type"],
            "string"
        );

        let call = |id: u64, tool_use: &str| {
            mcp(
                id,
                "tools/call",
                json!({"name": "read_file", "arguments": {}, "_meta": {"claudecode/toolUseId": tool_use}}),
            )
        };
        // No result yet: the request stays open.
        assert!(control_reply(&tools, &mut shared, &call(2, "toolu_a")).is_none());
        assert!(shared.parked.contains_key("toolu_a"));
        // The result arrived first: answered at once, exactly once.
        shared.results.insert("toolu_b".into(), "body".into());
        let reply = control_reply(&tools, &mut shared, &call(3, "toolu_b")).unwrap();
        assert_eq!(reply["response"]["request_id"], "r3");
        let response = &reply["response"]["response"]["mcp_response"];
        assert_eq!(response["id"], 3);
        assert_eq!(response["result"]["content"][0]["text"], "body");
        assert!(shared.results.is_empty());

        let permission = json!({"type": "control_request", "request_id": "p1", "request": {
            "subtype": "can_use_tool", "tool_name": "mcp__rc__read_file", "input": {"path": "a"},
        }});
        let allowed = control_reply(&tools, &mut shared, &permission).unwrap();
        assert_eq!(allowed["response"]["response"]["behavior"], "allow");
        assert_eq!(allowed["response"]["response"]["updatedInput"]["path"], "a");

        let unknown = json!({"type": "control_request", "request_id": "u1", "request": {"subtype": "hook_callback"}});
        let refused = control_reply(&tools, &mut shared, &unknown).unwrap();
        assert_eq!(refused["response"]["subtype"], "error");
    }

    #[test]
    fn plan_limits_map_to_the_subscription_quota_windows() {
        let event = limits_event(&json!({
            "status": "allowed",
            "unifiedWindows": {
                "five_hour": {"utilization": 0.23, "resetsAt": 1_791_383_400},
                "seven_day": {"utilization": 0.03, "resetsAt": 1_791_403_200},
            },
        }))
        .unwrap();
        let limits =
            crate::provider_auth::ProviderRateLimits::from_json(&event["rate_limits"], 0).unwrap();
        let primary = limits.primary.unwrap();
        assert!((primary.used_percent - 23.0).abs() < 1e-9);
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.resets_at, Some(1_791_383_400));
        assert_eq!(limits.secondary.unwrap().window_minutes, Some(10_080));
        assert!(limits_event(&json!({"status": "allowed"})).is_none());
    }

    /// A stand-in `claude`: answers one user turn with a tool call, waits for
    /// RustCode's result on the tunnelled MCP channel, then finishes the turn.
    #[cfg(unix)]
    const FAKE_CLI: &str = r#"#!/bin/sh
ev() { printf '{"type":"stream_event","parent_tool_use_id":null,"event":%s}\n' "$1"; }
while IFS= read -r line; do
  case "$line" in
    *'"subtype":"initialize"'*)
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"rustcode-init","response":{}}}\n' ;;
    *'"type":"user"'*)
      case "$line" in *'earlier_conversation'*) restored=yes ;; *) restored=no ;; esac
      printf '{"type":"rate_limit_event","rate_limit_info":{"unifiedWindows":{"five_hour":{"utilization":0.5,"resetsAt":99}}}}\n'
      ev '{"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":1}}}'
      ev '{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"mcp__rc__read_file","input":{}}}'
      ev '{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.rs\"}"}}'
      ev '{"type":"content_block_stop","index":0}'
      printf '{"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"mcp__rc__read_file","input":{"path":"a.rs"}}}\n'
      printf '{"type":"control_request","request_id":"c2","request":{"subtype":"mcp_message","server_name":"rc","message":{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"read_file","arguments":{"path":"a.rs"},"_meta":{"claudecode/toolUseId":"toolu_1"}}}}}\n'
      ev '{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}'
      ev '{"type":"message_stop"}' ;;
    *'"request_id":"c2"'*)
      case "$line" in *'file body'*) got=yes ;; *) got=no ;; esac
      ev '{"type":"message_start","message":{"usage":{"input_tokens":20,"output_tokens":1}}}'
      ev '{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":""}}'
      ev '{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}'
      ev "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"result=$got restored=$restored\"}}"
      ev '{"type":"content_block_stop","index":0}'
      ev '{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}'
      ev '{"type":"message_stop"}'
      printf '{"type":"result","subtype":"success","is_error":false,"result":"ok"}\n' ;;
  esac
done
"#;

    #[cfg(unix)]
    fn fake_cli(directory: &std::path::Path, script: &str) -> tokio::process::Command {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("claude");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = tokio::process::Command::new(path);
        command.kill_on_drop(true);
        command
    }

    #[cfg(unix)]
    async fn collect(stream: ByteStream) -> Vec<Value> {
        stream
            .map(|chunk| {
                let chunk = chunk.unwrap();
                let text = std::str::from_utf8(&chunk).unwrap();
                serde_json::from_str(text.strip_prefix("data: ").unwrap().trim()).unwrap()
            })
            .collect()
            .await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tool_round_trip_spans_two_requests_on_one_child() {
        let directory = tempfile::tempdir().unwrap();
        let session = format!("test-{}", uuid::Uuid::new_v4());
        let schema = json!({"type": "function", "function": {
            "name": "read_file", "description": "Read a file",
            "parameters": {"type": "object", "properties": {}},
        }});
        let mut history = vec![
            json!({"role": "system", "content": "Be brief."}),
            user("read a.rs"),
        ];
        fn request<'a>(
            session: &'a str,
            schema: &'a Value,
            messages: &'a [Value],
        ) -> RoundRequest<'a> {
            RoundRequest {
                session_id: session,
                model: "haiku",
                effort: None,
                messages,
                tool_schemas: std::slice::from_ref(schema),
                allow_tools: true,
            }
        }

        let first = start_round_with(
            fake_cli(directory.path(), FAKE_CLI),
            request(&session, &schema, &history),
        )
        .await
        .unwrap();
        let events = collect(first).await;
        assert_eq!(events[0]["type"], "rate_limits");
        let call = events
            .iter()
            .find(|event| event["type"] == "content_block_start")
            .unwrap();
        // The MCP prefix never reaches RustCode's dispatcher.
        assert_eq!(call["content_block"]["name"], "read_file");
        assert_eq!(events.last().unwrap()["type"], "message_stop");

        // The second request must reuse the child: a new one could not answer
        // the open tool call. Pass a command that cannot start to prove it.
        history.push(calling(&["toolu_1"]));
        history.push(tool("toolu_1", "file body"));
        let missing = tokio::process::Command::new(directory.path().join("missing"));
        let events = collect(
            start_round_with(missing, request(&session, &schema, &history))
                .await
                .unwrap(),
        )
        .await;
        let text = events
            .iter()
            .find(|event| event["delta"]["type"] == "text_delta")
            .unwrap();
        assert_eq!(text["delta"]["text"], "result=yes restored=no");
        assert!(
            events
                .iter()
                .all(|event| event["delta"]["type"] != "thinking_delta")
        );
        assert_eq!(events.last().unwrap()["type"], "message_stop");

        // A rewound history cannot continue the child, so a new one starts
        // and receives the earlier turns as a transcript.
        let rewound = [
            json!({"role": "system", "content": "Be brief."}),
            user("something else"),
            assistant("sure"),
            user("read a.rs again"),
        ];
        let missing = tokio::process::Command::new(directory.path().join("missing"));
        assert!(
            start_round_with(missing, request(&session, &schema, &rewound))
                .await
                .is_err()
        );
        let replay = start_round_with(
            fake_cli(directory.path(), FAKE_CLI),
            request(&session, &schema, &rewound),
        )
        .await
        .unwrap();
        assert_eq!(
            collect(replay).await.last().unwrap()["type"],
            "message_stop"
        );

        registry().retain(|live| {
            if live.session_id == session {
                live.kill();
            }
            live.session_id != session
        });
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_turn_that_ends_without_a_message_is_a_provider_error() {
        let directory = tempfile::tempdir().unwrap();
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo 'usage limit reached' >&2
      printf '{"type":"result","subtype":"success","is_error":true,"result":"You have hit your limit"}\n' ;;
  esac
done
"#;
        let session = format!("test-{}", uuid::Uuid::new_v4());
        let history = [user("hello")];
        let stream = start_round_with(
            fake_cli(directory.path(), script),
            RoundRequest {
                session_id: &session,
                model: "haiku",
                effort: None,
                messages: &history,
                tool_schemas: &[],
                allow_tools: true,
            },
        )
        .await
        .unwrap();
        let events = collect(stream).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "error");
        let message = events[0]["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("You have hit your limit"));
        // The failed child is not kept for the next request.
        assert!(registry().iter().all(|live| live.session_id != session));
    }
}
