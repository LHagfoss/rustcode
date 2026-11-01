//! Client-owned native desktop automation. The client enforces user consent
//! and snapshot provenance; the engine never executes desktop commands itself.
use crate::tools::{ToolErrorKind, ToolExecutionOutput};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub const TOOL_NAME: &str = "desktop_control";
pub const METHOD: &str = "_rustcode/desktop";
pub const PROMPT_HINT: &str = "\n# Desktop control\nUse `desktop_control` for native macOS applications only when explicitly requested. Snapshot the named app before acting on its refs, then observe again to verify the result. The user must approve each operation; denial or cancellation is not success.\n";

pub(crate) fn supported(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|m| m.get("rustcode"))
        .and_then(|m| m.get("desktop"))
        .and_then(Value::as_u64)
        .is_some_and(|v| v >= 1)
}
pub(crate) fn schema() -> Value {
    json!({"type":"object","properties":{
        "action":{"type":"string","enum":["snapshot","click","type"]},
        "app":{"type":"string","description":"Exact app name"},
        "ref":{"type":"string","description":"Snapshot-qualified element ref"},
        "value":{"type":"string","description":"Text to type"},
        "window_id":{"type":"string","description":"Optional window ID for snapshot"}
    },"required":["action","app"],"additionalProperties":false})
}
fn validate(args: &Value) -> Result<(), &'static str> {
    let obj = args.as_object().ok_or("expected an object")?;
    let action = obj
        .get("action")
        .and_then(Value::as_str)
        .ok_or("action is required")?;
    let keys: &[&str] = match action {
        "snapshot" => &["action", "app", "window_id"],
        "click" => &["action", "app", "ref"],
        "type" => &["action", "app", "ref", "value"],
        _ => return Err("unsupported action"),
    };
    if obj.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err("unexpected argument");
    }
    let valid = |key: &str, empty: bool| {
        obj.get(key).and_then(Value::as_str).is_some_and(|s| {
            s.len() <= 4096 && !s.contains('\0') && (empty || !s.trim().is_empty())
        })
    };
    if !valid("app", false) {
        return Err("app is required");
    }
    if obj.contains_key("window_id") && !valid("window_id", false) {
        return Err("invalid window_id");
    }
    if action != "snapshot" && !valid("ref", false) {
        return Err("ref is required");
    }
    if action == "type" && !valid("value", true) {
        return Err("value is required");
    }
    Ok(())
}
pub(crate) async fn execute(
    state: &Arc<Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
) -> ToolExecutionOutput {
    let fail = |message: String, kind| ToolExecutionOutput::failure_with_kind(message, kind, false);
    if let Err(reason) = validate(args) {
        return fail(
            format!("error: invalid desktop_control call: {reason}"),
            ToolErrorKind::InvalidArguments,
        );
    }
    let Some(client) = state.lock().await.desktop_client.clone() else {
        return fail(
            "error: desktop control is unavailable in this client".into(),
            ToolErrorKind::UnavailableDependency,
        );
    };
    let request = client.request(json!({"operation": args}));
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return fail("error: desktop operation cancelled; its outcome is unknown".into(), ToolErrorKind::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(150), request) => result,
    };
    match result {
        Ok(Ok(value)) if value.get("ok").and_then(Value::as_bool) == Some(true) => {
            ToolExecutionOutput::success(value.to_string())
        }
        Ok(Ok(value)) => fail(
            format!("error: desktop operation failed: {value}"),
            ToolErrorKind::Validation,
        ),
        Ok(Err(err)) => fail(
            format!("error: desktop client failed; outcome unknown: {err}"),
            ToolErrorKind::Unknown,
        ),
        Err(_) => fail(
            "error: desktop operation timed out; outcome unknown".into(),
            ToolErrorKind::Unknown,
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_actions_are_denied_in_plan_mode() {
        assert!(!crate::tools::allowed_in_plan_mode(TOOL_NAME));
    }

    #[test]
    fn validates_narrow_operations() {
        assert!(validate(&json!({"action":"snapshot","app":"Finder"})).is_ok());
        assert!(validate(&json!({"action":"click","app":"Finder","ref":"@s:e1"})).is_ok());
        assert!(validate(&json!({"action":"click","app":"Finder"})).is_err());
        assert!(validate(&json!({"action":"snapshot","app":"Finder","headed":true})).is_err());
        assert!(
            validate(&json!({"action":"type","app":"Finder","ref":"@s:e1","value":"a\0b"}))
                .is_err()
        );
    }
}
