//! The line a reader sees when a provider refuses a request (#2584).
//!
//! A failed request reaches the turn as a diagnostic such as
//! `stream_failure:provider_error status=507 bytes_received=0 … detail={"error":{"message":"…"}}`.
//! The reader needs the provider's own sentence, the HTTP status and which
//! profile answered; the byte and event counters stay in `debug.log`.

use serde_json::Value;

/// Prefix of the system message a provider failure writes to history. The
/// controller finds the failure by it; frontends show what follows it.
pub const PROVIDER_ERROR_PREFIX: &str = "Error from LLM Provider: ";

/// Longest provider message shown before it is cut with an ellipsis.
const MAX_MESSAGE_CHARS: usize = 600;

/// `<source> · <status>: <message>`, the provider's message taken from the
/// raw failure. `source` is the profile (or model) that was asked.
pub fn provider_error_line(raw: &str, source: &str) -> String {
    let (status, message) = provider_error_parts(raw);
    let source = source.trim();
    let head = match (source.is_empty(), status) {
        (false, Some(status)) => format!("{source} · {status}"),
        (false, None) => source.to_owned(),
        (true, Some(status)) => status.to_string(),
        (true, None) => "Provider".to_owned(),
    };
    let message = message
        .or_else(|| status.and_then(status_reason).map(str::to_owned))
        .unwrap_or_else(|| "the request failed without a message".to_owned());
    format!("{head}: {message}")
}

/// What a frontend shows for a provider-failure history message, or `None`
/// when `content` is not one. Older sessions stored the raw diagnostic after
/// the prefix; it is cleaned here so they read the same as new ones.
pub fn provider_error_display(content: &str) -> Option<String> {
    let rest = content
        .trim()
        .strip_prefix(PROVIDER_ERROR_PREFIX.trim_end())?;
    let rest = rest.trim();
    Some(if rest.starts_with("stream_failure:") {
        provider_error_line(rest, "")
    } else {
        rest.to_owned()
    })
}

/// The HTTP status and the provider's message in a raw failure.
fn provider_error_parts(raw: &str) -> (Option<u16>, Option<String>) {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("stream_failure:") {
        let (fields, detail) = match rest.split_once(" detail=") {
            Some((fields, detail)) => (fields, Some(detail)),
            None => (rest, None),
        };
        let status = fields
            .split_whitespace()
            .find_map(|field| field.strip_prefix("status="))
            .and_then(|status| status.parse().ok());
        return (status, detail.and_then(message_from_body));
    }
    // `429 Too Many Requests` and similar bare status lines.
    let status = raw
        .split_whitespace()
        .next()
        .filter(|word| word.len() == 3)
        .and_then(|word| word.parse::<u16>().ok())
        .filter(|code| (100..600).contains(code));
    let body = match status {
        Some(_) => raw[3..].trim_start_matches([' ', ':']),
        None => raw,
    };
    (status, message_from_body(body))
}

/// The provider's sentence in an error body: a JSON error object in any of
/// the common shapes, else the first line of a plain-text body.
fn message_from_body(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return message_from_json(&value).map(|message| clip(&message));
    }
    if body.starts_with('<') {
        // An HTML error page says nothing a status line does not.
        return None;
    }
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(clip)
}

fn message_from_json(value: &Value) -> Option<String> {
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    match value {
        Value::String(text) => Some(text.trim().to_owned()).filter(|text| !text.is_empty()),
        // Gemini wraps its error object in an array.
        Value::Array(items) => items.iter().find_map(message_from_json),
        Value::Object(object) => {
            // OpenAI, Responses, Anthropic, Gemini: `{"error":{"message":…}}`.
            if let Some(error) = object.get("error") {
                if let Some(message) = text(error.get("message")) {
                    return Some(message);
                }
                if let Some(message) = text(Some(error)) {
                    return Some(message);
                }
            }
            if let Some(message) = text(object.get("message")) {
                return Some(message);
            }
            // FastAPI-style servers: `{"detail":"…"}` or `{"detail":[{"msg":…}]}`.
            match object.get("detail") {
                Some(Value::String(detail)) if !detail.trim().is_empty() => {
                    Some(detail.trim().to_owned())
                }
                Some(Value::Array(items)) => items.iter().find_map(|item| text(item.get("msg"))),
                _ => text(object.get("error_description")),
            }
        }
        _ => None,
    }
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_MESSAGE_CHARS {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
    clipped.push('…');
    clipped
}

fn status_reason(status: u16) -> Option<&'static str> {
    reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
}

#[cfg(test)]
mod tests {
    use super::{provider_error_display, provider_error_line};

    const OMLX_507: &str = r#"stream_failure:provider_error status=507 bytes_received=0 events_received=0 partial_event_bytes=0 detail={"error":{"message":"Model 'LFM2.5-24B-A2B-MLX-4bit' (13.27GB) does not fit under the dynamic memory ceiling (9.93GB). Close other apps to free RAM.","type":"server_error","param":null,"code":null}}"#;

    #[test]
    fn the_provider_message_replaces_the_diagnostic() {
        assert_eq!(
            provider_error_line(OMLX_507, "lfm2.5-24b-a2b-mlx-4bit"),
            "lfm2.5-24b-a2b-mlx-4bit · 507: Model 'LFM2.5-24B-A2B-MLX-4bit' (13.27GB) does not fit under the dynamic memory ceiling (9.93GB). Close other apps to free RAM."
        );
    }

    #[test]
    fn common_error_shapes_give_their_message() {
        let line = |detail: &str| {
            provider_error_line(
                &format!(
                    "stream_failure:provider_error status=400 bytes_received=0 events_received=0 partial_event_bytes=0 detail={detail}"
                ),
                "p",
            )
        };
        // Anthropic.
        assert_eq!(
            line(
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is too large"}}"#
            ),
            "p · 400: max_tokens is too large"
        );
        // Gemini.
        assert_eq!(
            line(
                r#"[{"error":{"code":400,"message":"API key not valid","status":"INVALID_ARGUMENT"}}]"#
            ),
            "p · 400: API key not valid"
        );
        // A bare error string and a top-level message.
        assert_eq!(
            line(r#"{"error":"model not loaded"}"#),
            "p · 400: model not loaded"
        );
        assert_eq!(line(r#"{"message":"bad request"}"#), "p · 400: bad request");
        // FastAPI `detail`, as a string and as a validation list.
        assert_eq!(line(r#"{"detail":"Not Found"}"#), "p · 400: Not Found");
        assert_eq!(
            line(r#"{"detail":[{"loc":["body"],"msg":"field required","type":"missing"}]}"#),
            "p · 400: field required"
        );
        // Plain text keeps its first line.
        assert_eq!(
            line("Our servers are currently overloaded.\nRetry later."),
            "p · 400: Our servers are currently overloaded."
        );
    }

    #[test]
    fn a_failure_without_a_body_reads_as_its_status() {
        assert_eq!(
            provider_error_line(
                "stream_failure:provider_error status=503 bytes_received=0 events_received=0 partial_event_bytes=0",
                "p"
            ),
            "p · 503: Service Unavailable"
        );
        assert_eq!(
            provider_error_line(
                "stream_failure:provider_error status=502 bytes_received=0 events_received=0 partial_event_bytes=0 detail=<html><body>Bad gateway</body></html>",
                "p"
            ),
            "p · 502: Bad Gateway"
        );
        assert_eq!(
            provider_error_line("stream_failure:provider_error status=none", "p"),
            "p: the request failed without a message"
        );
        assert_eq!(
            provider_error_line("429 Too Many Requests", "p"),
            "p · 429: Too Many Requests"
        );
        assert_eq!(
            provider_error_line(
                "stream_failure:provider_error status=none bytes_received=0 events_received=0 partial_event_bytes=0 detail=Request failed: error sending request",
                "p"
            ),
            "p: Request failed: error sending request"
        );
    }

    #[test]
    fn a_stored_failure_is_shown_without_its_prefix_or_diagnostics() {
        assert_eq!(
            provider_error_display("Error from LLM Provider: p · 507: Out of memory").as_deref(),
            Some("p · 507: Out of memory")
        );
        // A session saved before the message was cleaned.
        assert!(
            provider_error_display(&format!("Error from LLM Provider: {OMLX_507}"))
                .unwrap()
                .starts_with("507: Model 'LFM2.5-24B-A2B-MLX-4bit'")
        );
        assert_eq!(provider_error_display("an ordinary notice"), None);
    }
}
