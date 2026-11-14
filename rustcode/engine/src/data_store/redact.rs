//! What a captured payload may keep.
//!
//! Telemetry payloads are opt-in (`capture_provider_payloads`,
//! `capture_harness_rpc_payloads`) and even then they are bounded and
//! redacted before they reach the store: a value under a key that names a
//! credential is replaced, a bearer token inside any string is replaced, and
//! the result is clipped to a byte cap while staying valid JSON.
//!
//! [`REDACTION_VERSION`] is stored with every payload-bearing row, so a rule
//! change is visible in the data instead of silently rewriting history.

use serde_json::{Map, Value};

/// The rules in this file, as recorded in `redaction_version`.
pub const REDACTION_VERSION: i64 = 2;

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[redacted]";

/// A dropped array item or object entry says so, in place.
pub const TRUNCATED: &str = "[truncated]";

/// The object key a clipped object adds when it dropped entries.
pub const TRUNCATED_KEY: &str = "__truncated";

/// Byte room kept free while clipping, for the markers the clip adds.
const MARKER_ALLOWANCE: usize = 64;

/// Key name fragments that mean "a credential is in here", matched
/// case-insensitively as a substring.
///
/// Deliberately broad: `max_tokens` is redacted too, because a rule that
/// tries to tell a token limit from a token would be one guess away from
/// writing a secret to disk.
const SENSITIVE_KEY_PARTS: [&str; 8] = [
    "authorization",
    "api_key",
    "apikey",
    "token",
    "secret",
    "credential",
    "password",
    "cookie",
];

/// True when a value under this key must not be stored. `X-API-Key` and
/// `X_API_KEY` are the same key here.
pub fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '.', ' '], "_");
    SENSITIVE_KEY_PARTS.iter().any(|part| key.contains(part))
}

/// A redacted copy of `value`: sensitive keys and bearer strings replaced.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // Browser typing may contain passwords or private form values.
            // Keep the selector, but never persist its text in captured RPCs.
            let browser_type = ["name", "tool", "tool_name"].iter().any(|key| {
                map.get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|name| name == "browser_type" || name.ends_with("__browser_type"))
            });
            let mut out = Map::with_capacity(map.len());
            for (key, value) in map {
                out.insert(
                    key.clone(),
                    // A number, flag or null under a sensitive-looking key is a count or a
                    // setting (`max_tokens`, `input_tokens`), never a secret: keep it.
                    if is_sensitive_key(key)
                        && !matches!(value, Value::Number(_) | Value::Bool(_) | Value::Null)
                    {
                        Value::String(REDACTED.to_owned())
                    } else {
                        let mut redacted = redact(value);
                        if browser_type && matches!(key.as_str(), "arguments" | "args" | "input") {
                            if let Some(text) = redacted.get_mut("text") {
                                *text = Value::String(REDACTED.to_owned());
                            }
                        }
                        redacted
                    },
                );
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        Value::String(text) => Value::String(redact_bearer(text)),
        other => other.clone(),
    }
}

/// Replaces the token of every `Bearer <token>` in `text`, leaving the rest
/// alone. Case-insensitive, idempotent, and usable on a string that is not
/// JSON (an error body, a header line).
pub fn redact_bearer(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // `to_ascii_lowercase` keeps every byte offset, so the offsets found here
    // are offsets in `text`.
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = lower[search..].find("bearer") {
        let word = search + found;
        let after_word = word + "bearer".len();
        let whole_word = text[..word]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_ascii_alphanumeric());
        if !whole_word {
            search = after_word;
            continue;
        }
        let mut token_start = after_word;
        while token_start < text.len() && text.as_bytes()[token_start].is_ascii_whitespace() {
            token_start += 1;
        }
        if token_start >= text.len() {
            break;
        }
        let mut token_end = token_start;
        while token_end < text.len() {
            let ch = text[token_end..].chars().next().unwrap_or(' ');
            if ch.is_whitespace()
                || matches!(ch, '"' | '\'' | ',' | ';' | '}' | ']' | ')' | '[' | '{')
            {
                break;
            }
            token_end += ch.len_utf8();
        }
        if token_end == token_start {
            // Nothing to replace (a bracket, or an empty value): step past one
            // character so the scan always advances.
            search = token_start + text[token_start..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        if &text[token_start..token_end] == REDACTED {
            search = token_end;
            continue;
        }
        out.push_str(&text[copied..token_start]);
        out.push_str(REDACTED);
        copied = token_end;
        search = token_end;
    }
    out.push_str(&text[copied..]);
    out
}

/// Redacts `value`, then clips it to `max_bytes` while keeping it valid JSON.
/// The flag says the payload was clipped.
pub fn prepare_payload(value: &Value, max_bytes: usize) -> (String, bool) {
    clip_json(&redact(value), max_bytes)
}

/// `value` as JSON, at most `max_bytes` long.
///
/// A value that already fits is returned unchanged. A longer one is clipped
/// structurally — long strings cut, array items and object entries dropped —
/// and marked with [`TRUNCATED_KEY`] or [`TRUNCATED`], so the result still
/// parses. The flag says the payload was clipped.
pub fn clip_json(value: &Value, max_bytes: usize) -> (String, bool) {
    let text = value.to_string();
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut room = max_bytes.saturating_sub(MARKER_ALLOWANCE);
    loop {
        let mut budget = room;
        let clipped = clip_value(value, &mut budget).to_string();
        if clipped.len() <= max_bytes {
            return (clipped, true);
        }
        // The structural pass keeps whole numbers and markers it cannot cut,
        // so it can still overshoot: try again with less room.
        if room <= 16 {
            break;
        }
        room /= 2;
    }
    // A cap too small for the value's shape: keep a head of the JSON as a
    // string, which is small and still parses.
    let head = clip_text(&text, max_bytes.saturating_sub(48));
    let fallback = serde_json::json!({
        TRUNCATED_KEY: true,
        "bytes": text.len(),
        "preview": head,
    })
    .to_string();
    if fallback.len() <= max_bytes {
        (fallback, true)
    } else {
        (format!(r#"{{"{TRUNCATED_KEY}":true}}"#), true)
    }
}

/// `text` cut to at most `max_bytes` at a character boundary.
pub fn clip_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut cut = max_bytes.min(text.len());
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_owned()
}

/// One value inside the shared `budget`, which it spends as it goes.
fn clip_value(value: &Value, budget: &mut usize) -> Value {
    match value {
        Value::Null => {
            spend(budget, 4);
            Value::Null
        }
        Value::Bool(true) => {
            spend(budget, 4);
            Value::Bool(true)
        }
        Value::Bool(false) => {
            spend(budget, 5);
            Value::Bool(false)
        }
        Value::Number(number) => {
            spend(budget, number.to_string().len());
            Value::Number(number.clone())
        }
        Value::String(text) => Value::String(clip_string(text, budget)),
        Value::Array(items) => {
            spend(budget, 1);
            let mut out = Vec::with_capacity(items.len());
            let mut dropped = false;
            let mut remaining = items.len().max(1);
            for item in items {
                if *budget <= 3 {
                    dropped = true;
                    break;
                }
                // Each item gets a share, so one long item cannot leave the
                // rest of the array with nothing; what a short one does not
                // use goes back to the pool.
                let share = budget.saturating_sub(1) / remaining;
                let mut child = share;
                let clipped = clip_value(item, &mut child);
                spend(budget, 1 + (share - child));
                out.push(clipped);
                remaining -= 1;
            }
            if dropped {
                out.push(Value::String(TRUNCATED.to_owned()));
            }
            spend(budget, 1);
            Value::Array(out)
        }
        Value::Object(map) => {
            spend(budget, 1);
            let mut out = Map::with_capacity(map.len());
            let mut dropped = false;
            let mut remaining = map.len().max(1);
            for (key, value) in map {
                let cost = key.len() + 4;
                if *budget <= cost + 2 {
                    dropped = true;
                    break;
                }
                // A share per entry, as for arrays: the first long value must
                // not eat the keys after it.
                let share = budget.saturating_sub(cost + 1) / remaining;
                let mut child = share;
                let clipped = clip_value(value, &mut child);
                spend(budget, cost + (share - child));
                out.insert(key.clone(), clipped);
                remaining -= 1;
            }
            if dropped {
                out.insert(TRUNCATED_KEY.to_owned(), Value::Bool(true));
            }
            spend(budget, 1);
            Value::Object(out)
        }
    }
}

/// A string clipped to what is left of the budget, quotes included.
fn clip_string(text: &str, budget: &mut usize) -> String {
    let room = budget.saturating_sub(2);
    if text.len() <= room {
        spend(budget, text.len() + 2);
        return text.to_owned();
    }
    let clipped = clip_text(text, room);
    spend(budget, clipped.len() + 2);
    clipped
}

fn spend(budget: &mut usize, amount: usize) {
    *budget = budget.saturating_sub(amount);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sensitive_keys_are_redacted_at_any_depth() {
        let payload = json!({
            "model": "gpt-6-luna",
            "headers": {
                "Authorization": "Bearer sk-live-123",
                "content-type": "application/json",
                "X-API-Key": "sk-live-456"
            },
            "auth": {"refresh_token": "rt-1", "scope": "openid"},
            "messages": [{"role": "user", "content": "hi", "password": "hunter2"}],
            "nested": [[{"client_secret": "shh"}]],
            "max_tokens": 4096,
            "usage": {"input_tokens": 12, "output_tokens": 3},
        });
        let redacted = redact(&payload);
        assert_eq!(redacted["model"], "gpt-6-luna");
        assert_eq!(redacted["headers"]["content-type"], "application/json");
        assert_eq!(redacted["headers"]["Authorization"], REDACTED);
        assert_eq!(redacted["headers"]["X-API-Key"], REDACTED);
        assert_eq!(redacted["auth"]["refresh_token"], REDACTED);
        assert_eq!(redacted["auth"]["scope"], "openid");
        assert_eq!(redacted["messages"][0]["content"], "hi");
        assert_eq!(redacted["messages"][0]["password"], REDACTED);
        assert_eq!(redacted["nested"][0][0]["client_secret"], REDACTED);
        // A count under a token-shaped key is not a secret.
        assert_eq!(redacted["max_tokens"], 4096);
        assert_eq!(redacted["usage"]["input_tokens"], 12);
        let text = redacted.to_string();
        assert!(!text.contains("sk-live"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn bearer_strings_are_redacted_wherever_they_appear() {
        assert_eq!(
            redact_bearer("Authorization: Bearer sk-abc123"),
            "Authorization: Bearer [redacted]"
        );
        assert_eq!(redact_bearer("bearer\tabc.def"), "bearer\t[redacted]");
        assert_eq!(
            redact_bearer("{\"h\":\"Bearer abc\"}"),
            "{\"h\":\"Bearer [redacted]\"}"
        );
        // A word that only contains "bearer" is left alone.
        assert_eq!(redact_bearer("shearer of sheep"), "shearer of sheep");
        // Already redacted stays as it is, so redacting twice is the same.
        let once = redact_bearer("Bearer sk-1");
        assert_eq!(redact_bearer(&once), once);
        // No token to replace: nothing to do and no panic.
        assert_eq!(redact_bearer("Bearer"), "Bearer");
        assert_eq!(redact_bearer(""), "");
    }

    #[test]
    fn clipped_json_still_parses_and_stays_under_the_cap() {
        let long = "x".repeat(4000);
        let payload = json!({
            "model": "gpt-6-luna",
            "messages": [{"content": long}],
        });
        let (text, truncated) = clip_json(&payload, 512);
        assert!(truncated, "a 4 KiB payload does not fit 512 bytes");
        assert!(text.len() <= 512, "{} bytes: {text}", text.len());
        let parsed: Value = serde_json::from_str(&text).expect("clipped JSON parses");
        assert_eq!(parsed["model"], "gpt-6-luna");

        // An array keeps a prefix of its items and says the rest is gone.
        let many = json!((0..500).collect::<Vec<i64>>());
        let (text, truncated) = clip_json(&many, 256);
        assert!(truncated);
        assert!(text.len() <= 256, "{} bytes: {text}", text.len());
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let kept = parsed.as_array().unwrap();
        assert!(kept.len() < 500, "the tail was dropped");
        assert_eq!(kept.last().unwrap(), &json!(TRUNCATED));
        assert_eq!(kept[0], json!(0));

        // An object keeps a prefix of its entries and marks the drop.
        let wide: Map<String, Value> = (0..500).map(|n| (format!("key{n}"), json!(n))).collect();
        let (text, truncated) = clip_json(&Value::Object(wide), 256);
        assert!(truncated);
        assert!(text.len() <= 256, "{} bytes: {text}", text.len());
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed[TRUNCATED_KEY], json!(true));

        // A payload that already fits is returned unchanged and unflagged.
        let (text, truncated) = clip_json(&json!({"a": 1}), 1024);
        assert_eq!(text, "{\"a\":1}");
        assert!(!truncated);
    }

    #[test]
    fn a_prepared_payload_is_redacted_before_it_is_clipped() {
        let payload = json!({"headers": {"authorization": "Bearer sk-9"}, "pad": "y".repeat(2000)});
        let (text, truncated) = prepare_payload(&payload, 256);
        assert!(truncated);
        assert!(!text.contains("sk-9"), "{text}");
        assert!(text.len() <= 256);
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["headers"]["authorization"], REDACTED);
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;
    #[test]
    fn browser_typing_is_redacted_in_captured_calls() {
        let value = serde_json::json!({"params":{"name":"mcp__rustcode__browser_type","arguments":{"selector":"#password","text":"private"}}});
        let redacted = redact(&value);
        assert_eq!(redacted["params"]["arguments"]["text"], REDACTED);
        assert_eq!(redacted["params"]["arguments"]["selector"], "#password");
    }
}
