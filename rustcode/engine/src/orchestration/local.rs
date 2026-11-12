//! The orchestration calls the engine answers itself, for a thread whose
//! client does not host them (#3422): a terminal or headless run, a session
//! owner started by a paired machine, or a server-only thread whose app has
//! closed. The app stays the host whenever its client is attached and another
//! call can reach it; this is the fallback behind that call, not a second
//! answer in front of it.
//!
//! Reads need no approval, so they are served here directly from the same
//! store the app reads: threads, harnesses, the paired machines on disk, the
//! link book the app saves, and the thread status files. The tools that change
//! another thread or ask the user (`start_session`, `message_session`,
//! `fan_out`, and every management tool) still need the app, and say so by
//! name instead of disappearing.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::app::ChatMessage;

/// The most threads one read returns: what the app's `list_sessions` lists.
const MAX_SESSIONS: usize = 50;
/// One `read_session` page, as the app pages it.
const READ_BYTES: usize = 16 * 1024;
/// Models listed per harness, and the most a `model` filter returns.
const LISTED_MODELS: usize = 12;
const FILTERED_MODELS: usize = 50;
/// The most delegates one `thread_status` reports.
const MAX_DELEGATES: usize = 12;
/// How often a local `wait_session` re-reads the link book.
const WAIT_CHECK: Duration = Duration::from_millis(250);
/// What `list_machines` calls this computer, as the app does.
const LOCAL_MACHINE_ID: &str = "local";

/// Answer one orchestration call without the app, as the tool result the
/// engine's built-in harness returns.
pub(crate) async fn answer(
    session_id: &str,
    cancel: &CancellationToken,
    name: &str,
    args: &Value,
) -> crate::tools::ToolExecutionOutput {
    super::output(name, answer_json(session_id, cancel, name, args).await)
}

/// Answer one orchestration call without the app, as the client's JSON shape,
/// so `orchestration::output` formats it the way it formats the app's answer.
pub(crate) async fn answer_json(
    session_id: &str,
    cancel: &CancellationToken,
    name: &str,
    args: &Value,
) -> Value {
    if !super::is_tool(name) {
        return error("invalid", format!("{name} is not an orchestration tool"));
    }
    if let Err(reason) = super::validate(name, args) {
        return error("invalid", format!("invalid {name} call: {reason}"));
    }
    // A retry key makes a call that must not happen twice safe to retry. The
    // app keeps its receipts for a week; this server keeps them for its life,
    // which is as long as the thread it serves.
    let key = args
        .get(super::CLIENT_REQUEST_ID)
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty());
    if let Some(key) = key {
        match receipts().earlier(session_id, key, name, args) {
            Earlier::Replayed(answer) => return replay(answer),
            Earlier::Conflict => {
                return error(
                    "invalid",
                    "client_request_id reused with different arguments",
                );
            }
            Earlier::None => {}
        }
    }
    let answer = match name {
        super::LIST_SESSIONS => list_sessions(session_id, args),
        super::READ_SESSION => read_session(session_id, args),
        super::LIST_HARNESSES => list_harnesses(args).await,
        super::LIST_MACHINES => list_machines(args),
        super::THREAD_STATUS => thread_status(session_id, args),
        super::WAIT_SESSION => wait_session(session_id, cancel, args).await,
        _ => app_required(name),
    };
    if let Some(key) = key
        && answer.get("error").is_none()
    {
        receipts().record(session_id, key, name, args, &answer);
    }
    answer
}

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

fn app_required(name: &str) -> Value {
    let verb = if super::KEYED_TOOL_NAMES.contains(&name) {
        "approve and run it"
    } else {
        "answer it"
    };
    error(
        "unavailable",
        format!(
            "{name} needs the RustCode app open to {verb}; this thread runs on a machine whose app is not attached. Open the RustCode app and retry. Nothing was sent."
        ),
    )
}

/// The call's `machine` names this computer, or the tool needs the app to
/// reach another one.
fn local_machine(args: &Value) -> Result<(), Value> {
    let Some(asked) = args
        .get("machine")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|asked| !asked.is_empty())
    else {
        return Ok(());
    };
    if asked.eq_ignore_ascii_case(LOCAL_MACHINE_ID) || asked.eq_ignore_ascii_case(&local_name()) {
        return Ok(());
    }
    Err(error(
        "unavailable",
        format!(
            "reading or acting on {asked} needs the RustCode app open; this engine serves only this computer's threads. Nothing was sent."
        ),
    ))
}

/// The app's "Let agents read threads in other projects" setting, which it
/// seeds into `<config>/settings.json`. On unless the machine says otherwise,
/// as the registry default is.
fn read_across_projects() -> bool {
    crate::acp::settings::value("read-across-projects")
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

fn reach_refused() -> String {
    "reading across projects is off; turn on “Let agents read threads in other projects” in Settings › Harnesses".to_owned()
}

/// Whether a read would have shown the app's approval card: a Full-access
/// thread reads without one, Auto-edit lets reads through, Ask asks first.
fn read_needs_approval(session_id: &str) -> bool {
    let full = crate::config::load_session_yolo(session_id)
        .unwrap_or_else(|| crate::config::load_config().2.yolo);
    if full {
        return false;
    }
    crate::acp::settings::value("default-permission-mode")
        .and_then(|value| value.as_str().map(str::to_owned))
        .is_none_or(|mode| mode == "ask")
}

/// `list_sessions`: the caller's project's threads, or every project's, newest
/// first and without transcripts.
fn list_sessions(session_id: &str, args: &Value) -> Value {
    if let Err(remote) = local_machine(args) {
        return remote;
    }
    let all_projects = args
        .get("all_projects")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // The app refuses a whole-store read while its cross-project setting is
    // off; a closed app must not widen that (#3422).
    if all_projects && !read_across_projects() {
        return error("not_allowed", reach_refused());
    }
    let words: Vec<String> = args
        .get("query")
        .and_then(Value::as_str)
        .map(|query| {
            query
                .to_lowercase()
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let caller = crate::config::session_meta_by_id(session_id);
    let caller_reach = caller.as_ref().map(|meta| reach_of(session_id, meta));
    let mut rows: Vec<(String, Value)> = Vec::new();
    for meta in crate::config::list_sessions() {
        let Some(id) = crate::config::session_id_from_path(&meta.path) else {
            continue;
        };
        if id == session_id {
            continue;
        }
        if !all_projects {
            let reach = reach_of(&id, &meta);
            let started_by_caller = crate::config::load_session_parent(&id)
                .and_then(|parent| parent.id)
                .is_some_and(|parent| parent == session_id);
            if !same_project(caller_reach.as_deref(), &reach) && !started_by_caller {
                continue;
            }
        }
        if !words
            .iter()
            .all(|word| meta.title.to_lowercase().contains(word))
        {
            continue;
        }
        let updated = updated_at(&meta);
        let mut row = json!({
            "session_id": id,
            "title": meta.title.clone(),
            "working": false,
            "status": crate::thread_status::current(&id).as_str(),
            "harness": crate::harness::store::load(&id)
                .map(|record| record.harness)
                .unwrap_or_else(|| crate::harness::BUILTIN_ID.to_owned()),
        });
        if all_projects {
            row["project"] = json!(project_of(&id, &meta));
        }
        if let Some(updated) = updated.as_ref() {
            row["updated_at"] = json!(updated);
        }
        if let Some(worktree) = crate::worktrees::list_meta(&id) {
            row["worktree"] = worktree;
        }
        rows.push((updated.unwrap_or_default(), row));
    }
    rows.sort_by(|left, right| right.0.cmp(&left.0));
    let sessions: Vec<Value> = rows
        .into_iter()
        .take(MAX_SESSIONS)
        .map(|(_, row)| row)
        .collect();
    json!({
        "machine": local_name(),
        "machine_id": LOCAL_MACHINE_ID,
        "sessions": sessions,
    })
}

/// `read_session`: the last turns of one thread as text, paged by byte offset.
fn read_session(caller: &str, args: &Value) -> Value {
    if let Err(remote) = local_machine(args) {
        return remote;
    }
    let target = args.get("session_id").and_then(Value::as_str).unwrap_or("");
    if target == caller {
        return error("not_allowed", "a thread cannot read itself");
    }
    let Some(meta) = crate::config::session_meta_by_id(target) else {
        return error("not_found", "no thread has that id; use list_sessions");
    };
    let caller_meta = crate::config::session_meta_by_id(caller);
    let reaches = caller_meta.as_ref().is_some_and(|caller_meta| {
        same_project(
            Some(&reach_of(caller, caller_meta)),
            &reach_of(target, &meta),
        )
    }) || crate::config::load_session_parent(target)
        .and_then(|parent| parent.id)
        .is_some_and(|parent| parent == caller);
    if !reaches && !read_across_projects() {
        return error("not_allowed", reach_refused());
    }
    // In Ask mode the app shows a read card before it reads another thread.
    // No app means no card, so refuse instead of reading without the answer.
    if read_needs_approval(caller) {
        return error(
            "unavailable",
            format!(
                "read_session on {target} needs the RustCode app open to approve the read; this thread runs on a machine whose app is not attached. Open the RustCode app and retry. Nothing was sent."
            ),
        );
    }
    let title = meta.title.clone();
    let status = crate::thread_status::current(target).as_str().to_owned();
    let project = project_of(target, &meta);
    let history = crate::config::load_session_file(&meta.path);
    let last_turns = args
        .get("last_turns")
        .and_then(Value::as_u64)
        .unwrap_or(super::DEFAULT_LAST_TURNS)
        .clamp(1, super::MAX_LAST_TURNS) as usize;
    let all = turns(&history);
    let start = all.len().saturating_sub(last_turns);
    let rendered = render_read(&title, target, &status, &project, &all[start..]);
    let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let (text, next) = match page_of(&rendered, offset) {
        Ok(page) => page,
        Err(reason) => return error("invalid", reason),
    };
    let mut answer = json!({
        "session_id": target,
        "title": title,
        "status": status,
        "project": project,
        "offset": offset,
        "text": text,
        "machine": local_name(),
        "machine_id": LOCAL_MACHINE_ID,
    });
    match next {
        Some(next) => answer["next_offset"] = json!(next),
        None => answer["complete"] = json!(true),
    }
    answer
}

/// A thread's turns as `read_session` gives them: the request, its replies,
/// and how many files its tools changed.
struct Turn {
    user: String,
    /// The turn's last assistant item, as the app's `turnsOf` keeps it.
    assistant: String,
    files: BTreeSet<String>,
}

fn turns(history: &[ChatMessage]) -> Vec<Turn> {
    let mut out = Vec::new();
    let mut current: Option<Turn> = None;
    for message in history {
        if message.is_user_request() {
            out.extend(current.take());
            current = Some(Turn {
                user: message.content.clone(),
                assistant: String::new(),
                files: BTreeSet::new(),
            });
            continue;
        }
        let Some(turn) = current.as_mut() else {
            continue;
        };
        if message.role == "assistant"
            && !message.conversation_recap
            && !message.content.trim().is_empty()
        {
            turn.assistant = message.content.clone();
        }
        if let Some(result) = &message.tool_result {
            turn.files.extend(result.changed_paths.iter().cloned());
        }
    }
    out.extend(current);
    out
}

fn render_read(
    title: &str,
    session_id: &str,
    status: &str,
    project: &str,
    turns: &[Turn],
) -> String {
    let mut lines = vec![
        format!("Thread: {title}"),
        format!("Session: {session_id}"),
        format!("Status: {status}"),
        format!("Project: {project}"),
        format!("Turns: {}", turns.len()),
        String::new(),
    ];
    for (index, turn) in turns.iter().enumerate() {
        lines.push(format!("Turn {}", index + 1));
        lines.push("  user:".to_owned());
        for line in turn.user.split('\n') {
            lines.push(format!("    {line}"));
        }
        lines.push("  assistant:".to_owned());
        for line in turn.assistant.split('\n') {
            lines.push(format!("    {line}"));
        }
        lines.push(format!("  files changed: {}", turn.files.len()));
        lines.push(String::new());
    }
    lines.join("\n")
}

/// `text` from byte `offset`, at most [`READ_BYTES`], cut on a character
/// boundary: the next offset, or `None` at the end.
fn page_of(text: &str, offset: usize) -> Result<(String, Option<usize>), String> {
    let bytes = text.as_bytes();
    if offset > bytes.len() {
        return Err(format!(
            "offset {offset} is out of range; this answer is {} bytes",
            bytes.len()
        ));
    }
    if offset < bytes.len() && (bytes[offset] & 0xc0) == 0x80 {
        return Err(
            "offset must be on a UTF-8 character boundary; use the returned next_offset".to_owned(),
        );
    }
    let mut end = (offset + READ_BYTES).min(bytes.len());
    while end > offset && end < bytes.len() && (bytes[end] & 0xc0) == 0x80 {
        end -= 1;
    }
    let page = text
        .get(offset..end)
        .ok_or_else(|| "offset must be on a UTF-8 character boundary".to_owned())?;
    Ok((page.to_owned(), (end < bytes.len()).then_some(end)))
}

/// `list_harnesses`: the harnesses this machine can run, with the models and
/// named accounts the app's own snapshot carries.
async fn list_harnesses(args: &Value) -> Value {
    if let Err(remote) = local_machine(args) {
        return remote;
    }
    let snapshot = match crate::harness::snapshot().await {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            return error(
                "failed",
                format!("the harness list could not be read: {reason}"),
            );
        }
    };
    let mut rows: Vec<Value> = snapshot
        .get("harnesses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let asked = args
        .get("harness")
        .and_then(Value::as_str)
        .map(|asked| asked.trim().to_lowercase())
        .filter(|asked| !asked.is_empty());
    if let Some(asked) = asked.as_ref() {
        let by_id: Vec<Value> = rows
            .iter()
            .filter(|row| {
                row.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.to_lowercase() == *asked)
            })
            .cloned()
            .collect();
        rows = if by_id.is_empty() {
            rows.iter()
                .filter(|row| {
                    row.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.to_lowercase() == *asked)
                })
                .cloned()
                .collect()
        } else {
            by_id
        };
        if rows.is_empty() {
            let known = snapshot
                .get("harnesses")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.get("id").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            return error(
                "invalid",
                format!("no harness “{asked}”; list_harnesses has {known}"),
            );
        }
        if rows.len() > 1 {
            let ids = rows
                .iter()
                .filter_map(|row| row.get("id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            return error(
                "invalid",
                format!(
                    "“{asked}” names {} harnesses; pass the id: {ids}",
                    rows.len()
                ),
            );
        }
    }
    let filter = args
        .get("model")
        .and_then(Value::as_str)
        .map(|model| model.trim().to_lowercase())
        .filter(|model| !model.is_empty());
    for row in &mut rows {
        let models = row
            .get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut matched: Vec<Value> = match filter.as_ref() {
            None => models,
            Some(filter) => models
                .into_iter()
                .filter(|model| {
                    ["id", "name"].iter().any(|key| {
                        model
                            .get(*key)
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.to_lowercase().contains(filter))
                    })
                })
                .collect(),
        };
        let cap = if filter.is_some() {
            FILTERED_MODELS
        } else {
            LISTED_MODELS
        };
        let matched_any = !matched.is_empty();
        if matched.len() > cap {
            let omitted = matched.len() - cap;
            matched.truncate(cap);
            row["omitted_models"] = json!(omitted);
            row["filter"] = json!("pass model to search the omitted ones");
        }
        row["models"] = Value::Array(matched);
        // A filter that misses this harness does not fail the others, as in
        // the app: the row says so and the caller sees every harness.
        if filter.is_some() && !matched_any {
            row["filter"] = json!("no model matches; pass a shorter substring");
        }
    }
    json!({
        "machine": local_name(),
        "machine_id": LOCAL_MACHINE_ID,
        "harnesses": rows,
        "default": snapshot.get("default").cloned().unwrap_or(Value::Null),
    })
}

/// One saved machine, as `machines.json` keeps it. Only what `list_machines`
/// reports is read; a field this release does not know is ignored.
#[derive(Debug, Default, serde::Deserialize)]
struct SavedMachine {
    #[serde(default)]
    host_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    icon_tint: Option<String>,
    #[serde(default = "unknown_platform")]
    platform: String,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    last_connected_at: Option<String>,
    #[serde(default)]
    routes: Vec<SavedRoute>,
    #[serde(default)]
    probes: Vec<SavedProbe>,
}

/// One saved route; its kind is `lan`, `vpn`, `tailscale` or `public`.
#[derive(Debug, Default, serde::Deserialize)]
struct SavedRoute {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    address: String,
}

/// What the last test of one route found, as the app saved it.
#[derive(Debug, Default, serde::Deserialize)]
struct SavedProbe {
    #[serde(default)]
    address: String,
    #[serde(default)]
    outcome: String,
    #[serde(default)]
    latency_ms: Option<u64>,
    #[serde(default)]
    at: Option<String>,
}

/// The gateway's port, which an address without one means.
const GATEWAY_PORT: u16 = 17879;

/// Whether two spellings name one endpoint: `studio.local:17879` and
/// `studio.local`, `[fd00::1]:17879` and `fd00::1`. The engine does not link
/// the client, so this is `rustcode-client`'s `registry::same_address`, which
/// matches probes to routes in the app.
fn same_address(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right) || endpoint(left) == endpoint(right)
}

/// An address's lowercase host and port, with the scheme and path dropped.
fn endpoint(address: &str) -> (String, u16) {
    let address = address.trim();
    let address = address.split_once("://").map_or(address, |(_, rest)| rest);
    let address = address.split('/').next().unwrap_or(address);
    let (host, port) = if let Some(rest) = address.strip_prefix('[') {
        let (host, after) = rest.split_once(']').unwrap_or((rest, ""));
        (host, after.strip_prefix(':'))
    } else {
        match address.rsplit_once(':') {
            Some((host, port))
                if !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) =>
            {
                (host, Some(port))
            }
            _ => (address, None),
        }
    };
    let port = port
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(GATEWAY_PORT);
    (host.to_ascii_lowercase(), port)
}

impl SavedMachine {
    /// Each route with the last test of it, or `null` for one never tested.
    fn route_rows(&self) -> Vec<Value> {
        self.routes
            .iter()
            .map(|route| {
                let probe = self
                    .probes
                    .iter()
                    .find(|probe| same_address(&probe.address, &route.address))
                    .map(|probe| {
                        let mut row = json!({"result": probe.outcome, "tested_at": probe.at});
                        if let Some(latency) = probe.latency_ms {
                            row["latency_ms"] = json!(latency);
                        }
                        row
                    });
                json!({"kind": route.kind, "address": route.address, "probe": probe})
            })
            .collect()
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct MachineRegistry {
    #[serde(default)]
    machines: Vec<SavedMachine>,
}

fn unknown_platform() -> String {
    "unknown".to_owned()
}

/// `list_machines`: this computer and every machine paired on disk. The app
/// owns the live connections, so a saved machine's state is `unknown` here, and
/// the routes are not tested: each shows the last test the app saved.
fn list_machines(args: &Value) -> Value {
    let mut machines = vec![json!({
        "id": LOCAL_MACHINE_ID,
        "name": local_name(),
        "display_name": Value::Null,
        "icon": Value::Null,
        "icon_tint": Value::Null,
        "platform": platform(),
        "current": true,
        "state": "connected",
    })];
    let saved = crate::config::get_config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("machines.json")).ok())
        .and_then(|text| serde_json::from_str::<MachineRegistry>(&text).ok())
        .unwrap_or_default();
    for machine in saved.machines {
        let mut row = json!({
            "id": machine.host_id,
            "name": machine.name,
            "display_name": machine.display_name,
            "icon": machine.icon,
            "icon_tint": machine.icon_tint,
            "platform": machine.platform,
            "state": "unknown",
        });
        if machine.paused {
            row["paused"] = json!(true);
        }
        let routes = machine.route_rows();
        if !routes.is_empty() {
            row["routes"] = json!(routes);
        }
        if let Some(last_connected) = machine.last_connected_at {
            row["last_connected"] = json!(last_connected);
        }
        machines.push(row);
    }
    let mut answer = json!({
        "current": LOCAL_MACHINE_ID,
        "machines": machines,
        "note": "The RustCode app is not attached, so paired machines come from machines.json and whether each one answers was not checked.",
    });
    if args.get("probe").and_then(Value::as_bool) == Some(true) {
        answer["probed"] = json!(false);
        answer["note"] = json!(
            "The RustCode app is not attached, so the routes were not tested: each shows the last test the app saved, or null if it never ran one."
        );
    }
    answer
}

/// `thread_status`: where this thread's delegates stand, from the link book and
/// each thread's own record. Pull requests and checks are the app's to read.
fn thread_status(caller: &str, args: &Value) -> Value {
    if let Err(remote) = local_machine(args) {
        return remote;
    }
    let links = link_book();
    let asked = args
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if asked == Some(caller) {
        return error(
            "not_allowed",
            "a thread reports its own pull request through list_pull_requests",
        );
    }
    let targets: Vec<String> = match asked {
        Some(target) => vec![target.to_owned()],
        None => {
            let mut seen = BTreeSet::new();
            let mut ids: Vec<String> = links
                .iter()
                .rev()
                .filter(|link| link.get("orchestrator").and_then(Value::as_str) == Some(caller))
                .filter_map(|link| link.get("target").and_then(Value::as_str))
                .filter(|target| *target != caller && seen.insert((*target).to_owned()))
                .map(str::to_owned)
                .collect();
            let omitted = ids.len().saturating_sub(MAX_DELEGATES);
            ids.truncate(MAX_DELEGATES);
            if omitted > 0 {
                let mut answer = delegates(&ids);
                answer["omitted"] = json!(omitted);
                return answer;
            }
            ids
        }
    };
    if let Some(target) = targets.first()
        && crate::config::session_meta_by_id(target).is_none()
    {
        return error("not_found", "no thread has that id; use list_sessions");
    }
    delegates(&targets)
}

fn delegates(targets: &[String]) -> Value {
    let rows: Vec<Value> = targets
        .iter()
        .filter_map(|target| {
            let meta = crate::config::session_meta_by_id(target)?;
            let worktree = crate::worktrees::list_meta(target);
            let mut row = json!({
                "session_id": target,
                "title": meta.title,
                "status": crate::thread_status::current(target).as_str(),
                "project": project_of(target, &meta),
                "pull": Value::Null,
            });
            if let Some(worktree) = worktree {
                row["branch"] = worktree.get("branch").cloned().unwrap_or(Value::Null);
                row["worktree"] = worktree;
            }
            Some(row)
        })
        .collect();
    json!({
        "delegates": rows,
        "note": "The RustCode app is not attached, so pull requests and checks were not read.",
    })
}

/// `wait_session`: wait until a link the caller owns settles, reading the link
/// book the app saves. With no app nothing updates a working link, so a wait
/// can time out where an open app would have answered.
async fn wait_session(caller: &str, cancel: &CancellationToken, args: &Value) -> Value {
    if let Err(remote) = local_machine(args) {
        return remote;
    }
    let mut ids: Vec<String> = args
        .get("link_ids")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if let Some(one) = args.get("link_id").and_then(Value::as_str) {
        ids.push(one.to_owned());
    }
    let mut mine: Vec<String> = Vec::new();
    for link in link_book() {
        let Some(id) = link.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !ids.iter().any(|asked| asked == id) {
            continue;
        }
        if link.get("orchestrator").and_then(Value::as_str) != Some(caller) {
            continue;
        }
        mine.push(id.to_owned());
    }
    if mine.is_empty() {
        return error("not_found", "no such link from this thread");
    }
    let wait = super::requested_wait(super::WAIT_SESSION, args)
        .unwrap_or(Duration::from_millis(super::DEFAULT_WAIT_MS));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if cancel.is_cancelled() {
            return error("cancelled", "the wait was cancelled. Nothing was changed.");
        }
        let links = link_book();
        let mut settled = None;
        let mut current = Vec::new();
        for id in &mine {
            let Some(link) = links
                .iter()
                .find(|link| link.get("id").and_then(Value::as_str) == Some(id.as_str()))
            else {
                settled.get_or_insert_with(|| {
                    json!({
                        "links": [{
                            "link_id": id,
                            "status": "failed",
                            "answer": "That link is not in the record any more.",
                        }],
                    })
                });
                continue;
            };
            let row = link_result(link);
            let state = row
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed");
            // `needs_user` is an answer, as it is in the app: the caller sees
            // the question instead of waiting out the timeout.
            if !matches!(state, "queued" | "working") {
                settled.get_or_insert_with(|| json!({ "links": [row.clone()] }));
            }
            current.push(row);
        }
        if let Some(settled) = settled {
            return settled;
        }
        if tokio::time::Instant::now() >= deadline {
            return json!({
                "status": "timeout",
                "waited_until": waited_until(wait),
                "links": current,
                "note": "Still running. The RustCode app is not attached, so these are the link states it last recorded; open the app to keep them current. Anything that settles arrives as a wake while it is open; do not poll with sleeps.",
            });
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                return error("cancelled", "the wait was cancelled. Nothing was changed.");
            }
            _ = tokio::time::sleep(WAIT_CHECK) => {}
        }
    }
}

/// `wait_session`'s row for one link, with the target's last reply when the
/// link itself carries no answer.
fn link_result(link: &Value) -> Value {
    let state = link
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let target = link.get("target").and_then(Value::as_str).unwrap_or("");
    let mut row = json!({
        "link_id": link.get("id").cloned().unwrap_or(Value::Null),
        "status": state,
        "session_id": target,
        "title": link.get("targetTitle").cloned().unwrap_or(Value::Null),
    });
    if let Some(answer) = link.get("answer").and_then(Value::as_str) {
        row["answer"] = json!(answer);
    } else if let Some(reply) = last_reply(target) {
        row["answer"] = json!(reply);
    }
    if link.get("truncated").and_then(Value::as_bool) == Some(true) {
        row["answer_truncated"] = json!(true);
    }
    if let Some(files) = link.get("filesChanged").and_then(Value::as_i64) {
        row["files_changed"] = json!(files);
    }
    row["thread_status"] = json!(crate::thread_status::current(target).as_str());
    if state == "needs_user" {
        row["note"] = json!(
            "That thread is waiting for the user to answer it in the app, in its chat or the one above it."
        );
    }
    row
}

/// A thread's last assistant reply, cut to [`MAX_ANSWER_BYTES`].
const MAX_ANSWER_BYTES: usize = 8 * 1024;

fn last_reply(session_id: &str) -> Option<String> {
    let meta = crate::config::session_meta_by_id(session_id)?;
    let history = crate::config::load_session_file(&meta.path);
    let reply = history
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.content.trim().is_empty())?;
    let mut end = reply.content.len().min(MAX_ANSWER_BYTES);
    while !reply.content.is_char_boundary(end) {
        end -= 1;
    }
    Some(reply.content[..end].to_owned())
}

fn waited_until(wait: Duration) -> String {
    let wait = chrono::Duration::from_std(wait).unwrap_or_default();
    (chrono::Utc::now() + wait).to_rfc3339()
}

fn link_book() -> Vec<Value> {
    crate::data_store::load_link_book()
        .ok()
        .and_then(|stored| stored.get("book").cloned())
        .and_then(|book| book.get("links").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

/// A thread's project: its worktree's repository, else its folder.
fn project_of(session_id: &str, meta: &crate::config::SessionMeta) -> String {
    let path = crate::worktrees::list_meta(session_id)
        .and_then(|worktree| {
            worktree
                .get("repo")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            meta.workspace_cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    clean_path(&path)
}

/// The reach of a thread's tools: its repository, where a worktree and its
/// checkout meet, else its folder.
fn reach_of(session_id: &str, meta: &crate::config::SessionMeta) -> String {
    crate::worktrees::of_session(session_id)
        .map(|worktree| clean_path(&worktree.repo.to_string_lossy()))
        .or_else(|| {
            crate::config::load_session_metadata(session_id)
                .and_then(|metadata| metadata.repo)
                .map(|repo| clean_path(&repo.to_string_lossy()))
        })
        .unwrap_or_else(|| project_of(session_id, meta))
}

fn same_project(left: Option<&str>, right: &str) -> bool {
    left.is_some_and(|left| !left.is_empty() && left == right)
}

/// A project path as people write it: no Windows verbatim prefix, no trailing
/// slash, and no macOS `/private` in front of `/tmp`, `/var` or `/etc`.
fn clean_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = rustcode_session::strip_verbatim(Path::new(path));
    let mut cleaned = stripped.to_string_lossy().into_owned();
    while cleaned.len() > 1 && cleaned.ends_with('/') {
        cleaned.pop();
    }
    cleaned = cleaned.replace("/./", "/");
    for prefix in ["/tmp", "/var", "/etc"] {
        let private = format!("/private{prefix}");
        if cleaned == private || cleaned.starts_with(&format!("{private}/")) {
            cleaned = cleaned.replacen(&private, prefix, 1);
        }
    }
    cleaned
}

/// A thread's last update as RFC 3339, from its transcript or its file.
fn updated_at(meta: &crate::config::SessionMeta) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(&meta.when)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc).to_rfc3339())
        .or_else(|| {
            std::fs::metadata(&meta.path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from)
                .map(|timestamp| timestamp.to_rfc3339())
        })
}

/// The name this computer reports, as the app's `list_machines` names it.
fn local_name() -> String {
    sysinfo::System::host_name()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| LOCAL_MACHINE_ID.to_owned())
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        other => other,
    }
}

/// The answers a retry key already produced, mirroring the app's receipt
/// store for the life of this process.
#[derive(Default)]
struct Receipts {
    entries: Mutex<HashMap<(String, String), Receipt>>,
}

struct Receipt {
    tool: String,
    args_hash: String,
    answer: Value,
}

enum Earlier {
    None,
    Conflict,
    Replayed(Value),
}

/// The most calls a process remembers: a thread's retries, not a history.
const MAX_RECEIPTS: usize = 256;

impl Receipts {
    /// What an earlier call under this caller and key answered, or that the
    /// key was used for another action or other arguments.
    fn earlier(&self, caller: &str, key: &str, tool: &str, args: &Value) -> Earlier {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(receipt) = entries.get(&(caller.to_owned(), key.to_owned())) else {
            return Earlier::None;
        };
        if receipt.tool != tool || receipt.args_hash != super::args_hash(tool, args) {
            return Earlier::Conflict;
        }
        Earlier::Replayed(receipt.answer.clone())
    }

    /// Remember a successful answer. A full map drops everything: receipts
    /// only save a retry, and a lost one costs only a repeated call.
    fn record(&self, caller: &str, key: &str, tool: &str, args: &Value, answer: &Value) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries.len() >= MAX_RECEIPTS {
            entries.clear();
        }
        entries.insert(
            (caller.to_owned(), key.to_owned()),
            Receipt {
                tool: tool.to_owned(),
                args_hash: super::args_hash(tool, args),
                answer: answer.clone(),
            },
        );
    }
}

fn receipts() -> &'static Receipts {
    static RECEIPTS: LazyLock<Receipts> = LazyLock::new(Receipts::default);
    &RECEIPTS
}

/// A recorded answer, marked so an agent knows it is not new work.
fn replay(mut answer: Value) -> Value {
    if answer.get("error").is_none()
        && let Some(object) = answer.as_object_mut()
    {
        object.insert("replayed".to_owned(), json!(true));
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_machine_lists_each_route_with_its_last_test() {
        let saved: SavedMachine = serde_json::from_value(json!({
            "host_id": "gw-a",
            "routes": [
                {"kind": "lan", "address": "192.168.1.20:17879"},
                {"kind": "vpn", "address": "100.90.16.237:17879"}
            ],
            "probes": [
                {"address": "192.168.1.20", "outcome": "reachable", "latency_ms": 4, "at": "2026-11-12T10:00:00Z"}
            ]
        }))
        .unwrap();
        let rows = saved.route_rows();
        assert_eq!(rows[0]["probe"]["result"], "reachable");
        assert_eq!(rows[0]["probe"]["latency_ms"], 4);
        // A probe saved under one spelling of the address still matches the route.
        assert!(rows[1]["probe"].is_null(), "an untested route says so");
        assert!(same_address("[fd00::1]:17879", "FD00::1"));
        assert!(!same_address("studio.local:9000", "studio.local"));
        assert_eq!(rows[1]["address"], "100.90.16.237:17879");
    }

    /// A caller thread with Full access, so a read does not need the app's card.
    fn full_access_caller() -> String {
        let caller = crate::config::create_detached_session();
        crate::config::save_session_yolo(&caller, true);
        caller
    }

    fn set_setting(key: &str, value: Value) {
        let mut values = serde_json::Map::new();
        values.insert(key.to_owned(), value);
        crate::acp::settings::write(crate::acp::settings::SetSettingsRequest { values })
            .expect("the test setting is written");
    }

    #[test]
    fn a_retry_key_replays_the_same_call_and_refuses_another() {
        let receipts = Receipts::default();
        let args = json!({"all_projects": true});
        assert!(matches!(
            receipts.earlier("thread-a", "key-1", super::super::LIST_SESSIONS, &args),
            Earlier::None
        ));
        let answer = json!({"sessions": []});
        receipts.record(
            "thread-a",
            "key-1",
            super::super::LIST_SESSIONS,
            &args,
            &answer,
        );
        match receipts.earlier("thread-a", "key-1", super::super::LIST_SESSIONS, &args) {
            Earlier::Replayed(replayed) => assert_eq!(replayed, answer),
            _ => panic!("the receipt did not replay"),
        }
        // Another caller, another key, another action or other arguments is not the same call.
        assert!(matches!(
            receipts.earlier("thread-b", "key-1", super::super::LIST_SESSIONS, &args),
            Earlier::None
        ));
        assert!(matches!(
            receipts.earlier("thread-a", "key-1", super::super::READ_SESSION, &args),
            Earlier::Conflict
        ));
        assert!(matches!(
            receipts.earlier(
                "thread-a",
                "key-1",
                super::super::LIST_SESSIONS,
                &json!({"query": "other"})
            ),
            Earlier::Conflict
        ));
    }

    #[tokio::test]
    async fn the_tools_that_need_the_app_say_so_by_name() {
        let cases = [
            (super::super::START_SESSION, json!({"prompt": "go"})),
            (
                super::super::MESSAGE_SESSION,
                json!({"session_id": "thread-b", "message": "go"}),
            ),
            (
                super::super::FAN_OUT,
                json!({"prompt": "go", "harnesses": [{"harness": "codex"}, {"harness": "claude-code"}]}),
            ),
            (
                super::super::UPDATE_SESSION,
                json!({"session_id": "thread-b", "title": "renamed"}),
            ),
            (
                super::super::MERGE_WHEN_GREEN,
                json!({"session_id": "thread-b"}),
            ),
        ];
        for (tool, args) in cases {
            let answer = answer_json("thread-a", &CancellationToken::new(), tool, &args).await;
            let message = answer["error"]["message"].as_str().unwrap_or("");
            assert!(message.contains(tool), "{message}");
            assert!(message.contains("RustCode app open"), "{message}");
        }
    }

    #[tokio::test]
    async fn a_read_with_no_app_answers_from_the_store() {
        let session_id = crate::config::create_detached_session();
        assert!(!session_id.is_empty());
        let mut history = Vec::new();
        let mut request = ChatMessage::new("user", "find the failing test");
        request.timestamp = "2026-01-01T00:00:00Z".to_owned();
        history.push(request);
        history.push(ChatMessage::new("assistant", "The test is in `a.rs`."));
        crate::config::save_session_history(&session_id, &history);
        crate::config::flush_history();

        let caller = full_access_caller();
        let listed = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::LIST_SESSIONS,
            &json!({"all_projects": true}),
        )
        .await;
        let sessions = listed["sessions"].as_array().cloned().unwrap_or_default();
        assert!(
            sessions
                .iter()
                .any(|row| row["session_id"] == json!(session_id)),
            "{listed}"
        );

        let read = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::READ_SESSION,
            &json!({"session_id": session_id}),
        )
        .await;
        let text = read["text"].as_str().unwrap_or("");
        assert!(text.contains("Thread:"), "{read}");
        assert!(text.contains("find the failing test"), "{read}");
        assert!(text.contains("The test is in `a.rs`."), "{read}");

        let machines = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::LIST_MACHINES,
            &json!({}),
        )
        .await;
        assert_eq!(machines["current"], json!("local"));
        let _ = crate::config::get_active_session_dir(&session_id)
            .map(|dir| std::fs::remove_dir_all(dir));
    }

    #[tokio::test]
    async fn read_session_pages_on_a_character_boundary() {
        let session_id = crate::config::create_detached_session();
        let mut history = vec![ChatMessage::new("user", "å".repeat(READ_BYTES))];
        history.push(ChatMessage::new("assistant", "done"));
        crate::config::save_session_history(&session_id, &history);
        crate::config::flush_history();
        let caller = full_access_caller();
        let first = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::READ_SESSION,
            &json!({"session_id": session_id, "last_turns": 1}),
        )
        .await;
        let next = first["next_offset"].as_u64().expect("a next page") as usize;
        assert!(next > 0 && next % 2 == 0, "a cut inside å: {next}");
        assert_eq!(next, READ_BYTES);
        let second = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::READ_SESSION,
            &json!({"session_id": session_id, "last_turns": 1, "offset": next}),
        )
        .await;
        assert!(second["text"].is_string(), "{second}");
        let _ = crate::config::get_active_session_dir(&session_id)
            .map(|dir| std::fs::remove_dir_all(dir));
    }

    #[tokio::test]
    async fn a_closed_app_does_not_widen_cross_project_reads() {
        let caller = full_access_caller();
        let target = crate::config::create_detached_session();
        let history = vec![
            ChatMessage::new("user", "the other project's work"),
            ChatMessage::new("assistant", "done"),
        ];
        crate::config::save_session_history(&target, &history);
        crate::config::flush_history();

        // The app's setting is off: neither the whole-store list nor a
        // cross-project read may go through while the app is closed.
        set_setting("read-across-projects", json!(false));
        let listed = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::LIST_SESSIONS,
            &json!({"all_projects": true}),
        )
        .await;
        assert_eq!(listed["error"]["code"], json!("not_allowed"), "{listed}");
        let read = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::READ_SESSION,
            &json!({"session_id": target}),
        )
        .await;
        assert_eq!(read["error"]["code"], json!("not_allowed"), "{read}");

        // The setting is on, but the thread is on Ask: the app would show a
        // read card, so without the app the read waits for the app.
        set_setting("read-across-projects", json!(true));
        set_setting("default-permission-mode", json!("ask"));
        crate::config::save_session_yolo(&caller, false);
        let read = answer_json(
            &caller,
            &CancellationToken::new(),
            super::super::READ_SESSION,
            &json!({"session_id": target}),
        )
        .await;
        let message = read["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("RustCode app open"), "{read}");
        let _ =
            crate::config::get_active_session_dir(&caller).map(|dir| std::fs::remove_dir_all(dir));
        let _ =
            crate::config::get_active_session_dir(&target).map(|dir| std::fs::remove_dir_all(dir));
    }

    #[test]
    fn a_turn_keeps_its_last_assistant_item() {
        let history = vec![
            ChatMessage::new("user", "do it"),
            ChatMessage::new("assistant", "first"),
            ChatMessage::new("assistant", "last"),
        ];
        let turns = turns(&history);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].assistant, "last");
    }

    #[test]
    fn a_project_path_drops_what_is_not_its_own() {
        assert_eq!(clean_path("/tmp/project/"), "/tmp/project");
        assert_eq!(clean_path("/private/tmp/project"), "/tmp/project");
        assert_eq!(clean_path("/a/./b"), "/a/b");
    }
}
