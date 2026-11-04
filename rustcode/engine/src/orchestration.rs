//! Threads that message other threads (#2637), milestone 1 of
//! `docs/architecture/cross-session-orchestration.md`: an agent lists the
//! threads of its own project, sends one a message and waits for its answer.
//! Version 2 (`docs/orchestrator.md`) adds `read_session`, `start_session`
//! and `fan_out`, and offers the same tools to external harnesses through
//! `rustcode mcp-orchestrator` ([`mcp_server`], [`hub`]).
//!
//! The client that hosts the threads carries them out. Each call becomes a
//! `_rustcode/orchestration` request to the client that advertised
//! `clientCapabilities._meta.rustcode.orchestration`. That client asks the
//! user for a grant, delivers the message as an attributed turn of the
//! target thread through the same turn queue a typed message uses, and
//! answers with a bounded result. The engine never drives another session
//! itself, so the target keeps its own turn gate, sandbox and approvals.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::preview::PreviewClient;
use crate::tools::{ToolErrorKind, ToolExecutionOutput};

pub mod hub;
pub mod mcp_server;

pub const METHOD: &str = "_rustcode/orchestration";
pub const LIST_SESSIONS: &str = "list_sessions";
pub const MESSAGE_SESSION: &str = "message_session";
pub const WAIT_SESSION: &str = "wait_session";
pub const READ_SESSION: &str = "read_session";
pub const START_SESSION: &str = "start_session";
pub const FAN_OUT: &str = "fan_out";
pub const LIST_HARNESSES: &str = "list_harnesses";
pub const SEARCH_SESSIONS: &str = "search_sessions";
pub const LIST_WORKTREES: &str = "list_worktrees";
pub const UPDATE_SESSION: &str = "update_session";
pub const INTERRUPT_SESSION: &str = "interrupt_session";
pub const CONFIGURE_SESSION: &str = "configure_session";
/// The M1 tools, what a client advertising version 1 gets.
pub const TOOL_NAMES: [&str; 3] = [LIST_SESSIONS, MESSAGE_SESSION, WAIT_SESSION];
/// Added by version 2.
pub const V2_TOOL_NAMES: [&str; 3] = [READ_SESSION, START_SESSION, FAN_OUT];
/// Added by version 3: discover what can run, find threads, and manage them.
pub const V3_TOOL_NAMES: [&str; 6] = [
    LIST_HARNESSES,
    SEARCH_SESSIONS,
    LIST_WORKTREES,
    UPDATE_SESSION,
    INTERRUPT_SESSION,
    CONFIGURE_SESSION,
];
/// The newest version this engine speaks.
pub const VERSION: u8 = 3;
/// What `update_session` may set a thread's status to.
pub const SESSION_STATUSES: [&str; 3] = ["active", "review", "settled"];

/// Turns `read_session` returns by default, and at most.
pub const DEFAULT_LAST_TURNS: u64 = 3;
pub const MAX_LAST_TURNS: u64 = 10;
/// How many harnesses one `fan_out` starts.
pub const MIN_FAN_OUT: usize = 2;
pub const MAX_FAN_OUT: usize = 4;
/// The longest title a new thread takes.
pub const MAX_TITLE_CHARS: usize = 200;

/// The most text a message or an answer carries.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// How long `wait_session` waits by default, and at most.
pub const DEFAULT_WAIT_MS: u64 = 60_000;
pub const MAX_WAIT_MS: u64 = 3_600_000;
/// How long the client may take on a list or a message: a message can wait
/// for the user to answer the grant.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Starting threads also creates worktrees and starts harnesses after the
/// user answers.
const START_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub const UNAVAILABLE_MESSAGE: &str = "error: messaging other threads needs the RustCode macOS app with \"Let threads message each other\" turned on in Settings. Nothing was sent.";

pub const PROMPT_HINT: &str = "\n\nOther threads: `list_sessions` lists this project's threads; `message_session` sends one a message (the user approves it once, and the thread answers in its own turn); `wait_session` returns that answer. Use them only when the user asks to involve another thread.";

/// [`PROMPT_HINT`] for version 2 clients.
pub const PROMPT_HINT_V2: &str = "\n\nOther threads: `list_sessions` and `read_session` show this project's threads; `message_session` messages one, `start_session` starts one, `fan_out` runs one task on 2-4 harnesses; `wait_session` returns their answers. The user approves first. Use them only when the user asks to involve other threads.";

/// [`PROMPT_HINT_V2`] for version 3 clients.
pub const PROMPT_HINT_V3: &str = "\n\nOther threads: `list_sessions`, `search_sessions`, `read_session` and `list_worktrees` show threads; `list_harnesses` shows what can run. `start_session`, `fan_out` and `message_session` give threads work, `wait_session` returns their answers; `update_session`, `configure_session` and `interrupt_session` manage them. The user approves work on other threads. Use them only when the user asks.";

/// Every orchestration tool of any version.
pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name) || V2_TOOL_NAMES.contains(&name) || V3_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 2 client.
pub fn is_v2_tool(name: &str) -> bool {
    V2_TOOL_NAMES.contains(&name)
}

/// Whether `name` needs a version 3 client.
pub fn is_v3_tool(name: &str) -> bool {
    V3_TOOL_NAMES.contains(&name)
}

/// The tools a client of `version` gets, in order.
pub fn tool_names(version: u8) -> Vec<&'static str> {
    match version {
        0 => Vec::new(),
        1 => TOOL_NAMES.to_vec(),
        2 => [
            LIST_SESSIONS,
            READ_SESSION,
            MESSAGE_SESSION,
            WAIT_SESSION,
            START_SESSION,
            FAN_OUT,
        ]
        .to_vec(),
        _ => {
            let mut names = tool_names(2);
            names.extend(V3_TOOL_NAMES);
            names
        }
    }
}

/// The orchestration version initialize `clientCapabilities._meta`
/// advertises, capped at the one this engine speaks; 0 for none.
pub fn client_meta_version(meta: Option<&serde_json::Map<String, Value>>) -> u8 {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("orchestration"))
        .and_then(Value::as_u64)
        .map_or(0, |version| version.min(u64::from(VERSION)) as u8)
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
fn optional_name(args: &Value, key: &str) -> Result<(), String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(value)) if !value.trim().is_empty() && value.len() <= 200 => Ok(()),
        Some(_) => Err(format!("{key} must be a short non-empty string")),
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

/// Check the arguments before they leave the engine.
pub fn validate(name: &str, args: &Value) -> Result<(), String> {
    match name {
        LIST_SESSIONS | LIST_HARNESSES | LIST_WORKTREES => Ok(()),
        SEARCH_SESSIONS => {
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
            let changes = ["title", "pinned", "unread", "status"];
            if !changes
                .iter()
                .any(|key| !matches!(args.get(*key), None | Some(Value::Null)))
            {
                return Err("give at least one of title, pinned, unread or status".to_owned());
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
            Ok(())
        }
        READ_SESSION => {
            if text(args, "session_id").unwrap_or("").trim().is_empty() {
                return Err("session_id is required".to_owned());
            }
            match args.get("last_turns") {
                None | Some(Value::Null) => Ok(()),
                Some(turns)
                    if turns
                        .as_u64()
                        .is_some_and(|turns| (1..=MAX_LAST_TURNS).contains(&turns)) =>
                {
                    Ok(())
                }
                Some(_) => Err(format!("last_turns must be 1 to {MAX_LAST_TURNS}")),
            }
        }
        START_SESSION => {
            bounded_text(args, "prompt")?;
            optional_title(args)?;
            for key in ["harness", "model", "effort"] {
                optional_name(args, key)?;
            }
            if !matches!(
                args.get("worktree"),
                None | Some(Value::Null | Value::Bool(_))
            ) {
                return Err("worktree must be true or false".to_owned());
            }
            match args.get("project") {
                None | Some(Value::Null) => Ok(()),
                Some(Value::String(path)) if std::path::Path::new(path).is_absolute() => Ok(()),
                Some(_) => Err("project must be an absolute path".to_owned()),
            }
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
            let mut seen = Vec::with_capacity(harnesses.len());
            for entry in harnesses {
                if !entry.is_object() || text(entry, "harness").is_none() {
                    return Err("each harnesses entry needs a harness".to_owned());
                }
                for key in ["harness", "model", "effort"] {
                    optional_name(entry, key)?;
                }
                let key = (
                    text(entry, "harness"),
                    text(entry, "model"),
                    text(entry, "effort"),
                );
                if seen.contains(&key) {
                    return Err("harnesses has the same entry twice".to_owned());
                }
                seen.push(key);
            }
            Ok(())
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

/// How long to wait for the client's answer to `name`.
pub fn timeout(name: &str, args: &Value) -> Duration {
    if name == START_SESSION || name == FAN_OUT {
        START_TIMEOUT
    } else if name == WAIT_SESSION {
        let wait = args
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_WAIT_MS)
            .clamp(1, MAX_WAIT_MS);
        // Room for the client to answer after its own wait ends.
        Duration::from_millis(wait) + Duration::from_secs(30)
    } else {
        REQUEST_TIMEOUT
    }
}

/// Run one call through the session's host.
pub async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    let (client, version) = {
        let state = state.lock().await;
        (
            state.orchestration_client.clone(),
            state.orchestration_version,
        )
    };
    execute_versioned(client, version, cancel, name, args).await
}

/// Run one call for a client of `version`: a version 1 client never gets
/// a version 2 call, even one the model made up.
pub async fn execute_versioned(
    client: Option<Arc<dyn PreviewClient>>,
    version: u8,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    if client.is_some() && !tool_names(version).contains(&name) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} needs a newer RustCode app. Nothing was sent."),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    }
    execute_with(client, cancel, name, args).await
}

pub async fn execute_with(
    client: Option<Arc<dyn PreviewClient>>,
    cancel: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &Value,
) -> ToolExecutionOutput {
    if let Err(reason) = validate(name, args) {
        return ToolExecutionOutput::failure_with_kind(
            format!("error: invalid {name} call: {reason}. Nothing was sent."),
            ToolErrorKind::InvalidArguments,
            false,
        );
    }
    let Some(client) = client else {
        return ToolExecutionOutput::failure_with_kind(
            UNAVAILABLE_MESSAGE.to_owned(),
            ToolErrorKind::UnavailableDependency,
            false,
        );
    };
    let request = client.request(json!({ "tool": name, "args": args }));
    let answer = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            return ToolExecutionOutput::failure_with_kind(
                format!("error: {name} was cancelled before the app answered; if it was a message, it may have been delivered"),
                ToolErrorKind::Cancelled,
                false,
            );
        }
        answer = tokio::time::timeout(timeout(name, args), request) => answer,
    };
    match answer {
        Err(_) => ToolExecutionOutput::failure_with_kind(
            format!(
                "error: the app did not answer {name} in time; if it was a message, it may have been delivered"
            ),
            ToolErrorKind::UnavailableDependency,
            true,
        ),
        Ok(Err(error)) => ToolExecutionOutput::failure_with_kind(
            format!("error: {name} failed: {error}"),
            ToolErrorKind::UnavailableDependency,
            false,
        ),
        Ok(Ok(value)) => output(name, value),
    }
}

/// The client's answer as the tool's result: its `error` as a failure,
/// the rest as JSON for the model.
fn output(name: &str, value: Value) -> ToolExecutionOutput {
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        let kind = match code {
            "denied" => ToolErrorKind::PermissionDenied,
            "not_found" | "target_unavailable" | "not_allowed" => ToolErrorKind::Validation,
            _ => ToolErrorKind::Unknown,
        };
        return ToolExecutionOutput::failure_with_kind(
            format!("error: {name} {code}: {message}"),
            kind,
            false,
        );
    }
    let mut content = serde_json::to_string_pretty(&value).unwrap_or_default();
    // The client caps answers; this holds if one does not.
    if content.len() > MAX_MESSAGE_BYTES * 2 {
        let mut end = MAX_MESSAGE_BYTES * 2;
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        content.truncate(end);
        content.push_str("\n[truncated]");
    }
    ToolExecutionOutput::success(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Answering(Value);

    impl PreviewClient for Answering {
        fn request(&self, params: Value) -> crate::preview::PreviewFuture {
            let answer = self.0.clone();
            Box::pin(async move {
                assert!(params.get("tool").is_some());
                Ok(answer)
            })
        }
    }

    fn client(answer: Value) -> Option<Arc<dyn PreviewClient>> {
        Some(Arc::new(Answering(answer)) as Arc<dyn PreviewClient>)
    }

    #[test]
    fn the_client_must_advertise_the_host() {
        let meta = |value: Value| value.as_object().cloned();
        let version = |value: Value| client_meta_version(meta(value).as_ref());
        assert_eq!(version(json!({"rustcode": {"orchestration": 1}})), 1);
        assert_eq!(version(json!({"rustcode": {"orchestration": 2}})), 2);
        // A newer client gets what this engine speaks.
        assert_eq!(version(json!({"rustcode": {"orchestration": 7}})), VERSION);
        assert_eq!(version(json!({"rustcode": {"preview": 1}})), 0);
        assert_eq!(client_meta_version(None), 0);
    }

    #[test]
    fn version_one_keeps_exactly_the_m1_tools() {
        assert!(tool_names(0).is_empty());
        assert_eq!(tool_names(1), TOOL_NAMES.to_vec());
        let v2 = tool_names(2);
        assert_eq!(v2.len(), 6);
        for name in TOOL_NAMES.iter().chain(V2_TOOL_NAMES.iter()) {
            assert!(v2.contains(name), "{name}");
            assert!(is_tool(name));
        }
        assert!(V2_TOOL_NAMES.iter().all(|name| is_v2_tool(name)));
        assert!(!TOOL_NAMES.iter().any(|name| is_v2_tool(name)));
    }

    #[test]
    fn version_three_adds_the_management_tools_only_for_its_clients() {
        assert!(
            V3_TOOL_NAMES
                .iter()
                .all(|name| !tool_names(2).contains(name))
        );
        let v3 = tool_names(3);
        assert_eq!(v3.len(), 12);
        for name in V3_TOOL_NAMES {
            assert!(v3.contains(&name), "{name}");
            assert!(is_tool(name) && is_v3_tool(name));
            assert!(PROMPT_HINT_V3.contains(name), "{name}");
        }
    }

    #[test]
    fn version_three_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(LIST_HARNESSES, json!({})));
        assert!(ok(SEARCH_SESSIONS, json!({"query": "login"})));
        assert!(!ok(SEARCH_SESSIONS, json!({"query": " "})));
        assert!(ok(INTERRUPT_SESSION, json!({"session_id": "b"})));
        assert!(!ok(INTERRUPT_SESSION, json!({})));
        assert!(ok(CONFIGURE_SESSION, json!({})));
        assert!(ok(
            CONFIGURE_SESSION,
            json!({"model": "m", "effort": "high"})
        ));
        assert!(ok(UPDATE_SESSION, json!({"pinned": true})));
        assert!(ok(
            UPDATE_SESSION,
            json!({"session_id": "b", "status": "settled"})
        ));
        assert!(!ok(UPDATE_SESSION, json!({"session_id": "b"})));
        assert!(!ok(UPDATE_SESSION, json!({"status": "gone"})));
        assert!(!ok(UPDATE_SESSION, json!({"pinned": "yes"})));
    }

    #[test]
    fn version_two_arguments_are_checked() {
        let ok = |name: &str, args: Value| validate(name, &args).is_ok();
        assert!(ok(READ_SESSION, json!({"session_id": "b"})));
        assert!(ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 10})
        ));
        assert!(!ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 0})
        ));
        assert!(!ok(
            READ_SESSION,
            json!({"session_id": "b", "last_turns": 11})
        ));
        assert!(!ok(READ_SESSION, json!({})));

        assert!(ok(START_SESSION, json!({"prompt": "fix it"})));
        assert!(ok(
            START_SESSION,
            json!({"prompt": "fix it", "title": "Fix", "harness": "codex", "model": "m",
                   "effort": "high", "worktree": false, "project": "/abs/repo"})
        ));
        assert!(!ok(START_SESSION, json!({"prompt": " "})));
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(!ok(START_SESSION, json!({"prompt": long})));
        assert!(!ok(
            START_SESSION,
            json!({"prompt": "a", "project": "relative/path"})
        ));
        assert!(!ok(
            START_SESSION,
            json!({"prompt": "a", "worktree": "yes"})
        ));
        assert!(!ok(START_SESSION, json!({"prompt": "a", "harness": ""})));
        let title = "t".repeat(MAX_TITLE_CHARS + 1);
        assert!(!ok(START_SESSION, json!({"prompt": "a", "title": title})));

        let two = json!([{"harness": "codex"}, {"harness": "claude-code", "model": "m"}]);
        assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": two})));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": [{"harness": "codex"}]})
        ));
        let five = json!([{"harness": "a"}, {"harness": "b"}, {"harness": "c"},
                          {"harness": "d"}, {"harness": "e"}]);
        assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": five})));
        let twice = json!([{"harness": "codex"}, {"harness": "codex"}]);
        assert!(!ok(FAN_OUT, json!({"prompt": "a", "harnesses": twice})));
        // The same harness on two models is a fair comparison.
        let models =
            json!([{"harness": "codex", "model": "a"}, {"harness": "codex", "model": "b"}]);
        assert!(ok(FAN_OUT, json!({"prompt": "a", "harnesses": models})));
        assert!(!ok(
            FAN_OUT,
            json!({"prompt": "a", "harnesses": [{"model": "x"}, {"harness": "b"}]})
        ));
        assert!(!ok(FAN_OUT, json!({"harnesses": two})));

        assert_eq!(timeout(START_SESSION, &json!({})), START_TIMEOUT);
        assert_eq!(timeout(FAN_OUT, &json!({})), START_TIMEOUT);
        assert_eq!(timeout(READ_SESSION, &json!({})), REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn a_version_one_client_never_gets_a_version_two_call() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let args = json!({"prompt": "go"});
        let refused = execute_versioned(
            client(json!({"session_id": "c"})),
            1,
            &cancel,
            START_SESSION,
            &args,
        )
        .await;
        assert!(!refused.success);
        assert!(refused.content.contains("Nothing was sent"));
        let started = execute_versioned(
            client(json!({"session_id": "c"})),
            2,
            &cancel,
            START_SESSION,
            &args,
        )
        .await;
        assert!(started.success, "{}", started.content);
    }

    #[test]
    fn the_version_two_hint_stays_short() {
        let tokens = crate::network::compaction::estimate_tokens(PROMPT_HINT_V2);
        assert!(tokens < 90, "{tokens}");
        for name in tool_names(2) {
            assert!(PROMPT_HINT_V2.contains(name), "{name}");
        }
    }

    #[test]
    fn arguments_are_checked_before_anything_is_sent() {
        assert!(
            validate(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": "hi"})
            )
            .is_ok()
        );
        assert!(validate(MESSAGE_SESSION, &json!({"session_id": "b"})).is_err());
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        assert!(
            validate(
                MESSAGE_SESSION,
                &json!({"session_id": "b", "message": long})
            )
            .is_err()
        );
        assert!(validate(WAIT_SESSION, &json!({})).is_err());
        assert!(validate(WAIT_SESSION, &json!({"link_ids": ["a/1"]})).is_ok());
        assert_eq!(
            timeout(WAIT_SESSION, &json!({"timeout_ms": 9_999_999})),
            Duration::from_millis(MAX_WAIT_MS) + Duration::from_secs(30)
        );
    }

    #[tokio::test]
    async fn calls_go_to_the_host_and_its_errors_come_back_as_failures() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let none = execute_with(None, &cancel, LIST_SESSIONS, &json!({})).await;
        assert!(!none.success);
        assert!(none.content.contains("macOS app"));

        let listed = execute_with(
            client(json!({"sessions": [{"session_id": "b"}]})),
            &cancel,
            LIST_SESSIONS,
            &json!({}),
        )
        .await;
        assert!(listed.success);
        assert!(listed.content.contains("\"b\""));

        let denied = execute_with(
            client(json!({"error": {"code": "denied", "message": "the user said no"}})),
            &cancel,
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"}),
        )
        .await;
        assert!(!denied.success);
        assert_eq!(denied.error_kind, Some(ToolErrorKind::PermissionDenied));

        let invalid = execute_with(client(json!({})), &cancel, MESSAGE_SESSION, &json!({})).await;
        assert_eq!(invalid.error_kind, Some(ToolErrorKind::InvalidArguments));
    }

    #[tokio::test]
    async fn a_cancelled_call_says_a_message_may_have_gone() {
        struct Never;
        impl PreviewClient for Never {
            fn request(&self, _params: Value) -> crate::preview::PreviewFuture {
                Box::pin(std::future::pending())
            }
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let output = execute_with(
            Some(Arc::new(Never) as Arc<dyn PreviewClient>),
            &cancel,
            MESSAGE_SESSION,
            &json!({"session_id": "b", "message": "hi"}),
        )
        .await;
        assert_eq!(output.error_kind, Some(ToolErrorKind::Cancelled));
        assert!(output.content.contains("may have been delivered"));
    }
}
