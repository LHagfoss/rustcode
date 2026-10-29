//! Qwen3-Coder-style XML tool calls.
//!
//! Several open models (Qwen3-Coder, and LFM3 behind some chat templates)
//! write calls as
//! `<tool_call><function=name><parameter=key>value</parameter></function></tool_call>`
//! whatever the textual contract in the system prompt says. Only a closed
//! `<function=…>…</function>` block is parsed, so prose that merely mentions
//! the syntax is never executed.

use crate::ToolCall;
use serde_json::{Map, Value};

pub const FUNCTION_START: &str = "<function=";
const FUNCTION_END: &str = "</function>";
const PARAMETER_START: &str = "<parameter=";
const PARAMETER_END: &str = "</parameter>";
const CALL_START: &str = "<tool_call>";
const CALL_END: &str = "</tool_call>";

/// Parse every closed function block in `text`. A block whose name or
/// parameters do not parse contributes nothing, so the caller's
/// malformed-call recovery runs.
pub(crate) fn parse_functions(text: &str, calls: &mut Vec<ToolCall>) {
    let mut rest = text;
    while let Some(start) = rest.find(FUNCTION_START) {
        let after = &rest[start + FUNCTION_START.len()..];
        let Some(name_end) = after.find('>') else {
            break;
        };
        let name = after[..name_end].trim();
        let body_and_tail = &after[name_end + 1..];
        let Some(end) = body_and_tail.find(FUNCTION_END) else {
            break;
        };
        if let Some(call) = parse_function(name, &body_and_tail[..end]) {
            calls.push(call);
        }
        rest = &body_and_tail[end + FUNCTION_END.len()..];
    }
}

fn parse_function(name: &str, body: &str) -> Option<ToolCall> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    let trimmed = body.trim();
    let arguments = if trimmed.is_empty() {
        Value::Object(Map::new())
    } else if trimmed.starts_with('{') {
        // Some models put a JSON object inside the function block instead.
        let value = serde_json::from_str::<Value>(&crate::repair_json(trimmed)).ok()?;
        value.is_object().then_some(value)?
    } else {
        Value::Object(parse_parameters(body)?)
    };
    Some(ToolCall {
        name: name.to_string(),
        arguments,
        call_id: None,
    })
}

fn parse_parameters(body: &str) -> Option<Map<String, Value>> {
    let mut map = Map::new();
    let mut rest = body;
    loop {
        let before = match rest.find(PARAMETER_START) {
            Some(start) => {
                let (before, after) = rest.split_at(start);
                rest = &after[PARAMETER_START.len()..];
                before
            }
            None => {
                // Anything but whitespace outside a parameter is not a call.
                return rest.trim().is_empty().then_some(map);
            }
        };
        if !before.trim().is_empty() {
            return None;
        }
        let key_end = rest.find('>')?;
        let key = rest[..key_end].trim();
        if key.is_empty() {
            return None;
        }
        let value_and_tail = &rest[key_end + 1..];
        let end = value_and_tail.find(PARAMETER_END)?;
        map.insert(key.to_string(), parameter_value(&value_and_tail[..end]));
        rest = &value_and_tail[end + PARAMETER_END.len()..];
    }
}

/// The template puts each value on its own lines; one newline on each side is
/// formatting. Values that read as JSON (numbers, booleans, arrays, objects)
/// keep their type, so schema validation sees `3`, not `"3"`.
fn parameter_value(raw: &str) -> Value {
    let raw = raw.strip_prefix('\n').unwrap_or(raw);
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    let trimmed = raw.trim();
    let looks_typed = matches!(trimmed, "true" | "false" | "null")
        || trimmed.starts_with(['{', '['])
        || trimmed.parse::<f64>().is_ok();
    if looks_typed && let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return value;
    }
    Value::String(raw.to_string())
}

/// Byte spans of every call, for stripping them from prose. A `<tool_call>`
/// wrapper is included when present; an unclosed block runs to the end.
pub(crate) fn call_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(FUNCTION_START) {
        let mut start = offset + relative;
        if text[offset..start].trim_end().ends_with(CALL_START) {
            start = offset + text[offset..start].trim_end().len() - CALL_START.len();
        }
        let body_start = offset + relative + FUNCTION_START.len();
        let Some(end) = text[body_start..].find(FUNCTION_END) else {
            spans.push((start, text.len()));
            break;
        };
        let mut stop = body_start + end + FUNCTION_END.len();
        let tail = &text[stop..];
        if tail.trim_start().starts_with(CALL_END) {
            stop += tail.len() - tail.trim_start().len() + CALL_END.len();
        }
        spans.push((start, stop));
        offset = stop;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Vec<ToolCall> {
        let mut calls = Vec::new();
        parse_functions(text, &mut calls);
        calls
    }

    #[test]
    fn parses_a_call_without_parameters() {
        // Session 01a1874e…: LFM3-1.7B answered "what time is it" like this.
        let calls = parse("<tool_call>\n<function=get_time>\n</function>\n</tool_call>");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_time");
        assert_eq!(calls[0].arguments, json!({}));
    }

    #[test]
    fn parses_typed_and_multiline_parameters() {
        let calls = parse(
            "<tool_call>\n<function=read_file>\n<parameter=path>\nsrc/lib.rs\n</parameter>\n\
             <parameter=start_line>\n10\n</parameter>\n<parameter=raw>\ntrue\n</parameter>\n\
             </function>\n</tool_call>\n<tool_call>\n<function=write_to_file>\n\
             <parameter=content>\nfn a() {}\n\nfn b() {}\n</parameter>\n</function>\n</tool_call>",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].arguments,
            json!({"path": "src/lib.rs", "start_line": 10, "raw": true})
        );
        assert_eq!(
            calls[1].arguments,
            json!({"content": "fn a() {}\n\nfn b() {}"})
        );
    }

    #[test]
    fn accepts_a_json_body() {
        let calls = parse("<tool_call><function=grep>{\"pattern\": \"a\"}</function></tool_call>");
        assert_eq!(calls[0].arguments, json!({"pattern": "a"}));
    }

    #[test]
    fn ignores_unclosed_or_malformed_blocks() {
        assert!(parse("<function=write_to_file><parameter=path>x</parameter>").is_empty());
        assert!(parse("<function=bad name></function>").is_empty());
        assert!(parse("<function=grep>prose<parameter=a>b</parameter></function>").is_empty());
        assert!(parse("Use the `<function=name>` syntax to call tools.").is_empty());
    }

    #[test]
    fn spans_cover_the_wrapper() {
        let text = "Checking.\n<tool_call>\n<function=get_time>\n</function>\n</tool_call>\nDone.";
        let spans = call_spans(text);
        assert_eq!(spans.len(), 1);
        let (start, end) = spans[0];
        assert_eq!(&text[..start], "Checking.\n");
        assert_eq!(&text[end..], "\nDone.");
    }
}
