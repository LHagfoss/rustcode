//! Threads that message other threads (#2637), milestone 1 of
//! `docs/architecture/cross-session-orchestration.md`: an agent lists the
//! threads of its own project, sends one a message and waits for its answer.
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

pub const METHOD: &str = "_rustcode/orchestration";
pub const LIST_SESSIONS: &str = "list_sessions";
pub const MESSAGE_SESSION: &str = "message_session";
pub const WAIT_SESSION: &str = "wait_session";
pub const TOOL_NAMES: [&str; 3] = [LIST_SESSIONS, MESSAGE_SESSION, WAIT_SESSION];

/// The most text a message or an answer carries.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// How long `wait_session` waits by default, and at most.
pub const DEFAULT_WAIT_MS: u64 = 60_000;
pub const MAX_WAIT_MS: u64 = 3_600_000;
/// How long the client may take on a list or a message: a message can wait
/// for the user to answer the grant.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub const UNAVAILABLE_MESSAGE: &str = "error: messaging other threads needs the RustCode macOS app with \"Let threads message each other\" turned on in Settings. Nothing was sent.";

pub const PROMPT_HINT: &str = "\n\nOther threads: `list_sessions` lists this project's threads; `message_session` sends one a message (the user approves it once, and the thread answers in its own turn); `wait_session` returns that answer. Use them only when the user asks to involve another thread.";

pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// Whether initialize `clientCapabilities._meta` advertises the host.
pub fn client_meta_supports(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("orchestration"))
        .and_then(Value::as_u64)
        .is_some_and(|version| version >= 1)
}

/// Check the arguments before they leave the engine.
pub fn validate(name: &str, args: &Value) -> Result<(), String> {
    match name {
        LIST_SESSIONS => Ok(()),
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
    if name == WAIT_SESSION {
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
    let client = state.lock().await.orchestration_client.clone();
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
        assert!(client_meta_supports(
            meta(json!({"rustcode": {"orchestration": 1}})).as_ref()
        ));
        assert!(!client_meta_supports(
            meta(json!({"rustcode": {"preview": 1}})).as_ref()
        ));
        assert!(!client_meta_supports(None));
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
