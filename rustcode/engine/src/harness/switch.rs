//! A thread's model or effort changing part way through: a marker in the
//! history, so the change stays visible where it happened after a reload, and
//! the divider the client draws for it. Picks before the first message are
//! only the thread's starting choice and record nothing; several picks with no
//! message between them are one change.

use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
use rustcode_core::ChatMessage;
use serde_json::{Value, json};

pub(crate) const ROLE: &str = "switch";

/// A thread's model and effort at one moment.
pub(crate) type Choice = (Option<String>, Option<String>);

/// Record the change from `before` to `after` in `history`, and return the
/// update to show. A pick that undoes the pending change removes its marker,
/// and the update then says nothing changed so the client drops its divider.
pub(crate) fn note(
    history: &mut Vec<ChatMessage>,
    before: &Choice,
    after: &Choice,
) -> Option<SessionUpdate> {
    if before == after || !history.iter().any(|message| message.role == "user") {
        return None;
    }
    // Merges into a change no message has come after yet.
    let from = match history.last() {
        Some(last) if last.role == ROLE => {
            let pending = history.pop().and_then(|marker| parse(&marker));
            pending.map_or_else(|| before.clone(), |(from, _)| from)
        }
        _ => before.clone(),
    };
    let marker = marker(&from, after);
    let update = divider(&marker);
    if from != *after {
        history.push(marker);
    }
    update
}

fn marker(from: &Choice, to: &Choice) -> ChatMessage {
    let mut message = ChatMessage::new(
        ROLE,
        json!({
            "model_from": from.0, "model_to": to.0,
            "effort_from": from.1, "effort_to": to.1,
        })
        .to_string(),
    );
    message.timestamp = chrono::Local::now().to_rfc3339();
    message
}

fn parse(marker: &ChatMessage) -> Option<(Choice, Choice)> {
    let value = serde_json::from_str::<Value>(&marker.content).ok()?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some((
        (text("model_from"), text("effort_from")),
        (text("model_to"), text("effort_to")),
    ))
}

/// The divider the client draws for a recorded change.
pub(crate) fn divider(marker: &ChatMessage) -> Option<SessionUpdate> {
    let (from, to) = parse(marker)?;
    let mut changes = Vec::new();
    if from.0 != to.0
        && let Some(model) = &to.0
    {
        changes.push(format!("model to {model}"));
    }
    if from.1 != to.1
        && let Some(effort) = &to.1
    {
        changes.push(format!("effort to {effort}"));
    }
    let text = format!("Changed {}", changes.join(" and "));
    let mut meta = serde_json::Map::new();
    meta.insert(
        "rustcode".to_owned(),
        json!({
            "kind": "switch",
            "model_from": from.0, "model_to": to.0,
            "effort_from": from.1, "effort_to": to.1,
        }),
    );
    Some(SessionUpdate::AgentMessageChunk(
        ContentChunk::new(ContentBlock::Text(TextContent::new(text))).meta(meta),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(model: &str, effort: &str) -> Choice {
        (Some(model.to_owned()), Some(effort.to_owned()))
    }

    fn started() -> Vec<ChatMessage> {
        vec![ChatMessage::new("user", "hello".to_owned())]
    }

    #[test]
    fn a_pick_before_the_first_message_records_nothing() {
        let mut history = Vec::new();
        assert!(note(&mut history, &choice("a", "low"), &choice("b", "low")).is_none());
        assert!(history.is_empty());
    }

    #[test]
    fn the_same_choice_records_nothing() {
        let mut history = started();
        assert!(note(&mut history, &choice("a", "low"), &choice("a", "low")).is_none());
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn a_change_replays_as_a_divider() {
        let mut history = started();
        note(&mut history, &choice("a", "low"), &choice("b", "high")).unwrap();
        let value = serde_json::to_value(divider(history.last().unwrap()).unwrap()).unwrap();
        assert_eq!(value["_meta"]["rustcode"]["kind"], "switch");
        assert_eq!(value["_meta"]["rustcode"]["model_from"], "a");
        assert_eq!(value["_meta"]["rustcode"]["effort_to"], "high");
        assert_eq!(
            value["content"]["text"],
            "Changed model to b and effort to high"
        );
    }

    #[test]
    fn picks_with_no_message_between_merge() {
        let mut history = started();
        note(&mut history, &choice("a", "low"), &choice("b", "low")).unwrap();
        note(&mut history, &choice("b", "low"), &choice("c", "low")).unwrap();
        assert_eq!(history.len(), 2);
        let (from, to) = parse(history.last().unwrap()).unwrap();
        assert_eq!(from, choice("a", "low"));
        assert_eq!(to, choice("c", "low"));
    }

    #[test]
    fn undoing_a_pending_change_removes_it_and_tells_the_client() {
        let mut history = started();
        note(&mut history, &choice("a", "low"), &choice("b", "low")).unwrap();
        let update = note(&mut history, &choice("b", "low"), &choice("a", "low")).unwrap();
        assert_eq!(history.len(), 1);
        let value = serde_json::to_value(update).unwrap();
        assert_eq!(
            value["_meta"]["rustcode"]["model_from"],
            value["_meta"]["rustcode"]["model_to"]
        );
    }

    #[test]
    fn a_new_message_starts_a_new_change() {
        let mut history = started();
        note(&mut history, &choice("a", "low"), &choice("b", "low")).unwrap();
        history.push(ChatMessage::new("user", "more".to_owned()));
        note(&mut history, &choice("b", "low"), &choice("c", "low")).unwrap();
        assert_eq!(history.iter().filter(|m| m.role == ROLE).count(), 2);
    }
}
