//! Thread names on the wire (#2632).
//!
//! Protocol v1 names a conversation a "session" (`session_id`,
//! `list_sessions`, `kind: "sessions"`, …). A device that asks for it in the
//! handshake ([`super::handshake::WIRE_NAMES_THREAD`]) reads and writes the
//! same frames with "thread" in those names instead. Only the names change:
//! the frames, their order and their meaning are v1's, and the gateway, the
//! router and the owners keep working in v1 names. This module translates at
//! the device's socket, in both directions.
//!
//! Only the names the v1 schema defines are translated: the object keys
//! below, and the tag values below under `type`, `kind`, `code` and
//! `reason`. Text and other data inside a frame are never touched, even when
//! they contain the word.

use serde_json::Value;

/// Object keys, as `(v1, thread)`.
const KEYS: &[(&str, &str)] = &[
    ("session_id", "thread_id"),
    ("session", "thread"),
    ("sessions", "threads"),
];

/// Fields whose string value is a protocol tag.
const TAG_FIELDS: &[&str] = &["type", "kind", "code", "reason"];

/// Tag values, as `(v1, thread)`.
const TAGS: &[(&str, &str)] = &[
    ("attach_session", "attach_thread"),
    ("create_session", "create_thread"),
    ("detach_session", "detach_thread"),
    ("list_saved_sessions", "list_saved_threads"),
    ("list_sessions", "list_threads"),
    ("resume_session", "resume_thread"),
    ("saved_sessions", "saved_threads"),
    ("session_changed", "thread_changed"),
    ("session_closed", "thread_closed"),
    ("session_created", "thread_created"),
    ("session_resumed", "thread_resumed"),
    ("session_settings_updated", "thread_settings_updated"),
    ("sessions", "threads"),
    ("set_session_settings", "set_thread_settings"),
    ("stale_session", "stale_thread"),
    ("subscribe_sessions", "subscribe_threads"),
];

#[derive(Clone, Copy)]
enum Direction {
    ToThread,
    ToSession,
}

fn rename<'a>(
    table: &'a [(&'a str, &'a str)],
    name: &str,
    direction: Direction,
) -> Option<&'a str> {
    table.iter().find_map(|(v1, thread)| match direction {
        Direction::ToThread => (*v1 == name).then_some(*thread),
        Direction::ToSession => (*thread == name).then_some(*v1),
    })
}

fn translate(value: &mut Value, direction: Direction) {
    match value {
        Value::Object(fields) => {
            let renamed: Vec<(String, String)> = fields
                .keys()
                .filter_map(|key| {
                    rename(KEYS, key, direction).map(|new| (key.clone(), new.to_owned()))
                })
                .collect();
            for (old, new) in renamed {
                if let Some(field) = fields.remove(&old) {
                    fields.insert(new, field);
                }
            }
            for (key, field) in fields.iter_mut() {
                if TAG_FIELDS.contains(&key.as_str())
                    && let Value::String(tag) = field
                {
                    if let Some(new) = rename(TAGS, tag, direction) {
                        *tag = new.to_owned();
                    }
                    continue;
                }
                translate(field, direction);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| translate(item, direction)),
        _ => {}
    }
}

/// A frame the gateway sends, in thread names. Text that is not JSON is
/// passed on as it is.
pub fn to_thread_names(frame: &str) -> String {
    rewrite(frame, Direction::ToThread)
}

/// A frame a device sent in thread names, in v1 names. Text that is not
/// JSON is passed on as it is, so the router reports it as usual.
pub fn to_session_names(frame: &str) -> String {
    rewrite(frame, Direction::ToSession)
}

fn rewrite(frame: &str, direction: Direction) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(frame) else {
        return frame.to_owned();
    };
    translate(&mut value, direction);
    serde_json::to_string(&value).unwrap_or_else(|_| frame.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn thread(value: Value) -> Value {
        serde_json::from_str(&to_thread_names(&value.to_string())).unwrap()
    }

    fn session(value: Value) -> Value {
        serde_json::from_str(&to_session_names(&value.to_string())).unwrap()
    }

    #[test]
    fn requests_in_thread_names_reach_the_router_in_v1_names() {
        let sent = json!({
            "protocol_version": 1,
            "request_id": "r1",
            "type": "attach_thread",
            "thread_id": "01a1",
        });
        assert_eq!(
            session(sent),
            json!({"protocol_version": 1, "request_id": "r1", "type": "attach_session", "session_id": "01a1"})
        );
    }

    #[test]
    fn frames_reach_the_device_in_thread_names() {
        let frame = json!({
            "kind": "sessions",
            "sessions": [{"session_id": "a", "title": "Fix the session store"}],
            "event": {"type": "session_closed", "reason": "session_changed"},
            "error": {"code": "stale_session", "message": "this session moved"},
        });
        assert_eq!(
            thread(frame),
            json!({
                "kind": "threads",
                "threads": [{"thread_id": "a", "title": "Fix the session store"}],
                "event": {"type": "thread_closed", "reason": "thread_changed"},
                "error": {"code": "stale_thread", "message": "this session moved"},
            })
        );
    }

    #[test]
    fn text_and_unknown_names_are_left_alone() {
        // A prompt or title that happens to be a tag is still text.
        let request = json!({"type": "submit", "prompt": "list_threads", "title": "threads"});
        assert_eq!(session(request.clone()), request);
        // Keys and tags the schema does not define stay as they are.
        let frame = json!({"type": "session_note", "session_count": 2});
        assert_eq!(thread(frame.clone()), frame);
        assert_eq!(to_session_names("not json"), "not json");
    }

    #[test]
    fn every_name_round_trips() {
        for (v1, renamed) in TAGS {
            let frame = json!({"type": v1});
            assert_eq!(thread(frame.clone()), json!({"type": renamed}));
            assert_eq!(session(thread(frame.clone())), frame);
        }
        for (v1, renamed) in KEYS {
            let frame = json!({ *v1: 1 });
            assert_eq!(thread(frame.clone()), json!({ *renamed: 1 }));
            assert_eq!(session(thread(frame.clone())), frame);
        }
    }

    /// Every name with "session" in it that the v1 schema defines is in the
    /// tables above, so a new request or frame cannot reach a device in the
    /// old names by accident.
    #[test]
    fn the_tables_cover_every_session_name_in_the_schema() {
        fn walk(node: &Value, keys: &mut Vec<String>, tags: &mut Vec<String>) {
            match node {
                Value::Object(fields) => {
                    if let Some(Value::Object(properties)) = fields.get("properties") {
                        for (name, property) in properties {
                            if name.contains("session") {
                                keys.push(name.clone());
                            }
                            if TAG_FIELDS.contains(&name.as_str()) {
                                let values = property.get("const").into_iter().chain(
                                    property
                                        .get("enum")
                                        .and_then(Value::as_array)
                                        .into_iter()
                                        .flatten(),
                                );
                                for value in values.filter_map(Value::as_str) {
                                    if value.contains("session") {
                                        tags.push(value.to_owned());
                                    }
                                }
                            }
                        }
                    }
                    if let Some(Value::String(value)) = fields.get("const")
                        && value.contains("session")
                    {
                        tags.push(value.clone());
                    }
                    fields.values().for_each(|field| walk(field, keys, tags));
                }
                Value::Array(items) => items.iter().for_each(|item| walk(item, keys, tags)),
                _ => {}
            }
        }
        let (mut keys, mut tags) = (Vec::new(), Vec::new());
        for (_, document) in crate::remote::contract::schema_documents() {
            walk(
                &serde_json::from_str(&document).unwrap(),
                &mut keys,
                &mut tags,
            );
        }
        assert!(!tags.is_empty() && !keys.is_empty());
        for key in keys {
            assert!(
                KEYS.iter().any(|(v1, _)| *v1 == key),
                "key {key} is not translated"
            );
        }
        for tag in tags {
            assert!(
                TAGS.iter().any(|(v1, _)| *v1 == tag),
                "tag {tag} is not translated"
            );
        }
    }
}
