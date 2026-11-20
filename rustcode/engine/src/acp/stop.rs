//! Stop, traced by attempt (issue 4086). `session/cancel` is a notification,
//! so nothing told the app whether a Stop reached the engine or the harness.
//! `_rustcode/session/stop` does the same cancel and answers where it went;
//! both log `stop.received` and `stop.delivered` under the app's attempt id,
//! and the harness logs how it answered (`stop.harness_cancel`,
//! `stop.harness_stopped`). Ids and reason codes only, never prompt text.

use super::{AcpServer, harnesses};
use serde_json::{Value, json};

/// `_rustcode/session/stop`: stop the thread's running turn and say where the
/// stop went.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/session/stop", response = serde_json::Value)]
pub(crate) struct StopRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    /// The app's id for this Stop press, logged with every stage.
    #[serde(default)]
    pub attempt: Option<String>,
}

/// The longest attempt id kept: the app sends a short one.
const MAX_ATTEMPT_CHARS: usize = 64;

/// The attempt id when it is a short token, so a log line never carries
/// anything else.
pub(super) fn attempt_id(raw: Option<&str>) -> Option<&str> {
    raw.filter(|id| {
        !id.is_empty()
            && id.len() <= MAX_ATTEMPT_CHARS
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

/// The attempt id `session/cancel` carries in `_meta.rustcode.stopAttempt`.
pub(super) fn notification_attempt(
    meta: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    let raw = meta?.get("rustcode")?.get("stopAttempt")?.as_str();
    attempt_id(raw).map(str::to_owned)
}

/// Stop `session_id`'s turn: its external harness, else the built-in turn.
/// `via` names the method that asked (`request`, `notification`). Answers
/// `{route, active}`: `harness`, `builtin` or `none` (no such thread here),
/// and whether a turn was running to stop.
pub(super) async fn stop(
    server: &AcpServer,
    session_id: &str,
    attempt: Option<&str>,
    via: &'static str,
) -> Value {
    let attempt = attempt_id(attempt);
    event("stop.received", session_id, attempt, via, None, None);
    let (route, active) = if let Some(external) = harnesses::get(&server.external, session_id).await
    {
        ("harness", external.stop(attempt))
    } else {
        let turns = server
            .sessions
            .lock()
            .await
            .get(session_id)
            .map(|session| std::sync::Arc::clone(&session.turns));
        match turns {
            Some(turns) => ("builtin", turns.cancel_active().await),
            None => ("none", false),
        }
    };
    if route != "harness" {
        crate::tools::abort_background_starts(session_id);
    }
    event(
        "stop.delivered",
        session_id,
        attempt,
        via,
        Some(route),
        Some(active),
    );
    json!({ "route": route, "active": active })
}

fn event(
    name: &str,
    session_id: &str,
    attempt: Option<&str>,
    via: &str,
    route: Option<&str>,
    active: Option<bool>,
) {
    crate::logger::operational_event(
        name,
        json!({
            "session_id": session_id,
            "attempt": attempt,
            "via": via,
            "route": route,
            "active": active,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_short_token_attempt_ids_are_kept() {
        assert_eq!(attempt_id(Some("s-1a2b_3")), Some("s-1a2b_3"));
        assert_eq!(attempt_id(Some("")), None);
        assert_eq!(attempt_id(Some("has space")), None);
        assert_eq!(attempt_id(Some(&"x".repeat(65))), None);
        assert_eq!(attempt_id(None), None);
    }

    #[test]
    fn a_cancel_names_its_attempt_in_meta() {
        let meta = json!({ "rustcode": { "stopAttempt": "s-7" } });
        assert_eq!(
            notification_attempt(meta.as_object()).as_deref(),
            Some("s-7")
        );
        assert_eq!(notification_attempt(None), None);
        let other = json!({ "rustcode": { "stopAttempt": "a b" } });
        assert_eq!(notification_attempt(other.as_object()), None);
    }
}
