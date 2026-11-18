//! A harness's own refusal, told apart from an ordinary failed call.
//!
//! Every harness has a permission layer of its own that RustCode does not run.
//! Claude Code's auto mode, for one, has a classifier that refuses a call
//! without sending `session/request_permission`, so RustCode never sees a
//! request to answer: the call just fails, and its text says why. Left as it
//! is, the thread shows a failed call and the user learns only from the
//! agent's reply that a layer RustCode's access level does not control said
//! no (`docs/permissions.md`).
//!
//! This reads the failed call's text against a short table of known refusals.
//! A match is marked on the update (`_meta.rustcode.harnessDenial`, which the
//! app draws on the tool row) and counted as a `harness.permission_denied`
//! operational event. Only the reason is kept, never the call's input.

use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus, ToolCallUpdate,
};
use serde_json::{Value, json};

/// The operational event a harness's refusal is recorded as.
pub(crate) const DENIAL_EVENT: &str = "harness.permission_denied";

/// One harness refusal: who refused, the reason it gave, and where it is
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Denial {
    /// The harness's name, as the row says it: "Claude Code".
    pub(crate) by: &'static str,
    /// Which of its layers: "auto mode".
    pub(crate) layer: &'static str,
    /// The reason the harness gave, when it gave one.
    pub(crate) reason: Option<String>,
    /// Where the user changes it, in words.
    pub(crate) change: &'static str,
}

/// A refusal one harness writes into a failed call's result.
struct Known {
    harness: &'static str,
    by: &'static str,
    layer: &'static str,
    /// Text the result contains.
    marker: &'static str,
    change: &'static str,
}

const KNOWN: &[Known] = &[Known {
    harness: "claude-code",
    by: "Claude Code",
    layer: "auto mode",
    marker: "denied by the Claude Code auto mode classifier",
    change: "Claude Code's own permission mode decides this, not RustCode's access level: run the step yourself, or change the mode in Claude Code's settings (`permissions.defaultMode` in its settings.json)",
}];

/// The refusal a failed call's text shows, for the harness that ran it.
pub(crate) fn detect(harness: &str, text: &str) -> Option<Denial> {
    let known = KNOWN
        .iter()
        .find(|known| known.harness == harness && text.contains(known.marker))?;
    Some(Denial {
        by: known.by,
        layer: known.layer,
        reason: reason_of(text),
        change: known.change,
    })
}

/// `Reason: [Undermining Oversight]` → `Undermining Oversight`.
fn reason_of(text: &str) -> Option<String> {
    let start = text.find("Reason: [")? + "Reason: [".len();
    let end = text[start..].find(']')?;
    let reason = text[start..start + end].trim();
    (!reason.is_empty() && reason.len() <= 120).then(|| reason.to_owned())
}

/// A failed call's text, when this update fails one.
fn failed_text(update: &ToolCallUpdate) -> Option<String> {
    if update.fields.status != Some(ToolCallStatus::Failed) {
        return None;
    }
    let content = update.fields.content.as_ref()?;
    let text: Vec<&str> = content
        .iter()
        .filter_map(|item| match item {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

/// Mark a harness's refusal on its update and count it against the thread.
/// Any other update passes through as it came.
pub(crate) fn mark(update: SessionUpdate, thread_id: &str, harness: &str) -> SessionUpdate {
    let SessionUpdate::ToolCallUpdate(mut call) = update else {
        return update;
    };
    let Some(denial) = failed_text(&call).and_then(|text| detect(harness, &text)) else {
        return SessionUpdate::ToolCallUpdate(call);
    };
    let meta = call.meta.get_or_insert_with(serde_json::Map::new);
    let rustcode = meta
        .entry("rustcode")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Value::Object(rustcode) = rustcode {
        rustcode.insert(
            "harnessDenial".to_owned(),
            json!({
                "by": denial.by,
                "layer": denial.layer,
                "reason": denial.reason,
                "change": denial.change,
            }),
        );
    }
    record(thread_id, harness, &denial);
    SessionUpdate::ToolCallUpdate(call)
}

fn record(thread_id: &str, harness: &str, denial: &Denial) {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return;
    }
    let fields = json!({
        "harness": harness,
        "layer": denial.layer,
        "reason": denial.reason,
    });
    let row = crate::data_store::OperationalEventRow {
        thread_id: thread_id.to_owned(),
        turn_id: None,
        event_type: DENIAL_EVENT.to_owned(),
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        fields_json: Some(fields.to_string()),
        original_bytes: None,
        captured_bytes: None,
        payload_truncated: false,
    };
    crate::data_store::record_operational_event(&row);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ToolCallUpdateFields;

    const CLASSIFIER: &str = "Permission for this action was denied by the Claude Code auto mode classifier. Reason: [Undermining Oversight]. If you have other tasks that don't depend on this action, continue working on those.";

    fn failed(text: &str) -> SessionUpdate {
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Failed)
                .content(vec![ToolCallContent::from(text.to_owned())]),
        ))
    }

    fn denial_of(update: &SessionUpdate) -> Option<Value> {
        let SessionUpdate::ToolCallUpdate(call) = update else {
            return None;
        };
        call.meta
            .as_ref()?
            .get("rustcode")?
            .get("harnessDenial")
            .cloned()
    }

    #[test]
    fn the_auto_mode_classifier_is_told_apart_with_its_reason() {
        let denial = detect("claude-code", CLASSIFIER).unwrap();
        assert_eq!(denial.by, "Claude Code");
        assert_eq!(denial.reason.as_deref(), Some("Undermining Oversight"));
        assert!(denial.change.contains("permissions.defaultMode"));
    }

    #[test]
    fn only_the_harness_that_writes_it_matches() {
        assert!(detect("codex", CLASSIFIER).is_none());
        assert!(detect("claude-code", "error: exit status 1").is_none());
    }

    #[test]
    fn a_failed_call_is_marked_and_others_pass_through() {
        let marked = mark(failed(CLASSIFIER), "", "claude-code");
        let denial = denial_of(&marked).unwrap();
        assert_eq!(denial["by"], json!("Claude Code"));
        assert_eq!(denial["reason"], json!("Undermining Oversight"));
        assert!(denial_of(&mark(failed("cargo: not found"), "", "claude-code")).is_none());
        let running = SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "call-2",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::InProgress)
                .content(vec![ToolCallContent::from(CLASSIFIER.to_owned())]),
        ));
        assert!(denial_of(&mark(running, "", "claude-code")).is_none());
    }
}
