use super::*;

/// A retry key: 1 to [`MAX_CLIENT_REQUEST_ID_CHARS`] characters of A-Z, a-z,
/// 0-9, `.`, `_`, `:` and `-`. The app's receipt store checks the same thing,
/// through this one rule.
pub(crate) fn check_request_id(id: &str) -> Result<(), String> {
    let allowed = !id.is_empty()
        && id.chars().count() <= MAX_CLIENT_REQUEST_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if allowed {
        return Ok(());
    }
    Err(format!(
        "{CLIENT_REQUEST_ID} must be 1 to {MAX_CLIENT_REQUEST_ID_CHARS} characters of A-Z, a-z, 0-9, '.', '_', ':' or '-'"
    ))
}

/// An optional retry key, absent or [`check_request_id`]'s shape.
fn optional_request_id(args: &Value) -> Result<(), String> {
    match args.get(CLIENT_REQUEST_ID) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(id)) => check_request_id(id),
        Some(_) => Err(format!(
            "{CLIENT_REQUEST_ID} must be 1 to {MAX_CLIENT_REQUEST_ID_CHARS} characters of A-Z, a-z, 0-9, '.', '_', ':' or '-'"
        )),
    }
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// A required non-empty text of at most [`MAX_MESSAGE_BYTES`].
fn bounded_text(args: &Value, key: &str) -> Result<(), String> {
    let value = text(args, key).unwrap_or("");
    if value.trim().is_empty() {
        return Err(format!("{key} is required"));
    }
    if value.len() > MAX_MESSAGE_BYTES {
        return Err(format!("{key} is over {MAX_MESSAGE_BYTES} bytes"));
    }
    Ok(())
}

/// An optional field that, when present, must be a non-empty string.
fn required_name(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(()),
        _ => Err(format!("{key} is required")),
    }
}

/// An optional `access` for a new thread: it can only lower what the thread
/// would get (#3391).
fn optional_access(args: &Value) -> Result<(), String> {
    match args.get("access") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(mode)) if matches!(mode.as_str(), "ask" | "edits" | "full") => Ok(()),
        Some(_) => Err("access must be ask, edits or full".to_owned()),
    }
}

fn optional_name(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if !value.trim().is_empty() && value.len() <= 200 => Ok(()),
        Some(_) => Err(format!("{key} must be a short non-empty string")),
    }
}

/// An optional `attachments` list: absolute file paths, at most
/// [`MAX_ATTACHMENTS`] of them. Each must be a file of at most
/// [`MAX_ATTACHMENT_BYTES`], which only the app can measure; the engine checks
/// the shape so a harness cannot send a path it made up as an attachment.
fn optional_attachments(args: &Value) -> Result<(), String> {
    match args.get("attachments") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Array(paths)) => {
            if paths.len() > MAX_ATTACHMENTS {
                return Err(format!("attachments takes at most {MAX_ATTACHMENTS} files"));
            }
            for path in paths {
                let Some(path) = path.as_str().filter(|path| !path.trim().is_empty()) else {
                    return Err("each attachments entry must be a path".to_owned());
                };
                if !std::path::Path::new(path).is_absolute() {
                    return Err(format!("attachments entry {path} is not an absolute path"));
                }
            }
            Ok(())
        }
        Some(_) => Err("attachments must be a list of absolute paths".to_owned()),
    }
}

/// An optional flag: when present it must be a boolean, never a string.
fn optional_flag(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        None | Some(Value::Null | Value::Bool(_)) => Ok(()),
        Some(_) => Err(format!("{key} must be true or false")),
    }
}

fn optional_title(args: &Value) -> Result<(), String> {
    match args.get("title") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(title)) if title.chars().count() <= MAX_TITLE_CHARS => Ok(()),
        Some(_) => Err(format!(
            "title must be a string of at most {MAX_TITLE_CHARS} characters"
        )),
    }
}

/// Whether `path` is absolute on the machine it names, which may not be this
/// one: a Mac starting a thread on a Windows machine passes `C:\Users\…`, and
/// `Path::is_absolute` on macOS calls that relative. The machine that gets
/// the call matches it against its own projects, so only the shape is checked
/// here: `/…`, a drive (`C:\` or `C:/`), or a UNC or verbatim path (`\\…`).
pub(crate) fn absolute_on_any_machine(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with(r"\\") {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// `path`: the absolute folder of an existing project, which every project
/// tool names.
fn absolute_project(args: &Value) -> Result<(), String> {
    match args.get("path") {
        Some(Value::String(path)) if absolute_on_any_machine(path) => Ok(()),
        Some(Value::String(_)) => Err("path must be an absolute folder".to_owned()),
        _ => Err("path is required".to_owned()),
    }
}

/// Check the arguments before they leave the engine.
pub fn validate(name: &str, args: &Value) -> Result<(), String> {
    // The machine a call acts on is the same argument on every tool that takes it.
    if machine_version(name).is_some() {
        optional_name(args, "machine")?;
    }
    match name {
        LIST_SESSIONS | LIST_WORKTREES => optional_flag(args, "all_projects"),
        THREAD_USAGE => {
            if let Some(since) = args.get("since").and_then(Value::as_str) {
                crate::data_store::parse_since(since)?;
            }
            match args.get("group_by") {
                None | Some(Value::Null) => {}
                Some(Value::String(group))
                    if crate::data_store::GroupBy::parse(group).is_some() => {}
                Some(_) => {
                    return Err(format!(
                        "group_by must be one of {:?}",
                        crate::data_store::GroupBy::ALL
                    ));
                }
            }
            optional_name(args, "project")
        }
        QUERY_DATA => {
            let sql = text(args, "sql").unwrap_or("");
            if sql.trim().is_empty() {
                return Err("sql is required".to_owned());
            }
            if sql.len() > MAX_SQL_BYTES {
                return Err(format!("sql is over {MAX_SQL_BYTES} bytes"));
            }
            match args.get("limit") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::Number(limit)) if limit.as_u64().is_some() => Ok(()),
                Some(_) => Err("limit must be a whole number of rows".to_owned()),
            }
        }
        LIST_HARNESSES => {
            optional_name(args, "harness")?;
            optional_name(args, "model")
        }
        LIST_MACHINES => optional_flag(args, "probe"),
        RUN_MACHINE_COMMAND => {
            bounded_text(args, "command")?;
            match args.get("cwd") {
                None | Some(Value::Null) => {}
                Some(Value::String(path)) if absolute_on_any_machine(path) => {}
                Some(_) => return Err("cwd must be an absolute directory path".to_owned()),
            }
            match args.get("timeout_secs") {
                None | Some(Value::Null) => Ok(()),
                Some(value)
                    if value.as_u64().is_some_and(|secs| {
                        (1..=crate::acp::machine_exec::MAX_TIMEOUT_SECS).contains(&secs)
                    }) =>
                {
                    Ok(())
                }
                Some(_) => Err(format!(
                    "timeout_secs must be 1 to {}",
                    crate::acp::machine_exec::MAX_TIMEOUT_SECS
                )),
            }
        }
        LIST_PROJECTS | LIST_PENDING_REQUESTS => Ok(()),
        TRANSFER_SESSION => {
            required_name(args, "session_id")?;
            required_name(args, "harness")?;
            optional_name(args, "account")?;
            optional_name(args, "model")
        }
        LIST_PULL_REQUESTS => match args.get("state") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(state)) if PR_STATES.contains(&state.as_str()) => Ok(()),
            Some(_) => Err(format!("state must be one of {PR_STATES:?}")),
        },
        SEARCH_SESSIONS => {
            optional_flag(args, "all_projects")?;
            if text(args, "query").unwrap_or("").trim().is_empty() {
                return Err("query is required".to_owned());
            }
            Ok(())
        }
        INTERRUPT_SESSION => {
            if text(args, "session_id").unwrap_or("").trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            Ok(())
        }
        CONFIGURE_SESSION => {
            optional_name(args, "session_id")?;
            optional_name(args, "model")?;
            optional_name(args, "effort")
        }
        COMPACT_SESSION => {
            optional_name(args, "session_id")?;
            match args.get("resume") {
                None | Some(Value::Bool(_)) => Ok(()),
                _ => Err("resume must be true or false".to_owned()),
            }
        }
        UPDATE_SESSION => {
            optional_name(args, "session_id")?;
            optional_title(args)?;
            for key in ["pinned", "unread"] {
                if !matches!(args.get(key), None | Some(Value::Null | Value::Bool(_))) {
                    return Err(format!("{key} must be true or false"));
                }
            }
            match args.get("status") {
                None | Some(Value::Null) => {}
                Some(Value::String(status)) if SESSION_STATUSES.contains(&status.as_str()) => {}
                Some(_) => return Err(format!("status must be one of {SESSION_STATUSES:?}")),
            }
            match args.get("snoozed_until") {
                None | Some(Value::Null | Value::String(_)) => {}
                Some(_) => return Err("snoozed_until must be 1h, evening, tomorrow, next_week, an ISO 8601 time or null".to_owned()),
            }
            // `snoozed_until: null` is a change: it brings the thread back.
            if args.get("snoozed_until").is_some() {
                return Ok(());
            }
            let changes = ["title", "pinned", "unread", "status"];
            if !changes
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err(
                    "give at least one of title, pinned, unread, status or snoozed_until"
                        .to_owned(),
                );
            }
            Ok(())
        }
        MESSAGE_SESSION => {
            let target = args.get("session_id").and_then(Value::as_str).unwrap_or("");
            let message = args.get("message").and_then(Value::as_str).unwrap_or("");
            if target.trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            if message.trim().is_empty() {
                return Err("message is required".to_owned());
            }
            if message.len() > MAX_MESSAGE_BYTES {
                return Err(format!("message is over {MAX_MESSAGE_BYTES} bytes"));
            }
            if args.get("steer").is_some_and(|value| !value.is_boolean()) {
                return Err("steer must be a boolean".to_owned());
            }
            if args.get("return_to").is_some_and(|value| {
                !value.is_null() && !matches!(value.as_str(), Some("me" | "none"))
            }) {
                return Err("return_to must be me or none".to_owned());
            }
            optional_attachments(args)?;
            optional_request_id(args)
        }
        READ_SESSION => {
            if text(args, "session_id").unwrap_or("").trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            match args.get("last_turns") {
                None | Some(Value::Null) => {}
                Some(turns)
                    if turns
                        .as_u64()
                        .is_some_and(|turns| (1..=MAX_LAST_TURNS).contains(&turns)) => {}
                Some(_) => return Err(format!("last_turns must be 1 to {MAX_LAST_TURNS}")),
            }
            match args.get("offset") {
                None | Some(Value::Null) => Ok(()),
                Some(offset) if offset.as_u64().is_some() => Ok(()),
                Some(_) => Err("offset must be a whole number of bytes".to_owned()),
            }
        }
        READ_ACTIVITY => {
            required_name(args, "session_id")?;
            match args.get("order") {
                None | Some(Value::Null) => {}
                Some(Value::String(order)) if order == "oldest" || order == "newest" => {}
                Some(_) => return Err("order must be oldest or newest".to_owned()),
            }
            match args.get("after") {
                None | Some(Value::Null) => {}
                Some(Value::String(after)) if after.len() <= MAX_ACTIVITY_CURSOR_BYTES => {}
                Some(_) => {
                    return Err(
                        "after must be the `next` token an earlier read_activity returned"
                            .to_owned(),
                    );
                }
            }
            match args.get("limit") {
                None | Some(Value::Null) => Ok(()),
                Some(limit) if limit.as_u64().is_some() => Ok(()),
                Some(_) => Err("limit must be a whole number of items".to_owned()),
            }
        }
        MANAGE_QUEUE => {
            required_name(args, "session_id")?;
            let action = args.get("action").and_then(Value::as_str).unwrap_or("");
            if !QUEUE_ACTIONS.contains(&action) {
                return Err(format!("action must be one of {QUEUE_ACTIONS:?}"));
            }
            // A list names no message; the other three act on one of this thread's own.
            if action == "list" {
                return Ok(());
            }
            let queue_id = text(args, "queue_id").unwrap_or("");
            if queue_id.trim().is_empty() {
                return Err(format!("queue_id is required to {action}"));
            }
            if queue_id.len() > MAX_QUEUE_ID_CHARS {
                return Err("queue_id is not one a list returned".to_owned());
            }
            if action == "edit" {
                bounded_text(args, "text")?;
            }
            Ok(())
        }
        THREAD_STATUS => optional_name(args, "session_id"),
        WORKTREE_STATUS => {
            optional_name(args, "session_id")?;
            optional_flag(args, "all_projects")
        }
        HANDOFF_WORKTREE => {
            required_name(args, "session_id")?;
            // `to` defaults to `checkout`: the direction an orchestrator asks for
            // when the user says "I'll take it from here".
            match args.get("to") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(to)) if HANDOFF_TARGETS.contains(&to.as_str()) => Ok(()),
                Some(_) => Err(format!("to must be one of {HANDOFF_TARGETS:?}")),
            }
        }
        ANSWER_REQUEST => {
            required_name(args, "session_id")?;
            if !matches!(args.get("request_id"), Some(Value::String(id)) if !id.trim().is_empty()) {
                return Err(
                    "request_id is required, as list_pending_requests printed it (a string)"
                        .to_owned(),
                );
            }
            let choice = match args.get("choice") {
                None | Some(Value::Null) => None,
                Some(Value::String(choice)) if APPROVAL_ANSWERS.contains(&choice.as_str()) => {
                    Some(choice)
                }
                Some(_) => return Err(format!("choice must be one of {APPROVAL_ANSWERS:?}")),
            };
            let answer = match args.get("answer") {
                None | Some(Value::Null) => None,
                Some(Value::String(answer)) if !answer.trim().is_empty() => Some(answer),
                Some(_) => return Err("answer must be a sentence or an option id".to_owned()),
            };
            match (choice, answer) {
                (Some(_), Some(_)) => {
                    Err("give choice for an approval or answer for a question, not both".to_owned())
                }
                (None, None) => {
                    Err("choice (an approval) or answer (a question) is required".to_owned())
                }
                (Some(_), None) | (None, Some(_)) => Ok(()),
            }
        }
        ORGANIZE_THREADS => {
            let Some(ids) = args.get("session_ids").and_then(Value::as_array) else {
                return Err("session_ids is required".to_owned());
            };
            if ids.is_empty() {
                return Err("session_ids needs at least one thread".to_owned());
            }
            if ids.len() > MAX_ORGANIZED_THREADS {
                return Err(format!(
                    "session_ids takes at most {MAX_ORGANIZED_THREADS} threads"
                ));
            }
            if ids
                .iter()
                .any(|id| id.as_str().is_none_or(|id| id.trim().is_empty()))
            {
                return Err("each session_ids entry must be a thread id".to_owned());
            }
            optional_title(args)?;
            for key in ["pinned", "unread", "archived"] {
                if !matches!(args.get(key), None | Some(Value::Null | Value::Bool(_))) {
                    return Err(format!("{key} must be true or false"));
                }
            }
            if !["title", "pinned", "unread", "archived"]
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err("give at least one of title, pinned, unread or archived".to_owned());
            }
            Ok(())
        }
        MERGE_WHEN_GREEN => {
            if args.get("number").is_some() {
                if args
                    .get("number")
                    .and_then(Value::as_u64)
                    .is_none_or(|n| n == 0)
                    || args.get("session_id").is_some()
                {
                    return Err("give session_id or a positive PR number".to_owned());
                }
            } else {
                required_name(args, "session_id")?;
            }
            match args.get("timeout_ms") {
                None | Some(Value::Null) => Ok(()),
                // A wait over the cap is cut to it (see `timeout`), not refused.
                Some(timeout) if timeout.as_u64().is_some_and(|ms| ms >= 1) => Ok(()),
                Some(_) => Err("timeout_ms must be a positive number of milliseconds".to_owned()),
            }
        }
        CLEANUP_WORKTREE => {
            required_name(args, "session_id")?;
            optional_flag(args, "settle")
        }
        READ_PROJECT | REMOVE_PROJECT => absolute_project(args),
        CREATE_PROJECT => {
            absolute_project(args)?;
            optional_flag(args, "init")
        }
        CLONE_PROJECT => {
            required_name(args, "url")?;
            match args.get("parent") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if absolute_on_any_machine(path) => Ok(()),
                Some(_) => Err("parent must be an absolute folder".to_owned()),
            }?;
            optional_name(args, "name")
        }
        UPDATE_PROJECT => {
            absolute_project(args)?;
            for key in ["name", "harness", "model"] {
                optional_name(args, key)?;
            }
            if !["name", "harness", "model"]
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err("give at least one of name, harness or model".to_owned());
            }
            Ok(())
        }
        SCHEDULE_TASK | CREATE_LOOP => {
            bounded_text(args, "prompt")?;
            optional_name(args, "name")?;
            if name == CREATE_LOOP {
                required_name(args, "name")?;
                optional_request_id(args)?;
                if args.get("session_id").is_some() {
                    return Err("use thread_id to reuse an existing thread".to_owned());
                }
                for field in ["harness", "account", "model", "effort", "thread_id"] {
                    optional_name(args, field)?;
                }
                if args.get("account").is_some_and(Value::is_string) {
                    required_name(args, "harness")?;
                }
                if args.get("thread_id").is_some_and(Value::is_string)
                    && ["harness", "account", "model", "effort"]
                        .iter()
                        .any(|key| args.get(key).is_some_and(Value::is_string))
                {
                    return Err(
                        "an existing thread uses its own harness, account, model and effort"
                            .to_owned(),
                    );
                }
                if args
                    .get("read_only")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err("read_only must be a boolean".to_owned());
                }
            } else {
                optional_name(args, "model_profile")?;
            }
            match args.get("project") {
                None | Some(Value::Null) => {}
                Some(Value::String(path)) if absolute_on_any_machine(path) => {}
                Some(_) => return Err("project must be an absolute path".to_owned()),
            }
            let Some(schedule) = args.get("schedule").filter(|value| value.is_object()) else {
                return Err("schedule is required".to_owned());
            };
            let Some(kind) = schedule
                .get("kind")
                .and_then(Value::as_str)
                .filter(|kind| SCHEDULE_KINDS.contains(kind))
            else {
                return Err(format!("schedule.kind must be one of {SCHEDULE_KINDS:?}"));
            };
            match schedule.get("expression") {
                None | Some(Value::Null) => {}
                Some(Value::String(expression)) if expression.split_whitespace().count() == 5 => {}
                Some(_) => {
                    return Err(
                        "schedule.expression must be a five-field cron expression".to_owned()
                    );
                }
            }
            match schedule.get("at") {
                None | Some(Value::Null) => {}
                Some(Value::String(at)) if chrono::DateTime::parse_from_rfc3339(at).is_ok() => {}
                Some(_) => {
                    return Err("schedule.at must be an ISO 8601 time with its offset".to_owned());
                }
            }
            // The kind's own field is the one `schedule_spec` reads; without it the call
            // would only fail later, as a target that stopped answering.
            match kind {
                "cron" if schedule.get("expression").and_then(Value::as_str).is_none() => {
                    Err("schedule.expression is required for a cron schedule".to_owned())
                }
                "at" if schedule.get("at").and_then(Value::as_str).is_none() => {
                    Err("schedule.at is required for a one-off schedule".to_owned())
                }
                _ => Ok(()),
            }
        }
        LIST_SCHEDULED_TASKS => Ok(()),
        UPDATE_LOOP => {
            required_name(args, "id")?;
            if args.get("session_id").is_some() {
                return Err("use thread_id to reuse an existing thread".to_owned());
            }
            if args
                .get("paused")
                .is_some_and(|value| !value.is_boolean() && !value.is_null())
            {
                return Err("paused must be a boolean".to_owned());
            }
            if args
                .get("read_only")
                .is_some_and(|value| !value.is_boolean() && !value.is_null())
            {
                return Err("read_only must be a boolean".to_owned());
            }
            if let Some(prompt) = args.get("prompt")
                && !prompt.is_null()
            {
                bounded_text(args, "prompt")?;
            }
            for field in ["name", "harness", "account", "model", "effort", "thread_id"] {
                optional_name(args, field)?;
            }
            if args.get("thread_id").is_some_and(Value::is_string)
                && ["harness", "account", "model", "effort"]
                    .iter()
                    .any(|key| args.get(key).is_some_and(Value::is_string))
            {
                return Err(
                    "an existing thread uses its own harness, account, model and effort".to_owned(),
                );
            }
            if let Some(schedule) = args.get("schedule")
                && !schedule.is_null()
            {
                let Some(obj) = schedule.as_object() else {
                    return Err("schedule must be an object".to_owned());
                };
                let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("");
                if kind == "manual" {
                    // Valid: runs on webhook or Run now.
                } else if !SCHEDULE_KINDS.contains(&kind) {
                    return Err(format!(
                        "schedule.kind must be one of {SCHEDULE_KINDS:?} or \"manual\""
                    ));
                } else {
                    match obj.get("expression") {
                        None | Some(Value::Null) => {}
                        Some(Value::String(expression))
                            if expression.split_whitespace().count() == 5 => {}
                        Some(_) => {
                            return Err("schedule.expression must be a five-field cron expression"
                                .to_owned());
                        }
                    }
                    match obj.get("at") {
                        None | Some(Value::Null) => {}
                        Some(Value::String(at))
                            if chrono::DateTime::parse_from_rfc3339(at).is_ok() => {}
                        Some(_) => {
                            return Err(
                                "schedule.at must be an ISO 8601 time with its offset".to_owned()
                            );
                        }
                    }
                    if kind == "cron" && obj.get("expression").and_then(Value::as_str).is_none() {
                        return Err(
                            "schedule.expression is required for a cron schedule".to_owned()
                        );
                    }
                    if kind == "at" && obj.get("at").and_then(Value::as_str).is_none() {
                        return Err("schedule.at is required for a one-off schedule".to_owned());
                    }
                }
            }
            let changes = [
                "paused",
                "name",
                "prompt",
                "schedule",
                "read_only",
                "harness",
                "account",
                "model",
                "effort",
                "thread_id",
            ]
            .iter()
            .any(|key| !matches!(args.get(*key), None | Some(Value::Null)));
            if !changes {
                return Err(
                    "give at least one field to update (paused, name, prompt, schedule, read_only, harness, account, model, effort or thread_id)"
                        .to_owned(),
                );
            }
            Ok(())
        }
        DELETE_SCHEDULED_TASK | RUN_SCHEDULED_TASK_NOW => required_name(args, "id"),
        START_SESSION => {
            bounded_text(args, "prompt")?;
            optional_title(args)?;
            for key in ["harness", "model", "effort", "account", presets::ARG] {
                optional_name(args, key)?;
            }
            optional_access(args)?;
            if !matches!(
                args.get("worktree"),
                None | Some(Value::Null | Value::Bool(_))
            ) {
                return Err("worktree must be true or false".to_owned());
            }
            match args.get("project") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if absolute_on_any_machine(path) => Ok(()),
                Some(_) => Err("project must be an absolute path".to_owned()),
            }?;
            optional_request_id(args)
        }
        FAN_OUT => {
            bounded_text(args, "prompt")?;
            optional_title(args)?;
            let Some(harnesses) = args.get("harnesses").and_then(Value::as_array) else {
                return Err("harnesses is required".to_owned());
            };
            if !(MIN_FAN_OUT..=MAX_FAN_OUT).contains(&harnesses.len()) {
                return Err(format!(
                    "harnesses takes {MIN_FAN_OUT} to {MAX_FAN_OUT} entries"
                ));
            }
            optional_name(args, presets::ARG)?;
            optional_access(args)?;
            let mut seen = Vec::with_capacity(harnesses.len());
            for entry in harnesses {
                // A delegate preset may give the harness (#3391); it is
                // expanded, and the call checked again, before it is sent.
                if !entry.is_object()
                    || (text(entry, "harness").is_none() && text(entry, presets::ARG).is_none())
                {
                    return Err("each harnesses entry needs a harness or a preset".to_owned());
                }
                for key in ["harness", "model", "effort", "account", presets::ARG] {
                    optional_name(entry, key)?;
                }
                // The same harness on two models, or on two named accounts, is a fair comparison.
                let key = (
                    text(entry, "harness"),
                    text(entry, "model"),
                    text(entry, "effort"),
                    text(entry, "account"),
                    text(entry, presets::ARG),
                );
                if seen.contains(&key) {
                    return Err("harnesses has the same entry twice".to_owned());
                }
                seen.push(key);
            }
            // The project on that machine, as `start_session` takes it.
            match args.get("project") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if absolute_on_any_machine(path) => Ok(()),
                Some(_) => Err("project must be an absolute path".to_owned()),
            }?;
            optional_request_id(args)
        }
        WAIT_SESSION => {
            let one = args.get("link_id").and_then(Value::as_str).is_some();
            let many = args
                .get("link_ids")
                .and_then(Value::as_array)
                .is_some_and(|ids| !ids.is_empty());
            if one || many {
                Ok(())
            } else {
                Err("link_id or link_ids is required".to_owned())
            }
        }
        other => Err(format!("{other} is not an orchestration tool")),
    }
}
