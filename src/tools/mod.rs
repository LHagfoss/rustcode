use regex::Regex;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, OnceLock};
use std::time::Instant;

mod exec;
mod filesystem;
mod misc;
mod search;

/// A parsed tool request emitted by a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Validate parsed calls before they reach an executor. Text protocols are
/// intentionally permissive while parsing, but execution must be strict and
/// fail closed when the model emits an unknown tool or malformed arguments.
pub fn validate_tool_calls(calls: &[ToolCall]) -> Result<(), String> {
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
            return Err(format!("invalid arguments for '{}': {reason}", call.name));
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

fn validate_value_against_schema(
    value: &Value,
    schema: &Value,
    path: &str,
) -> Result<(), String> {
    let expected = schema.get("type").and_then(Value::as_str).unwrap_or("object");
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
        if schema
            .get("additionalProperties")
            .and_then(Value::as_bool)
            == Some(false)
        {
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                if let Some(unknown) = object.keys().find(|key| !properties.contains_key(*key)) {
                    return Err(format!("{path}.{unknown} is not an advertised argument"));
                }
            }
        }
        if let Some(ap_schema) = schema.get("additionalProperties").filter(|v| v.is_object()) {
            if let Some(obj) = value.as_object() {
                for (key, val) in obj {
                    validate_value_against_schema(val, ap_schema, &format!("{path}.{key}"))?;
                }
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

pub(crate) static WAKEUP_CALLBACK: OnceLock<
    Box<dyn Fn(String, String, String) + Send + Sync + 'static>,
> = OnceLock::new();

pub fn register_wakeup_callback<F>(cb: F)
where
    F: Fn(String, String, String) + Send + Sync + 'static,
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

    if raw_path.starts_with("~/") || raw_path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            let tail = raw_path.strip_prefix('~').unwrap_or("");
            let tail = tail.strip_prefix('/').unwrap_or(tail);
            return PathBuf::from(home).join(tail);
        }
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

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,

    pub arguments: &'static str,
    pub handler: fn(&Value) -> Result<String, String>,
    /// If true, the agent loop will pause and show a Y/N confirmation modal
    /// to the user before executing. Use for destructive tools (write, create, run).
    pub requires_confirmation: bool,
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
    if mode == crate::config::AgentMode::Plan && !allowed_in_plan_mode(name) {
        return AuthorizationDecision::Deny(
            "Plan mode blocks workspace mutation, command execution, delegation, and unknown tools"
                .to_string(),
        );
    }
    if !bypass_confirmation
        && !auto_confirm
        && (needs_confirmation(name) || matches!(tool_safety(name), ToolSafety::Unknown))
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
    match name {
        "view_file" | "list_directory" | "grep" | "glob" | "find_symbol" | "get_project_map" => &[ReadWorkspace],
        "get_time" => &[],
        "search_web" => &[Network],
        "ask_question" => &[UserInteraction],
        "use_skill" | "todo_write" => &[SessionState],
        "replace_file_content" | "multi_replace_file_content" | "write_to_file"
        | "delete_file" | "move_file" | "copy_file" => &[WriteWorkspace],
        "run_command" | "manage_task" => &[ExecuteCommands],
        "spawn_agent" | "send_agent" | "set_goal" => &[AgentDelegation, SessionState],
        "complete_task" => &[SessionState],
        _ => &[],
    }
}

/// Plan mode is intentionally deny-by-default for tools not explicitly known
/// to be read-only or user-facing.
pub fn allowed_in_plan_mode(name: &str) -> bool {
    use ToolCapability::*;
    let capabilities = tool_capabilities(name);
    capabilities.iter().all(|cap| {
        matches!(cap, ReadWorkspace | Network | UserInteraction | SessionState)
    }) && (capabilities.contains(&ReadWorkspace)
        || capabilities.contains(&Network)
        || capabilities.contains(&UserInteraction)
        || name == "get_time"
        || name == "use_skill"
        || name == "todo_write")
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "ask_question",
        description: "Ask the user a multiple-choice question to clarify underspecified requirements, solicit design choices, or select an option. Only call this when explicit user validation or decision-making is needed. Do not use for trivial yes/no or routine commands.",
        arguments: r#"{"question": "The question title or description to ask", "options": ["Option 1 text", "Option 2 text", "Option 3 text"], "is_multi_select": false}"#,
        handler: misc::ask_question,
        requires_confirmation: false,
    },
    Tool {
        name: "get_time",
        description: "Get the current local date and time",
        arguments: r#"{} (no arguments)"#,
        handler: misc::get_time,
        requires_confirmation: false,
    },
    Tool {
        name: "grep",
        description: "Recursively search file contents with regex. Respects                       .gitignore and skips hidden files. Use this to find where                       functions, classes, strings, or patterns are defined or used",
        arguments: r#"{"pattern": "regex pattern", "path": "optional directory or file (default current dir)", "include": "optional file glob filter e.g. '*.rs'", "ignore_case": optional bool (default false)}"#,
        handler: search::grep,
        requires_confirmation: false,
    },
    Tool {
        name: "glob",
        description: "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ts').                       Respects .gitignore and skips hidden files. Returns matching                       paths, sorted. Use this to discover files by name",
        arguments: r#"{"pattern": "glob pattern", "path": "optional root directory (default current dir)"}"#,
        handler: search::glob,
        requires_confirmation: false,
    },
    Tool {
        name: "list_directory",
        description: "List files in a directory",
        arguments: r#"{"path": "directory path, defaults to current dir"}"#,
        handler: search::list_directory,
        requires_confirmation: false,
    },
    Tool {
        name: "delete_file",
        description: "Delete a file from the filesystem",
        arguments: r#"{"path": "file to delete"}"#,
        handler: filesystem::delete_file,
        requires_confirmation: true,
    },
    Tool {
        name: "move_file",
        description: "Move or rename a file or directory to a new path",
        arguments: r#"{"src": "source path", "dest": "destination path"}"#,
        handler: filesystem::move_file,
        requires_confirmation: true,
    },
    Tool {
        name: "copy_file",
        description: "Copy a file to a new path",
        arguments: r#"{"src": "source path to copy", "dest": "destination path"}"#,
        handler: filesystem::copy_file,
        requires_confirmation: true,
    },
    Tool {
        name: "run_command",
        description: "Run one command through the platform shell and return stdout/stderr and the exit code. The command may use normal shell syntax, including ';' or '&&' to chain commands, pipes, redirects, and environment assignments. Supports an optional working directory, environment overrides, timeout (default 120s), and background execution ('background': true). Note: Interactive 'sudo' requiring passwords is disabled; use non-privileged commands or 'sudo -n'.",
        arguments: r#"{"command": "full shell command string", "cwd": "optional working directory", "timeout_ms": "optional timeout in ms", "background": "optional bool to run asynchronously in background (default false)"}"#,
        handler: exec::run_command,
        requires_confirmation: true,
    },
    Tool {
        name: "manage_task",
        description: "Manage background tasks spawned with run_command (action: 'list', 'status', or 'kill').",
        arguments: r#"{"action": "list, status, or kill", "task_id": "required for status/kill"}"#,
        handler: exec::manage_task_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "search_web",
        description: "Performs a web search to look up documentation, API details, or code patterns.",
        arguments: r#"{"query": "search query terms", "domain": "optional domain filter e.g. 'docs.rs'"}"#,
        handler: misc::search_web,
        requires_confirmation: false,
    },
    Tool {
        name: "find_symbol",
        description: "Queries the codebase symbol index for matching structures, functions, enums, impls, traits, or modules. Returns definition location and signature.",
        arguments: r#"{"query": "search query string (fuzzy matching on symbol name)"}"#,
        handler: search::find_symbol_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "get_project_map",
        description: "Generates a compressed map of all symbols and API signatures in the codebase to understand project structure.",
        arguments: r#"{}"#,
        handler: search::get_project_map_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "view_file",
        description: "View the contents of a file or directory. Supports line ranges (1-indexed) and optional byte offset if content is truncated.",
        arguments: r#"{"path": "absolute or relative path to file or directory", "start_line": "optional start line number, 1-indexed (default 1)", "end_line": "optional end line number, 1-indexed (default start_line + 2000)", "content_offset": "optional byte offset into content"}"#,
        handler: filesystem::view_file_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "replace_file_content",
        description: "Surgically edit code in an existing file. Supports single replacement (target_content/replacement_content or old_string/new_string) or array of batch edits (edits: [{old_string, new_string}]). Line numbers are optional.",
        arguments: r#"{"path": "absolute or relative path to file", "target_content": "precise block of code to edit (or old_string)", "replacement_content": "complete replacement text (or new_string)", "edits": "optional array of [{old_string, new_string}] for multiple edits in 1 call"}"#,
        handler: filesystem::replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "multi_replace_file_content",
        description: "Apply multiple non-contiguous edits across a single file in a single tool call.                       Specify each edit as a separate replacement chunk.",
        arguments: r#"{"path": "absolute or relative path to file", "replacements": "array of objects, each containing: {start_line, end_line, target_content, replacement_content}"}"#,
        handler: filesystem::multi_replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "write_to_file",
        description: "Create a new file or overwrite an existing file with complete content.                       Creates parent directories automatically.",
        arguments: r#"{"path": "absolute or relative path to file", "content": "entire contents to write", "overwrite": "set true to allow overwriting an existing file (default false)"}"#,
        handler: filesystem::write_to_file_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "complete_task",
        description: "Mark the continuous goal/task as successfully complete.",
        arguments: r#"{"result": "summary of what was achieved and final results"}"#,
        handler: misc::complete_task_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "use_skill",
        description: "Load a skill by name to get its instructions and available files.",
        arguments: r#"{"name": "skill name"}"#,
        handler: misc::use_skill,
        requires_confirmation: false,
    },
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
    ControlPlane,
    ReadOnly,
    WorkspaceMutation,
    ProcessControl,
    Interactive,
    Delegation,
    Unknown,
}

pub fn tool_safety(name: &str) -> ToolSafety {
    match name {
        "use_skill" => ToolSafety::ControlPlane,
        "view_file" | "list_directory" | "grep" | "glob" | "get_time" | "find_symbol"
        | "get_project_map" | "search_web" => ToolSafety::ReadOnly,
        "replace_file_content" | "multi_replace_file_content" | "write_to_file"
        | "delete_file" | "move_file" | "copy_file" => ToolSafety::WorkspaceMutation,
        "run_command" | "background_output" | "write_stdin" => ToolSafety::ProcessControl,
        "ask_question" => ToolSafety::Interactive,
        "spawn_agent" | "send_agent" | "set_goal" | "todo_write" => ToolSafety::Delegation,
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
fn collect_mcp_tools() -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        for client in reg.values() {
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
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
                    out.push((name.to_string(), desc, schema));
                }
            }
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

/// Canonical JSON Schemas for built-in tools.
///
/// The text protocol still uses `Tool::arguments` as compact documentation, but
/// native providers must receive real types, required fields, and nested item
/// schemas. Keeping this in one place prevents the API-native contract from
/// silently drifting away from the handlers.
fn schema_for_tool(name: &str) -> Value {
    match name {
        "ask_question" => serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "Question to ask the user" },
                "options": { "type": "array", "items": { "type": "string" }, "description": "Choices shown to the user" },
                "is_multi_select": { "type": "boolean", "default": false }
            },
            "required": ["question", "options"]
        }),
        "get_time" | "get_project_map" => serde_json::json!({
            "type": "object", "properties": {}, "additionalProperties": false
        }),
        "grep" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" }, "path": { "type": "string" },
                "include": { "type": "string" }, "ignore_case": { "type": "boolean", "default": false }
            }, "required": ["pattern"]
        }),
        "glob" => serde_json::json!({
            "type": "object", "properties": {
                "pattern": { "type": "string" }, "path": { "type": "string" }
            }, "required": ["pattern"]
        }),
        "list_directory" => serde_json::json!({
            "type": "object", "properties": { "path": { "type": "string" } }
        }),
        "delete_file" => serde_json::json!({
            "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"]
        }),
        "move_file" | "copy_file" => serde_json::json!({
            "type": "object", "properties": {
                "src": { "type": "string" }, "dest": { "type": "string" }
            }, "required": ["src", "dest"]
        }),
        "run_command" => serde_json::json!({
            "type": "object", "properties": {
                "command": { "type": "string" }, "cwd": { "type": "string" },
                "timeout_ms": { "type": "integer", "minimum": 1 },
                "background": { "type": "boolean", "default": false },
                "env": { "type": "object", "additionalProperties": { "type": "string" } }
            }, "required": ["command"]
        }),
        "manage_task" => serde_json::json!({
            "type": "object", "properties": {
                "action": { "type": "string", "enum": ["list", "status", "kill"] },
                "task_id": { "type": "string" }
            }, "required": ["action"]
        }),
        "search_web" => serde_json::json!({
            "type": "object", "properties": {
                "query": { "type": "string" }, "domain": { "type": "string" }
            }, "required": ["query"]
        }),
        "find_symbol" => serde_json::json!({
            "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"]
        }),
        "view_file" => serde_json::json!({
            "type": "object", "properties": {
                "path": { "type": "string" }, "start_line": { "type": "integer", "minimum": 1 },
                "end_line": { "type": "integer", "minimum": 1 }, "content_offset": { "type": "integer", "minimum": 0 }
            }, "required": ["path"]
        }),
        "replace_file_content" => serde_json::json!({
            "type": "object", "properties": {
                "path": { "type": "string" }, "target_content": { "type": "string" },
                "replacement_content": { "type": "string" },
                "edits": { "type": "array", "items": { "type": "object", "properties": {
                    "old_string": { "type": "string" }, "new_string": { "type": "string" },
                    "start_line": { "type": "integer" }, "end_line": { "type": "integer" }
                }, "required": ["old_string", "new_string"] } }
            }, "required": ["path"]
        }),
        "multi_replace_file_content" => serde_json::json!({
            "type": "object", "properties": {
                "path": { "type": "string" }, "replacements": { "type": "array", "items": { "type": "object", "properties": {
                    "start_line": { "type": "integer" }, "end_line": { "type": "integer" },
                    "target_content": { "type": "string" }, "replacement_content": { "type": "string" }
                }, "required": ["start_line", "end_line", "target_content", "replacement_content"] } }
            }, "required": ["path", "replacements"]
        }),
        "write_to_file" => serde_json::json!({
            "type": "object", "properties": {
                "path": { "type": "string" }, "content": { "type": "string" },
                "overwrite": { "type": "boolean", "default": false }
            }, "required": ["path", "content"]
        }),
        "complete_task" => serde_json::json!({
            "type": "object", "properties": { "result": { "type": "string" } }, "required": ["result"]
        }),
        "use_skill" => serde_json::json!({
            "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"]
        }),
        _ => schema_from_arguments("{}"),
    }
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
            "Use the 'use_skill' tool to load a skill when a task matches its description.\n\n",
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
- Choose verification from the project structure: first locate the nearest `Cargo.toml`, `package.json`, `pyproject.toml`, or equivalent manifest. Run project checks from that project root. Do NOT run `cargo check` on a standalone `.rs` file outside a Cargo project; use an appropriate standalone checker such as `rustc` only when practical, or clearly report that project verification is not applicable.\n\
- Tool results are authoritative evidence. If a tool or compiler check reports an error, fix it before giving a final answer. Never replace a concrete tool result with a claim that tools were unavailable.\n\
- A subagent's report is advisory, not proof that work is complete or blocked. If a subagent says it could not use tools, continue the task yourself and inspect the workspace directly.\n\
- Explore first: use `grep` or `glob` to locate exact function definitions before reading. DO NOT page through large files from line 1 to end with sequential `view_file` calls — use `grep` first to find line numbers, then `view_file` only the target section.\n\
- Editing an existing file: use `replace_file_content` (pass an `edits` array to batch several changes in one call). Use `write_to_file` only to create a new file or fully rewrite one. `multi_replace_file_content` is a niche variant that needs exact line numbers and exact text — prefer `replace_file_content`, whose matching is more forgiving. Before modifying an existing file, you MUST inspect its actual content using `view_file` or `grep`. Never guess or hallucinate line numbers, imports, dependencies, or struct fields for files you have not inspected in this session.\n\
- EXECUTE TOOL CALLS SEQUENTIALLY: Emit at most 1 or 2 tool calls at a time. Never output speculative multi-step batches of 5+ tool calls (such as predicting edits, builds, git commits, and PR creations all in a single turn). Execute tools step-by-step and inspect results.\n\
- Chaining shell commands is different from speculative tool batching: it is encouraged for small, related, inspectable command sequences, especially status/log/diff checks and the verified publish sequence. Inspect output before deciding the next mutation.\n\
- DO NOT use `run_command` with `cat`, `sed`, `head`, `tail`, or `less`/`more` to read/search files. Always use the native `view_file` or `grep` tools.\n\
- Match project code style.\n\
- Before adding new code, study how the nearest EXISTING code does the same thing (sibling functions, other match arms, similar handlers) and mirror its patterns — function signatures, how shared state/locks are passed, error handling. Do NOT invent a new pattern when neighbors establish one; diverging from local conventions is a common source of subtle bugs (deadlocks, double-locks, lifetime issues) that compile fine but break at runtime.\n\
- Prefer the smallest effective tool sequence: locate first, inspect only the relevant range, make one focused change, then verify from the correct project root. Do not repeat successful reads or run broad checks unrelated to the files changed.\n\
- Run focused tests or checks after code changes unless the user says not to; ask before expensive or externally visible operations.\n\
- Read-only tools run immediately; modifying/destructive tools require confirmation.\n\
- Use `ask_question` ONLY when you require clarification on ambiguous user requirements, design choices, or need explicit user validation before proceeding. Do NOT invoke `ask_question` for routine tool calls or trivial confirmations.\n\
- When the task is complete, output a plain-text final summary (with no tool block).\n\n\
# Working memory & avoiding loops
- If a tool execution or compiler check returns compilation errors or warnings, prioritize fixing them immediately before proceeding to other steps.
- File contents you have already read this session are STILL VISIBLE in the conversation. Do NOT re-read a file you already have unless it changed on disk.
- Do not repeat a tool call you just made with the same arguments. If a tool call returns an error, correct your arguments or approach instead of repeating the identical call. If a read or search came up empty, change your query or your approach rather than retrying.
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
                - Emit one tool call at a time. The harness executes calls sequentially so later calls can depend on earlier results.\n\n"
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
    let name = json.get("name")?.as_str()?.to_string();
    let args = if let Some(args_val) = json.get("arguments") {
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
    Some((name, args))
}

fn repair_json(s: &str) -> String {
    let mut repaired = s.to_string();
    repaired = repaired.trim_end().to_string();
    if repaired.ends_with(',') {
        repaired.pop();
    }

    let mut in_string = false;
    let mut escaped = false;
    let mut stack = Vec::new();

    for c in repaired.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && in_string {
            escaped = true;
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            continue;
        }
        if !in_string {
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
                    calls.push(ToolCall { name, arguments: json_val });
                } else {
                    let pattern = &*BRACE_OBJ_RE;
                    if let Some(mat) = pattern.find(raw_args)
                        && let Ok(json_val) = serde_json::from_str::<Value>(mat.as_str())
                    {
                        calls.push(ToolCall { name, arguments: json_val });
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
                calls.push(ToolCall { name: call.0, arguments: call.1 });
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
    // Look at every ```tool fence; report the first that fails to parse.
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
            if let Err(e) = serde_json::from_str::<Value>(&repaired) {
                let snippet: String = block.trim().chars().take(240).collect();
                return Some(format!(
                    "JSON parse error: {e}. A common cause is a backslash inside a string: every literal `\\` in the file must be written as `\\\\` in the JSON, and a real newline must be `\\n`. Offending block:\n```\n{snippet}\n```"
                ));
            }
        }
        if next.is_empty() {
            break;
        }
        search = next;
    }
    None
}

fn parse_tool_calls_impl(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Vec<ToolCall> {
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
            calls.push(ToolCall { name: call.0, arguments: call.1 });
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
                calls.push(ToolCall { name: call.0, arguments: call.1 });
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
        || trimmed.contains("[TOOL_CALLS]")
        || (trimmed.starts_with('{')
            && (trimmed.contains("\"name\"") || trimmed.contains("\"tool\"")))
}

pub fn parse_tool_call(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Option<ToolCall> {
    parse_tool_calls(text, protocol).into_iter().next()
}

pub fn execute(name: &str, args: &Value) -> String {
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        for client in reg.values() {
            if let Ok(tools) = client.get_tools()
                && tools
                    .iter()
                    .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
            {
                let handle = tokio::runtime::Handle::current();
                let client_clone = Arc::clone(client);
                let name_owned = name.to_string();
                let args_clone = args.clone();

                let res = handle.block_on(async move {
                    client_clone
                        .call(
                            "tools/call",
                            serde_json::json!({
                                "name": name_owned,
                                "arguments": args_clone
                            }),
                        )
                        .await
                });

                return match res {
                    Ok(val) => {
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
                            text_parts.join("\n")
                        } else {
                            serde_json::to_string_pretty(&val).unwrap_or_default()
                        }
                    }
                    Err(e) => format!("error: MCP tool call failed: {e}"),
                };
            }
        }
    }

    match TOOLS.iter().find(|t| t.name == name) {
        Some(tool) => match (tool.handler)(args) {
            Ok(out) => out,
            Err(e) => format!("error: {e}"),
        },
        None => format!(
            "error: unknown tool '{name}'. Available: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ),
    }
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
        assert!(enabled.iter().any(|t| t["function"]["name"] == "spawn_agent"));
        assert!(enabled.iter().any(|t| t["function"]["name"] == "send_agent"));
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
        assert_eq!(calls[0].arguments.get("path").unwrap().as_str().unwrap(), "/foo");
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
    fn test_parse_tool_calls_tag() {
        let text1 = "Let me check...[TOOL_CALLS]glob[ARGS]{\"pattern\": \"**/*.rs\"}";
        let calls1 = parse_tool_calls(text1, crate::config::ToolProtocol::Json);
        assert_eq!(calls1.len(), 1);
        assert_eq!(calls1[0].name, "glob");
        assert_eq!(
            calls1[0].arguments.get("pattern").unwrap().as_str().unwrap(),
            "**/*.rs"
        );

        let text2 = "Let me check...[TOOL_CALLS]glob\":{\"pattern\":\"**/*.rs\"}";
        let calls2 = parse_tool_calls(text2, crate::config::ToolProtocol::Json);
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].name, "glob");
        assert_eq!(
            calls2[0].arguments.get("pattern").unwrap().as_str().unwrap(),
            "**/*.rs"
        );

        let text3 = "Plan:[TOOL_CALLS]todo_write[ARGS]{\"todos\": [{\"content\": \"Fix bug\"}]}";
        let calls3 = parse_tool_calls(text3, crate::config::ToolProtocol::Json);
        assert_eq!(calls3.len(), 1);
        assert_eq!(calls3[0].name, "todo_write");
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
        assert_eq!(calls[0].arguments.get("pattern").unwrap().as_str().unwrap(), "foo");
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
        assert!(res.contains("@@ -1,1 +1,1 @@"));
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
        assert_eq!(tool_safety("use_skill"), ToolSafety::ControlPlane);
        assert_eq!(tool_safety("grep"), ToolSafety::ReadOnly);
        assert!(supports_parallel_execution("view_file"));
        assert_eq!(tool_safety("write_to_file"), ToolSafety::WorkspaceMutation);
        assert!(!supports_parallel_execution("write_to_file"));
        assert_eq!(tool_safety("run_command"), ToolSafety::ProcessControl);
        assert_eq!(tool_safety("unknown_mcp_tool"), ToolSafety::Unknown);
        assert!(!supports_parallel_execution("unknown_mcp_tool"));
    }

    #[test]
    fn control_plane_calls_are_isolated_from_side_effects() {
        let calls = vec![
            ToolCall {
                name: "use_skill".to_string(),
                arguments: serde_json::json!({"name": "spotify"}),
            },
            ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({"command": "spotify-cli p volume 3"}),
            },
        ];

        let (isolated, deferred) = isolate_control_plane_call(calls);

        assert_eq!(isolated.len(), 1);
        assert_eq!(isolated[0].name, "use_skill");
        assert_eq!(deferred, 1);
    }

    #[test]
    fn validation_rejects_unknown_duplicate_and_mixed_calls() {
        let valid = ToolCall {
            name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": "TODO"}),
        };
        assert!(validate_tool_calls(std::slice::from_ref(&valid)).is_ok());
        assert!(validate_tool_calls(&[valid.clone(), valid]).is_err());
        assert!(validate_tool_calls(&[ToolCall {
            name: "not_registered".to_string(),
            arguments: serde_json::json!({}),
        }])
        .is_err());
        assert!(validate_tool_calls(&[ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({}),
        }])
        .is_err());
        assert!(validate_tool_calls(&[
            ToolCall {
                name: "use_skill".to_string(),
                arguments: serde_json::json!({"name": "x"}),
            },
            ToolCall {
                name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "x"}),
            },
        ])
        .is_err());
    }

    #[test]
    fn authorization_is_centralized_and_conservative() {
        assert!(matches!(
            authorize_tool("write_to_file", crate::config::AgentMode::Plan, true, false),
            AuthorizationDecision::Deny(_)
        ));
        assert_eq!(
            authorize_tool("write_to_file", crate::config::AgentMode::Build, false, false),
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
        assert!(prompt.contains("Chaining shell commands is different from speculative tool batching"));
    }
}
