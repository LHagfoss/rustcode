use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};
#[cfg(unix)]
use crate::daemon::{
    client::DaemonClient,
    command,
    lifecycle::DaemonLifecycle,
    protocol::{DaemonRequest, DaemonResponse},
};

#[cfg(unix)]
const MAX_SCHEDULED_JOBS_OUTPUT_BYTES: usize = 16_384;
#[cfg(unix)]
const MAX_LIST_JOBS: usize = 100;
#[cfg(unix)]
const MAX_HISTORY_RUNS: usize = 50;

#[cfg(unix)]
fn manage_scheduled_jobs_schema() -> Value {
    let misfire = serde_json::json!({"type":"string","enum":["skip_missed","run_once"],"default":"skip_missed"});
    serde_json::json!({
        "type":"object",
        "properties": {
            "operation": {"type":"string","enum":["create","list","pause","resume","run","history","delete"]},
            "job_id": {"type":"string","minLength":1},
            "id": {"type":"string","minLength":1},
            "name": {"type":"string","minLength":1},
            "workspace": {"type":"string","minLength":1},
            "target_session": {"type":["string","null"]},
            "schedule": {"oneOf":[
                {"type":"object","properties":{"kind":{"const":"daily"},"hour":{"type":"integer","minimum":0,"maximum":23},"minute":{"type":"integer","minimum":0,"maximum":59},"timezone":{"type":"string"},"misfire_policy":misfire.clone()},"required":["kind","hour","minute","timezone"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"monthly"},"day":{"type":"integer","minimum":1,"maximum":31},"hour":{"type":"integer","minimum":0,"maximum":23},"minute":{"type":"integer","minimum":0,"maximum":59},"timezone":{"type":"string"},"misfire_policy":misfire.clone()},"required":["kind","day","hour","minute","timezone"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"cron"},"expression":{"type":"string"},"timezone":{"type":"string"},"misfire_policy":misfire},"required":["kind","expression","timezone"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"const":"once"},"at":{"type":"string","format":"date-time"}},"required":["kind","at"],"additionalProperties":false}
            ]},
            "action": {"oneOf":[
                {"type":"object","properties":{"type":{"const":"mcp_call"},"server":{"type":"string"},"tool":{"type":"string"},"arguments":{},"workspace":{"type":"string"}},"required":["type","server","tool","workspace"],"additionalProperties":false},
                {"type":"object","properties":{"type":{"const":"prompt"},"prompt":{"type":"string"},"workspace":{"type":"string"},"model_profile":{"type":["string","null"]},"session_id":{"type":["string","null"]}},"required":["type","prompt","workspace"],"additionalProperties":false},
                {"type":"object","properties":{"type":{"const":"shell_command"},"command":{"type":"string"},"working_directory":{"type":"string"},"environment_allowlist":{"type":"array","items":{"type":"string"}},"timeout_seconds":{"type":"integer","minimum":1},"authorized":{"type":"boolean"}},"required":["type","command","working_directory","timeout_seconds"],"additionalProperties":false},
                {"type":"object","properties":{"type":{"const":"poll"},"action":{"type":"object"},"interval_seconds":{"type":"integer","minimum":1},"max_runs":{"type":"integer","minimum":1},"deadline_seconds":{"type":"integer","minimum":1},"stop_on_change":{"type":"boolean"}},"required":["type","action","interval_seconds","max_runs","deadline_seconds"],"additionalProperties":false}
            ]},
            "retry_policy": {"type":"object","properties":{"max_attempts":{"type":"integer","minimum":1},"initial_backoff_seconds":{"type":"integer"},"max_backoff_seconds":{"type":"integer"}},"required":["max_attempts","initial_backoff_seconds","max_backoff_seconds"],"additionalProperties":false},
            "limit": {"type":"integer","minimum":1,"maximum":50}
        },
        "required":["operation"],
        "additionalProperties":false
    })
}

#[cfg(unix)]
pub const MANAGE_SCHEDULED_JOBS: Tool = Tool {
    name: "manage_scheduled_jobs",
    description: "Create and manage durable scheduled jobs through the RustCode daemon. Supports create, list, pause, resume, run, history, and delete. The daemon must already be running.",
    arguments: r#"{"operation":"create|list|pause|resume|run|history|delete", "job_id":"required for job operations", "id":"required for create", "name":"required for create", "workspace":"required for create", "schedule":{"kind":"daily|monthly|cron|once",...}, "action":{"type":"mcp_call|prompt|shell_command|poll",...}, "limit":50}"#,
    handler: manage_scheduled_jobs,
    requires_confirmation: true,
    schema: manage_scheduled_jobs_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ControlPlane,
};

#[cfg(unix)]
fn manage_scheduled_jobs(args: &Value) -> Result<String, String> {
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    let client = DaemonClient::new(DaemonLifecycle::new(config_dir).socket_path());
    manage_scheduled_jobs_with_client(args, &client)
}

#[cfg(unix)]
pub(crate) fn manage_scheduled_jobs_with_client(
    args: &Value,
    client: &DaemonClient,
) -> Result<String, String> {
    let operation = required_string(args, "operation")?;
    let request = match operation {
        "create" => DaemonRequest::Create {
            job: create_job(args)?,
        },
        "list" => DaemonRequest::List,
        "pause" | "resume" => DaemonRequest::SetPaused {
            job_id: required_string(args, "job_id")?.to_owned(),
            paused: operation == "pause",
        },
        "run" => DaemonRequest::RunNow {
            job_id: required_string(args, "job_id")?.to_owned(),
        },
        "history" => DaemonRequest::History {
            job_id: required_string(args, "job_id")?.to_owned(),
            limit: args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, MAX_HISTORY_RUNS as u64) as usize,
        },
        "delete" => DaemonRequest::Delete {
            job_id: required_string(args, "job_id")?.to_owned(),
        },
        _ => return Err(format!("unknown operation '{operation}'")),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| format!("error: daemon client runtime unavailable: {error}"))?;
    let response = runtime
        .block_on(client.request(request))
        .map_err(|error| format!("error: daemon unavailable: {error}"))?;
    format_daemon_response(operation, response)
}

#[cfg(unix)]
fn required_string<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing '{field}'"))
}

#[cfg(unix)]
fn create_job(args: &Value) -> Result<crate::daemon::model::JobRecord, String> {
    let id = required_string(args, "id")?;
    let name = required_string(args, "name")?;
    let workspace = required_string(args, "workspace")?;
    let schedule = serde_json::to_string(args.get("schedule").ok_or("missing 'schedule'")?)
        .map_err(|error| format!("invalid schedule: {error}"))?;
    let action = serde_json::to_string(args.get("action").ok_or("missing 'action'")?)
        .map_err(|error| format!("invalid action: {error}"))?;
    let retry_policy = args
        .get("retry_policy")
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| format!("invalid retry_policy: {error}"))?;
    command::create_job(
        id,
        name,
        workspace,
        &schedule,
        &action,
        args.get("target_session").and_then(Value::as_str),
        retry_policy.as_deref(),
    )
    .map_err(|error| error.message)
}

#[cfg(unix)]
fn format_daemon_response(operation: &str, response: DaemonResponse) -> Result<String, String> {
    match response {
        DaemonResponse::Error { message, .. } => Err(format!("error: {message}")),
        DaemonResponse::Ack => Ok(match operation {
            "create" => "Scheduled job created.".to_owned(),
            "pause" => "Scheduled job paused.".to_owned(),
            "resume" => "Scheduled job resumed.".to_owned(),
            "delete" => "Scheduled job deleted.".to_owned(),
            _ => "Scheduled job updated.".to_owned(),
        }),
        DaemonResponse::RunAccepted { job_id, state } => {
            Ok(format!("Run accepted for '{job_id}' ({state:?})."))
        }
        DaemonResponse::Jobs { jobs } => {
            let total = jobs.len();
            let rows: Vec<Value> = jobs.into_iter().take(MAX_LIST_JOBS).map(|job| serde_json::json!({
                "id":job.id,"name":job.name,"paused":job.paused,"next_due_at":job.next_due_at,
                "schedule":job.schedule
            })).collect();
            bounded_rows("jobs", rows, total)
        }
        DaemonResponse::History { runs } => {
            let total = runs.len();
            let rows: Vec<Value> = runs.into_iter().take(MAX_HISTORY_RUNS).map(|run| serde_json::json!({
                "id":run.id,"job_id":run.job_id,"scheduled_at":run.scheduled_at,"state":run.state,
                "attempt":run.attempt,"started_at":run.started_at,"finished_at":run.finished_at,
                "result_summary":run.result_summary,"error_class":run.error_class
            })).collect();
            bounded_rows("runs", rows, total)
        }
        DaemonResponse::Job { job } if operation == "create" => {
            Ok(format!("Scheduled job '{}' created.", job.id))
        }
        other => Err(format!(
            "error: unexpected daemon response for {operation}: {other:?}"
        )),
    }
}

#[cfg(unix)]
fn bounded_rows(key: &str, mut rows: Vec<Value>, total: usize) -> Result<String, String> {
    loop {
        let included = rows.len();
        let output = serde_json::to_string(&serde_json::json!({
            key: rows,
            "total": total,
            "truncated": included < total,
        }))
        .map_err(|error| format!("error: serializing daemon response: {error}"))?;
        if output.len() <= MAX_SCHEDULED_JOBS_OUTPUT_BYTES || included == 0 {
            return Ok(output);
        }
        rows.pop();
    }
}

pub(crate) const MAX_SESSION_TITLE_CHARS: usize = 80;

fn ask_question_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "description": "Question to ask the user (single-question shape)" },
            "options": { "type": "array", "items": { "type": "string" }, "description": "Choices shown to the user (single-question shape)" },
            "is_multi_select": { "type": "boolean", "default": false },
            "questions": {
                "type": "array",
                "description": "Chained shape: ask several questions in one call; the user answers each in turn (arrow keys move, space ticks, tab switches question, enter submits all)",
                "items": {
                    "type": "object",
                    "properties": {
                        "header": { "type": "string", "description": "Short label shown above the question (max ~30 chars)" },
                        "question": { "type": "string" },
                        "options": { "type": "array", "items": { "type": ["string", "object"] }, "description": "Choices; objects take {label, description}" },
                        "multiple": { "type": "boolean", "default": false, "description": "Allow ticking several options" }
                    },
                    "required": ["question", "options"]
                }
            }
        }
    })
}

pub const ASK_QUESTION: Tool = Tool {
    name: "ask_question",
    description: "Ask the user multiple-choice questions to clarify underspecified requirements, solicit design choices, or select options. Only call this when explicit user validation or decision-making is needed. Do not use for trivial yes/no or routine commands. Prefer the chained 'questions' array (each with a short header, question, options with label/description, and multiple flag) when several decisions are needed: the user answers each in turn with arrow keys, ticks with space for multi-select, moves with tab, and submits all with enter. The UI automatically appends a 'write your own answer' slot for free-form text, so never add your own 'Other' option and never pass an empty options list.",
    arguments: r#"{"questions": [{"header": "Data source", "question": "Where should the version data come from?", "options": [{"label": "CHANGELOG.md", "description": "Curated release notes"}, {"label": "Releases API", "description": "Live GitHub data"}], "multiple": false}]} (chained; or legacy {"question": "...", "options": ["A", "B"], "is_multi_select": false})"#,
    handler: ask_question,
    requires_confirmation: false,
    schema: ask_question_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn get_time_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {}, "additionalProperties": false
    })
}

pub const GET_TIME: Tool = Tool {
    name: "get_time",
    description: "Get the current local date and time",
    arguments: r#"{} (no arguments)"#,
    handler: get_time,
    requires_confirmation: false,
    schema: get_time_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

fn set_session_title_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "minLength": 1,
                "maxLength": MAX_SESSION_TITLE_CHARS,
                "description": "Concise session title; do not copy the full user prompt"
            }
        },
        "required": ["title"],
        "additionalProperties": false
    })
}

pub const SET_SESSION_TITLE: Tool = Tool {
    name: "set_session_title",
    description: "Set a concise title for this new session's first turn. Derive it from the user's request without copying the full prompt, transcript, or secrets.",
    arguments: r#"{"title": "concise session title (1-80 characters)"}"#,
    handler: set_session_title,
    requires_confirmation: false,
    schema: set_session_title_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ControlPlane,
};

fn set_session_title(args: &Value) -> Result<String, String> {
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .ok_or("missing 'title'")?
        .trim();
    if title.is_empty() {
        return Err("title must not be empty".to_string());
    }
    if title.chars().count() > MAX_SESSION_TITLE_CHARS {
        return Err(format!(
            "title must be at most {MAX_SESSION_TITLE_CHARS} characters"
        ));
    }
    if title.chars().any(char::is_control) {
        return Err("title must be a single line without control characters".to_string());
    }
    let session_id = super::get_active_session_id()
        .ok_or("active session is unavailable for setting a title")?;
    crate::config::save_session_title(&session_id, title);
    Ok("Session title saved.".to_string())
}

const MAX_MCP_DISCOVERY_RESULTS: usize = 50;
const DEFAULT_MCP_DISCOVERY_RESULTS: usize = 20;
const MAX_MCP_DISCOVERY_DESCRIPTION_CHARS: usize = 120;
const MAX_MCP_DISCOVERY_CAPABILITIES: usize = 3;

fn list_mcp_tools_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "server": {"type":"string", "minLength":1, "description":"Show tools from this exact registered server"},
            "query": {"type":"string", "minLength":1, "description":"Find tools by name or description"},
            "limit": {"type":"integer", "minimum":1, "maximum":50, "default":20, "description":"Maximum matching tools to return"}
        },
        "additionalProperties": false
    })
}

pub const LIST_MCP_TOOLS: Tool = Tool {
    name: "list_mcp_tools",
    description: "Discover MCP tools from the in-process registry. The default call gives compact server summaries; pass server for that server's tools, or query to search tool names and descriptions. Results include callable_name for invoking discovered tools even when their schema is not in the current request. Never read source files or secrets for discovery.",
    arguments: r#"{"server":"exact server name", "query":"name or description text", "limit":20}"#,
    handler: list_mcp_tools,
    requires_confirmation: false,
    schema: list_mcp_tools_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ReadOnly,
};

fn subagent_id_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "oneOf": [{"type": "integer", "minimum": 1}, {"type": "string"}],
                "description": "Subagent id returned by spawn_agent"
            }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn async_agent_tool(_args: &Value) -> Result<String, String> {
    Err("agent lifecycle tools require the asynchronous session executor".to_owned())
}

pub const WAIT_AGENT: Tool = Tool {
    name: "wait_agent",
    description: "Wait for a subagent to reach one terminal state and return its bounded completion result. This waits for lifecycle activity instead of polling.",
    arguments: r#"{"id": "subagent id"}"#,
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: subagent_id_schema,
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};

pub const CANCEL_AGENT: Tool = Tool {
    name: "cancel_agent",
    description: "Cancel a running or queued subagent task by id.",
    arguments: r#"{"id": "subagent id"}"#,
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: subagent_id_schema,
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};

fn search_web_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "query": { "type": "string" }, "domain": { "type": "string" }
        }, "required": ["query"]
    })
}

pub const SEARCH_WEB: Tool = Tool {
    name: "search_web",
    description: "Performs a web search to look up documentation, API details, or code patterns.",
    arguments: r#"{"query": "search query terms", "domain": "optional domain filter e.g. 'docs.rs'"}"#,
    handler: search_web,
    requires_confirmation: false,
    schema: search_web_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::ReadOnly,
};

fn complete_task_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "result": { "type": "string" } }, "required": ["result"]
    })
}

pub const COMPLETE_TASK: Tool = Tool {
    name: "complete_task",
    description: "Mark the continuous goal/task as successfully complete.",
    arguments: r#"{"result": "summary of what was achieved and final results"}"#,
    handler: complete_task_tool,
    requires_confirmation: false,
    schema: complete_task_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn remember_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Unique key for the fact (e.g. 'package_manager', 'db_port', 'test_runner')" },
            "value": { "type": "string", "description": "The concise fact or rule to remember (max 512 characters)" },
            "category": { "type": "string", "description": "Optional category tag (e.g. 'build', 'architecture', 'convention', 'preference')", "default": "general" },
            "scope": { "type": "string", "enum": ["project", "global"], "description": "Scope of the memory. 'project' (default) is scoped to the current repository; 'global' applies across all projects.", "default": "project" }
        },
        "required": ["key", "value"]
    })
}

pub const REMEMBER: Tool = Tool {
    name: "remember",
    description: "Store a concise, high-value fact, user preference, architecture detail, or convention into persistent memory. Use this when explicitly asked by the user to remember something, or when a durable project convention is established. Do not store secrets, tokens, or entire files.",
    arguments: r#"{"key": "package_manager", "value": "Use pnpm for all install and build commands", "category": "build", "scope": "project"}"#,
    handler: remember,
    requires_confirmation: false,
    schema: remember_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn recall_memory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Search query terms to match against stored memory keys, categories, and values" },
            "scope": { "type": "string", "enum": ["all", "project", "global"], "description": "Scope to search. Defaults to 'all'.", "default": "all" }
        },
        "required": ["query"]
    })
}

pub const RECALL_MEMORY: Tool = Tool {
    name: "recall_memory",
    description: "Search persistent project and global memory for remembered facts, user preferences, architecture decisions, or build instructions matching a query.",
    arguments: r#"{"query": "database port", "scope": "all"}"#,
    handler: recall_memory,
    requires_confirmation: false,
    schema: recall_memory_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

fn forget_memory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "The exact key or category of the fact to remove" },
            "scope": { "type": "string", "enum": ["project", "global", "all"], "description": "Scope to remove from. Defaults to 'project'.", "default": "project" }
        },
        "required": ["key"]
    })
}

pub const FORGET_MEMORY: Tool = Tool {
    name: "forget_memory",
    description: "Remove a fact from persistent memory by key or category. Use when a remembered fact is obsolete, contradicted, or when asked by the user to forget something.",
    arguments: r#"{"key": "package_manager", "scope": "project"}"#,
    handler: forget_memory,
    requires_confirmation: false,
    schema: forget_memory_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::Unknown,
};

fn use_skill_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"]
    })
}

fn list_skills_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

pub const LIST_SKILLS: Tool = Tool {
    name: "list_skills",
    description: "Discover available skills and their short descriptions without loading their instruction bodies.",
    arguments: r#"{}"#,
    handler: list_skills,
    requires_confirmation: false,
    schema: list_skills_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub const USE_SKILL: Tool = Tool {
    name: "use_skill",
    description: "Load a skill by name to get its instructions and available files. This control-plane call must be emitted alone so the loaded instructions apply before the next action.",
    arguments: r#"{"name": "skill name"}"#,
    handler: use_skill,
    requires_confirmation: false,
    schema: use_skill_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ControlPlane,
};

pub fn list_skills(args: &Value) -> Result<String, String> {
    if !args.is_object() {
        return Err("arguments must be a JSON object".to_string());
    }

    let mut skills = crate::skills::discover_skills_for_catalog();
    if skills.is_empty() {
        return Ok(crate::skills::no_skills_message());
    }
    // Higher-priority skills first so the model sees the most relevant ones.
    skills.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.name.cmp(&b.name)));

    let mut out = format!("<available_skills count=\"{}\">\n", skills.len());
    for skill in &skills {
        out.push_str(&format!(
            "  <skill><name>{}</name><description>{}</description><priority>{}</priority></skill>\n",
            skill.name, skill.description, skill.priority
        ));
    }
    out.push_str("</available_skills>\n");
    out.push_str(
        "Call use_skill with the exact name of a matching skill to load its instructions.\n",
    );
    out.push_str(&crate::skills::format_skill_roots(
        &crate::skills::skill_roots(),
    ));
    Ok(out)
}

pub fn ask_question(args: &Value) -> Result<String, String> {
    // Chained shape: summarize every question; the interactive path in
    // tool_exec handles the live modal, this is the non-interactive fallback.
    if let Some(items) = args.get("questions").and_then(|v| v.as_array())
        && !items.is_empty()
    {
        let mut out = String::new();
        for (qi, item) in items.iter().enumerate() {
            let question = item
                .get("question")
                .or_else(|| item.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("(no question)");
            let multi = item
                .get("multiple")
                .or_else(|| item.get("is_multi_select"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            out.push_str(&format!(
                "ASK_QUESTION {}/{}: {} | Multi: {}",
                qi + 1,
                items.len(),
                question,
                multi
            ));
            if let Some(options) = item.get("options").and_then(|v| v.as_array()) {
                for (i, opt) in options.iter().enumerate() {
                    let label = opt
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| opt.get("label").and_then(|v| v.as_str()).map(str::to_owned))
                        .unwrap_or_default();
                    out.push_str(&format!("\n{}. {}", i + 1, label));
                }
            }
            out.push_str("\nOther: (type custom response)\n");
        }
        return Ok(out);
    }
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .ok_or("missing 'question'")?;
    let options = args
        .get("options")
        .and_then(|v| v.as_array())
        .ok_or("missing 'options'")?;
    let is_multi_select = args
        .get("is_multi_select")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut out = format!("ASK_QUESTION: {} | Multi: {}", question, is_multi_select);
    for (i, opt) in options.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, opt.as_str().unwrap_or("")));
    }
    out.push_str("\nOther: (type custom response)");
    Ok(out)
}

pub fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
}

pub fn list_mcp_tools(args: &Value) -> Result<String, String> {
    if !args.is_object() {
        return Err("arguments must be a JSON object".to_string());
    }

    for (key, _) in args.as_object().expect("object checked") {
        if !["server", "query", "limit"].contains(&key.as_str()) {
            return Err(format!("invalid argument '{key}'"));
        }
    }
    let server = optional_nonempty_string(args, "server")?;
    let query = optional_nonempty_string(args, "query")?;
    let limit = match args.get("limit") {
        None => DEFAULT_MCP_DISCOVERY_RESULTS,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=MAX_MCP_DISCOVERY_RESULTS as u64).contains(value))
            .map(|value| value as usize)
            .ok_or_else(|| {
                format!("'limit' must be an integer from 1 to {MAX_MCP_DISCOVERY_RESULTS}")
            })?,
    };

    let mut clients = {
        let registry_handle = crate::mcp::get_mcp_registry();
        let registry = registry_handle
            .lock()
            .map_err(|error| format!("MCP registry unavailable: {error}"))?;
        registry.values().cloned().collect::<Vec<_>>()
    };
    clients.sort_by(|a, b| a.name.cmp(&b.name));

    if let Some(server) = server
        && !clients.iter().any(|client| client.name == server)
    {
        return Err(format!("unknown MCP server '{server}'"));
    }

    let inventories = clients
        .iter()
        .filter(|client| server.is_none_or(|server| client.name == server))
        .map(|client| {
            let tools = client
                .get_tools()
                .map_err(|error| {
                    format!(
                        "MCP server '{}' tool metadata unavailable: {error}",
                        client.name
                    )
                })?
                .into_iter()
                .filter_map(|tool| {
                    let name = tool.get("name")?.as_str()?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    Some(McpDiscoveryTool {
                        name: name.to_owned(),
                        callable_name: super::schema::mcp_canonical_name_for_clients(
                            &client.name,
                            name,
                            &clients,
                        ),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    })
                })
                .collect::<Vec<_>>();
            Ok((client.name.clone(), tools))
        })
        .collect::<Result<Vec<_>, String>>()?;

    format_mcp_discovery(&inventories, server, query, limit)
}

fn optional_nonempty_string<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    args.get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("'{key}' must be a non-empty string"))
        })
        .transpose()
}

#[derive(Clone)]
struct McpDiscoveryTool {
    name: String,
    callable_name: String,
    description: String,
}

fn format_mcp_discovery(
    inventories: &[(String, Vec<McpDiscoveryTool>)],
    server: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> Result<String, String> {
    if server.is_none() && query.is_none() {
        let summaries = inventories
            .iter()
            .map(|(name, tools)| {
                let mut tools = tools.clone();
                tools.sort_by(|a, b| a.name.cmp(&b.name));
                let capabilities = tools
                    .iter()
                    .filter_map(|tool| {
                        let cue = if tool.description.trim().is_empty() {
                            tool.name.as_str()
                        } else {
                            tool.description.as_str()
                        };
                        Some(truncate_mcp_description(cue))
                    })
                    .filter(|cue| !cue.is_empty())
                    .take(MAX_MCP_DISCOVERY_CAPABILITIES)
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "name": name,
                    "tool_count": tools.len(),
                    "capabilities": capabilities
                })
            })
            .collect::<Vec<_>>();
        return serde_json::to_string(&serde_json::json!({ "servers": summaries }))
            .map_err(|error| format!("failed to serialize MCP inventory: {error}"));
    }

    if let Some(server) = server
        && !inventories.iter().any(|(name, _)| name == server)
    {
        return Err(format!("unknown MCP server '{server}'"));
    }
    let query = query.map(str::to_lowercase);
    let mut results = Vec::new();
    for (server_name, tools) in inventories {
        if server.is_some_and(|server| server_name != server) {
            continue;
        }
        for tool in tools {
            let matches = query.as_ref().is_none_or(|query| {
                tool.name.to_lowercase().contains(query)
                    || tool.description.to_lowercase().contains(query)
            });
            if matches {
                results.push(serde_json::json!({
                    "server": server_name,
                    "name": tool.name,
                    "callable_name": tool.callable_name,
                    "description": truncate_mcp_description(&tool.description)
                }));
            }
        }
    }
    results.sort_by(|a, b| {
        a.get("server")
            .and_then(Value::as_str)
            .cmp(&b.get("server").and_then(Value::as_str))
            .then_with(|| {
                a.get("name")
                    .and_then(Value::as_str)
                    .cmp(&b.get("name").and_then(Value::as_str))
            })
    });
    let total = results.len();
    results.truncate(limit);
    serde_json::to_string(&serde_json::json!({
        "results": results,
        "total": total,
        "truncated": total > limit
    }))
    .map_err(|error| format!("failed to serialize MCP search results: {error}"))
}

fn truncate_mcp_description(description: &str) -> String {
    let description = description.trim();
    let mut chars = description.chars();
    let short = chars
        .by_ref()
        .take(MAX_MCP_DISCOVERY_DESCRIPTION_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

pub fn complete_task_tool(args: &Value) -> Result<String, String> {
    let result = args
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or("missing 'result' argument")?;
    Ok(format!(
        "Task successfully marked as complete! Result: {result}"
    ))
}

pub fn search_web(args: &Value) -> Result<String, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create async runtime: {e}"))?;
    runtime.block_on(search_web_async(args, &reqwest::Client::new()))
}

pub(crate) async fn search_web_async(
    args: &Value,
    client: &reqwest::Client,
) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or("missing 'query' argument")?;
    let domain = args.get("domain").and_then(|d| d.as_str());

    let mut search_query = query.to_string();
    if let Some(dom) = domain {
        search_query.push_str(&format!(" site:{}", dom));
    }

    let exa_key = crate::shell_env::env_var("EXA_API_KEY")
        .unwrap_or_else(|| "9a49efa5-675c-4684-94c0-3f96979aa2ac".to_string());
    if !exa_key.is_empty() {
        let body = serde_json::json!({
            "query": search_query,
            "numResults": 5,
            "useAutoprompt": true,
            "contents": {
                "text": {
                    "maxCharacters": 1000
                }
            }
        });

        if let Ok(response) = client
            .post("https://api.exa.ai/search")
            .timeout(std::time::Duration::from_secs(10))
            .header("x-api-key", &exa_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            && response.status().is_success()
            && let Ok(res_json) = response.json::<serde_json::Value>().await
            && let Some(results) = res_json.get("results").and_then(|r| r.as_array())
        {
            let mut out = String::new();
            out.push_str(&format!(
                "Web Search Results for '{}' (via Exa AI):\n\n",
                search_query
            ));
            for (i, r) in results.iter().enumerate() {
                let title = r
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("No Title");
                let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("");
                let snippet = if text.len() > 300 { &text[..300] } else { text };

                out.push_str(&format!(
                    "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                    i + 1,
                    title,
                    snippet.trim(),
                    url
                ));
            }
            if !results.is_empty() {
                return Ok(out);
            }
        }
    }

    if let Some(api_key) = crate::shell_env::env_var("TAVILY_API_KEY") {
        let body = serde_json::json!({
            "api_key": api_key,
            "query": search_query,
            "max_results": 5
        });

        let response = client
            .post("https://api.tavily.com/search")
            .timeout(std::time::Duration::from_secs(10))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Tavily request failed: {e}"))?;

        if response.status().is_success() {
            let res_json: serde_json::Value = response
                .json()
                .await
                .map_err(|e| format!("failed to parse Tavily JSON: {e}"))?;

            if let Some(results) = res_json.get("results").and_then(|r| r.as_array()) {
                let mut out = String::new();
                out.push_str(&format!(
                    "Web Search Results for '{}' (via Tavily):\n\n",
                    search_query
                ));
                for (i, r) in results.iter().enumerate() {
                    let title = r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("No Title");
                    let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let content = r.get("content").and_then(|c| c.as_str()).unwrap_or("");

                    out.push_str(&format!(
                        "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                        i + 1,
                        title,
                        content,
                        url
                    ));
                }
                if !results.is_empty() {
                    return Ok(out);
                }
            }
        }
    }

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(&search_query)
    );

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| format!("failed to request search results: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "web search failed with status: {}",
            response.status()
        ));
    }

    let html_content = response
        .text()
        .await
        .map_err(|e| format!("failed to read search response body: {e}"))?;

    if html_content.contains("anomaly-modal") || html_content.contains("bots use DuckDuckGo too") {
        return Err("Web search failed because DuckDuckGo triggered bot/CAPTCHA protection.\n\
                   To bypass this and get reliable web search, please sign up for a free Tavily account (1,000 free searches/mo) at https://tavily.com and set the TAVILY_API_KEY environment variable.".to_string());
    }

    let document = scraper::Html::parse_document(&html_content);

    let result_selector = scraper::Selector::parse(".result").unwrap();
    let snippet_selector = scraper::Selector::parse(".result__snippet").unwrap();
    let url_selector = scraper::Selector::parse(".result__url").unwrap();

    let mut out = String::new();
    out.push_str(&format!(
        "Web Search Results for '{}' (via DuckDuckGo):\n\n",
        search_query
    ));

    let mut count = 0;
    for element in document.select(&result_selector) {
        if count >= 6 {
            break;
        }

        let snippet_node = element.select(&snippet_selector).next();
        let url_node = element.select(&url_selector).next();

        if let (Some(s_node), Some(u_node)) = (snippet_node, url_node) {
            let snippet = s_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let link = u_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();

            count += 1;
            out.push_str(&format!(
                "{}. Snippet: {}\n   Source: https://{}\n\n",
                count, snippet, link
            ));
        }
    }

    if count == 0 {
        return Ok("No results found. Try refining your query.".to_string());
    }

    Ok(out)
}

pub fn use_skill(args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("missing 'name' argument")?;

    let skill = crate::skills::get_skill_content(name);
    crate::skills::bump_skill_catalog_generation();
    let skill = skill.ok_or_else(|| {
        format!(
            "Skill '{}' not found. Call list_skills to discover available skills.",
            name
        )
    })?;

    let files = crate::skills::list_skill_files(&skill.path);
    let mut out = format!("<skill_content name=\"{}\">\n", skill.name);
    out.push_str(&skill.content);
    if !files.is_empty() {
        out.push_str("\n---\nFiles in skill directory:\n");
        for f in &files {
            out.push_str(&format!("  - {}\n", f));
        }
    }
    out.push_str(
        "\n---\n<harness_execution_paths>\n\
  <path tool=\"run_command\" available=\"true\">Use this registered tool for CLI workflows explicitly described by the skill.</path>\n\
  <path tool=\"native_registry\" available=\"true\">Only tools listed in the current tool inventory are executable as native tools.</path>\n\
  <path tool=\"unknown_native_tools\" available=\"false\">A skill cannot create or imply a native tool that is absent from the registry.</path>\n\
</harness_execution_paths>\n",
    );
    out.push_str("</skill_content>\n");
    Ok(out)
}

pub fn remember(args: &Value) -> Result<String, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'key'")?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'value'")?;
    let category = args
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("general");
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("project");

    let fact = crate::memory::fact(category, key, value, "agent memory tool");
    if scope == "global" {
        crate::memory::upsert_global(fact)?;
        Ok(format!("Remembered globally: [{category}] {key} = {value}"))
    } else {
        crate::memory::upsert(None, fact)?;
        Ok(format!(
            "Remembered for this project: [{category}] {key} = {value}"
        ))
    }
}

pub fn recall_memory(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'query'")?;
    let scope = args.get("scope").and_then(|v| v.as_str()).unwrap_or("all");

    let facts = crate::memory::search_facts(None, query, scope);
    if facts.is_empty() {
        return Ok(format!("No remembered facts found matching '{query}'."));
    }

    let mut output = format!("Found {} relevant memory item(s):\n", facts.len());
    for (item_scope, fact) in facts {
        output.push_str(&format!(
            "- ({item_scope}) [{}] {}: {}\n",
            fact.category, fact.key, fact.value
        ));
    }
    Ok(output.trim_end().to_string())
}

pub fn forget_memory(args: &Value) -> Result<String, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("missing required argument 'key'")?;
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .unwrap_or("project");

    let mut total_removed = 0;
    if scope == "all" || scope == "global" {
        if let Ok(removed) = crate::memory::remove_global(key) {
            total_removed += removed;
        }
    }
    if scope == "all" || scope == "project" {
        if let Ok(removed) = crate::memory::remove(None, key) {
            total_removed += removed;
        }
    }

    if total_removed == 0 {
        Ok(format!(
            "No memory facts found for '{key}' in scope '{scope}'."
        ))
    } else {
        Ok(format!("Removed {total_removed} fact(s) matching '{key}'."))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LIST_MCP_TOOLS, McpDiscoveryTool, format_mcp_discovery, list_mcp_tools, search_web_async,
    };

    fn discovery_fixture() -> Vec<(String, Vec<McpDiscoveryTool>)> {
        vec![
            (
                "calendar".to_string(),
                (0..12)
                    .map(|index| McpDiscoveryTool {
                        name: format!("event_{index:02}"),
                        callable_name: format!("event_{index:02}"),
                        description: format!(
                            "Create and update calendar events with attendee metadata and long description {index}"
                        ),
                    })
                    .collect(),
            ),
            (
                "mail".to_string(),
                vec![
                    McpDiscoveryTool {
                        name: "search_messages".to_string(),
                        callable_name: "search_messages".to_string(),
                        description: "Search email messages by sender or subject".to_string(),
                    },
                    McpDiscoveryTool {
                        name: "send_message".to_string(),
                        callable_name: "send_message".to_string(),
                        description: "Send an email message".to_string(),
                    },
                ],
            ),
        ]
    }

    #[test]
    fn mcp_inventory_has_a_strict_discovery_argument_schema() {
        let schema = (LIST_MCP_TOOLS.schema)();
        assert_eq!(
            schema.get("type").and_then(|value| value.as_str()),
            Some("object")
        );
        let properties = schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("discovery options should be declared");
        assert_eq!(
            properties.keys().map(String::as_str).collect::<Vec<_>>(),
            ["server", "query", "limit"]
        );
        assert_eq!(properties["server"]["type"], "string");
        assert_eq!(properties["query"]["type"], "string");
        assert_eq!(properties["limit"]["maximum"], 50);
        assert_eq!(
            schema.get("additionalProperties"),
            Some(&serde_json::json!(false))
        );
        assert_eq!(LIST_MCP_TOOLS.safety, super::ToolSafety::ReadOnly);
        assert!(!LIST_MCP_TOOLS.requires_confirmation);
    }

    #[test]
    fn mcp_default_inventory_is_compact_and_summarizes_registered_tools() {
        let inventories = discovery_fixture();
        let result =
            format_mcp_discovery(&inventories, None, None, 20).expect("summary should succeed");
        let inventory: serde_json::Value =
            serde_json::from_str(&result).expect("inventory should be JSON");
        let servers = inventory
            .get("servers")
            .and_then(serde_json::Value::as_array)
            .expect("inventory should contain a server array");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0]["name"], "calendar");
        assert_eq!(servers[0]["tool_count"], 12);
        assert!(servers[0]["capabilities"].as_array().unwrap().len() <= 3);
        assert!(servers[0].get("tools").is_none());

        let legacy = serde_json::to_string_pretty(&serde_json::json!({
            "servers": inventories.iter().map(|(name, tools)| serde_json::json!({
                "name": name,
                "tools": tools.iter().map(|tool| serde_json::json!({
                    "name": tool.name,
                    "description": tool.description
                })).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        }))
        .unwrap();
        assert!(
            result.len() < legacy.len() / 2,
            "{} vs {} bytes",
            result.len(),
            legacy.len()
        );
    }

    #[test]
    fn mcp_discovery_searches_metadata_and_bounds_results() {
        let inventories = discovery_fixture();
        let result = format_mcp_discovery(&inventories, None, Some("calendar"), 4)
            .expect("search should succeed");
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["total"], 12);
        assert_eq!(result["results"].as_array().unwrap().len(), 4);
        assert_eq!(result["truncated"], true);

        let result = format_mcp_discovery(&inventories, None, Some("sender"), 20)
            .expect("description search should succeed");
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["results"][0]["name"], "search_messages");
        assert_eq!(result["results"][0]["callable_name"], "search_messages");
    }

    #[test]
    fn mcp_discovery_scopes_to_exact_server_and_reports_unknown_servers() {
        let inventories = discovery_fixture();
        let result = format_mcp_discovery(&inventories, Some("mail"), None, 20)
            .expect("server inventory should succeed");
        let result: serde_json::Value = serde_json::from_str(&result).unwrap();
        let rows = result["results"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row["server"] == "mail"));
        assert!(
            format_mcp_discovery(&inventories, Some("Mail"), None, 20)
                .unwrap_err()
                .contains("unknown MCP server 'Mail'")
        );
    }

    #[test]
    fn mcp_discovery_rejects_invalid_arguments_explicitly() {
        assert!(
            list_mcp_tools(&serde_json::json!({"unexpected": true}))
                .unwrap_err()
                .contains("invalid argument 'unexpected'")
        );
        assert!(
            list_mcp_tools(&serde_json::json!({"limit": 0}))
                .unwrap_err()
                .contains("'limit' must be an integer")
        );
        assert!(
            list_mcp_tools(&serde_json::json!({"query": "  "}))
                .unwrap_err()
                .contains("'query' must be a non-empty string")
        );
    }

    #[tokio::test]
    async fn async_search_web_requires_query() {
        let error = search_web_async(&serde_json::json!({}), &reqwest::Client::new())
            .await
            .expect_err("missing query should fail before any request");

        assert_eq!(error, "missing 'query' argument");
    }
}
