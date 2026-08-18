use regex::Regex;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, OnceLock};
use std::time::Instant;

mod envelope;
mod exec;
mod filesystem;
mod misc;
mod search;

#[allow(unused_imports)]
pub use envelope::{ToolCallEnvelope, ToolErrorKind, ToolResultEnvelope};

pub(crate) use exec::{
    CommandProgressCallback, command_confirmation_preview, command_requires_confirmation,
    run_command_output_with_progress,
};

pub(crate) use filesystem::edit_target_and_replacement;

/// A parsed tool request emitted by a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    /// Provider/native identity when the source supplied one. Text protocols
    /// leave this unset and the execution boundary supplies a local identity.
    pub call_id: Option<String>,
}

/// Authoritative facts returned by a tool invocation alongside its display
/// text. Consumers must not reconstruct these fields from `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolExecutionOutput {
    pub(crate) content: String,
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) truncated: bool,
    /// True when the harness served a bounded cached read instead of running
    /// the tool again. This is execution state, not display prose.
    pub(crate) replayed: bool,
    pub(crate) error_kind: Option<ToolErrorKind>,
    pub(crate) retryable: bool,
}

impl ToolExecutionOutput {
    pub(crate) fn success(content: String) -> Self {
        Self {
            content,
            success: true,
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: None,
            retryable: false,
        }
    }

    pub(crate) fn failure(content: String) -> Self {
        Self {
            content,
            success: false,
            exit_code: None,
            truncated: false,
            replayed: false,
            error_kind: Some(ToolErrorKind::Internal),
            retryable: false,
        }
    }

    pub(crate) fn failure_with_kind(
        content: String,
        error_kind: ToolErrorKind,
        retryable: bool,
    ) -> Self {
        Self {
            error_kind: Some(error_kind),
            retryable,
            ..Self::failure(content)
        }
    }
}

/// How many calls that can change the workspace may run from one response.
///
/// The limit exists so each edit is grounded in the result of the previous one,
/// not to ration throughput: a model planning six edits ahead is predicting file
/// contents it has not read. Shell commands may still chain with any normal
/// operator because they are one call.
pub const MAX_MUTATING_CALLS_PER_RESPONSE: usize = 4;

/// Absolute ceiling on calls from one response, whatever their kind. Reads are
/// cheap and safe to fan out — searching six paths at once is one thought, not
/// six — so they are bounded only by this backstop against runaway generation.
pub const MAX_TOOL_CALLS_PER_RESPONSE: usize = 32;

/// Cut an over-eager batch down to the calls that may run this round, returning
/// the kept prefix and how many were dropped.
///
/// Rejecting the whole response teaches the model nothing: with no tool output
/// it re-plans from the same context and emits the same oversized batch again.
/// Running the leading calls puts real results in front of it instead, which is
/// the only thing that reliably corrects a model that has started inventing
/// tool output. Order is preserved because later calls were written expecting
/// the earlier ones to have run.
///
/// A control-plane call (`use_skill`) must execute alone, so it is either the
/// entire kept batch — when it leads — or the boundary the prefix stops at.
pub fn truncate_tool_batch(mut calls: Vec<ToolCall>) -> (Vec<ToolCall>, usize) {
    let total = calls.len();
    let is_control = |call: &ToolCall| matches!(tool_safety(&call.name), ToolSafety::ControlPlane);

    let keep = if calls.first().is_some_and(is_control) {
        1
    } else {
        let limit = calls.len().min(MAX_TOOL_CALLS_PER_RESPONSE);
        let mut mutating = 0;
        let mut kept = limit;
        for (index, call) in calls[..limit].iter().enumerate() {
            if is_control(call) {
                kept = index;
                break;
            }
            if !supports_parallel_execution(&call.name) {
                mutating += 1;
                if mutating > MAX_MUTATING_CALLS_PER_RESPONSE {
                    kept = index;
                    break;
                }
            }
        }
        kept
    };

    calls.truncate(keep);
    (calls, total - keep)
}

/// Validate parsed calls before they reach an executor. Text protocols are
/// intentionally permissive while parsing, but execution must be strict and
/// fail closed when the model emits an unknown tool or malformed arguments.
pub fn validate_tool_calls(calls: &[ToolCall]) -> Result<(), String> {
    if calls.len() > MAX_TOOL_CALLS_PER_RESPONSE {
        return Err(format!(
            "too many tool calls in one response ({}; maximum is {}); chain related shell operations inside one run_command and emit the next action after receiving results",
            calls.len(),
            MAX_TOOL_CALLS_PER_RESPONSE
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let has_control_plane = calls
        .iter()
        .any(|call| matches!(tool_safety(&call.name), ToolSafety::ControlPlane));

    if has_control_plane && calls.len() > 1 {
        return Err(
            "control-plane calls such as use_skill must be emitted alone; retry the deferred action in the next turn"
                .to_string(),
        );
    }

    for call in calls {
        let fingerprint = format!("{}:{}", call.name, call.arguments);
        if !seen.insert(fingerprint) {
            return Err(format!("duplicate tool call rejected: {}", call.name));
        }

        let Some(schema) = registered_tool_schema(&call.name) else {
            return Err(format!(
                "unknown or unavailable tool '{}'; use only tools in the current registry",
                call.name
            ));
        };

        if let Err(reason) = validate_value_against_schema(&call.arguments, &schema, "$") {
            let guidance = tool_argument_guidance(&call.name).unwrap_or_default();
            return Err(format!(
                "invalid arguments for '{}'. Schema path: {reason}.{guidance}",
                call.name
            ));
        }
    }

    Ok(())
}

fn registered_tool_schema(name: &str) -> Option<Value> {
    if let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) {
        return Some(schema_for_tool(tool.name));
    }
    if let Some((_, _, schema)) = collect_mcp_tools().into_iter().find(|(n, _, _)| n == name) {
        return Some(schema);
    }
    if AGENT_TOOL_SPECS.iter().any(|(n, _, _)| *n == name) {
        return Some(schema_for_agent_tool(name));
    }
    None
}

fn example_value_for_schema(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut object = serde_json::Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object)
                && let Some(required) = schema.get("required").and_then(Value::as_array)
            {
                for field in required.iter().filter_map(Value::as_str) {
                    if let Some(property) = properties.get(field) {
                        object.insert(field.to_string(), example_value_for_schema(property));
                    }
                }
            }
            Value::Object(object)
        }
        Some("array") => schema
            .get("items")
            .map(example_value_for_schema)
            .map(|item| Value::Array(vec![item]))
            .unwrap_or_else(|| Value::Array(Vec::new())),
        Some("boolean") => Value::Bool(false),
        Some("integer") => Value::from(1),
        Some("number") => Value::from(1),
        Some("string") => Value::String("...".to_string()),
        _ => Value::Null,
    }
}

fn tool_argument_guidance(name: &str) -> Option<String> {
    let schema = registered_tool_schema(name)?;
    let properties = schema.get("properties").and_then(Value::as_object)?;
    let keys = properties
        .keys()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let example = if name == "replace_file_content" {
        serde_json::json!({
            "path": "src/example.ts",
            "edits": [{"old_string": "old", "new_string": "new"}]
        })
    } else {
        example_value_for_schema(&schema)
    };
    let example = serde_json::to_string(&example).unwrap_or_else(|_| "{}".to_string());
    Some(format!(
        " Expected arguments for '{name}' use these keys: [{keys}]. Example: {example}"
    ))
}

fn validate_value_against_schema(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let expected = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");
    let type_matches = match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        _ => true,
    };
    if !type_matches {
        return Err(format!("{path} must be {expected}"));
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("{path}.{field} is required"));
                }
            }
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && let Some(properties) = schema.get("properties").and_then(Value::as_object)
            && let Some(unknown) = object.keys().find(|key| !properties.contains_key(*key))
        {
            return Err(format!("{path}.{unknown} is not an advertised argument"));
        }
        if let Some(ap_schema) = schema.get("additionalProperties").filter(|v| v.is_object())
            && let Some(obj) = value.as_object()
        {
            for (key, val) in obj {
                validate_value_against_schema(val, ap_schema, &format!("{path}.{key}"))?;
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in properties {
                if let Some(actual) = object.get(key) {
                    validate_value_against_schema(actual, child, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    if let Some(items) = schema.get("items")
        && let Some(array) = value.as_array()
    {
        for (index, item) in array.iter().enumerate() {
            validate_value_against_schema(item, items, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub struct BackgroundTaskInfo {
    pub id: String,
    pub command: String,
    pub start_time: Instant,
    pub child_pid: Option<u32>,
    pub cancel_sender: Option<tokio::sync::oneshot::Sender<()>>,
}

pub fn get_background_tasks() -> &'static StdMutex<HashMap<String, BackgroundTaskInfo>> {
    static TASKS: OnceLock<StdMutex<HashMap<String, BackgroundTaskInfo>>> = OnceLock::new();
    TASKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

type WakeupCallback = Box<dyn Fn(String, String, ToolExecutionOutput) + Send + Sync + 'static>;

pub(crate) static WAKEUP_CALLBACK: OnceLock<WakeupCallback> = OnceLock::new();

pub fn register_wakeup_callback<F>(cb: F)
where
    F: Fn(String, String, ToolExecutionOutput) + Send + Sync + 'static,
{
    let _ = WAKEUP_CALLBACK.set(Box::new(cb));
}

thread_local! {
    static ACTIVE_SESSION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static ACTIVE_WORKSPACE_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub fn set_active_session_id(id: Option<String>) {
    ACTIVE_SESSION_ID.with(|f| {
        *f.borrow_mut() = id;
    });
}

pub fn get_active_session_id() -> Option<String> {
    ACTIVE_SESSION_ID.with(|f| f.borrow().clone())
}

pub fn set_active_workspace_root(root: Option<PathBuf>) {
    ACTIVE_WORKSPACE_ROOT.with(|current| {
        *current.borrow_mut() = root;
    });
}

pub(crate) fn resolve_tool_path(raw_path: &str) -> PathBuf {
    let p = Path::new(raw_path);

    if !p.is_absolute()
        && let Some(root) = ACTIVE_WORKSPACE_ROOT.with(|current| current.borrow().clone())
    {
        return root.join(p);
    }

    // Check if the path contains a component named "sandbox"
    let mut parts_sandbox = Vec::new();
    let mut found_sandbox = false;
    for component in p.components() {
        let name = component.as_os_str();
        if found_sandbox {
            parts_sandbox.push(name);
        } else if name == "sandbox" {
            found_sandbox = true;
        }
    }

    if found_sandbox
        && let Some(session_id) = get_active_session_id()
        && let Some(sandbox_dir) = crate::config::get_active_session_sandbox_dir(&session_id)
    {
        let mut resolved = sandbox_dir;
        for part in parts_sandbox {
            resolved.push(part);
        }
        return resolved;
    }

    // Check if the path contains a component named "artifacts"
    let mut parts_artifacts = Vec::new();
    let mut found_artifacts = false;
    for component in p.components() {
        let name = component.as_os_str();
        if found_artifacts {
            parts_artifacts.push(name);
        } else if name == "artifacts" {
            found_artifacts = true;
        }
    }

    if found_artifacts
        && let Some(session_id) = get_active_session_id()
        && let Some(artifacts_dir) = crate::config::get_active_session_artifacts_dir(&session_id)
    {
        let mut resolved = artifacts_dir;
        for part in parts_artifacts {
            resolved.push(part);
        }
        return resolved;
    }

    if (raw_path.starts_with("~/") || raw_path == "~")
        && let Ok(home) = std::env::var("HOME")
    {
        let tail = raw_path.strip_prefix('~').unwrap_or("");
        let tail = tail.strip_prefix('/').unwrap_or(tail);
        return PathBuf::from(home).join(tail);
    }

    PathBuf::from(raw_path)
}

pub(crate) fn parse_json_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(s) = v.as_str() {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

/// Read a JSON array argument, tolerating a provider that delivered it as a
/// stringified JSON array (`"[{...}]"`) instead of a real array — some strict
/// function-calling backends do this despite the schema.
pub(crate) fn coerce_array(v: &Value) -> Option<Vec<Value>> {
    if let Some(a) = v.as_array() {
        return Some(a.clone());
    }
    if let Some(s) = v.as_str()
        && let Ok(Value::Array(a)) = serde_json::from_str::<Value>(s)
    {
        return Some(a);
    }
    None
}

pub(crate) fn parse_json_bool(v: &Value) -> Option<bool> {
    if let Some(b) = v.as_bool() {
        Some(b)
    } else if let Some(s) = v.as_str() {
        match s.to_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

/// A fully self-contained built-in tool definition. Adding a new built-in
/// tool means writing one `pub const …: Tool` literal in the module that holds
/// its handler and referencing it from the `TOOLS` slice below — no other
/// tables need updating, since schema, capabilities, and safety all live here.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,

    pub arguments: &'static str,
    pub handler: fn(&Value) -> Result<String, String>,
    /// If true, the agent loop will pause and show a Y/N confirmation modal
    /// to the user before executing. Use for destructive tools (write, create, run).
    pub requires_confirmation: bool,
    /// Canonical JSON Schema advertised to API-native providers. The text
    /// protocol still uses `arguments` as compact documentation, but native
    /// providers must receive real types, required fields, and nested item
    /// schemas.
    pub schema: fn() -> Value,
    /// Runtime capabilities used to enforce agent modes and safety policy.
    pub capabilities: &'static [ToolCapability],
    /// Execution safety class used by the scheduler to decide which calls may
    /// safely run concurrently.
    pub safety: ToolSafety,
}

/// Runtime capabilities used to enforce agent modes and safety policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCapability {
    ReadWorkspace,
    WriteWorkspace,
    ExecuteCommands,
    Network,
    UserInteraction,
    AgentDelegation,
    SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Allow,
    RequireConfirmation,
    Deny(String),
}

/// Single authorization policy used by every execution path. Unknown tools
/// are never silently treated as safe; registered MCP tools must still opt in
/// through confirmation unless the caller has explicitly bypassed it.
pub fn authorize_tool(
    name: &str,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    bypass_confirmation: bool,
) -> AuthorizationDecision {
    authorize_tool_with_args(name, &Value::Null, mode, auto_confirm, bypass_confirmation)
}

pub fn authorize_tool_with_args(
    name: &str,
    args: &Value,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    bypass_confirmation: bool,
) -> AuthorizationDecision {
    if mode == crate::config::AgentMode::Plan && !allowed_in_plan_mode(name) {
        return AuthorizationDecision::Deny(
            "Plan mode blocks workspace mutation, command execution, delegation, and unknown tools"
                .to_string(),
        );
    }
    let command_is_destructive = name == "run_command" && command_requires_confirmation(args);
    let requires_confirmation = if name == "run_command" {
        command_is_destructive
    } else {
        needs_confirmation(name)
    };
    if !bypass_confirmation
        && !auto_confirm
        && (requires_confirmation || matches!(tool_safety(name), ToolSafety::Unknown))
    {
        return AuthorizationDecision::RequireConfirmation;
    }
    AuthorizationDecision::Allow
}

/// Return the capabilities of a built-in or agent tool.
/// Unknown tools (including MCP tools) deliberately receive no capabilities;
/// callers must opt them into a mode explicitly instead of assuming safety.
pub fn tool_capabilities(name: &str) -> &'static [ToolCapability] {
    use ToolCapability::*;
    if let Some(tool) = TOOLS.iter().find(|t| t.name == name) {
        return tool.capabilities;
    }
    // Agent tools live outside `TOOLS`; keep their capabilities here.
    match name {
        "spawn_agent" | "send_agent" | "set_goal" => &[AgentDelegation, SessionState],
        "todo_write" => &[SessionState],
        _ => &[],
    }
}

/// Plan mode is intentionally deny-by-default for tools not explicitly known
/// to be read-only or user-facing.
pub fn allowed_in_plan_mode(name: &str) -> bool {
    use ToolCapability::*;
    let capabilities = tool_capabilities(name);
    capabilities.iter().all(|cap| {
        matches!(
            cap,
            ReadWorkspace | Network | UserInteraction | SessionState
        )
    }) && (capabilities.contains(&ReadWorkspace)
        || capabilities.contains(&Network)
        || capabilities.contains(&UserInteraction)
        || name == "get_time"
        || name == "use_skill"
        || name == "todo_write")
}

/// Registry of built-in tools. Each entry is a self-contained `Tool`
/// definition colocated with its handler in the sibling module; this slice
/// only fixes the ordering in which tools are advertised.
pub const TOOLS: &[Tool] = &[
    misc::ASK_QUESTION,
    misc::GET_TIME,
    search::GREP,
    search::GLOB,
    search::LIST_DIRECTORY,
    filesystem::DELETE_FILE,
    filesystem::MOVE_FILE,
    filesystem::COPY_FILE,
    exec::RUN_COMMAND,
    exec::MANAGE_TASK,
    misc::SEARCH_WEB,
    search::FIND_SYMBOL,
    search::GET_PROJECT_MAP,
    filesystem::VIEW_FILE,
    filesystem::REPLACE_FILE_CONTENT,
    filesystem::MULTI_REPLACE_FILE_CONTENT,
    filesystem::WRITE_TO_FILE,
    misc::COMPLETE_TASK,
    misc::USE_SKILL,
    misc::REMEMBER,
    misc::RECALL_MEMORY,
    misc::FORGET_MEMORY,
];

pub fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent" | "send_agent" | "set_goal" | "todo_write"
    )
}

/// Execution capability used by the scheduler to decide which calls may
/// safely run concurrently. Unknown and stateful tools are conservative by
/// default and must not be parallelized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSafety {
    #[allow(dead_code)]
    ControlPlane,
    ReadOnly,
    WorkspaceMutation,
    ProcessControl,
    Interactive,
    Delegation,
    Unknown,
}

pub fn tool_safety(name: &str) -> ToolSafety {
    if let Some(tool) = TOOLS.iter().find(|t| t.name == name) {
        return tool.safety;
    }
    // Tools that live outside `TOOLS`: the agent tools, plus the legacy
    // `background_output`/`write_stdin` names kept for safety classification.
    match name {
        "spawn_agent" | "send_agent" | "set_goal" | "todo_write" => ToolSafety::Delegation,
        "background_output" | "write_stdin" => ToolSafety::ProcessControl,
        _ => ToolSafety::Unknown,
    }
}

pub fn supports_parallel_execution(name: &str) -> bool {
    matches!(tool_safety(name), ToolSafety::ReadOnly)
}

/// Enforce a control-plane barrier. A control-plane call such as `use_skill`
/// must execute alone so its result can affect the next model request before
/// any side-effecting call from the same response is considered.
pub fn isolate_control_plane_call(calls: Vec<ToolCall>) -> (Vec<ToolCall>, usize) {
    let Some(index) = calls
        .iter()
        .position(|call| matches!(tool_safety(&call.name), ToolSafety::ControlPlane))
    else {
        return (calls, 0);
    };

    let control_call = calls[index].clone();
    (vec![control_call], calls.len().saturating_sub(1))
}

/// Agent tools that live outside the `TOOLS` table. `(name, description, args)`
/// mirrors what `tool_system_prompt` lists for the text protocols, reused here
/// to build the native function schema.
const AGENT_TOOL_SPECS: &[(&str, &str, &str)] = &[
    (
        "spawn_agent",
        "Delegate a task to a read-only subagent. Write access, allowed paths, and verification must be explicit.",
        r#"{"task": "task description", "write_access": false, "allowed_paths": ["src/"], "verification_command": "cargo test"}"#,
    ),
    (
        "send_agent",
        "Send a follow-up message to a running subagent.",
        r#"{"id": "subagent id", "message": "message text"}"#,
    ),
    (
        "set_goal",
        "Set a new long-running task and switch the agent to continuous autoloop mode.",
        r#"{"goal": "goal description"}"#,
    ),
    (
        "todo_write",
        "Replace the persistent task plan with a list of steps.",
        r#"{"todos": "list of steps, each with content, status and priority"}"#,
    ),
];

/// Derive a permissive JSON Schema object from a tool's human-readable
/// `arguments` string (e.g. `{"path": "file path", "start_line": optional}`).
/// Every parameter is declared as an optional `string`; the tool handlers
/// already coerce strings to numbers/bools (see `parse_json_number`/
/// `parse_json_bool`), so this stays correct without a real schema per tool.
fn schema_from_arguments(arguments: &str) -> Value {
    let mut properties = serde_json::Map::new();
    let bytes = arguments.as_bytes();
    let read_string = |start: usize| -> (String, usize) {
        // `start` points just past the opening quote; returns (contents, index of closing quote).
        let mut j = start;
        while j < bytes.len() && bytes[j] != b'"' {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        (arguments[start..j.min(arguments.len())].to_string(), j)
    };
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let (token, end) = read_string(i + 1);
        // A key is a string immediately followed (past whitespace) by ':'.
        let mut k = end + 1;
        while k < bytes.len() && (bytes[k] as char).is_whitespace() {
            k += 1;
        }
        if k < bytes.len() && bytes[k] == b':' {
            // Optional description: the following string literal, if any.
            let mut m = k + 1;
            while m < bytes.len() && (bytes[m] as char).is_whitespace() {
                m += 1;
            }
            // An array-literal value (`[...]`) marks a structured param even
            // when no description string follows it (e.g. `options`).
            let is_array_literal = m < bytes.len() && bytes[m] == b'[';
            let desc = if m < bytes.len() && bytes[m] == b'"' {
                read_string(m + 1).0
            } else {
                String::new()
            };
            let mut prop = serde_json::Map::new();
            // Params whose description says "array" carry structured JSON (e.g.
            // `edits`, `replacements`). Advertising them as `string` makes
            // strict ApiNative providers stringify the value, which the handlers
            // then fail to read via `as_array()`. Emit a real array schema so the
            // model passes structured data. Everything else stays an optional
            // string (handlers coerce scalars via parse_json_number/bool).
            if is_array_literal || desc.to_lowercase().contains("array") {
                prop.insert("type".into(), Value::String("array".into()));
                prop.insert("items".into(), serde_json::json!({ "type": "object" }));
            } else {
                prop.insert("type".into(), Value::String("string".into()));
            }
            if !desc.is_empty() {
                prop.insert("description".into(), Value::String(desc));
            }
            properties
                .entry(token)
                .or_insert_with(|| Value::Object(prop));
        }
        i = end + 1;
    }
    serde_json::json!({ "type": "object", "properties": properties })
}

/// Build the OpenAI-style `tools` array sent in the request when the tool
/// protocol is `ApiNative`. Covers the built-in `TOOLS`, any MCP tools (which
/// carry real JSON Schemas), and the agent tools.
/// Gather all connected MCP tools as `(name, description, input_schema)`,
/// sorted by name for a deterministic, cache-stable ordering. Shared by both
/// the native schema builder and the text-protocol prompt listing.
fn mcp_canonical_name(server: &str, tool: &str) -> String {
    let server = server
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    format!("mcp__{server}__{tool}")
}

fn mcp_canonical_name_for_clients(
    server: &str,
    tool: &str,
    clients: &[Arc<crate::mcp::McpClient>],
) -> String {
    let base = mcp_canonical_name(server, tool);
    let collides = clients
        .iter()
        .filter(|client| mcp_canonical_name(&client.name, tool) == base)
        .count()
        > 1;
    if !collides {
        return base;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server.hash(&mut hasher);
    format!("{base}__{:016x}", hasher.finish())
}

fn mcp_raw_name_is_unique(
    name: &str,
    clients: &[Arc<crate::mcp::McpClient>],
) -> bool {
    if TOOLS.iter().any(|tool| tool.name == name)
        || AGENT_TOOL_SPECS.iter().any(|(tool, _, _)| *tool == name)
    {
        return false;
    }
    clients
        .iter()
        .filter_map(|client| client.get_tools().ok())
        .flatten()
        .filter(|tool| tool.get("name").and_then(|value| value.as_str()) == Some(name))
        .count()
        == 1
}

fn collect_mcp_tools() -> Vec<(String, String, Value)> {
    let mut discovered = Vec::new();
    let mut clients_for_names = Vec::new();
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = reg.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        clients_for_names = clients.clone();
        for client in &clients {
            if let Ok(mcp_tools) = client.get_tools() {
                for tool in mcp_tools {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    let desc = tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();
                    let schema = tool
                        .get("inputSchema")
                        .filter(|schema| {
                            schema.is_object()
                                && schema.get("type").and_then(Value::as_str) == Some("object")
                        })
                        .cloned()
                        .unwrap_or_else(|| {
                            serde_json::json!({"type": "object", "properties": {}})
                        });
                    discovered.push((client.name.clone(), name.to_string(), desc, schema));
                }
            }
        }
    }
    discovered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut counts = HashMap::new();
    for (_, name, _, _) in &discovered {
        *counts.entry(name.clone()).or_insert(0usize) += 1;
    }
    let mut out = Vec::new();
    let mut emitted = std::collections::HashSet::new();
    for (server, raw_name, desc, schema) in discovered {
        let qualified = counts.get(&raw_name).copied().unwrap_or(0) > 1
            || TOOLS.iter().any(|tool| tool.name == raw_name)
            || AGENT_TOOL_SPECS.iter().any(|(name, _, _)| *name == raw_name);
        let name = if qualified {
            mcp_canonical_name_for_clients(&server, &raw_name, &clients_for_names)
        } else {
            raw_name
        };
        if emitted.insert(name.clone()) {
            out.push((name, desc, schema));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn native_tools_schema(include_agent_tools: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    for t in TOOLS {
        tools.push(serde_json::json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": schema_for_tool(t.name),
            }
        }));
    }
    // MCP tools, emitted in a deterministic (name-sorted) order. The registry is
    // a HashMap, so iterating it directly yields a hash-dependent order that can
    // shift after a rehash and silently break the provider's prefix cache. A
    // stable byte-for-byte layout keeps the cached prefix valid across turns.
    for (name, desc, schema) in collect_mcp_tools() {
        tools.push(serde_json::json!({
            "type": "function",
            "function": { "name": name, "description": desc, "parameters": schema }
        }));
    }
    if include_agent_tools {
        for (name, desc, _args) in AGENT_TOOL_SPECS {
            tools.push(serde_json::json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": desc,
                    "parameters": schema_for_agent_tool(name),
                }
            }));
        }
    }
    tools
}

/// Canonical JSON Schema for a built-in tool, resolved from its `Tool`
/// definition. Unknown names fall back to an empty permissive object schema.
fn schema_for_tool(name: &str) -> Value {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| (t.schema)())
        .unwrap_or_else(|| schema_from_arguments("{}"))
}

fn schema_for_agent_tool(name: &str) -> Value {
    match name {
        "spawn_agent" => {
            serde_json::json!({"type":"object","properties":{"task":{"type":"string"},"write_access":{"type":"boolean","default":false},"allowed_paths":{"type":"array","items":{"type":"string"}},"verification_command":{"type":"string"}},"required":["task"]})
        }
        "send_agent" => {
            serde_json::json!({"type":"object","properties":{"id":{"type":"string"},"message":{"type":"string"}},"required":["id","message"]})
        }
        "set_goal" => {
            serde_json::json!({"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"]})
        }
        "todo_write" => serde_json::json!({
            "type":"object", "properties": {"todos": {"type":"array", "items": {"type":"object", "properties": {
                "content":{"type":"string"}, "status":{"type":"string","enum":["pending","in_progress","completed"]},
                "priority":{"type":"string","enum":["high","medium","low"]}
            }, "required":["content"]}}}, "required":["todos"]
        }),
        _ => schema_from_arguments("{}"),
    }
}

pub fn tool_system_prompt(
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
) -> String {
    let mut p = String::new();

    let skills = crate::skills::discover_skills();
    if !skills.is_empty() {
        p.push_str("\n# Available Skills\n");
        p.push_str("Skills provide specialized instructions and workflows for specific tasks.\n");
        p.push_str(
            "ALWAYS check your task intent against the available skills below at the START of a task. \
             If a skill matches the task (such as `git-feature-workflow` for git/feature changes, or `release-automation` for releases), \
             you MUST invoke `use_skill` immediately as your FIRST action to load its workflow.\n\n",
        );
        p.push_str("<available_skills>\n");
        for skill in &skills {
            p.push_str("  <skill>\n");
            p.push_str(&format!("    <name>{}</name>\n", skill.name));
            p.push_str(&format!(
                "    <description>{}</description>\n",
                skill.description
            ));
            p.push_str("  </skill>\n");
        }
        p.push_str("</available_skills>\n\n");
    }

    if agent_mode == crate::config::AgentMode::Plan {
        p.push_str(
            "CRITICAL: You are operating in PLAN MODE (Read-only / Design mode).\n\
             - File writing, deletion, shell commands, delegation, and unknown tools are disabled.\n\
             - You can read, search, ask questions, and design solutions, but you CANNOT modify files or execute commands.\n\
             - Investigate before planning. `grep`, `glob`, and `view_file` are available and you are expected to use them: read the manifest, find the real call sites, and confirm which crates and patterns the project already uses.\n\
             - The plan must be specific to THIS repository. Name the files to change, the functions and structs involved, and the line ranges you inspected. A step that says to go find out where something lives is not a plan — resolve it now, while you have the tools.\n\
             - Never guess at dependencies, argument-parsing libraries, or module layout: those are in the repository, so read them. State what you verified and what remains uncertain.\n\
             - Explain the plan and tell the user to switch to Build Mode (press Tab) to implement it.\n\n"
        );
    }

    p.push_str(
        "You are rustcode, a terminal-based coding assistant.\n\
- Use `sandbox/` for temporary scripts/builds, and `artifacts/` for persistent designs/reports.\n\
- For long commands (>2s, e.g. build, test, install), set `\"background\": true` in `run_command`.\n\n\
- `run_command` executes the complete `command` string through the platform shell. Chain related shell commands when that is clearer and efficient: use `&&` for dependent steps and `;` for independent observations (for example, `git status --short --branch; git log -5 --oneline`). Keep destructive operations inspectable and do not hide a required failure with `;`.\n\
# Rules\n\
- Be concise and direct. No filler or preamble. Execute tools immediately without conversational fluff.\n\
- Keep responses concise, but include changed files, verification, blockers, and next steps when relevant.\n\
- DO NOT add code comments (such as `// ...` or `/* ... */`) to code files unless explicitly requested by the user.\n\
- After edits, inspect the result and run the most relevant check when safe and useful; then report what changed and what was verified.\n\
- When the `git-feature-workflow` skill is available and the task changes files, load and follow it: inspect branch/status first, preserve unrelated work, create a feature branch, stage only this feature, verify, push, create/merge the PR, then return to `main` and pull. Never use `git add .` when unrelated changes may exist.\n\
- Treat tool results as the source of truth for verification: never claim a check, test, formatter, or lint command passed unless its observed exit code is 0. If the harness blocks completion for stale or failed verification, run a fresh relevant check after the latest edit and report the actual result.\n\
- Stage only explicit feature paths in Git. Broad staging commands such as `git add .`, `git add -A`, and `git add --all` are rejected so unrelated user changes remain untouched.\n\
- Choose verification from the project structure: first locate the nearest `Cargo.toml`, `package.json`, `pyproject.toml`, or equivalent manifest. Run project checks from that project root. Do NOT run `cargo check` on a standalone `.rs` file outside a Cargo project; use an appropriate standalone checker such as `rustc` only when practical, or clearly report that project verification is not applicable.\n\
- Tool results are authoritative evidence. If a tool or compiler check reports an error, fix it before giving a final answer. Never replace a concrete tool result with a claim that tools were unavailable.\n\
- A subagent's report is advisory, not proof that work is complete or blocked. If a subagent says it could not use tools, continue the task yourself and inspect the workspace directly.\n\
- Explore first: use `grep` or `glob` to locate exact function definitions before reading. DO NOT page through large files from line 1 to end with sequential `view_file` calls — use `grep` first to find line numbers, then `view_file` only the target section.\n\
- Editing an existing file: use `replace_file_content` (for a single edit, pass `target_content` and `replacement_content` with `start_line` and `end_line`; for multiple edits, pass an `edits` array with `old_string` and `new_string`). Use `write_to_file` only to create a new file or fully rewrite one. `multi_replace_file_content` is a niche variant that needs exact line numbers and exact text — prefer `replace_file_content`, whose matching is more forgiving. Before modifying an existing file, you MUST inspect its actual content using `view_file` or `grep`. Never guess or hallucinate line numbers, imports, dependencies, or struct fields for files you have not inspected in this session.\n\
- ISSUE INDEPENDENT READS TOGETHER: `view_file`, `grep`, `glob`, `list_directory`, `find_symbol`, `get_project_map`, `search_web`, and `use_skill` run in parallel when emitted in the same response, so when you need several facts or skills at once, ask for them at once — searching four paths or loading two skills is one thought, not two turns. Reads whose arguments depend on an earlier result must of course wait for it.\n\
- ONE CHANGE AT A TIME: anything that writes, runs a command, or delegates (`replace_file_content`, `write_to_file`, `run_command`, `spawn_agent`, …) executes alone and must be grounded in results you already have. Emit at most 4 such calls in a response, and prefer one. Never output a speculative chain that predicts its own results — edits, builds, commits, and a PR in a single turn is a story about what might happen, not work.\n\
- Chaining shell commands is different from speculative tool batching: it is encouraged for small, related, inspectable command sequences, especially status/log/diff checks and the verified publish sequence. Inspect output before deciding the next mutation.\n\
- Prefer the native `view_file` and `grep` tools for inspecting and searching files, which provide structured line ranges and context management.\n\
- Match project code style.\n\
- Before adding new code, study how the nearest EXISTING code does the same thing (sibling functions, other match arms, similar handlers) and mirror its patterns — function signatures, how shared state/locks are passed, error handling. Do NOT invent a new pattern when neighbors establish one; diverging from local conventions is a common source of subtle bugs (deadlocks, double-locks, lifetime issues) that compile fine but break at runtime.\n\
- Prefer the smallest effective tool sequence: locate first, inspect only the relevant range, make one focused change, then verify from the correct project root. Do not repeat successful reads or run broad checks unrelated to the files changed.\n\
- Run focused tests or checks after code changes unless the user says not to. When modifying algorithms, visual curves, or complex logic, verify edge/boundary conditions and add or update unit tests to prove correctness before completing the task.
- Ask before expensive or externally visible operations.\n\
- Read-only tools run immediately; modifying/destructive tools require confirmation.\n\
- Use `ask_question` ONLY when you require clarification on ambiguous user requirements, design choices, or need explicit user validation before proceeding. Do NOT invoke `ask_question` for routine tool calls or trivial confirmations. The UI automatically appends a 'write your own answer' slot with interactive text input, so NEVER include an 'Other' or 'Write your own' option in the options array.
- When the task is complete, output a plain-text final summary (with no tool block).\n\n\
# Working memory & avoiding loops\n\
- BACKGROUND TASKS & WAITING: When a background task is running (e.g. from `run_command` with `\"background\": true`), completion notifications arrive automatically when it finishes. Do NOT poll `manage_task` with action `status` or `list` in a loop while waiting for a background task — stop calling tools now so execution pauses until completion.\n\
- If a tool execution or compiler check returns compilation errors or warnings, prioritize fixing them immediately before proceeding to other steps.
- File contents you have already read this session are STILL VISIBLE in the conversation. Do NOT re-read a file you already have unless it changed on disk.
- Do not repeat a tool call you just made with the same arguments. If a tool call returns an error, correct your arguments or approach instead of repeating the identical call. If a read or search came up empty, change your query or your approach rather than retrying.
- An edit that reports \"already applied\" changed nothing on disk; re-issuing the identical edit will report the same no-op again, not succeed differently. Neither a no-op nor a failed edit counts as progress, and the harness ends the turn after a handful of either in a row — re-read the file or change your approach instead of repeating the call.
- Use `todo_write` ONLY for complex code refactors or multi-stage tasks (3+ steps). For routine tasks, git operations, single-file edits, or simple questions, DO NOT use `todo_write` — execute tools directly. Do not update `todo_write` after every single command; only update it when completing major milestones.\n\n"
    );

    p.push_str(
        "# Delegation Policy\n\\
- Do not spawn subagents unless the user explicitly requests delegation/parallel agent work or applicable project instructions require it.\n\\
- Before delegating, identify the critical path and keep blockers in the main agent. Delegate only bounded, self-contained side tasks with clear outputs and disjoint write scopes.\n\\
- Review every subagent result and inspect its workspace changes before treating the task as complete.\n\\n",
    );

    p.push_str("# Tool Format\n");
    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "To call a tool, output ONLY fenced `tool` blocks containing a single JSON object each. Do not output any conversational text or narration before or after the block.\n\n\
                ```tool\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}\n\
                ```\n\n\
                Rules:\n\
                - Keys must be \"name\" and \"arguments\".\n\
                - Pass correct type for arguments (no quotes for numbers/booleans).\n\
                - Use the ```tool fence ONLY. Never use ```tool_code, ```json, or any other fence for tool calls, and never repeat the same call in multiple fences.\n\
                - You may emit several ```tool fences in one response: independent reads in that batch run in parallel, and calls that change the workspace or run a command execute one after another, each grounded in the previous result. Never emit a fence whose arguments depend on a result you do not have yet — request it in a later response instead.\n\n"
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "To call a tool, output ONLY the tool call tag using native format. Do not output any conversational text or narration before or after the tag.\n\n\
                [TOOL_CALLS]tool_name[ARGS]{\"arg_name\": \"value\"}\n\n\
                Rules:\n\
                - Format must be [TOOL_CALLS]tool_name[ARGS]{...}.\n\
                - Arguments must be a valid JSON object matching the tool parameters.\n\n"
            );
        }
        crate::config::ToolProtocol::ApiNative => {
            p.push_str(
                "Tools are provided to you through the API's native function-calling interface. \
                Invoke them directly through that interface — do NOT print tool calls as text or JSON in your reply. \
                When the task is complete, reply with a plain-text summary and no tool call.\n\n"
            );
        }
    }

    // Text protocols enumerate tools in the prompt. ApiNative carries the full
    // tool schema in the request's `tools` field instead, so listing them here
    // would only duplicate that and waste context.
    if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        return p;
    }

    p.push_str("Available tools:\n");
    for t in TOOLS {
        if agent_mode == crate::config::AgentMode::Plan && !allowed_in_plan_mode(t.name) {
            continue;
        }
        p.push_str(&format!(
            "- {} | Args: {} | {}\n",
            t.name, t.arguments, t.description
        ));
    }
    for (name, desc, schema) in collect_mcp_tools() {
        if agent_mode == crate::config::AgentMode::Plan {
            continue;
        }
        p.push_str(&format!(
            "- {} | Args: {} | {}\n",
            name,
            serde_json::to_string(&schema).unwrap_or_default(),
            desc
        ));
    }
    if include_agent_tools && agent_mode != crate::config::AgentMode::Plan {
        p.push_str(
            "- spawn_agent | Args: {\"task\": \"task description\"} | Delegate task to a fresh subagent.\n\
            - send_agent | Args: {\"id\": subagent_id, \"message\": \"message\"} | Send follow-up to subagent.\n\
            - set_goal | Args: {\"goal\": \"goal description\"} | Set a new long-running task and switch the agent to continuous autoloop mode.\n\
            - todo_write | Args: {\"todos\": [{\"content\": \"step\", \"status\": \"pending|in_progress|completed\", \"priority\": \"high|medium|low\"}]} | Replace the persistent task plan. Use this at the start of multi-step work and update it as steps finish.\n",
        );
    }

    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
```tool\n\
{\"name\": \"grep\", \"arguments\": {\"pattern\": \"agent loop\", \"include\": \"*.rs\"}}\n\
```\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
[TOOL_CALLS]grep[ARGS]{\"pattern\": \"agent loop\", \"include\": \"*.rs\"}\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        // ApiNative returns early above (tools come from the request schema, not
        // the prompt), so this arm is unreachable but keeps the match exhaustive.
        crate::config::ToolProtocol::ApiNative => {}
    }

    p
}

fn extract_tool_call(json: &Value) -> Option<(String, Value)> {
    let top_level_name = json.get("name").and_then(|v| v.as_str());

    let mut args = if let Some(args_val) = json.get("arguments") {
        args_val.clone()
    } else if let Some(obj) = json.as_object() {
        let mut map = serde_json::Map::new();
        for (k, v) in obj {
            if k != "name" {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map)
    } else {
        Value::Object(Default::default())
    };

    let name = match top_level_name {
        Some(name) => name.to_string(),
        None => {
            // Tool name nested inside `arguments`: recover it as the tool name
            // and strip it from the args. Only done when there is no top-level
            // `name` — otherwise an argument literally called `name` (e.g.
            // use_skill's skill name) is legitimate and must be kept.
            match args.get("name").and_then(|v| v.as_str()) {
                Some(nested) => {
                    let nested = nested.to_string();
                    if nested != "use_skill"
                        && let Some(obj) = args.as_object_mut()
                    {
                        obj.remove("name");
                    }
                    nested
                }
                // No `name` anywhere. Some models drop the field entirely on
                // large-content calls. If the argument keys uniquely match one
                // tool's required signature, infer it rather than hard-failing
                // and forcing a retry loop the model tends not to recover from.
                None => infer_tool_name_from_args(&args)?.to_string(),
            }
        }
    };

    Some((name, args))
}

/// Best-effort recovery for tool calls that omit `name` entirely. Only
/// returns a match when the argument keys are distinctive enough that no
/// other tool could plausibly be meant.
fn infer_tool_name_from_args(args: &Value) -> Option<&'static str> {
    let obj = args.as_object()?;
    let has = |k: &str| obj.contains_key(k);

    if has("content") && has("path") {
        Some("write_to_file")
    } else if has("replacements") && has("path") {
        Some("multi_replace_file_content")
    } else if has("old_string") && has("new_string") && has("path") {
        Some("replace_file_content")
    } else if has("command") && obj.len() <= 2 {
        Some("run_command")
    } else if has("result") && obj.len() == 1 {
        Some("complete_task")
    } else {
        None
    }
}

fn repair_json(s: &str) -> String {
    let trimmed = s.trim_end();
    let mut s_clean = trimmed.to_string();
    if s_clean.ends_with(',') {
        s_clean.pop();
    }

    let mut repaired = String::with_capacity(s_clean.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    let mut stack = Vec::new();

    for c in s_clean.chars() {
        if escaped {
            escaped = false;
            repaired.push(c);
            continue;
        }
        if c == '\\' && in_string {
            escaped = true;
            repaired.push(c);
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            repaired.push(c);
            continue;
        }
        if in_string {
            match c {
                '\n' => repaired.push_str("\\n"),
                '\r' => repaired.push_str("\\r"),
                '\t' => repaired.push_str("\\t"),
                _ => repaired.push(c),
            }
            continue;
        }

        repaired.push(c);
        if c == '{' {
            stack.push('}');
        } else if c == '[' {
            stack.push(']');
        } else if (c == '}' || c == ']')
            && let Some(&last) = stack.last()
            && last == c
        {
            stack.pop();
        }
    }

    if in_string {
        repaired.push('"');
    }

    while let Some(close_char) = stack.pop() {
        repaired.push(close_char);
    }

    repaired
}

static TOOL_CALLS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[TOOL_CALLS\]\s*([a-zA-Z0-9_-]+)[\":]*\s*(?:\[ARGS\])?[\":]*\s*(\{[\s\S]*)"#)
        .unwrap()
});
static BRACE_OBJ_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^{}]*\}").unwrap());

fn parse_tool_calls_tags(text: &str, calls: &mut Vec<ToolCall>) {
    if text.contains("[TOOL_CALLS]") {
        let re = &*TOOL_CALLS_RE;
        for chunk in text.split("[TOOL_CALLS]") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let full = format!("[TOOL_CALLS]{chunk}");
            if let Some(caps) = re.captures(&full) {
                let name = caps.get(1).unwrap().as_str().to_string();
                let raw_args = caps.get(2).unwrap().as_str();

                let repaired = repair_json(raw_args);
                if let Ok(json_val) = serde_json::from_str::<Value>(&repaired) {
                    calls.push(ToolCall {
                        name,
                        arguments: json_val,
                        call_id: None,
                    });
                } else {
                    let pattern = &*BRACE_OBJ_RE;
                    if let Some(mat) = pattern.find(raw_args)
                        && let Ok(json_val) = serde_json::from_str::<Value>(mat.as_str())
                    {
                        calls.push(ToolCall {
                            name,
                            arguments: json_val,
                            call_id: None,
                        });
                    }
                }
            }
        }
    }
}

fn parse_tool_calls_fenced(text: &str, calls: &mut Vec<ToolCall>) {
    // Walk every ```tool fence, not just the first, so a model can batch
    // multiple tool calls in one turn (the executor runs them in parallel).
    // `find("```tool")` also matches ```tool_code (Gemini's code-exec fence);
    // require the fence tag to be exactly `tool` (next char whitespace) so we
    // skip those without eating the real call.
    let mut search = text;
    while let Some(rel) = search.find("```tool") {
        let after_tag = &search[rel + 7..];
        let (block, next) = match after_tag.find("```") {
            Some(end) => (&after_tag[..end], &after_tag[end + 3..]),
            None => (after_tag, ""),
        };

        let is_tool_fence = after_tag.chars().next().is_none_or(|c| c.is_whitespace());
        if is_tool_fence {
            let repaired = repair_json(block.trim());
            if let Ok(json_value) = serde_json::from_str::<Value>(&repaired)
                && let Some(call) = extract_tool_call(&json_value)
            {
                calls.push(ToolCall {
                    name: call.0,
                    arguments: call.1,
                    call_id: None,
                });
            }
        }

        if next.is_empty() {
            break;
        }
        search = next;
    }
}

/// When a tool call was clearly intended but produced zero parseable calls,
/// return a specific reason (the underlying JSON error and the offending block)
/// so the retry nudge can tell the model exactly what to fix instead of a vague
/// "malformed" message it tends to reproduce verbatim. Returns `None` when the
/// text was parseable or contained no recognizable tool syntax.
pub fn diagnose_failed_tool_call(text: &str) -> Option<String> {
    // Look at every ```tool fence; report the first that fails to parse or validate.
    let mut search = text;
    while let Some(rel) = search.find("```tool") {
        let after_tag = &search[rel + 7..];
        let (block, next) = match after_tag.find("```") {
            Some(end) => (&after_tag[..end], &after_tag[end + 3..]),
            None => (after_tag, ""),
        };
        let is_tool_fence = after_tag.chars().next().is_none_or(|c| c.is_whitespace());
        if is_tool_fence {
            let repaired = repair_json(block.trim());
            match serde_json::from_str::<Value>(&repaired) {
                Ok(val) => {
                    let has_name = val.get("name").is_some()
                        || val.get("arguments").and_then(|a| a.get("name")).is_some()
                        || val
                            .get("arguments")
                            .and_then(infer_tool_name_from_args)
                            .is_some();
                    if !has_name {
                        let snippet: String = block.trim().chars().take(240).collect();
                        return Some(format!(
                            "Missing required 'name' field in tool call JSON. Every tool call must include `\"name\": \"tool_name\"`. Offending block:\n```\n{snippet}\n```"
                        ));
                    }
                    if let Some((name, args)) = extract_tool_call(&val) {
                        if let Err(err) = validate_tool_calls(&[ToolCall {
                            name: name.clone(),
                            arguments: args,
                            call_id: None,
                        }]) {
                            let snippet: String = block.trim().chars().take(240).collect();
                            return Some(format!(
                                "Tool call '{name}' failed validation: {err}. Offending block:\n```\n{snippet}\n```"
                            ));
                        }
                    }
                }
                Err(e) => {
                    let snippet: String = block.trim().chars().take(240).collect();
                    return Some(format!(
                        "JSON parse error: {e}. A common cause is a backslash inside a string: every literal `\\` in the file must be written as `\\\\` in the JSON, and a real newline must be `\\n`. Offending block:\n```\n{snippet}\n```"
                    ));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        search = next;
    }
    None
}

fn parse_tool_calls_impl(text: &str, protocol: crate::config::ToolProtocol) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    match protocol {
        crate::config::ToolProtocol::Native => {
            parse_tool_calls_tags(text, &mut calls);
            if calls.is_empty() {
                parse_tool_calls_fenced(text, &mut calls);
            }
        }
        crate::config::ToolProtocol::Json | crate::config::ToolProtocol::ApiNative => {
            // ApiNative: the stream reader translates the provider's structured
            // `tool_calls` into the same fenced `tool` block the Json path emits,
            // so both parse identically.
            parse_tool_calls_fenced(text, &mut calls);
            if calls.is_empty() {
                parse_tool_calls_tags(text, &mut calls);
            }
        }
    }

    // If no tool blocks found, try to parse the whole text as JSON (with repair if it starts with '{')
    if calls.is_empty() {
        let cleaned = text.trim();
        let to_parse = if cleaned.starts_with('{') {
            repair_json(cleaned)
        } else {
            cleaned.to_string()
        };
        if let Ok(json_value) = serde_json::from_str::<Value>(&to_parse)
            && let Some(call) = extract_tool_call(&json_value)
        {
            calls.push(ToolCall {
                name: call.0,
                arguments: call.1,
                call_id: None,
            });
        }
    }

    // Try to find JSON objects in the text
    if calls.is_empty() {
        let pattern = &*BRACE_OBJ_RE;
        for mat in pattern.find_iter(text) {
            let json_str = mat.as_str();
            if let Ok(json_value) = serde_json::from_str::<Value>(json_str)
                && let Some(call) = extract_tool_call(&json_value)
            {
                calls.push(ToolCall {
                    name: call.0,
                    arguments: call.1,
                    call_id: None,
                });
            }
        }
    }

    calls.dedup();
    calls
}

pub fn parse_tool_calls(text: &str, protocol: crate::config::ToolProtocol) -> Vec<ToolCall> {
    let raw_calls = parse_tool_calls_impl(text, protocol);
    let mut unique_calls = Vec::new();
    for call in raw_calls {
        if !unique_calls
            .iter()
            .any(|existing: &ToolCall| existing == &call)
        {
            unique_calls.push(call);
        }
    }
    unique_calls
}

pub fn is_code_editing_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content" | "multi_replace_file_content" | "write_to_file"
    )
}

pub fn is_tool_call_start(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.contains("```tool")
        || trimmed.contains("```json")
        || trimmed.contains("[TOOL_CALLS]")
        || trimmed.contains("<tool_call>")
        || trimmed.contains("<function_call>")
        || trimmed.contains("\"tool_name\"")
        || trimmed.contains("\"tool_call\"")
        || (trimmed.contains('{')
            && (trimmed.contains("\"name\"")
                || trimmed.contains("\"tool\"")
                || trimmed.contains("\"action\"")
                || trimmed.contains("\"function\"")))
}

pub fn parse_tool_call(text: &str, protocol: crate::config::ToolProtocol) -> Option<ToolCall> {
    parse_tool_calls(text, protocol).into_iter().next()
}

/// Present a handler failure as the model-facing `error:` line.
///
/// Handlers are inconsistent about whether their message already opens with
/// `error:`, and prefixing unconditionally produced `error: error: ...`, which
/// reads like the harness lost track of its own output.
fn as_error_message(message: &str) -> String {
    let trimmed = message.trim_start();
    if trimmed.to_ascii_lowercase().starts_with("error:") {
        trimmed.to_string()
    } else {
        format!("error: {trimmed}")
    }
}

pub(crate) fn execute_with_metadata(name: &str, args: &Value) -> ToolExecutionOutput {
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        let mut clients = reg.values().cloned().collect::<Vec<_>>();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        for client in &clients {
            if let Ok(tools) = client.get_tools()
                && tools
                    .iter()
                    .find_map(|t| {
                        let raw = t.get("name").and_then(|n| n.as_str())?;
                        let canonical =
                            mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                            .then_some(raw)
                    })
                    .is_some()
            {
                let handle = tokio::runtime::Handle::current();
                let client_clone = Arc::clone(&client);
                let name_owned = name.to_string();
                let args_clone = args.clone();
                let raw_name = tools
                    .iter()
                    .find_map(|tool| {
                        let raw = tool.get("name").and_then(|n| n.as_str())?;
                        let canonical =
                            mcp_canonical_name_for_clients(&client.name, raw, &clients);
                        (name == canonical || (name == raw && mcp_raw_name_is_unique(name, &clients)))
                            .then_some(raw.to_string())
                    })
                    .unwrap_or(name_owned.clone());

                let res = handle.block_on(async move {
                    client_clone
                        .call(
                            "tools/call",
                            serde_json::json!({
                                "name": raw_name,
                                "arguments": args_clone
                            }),
                        )
                        .await
                });

                return match res {
                    Ok(val) => {
                        let success = !val
                            .get("result")
                            .and_then(|result| result.get("isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if let Some(content_arr) = val
                            .get("result")
                            .and_then(|r| r.get("content"))
                            .and_then(|c| c.as_array())
                        {
                            let mut text_parts = Vec::new();
                            for item in content_arr {
                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                    text_parts.push(text.to_string());
                                }
                            }
                            ToolExecutionOutput {
                                content: text_parts.join("\n"),
                                success,
                                exit_code: None,
                                truncated: false,
                                replayed: false,
                                error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                                retryable: false,
                            }
                        } else {
                            ToolExecutionOutput {
                                content: serde_json::to_string_pretty(&val).unwrap_or_default(),
                                success,
                                exit_code: None,
                                truncated: false,
                                replayed: false,
                                error_kind: (!success).then_some(ToolErrorKind::McpFailed),
                                retryable: false,
                            }
                        }
                    }
                    Err(e) => {
                        ToolExecutionOutput::failure_with_kind(
                            format!("error: MCP tool call failed: {e}"),
                            ToolErrorKind::McpFailed,
                            true,
                        )
                    }
                };
            }
        }
    }

    if name == "run_command" {
        return match exec::run_command_output(args) {
            Ok(output) => output,
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::CommandFailed,
                true,
            ),
        };
    }
    if name == "view_file" {
        return match filesystem::view_file_output(args) {
            Ok(output) => ToolExecutionOutput {
                content: output.content,
                success: true,
                exit_code: None,
                truncated: output.truncated,
                replayed: false,
                error_kind: None,
                retryable: false,
            },
            Err(error) => ToolExecutionOutput::failure_with_kind(
                as_error_message(&error),
                ToolErrorKind::InvalidArguments,
                false,
            ),
        };
    }

    match TOOLS.iter().find(|t| t.name == name) {
        Some(tool) => match (tool.handler)(args) {
            Ok(out) => ToolExecutionOutput::success(out),
            Err(e) => ToolExecutionOutput::failure(as_error_message(&e)),
        },
        None => ToolExecutionOutput::failure_with_kind(
            format!(
                "error: unknown tool '{name}'. Available: {}",
                TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
            ),
            ToolErrorKind::UnavailableDependency,
            false,
        ),
    }
}

#[allow(
    dead_code,
    reason = "preserved display-only interface for direct callers"
)]
pub fn execute(name: &str, args: &Value) -> String {
    execute_with_metadata(name, args).content
}

pub fn needs_confirmation(name: &str) -> bool {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.requires_confirmation)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_have_unique_names() {
        let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            TOOLS.len(),
            "duplicate tool names in TOOLS registry"
        );
    }

    #[test]
    fn every_tool_schema_is_a_json_object() {
        for tool in TOOLS {
            let schema = (tool.schema)();
            assert_eq!(
                schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "tool '{}' schema must be a JSON object with type \"object\"",
                tool.name
            );
            assert!(
                schema.get("properties").is_some(),
                "tool '{}' schema must declare properties",
                tool.name
            );
        }
    }

    #[test]
    fn policy_tables_match_registry() {
        use ToolCapability::*;
        // Representative spot-checks against the pre-refactor lookup tables.
        assert_eq!(tool_capabilities("grep"), &[ReadWorkspace]);
        assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
        assert!(!needs_confirmation("grep"));

        assert_eq!(tool_capabilities("run_command"), &[ExecuteCommands]);
        assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
        assert!(needs_confirmation("run_command"));

        assert_eq!(tool_capabilities("use_skill"), &[SessionState]);
        assert_eq!(tool_safety("use_skill"), ToolSafety::ReadOnly);
        assert!(!needs_confirmation("use_skill"));

        // manage_task deliberately stays Unknown (confirmation via authorize_tool).
        assert_eq!(tool_capabilities("manage_task"), &[ExecuteCommands]);
        assert_eq!(tool_safety("manage_task"), ToolSafety::Unknown);
        assert!(!needs_confirmation("manage_task"));

        // Agent tools live outside TOOLS and keep their fallback arms.
        assert_eq!(
            tool_capabilities("spawn_agent"),
            &[AgentDelegation, SessionState]
        );
        assert_eq!(tool_safety("spawn_agent"), ToolSafety::Delegation);
        assert!(!needs_confirmation("spawn_agent"));
        assert_eq!(tool_capabilities("todo_write"), &[SessionState]);
        assert_eq!(tool_safety("background_output"), ToolSafety::ProcessControl);
        assert_eq!(tool_safety("unknown_tool_xyz"), ToolSafety::Unknown);
    }

    #[test]
    fn schema_from_arguments_extracts_string_props() {
        let schema = schema_from_arguments(
            r#"{"pattern": "regex pattern", "path": "optional dir", "ignore_case": optional bool}"#,
        );
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(props["pattern"]["type"], "string");
        assert_eq!(props["pattern"]["description"], "regex pattern");
        // Non-string values (bool/number) still register as optional string props.
        assert_eq!(props["ignore_case"]["type"], "string");
        assert!(props.contains_key("path"));
    }

    #[test]
    fn schema_from_arguments_marks_array_params() {
        // Description says "array" → real array schema, not string.
        let schema = schema_from_arguments(
            r#"{"path": "file path", "edits": "optional array of {old_string, new_string}"}"#,
        );
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props["edits"]["type"], "array");
        assert_eq!(props["edits"]["items"]["type"], "object");
        assert_eq!(props["path"]["type"], "string");

        // Array-literal value with no description → still an array.
        let schema2 =
            schema_from_arguments(r#"{"question": "q", "options": ["Option 1", "Option 2"]}"#);
        let props2 = schema2["properties"].as_object().unwrap();
        assert_eq!(props2["options"]["type"], "array");
    }

    #[test]
    fn coerce_array_accepts_real_and_stringified() {
        assert_eq!(coerce_array(&serde_json::json!([1, 2])).unwrap().len(), 2);
        assert_eq!(
            coerce_array(&serde_json::json!("[1, 2, 3]")).unwrap().len(),
            3
        );
        assert!(coerce_array(&serde_json::json!("not json")).is_none());
        assert!(coerce_array(&serde_json::json!(5)).is_none());
    }

    #[test]
    fn schema_from_arguments_handles_no_args() {
        let schema = schema_from_arguments("{} (no arguments)");
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"].as_object().unwrap().is_empty());
    }

    #[test]
    fn native_tools_schema_covers_builtins_and_agent_tools() {
        let tools = native_tools_schema(true);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        // Every entry is a well-formed function tool.
        assert!(tools.iter().all(|t| t["type"] == "function"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"view_file"));
        assert!(names.contains(&"complete_task"));
        // Agent tools are included when requested.
        assert!(names.contains(&"spawn_agent"));
        assert!(names.contains(&"todo_write"));
        // Excluded when not requested.
        let no_agents = native_tools_schema(false);
        assert!(
            !no_agents
                .iter()
                .any(|t| t["function"]["name"] == "spawn_agent")
        );
    }

    #[test]
    fn native_tools_schema_requires_explicit_delegation() {
        let disabled = native_tools_schema(false);
        let enabled = native_tools_schema(true);
        assert!(disabled.iter().all(|t| {
            !matches!(
                t["function"]["name"].as_str(),
                Some("spawn_agent") | Some("send_agent")
            )
        }));
        assert!(
            enabled
                .iter()
                .any(|t| t["function"]["name"] == "spawn_agent")
        );
        assert!(
            enabled
                .iter()
                .any(|t| t["function"]["name"] == "send_agent")
        );
    }

    #[test]
    fn mcp_names_are_deterministically_qualified() {
        assert_eq!(
            mcp_canonical_name("build-server", "run"),
            "mcp__build_server__run"
        );
        assert_eq!(
            mcp_canonical_name("build-server", "run"),
            mcp_canonical_name("build-server", "run")
        );
        assert_ne!(
            mcp_canonical_name("build-server", "run"),
            mcp_canonical_name("test-server", "run")
        );
    }

    #[test]
    fn test_repair_json() {
        assert_eq!(repair_json("{\"name\": \"test\""), "{\"name\": \"test\"}");
        assert_eq!(
            repair_json("{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\""),
            "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\"}}"
        );
        assert_eq!(
            repair_json(
                "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello"
            ),
            "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello\"}}"
        );
    }

    #[test]
    fn test_parse_truncated_tool_call() {
        let text = "```tool\n{\"name\": \"write_to_file\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_to_file");
        assert_eq!(
            calls[0].arguments.get("path").unwrap().as_str().unwrap(),
            "/foo"
        );
        assert_eq!(
            calls[0].arguments.get("content").unwrap().as_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn diagnose_reports_specific_json_error() {
        // Invalid JSON (bad escape + trailing junk) that repair can't rescue.
        let text =
            "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"a\\zb\"} garbage}\n```";
        let diag = diagnose_failed_tool_call(text);
        assert!(diag.is_some(), "should diagnose an unparseable fence");
        let d = diag.unwrap();
        assert!(d.contains("JSON parse error"), "got: {d}");
        assert!(d.contains("Offending block"), "should echo the block: {d}");
    }

    #[test]
    fn diagnose_ignores_valid_tool_call() {
        let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"x\"}}\n```";
        assert!(diagnose_failed_tool_call(text).is_none());
    }

    #[test]
    fn diagnose_reports_validation_error() {
        let text = "```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```";
        let diag = diagnose_failed_tool_call(text);
        assert!(diag.is_some(), "should diagnose invalid arguments");
        let d = diag.unwrap();
        assert!(d.contains("failed validation"), "got: {d}");
        assert!(d.contains("grep"), "got: {d}");
    }

    #[test]
    fn diagnose_reports_schema_guidance_for_an_invalid_edit_shape() {
        let text = "```tool\n{\"name\": \"replace_file_content\", \"arguments\": {\"path\": \"src/store.ts\", \"replacements\": []}}\n```";
        let diag = diagnose_failed_tool_call(text).expect("should diagnose invalid arguments");
        assert!(
            diag.contains("replacements"),
            "must identify the invalid field: {diag}"
        );
        assert!(
            diag.contains("Expected arguments"),
            "must show the expected shape: {diag}"
        );
        assert!(
            diag.contains("\"edits\""),
            "must name the valid batch field: {diag}"
        );
        assert!(
            diag.contains("Example"),
            "must include a minimal valid example: {diag}"
        );
    }

    #[test]
    fn validation_error_names_schema_path_and_valid_example() {
        let error = validate_tool_calls(&[ToolCall {
            name: "replace_file_content".to_string(),
            arguments: serde_json::json!({
                "path": "src/store.ts",
                "edits": "[]"
            }),
            call_id: None,
        }])
        .expect_err("a stringified edits array must remain invalid");

        assert!(
            error.contains("Schema path: $.edits must be array"),
            "{error}"
        );
        assert!(error.contains("Expected arguments"), "{error}");
        assert!(error.contains("Example"), "{error}");
    }

    #[test]
    fn diagnose_reports_unknown_tool_as_unavailable() {
        let text = "```tool\n{\"name\": \"not_a_real_tool\", \"arguments\": {}}\n```";
        let diag = diagnose_failed_tool_call(text).expect("should diagnose unknown tool");
        assert!(diag.contains("unknown or unavailable tool"), "got: {diag}");
        assert!(
            diag.contains("not_a_real_tool"),
            "must name the unknown tool: {diag}"
        );
    }

    #[test]
    fn test_parse_tool_calls_tag() {
        let text1 = "Let me check...[TOOL_CALLS]glob[ARGS]{\"pattern\": \"**/*.rs\"}";
        let calls1 = parse_tool_calls(text1, crate::config::ToolProtocol::Json);
        assert_eq!(calls1.len(), 1);
        assert_eq!(calls1[0].name, "glob");
        assert_eq!(
            calls1[0]
                .arguments
                .get("pattern")
                .unwrap()
                .as_str()
                .unwrap(),
            "**/*.rs"
        );

        let text2 = "Let me check...[TOOL_CALLS]glob\":{\"pattern\":\"**/*.rs\"}";
        let calls2 = parse_tool_calls(text2, crate::config::ToolProtocol::Json);
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].name, "glob");
        assert_eq!(
            calls2[0]
                .arguments
                .get("pattern")
                .unwrap()
                .as_str()
                .unwrap(),
            "**/*.rs"
        );

        let text3 = "Plan:[TOOL_CALLS]todo_write[ARGS]{\"todos\": [{\"content\": \"Fix bug\"}]}";
        let calls3 = parse_tool_calls(text3, crate::config::ToolProtocol::Json);
        assert_eq!(calls3.len(), 1);
        assert_eq!(calls3[0].name, "todo_write");
    }

    #[test]
    fn test_repair_json_escapes_multiline_string_literals() {
        let text = "```tool\n{\"name\": \"replace_file_content\", \"arguments\": {\"path\": \"src/config.rs\", \"edits\": [{\"old_string\": \"line1\", \"new_string\": \"line1\nline2\nline3\"}]}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "replace_file_content");
    }

    #[test]
    fn test_is_tool_call_start_detects_json_and_embedded_tool_syntax() {
        assert!(is_tool_call_start("```tool\n{\"name\": \"run_command\"}"));
        assert!(is_tool_call_start("```json\n{\"name\": \"run_command\"}"));
        assert!(is_tool_call_start("[TOOL_CALLS]"));
        assert!(is_tool_call_start("<tool_call>"));
        assert!(is_tool_call_start(
            "Let me execute this:\n{\"action\": \"manage_task\", \"task_id\": \"task-123\"}"
        ));
        assert!(!is_tool_call_start(
            "Here is a regular markdown code block:\n```rust\nfn main() {}\n```"
        ));
    }

    #[test]
    fn test_diagnose_reports_missing_name_field() {
        let text = "```tool\n{\"arguments\": {\"path\": \"src/config.rs\"}}\n```";
        let diag = diagnose_failed_tool_call(text).unwrap();
        assert!(diag.contains("Missing required 'name' field"));
    }

    #[test]
    fn test_extract_tool_call_recovers_name_nested_in_arguments() {
        let text = "```tool\n{\"arguments\": {\"name\": \"multi_replace_file_content\", \"path\": \"src/config.rs\", \"replacements\": []}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "multi_replace_file_content");
        assert_eq!(
            calls[0].arguments.get("path").unwrap().as_str().unwrap(),
            "src/config.rs"
        );
        assert!(calls[0].arguments.get("name").is_none());
    }

    #[test]
    fn test_extract_tool_call_keeps_legitimate_name_argument() {
        // use_skill's only parameter is literally called `name`; with a proper
        // {"name", "arguments"} envelope it must survive extraction.
        let text =
            "```tool\n{\"name\": \"use_skill\", \"arguments\": {\"name\": \"spotify\"}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "use_skill");
        assert_eq!(
            calls[0].arguments.get("name").unwrap().as_str().unwrap(),
            "spotify"
        );
        // And the full validation path accepts it.
        validate_tool_calls(&calls).unwrap();
    }

    #[test]
    fn test_parse_tool_call_with_escaped_quotes_in_values() {
        // Reproduces the exact shape from session 1785743879558 MSG 7:
        // \\\" inside a JSON string value (e.g. serde rename_all = \"lowercase\")
        let text = "```tool\n{\"name\":\"replace_file_content\",\"arguments\":{\"edits\":[{\"new_string\":\"#[derive(Debug)]\\n#[serde(rename_all = \\\"lowercase\\\")]\\npub enum Foo {\",\"old_string\":\"#[derive(Debug)]\\npub enum Foo {\"}],\"path\":\"src/config.rs\"}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(
            calls.len(),
            1,
            "Expected 1 call, got {}: {:?}",
            calls.len(),
            calls
        );
        assert_eq!(calls[0].name, "replace_file_content");
        assert_eq!(
            calls[0].arguments.get("path").unwrap().as_str().unwrap(),
            "src/config.rs"
        );
    }

    #[test]
    fn test_parse_multiple_fenced_tool_calls() {
        // Two distinct ```tool blocks in one turn → both parsed, in order.
        let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    some prose\n\
                    ```tool\n{\"name\": \"view_file\", \"arguments\": {\"path\": \"src/x.rs\"}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "grep");
        assert_eq!(
            calls[0].arguments.get("pattern").unwrap().as_str().unwrap(),
            "foo"
        );
        assert_eq!(calls[1].name, "view_file");
        assert_eq!(
            calls[1].arguments.get("path").unwrap().as_str().unwrap(),
            "src/x.rs"
        );
    }

    #[test]
    fn test_tool_code_decoy_is_skipped() {
        // Gemini habit: a real ```tool block plus redundant ```tool_code /
        // ```json decoys of the SAME call. Only one call, and the tool_code
        // fence must not be mis-parsed into garbage.
        let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    ```tool_code\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}\n```\n\
                    ```json\n{\"tool_code\": {\"name\": \"grep\", \"arguments\": {\"pattern\": \"foo\"}}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "grep");
    }

    #[test]
    fn test_two_tool_calls_with_tool_code_between() {
        // A tool_code decoy sitting between two real, distinct calls must be
        // skipped without swallowing the second call.
        let text = "```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"a\"}}\n```\n\
                    ```tool_code\n{\"name\": \"noise\", \"arguments\": {}}\n```\n\
                    ```tool\n{\"name\": \"glob\", \"arguments\": {\"pattern\": \"*.rs\"}}\n```";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "grep");
        assert_eq!(calls[1].name, "glob");
    }

    #[test]
    fn test_replace_file_content_tool_aliases_and_batch_edits() {
        let temp_dir = std::env::temp_dir().join(format!("rustcode_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("test.rs");
        std::fs::write(&test_file, "fn foo() {}\nfn bar() {}\n").unwrap();

        // 1. Single edit using old_string / new_string aliases
        let args = serde_json::json!({
            "path": test_file.to_str().unwrap(),
            "old_string": "fn foo() {}",
            "new_string": "fn foo_updated() {}"
        });
        let res = filesystem::replace_file_content_tool(&args).unwrap();
        assert!(res.contains("successfully replaced"));
        // The diff reflects the real two-line file (changed line 1, unchanged
        // line 2 as context), not a fabrication derived purely from the
        // single-line old_string/new_string arguments.
        assert!(res.contains("@@ -1,2 +1,2 @@"), "got: {res}");
        assert!(res.contains("-fn foo() {}"), "got: {res}");
        assert!(res.contains("+fn foo_updated() {}"), "got: {res}");
        let updated = std::fs::read_to_string(&test_file).unwrap();
        assert!(updated.contains("fn foo_updated() {}"));

        // 2. Batch edits using edits array
        let args_batch = serde_json::json!({
            "path": test_file.to_str().unwrap(),
            "edits": [
                {"old_string": "fn foo_updated() {}", "new_string": "fn foo_v2() {}"},
                {"old_string": "fn bar() {}", "new_string": "fn bar_v2() {}"}
            ]
        });
        let res_batch = filesystem::replace_file_content_tool(&args_batch).unwrap();
        assert!(res_batch.contains("successfully applied 2 edits"));
        let final_content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(final_content, "fn foo_v2() {}\nfn bar_v2() {}\n");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_view_file_tool_directory_fallback() {
        let temp_dir =
            std::env::temp_dir().join(format!("rustcode_dir_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        std::fs::write(temp_dir.join("sample.txt"), "hello").unwrap();

        let args = serde_json::json!({
            "path": temp_dir.to_str().unwrap()
        });
        let res = filesystem::view_file_tool(&args).unwrap();
        assert!(res.contains("sample.txt"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn plan_mode_allows_reads_but_denies_mutation_execution_and_unknown_tools() {
        assert!(allowed_in_plan_mode("view_file"));
        assert!(allowed_in_plan_mode("grep"));
        assert!(allowed_in_plan_mode("search_web"));
        assert!(allowed_in_plan_mode("ask_question"));
        assert!(!allowed_in_plan_mode("write_to_file"));
        assert!(!allowed_in_plan_mode("run_command"));
        assert!(!allowed_in_plan_mode("spawn_agent"));
        assert!(!allowed_in_plan_mode("unknown_mcp_tool"));
    }

    #[test]
    fn tool_safety_is_conservative_and_parallelizes_only_reads() {
        assert_eq!(tool_safety("use_skill"), ToolSafety::ReadOnly);
        assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
        assert!(supports_parallel_execution("use_skill"));
        assert!(supports_parallel_execution("view_file"));
        assert_eq!(tool_safety("write_to_file"), ToolSafety::WorkspaceMutation);
        assert!(!supports_parallel_execution("write_to_file"));
        assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
        assert_eq!(tool_safety("unknown_mcp_tool"), ToolSafety::Unknown);
        assert!(!supports_parallel_execution("unknown_mcp_tool"));
    }

    #[test]
    fn skills_are_read_only_and_not_isolated_as_control_plane() {
        let calls = vec![
            ToolCall {
                name: "use_skill".to_string(),
                arguments: serde_json::json!({"name": "spotify"}),
                call_id: None,
            },
            ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({"command": "spotify-cli p volume 3"}),
                call_id: None,
            },
        ];

        let (isolated, deferred) = isolate_control_plane_call(calls);

        assert_eq!(isolated.len(), 2);
        assert_eq!(deferred, 0);
    }

    // Regression: session 1785595713111. Asked to plan a --json flag, the model
    // produced a plan containing "use grep to find the argument parsing" as a
    // future step and hedged over whether the project uses clap or structopt —
    // both answerable from a Cargo.toml it was allowed to read. It made zero
    // tool calls.
    // Regression: session 1785595170460 msg 4 read
    // "replace_file_content: error: error: target_content (old_string) is empty".
    // Every test session opened its edit with old_string: "" — the model's
    // instinct for "add a line at the top" — costing a turn before the error
    // taught it otherwise. The spec the model reads before calling now says it.
    // The prompt used to cap every response at "1 or 2 tool calls", so the model
    // never issued a batch and the parallel read path was never exercised — the
    // harness runs independent reads concurrently and only rations the calls
    // that change things.
    #[test]
    fn the_prompt_matches_what_the_executor_actually_does() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );

        assert!(
            prompt.contains("ISSUE INDEPENDENT READS TOGETHER"),
            "got: {prompt}"
        );
        assert!(prompt.contains("run in parallel"), "got: {prompt}");
        assert!(
            !prompt.contains("at most 1 or 2 tool calls"),
            "the old cap is gone"
        );

        // Every tool the prompt names as parallel must actually be one.
        for name in [
            "view_file",
            "grep",
            "glob",
            "list_directory",
            "find_symbol",
            "get_project_map",
            "search_web",
        ] {
            assert!(
                supports_parallel_execution(name),
                "{name} is not parallel-capable"
            );
        }
        // And the stated limit on changes must be the one the executor enforces.
        assert!(
            prompt.contains(&format!(
                "at most {MAX_MUTATING_CALLS_PER_RESPONSE} such calls"
            )),
            "got: {prompt}"
        );
    }

    // Regression: the JSON "Tool Format" section used to tell the model to
    // "Emit one tool call at a time" and claimed "the harness executes calls
    // sequentially" — flatly contradicting the Rules section's "ISSUE
    // INDEPENDENT READS TOGETHER" batching instruction a few paragraphs
    // earlier, and contradicting `parse_tool_calls_fenced`, which walks every
    // ```tool fence in a response specifically so a model can batch several
    // calls in one turn. This asserts the contradiction is gone and the
    // format section now agrees with the executor's real parallel-reads /
    // serialized-mutations behavior.
    #[test]
    fn json_protocol_tool_format_does_not_contradict_the_batching_rule() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );

        assert!(
            !prompt.contains("Emit one tool call at a time"),
            "got: {prompt}"
        );
        assert!(
            !prompt.contains("executes calls sequentially"),
            "got: {prompt}"
        );
        assert!(
            prompt.contains("You may emit several ```tool fences in one response"),
            "got: {prompt}"
        );
        assert!(
            prompt.contains("ISSUE INDEPENDENT READS TOGETHER"),
            "got: {prompt}"
        );
    }

    // Regression: a repeated `replace_file_content` call that reports
    // "already applied" (PR #306's idempotency guard) is a true no-op — not a
    // silently-different second edit — and neither a no-op nor a failed
    // mutation counts toward progress, so the turn's safety budget
    // (MAX_CONSECUTIVE_NO_PROGRESS / MAX_CONSECUTIVE_FAILED_MUTATIONS in
    // src/network.rs) ends the turn after a handful of repeats. The prompt
    // must tell the model this instead of leaving it to rediscover by
    // retrying.
    #[test]
    fn prompt_explains_repeated_edits_are_pointless_noops() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );

        assert!(prompt.contains("already applied"), "got: {prompt}");
        assert!(
            prompt.contains("harness ends the turn after a handful"),
            "got: {prompt}"
        );
    }

    // The native tag protocol's parser (`parse_tool_calls_tags`) also walks
    // every `[TOOL_CALLS]` occurrence in one response, so it must not carry
    // the same "one at a time" claim the JSON section used to have, and its
    // format text must actually differ from the JSON fence syntax it
    // describes.
    #[test]
    fn native_protocol_prompt_describes_native_syntax_and_not_json_fences() {
        let native = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Native,
            crate::config::AgentMode::Build,
        );
        let json = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );

        assert!(
            !native.contains("Emit one tool call at a time"),
            "got: {native}"
        );
        assert!(
            native.contains("[TOOL_CALLS]tool_name[ARGS]"),
            "got: {native}"
        );
        assert!(
            !native.contains("```tool"),
            "native prompt must not describe the JSON fence syntax"
        );
        assert!(
            !json.contains("[TOOL_CALLS]tool_name[ARGS]"),
            "json prompt must not describe the native tag syntax"
        );
    }

    // ApiNative carries tool schemas through the request's native `tools`
    // field, not the prompt text, so its section must say to use that
    // interface directly and must not print either text-based protocol's
    // call syntax.
    #[test]
    fn api_native_protocol_prompt_defers_to_the_request_schema() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::ApiNative,
            crate::config::AgentMode::Build,
        );

        assert!(
            prompt.contains("native function-calling interface"),
            "got: {prompt}"
        );
        assert!(!prompt.contains("```tool"), "got: {prompt}");
        assert!(!prompt.contains("[TOOL_CALLS]"), "got: {prompt}");
    }

    #[test]
    fn the_edit_tool_spec_explains_how_to_insert() {
        let spec = TOOLS
            .iter()
            .find(|tool| tool.name == "replace_file_content")
            .expect("tool exists");

        assert!(
            spec.description.contains("to INSERT text"),
            "got: {}",
            spec.description
        );
        assert!(
            spec.description.contains("prepend"),
            "got: {}",
            spec.description
        );
        assert!(
            spec.description.contains("An empty target is rejected"),
            "got: {}",
            spec.description
        );
        assert!(
            spec.arguments.contains("never empty"),
            "got: {}",
            spec.arguments
        );
    }

    #[test]
    fn the_view_file_spec_describes_the_hard_read_window() {
        let spec = TOOLS
            .iter()
            .find(|tool| tool.name == "view_file")
            .expect("tool exists");

        assert!(
            spec.description.contains("250-line hard cap"),
            "got: {}",
            spec.description
        );
        assert!(
            spec.description.contains("targeted follow-up ranges"),
            "got: {}",
            spec.description
        );
        assert!(
            spec.arguments.contains("250 lines"),
            "got: {}",
            spec.arguments
        );
        assert!(
            spec.arguments.contains("targeted follow-up"),
            "got: {}",
            spec.arguments
        );
        assert!(
            !spec.arguments.contains("start_line + 2000"),
            "got: {}",
            spec.arguments
        );

        let schema = schema_for_tool("view_file");
        assert_eq!(
            schema["properties"]["end_line"]["description"],
            "Inclusive end line; each call is capped at 250 lines. Request targeted follow-up ranges for more content."
        );
    }

    #[test]
    fn error_messages_are_prefixed_once() {
        assert_eq!(
            as_error_message("target_content is empty"),
            "error: target_content is empty"
        );
        // A handler that already framed its message keeps it as written.
        assert_eq!(
            as_error_message("error: target_content is empty"),
            "error: target_content is empty"
        );
        assert_eq!(
            as_error_message("  Error: file not found"),
            "Error: file not found"
        );
    }

    #[test]
    fn plan_mode_prompt_demands_investigation_not_a_plan_to_investigate() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Plan,
        );

        assert!(prompt.contains("PLAN MODE"));
        assert!(
            prompt.contains("Investigate before planning"),
            "got: {prompt}"
        );
        assert!(
            prompt.contains("specific to THIS repository"),
            "got: {prompt}"
        );
        assert!(prompt.contains("is not a plan"), "got: {prompt}");

        // Build mode must not carry the plan-mode restrictions.
        let build = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );
        assert!(!build.contains("PLAN MODE"));
    }

    #[test]
    fn truncate_keeps_leading_calls_and_reports_the_drop() {
        let call = |name: &str| ToolCall {
            name: name.to_string(),
            arguments: serde_json::json!({}),
            call_id: None,
        };

        // Under the limit: nothing is touched.
        let (kept, dropped) = truncate_tool_batch(vec![call("grep"), call("view_file")]);
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped, 0);

        // Reads fan out freely: ten searches are one thought, not ten.
        let reads: Vec<ToolCall> = (0..10).map(|_| call("grep")).collect();
        let (kept, dropped) = truncate_tool_batch(reads);
        assert_eq!(kept.len(), 10);
        assert_eq!(dropped, 0);

        // Calls that can change the workspace are rationed, and the prefix
        // before the surplus one survives in order.
        let over = vec![
            call("grep"),
            call("run_command"),
            call("write_to_file"),
            call("run_command"),
            call("write_to_file"),
            call("run_command"),
            call("grep"),
        ];
        let (kept, dropped) = truncate_tool_batch(over);
        assert_eq!(kept.len(), MAX_MUTATING_CALLS_PER_RESPONSE + 1);
        assert_eq!(dropped, 2);
        assert_eq!(kept[0].name, "grep");
        assert_eq!(kept[4].name, "write_to_file");

        // The absolute ceiling still applies to a runaway response.
        let runaway: Vec<ToolCall> = (0..50).map(|_| call("grep")).collect();
        let (kept, dropped) = truncate_tool_batch(runaway);
        assert_eq!(kept.len(), MAX_TOOL_CALLS_PER_RESPONSE);
        assert_eq!(dropped, 50 - MAX_TOOL_CALLS_PER_RESPONSE);
    }

    #[test]
    fn truncate_allows_multiple_skills_and_parallel_reads() {
        let call = |name: &str| ToolCall {
            name: name.to_string(),
            arguments: serde_json::json!({}),
            call_id: None,
        };

        // Multiple use_skill calls and read tools run together in parallel.
        let (kept, dropped) = truncate_tool_batch(vec![
            call("use_skill"),
            call("use_skill"),
            call("grep"),
            call("run_command"),
        ]);
        assert_eq!(kept.len(), 4);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn validation_rejects_unknown_duplicate_and_mixed_calls() {
        let valid = ToolCall {
            name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": "TODO"}),
            call_id: None,
        };
        assert!(validate_tool_calls(std::slice::from_ref(&valid)).is_ok());
        assert!(
            validate_tool_calls(&[ToolCall {
                name: "replace_file_content".to_string(),
                arguments: serde_json::json!({
                    "path": "src/store.ts",
                    "edits": [{"old_string": "old", "new_string": "new"}]
                }),
                call_id: None,
            }])
            .is_ok()
        );
        assert!(validate_tool_calls(&[valid.clone(), valid]).is_err());
        assert!(
            validate_tool_calls(&[ToolCall {
                name: "not_registered".to_string(),
                arguments: serde_json::json!({}),
                call_id: None,
            }])
            .is_err()
        );
        let calls = (0..=MAX_TOOL_CALLS_PER_RESPONSE)
            .map(|_| ToolCall {
                name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "TODO"}),
                call_id: None,
            })
            .collect::<Vec<_>>();
        assert!(validate_tool_calls(&calls).is_err());
        assert!(
            validate_tool_calls(&[ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({}),
                call_id: None,
            }])
            .is_err()
        );
        assert!(
            validate_tool_calls(&[
                ToolCall {
                    name: "use_skill".to_string(),
                    arguments: serde_json::json!({"name": "x"}),
                    call_id: None,
                },
                ToolCall {
                    name: "grep".to_string(),
                    arguments: serde_json::json!({"pattern": "x"}),
                    call_id: None,
                },
            ])
            .is_ok()
        );
    }

    #[test]
    fn authorization_is_centralized_and_conservative() {
        assert!(matches!(
            authorize_tool("write_to_file", crate::config::AgentMode::Plan, true, false),
            AuthorizationDecision::Deny(_)
        ));
        assert_eq!(
            authorize_tool(
                "write_to_file",
                crate::config::AgentMode::Build,
                false,
                false
            ),
            AuthorizationDecision::RequireConfirmation
        );
        assert_eq!(
            authorize_tool("mcp_unknown", crate::config::AgentMode::Build, false, false),
            AuthorizationDecision::RequireConfirmation
        );
        assert_eq!(
            authorize_tool("grep", crate::config::AgentMode::Build, false, false),
            AuthorizationDecision::Allow
        );
    }

    #[test]
    fn command_authorization_distinguishes_git_inspection_from_recovery() {
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": "git status --short"}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": "git restore -- src/GameScene.ts"}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::RequireConfirmation
        );
    }

    #[test]
    fn command_authorization_distinguishes_safe_and_unknown_shell_commands() {
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": "gh issue list --repo lhagfoss/rustcode"}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            authorize_tool_with_args(
                "run_command",
                &serde_json::json!({"command": "gh issue close 1 --repo lhagfoss/rustcode"}),
                crate::config::AgentMode::Build,
                false,
                false,
            ),
            AuthorizationDecision::RequireConfirmation
        );
    }

    #[test]
    fn prompt_makes_delegation_explicitly_opt_in() {
        let prompt = tool_system_prompt(
            false,
            crate::config::ToolProtocol::Json,
            crate::config::AgentMode::Build,
        );
        assert!(prompt.contains("Do not spawn subagents unless the user explicitly requests"));
        assert!(prompt.contains("Review every subagent result"));
        assert!(prompt.contains("Do NOT run `cargo check` on a standalone `.rs` file"));
        assert!(prompt.contains("Prefer the smallest effective tool sequence"));
        assert!(prompt.contains("git-feature-workflow"));
        assert!(
            prompt.contains("Chaining shell commands is different from speculative tool batching")
        );
        assert!(prompt.contains("never claim a check, test, formatter, or lint command passed"));
        assert!(prompt.contains("Stage only explicit feature paths in Git"));
    }

    #[test]
    fn memory_tools_lifecycle_execution() {
        assert!(TOOLS.iter().any(|t| t.name == "remember"));
        assert!(TOOLS.iter().any(|t| t.name == "recall_memory"));
        assert!(TOOLS.iter().any(|t| t.name == "forget_memory"));

        let res = (super::misc::REMEMBER.handler)(&serde_json::json!({
            "key": "test_db_port",
            "value": "5433",
            "category": "database",
            "scope": "global"
        })).unwrap();
        assert!(res.contains("Remembered globally"));

        let recall_res = (super::misc::RECALL_MEMORY.handler)(&serde_json::json!({
            "query": "test_db_port",
            "scope": "global"
        })).unwrap();
        assert!(recall_res.contains("5433"));

        let forget_res = (super::misc::FORGET_MEMORY.handler)(&serde_json::json!({
            "key": "test_db_port",
            "scope": "global"
        })).unwrap();
        assert!(forget_res.contains("Removed"));
    }
}
