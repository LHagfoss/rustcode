mod orchestration_tools;
mod web;

pub use orchestration_tools::{
    ANSWER_REQUEST, CLEANUP_WORKTREE, CLONE_PROJECT, COMPACT_SESSION, CONFIGURE_SESSION,
    CREATE_LOOP, CREATE_PROJECT, DELETE_SCHEDULED_TASK, FAN_OUT, HANDOFF_WORKTREE,
    INTERRUPT_SESSION, LIST_HARNESSES, LIST_MACHINES, LIST_PENDING_REQUESTS, LIST_PROJECTS,
    LIST_PULL_REQUESTS, LIST_SCHEDULED_TASKS, LIST_SESSIONS, LIST_WORKTREES, MANAGE_PULL_LINKS,
    MANAGE_QUEUE, MERGE_WHEN_GREEN, MESSAGE_SESSION, ORGANIZE_THREADS, QUERY_DATA, READ_ACTIVITY,
    READ_PROJECT, READ_SESSION, REMOVE_PROJECT, RUN_MACHINE_COMMAND, RUN_SCHEDULED_TASK_NOW,
    SCHEDULE_TASK, SEARCH_SESSIONS, START_SESSION, THREAD_STATUS, THREAD_USAGE, TRANSFER_SESSION,
    UPDATE_LOOP, UPDATE_PROJECT, UPDATE_SESSION, WAIT_SESSION, WORKTREE_STATUS,
};
pub(crate) use web::search_web_async;
#[cfg(test)]
use web::{clip_snippet, page_slice, readable_page_text, scoped_search_query};
pub use web::{read_url, search_web};

use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};
use crate::daemon::{
    client::DaemonClient,
    command,
    lifecycle::DaemonLifecycle,
    protocol::{DaemonRequest, DaemonResponse},
};

const MAX_SCHEDULED_JOBS_OUTPUT_BYTES: usize = 16_384;
const MAX_LIST_JOBS: usize = 100;
const MAX_HISTORY_RUNS: usize = 50;

fn manage_scheduled_jobs_schema() -> Value {
    // One flat object for each nested spec instead of one branch per kind: the
    // native `tools` block carries this schema on every request, and the four
    // `oneOf` branches repeated `timezone`, `hour`, `minute` and
    // `misfire_policy` up to four times each (#3376). Every field is still
    // declared, and the daemon (`create_job`) validates the fields each kind
    // and type needs, so a call the schema no longer rules out fails there with
    // the field it wants.
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
            "schedule": {"type":"object","properties":{
                "kind":{"type":"string","enum":["daily","monthly","cron","once"]},
                "hour":{"type":"integer","minimum":0,"maximum":23},
                "minute":{"type":"integer","minimum":0,"maximum":59},
                "day":{"type":"integer","minimum":1,"maximum":31},
                "timezone":{"type":"string"},
                "expression":{"type":"string"},
                "at":{"type":"string","format":"date-time"},
                "misfire_policy":misfire
            },"required":["kind"],"additionalProperties":false},
            "action": {"type":"object","properties":{
                "type":{"type":"string","enum":["mcp_call","prompt","shell_command","poll"]},
                "server":{"type":"string"},
                "tool":{"type":"string"},
                "arguments":{},
                "prompt":{"type":"string"},
                "command":{"type":"string"},
                "working_directory":{"type":"string"},
                "environment_allowlist":{"type":"array","items":{"type":"string"}},
                "timeout_seconds":{"type":"integer","minimum":1},
                "authorized":{"type":"boolean"},
                "model_profile":{"type":["string","null"]},
                "session_id":{"type":["string","null"]},
                "action":{"type":"object"},
                "interval_seconds":{"type":"integer","minimum":1},
                "max_runs":{"type":"integer","minimum":1},
                "deadline_seconds":{"type":"integer","minimum":1},
                "stop_on_change":{"type":"boolean"}
            },"required":["type"],"additionalProperties":false},
            "retry_policy": {"type":"object","description":"max_attempts, initial_backoff_seconds and max_backoff_seconds; the daemon fills in its defaults"},
            "limit": {"type":"integer","minimum":1,"maximum":50}
        },
        "required":["operation"],
        "additionalProperties":false
    })
}

pub const MANAGE_SCHEDULED_JOBS: Tool = Tool {
    name: "manage_scheduled_jobs",
    description: "Manage durable jobs; daemon must be running. Required fields by kind/type: daily hour/minute/timezone; monthly adds day; cron expression/timezone; once at; mcp_call server/tool/workspace; prompt prompt/workspace; shell_command command/working_directory/timeout_seconds; poll action/interval_seconds/max_runs/deadline_seconds.",
    arguments: r#"{"operation":"create|list|pause|resume|run|history|delete", "job_id":"required for job operations", "id":"required for create", "name":"required for create", "workspace":"required for create", "schedule":{"kind":"daily|monthly|cron|once",...}, "action":{"type":"mcp_call|prompt|shell_command|poll",...}, "limit":50}"#,
    handler: manage_scheduled_jobs,
    requires_confirmation: true,
    schema: manage_scheduled_jobs_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ControlPlane,
};

fn manage_scheduled_jobs(args: &Value) -> Result<String, String> {
    let config_dir =
        crate::config::get_config_dir().ok_or("error: config directory unavailable")?;
    let client = DaemonClient::new(DaemonLifecycle::new(config_dir).socket_path());
    manage_scheduled_jobs_with_client(args, &client)
}

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
            input: None,
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

fn required_string<'a>(args: &'a Value, field: &str) -> Result<&'a str, String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing '{field}'"))
}

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

pub(crate) const MAX_SESSION_TITLE_CHARS: usize = rustcode_session::MAX_AUTOMATIC_TITLE_CHARS;

fn ask_question_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "description": "Single-question shape" },
            "options": { "type": "array", "items": { "type": ["string", "object"] } },
            "is_multi_select": { "type": "boolean", "default": false },
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "header": { "type": "string", "description": "Short label (~30 chars)" },
                        "question": { "type": "string" },
                        "options": { "type": "array", "items": { "type": ["string", "object"] }, "description": "Strings or {label, description}" },
                        "multiple": { "type": "boolean", "default": false }
                    },
                    "required": ["question", "options"]
                }
            }
        }
    })
}

pub const ASK_QUESTION: Tool = Tool {
    name: "ask_question",
    description: "Ask the user multiple-choice questions. Prefer the questions array for several decisions. Never pass empty options or an 'Other' option: the UI adds a free-text slot.",
    arguments: r#"{"questions": [{"header": "Data source", "question": "Where should the version data come from?", "options": [{"label": "CHANGELOG.md", "description": "Curated release notes"}, {"label": "Releases API", "description": "Live GitHub data"}], "multiple": false}]} (chained; or legacy {"question": "...", "options": ["A", "B"], "is_multi_select": false})"#,
    handler: ask_question,
    requires_confirmation: false,
    schema: ask_question_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn desktop_control_schema() -> Value {
    crate::desktop_control::schema()
}
fn desktop_control_unavailable(_: &Value) -> Result<String, String> {
    Err("Desktop control is unavailable in this client.".into())
}
pub const DESKTOP_CONTROL: Tool = Tool {
    name: crate::desktop_control::TOOL_NAME,
    description: "Control a user-approved native macOS application. Snapshot an app, then click or type using its returned ref; observe again to verify. Each operation requires user approval.",
    arguments: r#"{"action":"snapshot","app":"Finder"}"#,
    handler: desktop_control_unavailable,
    requires_confirmation: false,
    schema: desktop_control_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const SHARE_PREVIEW: Tool = Tool {
    name: crate::acp::preview_share::TOOL_NAME,
    description: "Ask for one-time approval to share this thread's loopback HTTP server via a public link for one hour. Anyone with the link can access the server. Prefer a static build server to avoid exposing a dev server's source files.",
    arguments: r#"{"port": 4173}"#,
    handler: |_| Err("Preview sharing needs a thread and a RustCode approval window.".to_owned()),
    requires_confirmation: false,
    schema: crate::acp::preview_share::schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn localhost_preview_schema() -> Value {
    crate::preview::operation_schema()
}

/// The macOS app's localhost preview, driven by the agent (#2138). Offered
/// only to sessions whose client advertised `_meta.rustcode.preview`; the
/// call is forwarded to that client, which asks the user before the first
/// action and runs it in the visible preview. Executed anywhere else it
/// fails: this handler is the fallback, never the real bridge.
pub const LOCALHOST_PREVIEW: Tool = Tool {
    name: crate::preview::PREVIEW_TOOL_NAME,
    description: "The user's live preview panel in the RustCode macOS app (localhost only, user-approved). `navigate` to a local URL such as http://localhost:5173 opens the panel and shows that page, even when nothing is open yet; then inspect, screenshot, click, fill, select or scroll it. When the user asks to open or show a page in the (live) preview, this is the tool. Use it to check a change once, not to explore: results list errors the page threw, so read them before guessing. When the page is blank or wrong and they do not explain why, ask the user what they see rather than probing again.",
    arguments: r##"{"action": "inspect"} | {"action": "screenshot"} | {"action": "navigate", "url": "/settings"} | {"action": "click", "selector": "#save"} | {"action": "fill", "selector": "input[name=email]", "value": "a@b.c"} | {"action": "select", "selector": "#plan", "value": "pro"} | {"action": "scroll", "dy": 600}"##,
    handler: localhost_preview_unavailable,
    requires_confirmation: false,
    schema: localhost_preview_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn smart_ui_preview_schema() -> Value {
    crate::smart_ui::schema()
}

/// Renders a Smart UI block in the macOS app before the agent writes it.
/// Offered only while Smart UI is on and the app can render previews; the
/// call is forwarded to the app, so this handler is only the fallback.
pub const SMART_UI_PREVIEW: Tool = Tool {
    name: crate::smart_ui::PREVIEW_TOOL_NAME,
    description: "Render a rustcode-ui v1 block's HTML offscreen exactly as the chat card will show it (same sandbox, width and height limits) and get back a screenshot, the content height, the element count and any errors the page threw. Nothing is shown to the user. Use it before writing the block, then write the fence with the HTML you checked.",
    arguments: r##"{"html": "<title>Slowest tests</title><table>…</table><script>…</script>"}"##,
    handler: smart_ui_preview_unavailable,
    requires_confirmation: false,
    schema: smart_ui_preview_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

fn smart_ui_preview_unavailable(_args: &Value) -> Result<String, String> {
    Err(crate::smart_ui::UNAVAILABLE_MESSAGE.to_owned())
}

/// Checks a `rustcode-ui v1` block without running it. Offered to external
/// harnesses through the `rustcode` MCP server, which answers it itself
/// (`orchestration/mcp_server.rs`); a RustCode thread uses its preview instead.
pub const SMART_UI_CHECK: Tool = Tool {
    name: crate::smart_ui::CHECK_TOOL_NAME,
    description: "Check a ```rustcode-ui v1 block before you write it: its size, about how many elements it builds, a missing title, an unclosed script or style tag, a rustcode-lib the app does not bundle, and what the page cannot do (network, storage, vh heights, system colours on a canvas), each with its line. It does not run the page, so it reports no script errors, no fitted height and no screenshot.",
    arguments: r##"{"html": "<title>…</title><script src=\"rustcode-lib:chart.js\"></script>…"}"##,
    handler: smart_ui_check_unavailable,
    requires_confirmation: false,
    schema: crate::smart_ui::check_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

/// A multiple-choice question for an external harness with no question tool
/// of its own (#3393). Offered through the `rustcode` MCP server only to such
/// a harness; the engine's hub asks it (`orchestration/ask_user.rs`). A
/// RustCode thread asks with `ask_question` instead.
pub const ASK_USER_QUESTION: Tool = Tool {
    name: crate::orchestration::ask_user::TOOL_NAME,
    description: "Stop and ask the user 1 to 4 multiple-choice questions when a decision is theirs to make, instead of guessing or asking in prose. Each question has 2 to 4 options with a label and a description; set multi_select to let them pick several. Do not add an 'Other' option: the app offers a free-text answer itself. Blocks until they answer and returns {\"answers\": [{\"question\", \"selected\": [labels], \"text\"?}]}; if they dismiss it, continue on your own judgement.",
    arguments: r#"{"questions": [{"header": "Data source", "question": "Where should the version data come from?", "options": [{"label": "CHANGELOG.md", "description": "Curated release notes"}, {"label": "Releases API", "description": "Live GitHub data"}], "multi_select": false}]}"#,
    handler: ask_user_question_unavailable,
    requires_confirmation: false,
    schema: crate::orchestration::ask_user::schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Start a durable workflow run (#3394). Offered through the `rustcode` MCP
/// server, where the engine's hub runs it, and to a built-in turn with the
/// version 2 orchestration tools, where `network/tool_exec.rs` routes it to
/// the same runner (#3943); the handler is never reached.
pub const RUN_WORKFLOW: Tool = Tool {
    name: crate::workflow::RUN_TOOL_NAME,
    description: "Start a workflow run: a JavaScript script that fans work out to worker threads and replays its finished calls after a restart. Give `script` (the body of an async function that returns a JSON value) or `name` (a file in .rustcode/workflows/). In the script: agent(prompt, {schema?, harness?, model?, effort?, title?}) starts a delegate thread and returns its reply, parsed and checked against `schema` if given; parallel([() => agent(...), ...]) runs calls together; phase(title) names the next step; log(...); `args` is your `args`. No files, shell, network, Date or Math.random. Returns a run_id at once; read it with workflow_status. `concurrency`, `call_budget` and `timeout_minutes` can only lower the user's limits.",
    arguments: r#"{"script": "phase('review'); const reviews = await parallel(args.files.map((f) => () => agent('Review ' + f, {schema: {type: 'object', properties: {issues: {type: 'array', items: {type: 'string'}}}, required: ['issues']}}))); phase('verify'); return await agent('Verify these findings: ' + JSON.stringify(reviews));", "args": {"files": ["src/a.rs", "src/b.rs"]}}"#,
    handler: workflow_unavailable,
    requires_confirmation: false,
    schema: crate::workflow::run_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

/// Read a workflow run (#3394): its phases, its workers and its result.
pub const WORKFLOW_STATUS: Tool = Tool {
    name: crate::workflow::STATUS_TOOL_NAME,
    description: "Read a workflow run started with run_workflow: its status (running, done, failed, timed_out, cancelled), phases with how many workers each has running, done and failed, each worker's thread (session_id, link_id) and state, the last log lines, and the final structured result. `wait_ms` waits up to that long for the run to end and answers sooner when it does; without it the answer is the run's state now. Without `run_id` it lists this thread's runs.",
    arguments: r#"{"run_id": "wf-19b2c3d4e5f-0a1b2c3d", "wait_ms": 600000}"#,
    handler: workflow_unavailable,
    requires_confirmation: false,
    schema: crate::workflow::status_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

/// Stop a workflow run (#3394): no further agent() call starts and its
/// running workers are interrupted.
pub const STOP_WORKFLOW: Tool = Tool {
    name: crate::workflow::STOP_TOOL_NAME,
    description: "Stop a workflow run started with run_workflow: its script ends, no further agent() call starts, and the worker threads still running are interrupted. The run becomes cancelled and keeps the calls it had finished. Answers with the run's state.",
    arguments: r#"{"run_id": "wf-19b2c3d4e5f-0a1b2c3d"}"#,
    handler: workflow_unavailable,
    requires_confirmation: false,
    schema: crate::workflow::stop_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

fn workflow_unavailable(_args: &Value) -> Result<String, String> {
    Err(
        "error: workflows are run by the rustcode MCP server; this thread does not have it"
            .to_owned(),
    )
}

fn ask_user_question_unavailable(_args: &Value) -> Result<String, String> {
    Err(
        "error: ask_user_question is answered by the rustcode MCP server; use ask_question here"
            .to_owned(),
    )
}

fn smart_ui_check_unavailable(_args: &Value) -> Result<String, String> {
    Err(
        "error: smart_ui_check is answered by the rustcode MCP server; this thread does not have it"
            .to_owned(),
    )
}

fn localhost_preview_unavailable(_args: &Value) -> Result<String, String> {
    Err(crate::preview::UNAVAILABLE_MESSAGE.to_owned())
}

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
    arguments: r#"{"title": "short task title (1-32 characters)"}"#,
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
    crate::sessions::save_session_title(&session_id, title);
    Ok("Session title saved.".to_string())
}

const MAX_MCP_DISCOVERY_DESCRIPTION_CHARS: usize = 120;
const MAX_MCP_BOUND_DESCRIPTION_CHARS: usize = 300;

fn list_mcp_tools_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "server": {"type":"string", "minLength":1, "description":"Server name from the MCP Servers list; omit for all"},
            "query": {"type":"string", "minLength":1, "description":"Words describing the capability"},
            "select": {"type":"array", "items":{"type":"string", "minLength":1}, "minItems":1, "maxItems":super::mcp_discovery::MAX_SELECT_NAMES, "description":"Exact names: tool, server.tool or mcp__server__tool"},
            "limit": {"type":"integer", "minimum":1, "maximum":super::mcp_discovery::MAX_DISCOVERY_RESULTS, "default":super::mcp_discovery::DEFAULT_DISCOVERY_RESULTS}
        },
        "additionalProperties": false
    })
}

pub const LIST_MCP_TOOLS: Tool = Tool {
    name: "list_mcp_tools",
    description: "MCP tools only, not built-ins, agents or shell. Take server names from the MCP Servers prompt list; never guess them. `select` returns exact schemas; `query` ranks matches. Listed schemas bind next request.",
    arguments: r#"{"select":["server.tool"], "query":"capability words", "server":"name from the MCP Servers list", "limit":20}"#,
    handler: list_mcp_tools,
    requires_confirmation: false,
    schema: list_mcp_tools_schema,
    capabilities: &[ToolCapability::SessionState],
    safety: ToolSafety::ReadOnly,
};

fn manage_mcp_servers_schema() -> Value {
    let strings = |description: &str| serde_json::json!({"type":"array","items":{"type":"string"},"description":description});
    serde_json::json!({
        "type": "object",
        "properties": {
            "operation": {"type":"string","enum":["add","list","start","remove"]},
            "name": {"type":"string"},
            "target": strings("URL, or command then arguments"),
            "transport": {"type":"string","enum":["stdio","http"]},
            "headers": strings("`Name: value`, remote only"),
            "env": strings("`KEY=value`, stdio only"),
            "client_id": {"type":"string"},
            "always_include": {"type":"boolean"},
            "replace": {"type":"boolean"},
            "start": {"type":"boolean"}
        },
        "required": ["operation"],
        "additionalProperties": false
    })
}

pub const MANAGE_MCP_SERVERS: Tool = Tool {
    name: "manage_mcp_servers",
    description: "Add, list, start or remove MCP servers in the user config, validated like `rc mcp add`. `add` also starts the server in this session, so its tools are callable at once. Never edit config.toml or pipe JSON-RPC from a shell instead. Secret values are stored, never shown.",
    arguments: r#"{"operation":"add|list|start|remove", "name":"server name (add, start, remove)", "target":["URL, or command then its arguments"], "transport":"stdio|http", "headers":["Name: value"], "env":["KEY=value"], "client_id":"OAuth client id", "always_include":false, "replace":false, "start":true}"#,
    handler: manage_mcp_servers,
    // Every operation prompts: `add` and `start` launch a command or open a
    // connection of the model's choosing, and `add`/`remove` edit the user config.
    requires_confirmation: true,
    schema: manage_mcp_servers_schema,
    capabilities: &[
        ToolCapability::ExecuteCommands,
        ToolCapability::Network,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::ControlPlane,
};

fn subagent_id_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "anyOf": [
                    {"type": "integer", "minimum": 1},
                    {"type": "string", "pattern": "^[0-9]*[1-9][0-9]*$"}
                ],
                "description": "Agent id"
            }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn async_agent_tool(_args: &Value) -> Result<String, String> {
    Err("agent lifecycle tools require the asynchronous session executor".to_owned())
}

fn wait_agent_schema() -> Value {
    let mut schema = subagent_id_schema();
    schema["properties"]["timeout_ms"] =
        serde_json::json!({"type":"integer", "minimum":1, "maximum":3600000, "default":60000});
    // Either one `id` or several `ids`; the handler requires one of them.
    schema["properties"]["ids"] = serde_json::json!({
        "type": "array",
        "minItems": 1,
        "items": {"type": "integer", "minimum": 1}
    });
    schema["required"] = serde_json::json!([]);
    schema
}

pub const WAIT_AGENT: Tool = Tool {
    name: "wait_agent",
    description: "Block until a subagent finishes and return its bounded result; never poll. With ids, returns the first to finish. Waits at least 30s.",
    arguments: r#"{"id": "subagent id", "ids": [1, 2]}"#,
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: wait_agent_schema,
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};

fn agent_message_schema(minimum_id: u32) -> Value {
    let mut schema = subagent_id_schema();
    let string_id_pattern = if minimum_id == 0 {
        "^[0-9]+$"
    } else {
        "^[0-9]*[1-9][0-9]*$"
    };
    schema["properties"]["id"] = serde_json::json!({
        "anyOf": [
            {"type":"integer", "minimum":minimum_id},
            {"type":"string", "pattern":string_id_pattern}
        ],
        "description": if minimum_id == 0 {
            "Agent id; a child may use 0 for main"
        } else {
            "Agent id"
        }
    });
    schema["properties"]["message"] =
        serde_json::json!({"type":"string", "minLength":1, "maxLength":8192});
    schema["required"] = serde_json::json!(["id", "message"]);
    schema
}

fn list_agents_schema() -> Value {
    serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false})
}

pub const SEND_MESSAGE: Tool = Tool {
    name: "send_message",
    description: "Queue a message for an agent, delivered at its next safe request boundary; an idle agent stays idle until a follow-up.",
    arguments: r#"{"id":"subagent id","message":"new evidence"}"#,
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: || agent_message_schema(0),
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};
/// `followup_task`: a message, plus the choice to stop the child's current
/// turn for it instead of queueing behind it.
fn followup_task_schema() -> Value {
    let mut schema = agent_message_schema(1);
    schema["properties"]["interrupt"] = interrupt_schema();
    schema
}

/// Kept bare: the tool description says what it does, and the built-in
/// schema block is sent with every request.
pub(super) fn interrupt_schema() -> Value {
    serde_json::json!({"type": "boolean"})
}

pub const FOLLOWUP_TASK: Tool = Tool {
    name: "followup_task",
    description: "Start a follow-up on an idle, completed or interrupted child, or queue for an active one; interrupt redirects it now.",
    arguments: r#"{"id":"subagent id","message":"next task","interrupt":false}"#,
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: followup_task_schema,
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};
pub const LIST_AGENTS: Tool = Tool {
    name: "list_agents",
    description: "List session agents: id, parent, status, model, context strategy and elapsed time. Reads no transcripts.",
    arguments: "{}",
    handler: async_agent_tool,
    requires_confirmation: false,
    schema: list_agents_schema,
    capabilities: &[
        ToolCapability::AgentDelegation,
        ToolCapability::SessionState,
    ],
    safety: ToolSafety::Delegation,
};
pub const INSPECT_AGENT: Tool = Tool {
    name: "inspect_agent",
    description: "Inspect an agent and a bounded tail of its transcript.",
    arguments: r#"{"id":"subagent id"}"#,
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
    description: "Search the web for documentation, API details or code patterns.",
    arguments: r#"{"query": "search query terms", "domain": "optional domain filter e.g. 'docs.rs'"}"#,
    handler: search_web,
    requires_confirmation: false,
    schema: search_web_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::ReadOnly,
};

fn read_url_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "url": { "type": "string" },
            "offset": { "type": "integer", "minimum": 0 },
            "max_chars": { "type": "integer", "minimum": 500, "maximum": 20000 }
        }, "required": ["url"]
    })
}

pub const READ_URL: Tool = Tool {
    name: "read_url",
    description: "Fetch one http(s) page and return its readable text. Use it for a link the user gave or a search result worth reading in full.",
    arguments: r#"{"url": "https://example.com/page", "offset": "optional character offset to continue a long page", "max_chars": "optional, default 8000, max 20000"}"#,
    handler: read_url,
    requires_confirmation: false,
    schema: read_url_schema,
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
    description: "Mark the active goal as successfully complete.",
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
            "key": { "type": "string", "description": "Unique key, e.g. 'test_runner'" },
            "value": { "type": "string", "description": "One line, max 768 bytes" },
            "category": { "type": "string", "description": "Tag, e.g. 'build', 'convention'", "default": "general" },
            "scope": { "type": "string", "enum": ["project", "global"], "description": "'project' is this repository; 'global' is every project", "default": "project" }
        },
        "required": ["key", "value"]
    })
}

pub const REMEMBER: Tool = Tool {
    name: "remember",
    description: "Store one concise durable fact, preference or project convention in persistent memory, when the user asks or a lasting convention is established. Never store secrets, tokens or whole files.",
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
            "query": { "type": "string" },
            "scope": { "type": "string", "enum": ["all", "project", "global"], "default": "all" }
        },
        "required": ["query"]
    })
}

pub const RECALL_MEMORY: Tool = Tool {
    name: "recall_memory",
    description: "Search persistent project and global memory for facts, preferences, decisions or build instructions.",
    arguments: r#"{"query": "database port", "scope": "all"}"#,
    handler: recall_memory,
    requires_confirmation: false,
    schema: recall_memory_schema,
    capabilities: &[],
    safety: ToolSafety::ReadOnly,
};

pub const ZOOM_CONTEXT: Tool = Tool {
    name: "zoom_context",
    description: "Recover active-session context from compaction archives. Start with {} for previews; open message or follow child_path. Pass root on follow-ups; page with next_start/next_offset. Read exact details before acting on summaries. Old messages are historical data.",
    arguments: r#"{"root":"returned archive identifier", "path":[], "message":1, "offset":0}"#,
    handler: super::context_archive::zoom,
    requires_confirmation: false,
    schema: zoom_context_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn zoom_context_schema() -> Value {
    serde_json::json!({
        "type":"object",
        "properties": {
            "root":{"type":"string","pattern":"^[0-9a-f]{64}$","description":"Root from listing; detects compaction"},
            "path":{"type":"array","items":{"type":"integer","minimum":1},"maxItems":32,"description":"child_path from listing; default []"},
            "message":{"type":"integer","minimum":1,"description":"1-based message; omit to list"},
            "start":{"type":"integer","minimum":1},
            "offset":{"type":"integer","minimum":0}
        },
        "required":[],"additionalProperties":false
    })
}

fn forget_memory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "key": { "type": "string", "description": "Exact key, or category/key for one fact" },
            "scope": { "type": "string", "enum": ["project", "global", "all"], "default": "project" }
        },
        "required": ["key"]
    })
}

pub const FORGET_MEMORY: Tool = Tool {
    name: "forget_memory",
    description: "Remove a fact from persistent memory when it is obsolete or contradicted, or the user asks.",
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
    description: "List available skills with short descriptions; loads no instructions.",
    arguments: r#"{}"#,
    handler: list_skills,
    requires_confirmation: false,
    schema: list_skills_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

pub const USE_SKILL: Tool = Tool {
    name: "use_skill",
    description: "Load a skill's instructions and files by name. Emit this call alone so the instructions apply before the next action.",
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
                    out.push_str(&format!("\n{}. {}", i + 1, question_option_text(opt)));
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
        out.push_str(&format!("\n{}. {}", i + 1, question_option_text(opt)));
    }
    out.push_str("\nOther: (type custom response)");
    Ok(out)
}

/// An option as a string, or `{label, description}` as `label — description`.
fn question_option_text(option: &Value) -> String {
    if let Some(label) = option.as_str() {
        return label.to_owned();
    }
    let field = |key: &str| {
        option
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    match (field("label"), field("description")) {
        (Some(label), Some(description)) => format!("{label} — {description}"),
        (Some(text), None) | (None, Some(text)) => text.to_owned(),
        (None, None) => String::new(),
    }
}

pub fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
}

pub fn list_mcp_tools(args: &Value) -> Result<String, String> {
    let args = super::mcp_discovery::DiscoveryArgs::parse(args)?;
    start_named_mcp_servers(&args);
    let mut clients = {
        let registry_handle = crate::mcp::get_mcp_registry();
        let registry = registry_handle
            .lock()
            .map_err(|error| format!("MCP registry unavailable: {error}"))?;
        registry.values().cloned().collect::<Vec<_>>()
    };
    clients.sort_by(|a, b| a.name.cmp(&b.name));
    // The same collection the next request's schema selection reads, so the
    // tools this call binds are the ones that request carries.
    let tools = super::schema::collect_mcp_tools_with_servers()
        .into_iter()
        .map(
            |(advertised, server, name, description, input_schema)| McpDiscoveryTool {
                callable_name: super::schema::mcp_canonical_name_for_clients(
                    &server, &name, &clients,
                ),
                server,
                name,
                advertised,
                description,
                input_schema,
            },
        )
        .collect::<Vec<_>>();
    let statuses = super::mcp_discovery::current_server_statuses();
    format_mcp_discovery(&tools, &statuses, &args)
}

/// Start the enabled, configured servers a discovery call names that are not
/// running: its `server`, or the server part of a `select` name. They are
/// in the user's config, so starting one is what a session start would have
/// done; listing them as stopped only sent the model to another tool that
/// asks for approval. A failure stays visible in the catalogue and results.
fn start_named_mcp_servers(args: &super::mcp_discovery::DiscoveryArgs) {
    use super::mcp_discovery::McpServerState;
    let stopped = super::mcp_discovery::current_server_statuses()
        .into_iter()
        .filter(|status| {
            matches!(
                status.state,
                McpServerState::NotRunning | McpServerState::Failed(_)
            )
        })
        .map(|status| status.name)
        .collect::<Vec<_>>();
    if stopped.is_empty() {
        return;
    }
    let workspace = mcp_workspace(None);
    let configured = crate::config::get_config_dir()
        .map(|dir| crate::config::workspace_mcp_servers_in(&dir, &workspace))
        .unwrap_or_default();
    for server in configured.iter().filter(|server| {
        server.enabled
            && stopped.contains(&server.name)
            && discovery_names_server(args, &server.name)
    }) {
        if let Err(error) = start_mcp_server_in_session(server, &workspace) {
            crate::mcp::record_mcp_startup_failure(&server.name, &error);
        }
    }
}

/// Whether a discovery call names `server`: as its `server`, or as the
/// server part of a `server.tool` or `mcp__server__tool` select name.
fn discovery_names_server(args: &super::mcp_discovery::DiscoveryArgs, server: &str) -> bool {
    let canonical = format!("mcp__{}__", server.replace('-', "_"));
    args.server.as_deref() == Some(server)
        || args.select.iter().any(|name| {
            name.strip_prefix(server)
                .is_some_and(|rest| rest.starts_with('.'))
                || name.starts_with(&canonical)
        })
}

#[derive(Clone)]
struct McpDiscoveryTool {
    server: String,
    /// The server's own tool name.
    name: String,
    /// The name the provider request advertises (bare when unique).
    advertised: String,
    /// Always the `mcp__server__tool` form; dispatch also accepts the bare name.
    callable_name: String,
    input_schema: Value,
    description: String,
}

fn format_mcp_discovery(
    tools: &[McpDiscoveryTool],
    statuses: &[super::mcp_discovery::McpServerStatus],
    args: &super::mcp_discovery::DiscoveryArgs,
) -> Result<String, String> {
    use super::mcp_discovery::{
        DISCOVERY_OUTPUT_BYTES, MAX_LISTED_SCHEMA_BYTES, McpServerState, McpToolDoc,
    };
    let docs = tools
        .iter()
        .map(|tool| McpToolDoc {
            server: &tool.server,
            raw_name: &tool.name,
            advertised: &tool.advertised,
            description: &tool.description,
        })
        .collect::<Vec<_>>();
    let running = statuses
        .iter()
        .filter(|status| matches!(status.state, McpServerState::Running { .. }))
        .map(|status| status.name.as_str())
        .collect::<Vec<_>>();
    let outcome = super::mcp_discovery::discover(&docs, &running, args);
    let serialize = |value: Value| {
        serde_json::to_string(&value)
            .map_err(|error| format!("failed to serialize MCP discovery: {error}"))
    };
    if outcome.summary {
        return serialize(mcp_server_summaries(statuses));
    }

    let bound = outcome.bound(args.limit);
    let mut rows = Vec::new();
    let mut used = 0usize;
    let mut unbound_listed = false;
    let mut truncated = false;
    for index in outcome
        .selected
        .iter()
        .chain(outcome.listed(args.limit))
        .copied()
    {
        let tool = &tools[index];
        let is_bound = bound.contains(&index);
        let mut row = serde_json::json!({
            "server": tool.server,
            "name": tool.name,
            "callable_name": tool.callable_name,
            "description": truncate_mcp_description(
                &tool.description,
                if is_bound {
                    MAX_MCP_BOUND_DESCRIPTION_CHARS
                } else {
                    MAX_MCP_DISCOVERY_DESCRIPTION_CHARS
                },
            ),
        });
        let row_bytes = row.to_string().len();
        let selected = outcome.selected.contains(&index);
        if !selected && used.saturating_add(row_bytes) > DISCOVERY_OUTPUT_BYTES {
            truncated = true;
            break;
        }
        used = used.saturating_add(row_bytes);
        if is_bound {
            let schema_bytes = tool.input_schema.to_string().len();
            if schema_bytes <= MAX_LISTED_SCHEMA_BYTES
                && (selected || used.saturating_add(schema_bytes) <= DISCOVERY_OUTPUT_BYTES)
            {
                row["input_schema"] = tool.input_schema.clone();
                used = used.saturating_add(schema_bytes);
            } else {
                row["input_schema_omitted"] =
                    Value::String("too large to list; bound in your next tool list".to_owned());
            }
        } else {
            unbound_listed = true;
        }
        rows.push(row);
    }
    let total = outcome.selected.len() + outcome.matches.len();
    truncated |= rows.len() < total;
    let mut result = serde_json::json!({
        "results": rows,
        "total": total,
        "truncated": truncated,
    });

    if !outcome.unresolved.is_empty() {
        result["not_found"] = Value::Array(
            outcome
                .unresolved
                .iter()
                .map(|(name, suggestions)| {
                    serde_json::json!({
                        "name": name,
                        "suggestions": suggestions
                            .iter()
                            .map(|index| tools[*index].callable_name.clone())
                            .collect::<Vec<_>>(),
                    })
                })
                .collect(),
        );
    }

    let mut notes = Vec::new();
    if let Some(server) = outcome.unknown_server.as_deref() {
        notes.push(unknown_mcp_server_note(server, statuses, args));
        result["valid_servers"] = Value::Array(
            statuses
                .iter()
                .map(|status| Value::String(format!("{}: {}", status.name, status.label())))
                .collect(),
        );
    }
    let stopped = statuses
        .iter()
        .filter(|status| {
            matches!(
                status.state,
                McpServerState::NotRunning | McpServerState::Failed(_)
            )
        })
        .map(|status| status.name.clone())
        .collect::<Vec<_>>();
    if !stopped.is_empty() && outcome.unknown_server.is_none() {
        result["not_running"] = serde_json::json!(stopped);
        notes.push("Servers in not_running are configured but not started, so their tools are not listed. Name a relevant one in `server` and this call starts it.".to_owned());
    }
    if unbound_listed {
        notes.push("Results without input_schema are not bound; call list_mcp_tools with select:[callable_name] for the ones you need.".to_owned());
    }
    if total == 0 && outcome.unresolved.is_empty() {
        notes.push("No MCP tool matched. Use fewer, more specific words, or select a tool by name from the MCP Servers list.".to_owned());
    }
    if !notes.is_empty() {
        result["hint"] = Value::String(notes.join(" "));
    }
    serialize(result)
}

/// Explain a `server` that is not a running server, and that every server
/// was searched instead.
fn unknown_mcp_server_note(
    server: &str,
    statuses: &[super::mcp_discovery::McpServerStatus],
    args: &super::mcp_discovery::DiscoveryArgs,
) -> String {
    use super::mcp_discovery::McpServerState;
    let mut note = match statuses.iter().find(|status| status.name == server) {
        Some(status) => match &status.state {
            McpServerState::Disabled => format!(
                "MCP server '{server}' is disabled in the config; the user can enable it in /mcp."
            ),
            McpServerState::Failed(reason) => {
                format!("MCP server '{server}' failed to start: {reason}.")
            }
            _ => format!("MCP server '{server}' is configured but not running."),
        },
        None => {
            let names = statuses
                .iter()
                .map(|status| status.name.as_str())
                .collect::<Vec<_>>();
            match super::mcp_discovery::closest_server(server, &names) {
                Some(closest) => {
                    format!("Unknown MCP server '{server}'; the closest is '{closest}'.")
                }
                None => format!("Unknown MCP server '{server}'."),
            }
        }
    };
    if args.select.is_empty() {
        let searched = args.query.as_deref().unwrap_or(server);
        note.push_str(&format!(
            " Searched every running server for '{searched}' instead; valid_servers lists the server names."
        ));
    } else {
        note.push_str(" valid_servers lists the server names.");
    }
    note
}

/// The no-argument listing: every configured server with its state and tool
/// names, bounded so a large catalogue still fits one result.
fn mcp_server_summaries(statuses: &[super::mcp_discovery::McpServerStatus]) -> Value {
    use super::mcp_discovery::{DISCOVERY_OUTPUT_BYTES, McpServerState};
    let running = statuses
        .iter()
        .filter(|status| matches!(status.state, McpServerState::Running { .. }))
        .count()
        .max(1);
    let per_server = DISCOVERY_OUTPUT_BYTES / running;
    let servers = statuses
        .iter()
        .map(|status| {
            let mut summary = serde_json::json!({
                "name": status.name,
                "status": status.label(),
            });
            if let McpServerState::Running { tools } = &status.state {
                summary["tool_count"] = serde_json::json!(tools.len());
                let mut names = Vec::new();
                let mut bytes = 0usize;
                for name in tools {
                    if bytes.saturating_add(name.len() + 3) > per_server {
                        names.push(format!("+{} more", tools.len() - names.len()));
                        break;
                    }
                    bytes = bytes.saturating_add(name.len() + 3);
                    names.push(name.clone());
                }
                summary["tools"] = serde_json::json!(names);
            }
            summary
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "servers": servers,
        "hint": "Call list_mcp_tools with select:[\"server.tool\"] for exact schemas, or query:\"capability words\" to search.",
    })
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

fn truncate_mcp_description(description: &str, max_chars: usize) -> String {
    let description = description.trim();
    let mut chars = description.chars();
    let short = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// A cold `npx`-style launch downloads its package first, so a server the
/// model just registered gets longer than the launch-time startup budget.
const MCP_SESSION_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Shorter values are too likely to be ordinary words to blank out of an error.
const MIN_REDACTED_SECRET_CHARS: usize = 6;
const MCP_SECRET_PLACEHOLDER: &str = "[REDACTED]";

fn manage_mcp_servers(args: &Value) -> Result<String, String> {
    let dir = crate::config::get_config_dir().ok_or("config directory unavailable")?;
    manage_mcp_servers_in(args, &dir, &mcp_workspace(None))
}

pub(super) fn mcp_workspace(workspace: Option<&std::path::Path>) -> std::path::PathBuf {
    workspace
        .map(std::path::Path::to_path_buf)
        .or_else(super::active_workspace_root)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub(crate) fn manage_mcp_servers_in(
    args: &Value,
    dir: &std::path::Path,
    workspace: &std::path::Path,
) -> Result<String, String> {
    let operation = args
        .get("operation")
        .and_then(Value::as_str)
        .ok_or("missing 'operation'")?;
    let server_name = || {
        optional_nonempty_string(args, "name")?
            .ok_or_else(|| format!("'{operation}' needs the server 'name'"))
    };
    let active = |name: &str| {
        crate::config::workspace_mcp_servers_in(dir, workspace)
            .into_iter()
            .find(|server| server.name == name)
    };
    match operation {
        "list" => {
            let servers = crate::config::workspace_mcp_servers_in(dir, workspace);
            if servers.is_empty() {
                return Ok("No MCP servers configured.".to_owned());
            }
            let running = running_mcp_servers();
            Ok(servers
                .iter()
                .map(|server| {
                    let state = if running.contains(&server.name) {
                        "running"
                    } else {
                        "not running"
                    };
                    format!("{}  [{state}]", crate::config::mcp_server_summary(server))
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "add" => {
            let server = crate::config::mcp_server_from_spec(&mcp_server_spec(args)?)?;
            let replace = mcp_flag(args, "replace", false)?;
            let start = mcp_flag(args, "start", true)?;
            let replaced = crate::config::add_mcp_server_in(dir, server.clone(), replace)
                .map_err(|error| error.replace("pass --force", "set \"replace\": true"))?;
            let saved = format!(
                "{} {} in the user config.",
                if replaced { "Replaced" } else { "Added" },
                crate::config::mcp_server_summary(&server)
            );
            if active(&server.name).as_ref() != Some(&server) {
                return Ok(format!(
                    "{saved} A project .rustcode/config.toml sets its own mcp_servers, so '{}' is not active in this workspace and was not started.",
                    server.name
                ));
            }
            if !start {
                return Ok(format!("{saved} Not started."));
            }
            match start_mcp_server_in_session(&server, workspace) {
                Ok(started) => Ok(format!("{saved} {started}")),
                Err(error) => Err(format!(
                    "{saved} It failed to start: {error}. Fix the cause, then call operation \"start\", or \"add\" again with \"replace\": true."
                )),
            }
        }
        "start" => {
            let name = server_name()?;
            let server = active(name)
                .ok_or_else(|| format!("no MCP server named '{name}' is configured"))?;
            if !server.enabled {
                return Err(format!(
                    "MCP server '{name}' is disabled; the user can enable it in /mcp"
                ));
            }
            start_mcp_server_in_session(&server, workspace)
        }
        "remove" => {
            let name = server_name()?;
            crate::config::remove_mcp_server_in(dir, name)?;
            // A project file may still declare a server of this name; that one
            // stays configured, so it keeps running.
            if active(name).is_some() {
                return Ok(format!(
                    "Removed '{name}' from the user config. A project .rustcode/config.toml still declares it, so it was left running."
                ));
            }
            let stopped = running_mcp_servers().iter().any(|running| running == name);
            if stopped {
                mcp_runtime()?.block_on(crate::mcp::shutdown_server(name));
            }
            Ok(format!(
                "Removed '{name}' from the user config{}.",
                if stopped { " and stopped it" } else { "" }
            ))
        }
        _ => Err(format!("unknown operation '{operation}'")),
    }
}

fn mcp_server_spec(args: &Value) -> Result<crate::config::McpServerSpec, String> {
    let strings = |key: &str| -> Result<Vec<String>, String> {
        let Some(value) = args.get(key) else {
            return Ok(Vec::new());
        };
        value
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .ok_or_else(|| format!("'{key}' must be an array of strings"))
    };
    Ok(crate::config::McpServerSpec {
        name: optional_nonempty_string(args, "name")?
            .ok_or("'add' needs the server 'name'")?
            .to_owned(),
        target: strings("target")?,
        transport: match args.get("transport").map(|value| value.as_str()) {
            None => None,
            Some(Some("stdio")) => Some(crate::config::McpTransport::Stdio),
            Some(Some("http")) => Some(crate::config::McpTransport::Http),
            Some(_) => return Err("'transport' must be \"stdio\" or \"http\"".to_owned()),
        },
        headers: strings("headers")?,
        env: strings("env")?,
        client_id: optional_nonempty_string(args, "client_id")?.map(str::to_owned),
        always_include: mcp_flag(args, "always_include", false)?,
    })
}

fn mcp_flag(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    args.get(key).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| format!("'{key}' must be true or false"))
    })
}

/// The session runtime the tool call was dispatched from. Server tasks have
/// to outlive this call, so a private runtime would not do.
fn mcp_runtime() -> Result<tokio::runtime::Handle, String> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| "no session runtime is available to manage MCP servers".to_owned())
}

fn running_mcp_servers() -> Vec<String> {
    crate::mcp::get_mcp_registry()
        .lock()
        .map(|registry| registry.keys().cloned().collect())
        .unwrap_or_default()
}

/// Start `server` in the running session and report the tools it added. The
/// registry is only locked for the snapshot afterwards, never while the
/// server spawns or connects.
fn start_mcp_server_in_session(
    server: &crate::config::McpServerConfig,
    workspace: &std::path::Path,
) -> Result<String, String> {
    let name = &server.name;
    let started = mcp_runtime()?.block_on(async {
        tokio::time::timeout(
            MCP_SESSION_START_TIMEOUT,
            crate::mcp::start_configured_server(server.clone(), workspace),
        )
        .await
    });
    match started {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(without_mcp_secrets(&error, server)),
        Err(_) => {
            return Err(format!(
                "MCP server '{name}' did not finish starting within {}s",
                MCP_SESSION_START_TIMEOUT.as_secs()
            ));
        }
    }
    let mut clients = crate::mcp::get_mcp_registry()
        .lock()
        .map(|registry| registry.values().cloned().collect::<Vec<_>>())
        .map_err(|error| format!("MCP registry unavailable: {error}"))?;
    clients.sort_by(|a, b| a.name.cmp(&b.name));
    let tools = clients
        .iter()
        .find(|client| &client.name == name)
        .and_then(|client| client.get_tools().ok())
        .unwrap_or_default()
        .iter()
        .filter(|tool| crate::mcp::tool_visible_to(tool, "model"))
        .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
        .map(|tool| super::schema::mcp_canonical_name_for_clients(name, &tool, &clients))
        .collect::<Vec<_>>();
    Ok(if tools.is_empty() {
        format!("Started '{name}' in this session; it lists no tools.")
    } else {
        format!(
            "Started '{name}' in this session. Callable now: {}.",
            tools.join(", ")
        )
    })
}

/// Blank out the server's header and environment values wherever a transport
/// or process error quotes them back.
fn without_mcp_secrets(text: &str, server: &crate::config::McpServerConfig) -> String {
    server
        .headers
        .values()
        .chain(server.env.values())
        .filter(|value| value.chars().count() >= MIN_REDACTED_SECRET_CHARS)
        .fold(text.to_owned(), |text, value| {
            text.replace(value.as_str(), MCP_SECRET_PLACEHOLDER)
        })
}

/// What an approval prompt shows for one `manage_mcp_servers` call.
pub(crate) struct McpServersApproval {
    /// The operation and the server it acts on, with secret values left out.
    pub(crate) label: String,
    pub(crate) preview: String,
    /// The call's arguments with header and environment values blanked.
    pub(crate) arguments: Value,
}

/// Describe a `manage_mcp_servers` call for the user who has to approve it:
/// the command or URL it would launch, and no header or environment values.
/// Returns `None` for every other tool.
pub(crate) fn mcp_servers_approval(
    name: &str,
    args: &Value,
    workspace: Option<&std::path::Path>,
) -> Option<McpServersApproval> {
    if name != MANAGE_MCP_SERVERS.name {
        return None;
    }
    let operation = args.get("operation").and_then(Value::as_str).unwrap_or("?");
    let server = args.get("name").and_then(Value::as_str).unwrap_or("");
    let configured = || {
        let dir = crate::config::get_config_dir()?;
        crate::config::workspace_mcp_servers_in(&dir, &mcp_workspace(workspace))
            .into_iter()
            .find(|entry| entry.name == server)
    };
    let summary = match operation {
        "add" => mcp_server_spec(args)
            .and_then(|spec| crate::config::mcp_server_from_spec(&spec))
            .ok(),
        "start" | "remove" => configured(),
        _ => None,
    }
    .map(|entry| crate::config::mcp_server_summary(&entry))
    .unwrap_or_else(|| server.to_owned());
    let preview = match operation {
        "add" if args.get("start").and_then(Value::as_bool) == Some(false) => {
            "Saves this server to the user config. Header and environment values are hidden."
        }
        "add" => {
            "Saves this server to the user config and starts it in this session.\nHeader and environment values are hidden."
        }
        "start" => "Starts this configured server in this session.",
        "remove" => "Removes this server from the user config and stops it.",
        _ => "",
    };
    Some(McpServersApproval {
        label: format!("{operation} {summary}").trim().to_owned(),
        preview: preview.to_owned(),
        arguments: mcp_arguments_without_secrets(args),
    })
}

fn mcp_arguments_without_secrets(args: &Value) -> Value {
    let mut redacted = args.clone();
    for (key, separator, shown) in [("headers", ':', ": "), ("env", '=', "=")] {
        let Some(entries) = redacted.get_mut(key).and_then(Value::as_array_mut) else {
            continue;
        };
        for entry in entries {
            let name = entry
                .as_str()
                .and_then(|raw| raw.split_once(separator))
                .map(|(name, _)| format!("{}{shown}", name.trim()))
                .unwrap_or_default();
            *entry = Value::String(format!("{name}{MCP_SECRET_PLACEHOLDER}"));
        }
    }
    redacted
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

pub fn use_skill(args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("missing 'name' argument")?;

    let skill = crate::skills::get_skill_content(name);
    crate::skills::bump_skill_catalog_generation();
    let skill = skill.ok_or_else(|| {
        if crate::skills::disabled_skill_names().contains(name) {
            format!(
                "Skill '{name}' is turned off. The user can enable it with /skills or in Settings."
            )
        } else {
            format!(
                "Skill '{}' not found. Call list_skills to discover available skills.",
                name
            )
        }
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
        crate::memory::upsert(crate::memory::tool_scope_root().as_deref(), fact)?;
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

    let facts =
        crate::memory::search_facts(crate::memory::tool_scope_root().as_deref(), query, scope);
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
        if let Ok(removed) = crate::memory::remove(crate::memory::tool_scope_root().as_deref(), key)
        {
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
mod tests;

fn browser_open_schema() -> Value {
    crate::computer_use::tools(&crate::computer_use::Settings {
        enabled: true,
        ..Default::default()
    })
    .into_iter()
    .find(|tool| tool["name"] == "browser_open")
    .unwrap()["inputSchema"]
        .clone()
}
pub const BROWSER_OPEN: Tool = Tool {
    name: "browser_open",
    description: "Use RustCode's isolated browser; requires computer_use.enabled and approval. See docs/computer-use.md.",
    arguments: "{}",
    handler: browser_unavailable,
    requires_confirmation: true,
    schema: browser_open_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Interactive,
};

fn browser_read_schema() -> Value {
    crate::computer_use::tools(&crate::computer_use::Settings {
        enabled: true,
        ..Default::default()
    })
    .into_iter()
    .find(|tool| tool["name"] == "browser_read")
    .unwrap()["inputSchema"]
        .clone()
}
pub const BROWSER_READ: Tool = Tool {
    name: "browser_read",
    description: "Use RustCode's isolated browser; requires computer_use.enabled and approval. See docs/computer-use.md.",
    arguments: "{}",
    handler: browser_unavailable,
    requires_confirmation: true,
    schema: browser_read_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Interactive,
};

fn browser_screenshot_schema() -> Value {
    crate::computer_use::tools(&crate::computer_use::Settings {
        enabled: true,
        ..Default::default()
    })
    .into_iter()
    .find(|tool| tool["name"] == "browser_screenshot")
    .unwrap()["inputSchema"]
        .clone()
}
pub const BROWSER_SCREENSHOT: Tool = Tool {
    name: "browser_screenshot",
    description: "Use RustCode's isolated browser; requires computer_use.enabled and approval. See docs/computer-use.md.",
    arguments: "{}",
    handler: browser_unavailable,
    requires_confirmation: true,
    schema: browser_screenshot_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Interactive,
};

fn browser_click_schema() -> Value {
    crate::computer_use::tools(&crate::computer_use::Settings {
        enabled: true,
        ..Default::default()
    })
    .into_iter()
    .find(|tool| tool["name"] == "browser_click")
    .unwrap()["inputSchema"]
        .clone()
}
pub const BROWSER_CLICK: Tool = Tool {
    name: "browser_click",
    description: "Use RustCode's isolated browser; requires computer_use.enabled and approval. See docs/computer-use.md.",
    arguments: "{}",
    handler: browser_unavailable,
    requires_confirmation: true,
    schema: browser_click_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Interactive,
};

fn browser_type_schema() -> Value {
    crate::computer_use::tools(&crate::computer_use::Settings {
        enabled: true,
        ..Default::default()
    })
    .into_iter()
    .find(|tool| tool["name"] == "browser_type")
    .unwrap()["inputSchema"]
        .clone()
}
pub const BROWSER_TYPE: Tool = Tool {
    name: "browser_type",
    description: "Use RustCode's isolated browser; requires computer_use.enabled and approval. See docs/computer-use.md.",
    arguments: "{}",
    handler: browser_unavailable,
    requires_confirmation: true,
    schema: browser_type_schema,
    capabilities: &[ToolCapability::Network],
    safety: ToolSafety::Interactive,
};

fn browser_unavailable(_: &Value) -> Result<String, String> {
    Err("Browser runtime unavailable".into())
}

pub const MANAGE_SETTINGS: Tool = Tool {
    name: crate::manage::MANAGE_SETTINGS,
    description: "Read and change RustCode's own settings (settings.json): list them with values, defaults, types and accepted values; get one; set or reset one for this machine or a project. Reads answer at once; set and reset show the user the value before and after, and change nothing unless they allow it. Only change a setting when the user asks; never hand-edit it.",
    arguments: r#"{"action":"list|get|set|reset","key":"optional auto-compact-threshold","value":"optional 200000","project":"optional absolute folder"}"#,
    handler: ask_user_question_unavailable,
    requires_confirmation: false,
    schema: crate::manage::settings_schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const MANAGE_CAPABILITIES: Tool = Tool {
    name: crate::manage::MANAGE_CAPABILITIES,
    description: "List and change what RustCode threads can use: turn a skill or MCP server on or off, restart an MCP server, pick the default harness (and model) for new threads, or switch the theme. Lists answer at once; every change asks the user first. Only change one when the user asks; never hand-edit config files.",
    arguments: r#"{"kind":"skill|mcp|harness|theme","action":"list|enable|disable|restart|default|set","name":"optional","model":"optional","effort":"optional"}"#,
    handler: ask_user_question_unavailable,
    requires_confirmation: false,
    schema: crate::manage::capabilities::schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};

pub const REQUEST_SECRET: Tool = Tool {
    name: crate::orchestration::secret::TOOL_NAME,
    description: "Ask for a credential privately, not in chat. Saves to an untracked dotenv destination; env=true sets $NAME for later commands. Never print it.",
    arguments: r#"{"name":"API_KEY","reason":"Deploy the service","destination":"optional .env.local","env":"optional true"}"#,
    handler: ask_user_question_unavailable,
    requires_confirmation: false,
    schema: crate::orchestration::secret::schema,
    capabilities: &[ToolCapability::UserInteraction],
    safety: ToolSafety::Interactive,
};
