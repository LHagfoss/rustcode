//! What a job's webhook delivery carries besides its body (#4386): the event
//! name and delivery id from the request headers (`X-GitHub-Event`,
//! `X-GitHub-Delivery`, or the generic `X-Event-Type`), and which events a job
//! takes (`webhook_events`).
//!
//! The listener wraps a delivery as a JSON envelope in the run's input, so the
//! store and the protocol keep one `input` string: [`Delivery::to_input`]
//! writes it and [`Delivery::from_input`] reads it back, taking an input
//! without the envelope (a run from before #4386) as a bare body.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const ENVELOPE_KEY: &str = "rustcode_webhook";
/// How many delivery ids a job remembers to drop a redelivery.
pub const KEPT_DELIVERIES: usize = 200;
/// The longest event filter entry and how many a job may have.
const MAX_EVENT_LEN: usize = 100;
const MAX_EVENTS: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    #[serde(default)]
    pub body: String,
}

fn header_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 200)
        .filter(|value| value.chars().all(|c| !c.is_control()))
        .map(str::to_owned)
}

impl Delivery {
    pub fn new(event: Option<&str>, delivery: Option<&str>, body: String) -> Self {
        Self {
            event: header_value(event),
            delivery: header_value(delivery),
            body,
        }
    }

    pub fn to_input(&self) -> String {
        serde_json::json!({ ENVELOPE_KEY: self }).to_string()
    }

    pub fn from_input(input: &str) -> Self {
        serde_json::from_str::<Value>(input)
            .ok()
            .and_then(|value| value.get(ENVELOPE_KEY).cloned())
            .and_then(|envelope| serde_json::from_value(envelope).ok())
            .unwrap_or_else(|| Self {
                body: input.to_owned(),
                ..Self::default()
            })
    }

    /// The payload's `action` (`opened`, `completed`, …), when it has one.
    fn action(&self) -> Option<String> {
        serde_json::from_str::<Value>(&self.body)
            .ok()?
            .get("action")?
            .as_str()
            .map(str::to_owned)
    }

    /// Whether a job filtered on `events` takes this delivery. An empty filter
    /// takes every delivery; an entry is an event (`check_suite`) or an event
    /// and its action (`pull_request.review_requested`).
    pub fn matches(&self, events: &[String]) -> bool {
        if events.is_empty() {
            return true;
        }
        let Some(event) = self.event.as_deref() else {
            return false;
        };
        let action = self.action();
        events.iter().any(|wanted| match wanted.split_once('.') {
            Some((name, wanted_action)) => {
                name == event && action.as_deref() == Some(wanted_action)
            }
            None => wanted == event,
        })
    }

    /// The line the run's prompt names the delivery with.
    pub fn heading(&self) -> Option<String> {
        let event = self.event.as_deref()?;
        let action = self
            .action()
            .map(|action| format!(" ({action})"))
            .unwrap_or_default();
        let delivery = self
            .delivery
            .as_deref()
            .map(|id| format!(", delivery {id}"))
            .unwrap_or_default();
        Some(format!("Webhook event `{event}`{action}{delivery}."))
    }

    /// What the job's pre-check script sees: the event, the delivery and the
    /// path of a private file that holds the body.
    pub fn script_env(&self, body_file: &std::path::Path) -> Vec<(String, String)> {
        let mut env = vec![(
            "RUSTCODE_WEBHOOK_BODY_FILE".to_owned(),
            body_file.to_string_lossy().into_owned(),
        )];
        if let Some(event) = &self.event {
            env.push(("RUSTCODE_WEBHOOK_EVENT".to_owned(), event.clone()));
        }
        if let Some(delivery) = &self.delivery {
            env.push(("RUSTCODE_WEBHOOK_DELIVERY".to_owned(), delivery.clone()));
        }
        env
    }
}

/// A job's event filter as saved: trimmed, without empty or repeated entries.
pub fn clean_events(events: &[String]) -> Result<Vec<String>, String> {
    let mut cleaned: Vec<String> = Vec::new();
    for event in events.iter().map(|event| event.trim()) {
        if event.is_empty() || cleaned.iter().any(|kept| kept == event) {
            continue;
        }
        let ok = event.len() <= MAX_EVENT_LEN
            && event
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
        if !ok {
            return Err(format!(
                "webhook event \"{event}\" is not an event name (letters, digits, '_', '-', and '.' before an action)"
            ));
        }
        cleaned.push(event.to_owned());
    }
    if cleaned.len() > MAX_EVENTS {
        return Err(format!("at most {MAX_EVENTS} webhook events"));
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery(event: Option<&str>, body: &str) -> Delivery {
        Delivery::new(event, Some("abc-1"), body.to_owned())
    }

    #[test]
    fn a_delivery_round_trips_through_the_run_input_and_a_bare_body_stays_a_body() {
        let sent = delivery(Some("check_suite"), r#"{"action":"completed"}"#);
        assert_eq!(Delivery::from_input(&sent.to_input()), sent);
        let legacy = Delivery::from_input(r#"{"action":"opened"}"#);
        assert_eq!(legacy.event, None);
        assert_eq!(legacy.body, r#"{"action":"opened"}"#);
        assert_eq!(Delivery::from_input("plain text").body, "plain text");
    }

    #[test]
    fn headers_that_are_empty_long_or_carry_control_characters_are_dropped() {
        let d = Delivery::new(Some("  "), Some("a\nb"), String::new());
        assert_eq!((d.event, d.delivery), (None, None));
        let long = "x".repeat(201);
        assert_eq!(Delivery::new(Some(&long), None, String::new()).event, None);
        assert_eq!(
            Delivery::new(Some(" push "), None, String::new()).event,
            Some("push".to_owned())
        );
    }

    #[test]
    fn a_filter_takes_an_event_or_an_event_with_its_action() {
        let events = vec![
            "check_suite".to_owned(),
            "pull_request.review_requested".to_owned(),
        ];
        assert!(delivery(Some("check_suite"), "{}").matches(&events));
        assert!(
            delivery(Some("pull_request"), r#"{"action":"review_requested"}"#).matches(&events)
        );
        assert!(!delivery(Some("pull_request"), r#"{"action":"opened"}"#).matches(&events));
        assert!(!delivery(Some("push"), "{}").matches(&events));
        assert!(!delivery(None, "{}").matches(&events));
        assert!(delivery(None, "{}").matches(&[]));
    }

    #[test]
    fn the_heading_names_the_event_action_and_delivery() {
        assert_eq!(
            delivery(Some("check_suite"), r#"{"action":"completed"}"#).heading(),
            Some("Webhook event `check_suite` (completed), delivery abc-1.".to_owned())
        );
        assert_eq!(delivery(None, "{}").heading(), None);
    }

    #[test]
    fn the_script_gets_the_body_file_event_and_delivery() {
        let env = delivery(Some("push"), "{}").script_env(std::path::Path::new("/tmp/body"));
        assert!(env.contains(&(
            "RUSTCODE_WEBHOOK_BODY_FILE".to_owned(),
            "/tmp/body".to_owned()
        )));
        assert!(env.contains(&("RUSTCODE_WEBHOOK_EVENT".to_owned(), "push".to_owned())));
        assert!(env.contains(&("RUSTCODE_WEBHOOK_DELIVERY".to_owned(), "abc-1".to_owned())));
    }

    #[test]
    fn a_saved_filter_is_trimmed_deduplicated_and_checked() {
        let input = vec![
            " push ".to_owned(),
            "push".to_owned(),
            String::new(),
            "pull_request.opened".to_owned(),
        ];
        assert_eq!(
            clean_events(&input).unwrap(),
            vec!["push".to_owned(), "pull_request.opened".to_owned()]
        );
        assert!(clean_events(&["push; rm".to_owned()]).is_err());
        let many: Vec<String> = (0..33).map(|n| format!("e{n}")).collect();
        assert!(clean_events(&many).is_err());
    }
}
