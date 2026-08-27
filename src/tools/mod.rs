use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Instant;

mod audio;
mod dispatch;
mod envelope;
mod exec;
mod filesystem;
mod misc;
mod parser;
mod schema;
mod search;
mod video;

#[allow(unused_imports)]
pub use dispatch::{execute, needs_confirmation};
pub use parser::{
    diagnose_failed_tool_call, is_code_editing_tool, is_tool_call_start, parse_tool_call,
    parse_tool_calls,
};
pub use schema::{native_tools_schema, tool_system_prompt};

#[allow(unused_imports)]
pub(crate) use dispatch::{
    execute_video_with_progress, execute_with_metadata, execute_with_metadata_cancellable,
};
pub(crate) use parser::find_closing_tool_fence;
pub(crate) use schema::{
    MAX_MCP_NATIVE_SCHEMAS, McpSchemaSelectionStats, ToolSchemaPolicy,
    native_tools_schema_for_context, native_tools_schema_for_context_with_sticky,
    tool_system_prompt_for_policy,
};

use schema::{AGENT_TOOL_SPECS, collect_mcp_tools, schema_for_agent_tool, schema_for_tool};

#[cfg(test)]
use dispatch::as_error_message;
#[cfg(test)]
use parser::repair_json;
#[cfg(test)]
use schema::{
    MCP_DISCOVERY_FALLBACK_COUNT, mcp_canonical_name, provider_compatible_schema,
    schema_from_arguments, select_mcp_tools_for_context, select_mcp_tools_for_context_with_sticky,
};

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use envelope::{ToolCallEnvelope, ToolErrorKind, ToolResultEnvelope};

pub(crate) use exec::{
    CommandProgressCallback, command_confirmation_preview, command_requires_confirmation,
    run_command_output_with_progress,
};

pub(crate) use filesystem::edit_target_and_replacement;
pub(crate) use misc::search_web_async;
pub(crate) use video::render_confirmation_preview;

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
    pub(crate) pending: bool,
    pub(crate) command: Option<String>,
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
            pending: false,
            command: None,
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
            pending: false,
            command: None,
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

        // Only built-in handlers coerce string-encoded integers
        // (parse_json_number); MCP servers receive arguments verbatim.
        let string_integers = TOOLS.iter().any(|tool| tool.name == call.name);
        if let Err(reason) =
            validate_value_against_schema(&call.arguments, &schema, "$", string_integers)
        {
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

fn validate_value_against_schema(
    value: &Value,
    schema: &Value,
    path: &str,
    string_integers: bool,
) -> Result<(), String> {
    let expected = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");
    let type_matches = match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        // Built-in handlers read line numbers through parse_json_number, which
        // also accepts string-encoded integers from lenient providers. MCP
        // tools receive arguments verbatim with no such coercion, so the
        // leniency is scoped to built-ins only.
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || (string_integers && value.as_str().is_some_and(|s| s.parse::<u64>().is_ok()))
        }
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
                validate_value_against_schema(
                    val,
                    ap_schema,
                    &format!("{path}.{key}"),
                    string_integers,
                )?;
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in properties {
                if let Some(actual) = object.get(key) {
                    validate_value_against_schema(
                        actual,
                        child,
                        &format!("{path}.{key}"),
                        string_integers,
                    )?;
                }
            }
        }
    }
    if let Some(items) = schema.get("items")
        && let Some(array) = value.as_array()
    {
        for (index, item) in array.iter().enumerate() {
            validate_value_against_schema(
                item,
                items,
                &format!("{path}[{index}]"),
                string_integers,
            )?;
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
    misc::WAIT_AGENT,
    misc::CANCEL_AGENT,
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
    misc::LIST_SKILLS,
    misc::USE_SKILL,
    misc::REMEMBER,
    misc::RECALL_MEMORY,
    misc::FORGET_MEMORY,
    audio::GENERATE_SOUND_EFFECT,
    audio::GENERATE_MUSIC,
    audio::INSPECT_AUDIO,
    video::INSPECT_MEDIA,
    video::VALIDATE_VIDEO_PROJECT,
    video::RENDER_VIDEO,
];

pub fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent" | "send_agent" | "wait_agent" | "cancel_agent" | "set_goal" | "todo_write"
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
