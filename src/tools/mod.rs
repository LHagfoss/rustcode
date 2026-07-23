use regex::Regex;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, OnceLock};
use std::time::Instant;

mod filesystem;
mod search;
mod exec;
mod misc;


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

pub(crate) static WAKEUP_CALLBACK: OnceLock<Box<dyn Fn(String, String, String) + Send + Sync + 'static>> =
    OnceLock::new();

pub fn register_wakeup_callback<F>(cb: F)
where
    F: Fn(String, String, String) + Send + Sync + 'static,
{
    let _ = WAKEUP_CALLBACK.set(Box::new(cb));
}

thread_local! {
    static ACTIVE_SESSION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn set_active_session_id(id: Option<String>) {
    ACTIVE_SESSION_ID.with(|f| {
        *f.borrow_mut() = id;
    });
}

pub fn get_active_session_id() -> Option<String> {
    ACTIVE_SESSION_ID.with(|f| f.borrow().clone())
}

pub(crate) fn resolve_tool_path(raw_path: &str) -> PathBuf {
    let p = Path::new(raw_path);

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
            && let Some(sandbox_dir) = crate::config::get_active_session_sandbox_dir(&session_id) {
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
            && let Some(artifacts_dir) =
                crate::config::get_active_session_artifacts_dir(&session_id)
            {
                let mut resolved = artifacts_dir;
                for part in parts_artifacts {
                    resolved.push(part);
                }
                return resolved;
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

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "check_match",
        description: "Check football match data from api-sports.io.                      Useful for checking live scores during matches or finding specific games.                     Example: check_match with team='Norway' and date='2026-07-11'",
        arguments: r#"{"date": "YYYY-MM-DD format required", "team": "optional team name filter", "status": "optional status (LIVE, FT, NS)"}"#,
        handler: misc::check_match,
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
        description: "Run a shell command and return its stdout/stderr and exit code.                       Supports an optional working directory, environment overrides, timeout (default 120s),                       and background execution ('background': true to run asynchronously in background). Use for builds, tests, dev servers, etc.",
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
        description: "View the contents of a file. Supports line ranges (1-indexed) and optional byte offset if content is truncated.",
        arguments: r#"{"path": "absolute or relative path to file", "start_line": "optional start line number, 1-indexed (default 1)", "end_line": "optional end line number, 1-indexed (default start_line + 500)", "content_offset": "optional byte offset into content"}"#,
        handler: filesystem::view_file_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "replace_file_content",
        description: "Surgically edit a contiguous block of text in an existing file. Locates the block using target_content (semantic search-and-replace). start_line and end_line are optional helper fields.",
        arguments: r#"{"path": "absolute or relative path to file", "target_content": "precise block of code to edit (must match file exactly)", "replacement_content": "complete replacement text for that block", "start_line": "optional 1-indexed start line containing target content", "end_line": "optional 1-indexed end line containing target content"}"#,
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

#[allow(dead_code)]
pub const MAX_TOOL_ROUNDS: usize = 60;

pub fn is_agent_tool(name: &str) -> bool {
    matches!(name, "spawn_agent" | "send_agent" | "set_goal" | "todo_write")
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
        p.push_str("Use the 'use_skill' tool to load a skill when a task matches its description.\n\n");
        p.push_str("<available_skills>\n");
        for skill in &skills {
            p.push_str("  <skill>\n");
            p.push_str(&format!("    <name>{}</name>\n", skill.name));
            p.push_str(&format!("    <description>{}</description>\n", skill.description));
            p.push_str("  </skill>\n");
        }
        p.push_str("</available_skills>\n\n");
    }

    if agent_mode == crate::config::AgentMode::Plan {
        p.push_str(
            "CRITICAL: You are operating in PLAN MODE (Read-only / Design mode).\n\
             - File writing, editing, or deletion tools are disabled.\n\
             - You can read files and grep the codebase to design solutions, but you CANNOT write or modify files.\n\
             - Under no circumstances should you call write or edit tools. Explain the plan, and tell the user to switch to Build Mode (press Tab) if they want you to implement the changes.\n\n"
        );
    }

    p.push_str(
        "You are rustcode, a terminal-based coding assistant.\n\
- Use `sandbox/` for temporary scripts/builds, and `artifacts/` for persistent designs/reports.\n\
- For long commands (>2s, e.g. build, test, install), set `\"background\": true` in `run_command`.\n\n\
# Rules\n\
- Be concise and direct. No filler or preamble. Execute tools immediately without conversational fluff.\n\
- Answer concisely in fewer than 4 lines of text (excluding tool call blocks) unless the user explicitly requests detail.\n\
- DO NOT add code comments (such as `// ...` or `/* ... */`) to code files unless explicitly requested by the user.\n\
- After completing a file edit or tool action, stop directly without outputting post-edit summaries or preambles (\"Here is what I changed...\").\n\
- Explore first: use `grep` or `glob` to locate exact function definitions before reading. DO NOT page through large files from line 1 to end with sequential `view_file` calls — use `grep` first to find line numbers, then `view_file` only the target section.\n\
- For editing, use `replace_file_content`. You do NOT need to specify `start_line` or `end_line` — simply copy the code block you want to edit into `target_content` and it will be replaced.\n\
- DO NOT use `run_command` with `cat`, `sed`, `head`, `tail`, or `less`/`more` to read/search files. Always use the native `view_file` or `grep` tools.\n\
- Match project code style.\n\
- Only run tests/builds or commit/push code when explicitly requested by the user.\n\
- Read-only tools run immediately; modifying/destructive tools require confirmation.\n\
- When the task is complete, output a plain-text final summary (with no tool block).\n\n\
# Working memory & avoiding loops
- If a tool execution or compiler check returns compilation errors or warnings, prioritize fixing them immediately before proceeding to other steps.
- File contents you have already read this session are STILL VISIBLE in the conversation. Do NOT re-read a file you already have unless it changed on disk.
- Do not repeat a tool call you just made with the same arguments. If a tool call returns an error, correct your arguments or approach instead of repeating the identical call. If a read or search came up empty, change your query or your approach rather than retrying.
- Use `todo_write` ONLY for complex code refactors or multi-stage tasks (3+ steps). For routine tasks, git operations, single-file edits, or simple questions, DO NOT use `todo_write` — execute tools directly. Do not update `todo_write` after every single command; only update it when completing major milestones.\n\n"
    );

    p.push_str("# Tool Format\n");
    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "To call a tool, output ONLY the fenced `tool` block containing a single JSON object. Do not output any conversational text or narration before or after the block.\n\n\
                ```tool\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}\n\
                ```\n\n\
                Rules:\n\
                - Keys must be \"name\" and \"arguments\".\n\
                - Pass correct type for arguments (no quotes for numbers/booleans).\n\n"
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
    }

    p.push_str("Available tools:\n");
    for t in TOOLS {
        p.push_str(&format!(
            "- {} | Args: {} | {}\n",
            t.name, t.arguments, t.description
        ));
    }
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        for client in reg.values() {
            if let Ok(tools) = client.get_tools() {
                for tool in tools {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let desc = tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let schema = tool.get("inputSchema").unwrap_or(&serde_json::Value::Null);
                    p.push_str(&format!(
                        "- {} | Args: {} | {}\n",
                        name,
                        serde_json::to_string(schema).unwrap_or_default(),
                        desc
                    ));
                }
            }
        }
    }
    if include_agent_tools {
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
                    && last == c {
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

fn parse_tool_calls_tags(text: &str, calls: &mut Vec<(String, Value)>) {
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
                    calls.push((name, json_val));
                } else {
                    let pattern = &*BRACE_OBJ_RE;
                    if let Some(mat) = pattern.find(raw_args)
                        && let Ok(json_val) = serde_json::from_str::<Value>(mat.as_str()) {
                            calls.push((name, json_val));
                        }
                }
            }
        }
    }
}

fn parse_tool_calls_fenced(text: &str, calls: &mut Vec<(String, Value)>) {
    let mut tool_block_to_parse = None;
    if let Some(start) = text.find("```tool") {
        let rest = &text[start + 7..];
        if let Some(end) = rest.find("```") {
            tool_block_to_parse = Some(rest[..end].trim().to_string());
        } else {
            tool_block_to_parse = Some(rest.trim().to_string());
        }
    }

    if let Some(block) = tool_block_to_parse {
        let repaired = repair_json(&block);
        if let Ok(json_value) = serde_json::from_str::<Value>(&repaired)
            && let Some(call) = extract_tool_call(&json_value) {
                calls.push(call);
            }
    }
}

fn parse_tool_calls_impl(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Vec<(String, Value)> {
    let mut calls = Vec::new();

    match protocol {
        crate::config::ToolProtocol::Native => {
            parse_tool_calls_tags(text, &mut calls);
            if calls.is_empty() {
                parse_tool_calls_fenced(text, &mut calls);
            }
        }
        crate::config::ToolProtocol::Json => {
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
            && let Some(call) = extract_tool_call(&json_value) {
                calls.push(call);
            }
    }

    // Try to find JSON objects in the text
    if calls.is_empty() {
        let pattern = &*BRACE_OBJ_RE;
        for mat in pattern.find_iter(text) {
            let json_str = mat.as_str();
            if let Ok(json_value) = serde_json::from_str::<Value>(json_str)
                && let Some(call) = extract_tool_call(&json_value) {
                    calls.push(call);
                }
        }
    }

    calls.dedup();
    calls
}

pub fn parse_tool_calls(text: &str, protocol: crate::config::ToolProtocol) -> Vec<(String, Value)> {
    let raw_calls = parse_tool_calls_impl(text, protocol);
    let mut unique_calls = Vec::new();
    for call in raw_calls {
        if !unique_calls
            .iter()
            .any(|(n, a)| n == &call.0 && a == &call.1)
        {
            unique_calls.push(call);
        }
    }
    unique_calls
}

pub fn parse_tool_call(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Option<(String, Value)> {
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
    fn test_repair_json() {
        assert_eq!(repair_json("{\"name\": \"test\""), "{\"name\": \"test\"}");
        assert_eq!(
            repair_json("{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\""),
            "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\"}}"
        );
        assert_eq!(
            repair_json("{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello"),
            "{\"name\": \"test\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello\"}}"
        );
    }
    
    #[test]
    fn test_parse_truncated_tool_call() {
        let text = "```tool\n{\"name\": \"write_to_file\", \"arguments\": {\"path\": \"/foo\", \"content\": \"hello";
        let calls = parse_tool_calls(text, crate::config::ToolProtocol::Json);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "write_to_file");
        assert_eq!(calls[0].1.get("path").unwrap().as_str().unwrap(), "/foo");
        assert_eq!(calls[0].1.get("content").unwrap().as_str().unwrap(), "hello");
    }

    #[test]
    fn test_parse_tool_calls_tag() {
        let text1 = "Let me check...[TOOL_CALLS]glob[ARGS]{\"pattern\": \"**/*.rs\"}";
        let calls1 = parse_tool_calls(text1, crate::config::ToolProtocol::Json);
        assert_eq!(calls1.len(), 1);
        assert_eq!(calls1[0].0, "glob");
        assert_eq!(calls1[0].1.get("pattern").unwrap().as_str().unwrap(), "**/*.rs");

        let text2 = "Let me check...[TOOL_CALLS]glob\":{\"pattern\":\"**/*.rs\"}";
        let calls2 = parse_tool_calls(text2, crate::config::ToolProtocol::Json);
        assert_eq!(calls2.len(), 1);
        assert_eq!(calls2[0].0, "glob");
        assert_eq!(calls2[0].1.get("pattern").unwrap().as_str().unwrap(), "**/*.rs");

        let text3 = "Plan:[TOOL_CALLS]todo_write[ARGS]{\"todos\": [{\"content\": \"Fix bug\"}]}";
        let calls3 = parse_tool_calls(text3, crate::config::ToolProtocol::Json);
        assert_eq!(calls3.len(), 1);
        assert_eq!(calls3[0].0, "todo_write");
    }
}
