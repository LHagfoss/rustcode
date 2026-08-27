use super::{ToolCall, validate_tool_calls};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

fn extract_tool_call(json: &Value) -> Option<(String, Value)> {
    let top_level_name = json.get("name").and_then(|v| v.as_str());

    let mut args = if let Some(args_val) = json.get("arguments") {
        args_val.clone()
    } else if let Some(obj) = json.as_object() {
        let mut map = serde_json::Map::new();
        for (k, v) in obj {
            if k != "name" {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map)
    } else {
        Value::Object(Default::default())
    };

    let name = match top_level_name {
        Some(name) => name.to_string(),
        None => {
            // Tool name nested inside `arguments`: recover it as the tool name
            // and strip it from the args. Only done when there is no top-level
            // `name` — otherwise an argument literally called `name` (e.g.
            // use_skill's skill name) is legitimate and must be kept.
            match args.get("name").and_then(|v| v.as_str()) {
                Some(nested) => {
                    let nested = nested.to_string();
                    if nested != "use_skill"
                        && let Some(obj) = args.as_object_mut()
                    {
                        obj.remove("name");
                    }
                    nested
                }
                // No `name` anywhere. Some models drop the field entirely on
                // large-content calls. If the argument keys uniquely match one
                // tool's required signature, infer it rather than hard-failing
                // and forcing a retry loop the model tends not to recover from.
                None => infer_tool_name_from_args(&args)?.to_string(),
            }
        }
    };

    Some((name, args))
}

/// Best-effort recovery for tool calls that omit `name` entirely. Only
/// returns a match when the argument keys are distinctive enough that no
/// other tool could plausibly be meant.
fn infer_tool_name_from_args(args: &Value) -> Option<&'static str> {
    let obj = args.as_object()?;
    let has = |k: &str| obj.contains_key(k);

    if has("content") && has("path") {
        Some("write_to_file")
    } else if has("replacements") && has("path") {
        Some("multi_replace_file_content")
    } else if has("old_string") && has("new_string") && has("path") {
        Some("replace_file_content")
    } else if has("command") && obj.len() <= 2 {
        Some("run_command")
    } else if has("result") && obj.len() == 1 {
        Some("complete_task")
    } else {
        None
    }
}

pub(super) fn repair_json(s: &str) -> String {
    let trimmed = s.trim_end();
    let mut s_clean = trimmed.to_string();
    if s_clean.ends_with(',') {
        s_clean.pop();
    }

    let mut repaired = String::with_capacity(s_clean.len() + 16);
    let mut in_string = false;
    let mut escaped = false;
    let mut stack = Vec::new();

    for c in s_clean.chars() {
        if escaped {
            escaped = false;
            repaired.push(c);
            continue;
        }
        if c == '\\' && in_string {
            escaped = true;
            repaired.push(c);
            continue;
        }
        if c == '"' {
            in_string = !in_string;
            repaired.push(c);
            continue;
        }
        if in_string {
            match c {
                '\n' => repaired.push_str("\\n"),
                '\r' => repaired.push_str("\\r"),
                '\t' => repaired.push_str("\\t"),
                _ => repaired.push(c),
            }
            continue;
        }

        repaired.push(c);
        if c == '{' {
            stack.push('}');
        } else if c == '[' {
            stack.push(']');
        } else if (c == '}' || c == ']')
            && let Some(&last) = stack.last()
            && last == c
        {
            stack.pop();
        }
    }

    if in_string {
        repaired.push('"');
    }

    while let Some(close_char) = stack.pop() {
        repaired.push(close_char);
    }

    repaired
}

static TOOL_CALLS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[TOOL_CALLS\]\s*([a-zA-Z0-9_-]+)[\":]*\s*(?:\[ARGS\])?[\":]*\s*(\{[\s\S]*)"#)
        .unwrap()
});
static BRACE_OBJ_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^{}]*\}").unwrap());

fn parse_tool_calls_tags(text: &str, calls: &mut Vec<ToolCall>) {
    if text.contains("[TOOL_CALLS]") {
        let re = &*TOOL_CALLS_RE;
        for chunk in text.split("[TOOL_CALLS]") {
            let chunk = chunk.trim();
            if chunk.is_empty() {
                continue;
            }
            let full = format!("[TOOL_CALLS]{chunk}");
            if let Some(caps) = re.captures(&full) {
                let name = caps.get(1).unwrap().as_str().to_string();
                let raw_args = caps.get(2).unwrap().as_str();

                let repaired = repair_json(raw_args);
                if let Ok(json_val) = serde_json::from_str::<Value>(&repaired) {
                    calls.push(ToolCall {
                        name,
                        arguments: json_val,
                        call_id: None,
                    });
                } else {
                    let pattern = &*BRACE_OBJ_RE;
                    if let Some(mat) = pattern.find(raw_args)
                        && let Ok(json_val) = serde_json::from_str::<Value>(mat.as_str())
                    {
                        calls.push(ToolCall {
                            name,
                            arguments: json_val,
                            call_id: None,
                        });
                    }
                }
            }
        }
    }
}

/// Locate the matching closing fence for a tool block.
///
/// Unlike a naive `after_tag.find("```")`, this parser respects JSON string
/// literals and escape sequences so backticks inside string arguments (such as
/// Markdown code fences inside `target_content`, `replacement_content`, or shell
/// scripts) do not prematurely terminate the tool call block.
pub(crate) fn find_closing_tool_fence(after_tag: &str) -> (usize, usize) {
    let mut in_string = false;
    let mut escaped = false;
    let bytes = after_tag.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            i += 1;
        } else {
            match b {
                b'"' => {
                    in_string = true;
                    escaped = false;
                    i += 1;
                }
                b'`' => {
                    // Check for at least 3 consecutive backticks outside of JSON strings
                    let count = bytes[i..].iter().take_while(|&&c| c == b'`').count();
                    if count >= 3 {
                        let block_end = i;
                        let next_start = i + count;
                        return (block_end, next_start);
                    }
                    i += count;
                }
                _ => {
                    i += 1;
                }
            }
        }
    }

    (len, len)
}

fn parse_tool_calls_fenced(text: &str, calls: &mut Vec<ToolCall>) {
    // Walk every ```tool fence, not just the first, so a model can batch
    // multiple tool calls in one turn (the executor runs them in parallel).
    // `find("```tool")` also matches ```tool_code (Gemini's code-exec fence);
    // require the fence tag to be exactly `tool` (next char whitespace) so we
    // skip those without eating the real call.
    let mut search = text;
    while let Some(rel) = search.find("```tool") {
        let after_tag = &search[rel + 7..];
        let (rel_end, next_rel) = find_closing_tool_fence(after_tag);
        let block = &after_tag[..rel_end];
        let next = &after_tag[next_rel..];

        let is_tool_fence = after_tag.chars().next().is_none_or(|c| c.is_whitespace());
        if is_tool_fence {
            let repaired = repair_json(block.trim());
            if let Ok(json_value) = serde_json::from_str::<Value>(&repaired)
                && let Some(call) = extract_tool_call(&json_value)
            {
                calls.push(ToolCall {
                    name: call.0,
                    arguments: call.1,
                    call_id: None,
                });
            }
        }

        if next.is_empty() {
            break;
        }
        search = next;
    }
}

/// When a tool call was clearly intended but produced zero parseable calls,
/// return a specific reason (the underlying JSON error and the offending block)
/// so the retry nudge can tell the model exactly what to fix instead of a vague
/// "malformed" message it tends to reproduce verbatim. Returns `None` when the
/// text was parseable or contained no recognizable tool syntax.
pub fn diagnose_failed_tool_call(text: &str) -> Option<String> {
    // Look at every ```tool fence; report the first that fails to parse or validate.
    let mut search = text;
    while let Some(rel) = search.find("```tool") {
        let after_tag = &search[rel + 7..];
        let (rel_end, next_rel) = find_closing_tool_fence(after_tag);
        let block = &after_tag[..rel_end];
        let next = &after_tag[next_rel..];
        let is_tool_fence = after_tag.chars().next().is_none_or(|c| c.is_whitespace());
        if is_tool_fence {
            let repaired = repair_json(block.trim());
            match serde_json::from_str::<Value>(&repaired) {
                Ok(val) => {
                    let has_name = val.get("name").is_some()
                        || val.get("arguments").and_then(|a| a.get("name")).is_some()
                        || val
                            .get("arguments")
                            .and_then(infer_tool_name_from_args)
                            .is_some();
                    if !has_name {
                        let snippet: String = block.trim().chars().take(240).collect();
                        return Some(format!(
                            "Missing required 'name' field in tool call JSON. Every tool call must include `\"name\": \"tool_name\"`. Offending block:\n```\n{snippet}\n```"
                        ));
                    }
                    if let Some((name, args)) = extract_tool_call(&val) {
                        if let Err(err) = validate_tool_calls(&[ToolCall {
                            name: name.clone(),
                            arguments: args,
                            call_id: None,
                        }]) {
                            let snippet: String = block.trim().chars().take(240).collect();
                            return Some(format!(
                                "Tool call '{name}' failed validation: {err}. Offending block:\n```\n{snippet}\n```"
                            ));
                        }
                    }
                }
                Err(e) => {
                    let snippet: String = block.trim().chars().take(240).collect();
                    return Some(format!(
                        "JSON parse error: {e}. A common cause is a backslash inside a string: every literal `\\` in the file must be written as `\\\\` in the JSON, and a real newline must be `\\n`. Offending block:\n```\n{snippet}\n```"
                    ));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        search = next;
    }
    None
}

fn parse_tool_calls_impl(text: &str, protocol: crate::config::ToolProtocol) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    match protocol {
        crate::config::ToolProtocol::Native => {
            parse_tool_calls_tags(text, &mut calls);
            if calls.is_empty() {
                parse_tool_calls_fenced(text, &mut calls);
            }
        }
        crate::config::ToolProtocol::Json | crate::config::ToolProtocol::ApiNative => {
            // ApiNative: the stream reader translates the provider's structured
            // `tool_calls` into the same fenced `tool` block the Json path emits,
            // so both parse identically.
            parse_tool_calls_fenced(text, &mut calls);
            if calls.is_empty() {
                parse_tool_calls_tags(text, &mut calls);
            }
        }
    }

    // If no tool blocks found, try to parse the whole text as JSON (with repair if it starts with '{')
    if calls.is_empty() {
        let cleaned = text.trim();
        let to_parse = if cleaned.starts_with('{') {
            repair_json(cleaned)
        } else {
            cleaned.to_string()
        };
        if let Ok(json_value) = serde_json::from_str::<Value>(&to_parse)
            && let Some(call) = extract_tool_call(&json_value)
        {
            calls.push(ToolCall {
                name: call.0,
                arguments: call.1,
                call_id: None,
            });
        }
    }

    // Try to find JSON objects in the text
    if calls.is_empty() {
        let pattern = &*BRACE_OBJ_RE;
        for mat in pattern.find_iter(text) {
            let json_str = mat.as_str();
            if let Ok(json_value) = serde_json::from_str::<Value>(json_str)
                && let Some(call) = extract_tool_call(&json_value)
            {
                calls.push(ToolCall {
                    name: call.0,
                    arguments: call.1,
                    call_id: None,
                });
            }
        }
    }

    calls.dedup();
    calls
}

pub fn parse_tool_calls(text: &str, protocol: crate::config::ToolProtocol) -> Vec<ToolCall> {
    let raw_calls = parse_tool_calls_impl(text, protocol);
    let mut unique_calls = Vec::new();
    for call in raw_calls {
        if !unique_calls
            .iter()
            .any(|existing: &ToolCall| existing == &call)
        {
            unique_calls.push(call);
        }
    }
    unique_calls
}

pub fn is_code_editing_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content" | "multi_replace_file_content" | "write_to_file"
    )
}

pub fn is_tool_call_start(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.contains("```tool")
        || trimmed.contains("[TOOL_CALLS]")
        || trimmed.contains("<tool_call>")
        || trimmed.contains("<function_call>")
        || trimmed.contains("\"tool_name\"")
        || trimmed.contains("\"tool_call\"")
        || (trimmed.contains('{')
            && (trimmed.contains("\"name\"")
                || trimmed.contains("\"tool\"")
                || trimmed.contains("\"action\"")
                || trimmed.contains("\"function\"")))
}

pub fn parse_tool_call(text: &str, protocol: crate::config::ToolProtocol) -> Option<ToolCall> {
    parse_tool_calls(text, protocol).into_iter().next()
}
