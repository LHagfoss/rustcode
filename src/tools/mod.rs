use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

mod audio;
mod delegate;
mod dispatch;
mod envelope;
pub(crate) mod exec;
mod filesystem;
mod git;
mod misc;
mod openapi;
mod parser;
#[cfg(all(test, unix))]
mod scheduled_jobs_tests;
mod schema;
mod search;
mod video;

#[allow(unused_imports)]
pub use dispatch::{execute, needs_confirmation};
pub use parser::{
    diagnose_failed_tool_call, has_incomplete_actionable_tool_call, is_code_editing_tool,
    is_tool_call_start, parse_tool_call, parse_tool_calls,
};
#[cfg(test)]
pub use schema::native_tools_schema;
pub use schema::tool_system_prompt;

#[allow(unused_imports)]
pub(crate) use dispatch::{
    execute_video_with_progress, execute_with_metadata, execute_with_metadata_cancellable,
    execute_with_metadata_cancellable_for_call,
};
pub(crate) use parser::find_closing_tool_fence;
#[cfg(test)]
pub(crate) use schema::native_tools_schema_for_context;
#[cfg(test)]
pub(crate) use schema::native_tools_schema_for_context_with_sticky_at;
pub(crate) use schema::{
    MAX_MCP_NATIVE_SCHEMAS, McpSchemaSelectionStats, ToolSchemaPhase, ToolSchemaPolicy,
    ToolSurface, agent_tool_count, append_tool_response_limit, append_tool_response_policy,
    mcp_tool_display_name, mcp_tool_read_only_hint,
    native_tools_schema_for_context_with_sticky_at_and_reserved_servers, textual_tool_surface,
    tool_schema_phase, tool_system_prompt_for_policy,
};

#[cfg(test)]
pub(crate) use schema::install_native_schema_test_gate;

use schema::{AGENT_TOOL_SPECS, collect_mcp_tools, schema_for_agent_tool, schema_for_tool};

#[cfg(test)]
use dispatch::as_error_message;
#[cfg(test)]
use parser::repair_json;
#[cfg(test)]
use schema::{
    MAX_MCP_NATIVE_SCHEMA_BYTES, MCP_DISCOVERY_FALLBACK_COUNT, mcp_canonical_name,
    provider_compatible_schema, schema_from_arguments, select_mcp_tools_for_context,
    select_mcp_tools_for_context_in_phase, select_mcp_tools_for_context_with_sticky,
};

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use envelope::{ToolCallEnvelope, ToolResultEnvelope};
pub use rustcode_core::ToolErrorKind;

pub(crate) use exec::{
    CommandProgressCallback, abort_background_starts, approved_command_prefix_covers_call,
    background_task_manager, command_confirmation_preview, command_requires_confirmation,
    denied_command_prefix_covers_call, release_background_start,
    rememberable_command_forbid_prefix_for_call, rememberable_command_prefix_for_call,
    run_command_output_with_progress_cancellable_for_call, stop_background_tasks,
    task_event_to_tool_output,
};

pub(crate) use filesystem::edit_target_and_replacement;
pub(crate) use misc::search_web_async;
pub(crate) use video::render_confirmation_preview;

pub use rustcode_tool_protocol::ToolCall;

/// Resolve structured calls recorded in history, falling back to the parser
/// used by the selected text protocol. This behavior-specific adapter stays in
/// the tools crate so the core message type remains independent of dispatch
/// and parser implementation details.
pub(crate) fn resolve_tool_calls(
    message: &rustcode_core::ChatMessage,
    protocol: crate::config::ToolProtocol,
) -> Vec<ToolCall> {
    if message.unexecuted_tool_call_checkpoint {
        return Vec::new();
    }
    if !message.tool_calls.is_empty() {
        message
            .tool_calls
            .iter()
            .map(|call| ToolCall {
                name: call.name.clone(),
                arguments: serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null),
                call_id: Some(call.id.clone()),
            })
            .collect()
    } else {
        parse_tool_calls(&message.content, protocol)
    }
}

/// Resolve only conservative aliases for built-in tools. An alias is ignored
/// when its spelling is already claimed by an agent/MCP tool, so a third-party
/// tool can never be shadowed by a convenience name.
pub(crate) fn resolve_builtin_tool_alias(name: &str) -> Option<&'static str> {
    let canonical = match name {
        "read" => "view_file",
        "write" | "write_file" => "write_to_file",
        "bash" => "run_command",
        _ => return None,
    };
    let mcp_claims_name = crate::mcp::get_mcp_registry()
        .lock()
        .ok()
        .is_some_and(|registry| {
            registry.values().any(|client| {
                client
                    .get_tools()
                    .unwrap_or_default()
                    .iter()
                    .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            })
        });
    if TOOLS.iter().any(|tool| tool.name == name)
        || AGENT_TOOL_SPECS.iter().any(|(tool, _, _)| *tool == name)
        || mcp_claims_name
    {
        return None;
    }
    TOOLS
        .iter()
        .any(|tool| tool.name == canonical)
        .then_some(canonical)
}

fn normalize_tool_query(name: &str) -> String {
    normalize_alphanumeric_lower(name)
}

/// Shared case-folded alphanumeric normalization for fuzzy matching.
/// Used by tool-name and symbol ranking so every matcher folds queries
/// the same way.
pub(crate) fn normalize_alphanumeric_lower(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn levenshtein_capped(a: &str, b: &str, cap: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > cap {
        return cap + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            curr[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Fuzzy-resolve a misspelled tool name to its canonical built-in.
/// Exact (case-insensitive) matches win first, then conservative aliases,
/// then normalized substring, then edit distance <= 2. Returns `None` when
/// ambiguous or unknown so callers surface a helpful error instead of
/// guessing.
pub fn fuzzy_match_tool_name(query: &str) -> Option<&'static str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(exact) = TOOLS
        .iter()
        .find(|tool| tool.name.eq_ignore_ascii_case(trimmed))
    {
        return Some(exact.name);
    }
    if let Some(alias) = resolve_builtin_tool_alias(&trimmed.to_ascii_lowercase()) {
        return Some(alias);
    }
    let normalized = normalize_tool_query(trimmed);
    if normalized.is_empty() {
        return None;
    }
    let mut substring: Vec<&'static str> = TOOLS
        .iter()
        .filter(|tool| normalize_tool_query(tool.name).contains(normalized.as_str()))
        .map(|tool| tool.name)
        .collect();
    if substring.len() == 1 {
        return Some(substring[0]);
    }
    substring.sort();
    let mut best: Option<(&'static str, usize)> = None;
    for tool in TOOLS {
        let distance = levenshtein_capped(&normalize_tool_query(tool.name), &normalized, 2);
        if distance <= 2 {
            match best {
                Some((_, best_dist)) if best_dist < distance => {}
                Some((best_name, best_dist)) if best_dist == distance => {
                    if best_name != tool.name {
                        // Ambiguous typo: two tools equally close.
                        best = None;
                        break;
                    }
                }
                _ => best = Some((tool.name, distance)),
            }
        }
    }
    if let Some((name, _)) = best {
        return Some(name);
    }
    substring.into_iter().next()
}

/// Progressive tool discovery: rank built-ins for a free-text query so the
/// agent can `list` a small subset instead of dumping every schema.
/// Exact/prefix/substring outrank fuzzy matches; ties break by name.
#[cfg(test)]
pub fn filter_tools_by_query(query: &str, limit: usize) -> Vec<&'static str> {
    let normalized = normalize_tool_query(query);
    if normalized.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut scored: Vec<(&'static str, usize)> = TOOLS
        .iter()
        .filter_map(|tool| {
            let name = normalize_tool_query(tool.name);
            let score = if name == normalized {
                0
            } else if name.starts_with(normalized.as_str()) {
                1
            } else if name.contains(normalized.as_str()) {
                2
            } else {
                3 + levenshtein_capped(&name, &normalized, 2)
            };
            (score <= 5).then_some((tool.name, score))
        })
        .collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
    scored.truncate(limit);
    scored.into_iter().map(|(name, _)| name).collect()
}

/// Authoritative facts returned by a tool invocation alongside its display
/// text. Consumers must not reconstruct these fields from `content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolExecutionOutput {
    pub(crate) content: String,
    pub(crate) success: bool,
    pub(crate) pending: bool,
    pub(crate) command: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) truncated: bool,
    /// Machine-readable completeness of the output delivered by the tool.
    /// This must be set by the execution layer, never inferred from display text.
    pub(crate) completeness: rustcode_core::ToolResultCompleteness,
    /// True when the harness served a bounded cached read instead of running
    /// the tool again. This is execution state, not display prose.
    pub(crate) replayed: bool,
    pub(crate) error_kind: Option<ToolErrorKind>,
    pub(crate) retryable: bool,
    pub(crate) command_status: Option<rustcode_core::CommandResultMetadata>,
}

impl ToolExecutionOutput {
    pub(crate) fn success(content: String) -> Self {
        Self {
            content,
            success: true,
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: None,
            retryable: false,
            command_status: None,
        }
    }

    pub(crate) fn failure(content: String) -> Self {
        Self {
            content,
            success: false,
            pending: false,
            command: None,
            exit_code: None,
            truncated: false,
            completeness: rustcode_core::ToolResultCompleteness::Complete,
            replayed: false,
            error_kind: Some(ToolErrorKind::Internal),
            retryable: false,
            command_status: None,
        }
    }

    pub(crate) fn failure_with_kind(
        content: String,
        error_kind: ToolErrorKind,
        retryable: bool,
    ) -> Self {
        Self {
            error_kind: Some(error_kind),
            retryable,
            ..Self::failure(content)
        }
    }
}

/// How many calls that can change the workspace may run from one response.
///
/// The limit exists so each edit is grounded in the result of the previous one,
/// not to ration throughput: a model planning six edits ahead is predicting file
/// contents it has not read. Read-only inspection never consumes this budget —
/// see [`is_read_only_call`]. Shell commands may still chain with any normal
/// operator because they are one call.
/// Backwards-compatible name for the safe default. Runtime orchestration
/// resolves the active profile's limit and passes it to policy functions.
#[cfg(test)]
pub const MAX_MUTATING_CALLS_PER_RESPONSE: usize =
    crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE;

/// Cut an over-eager batch down to the calls that may run this round, returning
/// the kept calls and the calls that were dropped.
///
/// Read-only calls are retained throughout the provider batch. The
/// mutation budget limits only mutating calls (see [`is_read_only_call`]), so
/// a later read — or a read-only shell inspection such as `git status` — is
/// not lost merely because an earlier mutation used the budget. Order among
/// retained calls is kept. The root orchestrator uses strict one-call
/// scheduling by default; explicit trusted profiles may select a separate
/// bounded batch before calling the executor. This helper also serves
/// consumers that need to retain the complete parsed response.
pub fn partition_tool_batch(
    mut calls: Vec<ToolCall>,
    max_mutating_calls: usize,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let total = calls.len();
    let mut keep = vec![false; total];

    let mut mutating = 0;
    for (index, call) in calls.iter().enumerate() {
        if !is_read_only_call(call) {
            if mutating >= max_mutating_calls {
                continue;
            }
            mutating += 1;
        }
        keep[index] = true;
    }

    let mut dropped = Vec::new();
    let mut kept = Vec::with_capacity(total.saturating_sub(1));
    for (index, call) in calls.drain(..).enumerate() {
        if keep[index] {
            kept.push(call);
        } else {
            dropped.push(call);
        }
    }
    (kept, dropped)
}

/// Backwards-compatible count-only view of [`partition_tool_batch`].
#[cfg(test)]
pub fn truncate_tool_batch(
    calls: Vec<ToolCall>,
    max_mutating_calls: usize,
) -> (Vec<ToolCall>, usize) {
    let (kept, dropped) = partition_tool_batch(calls, max_mutating_calls);
    (kept, dropped.len())
}

/// Validate parsed calls before they reach an executor. Text protocols are
/// intentionally permissive while parsing, but execution must be strict and
/// fail closed when the model emits an unknown tool or malformed arguments.
pub fn validate_tool_calls(calls: &[ToolCall], max_mutating_calls: usize) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();

    for call in calls {
        let fingerprint = duplicate_tool_call_key(call);
        if !seen.insert(fingerprint) {
            return Err(format!("duplicate tool call rejected: {}", call.name));
        }

        validate_tool_call(call)?;
    }

    let mutating = calls.iter().filter(|call| !is_read_only_call(call)).count();
    if mutating > max_mutating_calls {
        return Err(format!(
            "too many workspace-changing tool calls in one response ({mutating}; maximum is {max_mutating_calls}); emit the next action after receiving the previous result"
        ));
    }

    Ok(())
}

/// Build a semantic duplicate key for one provider call. Most schemas are
/// already canonicalized by serde_json's object representation, but built-in
/// handlers also accept string-encoded integers and `view_file` defaults a
/// missing `start_line` to 1. Normalize those equivalent forms before the
/// duplicate check so they cannot race through the parallel read scheduler.
pub(crate) fn duplicate_tool_call_key(call: &ToolCall) -> String {
    let mut arguments = call.arguments.clone();
    if call.name == "view_file"
        && let Some(object) = arguments.as_object_mut()
    {
        normalize_integer_argument(object, "start_line", Some(1));
        normalize_integer_argument(object, "end_line", None);
        normalize_integer_argument(object, "content_offset", None);
    }
    format!(
        "{}:{}",
        call.name,
        serde_json::to_string(&arguments).unwrap_or_default()
    )
}

fn normalize_integer_argument(
    object: &mut serde_json::Map<String, Value>,
    name: &str,
    default: Option<u64>,
) {
    let normalized = object
        .get(name)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .or(default);
    if let Some(value) = normalized {
        object.insert(name.to_string(), Value::from(value));
    }
}

/// Return validation failures in input order so callers can answer each
/// provider tool call without attributing one call's schema error to another.
pub(crate) fn validation_errors_by_call(calls: &[ToolCall]) -> Vec<Option<String>> {
    calls
        .iter()
        .map(|call| validate_tool_call(call).err())
        .collect()
}

fn validate_tool_call(call: &ToolCall) -> Result<(), String> {
    let Some(schema) = registered_tool_schema(&call.name) else {
        return Err(format!(
            "unknown or unavailable tool '{}'; use only tools in the current registry",
            call.name
        ));
    };

    // ApiNative parsing deliberately preserves malformed provider arguments as
    // a bounded marker. Report that marker before schema validation would turn
    // it into a misleading "missing path" or "additional property" error.
    // Never attempt to repair or execute the malformed payload.
    if let Some(reason) = malformed_arguments_reason(&call.arguments) {
        let guidance = tool_argument_guidance(&call.name).unwrap_or_default();
        return Err(format!(
            "malformed arguments for '{}': {reason}. No tool was executed. Emit one complete JSON object.{guidance}",
            call.name
        ));
    }

    // A complete textual call can otherwise carry an unbounded JSON string all
    // the way to the filesystem handler. Keep the small-file convenience path,
    // but force large writes through the resumable chunk protocol before any
    // workspace mutation or confirmation is reached. This is deliberately a
    // runtime guard as well as prompt/schema guidance because textual models do
    // not enforce JSON Schema limits themselves.
    if call.name == "write_to_file"
        && let Some(content) = call.arguments.get("content").and_then(Value::as_str)
        && content.len() > rustcode_tools::filesystem::MAX_FILE_CHUNK_BYTES
    {
        return Err(format!(
            "write_to_file content is {} bytes, above the {}-byte single-call limit; no file was changed. Use write_file_chunk with contiguous offsets (start at offset 0 with truncate=true, then resume with each returned next_offset)",
            content.len(),
            rustcode_tools::filesystem::MAX_FILE_CHUNK_BYTES
        ));
    }

    // Only built-in handlers coerce string-encoded integers
    // (parse_json_number); MCP servers receive arguments verbatim.
    let string_integers = TOOLS.iter().any(|tool| tool.name == call.name);
    if let Err(reason) =
        validate_value_against_schema(&call.arguments, &schema, "$", string_integers)
    {
        let guidance = tool_argument_guidance(&call.name).unwrap_or_default();
        return Err(format!(
            "invalid arguments for '{}'. Schema path: {reason}.{guidance}",
            call.name
        ));
    }

    Ok(())
}

fn malformed_arguments_reason(arguments: &Value) -> Option<String> {
    let marker = arguments.get("_invalid_arguments")?.as_object()?;
    let kind = marker
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("invalid");
    let parse_error = arguments
        .get("_parse_error")
        .and_then(Value::as_str)
        .unwrap_or("provider arguments were not valid JSON");
    Some(format!(
        "provider emitted {kind} JSON arguments: {parse_error}"
    ))
}

fn registered_tool_schema(name: &str) -> Option<Value> {
    if let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) {
        return Some(schema_for_tool(tool.name));
    }
    if let Some((_, _, schema)) = collect_mcp_tools().into_iter().find(|(n, _, _)| n == name) {
        return Some(schema);
    }
    if AGENT_TOOL_SPECS.iter().any(|(n, _, _)| *n == name) {
        return Some(schema_for_agent_tool(name));
    }
    None
}

fn example_value_for_schema(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut object = serde_json::Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object)
                && let Some(required) = schema.get("required").and_then(Value::as_array)
            {
                for field in required.iter().filter_map(Value::as_str) {
                    if let Some(property) = properties.get(field) {
                        object.insert(field.to_string(), example_value_for_schema(property));
                    }
                }
            }
            Value::Object(object)
        }
        Some("array") => schema
            .get("items")
            .map(example_value_for_schema)
            .map(|item| Value::Array(vec![item]))
            .unwrap_or_else(|| Value::Array(Vec::new())),
        Some("boolean") => Value::Bool(false),
        Some("integer") => Value::from(1),
        Some("number") => Value::from(1),
        Some("string") => Value::String("...".to_string()),
        _ => Value::Null,
    }
}

fn tool_argument_guidance(name: &str) -> Option<String> {
    let schema = registered_tool_schema(name)?;
    let properties = schema.get("properties").and_then(Value::as_object)?;
    let keys = properties
        .keys()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let example = match name {
        "view_file" => serde_json::json!({
            "path": "src/example.rs",
            "start_line": 1,
            "end_line": 40
        }),
        "multi_replace_file_content" => serde_json::json!({
            "path": "src/example.rs",
            "replacements": [{
                "start_line": 10,
                "end_line": 10,
                "target_content": "old",
                "replacement_content": "new"
            }]
        }),
        "replace_file_content" => serde_json::json!({
            "path": "src/example.ts",
            "edits": [{"old_string": "old", "new_string": "new"}]
        }),
        _ => example_value_for_schema(&schema),
    };
    let example = serde_json::to_string(&example).unwrap_or_else(|_| "{}".to_string());
    Some(format!(
        " Expected arguments for '{name}' use these keys: [{keys}]. Example: {example}"
    ))
}

fn validate_value_against_schema(
    value: &Value,
    schema: &Value,
    path: &str,
    string_integers: bool,
) -> Result<(), String> {
    for keyword in ["anyOf", "oneOf"] {
        let Some(branches) = schema.get(keyword).and_then(Value::as_array) else {
            continue;
        };
        let results = branches
            .iter()
            .map(|branch| validate_value_against_schema(value, branch, path, string_integers))
            .collect::<Vec<_>>();
        let matching = results.iter().filter(|result| result.is_ok()).count();
        let valid = match keyword {
            "anyOf" => matching > 0,
            "oneOf" => matching == 1,
            _ => unreachable!(),
        };
        if !valid {
            if matching > 1 {
                return Err(format!(
                    "{path} must match exactly one oneOf branch, but matched {matching}"
                ));
            }
            let failures = results
                .into_iter()
                .enumerate()
                .filter_map(|(index, result)| {
                    result
                        .err()
                        .map(|error| format!("branch {}: {error}", index + 1))
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!("{path} does not match {keyword} ({failures})"));
        }
    }

    // JSON Schema permits either a single type string or an array of types.
    // schemars emits the latter for optional MCP fields such as
    // `{"type":["integer","null"]}`. Treat the array as a union instead of
    // falling back to the root object's type.
    let expected_types: Vec<&str> = match schema.get("type") {
        Some(Value::String(expected)) => vec![expected.as_str()],
        Some(Value::Array(expected)) => expected.iter().filter_map(Value::as_str).collect(),
        _ if schema.get("anyOf").is_some() || schema.get("oneOf").is_some() => Vec::new(),
        _ => vec!["object"],
    };
    let type_matches = expected_types.is_empty()
        || expected_types.iter().any(|expected| match *expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            // Built-in handlers read line numbers through parse_json_number, which
            // also accepts string-encoded integers from lenient providers. MCP
            // tools receive arguments verbatim with no such coercion, so the
            // leniency is scoped to built-ins only.
            "integer" => {
                value.as_i64().is_some()
                    || value.as_u64().is_some()
                    || (string_integers && value.as_str().is_some_and(|s| s.parse::<u64>().is_ok()))
            }
            "number" => value.is_number(),
            _ => true,
        });
    if !type_matches {
        return Err(format!("{path} must be {}", expected_types.join(" or ")));
    }

    if let Some(string) = value.as_str() {
        let length = string.chars().count();
        if let Some(bound) = schema.get("minLength").and_then(Value::as_u64)
            && length < bound as usize
        {
            return Err(format!("{path} must contain at least {bound} characters"));
        }
        if let Some(bound) = schema.get("maxLength").and_then(Value::as_u64)
            && length > bound as usize
        {
            return Err(format!("{path} must contain at most {bound} characters"));
        }
    }

    let numeric_value = value.as_f64().or_else(|| {
        string_integers
            .then(|| value.as_str()?.parse::<f64>().ok())
            .flatten()
    });
    if let Some(actual) = numeric_value {
        if let Some(bound) = schema.get("minimum").and_then(Value::as_f64)
            && actual < bound
        {
            return Err(format!("{path} must be >= {bound}"));
        }
        if let Some(bound) = schema.get("maximum").and_then(Value::as_f64)
            && actual > bound
        {
            return Err(format!("{path} must be <= {bound}"));
        }
        if let Some(bound) = schema.get("exclusiveMinimum").and_then(Value::as_f64)
            && actual <= bound
        {
            return Err(format!("{path} must be > {bound}"));
        }
        if let Some(bound) = schema.get("exclusiveMaximum").and_then(Value::as_f64)
            && actual >= bound
        {
            return Err(format!("{path} must be < {bound}"));
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("{path}.{field} is required"));
                }
            }
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && let Some(properties) = schema.get("properties").and_then(Value::as_object)
            && let Some(unknown) = object.keys().find(|key| !properties.contains_key(*key))
        {
            return Err(format!("{path}.{unknown} is not an advertised argument"));
        }
        if let Some(ap_schema) = schema.get("additionalProperties").filter(|v| v.is_object())
            && let Some(obj) = value.as_object()
        {
            for (key, val) in obj {
                validate_value_against_schema(
                    val,
                    ap_schema,
                    &format!("{path}.{key}"),
                    string_integers,
                )?;
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in properties {
                if let Some(actual) = object.get(key) {
                    validate_value_against_schema(
                        actual,
                        child,
                        &format!("{path}.{key}"),
                        string_integers,
                    )?;
                }
            }
        }
    }
    if let Some(items) = schema.get("items")
        && let Some(array) = value.as_array()
    {
        for (index, item) in array.iter().enumerate() {
            validate_value_against_schema(
                item,
                items,
                &format!("{path}[{index}]"),
                string_integers,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct BackgroundTaskSnapshot {
    pub id: String,
    pub command: String,
    pub start_time: Instant,
    pub child_pid: Option<u32>,
}

pub(crate) fn background_task_snapshots(session_id: &str) -> Vec<BackgroundTaskSnapshot> {
    background_task_manager()
        .list(session_id)
        .into_iter()
        .map(|task| BackgroundTaskSnapshot {
            id: task.id.to_string(),
            command: task.command,
            start_time: task.started_at,
            child_pid: match task.state {
                rustcode_tasks::TaskState::Running { pid } => Some(pid),
                rustcode_tasks::TaskState::Starting
                | rustcode_tasks::TaskState::Terminating { .. }
                | rustcode_tasks::TaskState::CancelRequested => None,
            },
        })
        .collect()
}

pub(crate) fn background_command_label(command: &str, max_chars: usize) -> String {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut label = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    label.push('…');
    label
}

/// Shared byte-bounded truncation for tool outputs. Keeps a UTF-8 boundary
/// and appends a `truncated to <max> bytes` marker instead of each tool
/// hand-rolling its own copy.
pub(crate) fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…\n[output truncated to {max_bytes} bytes]", &text[..end])
}

pub(crate) fn has_background_tasks(session_id: &str) -> bool {
    background_task_manager().has_running(session_id)
}

#[cfg(test)]
pub(crate) fn spawn_background_task_for_test(
    task_id: &str,
    session_id: &str,
    command: &str,
) -> Result<(), String> {
    background_task_manager()
        .spawn_with_id(
            task_id,
            rustcode_tasks::TaskSpec::new(
                rustcode_tasks::SessionId::new(session_id),
                rustcode_command::CommandRequest {
                    command: command.to_owned(),
                    status_command: None,
                    sandboxed_shell: false,
                    cwd: None,
                    env: Vec::new(),
                    timeout: std::time::Duration::from_secs(30),
                    process_group: true,
                    inherited_fds: Vec::new(),
                },
            ),
        )
        .map(|_| ())
}

thread_local! {
    static ACTIVE_SESSION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
    static ACTIVE_WORKSPACE_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static ACTIVE_TASK_WORKING_DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static ACTIVE_TASK_SCOPE_ESCAPE: RefCell<bool> = const { RefCell::new(false) };
}

pub fn set_active_session_id(id: Option<String>) {
    ACTIVE_SESSION_ID.with(|f| {
        *f.borrow_mut() = id;
    });
}

pub fn get_active_session_id() -> Option<String> {
    ACTIVE_SESSION_ID.with(|f| f.borrow().clone())
}

#[cfg(test)]
pub fn set_active_workspace_root(root: Option<PathBuf>) {
    ACTIVE_WORKSPACE_ROOT.with(|current| {
        *current.borrow_mut() = root.clone();
    });
    // Compatibility for callers that only know the old single-root API:
    // absent an explicit task directory, the boundary was also the project
    // scope.
    ACTIVE_TASK_WORKING_DIRECTORY.with(|current| *current.borrow_mut() = root);
    if ACTIVE_WORKSPACE_ROOT.with(|current| current.borrow().is_none()) {
        ACTIVE_TASK_SCOPE_ESCAPE.with(|current| *current.borrow_mut() = false);
    }
}

pub fn set_active_workspace_context(
    workspace_root: Option<PathBuf>,
    task_working_directory: Option<PathBuf>,
    allow_task_scope_escape: bool,
) {
    ACTIVE_WORKSPACE_ROOT.with(|current| *current.borrow_mut() = workspace_root);
    ACTIVE_TASK_WORKING_DIRECTORY.with(|current| {
        *current.borrow_mut() = task_working_directory;
    });
    ACTIVE_TASK_SCOPE_ESCAPE.with(|current| *current.borrow_mut() = allow_task_scope_escape);
}

pub(crate) fn current_tool_context() -> rustcode_tools::ToolContext {
    let workspace_root = ACTIVE_WORKSPACE_ROOT.with(|current| current.borrow().clone());
    let task_working_directory =
        ACTIVE_TASK_WORKING_DIRECTORY.with(|current| current.borrow().clone());
    let allow_task_scope_escape = ACTIVE_TASK_SCOPE_ESCAPE.with(|current| *current.borrow());
    let (sandbox_dir, artifacts_dir) = get_active_session_id()
        .map(|session_id| {
            (
                crate::config::get_active_session_sandbox_dir(&session_id),
                crate::config::get_active_session_artifacts_dir(&session_id),
            )
        })
        .unwrap_or((None, None));
    rustcode_tools::ToolContext {
        workspace_root,
        task_working_directory,
        sandbox_dir,
        artifacts_dir,
        allow_task_scope_escape,
    }
}

pub(crate) fn active_task_working_directory() -> Option<PathBuf> {
    ACTIVE_TASK_WORKING_DIRECTORY.with(|current| current.borrow().clone())
}

pub(crate) fn resolve_tool_path(raw_path: &str) -> PathBuf {
    rustcode_tools::resolve_tool_path_with_context(raw_path, &current_tool_context())
}

pub(crate) fn parse_json_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(s) = v.as_str() {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

/// Read a JSON array argument, tolerating a provider that delivered it as a
/// stringified JSON array (`"[{...}]"`) instead of a real array — some strict
/// function-calling backends do this despite the schema.
pub(crate) fn coerce_array(v: &Value) -> Option<Vec<Value>> {
    if let Some(a) = v.as_array() {
        return Some(a.clone());
    }
    if let Some(s) = v.as_str()
        && let Ok(Value::Array(a)) = serde_json::from_str::<Value>(s)
    {
        return Some(a);
    }
    None
}

pub(crate) fn parse_json_bool(v: &Value) -> Option<bool> {
    if let Some(b) = v.as_bool() {
        Some(b)
    } else if let Some(s) = v.as_str() {
        match s.to_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

/// A fully self-contained built-in tool definition. Adding a new built-in
/// tool means writing one `pub const …: Tool` literal in the module that holds
/// its handler and referencing it from the `TOOLS` slice below — no other
/// tables need updating, since schema, capabilities, and safety all live here.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,

    pub arguments: &'static str,
    pub handler: fn(&Value) -> Result<String, String>,
    /// If true, the agent loop will pause and show a Y/N confirmation modal
    /// to the user before executing. Use for destructive tools (write, create, run).
    pub requires_confirmation: bool,
    /// Canonical JSON Schema advertised to API-native providers. The text
    /// protocol still uses `arguments` as compact documentation, but native
    /// providers must receive real types, required fields, and nested item
    /// schemas.
    pub schema: fn() -> Value,
    /// Runtime capabilities used to enforce agent modes and safety policy.
    pub capabilities: &'static [ToolCapability],
    /// Execution safety class used by the scheduler to decide which calls may
    /// safely run concurrently.
    pub safety: ToolSafety,
}

/// Runtime capabilities used to enforce agent modes and safety policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCapability {
    ReadWorkspace,
    WriteWorkspace,
    ExecuteCommands,
    Network,
    UserInteraction,
    AgentDelegation,
    SessionState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Allow,
    RequireConfirmation,
    Deny(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShellAssessment {
    pub(crate) cache_key: String,
    pub(crate) call_signature: String,
    pub(crate) local_authorization: AuthorizationDecision,
}

pub(crate) type ShellAssessmentCache = std::collections::HashMap<String, ShellAssessment>;

pub(crate) fn shell_call_signature(call: &ToolCall) -> String {
    duplicate_tool_call_key(call)
}

pub(crate) fn shell_assessment_cache_key(call: &ToolCall) -> String {
    if let Some(call_id) = call.call_id.as_deref().filter(|id| !id.is_empty()) {
        return format!("call:{call_id}");
    }
    let mut digest = Sha256::new();
    digest.update(shell_call_signature(call).as_bytes());
    format!("signature:{:x}", digest.finalize())
}

pub(crate) fn shell_assessment_matches_call(assessment: &ShellAssessment, call: &ToolCall) -> bool {
    assessment.cache_key == shell_assessment_cache_key(call)
        && assessment.call_signature == shell_call_signature(call)
}

pub(crate) fn shell_assessment_for_call<'a>(
    cache: &'a ShellAssessmentCache,
    call: &ToolCall,
) -> Option<&'a ShellAssessment> {
    cache
        .get(&shell_assessment_cache_key(call))
        .filter(|assessment| shell_assessment_matches_call(assessment, call))
}

pub(crate) fn assess_shell_call(
    call: &ToolCall,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
) -> Option<ShellAssessment> {
    if call.name != "run_command" {
        return None;
    }
    let local_authorization =
        authorize_tool_with_args(&call.name, &call.arguments, mode, auto_confirm, false);
    Some(ShellAssessment {
        cache_key: shell_assessment_cache_key(call),
        call_signature: shell_call_signature(call),
        local_authorization,
    })
}

pub(crate) fn execution_authorization(
    name: &str,
    args: &Value,
    call_id: Option<&str>,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    bypass_confirmation: bool,
    assessment: Option<&ShellAssessment>,
) -> AuthorizationDecision {
    let existing = authorize_tool_with_args(name, args, mode, auto_confirm, bypass_confirmation);
    if name != "run_command" {
        return existing;
    }

    let call = ToolCall {
        name: name.to_string(),
        arguments: args.clone(),
        call_id: call_id.map(str::to_owned),
    };
    let matching_assessment =
        assessment.filter(|assessment| shell_assessment_matches_call(assessment, &call));
    if let Some(assessment) = matching_assessment {
        let current_local = authorize_tool_with_args(name, args, mode, auto_confirm, false);
        if matches!(current_local, AuthorizationDecision::Deny(_)) {
            return current_local;
        }
        if current_local == assessment.local_authorization {
            if bypass_confirmation
                && assessment.local_authorization == AuthorizationDecision::RequireConfirmation
            {
                return AuthorizationDecision::Allow;
            }
            return assessment.local_authorization.clone();
        }
        return current_local;
    }
    existing
}

/// Single authorization policy used by every execution path. Unknown tools
/// are never silently treated as safe; registered MCP tools must still opt in
/// through confirmation unless the caller has explicitly bypassed it.
#[cfg(test)]
pub fn authorize_tool(
    name: &str,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    bypass_confirmation: bool,
) -> AuthorizationDecision {
    authorize_tool_with_args(name, &Value::Null, mode, auto_confirm, bypass_confirmation)
}

pub fn authorize_tool_with_args(
    name: &str,
    args: &Value,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    bypass_confirmation: bool,
) -> AuthorizationDecision {
    if mode == crate::config::AgentMode::Plan && !allowed_in_plan_mode(name) {
        return AuthorizationDecision::Deny(
            "Plan mode blocks workspace mutation, command execution, delegation, and unknown tools"
                .to_string(),
        );
    }
    let command_is_destructive = name == "run_command" && command_requires_confirmation(args);
    let requires_confirmation = if name == "run_command" {
        command_is_destructive
    } else {
        needs_confirmation(name)
    };
    if !bypass_confirmation
        && !auto_confirm
        && (requires_confirmation || matches!(tool_safety(name), ToolSafety::Unknown))
    {
        return AuthorizationDecision::RequireConfirmation;
    }
    AuthorizationDecision::Allow
}

/// Return the capabilities of a built-in or agent tool.
/// Unknown tools (including MCP tools) deliberately receive no capabilities;
/// callers must opt them into a mode explicitly instead of assuming safety.
pub fn tool_capabilities(name: &str) -> &'static [ToolCapability] {
    use ToolCapability::*;
    if let Some(tool) = TOOLS.iter().find(|t| t.name == name) {
        return tool.capabilities;
    }
    // Agent tools live outside `TOOLS`; keep their capabilities here.
    match name {
        "spawn_agent" | "send_agent" | "set_goal" => &[AgentDelegation, SessionState],
        "todo_write" => &[SessionState],
        _ => &[],
    }
}

/// Plan mode is intentionally deny-by-default for tools not explicitly known
/// to be read-only or user-facing.
pub fn allowed_in_plan_mode(name: &str) -> bool {
    use ToolCapability::*;
    let capabilities = tool_capabilities(name);
    capabilities.iter().all(|cap| {
        matches!(
            cap,
            ReadWorkspace | Network | UserInteraction | SessionState
        )
    }) && (capabilities.contains(&ReadWorkspace)
        || capabilities.contains(&Network)
        || capabilities.contains(&UserInteraction)
        || name == "get_time"
        || name == "use_skill"
        || name == "todo_write")
}

/// Registry of built-in tools. Each entry is a self-contained `Tool`
/// definition colocated with its handler in the sibling module; this slice
/// only fixes the ordering in which tools are advertised.
pub const TOOLS: &[Tool] = &[
    misc::ASK_QUESTION,
    misc::GET_TIME,
    misc::SET_SESSION_TITLE,
    misc::LIST_MCP_TOOLS,
    misc::WAIT_AGENT,
    misc::CANCEL_AGENT,
    #[cfg(unix)]
    misc::MANAGE_SCHEDULED_JOBS,
    search::GREP,
    search::GLOB,
    search::LIST_DIRECTORY,
    git::GIT_STATUS,
    git::GIT_DIFF,
    git::GIT_ADD,
    git::GIT_COMMIT,
    openapi::OPENAPI_CALL,
    delegate::DELEGATE_TASK,
    filesystem::DELETE_FILE,
    filesystem::MOVE_FILE,
    filesystem::COPY_FILE,
    exec::RUN_COMMAND,
    exec::MANAGE_TASK,
    misc::SEARCH_WEB,
    search::FIND_SYMBOL,
    search::GET_PROJECT_MAP,
    filesystem::VIEW_FILE,
    filesystem::REPLACE_FILE_CONTENT,
    filesystem::MULTI_REPLACE_FILE_CONTENT,
    filesystem::WRITE_TO_FILE,
    filesystem::WRITE_FILE_CHUNK,
    misc::COMPLETE_TASK,
    misc::LIST_SKILLS,
    misc::USE_SKILL,
    misc::REMEMBER,
    misc::RECALL_MEMORY,
    misc::FORGET_MEMORY,
    audio::GENERATE_SOUND_EFFECT,
    audio::GENERATE_MUSIC,
    audio::INSPECT_AUDIO,
    video::INSPECT_MEDIA,
    video::VALIDATE_VIDEO_PROJECT,
    video::RENDER_VIDEO,
];

pub fn is_agent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent" | "send_agent" | "wait_agent" | "cancel_agent" | "set_goal" | "todo_write"
    )
}

/// Execution capability used by the scheduler to decide which calls may
/// safely run concurrently. Unknown and stateful tools are conservative by
/// default and must not be parallelized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSafety {
    #[allow(dead_code)]
    ControlPlane,
    ReadOnly,
    WorkspaceMutation,
    ProcessControl,
    Interactive,
    Delegation,
    Unknown,
}

pub fn tool_safety(name: &str) -> ToolSafety {
    if let Some(tool) = TOOLS.iter().find(|t| t.name == name) {
        return tool.safety;
    }
    // Tools that live outside `TOOLS`: the agent tools, plus the legacy
    // `background_output`/`write_stdin` names kept for safety classification.
    match name {
        "spawn_agent" | "send_agent" | "set_goal" | "todo_write" => ToolSafety::Delegation,
        "background_output" | "write_stdin" => ToolSafety::ProcessControl,
        _ => ToolSafety::Unknown,
    }
}

pub fn supports_parallel_execution(name: &str) -> bool {
    matches!(tool_safety(name), ToolSafety::ReadOnly)
}

/// Whether a single call is read-only inspection that never consumes the
/// mutation budget. Native read tools (see [`supports_parallel_execution`])
/// qualify, as do `run_command` calls whose shell text needs no confirmation
/// under the existing command policy — e.g. `git status`, `ls`, `cat`, or a
/// `rg … | head` pipeline. Anything unclassified stays mutating: a missing
/// command, an unknown binary, or a write-like shell (`cargo test`, `rm`,
/// redirections) still counts toward the limit. MCP tools outside the builtin
/// registry opt into inspection with the standard `readOnlyHint` annotation
/// (same rule the replay policy uses for repeatable reads).
/// Evaluate this before the mutating cap in every batch path, including
/// recovery, so inspection is never dropped or reprimanded for budget reasons.
pub fn is_read_only_call(call: &ToolCall) -> bool {
    if supports_parallel_execution(&call.name) {
        return true;
    }
    if call.name == "run_command" {
        return !command_requires_confirmation(&call.arguments);
    }
    if matches!(tool_safety(&call.name), ToolSafety::Unknown) {
        return mcp_tool_read_only_hint(&call.name);
    }
    false
}

/// Enforce a control-plane barrier. A control-plane call such as `use_skill`
/// must execute alone so its result can affect the next model request before
/// any side-effecting call from the same response is considered.
#[cfg(test)]
pub fn isolate_control_plane_call(calls: Vec<ToolCall>) -> (Vec<ToolCall>, usize) {
    let Some(index) = calls
        .iter()
        .position(|call| matches!(tool_safety(&call.name), ToolSafety::ControlPlane))
    else {
        return (calls, 0);
    };

    let control_call = calls[index].clone();
    (vec![control_call], calls.len().saturating_sub(1))
}
