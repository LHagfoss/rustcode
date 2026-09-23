use crate::app::{AppState, TokenUsage};
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

use super::lifecycle::{StreamFailure, StreamFailureKind, StreamTermination};
use super::retry;
use super::stream::{NativeToolCallCheckpoint, StreamBuffer};
use super::{align_alternating_messages, count_tokens, parse_sse_line};
use crate::network::CONTEXT_PREFLIGHT_STOP_PREFIX;

const RESPONSE_BODY_DECODE_ERROR: &str = "error decoding response body";

/// Tracks streamed tool-fence markers, including markers split across chunks.
#[cfg(test)]
#[derive(Default)]
struct ToolFenceCounter {
    seen: usize,
    tail: String,
}

#[cfg(test)]
impl ToolFenceCounter {
    const MARKER: &'static str = "```tool";

    fn push(&mut self, chunk: &str) -> usize {
        if chunk.is_empty() {
            return self.seen;
        }
        let mut window = std::mem::take(&mut self.tail);
        window.push_str(chunk);
        self.seen += window.matches(Self::MARKER).count();
        let carry = Self::MARKER.len() - 1;
        let mut kept: Vec<char> = window.chars().rev().take(carry).collect();
        kept.reverse();
        self.tail = kept.into_iter().collect();
        self.seen
    }
}

#[cfg(test)]
mod fence_counter_tests {
    use super::ToolFenceCounter;

    #[test]
    fn survives_chunk_boundaries() {
        let mut counter = ToolFenceCounter::default();
        assert_eq!(counter.push("some text ``"), 0);
        assert_eq!(counter.push("`to"), 0);
        assert_eq!(counter.push("ol\n{\"name\": \"grep\"}"), 1);
        assert_eq!(counter.push("```tool\n{}\n```\n```tool\n{}"), 3);
        assert_eq!(counter.push(" and then I will check the results"), 3);
    }
}

fn apply_profile_generation_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
    thinking_mode: ThinkingMode,
) {
    // `chat_template_kwargs` is an oMLX/Qwen extension, not part of the
    // OpenAI-compatible request contract. Profiles that do not explicitly
    // configure thinking must leave both thinking controls untouched so
    // providers such as Groq do not reject an otherwise ordinary request.
    let has_thinking_control = profile.is_some_and(|p| p.enable_thinking.is_some());
    let has_chat_template_controls =
        profile.is_some_and(|p| p.enable_thinking.is_some() || p.preserve_thinking.is_some());

    if thinking_mode == ThinkingMode::Disabled
        || profile.is_some_and(|p| p.enable_thinking == Some(false))
    {
        if has_thinking_control {
            payload["enable_thinking"] = serde_json::json!(false);
        }
        if has_chat_template_controls {
            payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(false);
        }
        return;
    }
    if thinking_mode == ThinkingMode::BoundedRecovery {
        // Recovery exists to escape a planning/no-progress loop. Giving the
        // model another reasoning budget here recreates the exact condition
        // that triggered recovery, especially on local models whose entire
        // 1024-token response can be consumed by <think> output. Keep tools
        // enabled, but make this one action-oriented retry non-thinking.
        if has_thinking_control {
            payload["enable_thinking"] = serde_json::json!(false);
        }
        if has_chat_template_controls {
            payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(false);
        }
        return;
    }
    if let Some(enable_thinking) = profile.and_then(|p| p.enable_thinking) {
        payload["enable_thinking"] = serde_json::json!(enable_thinking);
        if has_chat_template_controls {
            payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(enable_thinking);
        }
    }
    if let Some(effort) = profile
        .filter(|p| p.supports_reasoning_effort_wire())
        .and_then(|p| p.reasoning_effort.as_ref())
    {
        payload["reasoning_effort"] = serde_json::json!(effort);
    }
    if let Some(thinking_budget) = profile
        .filter(|p| p.supports_thinking_budget_wire())
        .and_then(|p| p.thinking_budget)
    {
        payload["thinking_budget"] = serde_json::json!(thinking_budget);
    }
}

fn apply_profile_sampling_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
) {
    let has_chat_template_controls =
        profile.is_some_and(|p| p.enable_thinking.is_some() || p.preserve_thinking.is_some());

    if let Some(temperature) = profile.and_then(|p| p.temperature) {
        payload["temperature"] = serde_json::json!(temperature);
    }
    if let Some(top_p) = profile.and_then(|p| p.top_p) {
        payload["top_p"] = serde_json::json!(top_p);
    }
    if let Some(top_k) = profile.and_then(|p| p.top_k) {
        payload["top_k"] = serde_json::json!(top_k);
    }
    if let Some(presence_penalty) = profile.and_then(|p| p.presence_penalty) {
        payload["presence_penalty"] = serde_json::json!(presence_penalty);
    }
    if let Some(force_sampling) = profile.and_then(|p| p.force_sampling) {
        payload["force_sampling"] = serde_json::json!(force_sampling);
    }
    if has_chat_template_controls
        && let Some(preserve_thinking) = profile.and_then(|p| p.preserve_thinking)
    {
        payload["chat_template_kwargs"]["preserve_thinking"] = serde_json::json!(preserve_thinking);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThinkingMode {
    Normal,
    BoundedRecovery,
    Disabled,
}

fn apply_api_native_tools(
    payload: &mut serde_json::Value,
    schema: Vec<serde_json::Value>,
    allow_tools: bool,
) {
    if allow_tools && !schema.is_empty() {
        payload["tools"] = serde_json::Value::Array(schema);
        payload["tool_choice"] = serde_json::json!("auto");
    }
}

/// Allow providers to return parallel native tool calls. RustCode's
/// scheduler remains the authoritative safety boundary: it executes every
/// valid read-only call, runs at most one workspace mutation per round
/// (more only for explicitly trusted batching profiles), and isolates
/// control-plane calls. Some gateways ignore this hint.
fn apply_provider_parallel_tool_call_policy(
    payload: &mut serde_json::Value,
    api_protocol: crate::config::ApiProtocol,
    allow_tools: bool,
) {
    if allow_tools
        && matches!(
            api_protocol,
            crate::config::ApiProtocol::ChatCompletions | crate::config::ApiProtocol::Responses
        )
        && payload
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    {
        payload["parallel_tool_calls"] = serde_json::json!(true);
    }
}

fn is_parallel_tool_calls_rejection(status: u16, body: &str) -> bool {
    if !matches!(status, 400 | 422) {
        return false;
    }
    let body = body.to_ascii_lowercase();
    let mentions_field =
        body.contains("parallel_tool_calls") || body.contains("parallel tool calls");
    mentions_field
        && [
            "invalid",
            "unknown",
            "unsupported",
            "unrecognized",
            "unexpected",
            "not allowed",
            "not support",
        ]
        .iter()
        .any(|marker| body.contains(marker))
}

fn is_openrouter_endpoint(url: &str) -> bool {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or_default();
    host.eq_ignore_ascii_case("openrouter.ai") || host.ends_with(".openrouter.ai")
}

fn bounded_openrouter_session_id(session_id: Option<&str>) -> Option<String> {
    let session_id = session_id?.trim();
    (!session_id.is_empty()).then(|| session_id.chars().take(256).collect())
}

fn apply_openrouter_session_affinity(
    payload: &mut serde_json::Value,
    url: &str,
    session_id: Option<&str>,
) {
    if is_openrouter_endpoint(url)
        && let Some(session_id) = bounded_openrouter_session_id(session_id)
    {
        payload["session_id"] = serde_json::json!(session_id);
    }
}

fn cache_usage_metrics(usage: &serde_json::Value) -> (Option<u32>, Option<u32>, Option<f64>) {
    let input_details = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"));
    let cached_tokens = input_details
        .and_then(|details| details.get("cached_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            input_details
                .and_then(|details| details.get("cache_read_input_tokens"))
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            usage
                .get("cached_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            usage
                .get("cache_read_input_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .map(|value| value as u32);
    let cache_write_tokens = input_details
        .and_then(|details| details.get("cache_write_tokens"))
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            input_details
                .and_then(|details| details.get("cache_creation_input_tokens"))
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            usage
                .get("cache_write_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            usage
                .get("cache_creation_input_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .map(|value| value as u32);
    let cache_discount = usage
        .get("cache_discount")
        .and_then(serde_json::Value::as_f64);
    (cached_tokens, cache_write_tokens, cache_discount)
}

fn provider_cache_observation(
    affinity_requested: bool,
    cached_tokens: Option<u32>,
    cache_write_tokens: Option<u32>,
) -> (&'static str, &'static str) {
    if !affinity_requested {
        return ("not_requested", "openrouter_session_affinity_unavailable");
    }
    if cached_tokens.is_some_and(|tokens| tokens > 0) {
        return ("hit", "provider_reported_cached_tokens");
    }
    if cache_write_tokens.is_some_and(|tokens| tokens > 0) {
        return ("write", "provider_reported_cache_write_without_read");
    }
    if cached_tokens == Some(0) || cache_write_tokens == Some(0) {
        return ("miss", "provider_reported_zero_cache_tokens");
    }
    ("unknown", "provider_did_not_report_cache_tokens")
}

/// Convert the internal Chat Completions-shaped history into the stateless
/// input-item form accepted by the OpenAI Responses API.
fn responses_input_from_messages(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut input = Vec::new();
    // A function_call / function_call_output run is one transaction: DeepSeek
    // rejects any other item type interleaved between them ("No tool output
    // found") even when every call_id has a matching output. Runtime notices
    // rendered as user/system text must not split the run, so buffer them
    // while calls are still awaiting outputs and flush once answered.
    let mut open_calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut buffered: Vec<serde_json::Value> = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("user");

        if role == "tool" {
            if let Some(call_id) = message
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
            {
                let output = response_message_text(message.get("content"));
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
                open_calls.remove(call_id);
                if open_calls.is_empty() {
                    input.append(&mut buffered);
                }
            }
            continue;
        }

        if role == "assistant"
            && let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array())
        {
            // Responses treats function calls and their outputs as one
            // transaction. Keep assistant prose before the function calls so
            // it cannot appear between a call and the outputs that answer it.
            // DeepSeek rejects that interleaving with "No tool output found"
            // even when every call_id has a matching function_call_output.
            let text = response_message_text(message.get("content"));
            if !text.is_empty() {
                input.push(serde_json::json!({
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text}],
                }));
            }

            for tool_call in tool_calls {
                let Some(function) = tool_call.get("function") else {
                    continue;
                };
                let Some(name) = function.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let call_id = tool_call
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("call_unknown");
                let arguments = function
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        function
                            .get("arguments")
                            .map_or_else(|| "{}".to_owned(), serde_json::Value::to_string)
                    });
                open_calls.insert(call_id.to_owned());
                input.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": arguments,
                }));
            }

            continue;
        }

        let text = response_message_text(message.get("content"));
        if !text.is_empty() {
            let content_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            let item = serde_json::json!({
                "role": role,
                "content": [{"type": content_type, "text": text}],
            });
            if open_calls.is_empty() {
                input.push(item);
            } else {
                // A runtime notice between calls and their outputs would
                // split the transaction; hold it until the run is answered.
                buffered.push(item);
            }
        }
    }
    input.append(&mut buffered);

    input
}

fn response_message_text(content: Option<&serde_json::Value>) -> String {
    let Some(content) = content else {
        return String::new();
    };
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("text")
                .and_then(|text| text.as_str())
                .or_else(|| item.get("content").and_then(|text| text.as_str()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn responses_tool_schemas(schemas: &[serde_json::Value]) -> Vec<serde_json::Value> {
    schemas
        .iter()
        .filter_map(|schema| {
            let function = schema.get("function")?;
            let name = function.get("name")?.as_str()?;
            let mut response_schema = serde_json::json!({
                "type": "function",
                "name": name,
                "description": function.get("description").cloned().unwrap_or(serde_json::Value::Null),
                "parameters": function.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({
                    "type": "object",
                    "properties": {},
                })),
            });
            if let Some(strict) = function.get("strict") {
                response_schema["strict"] = strict.clone();
            }
            Some(response_schema)
        })
        .collect()
}

fn apply_responses_generation_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
    thinking_mode: ThinkingMode,
) {
    if thinking_mode != ThinkingMode::Normal {
        return;
    }
    if let Some(effort) = profile
        .filter(|p| p.supports_reasoning_effort_wire())
        .and_then(|p| p.reasoning_effort.as_ref())
    {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
}

fn apply_responses_sampling_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
) {
    if let Some(temperature) = profile.and_then(|p| p.temperature) {
        payload["temperature"] = serde_json::json!(temperature);
    }
    if let Some(top_p) = profile.and_then(|p| p.top_p) {
        payload["top_p"] = serde_json::json!(top_p);
    }
}

fn responses_usage(value: &serde_json::Value) -> Option<serde_json::Value> {
    let usage = value
        .get("response")
        .and_then(|response| response.get("usage"))?;
    let prompt = usage.get("input_tokens").and_then(|v| v.as_u64())?;
    let completion = usage.get("output_tokens").and_then(|v| v.as_u64())?;
    let total = usage
        .get("total_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(prompt.saturating_add(completion));
    let mut normalized = serde_json::json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total,
    });
    let (cached, cache_write, cache_discount) = cache_usage_metrics(usage);
    if cached.is_some() || cache_write.is_some() {
        normalized["prompt_tokens_details"] = serde_json::json!({
            "cached_tokens": cached,
            "cache_write_tokens": cache_write,
        });
    }
    if let Some(cache_discount) = cache_discount {
        normalized["cache_discount"] = serde_json::json!(cache_discount);
    }
    if let Some(reasoning) = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(|v| v.as_u64())
    {
        normalized["completion_tokens_details"] =
            serde_json::json!({"reasoning_tokens": reasoning});
    }
    Some(normalized)
}

/// Normalize Responses stream events to the small internal shape consumed by
/// the existing stream/tool/reasoning state machine.
fn normalize_responses_event(
    value: &serde_json::Value,
    response_call_ids: &mut HashMap<String, String>,
    response_argument_deltas: &mut HashSet<usize>,
) -> Option<serde_json::Value> {
    let event_type = value.get("type").and_then(|v| v.as_str())?;
    let delta = value.get("delta").and_then(|v| v.as_str());
    match event_type {
        "response.output_text.delta" => {
            delta.map(|delta| serde_json::json!({"choices": [{"delta": {"content": delta}}]}))
        }
        "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
            delta.map(|delta| serde_json::json!({"choices": [{"delta": {"reasoning": delta}}]}))
        }
        "response.output_item.added" => {
            let item = value.get("item")?;
            if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                return None;
            }
            let item_id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or(item_id);
            if !item_id.is_empty() {
                response_call_ids.insert(item_id.to_owned(), call_id.to_owned());
            }
            let index = value
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or_default();
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            Some(serde_json::json!({
                "choices": [{"delta": {"tool_calls": [{
                    "index": index,
                    "id": call_id,
                    "function": {"name": name, "arguments": ""}
                }]}}]
            }))
        }
        "response.function_call_arguments.delta" => {
            let index = value
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as usize;
            response_argument_deltas.insert(index);
            let item_id = value
                .get("item_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let call_id = response_call_ids
                .get(item_id)
                .map(String::as_str)
                .unwrap_or(item_id);
            delta.map(|delta| {
                serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                    "index": index,
                    "id": call_id,
                    "function": {"arguments": delta}
                }]}}]})
            })
        }
        "response.function_call_arguments.done" => {
            let index = value
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or_default() as usize;
            if response_argument_deltas.contains(&index) {
                return None;
            }
            let item_id = value
                .get("item_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let call_id = response_call_ids
                .get(item_id)
                .map(String::as_str)
                .unwrap_or(item_id);
            let arguments = value
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            Some(serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                "index": index,
                "id": call_id,
                "function": {"arguments": arguments}
            }]}}]}))
        }
        "response.completed" => Some(serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": "stop"}],
            "usage": responses_usage(value),
        })),
        "response.incomplete" => Some(serde_json::json!({
            "choices": [{"delta": {}, "finish_reason": "length"}],
            "usage": responses_usage(value),
        })),
        "response.failed" => {
            let message = value
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| error.get("message"))
                .and_then(|message| message.as_str())
                .unwrap_or("Responses API request failed");
            Some(serde_json::json!({"error": {"message": message}}))
        }
        "error" => {
            let message = value
                .get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("Responses API request failed");
            Some(serde_json::json!({"error": {"message": message}}))
        }
        _ => None,
    }
}

const RECOVERY_MAX_TOKENS: u32 = 1024;
const MAX_NATIVE_TOOL_ARGUMENT_BYTES: usize = 40 * 1024;
const MAX_INVALID_ARGUMENT_PREVIEW_BYTES: usize = 1024;
const MAX_PROVIDER_TRACE_EVENTS: usize = 256;
const MAX_PROVIDER_TRACE_BYTES: usize = 64 * 1024;
const PROVIDER_TRACE_METADATA_RESERVE: usize = 4096;
const MAX_PROVIDER_TRACE_CHOICES_PER_EVENT: usize = 8;
const MAX_PROVIDER_TRACE_TOOL_CALLS_PER_CHOICE: usize = 64;
const MAX_NATIVE_CHECKPOINT_CALLS: usize = 64;
const MAX_NATIVE_CHECKPOINT_TEXT_BYTES: usize = 256;

fn structural_string(value: Option<&str>) -> serde_json::Value {
    let Some(value) = value else {
        return serde_json::json!({"present": false});
    };
    // A short non-reversible fingerprint lets a bug report show whether two
    // deltas carried the same opaque ID/name without copying provider data.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    serde_json::json!({
        "present": true,
        "bytes": value.len(),
        "fingerprint": format!("{hash:016x}"),
    })
}

fn safe_finish_reason(value: Option<&str>) -> Option<&'static str> {
    match value {
        None => None,
        Some("stop") => Some("stop"),
        Some("length") => Some("length"),
        Some("tool_calls") => Some("tool_calls"),
        Some("function_call") => Some("function_call"),
        Some("content_filter") => Some("content_filter"),
        Some(_) => Some("other"),
    }
}

/// Convert one provider event into metadata suitable for a bug report. This
/// intentionally never copies content, prompts, headers, or argument values.
fn sanitized_provider_stream_event(
    line_bytes: usize,
    value: &serde_json::Value,
) -> serde_json::Value {
    let choices = value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .map(|choices| {
            choices
                .iter()
                .take(MAX_PROVIDER_TRACE_CHOICES_PER_EVENT)
                .map(|choice| {
                    let delta = choice.get("delta");
                    let tool_calls = delta
                        .and_then(|delta| delta.get("tool_calls"))
                        .and_then(|tool_calls| tool_calls.as_array())
                        .map(|tool_calls| {
                            tool_calls
                                .iter()
                                .take(MAX_PROVIDER_TRACE_TOOL_CALLS_PER_CHOICE)
                                .map(|tool_call| {
                                    let function = tool_call.get("function");
                                    let arguments = function
                                        .and_then(|function| function.get("arguments"))
                                        .and_then(|arguments| arguments.as_str());
                                    serde_json::json!({
                                        "index": tool_call.get("index").and_then(|index| index.as_u64()),
                                        "id": structural_string(tool_call.get("id").and_then(|id| id.as_str())),
                                        "name": structural_string(function.and_then(|function| function.get("name")).and_then(|name| name.as_str())),
                                        "arguments_bytes": arguments.map_or(0, str::len),
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    serde_json::json!({
                        "finish_reason": safe_finish_reason(choice.get("finish_reason").and_then(|reason| reason.as_str())),
                        "content_bytes": delta
                            .and_then(|delta| delta.get("content").or_else(|| delta.get("text")))
                            .and_then(|content| content.as_str())
                            .map_or(0, str::len),
                        "reasoning_bytes": delta
                            .and_then(|delta| delta.get("reasoning").or_else(|| delta.get("reasoning_content")).or_else(|| delta.get("thought")).or_else(|| delta.get("thinking")))
                            .and_then(|reasoning| reasoning.as_str())
                            .map_or(0, str::len),
                        "tool_calls": tool_calls,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let usage = value.get("usage").and_then(|usage| {
        let number = |name| usage.get(name).and_then(|value| value.as_u64());
        if number("prompt_tokens").is_some()
            || number("completion_tokens").is_some()
            || number("total_tokens").is_some()
        {
            Some(serde_json::json!({
                "prompt_tokens": number("prompt_tokens"),
                "completion_tokens": number("completion_tokens"),
                "total_tokens": number("total_tokens"),
                "cached_tokens": cache_usage_metrics(usage).0,
                "cache_write_tokens": cache_usage_metrics(usage).1,
                "cache_discount": cache_usage_metrics(usage).2,
            }))
        } else {
            None
        }
    });
    serde_json::json!({
        "bytes": line_bytes,
        "choices": choices,
        "usage": usage,
    })
}

struct ProviderStreamTrace {
    enabled: bool,
    session_id: String,
    model: String,
    assistant_turn: usize,
    events: Vec<serde_json::Value>,
    event_bytes: usize,
    dropped_events: usize,
    finish_reason: Option<&'static str>,
    response_status: Option<u16>,
}

impl ProviderStreamTrace {
    fn new(enabled: bool, session_id: &str, model: &str, assistant_turn: usize) -> Self {
        Self {
            enabled,
            session_id: session_id.chars().take(128).collect(),
            model: model.chars().take(128).collect(),
            assistant_turn,
            events: Vec::new(),
            event_bytes: 0,
            dropped_events: 0,
            finish_reason: None,
            response_status: None,
        }
    }

    fn response_headers(&mut self, status: u16) {
        self.response_status = Some(status);
    }

    fn record(&mut self, line_bytes: usize, value: &serde_json::Value) {
        if !self.enabled {
            return;
        }
        let mut event = sanitized_provider_stream_event(line_bytes, value);
        event["sequence"] = serde_json::json!(self.events.len() + self.dropped_events + 1);
        if self.events.len() >= MAX_PROVIDER_TRACE_EVENTS {
            self.dropped_events += 1;
            return;
        }
        let event_bytes = serde_json::to_vec(&event).map_or(0, |bytes| bytes.len());
        if self.event_bytes.saturating_add(event_bytes)
            > MAX_PROVIDER_TRACE_BYTES.saturating_sub(PROVIDER_TRACE_METADATA_RESERVE)
        {
            self.dropped_events += 1;
            return;
        }
        self.event_bytes = self.event_bytes.saturating_add(event_bytes);
        if let Some(reason) = event["choices"].as_array().and_then(|choices| {
            choices
                .iter()
                .find_map(|choice| choice["finish_reason"].as_str())
        }) {
            self.finish_reason = Some(match reason {
                "stop" => "stop",
                "length" => "length",
                "tool_calls" => "tool_calls",
                "function_call" => "function_call",
                "content_filter" => "content_filter",
                _ => "other",
            });
        }
        self.events.push(event);
    }

    fn record_malformed(&mut self, line_bytes: usize) {
        if !self.enabled {
            return;
        }
        let mut event = sanitized_provider_stream_event(line_bytes, &serde_json::json!({}));
        event["sequence"] = serde_json::json!(self.events.len() + self.dropped_events + 1);
        event["malformed_json"] = serde_json::json!(true);
        let event_bytes = serde_json::to_vec(&event).map_or(0, |bytes| bytes.len());
        if self.events.len() >= MAX_PROVIDER_TRACE_EVENTS
            || self.event_bytes.saturating_add(event_bytes)
                > MAX_PROVIDER_TRACE_BYTES.saturating_sub(PROVIDER_TRACE_METADATA_RESERVE)
        {
            self.dropped_events += 1;
            return;
        }
        self.event_bytes = self.event_bytes.saturating_add(event_bytes);
        self.events.push(event);
    }

    #[cfg(test)]
    fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "session_id": self.session_id,
            "model": self.model,
            "assistant_turn": self.assistant_turn,
            "response_status": self.response_status,
            "finish_reason": self.finish_reason,
            "events": self.events,
            "event_count": self.events.len(),
            "dropped_events": self.dropped_events,
            "trace_bytes": self.event_bytes,
        })
    }
}

impl Drop for ProviderStreamTrace {
    fn drop(&mut self) {
        if self.enabled {
            crate::logger::operational_event(
                "provider.stream_trace",
                serde_json::json!({
                    "session_id": self.session_id,
                    "model": self.model,
                    "assistant_turn": self.assistant_turn,
                    "response_status": self.response_status,
                    "finish_reason": self.finish_reason,
                    "events": self.events,
                    "event_count": self.events.len(),
                    "dropped_events": self.dropped_events,
                    "trace_bytes": self.event_bytes,
                    "max_events": MAX_PROVIDER_TRACE_EVENTS,
                    "max_bytes": MAX_PROVIDER_TRACE_BYTES,
                }),
            );
        }
    }
}

#[derive(Debug, Default)]
struct ToolAccumulatorSet {
    calls: Vec<ToolAccumulator>,
    by_index: HashMap<usize, usize>,
    by_id: HashMap<String, usize>,
}

#[derive(Debug)]
struct ToolAccumulator {
    index: Option<usize>,
    id: String,
    name: String,
    arguments: String,
    argument_bytes: usize,
    arguments_overflowed: bool,
}

impl ToolAccumulator {
    fn new() -> Self {
        Self {
            index: None,
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
            argument_bytes: 0,
            arguments_overflowed: false,
        }
    }
}

impl ToolAccumulatorSet {
    /// Resolve a streamed tool call without treating a missing index as index
    /// zero. Providers commonly omit `index` while still sending stable IDs.
    /// If neither identity is present, each delta gets a new accumulator: this
    /// deliberately fails closed instead of guessing that unrelated fragments
    /// belong to one JSON document.
    fn resolve(&mut self, index: Option<usize>, id: Option<&str>) -> usize {
        let call_index = index
            .and_then(|index| self.by_index.get(&index).copied())
            .or_else(|| id.and_then(|id| self.by_id.get(id).copied()))
            .unwrap_or_else(|| {
                let index = self.calls.len();
                self.calls.push(ToolAccumulator::new());
                index
            });

        if let Some(index) = index {
            self.by_index.insert(index, call_index);
        }
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            self.by_id.insert(id.to_owned(), call_index);
        }
        call_index
    }

    fn add_delta(&mut self, tc: &serde_json::Value) -> usize {
        let index = tc
            .get("index")
            .and_then(|index| index.as_u64())
            .map(|index| index as usize);
        let id = tc.get("id").and_then(|id| id.as_str());
        let call_index = self.resolve(index, id);
        let acc = &mut self.calls[call_index];
        if let Some(index) = index {
            acc.index = Some(index);
        }
        if acc.id.is_empty() {
            if let Some(id) = id.filter(|id| !id.is_empty()) {
                acc.id = id.to_owned();
            }
        }
        if acc.name.is_empty() {
            if let Some(name) = tc
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(|name| name.as_str())
            {
                acc.name = name.to_owned();
            }
        }
        if let Some(arguments) = tc
            .get("function")
            .and_then(|function| function.get("arguments"))
            .and_then(|arguments| arguments.as_str())
        {
            acc.argument_bytes = acc.argument_bytes.saturating_add(arguments.len());
            if append_bounded_native_arguments(&mut acc.arguments, arguments) {
                acc.arguments_overflowed = true;
            }
        }
        call_index
    }

    fn checkpoints(&self) -> Vec<NativeToolCallCheckpoint> {
        self.calls
            .iter()
            .take(MAX_NATIVE_CHECKPOINT_CALLS)
            .filter(|call| {
                !call.id.is_empty() || !call.name.is_empty() || !call.arguments.is_empty()
            })
            .map(|call| {
                let (parsed_complete, mut diagnostic) =
                    match serde_json::from_str::<serde_json::Value>(&call.arguments) {
                        Ok(value) if value.is_object() => (true, String::new()),
                        Ok(_) => (false, "tool arguments must be a JSON object".to_owned()),
                        Err(error) => (false, bounded_checkpoint_text(&error.to_string())),
                    };
                let arguments_complete = if call.arguments_overflowed {
                    if diagnostic.is_empty() {
                        diagnostic = "tool arguments exceeded the local streaming limit".to_owned();
                    }
                    false
                } else {
                    parsed_complete
                };
                NativeToolCallCheckpoint {
                    index: call.index,
                    call_id: (!call.id.is_empty()).then(|| bounded_checkpoint_text(&call.id)),
                    tool_name: bounded_checkpoint_text(&call.name),
                    argument_bytes: call.argument_bytes,
                    arguments_complete,
                    arguments_overflowed: call.arguments_overflowed,
                    argument_fingerprint: argument_fingerprint(&call.arguments),
                    diagnostic,
                }
            })
            .collect()
    }
}

fn bounded_checkpoint_text(value: &str) -> String {
    let end = value.floor_char_boundary(MAX_NATIVE_CHECKPOINT_TEXT_BYTES.min(value.len()));
    let mut bounded = value[..end]
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if end < value.len() {
        bounded.push('…');
    }
    bounded
}

fn argument_fingerprint(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn clamp_request_max_tokens(max_tokens: u32, thinking_mode: ThinkingMode) -> u32 {
    if thinking_mode == ThinkingMode::BoundedRecovery {
        max_tokens.min(RECOVERY_MAX_TOKENS)
    } else {
        max_tokens
    }
}

fn apply_output_token_limit(
    payload: &mut serde_json::Value,
    field: crate::config::OutputTokenField,
    limit: Option<u32>,
) {
    if let Some(limit) = limit {
        payload[field.wire_name()] = serde_json::json!(limit);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BoundedReasoningChunk {
    text: String,
    estimated_tokens: u32,
    budget_exhausted: bool,
}

fn estimated_reasoning_tokens(text: &str) -> u32 {
    if text.is_empty() {
        0
    } else {
        ((text.len() as f64 * crate::app::TOKENS_PER_CHAR_APPROX).ceil() as u32).max(1)
    }
}

fn bound_reasoning_chunk(
    text: &str,
    used_tokens: u32,
    budget: Option<u32>,
) -> BoundedReasoningChunk {
    let Some(budget) = budget else {
        return BoundedReasoningChunk {
            text: text.to_string(),
            estimated_tokens: estimated_reasoning_tokens(text),
            budget_exhausted: false,
        };
    };
    let remaining = budget.saturating_sub(used_tokens);
    let full_estimate = estimated_reasoning_tokens(text);
    if full_estimate <= remaining {
        return BoundedReasoningChunk {
            text: text.to_string(),
            estimated_tokens: full_estimate,
            budget_exhausted: false,
        };
    }

    let max_bytes = ((remaining as f64) / crate::app::TOKENS_PER_CHAR_APPROX) as usize;
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let bounded = &text[..end];
    BoundedReasoningChunk {
        text: bounded.to_string(),
        estimated_tokens: estimated_reasoning_tokens(bounded).min(remaining),
        budget_exhausted: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn silent_sse_stream_returns_after_idle_timeout() {
        let (_writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let read = read_sse_line(&mut reader, &mut line);
        tokio::pin!(read);

        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1))
            .await;

        let error = read.await.expect_err("silent stream must not hang forever");
        assert!(error.to_string().contains("stream_idle_timeout"), "{error}");
    }

    #[tokio::test(start_paused = true)]
    async fn silent_sse_stream_before_first_event_has_distinct_timeout() {
        let (_writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let read = read_sse_line_with_state(&mut reader, &mut line, true);
        tokio::pin!(read);

        tokio::task::yield_now().await;
        tokio::time::advance(retry::FIRST_EVENT_TIMEOUT + std::time::Duration::from_millis(1))
            .await;

        let error = read
            .await
            .expect_err("first event must have a bounded wait");
        assert_eq!(error.kind(), StreamFailureKind::FirstEventTimeout);
        assert_eq!(error.partial_event_bytes(), 0);
    }

    #[tokio::test]
    async fn sse_data_before_idle_timeout_is_returned_normally() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let mut reader = BufReader::new(reader);
        let writer_task = tokio::spawn(async move {
            tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: ready\n")
                .await
                .unwrap();
        });
        let mut line = String::new();

        let bytes = read_sse_line(&mut reader, &mut line)
            .await
            .expect("data should arrive before the idle timeout");

        assert_eq!(bytes, "data: ready\n".len());
        assert_eq!(line, "data: ready\n");
        writer_task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn partial_sse_bytes_reset_idle_timeout_until_line_completes() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let read_task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let bytes = read_sse_line(&mut reader, &mut line)
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((bytes, line))
        });

        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data:")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
        tokio::io::AsyncWriteExt::write_all(&mut writer, b" still")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT - std::time::Duration::from_secs(1)).await;
        tokio::io::AsyncWriteExt::write_all(&mut writer, b" alive\n")
            .await
            .unwrap();

        let (bytes, line) = read_task.await.unwrap().unwrap();
        assert_eq!(bytes, "data: still alive\n".len());
        assert_eq!(line, "data: still alive\n");
    }

    #[tokio::test(start_paused = true)]
    async fn partial_sse_bytes_then_stall_returns_idle_timeout() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let read_task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            read_sse_line(&mut reader, &mut String::new()).await
        });

        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: partial")
            .await
            .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(retry::STREAM_IDLE_TIMEOUT + std::time::Duration::from_millis(1))
            .await;

        let error = read_task
            .await
            .unwrap()
            .expect_err("partial line must time out after a stall");
        assert!(error.to_string().contains("stream_idle_timeout"), "{error}");
    }

    #[tokio::test]
    async fn sse_progress_deadline_uses_first_event_budget_before_first_event() {
        let start = tokio::time::Instant::now();
        let first_event_deadline = start + retry::FIRST_EVENT_TIMEOUT;
        assert_eq!(
            sse_progress_deadline(first_event_deadline, start, 0),
            first_event_deadline
        );
    }

    #[tokio::test]
    async fn sse_progress_deadline_tracks_last_progress_after_first_event() {
        let start = tokio::time::Instant::now();
        let first_event_deadline = start + retry::FIRST_EVENT_TIMEOUT;
        let later = start + std::time::Duration::from_secs(3600);
        assert_eq!(
            sse_progress_deadline(first_event_deadline, later, 2),
            later + retry::STREAM_IDLE_TIMEOUT
        );
    }

    #[tokio::test(start_paused = true)]
    async fn keepalive_blank_lines_do_not_extend_first_event_deadline() {
        // Regression test for the hung-turn pattern: a provider or proxy that
        // emits periodic blank keep-alive lines must not postpone the
        // absolute first-event budget forever.
        let (mut writer, reader) = tokio::io::duplex(1024);
        let stream_start = tokio::time::Instant::now();
        let first_event_deadline = stream_start + retry::FIRST_EVENT_TIMEOUT;
        let read_task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut events = 0usize;
            let last_progress = stream_start;
            loop {
                let deadline = sse_progress_deadline(first_event_deadline, last_progress, events);
                let mut line = String::new();
                match tokio::time::timeout_at(deadline, read_sse_line(&mut reader, &mut line)).await
                {
                    Err(_) => return events,
                    Ok(Ok(0)) => return events,
                    Ok(Ok(_)) => {
                        if crate::network::parse_sse_line(line.trim()).is_some() {
                            events += 1;
                        }
                    }
                    Ok(Err(_)) => return events,
                }
            }
        });

        // Keep-alive blanks, each well inside the per-read budget.
        for _ in 0..3 {
            tokio::time::advance(retry::FIRST_EVENT_TIMEOUT / 4).await;
            tokio::task::yield_now().await;
            tokio::io::AsyncWriteExt::write_all(&mut writer, b"\n")
                .await
                .unwrap();
            tokio::task::yield_now().await;
        }
        // Push virtual time past the absolute first-event budget.
        tokio::time::advance(retry::FIRST_EVENT_TIMEOUT).await;
        tokio::task::yield_now().await;

        assert_eq!(
            read_task.await.unwrap(),
            0,
            "blank keep-alives must not count as progress"
        );
    }

    #[tokio::test]
    async fn sse_eof_returns_final_unterminated_line() {
        let (mut writer, reader) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut writer, b"data: final")
            .await
            .unwrap();
        drop(writer);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let bytes = read_sse_line(&mut reader, &mut line).await.unwrap();

        assert_eq!(bytes, "data: final".len());
        assert_eq!(line, "data: final");
    }

    async fn stream_test_state(
        endpoint: &str,
    ) -> (
        std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
        String,
    ) {
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(crate::app::AppState::new()));
        let session_id = {
            let mut state = state.lock().await;
            state.api_base_url = endpoint.to_owned();
            state.model_name = "stream-test".to_owned();
            state.config.models = vec![crate::config::ModelProfile {
                name: "stream-test".to_owned(),
                url: endpoint.to_owned(),
                model: "stream-test".to_owned(),
                context_window: Some(8_192),
                max_tokens: Some(1_024),
                ..Default::default()
            }];
            state.active_session_id.clone()
        };
        (state, session_id)
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;

        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let bytes = socket.read(&mut chunk).await.expect("read request");
            if bytes == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..bytes]);
        }
    }

    async fn write_sse_response(socket: &mut tokio::net::TcpStream, status: &str, body: &[u8]) {
        use tokio::io::AsyncWriteExt;

        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket
            .write_all(header.as_bytes())
            .await
            .expect("write response headers");
        socket.write_all(body).await.expect("write response body");
    }

    #[tokio::test]
    async fn stream_request_flushes_final_sse_content_and_records_usage() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"final answer\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"total_tokens\":15}}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            write_sse_response(&mut socket, "200 OK", &body).await;
            socket.shutdown().await.unwrap();
        });

        let (state, session_id) = stream_test_state(&endpoint).await;
        let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
        let finish = stream_request(
            &reqwest::Client::new(),
            state.clone(),
            tokio_util::sync::CancellationToken::new(),
            &endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            buffer.clone(),
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
        )
        .await
        .expect("SSE response should complete");

        assert_eq!(finish.as_deref(), Some("stop"));
        assert_eq!(buffer.lock().await.content, "final answer");
        assert_eq!(
            state.lock().await.current_token_usage,
            Some(crate::app::TokenUsage {
                prompt_tokens: 12,
                completion_tokens: 3,
                total_tokens: 15,
                ..Default::default()
            })
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stream_request_rejects_subagent_tool_result_interleaving_before_send() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            write_sse_response(
                &mut socket,
                "200 OK",
                b"data: {\"choices\":[{\"delta\":{\"content\":\"unexpected request\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            )
            .await;
            socket.shutdown().await.unwrap();
        });
        let (state, session_id) = stream_test_state(&endpoint).await;
        {
            let mut state = state.lock().await;
            state.config.models[0].tool_protocol = Some(crate::config::ToolProtocol::ApiNative);
        }

        // This is the subagent transcript shape after it drops one call from a
        // batch: the in-flight call/result pair has a system notice between
        // them. Alignment carries that notice as a user message, which breaks
        // the provider's native tool-call transaction.
        let messages = vec![
            serde_json::json!({"role": "system", "content": "subagent instructions"}),
            serde_json::json!({"role": "user", "content": "inspect"}),
            serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_kept",
                    "type": "function",
                    "function": {"name": "view_file", "arguments": "{}"}
                }]
            }),
            serde_json::json!({
                "role": "system",
                "content": "[Subagent tool batch kept 1 of 2 calls and dropped 1.]"
            }),
            serde_json::json!({
                "role": "tool", "tool_call_id": "call_kept", "content": "completed"
            }),
        ];

        let result = stream_request(
            &reqwest::Client::new(),
            state,
            tokio_util::sync::CancellationToken::new(),
            &endpoint,
            "stream-test",
            messages,
            std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
            true,
            true,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::subagent(),
            Some(&session_id),
            None,
        )
        .await;
        server.abort();
        let error = result.expect_err("interleaved native tool history must stop locally");

        assert_eq!(error.kind, StreamFailureKind::ProviderError);
        assert!(error.detail.as_deref().is_some_and(|detail| {
            detail.contains("Native tool history") && detail.contains("call_kept")
        }));
    }

    #[tokio::test]
    async fn stream_request_cancellation_interrupts_a_pending_response() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_http_request(&mut socket).await;
            accepted_tx.send(()).unwrap();
            let _ = release_rx.await;
            let _ = socket.shutdown().await;
        });

        let (state, session_id) = stream_test_state(&endpoint).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let client = reqwest::Client::new();
            stream_request(
                &client,
                state,
                task_cancel,
                &endpoint,
                "stream-test",
                vec![serde_json::json!({"role": "user", "content": "hello"})],
                std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new())),
                false,
                false,
                ThinkingMode::Normal,
                crate::tools::ToolSchemaPolicy::read_only_inspection(),
                Some(&session_id),
                None,
            )
            .await
        });
        accepted_rx.await.unwrap();
        cancel.cancel();
        let error = task.await.unwrap().expect_err("request should cancel");
        assert_eq!(error.kind, StreamFailureKind::Cancelled);
        release_tx.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stream_request_retries_a_retryable_http_response_before_streaming() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"retried\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .as_bytes()
        .to_vec();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            read_http_request(&mut first).await;
            write_sse_response(&mut first, "503 Service Unavailable", b"busy").await;
            first.shutdown().await.unwrap();

            let (mut second, _) = listener.accept().await.unwrap();
            read_http_request(&mut second).await;
            write_sse_response(&mut second, "200 OK", &body).await;
            second.shutdown().await.unwrap();
        });

        let (state, session_id) = stream_test_state(&endpoint).await;
        let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(StreamBuffer::new()));
        let finish = stream_request(
            &reqwest::Client::new(),
            state,
            tokio_util::sync::CancellationToken::new(),
            &endpoint,
            "stream-test",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            buffer.clone(),
            false,
            false,
            ThinkingMode::Normal,
            crate::tools::ToolSchemaPolicy::read_only_inspection(),
            Some(&session_id),
            None,
        )
        .await
        .expect("retry should reach the successful stream");

        assert_eq!(finish.as_deref(), Some("stop"));
        assert_eq!(buffer.lock().await.content, "retried");
        server.await.unwrap();
    }

    #[test]
    fn parse_speculative_arguments_extracts_complete_and_partial_json() {
        // Complete JSON
        let complete = r#"{"TargetFile": "src/symbols.rs", "Instruction": "fix bug"}"#;
        let parsed = parse_speculative_arguments(complete);
        assert_eq!(parsed["TargetFile"], "src/symbols.rs");
        assert_eq!(parsed["Instruction"], "fix bug");

        // Partial JSON cut mid-string
        let partial_string = r#"{"TargetFile": "src/main.rs", "Instruction": "refactor"#;
        let parsed_partial = parse_speculative_arguments(partial_string);
        assert_eq!(parsed_partial["TargetFile"], "src/main.rs");

        // Early partial JSON
        let early_partial = r#"{"CommandLine": "cargo check --tests""#;
        let parsed_early = parse_speculative_arguments(early_partial);
        assert_eq!(parsed_early["CommandLine"], "cargo check --tests");

        // Grep pattern
        let grep_partial = r#"{"pattern": "Config", "path": "src/""#;
        let parsed_grep = parse_speculative_arguments(grep_partial);
        assert_eq!(parsed_grep["pattern"], "Config");
        assert_eq!(parsed_grep["path"], "src/");
    }

    #[test]
    fn native_argument_markers_distinguish_syntax_from_shape() {
        let incomplete = parse_native_tool_arguments(r#"{"path":"src/main.rs""#);
        assert_eq!(
            incomplete["_invalid_arguments"]["kind"],
            "incomplete_syntax"
        );
        assert_eq!(incomplete["_invalid_arguments"]["execution"], "rejected");
        assert!(incomplete["_recovery"].as_str().is_some());

        let scalar = parse_native_tool_arguments("[]");
        assert_eq!(scalar["_invalid_arguments"]["kind"], "invalid_shape");
    }

    #[test]
    fn parse_speculative_text_tool_call_extracts_in_flight_fences() {
        let in_flight = "Let me search the codebase:\n```tool\n{\"name\": \"grep\", \"arguments\": {\"pattern\": \"AppConfig\"";
        let (name, args) =
            parse_speculative_text_tool_call(in_flight).expect("should extract in-flight call");
        assert_eq!(name, "grep");
        assert_eq!(args["pattern"], "AppConfig");

        let completed =
            "Done:\n```tool\n{\"name\": \"grep\", \"arguments\": {}}\n```\nHere are the results:";
        assert!(
            parse_speculative_text_tool_call(completed).is_none(),
            "completed fence is not in-flight"
        );
    }

    #[test]
    fn parse_speculative_native_tool_call_tracks_incremental_name_and_arguments() {
        assert!(parse_speculative_native_tool_call("[TOOL_CALLS]").is_none());

        let name_only = parse_speculative_native_tool_call("[TOOL_CALLS]grep")
            .expect("native tool name should be visible before arguments");
        assert_eq!(name_only.0, "grep");
        assert_eq!(name_only.1, serde_json::json!({}));

        let partial =
            parse_speculative_native_tool_call("[TOOL_CALLS]grep[ARGS]{\"pattern\": \"AppConfig\"")
                .expect("partial native arguments should be repaired");
        assert_eq!(partial.0, "grep");
        assert_eq!(partial.1["pattern"], "AppConfig");
    }

    #[test]
    fn responses_input_preserves_messages_and_tool_transactions() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "You are RustCode."}),
            serde_json::json!({"role": "user", "content": "Inspect the project."}),
            serde_json::json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {
                        "name": "glob",
                        "arguments": "{\"pattern\":\"src/**\"}"
                    }
                }]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-1",
                "content": "src/main.rs"
            }),
        ];

        let input = responses_input_from_messages(&messages);

        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["content"][0]["type"], "input_text");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call-1");
        assert_eq!(input[2]["arguments"], "{\"pattern\":\"src/**\"}");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call-1");
    }

    #[test]
    fn responses_input_keeps_assistant_prose_before_call_outputs() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": "Continue from the real results.",
                "tool_calls": [
                    {
                        "id": "call-a",
                        "type": "function",
                        "function": {"name": "view_file", "arguments": "{}"}
                    },
                    {
                        "id": "call-b",
                        "type": "function",
                        "function": {"name": "list_directory", "arguments": "{}"}
                    }
                ]
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-a",
                "content": "file result"
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-b",
                "content": "directory result"
            }),
        ];

        let input = responses_input_from_messages(&messages);

        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call-a");
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call-b");
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call-a");
        assert_eq!(input[4]["type"], "function_call_output");
        assert_eq!(input[4]["call_id"], "call-b");
    }

    #[test]
    fn responses_input_holds_runtime_notices_outside_call_output_runs() {
        let messages = vec![
            serde_json::json!({
                "role": "assistant",
                "content": "reading",
                "tool_calls": [
                    {
                        "id": "call-a",
                        "type": "function",
                        "function": {"name": "view_file", "arguments": "{}"}
                    },
                    {
                        "id": "call-b",
                        "type": "function",
                        "function": {"name": "list_directory", "arguments": "{}"}
                    }
                ]
            }),
            serde_json::json!({
                "role": "user",
                "content": "<rustcode_runtime_notice>loop warning</rustcode_runtime_notice>",
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-a",
                "content": "file result"
            }),
            serde_json::json!({
                "role": "tool",
                "tool_call_id": "call-b",
                "content": "directory result"
            }),
        ];

        let input = responses_input_from_messages(&messages);

        let first_call = input
            .iter()
            .position(|item| item["type"] == "function_call")
            .expect("call run");
        let last_output = input
            .iter()
            .rposition(|item| item["type"] == "function_call_output")
            .expect("output run");
        assert!(first_call < last_output);
        for item in &input[first_call..=last_output] {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            assert!(
                item_type == "function_call" || item_type == "function_call_output",
                "transaction split by {item:?}"
            );
        }
        assert!(
            input[last_output + 1..]
                .iter()
                .any(|item| item["content"].to_string().contains("loop warning")),
            "buffered notice must survive after the run: {input:?}"
        );
    }

    #[test]
    fn responses_tools_flatten_chat_function_schemas() {
        let schemas = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "view_file",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {}},
                "strict": true
            }
        })];

        let tools = responses_tool_schemas(&schemas);

        assert_eq!(
            tools,
            vec![serde_json::json!({
                "type": "function",
                "name": "view_file",
                "description": "Read a file",
                "parameters": {"type": "object", "properties": {}},
                "strict": true
            })]
        );
    }

    #[test]
    fn responses_events_normalize_text_tools_usage_and_errors() {
        let mut call_ids = HashMap::new();
        let mut argument_deltas = HashSet::new();
        let text = normalize_responses_event(
            &serde_json::json!({
                "type": "response.output_text.delta",
                "delta": "hello"
            }),
            &mut call_ids,
            &mut argument_deltas,
        )
        .unwrap();
        assert_eq!(text["choices"][0]["delta"]["content"], "hello");

        let added = normalize_responses_event(
            &serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "type": "function_call",
                    "id": "fc-item-1",
                    "call_id": "call-1",
                    "name": "glob"
                }
            }),
            &mut call_ids,
            &mut argument_deltas,
        )
        .unwrap();
        assert_eq!(
            added["choices"][0]["delta"]["tool_calls"][0]["id"],
            "call-1"
        );

        let arguments = normalize_responses_event(
            &serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 0,
                "item_id": "fc-item-1",
                "delta": "{}"
            }),
            &mut call_ids,
            &mut argument_deltas,
        )
        .unwrap();
        assert_eq!(
            arguments["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
        assert!(
            normalize_responses_event(
                &serde_json::json!({
                    "type": "response.function_call_arguments.done",
                    "output_index": 0,
                    "item_id": "fc-item-1",
                    "arguments": "{}"
                }),
                &mut call_ids,
                &mut argument_deltas,
            )
            .is_none()
        );

        let completed = normalize_responses_event(
            &serde_json::json!({
                "type": "response.completed",
                "response": {"usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "total_tokens": 14
                }}
            }),
            &mut call_ids,
            &mut argument_deltas,
        )
        .unwrap();
        assert_eq!(completed["choices"][0]["finish_reason"], "stop");
        assert_eq!(completed["usage"]["prompt_tokens"], 10);

        let error = normalize_responses_event(
            &serde_json::json!({"type": "error", "message": "bad request"}),
            &mut call_ids,
            &mut argument_deltas,
        )
        .unwrap();
        assert_eq!(error["error"]["message"], "bad request");
    }

    #[test]
    fn profile_generation_options_include_hard_thinking_budget() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("low".to_string()),
            thinking_budget: Some(4096),
            supports_reasoning_effort: Some(true),
            supports_thinking_budget: Some(true),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

        assert_eq!(payload["enable_thinking"], true);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(payload["reasoning_effort"], "low");
        assert_eq!(payload["thinking_budget"], 4096);
    }

    #[test]
    fn absent_profile_generation_options_do_not_override_server_defaults() {
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(
            &mut payload,
            Some(&crate::config::ModelProfile::default()),
            ThinkingMode::Normal,
        );

        assert!(payload.get("enable_thinking").is_none());
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn provider_without_thinking_controls_omits_disabled_thinking_fields() {
        let profile = crate::config::ModelProfile {
            url: "https://api.groq.com/openai/v1/chat/completions".to_string(),
            model: "groq/compound".to_string(),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Disabled);

        assert!(payload.get("enable_thinking").is_none());
        assert!(payload.get("chat_template_kwargs").is_none());
    }

    #[test]
    fn qwen_omlx_does_not_send_unverified_reasoning_extensions() {
        let profile = crate::config::ModelProfile {
            engine: Some("omlx".to_string()),
            enable_thinking: Some(true),
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            supports_reasoning_effort: Some(false),
            supports_thinking_budget: Some(false),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

        assert_eq!(payload["enable_thinking"], true);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], true);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn legacy_reasoning_profile_still_sends_existing_wire_fields() {
        let profile = crate::config::ModelProfile {
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

        assert_eq!(payload["reasoning_effort"], "medium");
        assert_eq!(payload["thinking_budget"], 4096);
    }

    #[test]
    fn native_tool_argument_accumulation_has_a_hard_byte_limit() {
        let mut arguments = String::new();
        assert!(!append_bounded_native_arguments(
            &mut arguments,
            "{\"path\":\""
        ));
        assert!(append_bounded_native_arguments(
            &mut arguments,
            &"repeat/".repeat(MAX_NATIVE_TOOL_ARGUMENT_BYTES)
        ));
        assert_eq!(arguments.len(), MAX_NATIVE_TOOL_ARGUMENT_BYTES);
    }

    #[test]
    fn native_tool_calls_without_indices_use_distinct_ids() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "id": "call-write",
            "function": {"name": "write_to_file", "arguments": "{\"path\":\"a\"}"}
        }));
        calls.add_delta(&serde_json::json!({
            "id": "call-run",
            "function": {"name": "run_command", "arguments": "{\"command\":\"pwd\"}"}
        }));

        assert_eq!(calls.calls.len(), 2);
        assert_eq!(calls.calls[0].id, "call-write");
        assert_eq!(calls.calls[0].name, "write_to_file");
        assert_eq!(calls.calls[1].id, "call-run");
        assert_eq!(calls.calls[1].name, "run_command");
        assert_eq!(calls.calls[0].arguments, r#"{"path":"a"}"#);
        assert_eq!(calls.calls[1].arguments, r#"{"command":"pwd"}"#);
    }

    #[test]
    fn native_tool_call_fragments_by_id_append_to_one_call() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "id": "call-1",
            "function": {"name": "run_command", "arguments": "{\"command\":"}
        }));
        calls.add_delta(&serde_json::json!({
            "id": "call-1",
            "function": {"arguments": "\"pwd\"}"}
        }));

        assert_eq!(calls.calls.len(), 1);
        assert_eq!(calls.calls[0].id, "call-1");
        assert_eq!(calls.calls[0].name, "run_command");
        assert_eq!(calls.calls[0].arguments, r#"{"command":"pwd"}"#);
    }

    #[test]
    fn partial_native_call_checkpoint_preserves_identity_without_arguments() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "index": 2,
            "id": "call-write",
            "function": {
                "name": "write_to_file",
                "arguments": "{\"path\":\"src/main.rs\",\"content\":\"secret"
            }
        }));

        let checkpoint = calls.checkpoints();
        assert_eq!(checkpoint.len(), 1);
        assert_eq!(checkpoint[0].index, Some(2));
        assert_eq!(checkpoint[0].call_id.as_deref(), Some("call-write"));
        assert_eq!(checkpoint[0].tool_name, "write_to_file");
        assert!(!checkpoint[0].arguments_complete);
        assert_eq!(checkpoint[0].argument_bytes, 39);
        assert!(!checkpoint[0].argument_fingerprint.is_empty());
        assert!(!checkpoint[0].diagnostic.is_empty());
        assert_ne!(checkpoint[0].diagnostic, "secret");
    }

    #[test]
    fn native_checkpoint_fields_are_bounded() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "id": "i".repeat(MAX_NATIVE_CHECKPOINT_TEXT_BYTES + 1),
            "function": {
                "name": "n".repeat(MAX_NATIVE_CHECKPOINT_TEXT_BYTES + 1),
                "arguments": "{"
            }
        }));

        let checkpoint = calls.checkpoints();
        assert_eq!(checkpoint.len(), 1);
        assert!(
            checkpoint[0]
                .call_id
                .as_ref()
                .is_some_and(|id| id.len() <= MAX_NATIVE_CHECKPOINT_TEXT_BYTES + "…".len())
        );
        assert!(checkpoint[0].tool_name.len() <= MAX_NATIVE_CHECKPOINT_TEXT_BYTES + "…".len());
    }

    #[test]
    fn native_tool_call_late_index_keeps_id_accumulator() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "id": "call-1",
            "function": {"name": "grep", "arguments": "{\"pattern\":"}
        }));
        calls.add_delta(&serde_json::json!({
            "index": 0,
            "id": "call-1",
            "function": {"arguments": "\"needle\"}"}
        }));

        assert_eq!(calls.calls.len(), 1);
        assert_eq!(calls.calls[0].arguments, r#"{"pattern":"needle"}"#);
        assert_eq!(calls.by_index.get(&0), Some(&0));
    }

    #[test]
    fn native_tool_calls_keep_index_order_even_if_first_delta_is_late() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "index": 1,
            "id": "call-1",
            "function": {"name": "second", "arguments": "{}"}
        }));
        calls.add_delta(&serde_json::json!({
            "index": 0,
            "id": "call-0",
            "function": {"name": "first", "arguments": "{}"}
        }));

        let mut order: Vec<_> = (0..calls.calls.len()).collect();
        order.sort_by_key(|&position| {
            let call = &calls.calls[position];
            (
                call.index.is_none(),
                call.index.unwrap_or(usize::MAX),
                position,
            )
        });
        assert_eq!(order, vec![1, 0]);
        assert_eq!(calls.calls[order[0]].name, "first");
        assert_eq!(calls.calls[order[1]].name, "second");
    }

    #[test]
    fn native_tool_call_without_identity_fails_closed_per_delta() {
        let mut calls = ToolAccumulatorSet::default();
        calls.add_delta(&serde_json::json!({
            "function": {"name": "first", "arguments": "{}"}
        }));
        calls.add_delta(&serde_json::json!({
            "function": {"name": "second", "arguments": "{}"}
        }));

        assert_eq!(calls.calls.len(), 2);
        assert_eq!(calls.calls[0].name, "first");
        assert_eq!(calls.calls[1].name, "second");
    }

    #[test]
    fn provider_trace_redacts_content_arguments_and_secrets() {
        let value = serde_json::json!({
            "authorization": "Bearer super-secret-token",
            "choices": [{
                "finish_reason": "tool_calls",
                "delta": {
                    "content": "private prompt text",
                    "tool_calls": [{
                        "index": 3,
                        "id": "call-3",
                        "function": {
                            "name": "run_command",
                            "arguments": "{\"command\":\"cat super-secret-file\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
        });
        let mut trace = ProviderStreamTrace::new(true, "session-1", "model", 2);
        trace.record(321, &value);
        let summary = trace.summary().to_string();

        assert!(!summary.contains("super-secret-token"));
        assert!(!summary.contains("private prompt text"));
        assert!(!summary.contains("super-secret-file"));
        assert!(summary.contains("arguments_bytes"));
        assert!(summary.contains("tool_calls"));
        assert!(summary.contains("\"index\":3"));
        assert!(summary.contains("\"present\":true"));
    }

    #[test]
    fn provider_trace_is_bounded_by_event_count_and_bytes() {
        let value = serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{"index": 0, "id": "x", "function": {"name": "grep", "arguments": "{}"}}]}}]
        });
        let mut trace = ProviderStreamTrace::new(true, "session", "model", 1);
        for _ in 0..(MAX_PROVIDER_TRACE_EVENTS + 100) {
            trace.record(100, &value);
        }
        let summary = trace.summary();
        assert!(summary["event_count"].as_u64().unwrap() <= MAX_PROVIDER_TRACE_EVENTS as u64);
        assert!(summary["trace_bytes"].as_u64().unwrap() <= MAX_PROVIDER_TRACE_BYTES as u64);
        assert!(summary["dropped_events"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn native_schema_tokens_are_in_prompt_and_total_usage_estimates() {
        let messages = vec![serde_json::json!({
            "role": "system",
            "content": "You are RustCode."
        })];
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "inspect_workspace",
                "description": "Inspect files in the workspace",
                "parameters": {"type": "object"}
            }
        })];

        let without_schema = estimate_token_usage(&messages, "reply").await.unwrap();
        let with_schema = estimate_token_usage_with_tool_schemas(&messages, "reply", &schema)
            .await
            .unwrap();
        let schema_tokens = crate::network::compaction::estimate_tool_schema_tokens(&schema) as u32;

        assert!(schema_tokens > 0);
        assert_eq!(
            with_schema.prompt_tokens,
            without_schema.prompt_tokens + schema_tokens
        );
        assert_eq!(
            with_schema.total_tokens,
            without_schema.total_tokens + schema_tokens
        );
        assert_eq!(
            with_schema.completion_tokens,
            without_schema.completion_tokens
        );
    }

    #[test]
    fn prompt_estimation_delta_percent_makes_provider_drift_actionable() {
        assert_eq!(prompt_estimation_delta_percent(10_000, 11_500), Some(15));
        assert_eq!(prompt_estimation_delta_percent(10_000, 9_000), Some(-10));
        assert_eq!(prompt_estimation_delta_percent(0, 100), None);
    }

    #[test]
    fn disabled_thinking_omits_reasoning_controls() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(false),
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Normal);

        assert_eq!(payload["enable_thinking"], false);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn request_override_disables_thinking_and_omits_reasoning_controls() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("high".to_string()),
            thinking_budget: Some(8192),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(&mut payload, Some(&profile), ThinkingMode::Disabled);

        assert_eq!(payload["enable_thinking"], false);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
    }

    #[test]
    fn recovery_disables_thinking_for_a_bounded_action_request() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            supports_reasoning_effort: Some(true),
            supports_thinking_budget: Some(true),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(
            &mut payload,
            Some(&profile),
            ThinkingMode::BoundedRecovery,
        );

        assert_eq!(payload["enable_thinking"], false);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(payload.get("thinking_budget").is_none());
        assert_eq!(
            clamp_request_max_tokens(16_000, ThinkingMode::BoundedRecovery),
            RECOVERY_MAX_TOKENS
        );
        assert_eq!(
            clamp_request_max_tokens(16_000, ThinkingMode::Normal),
            16_000
        );
    }

    #[test]
    fn output_limit_policy_omits_unconfigured_normal_cap() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.output_token_limit(false, false),
        );
        assert!(payload.get("max_tokens").is_none());
        assert!(payload.get("max_completion_tokens").is_none());
        assert!(payload.get("max_output_tokens").is_none());
    }

    #[test]
    fn output_limit_policy_selects_explicit_openai_compatible_field() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            max_tokens: Some(16_000),
            output_token_field: Some(crate::config::OutputTokenField::MaxCompletionTokens),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.output_token_limit(false, false),
        );
        assert_eq!(payload["max_completion_tokens"], 16_000);
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn tool_output_limit_is_present_when_profile_uses_provider_default() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            output_token_field: Some(crate::config::OutputTokenField::MaxOutputTokens),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.output_token_limit(true, false),
        );
        assert_eq!(payload["max_output_tokens"], 8192);
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn verified_profile_tool_output_starts_at_safe_cap() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            max_tokens: Some(16_000),
            tool_max_tokens: Some(16_000),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.output_token_limit(true, false),
        );
        assert_eq!(payload["max_tokens"], 8_192);
    }

    #[test]
    fn verified_kat_adaptive_tool_limit_is_written_to_the_wire_field() {
        let profile = crate::config::ModelProfile {
            name: "kat-coder".to_string(),
            url: "https://tokmax.paral.no/v1/chat/completions".to_string(),
            model: "KAT-Coder-V2.5-Dev-OptiQ-4bit".to_string(),
            context_window: Some(262_144),
            max_tokens: Some(16_000),
            output_token_field: Some(crate::config::OutputTokenField::MaxOutputTokens),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.verified_tool_output_ceiling(),
        );
        assert_eq!(payload["max_output_tokens"], 16_000);
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn native_google_payload_uses_max_output_tokens_capability_field() {
        let profile = crate::config::ModelProfile {
            url: "https://generativelanguage.googleapis.com/v1beta/models/gemini-3:generateContent"
                .to_string(),
            context_window: Some(128_000),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});
        apply_output_token_limit(
            &mut payload,
            profile.resolved_output_token_field(),
            profile.output_token_limit(true, false),
        );
        assert_eq!(payload["maxOutputTokens"], 8192);
        assert!(payload.get("max_tokens").is_none());
        assert!(payload.get("max_output_tokens").is_none());
    }

    #[test]
    fn bounded_recovery_keeps_output_cap_separate_and_small() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            max_tokens: Some(16_000),
            tool_max_tokens: Some(16_000),
            ..crate::config::ModelProfile::default()
        };
        let limit = profile
            .output_token_limit(false, true)
            .map(|value| clamp_request_max_tokens(value, ThinkingMode::BoundedRecovery));
        assert_eq!(limit, Some(RECOVERY_MAX_TOKENS));
    }

    #[test]
    fn profile_sampling_options_are_sent_without_changing_unspecified_defaults() {
        let profile = crate::config::ModelProfile {
            temperature: Some(1.0),
            top_p: Some(0.95),
            top_k: Some(20),
            presence_penalty: Some(1.5),
            frequency_penalty: Some(0.0),
            force_sampling: Some(true),
            preserve_thinking: Some(true),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_sampling_options(&mut payload, Some(&profile));

        assert_eq!(payload["temperature"], 1.0);
        assert_eq!(payload["top_p"], 0.95);
        assert_eq!(payload["top_k"], 20);
        assert_eq!(payload["presence_penalty"], 1.5);
        assert_eq!(payload["force_sampling"], true);
        assert_eq!(payload["chat_template_kwargs"]["preserve_thinking"], true);
        assert!(payload.get("frequency_penalty").is_none());
    }

    #[test]
    fn disabled_tools_omit_schema_and_tool_choice() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, false);

        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
    }

    #[test]
    fn enabled_tools_include_schema_and_auto_choice() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, true);

        assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(payload["tool_choice"], "auto");
    }

    #[test]
    fn default_chat_completions_tools_allow_provider_parallel_calls() {
        let mut payload = serde_json::json!({
            "tools": [{"type": "function", "function": {"name": "view_file"}}]
        });

        apply_provider_parallel_tool_call_policy(
            &mut payload,
            crate::config::ApiProtocol::ChatCompletions,
            true,
        );

        assert_eq!(payload["parallel_tool_calls"], true);
    }

    #[test]
    fn default_responses_tools_allow_provider_parallel_calls() {
        let mut payload = serde_json::json!({
            "tools": [{"type": "function", "name": "view_file"}]
        });

        apply_provider_parallel_tool_call_policy(
            &mut payload,
            crate::config::ApiProtocol::Responses,
            true,
        );

        assert_eq!(payload["parallel_tool_calls"], true);
    }

    #[test]
    fn trusted_batching_profile_enables_provider_parallel_calls() {
        let mut payload = serde_json::json!({
            "tools": [{"type": "function", "function": {"name": "view_file"}}]
        });

        apply_provider_parallel_tool_call_policy(
            &mut payload,
            crate::config::ApiProtocol::ChatCompletions,
            true,
        );

        assert_eq!(payload["parallel_tool_calls"], true);
    }

    #[test]
    fn parallel_call_hint_rejection_is_narrowly_classified() {
        assert!(is_parallel_tool_calls_rejection(
            400,
            "Unknown parameter: parallel_tool_calls"
        ));
        assert!(is_parallel_tool_calls_rejection(
            422,
            "parallel tool calls are not supported"
        ));
        assert!(!is_parallel_tool_calls_rejection(
            400,
            "Invalid tool_choice"
        ));
        assert!(!is_parallel_tool_calls_rejection(
            500,
            "Unknown parameter: parallel_tool_calls"
        ));
    }

    #[test]
    fn openrouter_requests_carry_a_bounded_session_id() {
        let mut payload = serde_json::json!({});
        apply_openrouter_session_affinity(
            &mut payload,
            "https://openrouter.ai/api/v1/chat/completions",
            Some("  session-123  "),
        );
        assert_eq!(payload["session_id"], "session-123");

        let mut payload = serde_json::json!({});
        apply_openrouter_session_affinity(
            &mut payload,
            "https://provider.example/v1/chat/completions",
            Some("session-123"),
        );
        assert!(payload.get("session_id").is_none());

        let mut payload = serde_json::json!({});
        apply_openrouter_session_affinity(
            &mut payload,
            "https://openrouter.ai/api/v1/chat/completions",
            Some(&"x".repeat(300)),
        );
        assert_eq!(payload["session_id"].as_str().map(str::len), Some(256));
    }

    #[test]
    fn provider_usage_preserves_cache_write_and_discount_metrics() {
        let value = serde_json::json!({
            "response": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "input_tokens_details": {
                        "cached_tokens": 80,
                        "cache_write_tokens": 20
                    },
                    "cache_discount": 0.25
                }
            }
        });

        let usage = responses_usage(&value).expect("usage should normalize");
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 80);
        assert_eq!(usage["prompt_tokens_details"]["cache_write_tokens"], 20);
        assert_eq!(usage["cache_discount"], 0.25);
    }

    #[test]
    fn provider_cache_observation_distinguishes_affinity_and_reported_state() {
        assert_eq!(
            provider_cache_observation(true, Some(80), Some(20)),
            ("hit", "provider_reported_cached_tokens")
        );
        assert_eq!(
            provider_cache_observation(true, Some(0), Some(0)),
            ("miss", "provider_reported_zero_cache_tokens")
        );
        assert_eq!(
            provider_cache_observation(true, None, None),
            ("unknown", "provider_did_not_report_cache_tokens")
        );
        assert_eq!(
            provider_cache_observation(false, None, None),
            ("not_requested", "openrouter_session_affinity_unavailable")
        );
    }

    #[test]
    fn alternate_provider_cache_usage_names_are_normalized() {
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 10,
            "total_tokens": 110,
            "prompt_tokens_details": {
                "cache_read_input_tokens": 75,
                "cache_creation_input_tokens": 25
            }
        });
        assert_eq!(cache_usage_metrics(&usage).0, Some(75));
        assert_eq!(cache_usage_metrics(&usage).1, Some(25));
    }

    #[test]
    fn recovery_tool_requests_allow_a_final_answer() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, true);

        assert_eq!(payload["tool_choice"], "auto");
    }

    #[test]
    fn reasoning_chunks_stop_at_the_configured_client_budget() {
        let first = bound_reasoning_chunk("a".repeat(12).as_str(), 0, Some(4));
        assert_eq!(first.estimated_tokens, 3);
        assert!(!first.budget_exhausted);

        let final_chunk = bound_reasoning_chunk("b".repeat(12).as_str(), 3, Some(4));
        assert_eq!(final_chunk.estimated_tokens, 1);
        assert_eq!(final_chunk.text.len(), 4);
        assert!(final_chunk.budget_exhausted);
    }

    #[test]
    fn absent_reasoning_budget_preserves_the_whole_chunk() {
        let chunk = bound_reasoning_chunk("reasoning", 0, None);
        assert_eq!(chunk.text, "reasoning");
        assert!(!chunk.budget_exhausted);
    }

    #[test]
    fn thinking_estimate_accumulates_chars_not_per_chunk_ceils() {
        // Token-piece streams deliver thousands of few-char deltas. Summing
        // per-chunk ceils would charge 2000 * ceil(9 * 0.25) = 6000 tokens
        // here; deriving once from kept chars charges ceil(18000 * 0.25).
        let mut buffer = StreamBuffer::new();
        let budget = Some(8192);
        for _ in 0..2000 {
            let used = buffer.thought_tokens_estimate();
            let bounded = bound_reasoning_chunk("123456789", used, budget);
            assert!(!bounded.budget_exhausted);
            buffer.thought_chars += bounded.text.len();
            buffer.thought_tokens = buffer.thought_tokens_estimate();
        }
        assert_eq!(buffer.thought_chars, 18_000);
        assert_eq!(buffer.thought_tokens_estimate(), 4500);
        assert_eq!(buffer.thought_tokens, 4500);
    }
}

#[derive(Debug)]
enum SseReadError {
    Timeout {
        kind: StreamFailureKind,
        partial_event_bytes: usize,
    },
    Io(String),
}

impl SseReadError {
    fn kind(&self) -> StreamFailureKind {
        match self {
            Self::Timeout { kind, .. } => *kind,
            Self::Io(error)
                if error.to_ascii_lowercase().contains("timed out")
                    || error.to_ascii_lowercase().contains("timeout") =>
            {
                // reqwest's transport-level read timeout can surface through
                // StreamReader as an I/O error instead of reaching the
                // application-level timeout wrapper. Keep it recoverable and
                // visible as the same idle-stream failure.
                StreamFailureKind::StreamIdleTimeout
            }
            Self::Io(error) if error.contains("invalid UTF-8") => StreamFailureKind::MalformedSse,
            Self::Io(error) if error.contains(RESPONSE_BODY_DECODE_ERROR) => {
                StreamFailureKind::ResponseBodyDecode
            }
            Self::Io(_) => StreamFailureKind::ProviderError,
        }
    }

    fn partial_event_bytes(&self) -> usize {
        match self {
            Self::Timeout {
                partial_event_bytes,
                ..
            } => *partial_event_bytes,
            Self::Io(_) => 0,
        }
    }
}

impl std::fmt::Display for SseReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout {
                kind,
                partial_event_bytes,
            } => write!(
                f,
                "SSE stream {kind} after {partial_event_bytes} partial event bytes"
            ),
            Self::Io(error) => write!(f, "SSE stream read failed: {error}"),
        }
    }
}

#[cfg(test)]
mod sse_read_error_tests {
    use super::{RESPONSE_BODY_DECODE_ERROR, SseReadError};
    use crate::network::lifecycle::StreamFailureKind;

    #[test]
    fn classifies_response_body_decode_errors_without_misclassifying_utf8() {
        assert_eq!(
            SseReadError::Io(RESPONSE_BODY_DECODE_ERROR.to_owned()).kind(),
            StreamFailureKind::ResponseBodyDecode
        );
        assert_eq!(
            SseReadError::Io("SSE stream contained invalid UTF-8".to_owned()).kind(),
            StreamFailureKind::MalformedSse
        );
        assert_eq!(
            SseReadError::Io(
                "SSE stream contained invalid UTF-8: error decoding response body".to_owned()
            )
            .kind(),
            StreamFailureKind::MalformedSse
        );
        assert_eq!(
            SseReadError::Io("error decoding response body: connection reset".to_owned()).kind(),
            StreamFailureKind::ResponseBodyDecode
        );
        assert_eq!(
            SseReadError::Io("request or response body error: operation timed out".to_owned())
                .kind(),
            StreamFailureKind::StreamIdleTimeout
        );
        assert_eq!(
            SseReadError::Io("connection reset by peer".to_owned()).kind(),
            StreamFailureKind::ProviderError
        );
    }
}

/// Backwards-compatible helper used by focused parser tests.  The streaming
/// request uses [`read_sse_line_with_state`] so the first meaningful event has
/// its own deadline.
#[cfg(test)]
async fn read_sse_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut String,
) -> Result<usize, SseReadError> {
    read_sse_line_with_state(reader, line_buf, false).await
}

async fn read_sse_line_with_state<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut String,
    waiting_for_first_event: bool,
) -> Result<usize, SseReadError> {
    let mut bytes = Vec::new();
    let timeout = if waiting_for_first_event {
        retry::FIRST_EVENT_TIMEOUT
    } else {
        retry::STREAM_IDLE_TIMEOUT
    };

    loop {
        let (chunk_len, line_complete) = {
            let chunk = match tokio::time::timeout(timeout, reader.fill_buf()).await {
                Ok(Ok(chunk)) => chunk,
                Ok(Err(error)) => return Err(SseReadError::Io(error.to_string())),
                Err(_) => {
                    return Err(SseReadError::Timeout {
                        kind: if waiting_for_first_event {
                            StreamFailureKind::FirstEventTimeout
                        } else {
                            StreamFailureKind::StreamIdleTimeout
                        },
                        partial_event_bytes: bytes.len(),
                    });
                }
            };

            if chunk.is_empty() {
                if bytes.is_empty() {
                    return Ok(0);
                }
                break;
            }

            let chunk_len = chunk
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(chunk.len(), |newline| newline + 1);
            bytes.extend_from_slice(&chunk[..chunk_len]);
            (chunk_len, chunk[chunk_len - 1] == b'\n')
        };
        reader.consume(chunk_len);

        if line_complete {
            break;
        }
    }

    let line = std::str::from_utf8(&bytes).map_err(|error| {
        SseReadError::Io(format!("SSE stream contained invalid UTF-8: {error}"))
    })?;
    line_buf.push_str(line);
    Ok(bytes.len())
}

/// Absolute read deadline for the SSE stream loop.
///
/// While no meaningful event arrived, the first-event budget
/// (`FIRST_EVENT_TIMEOUT` since headers) applies; afterwards the idle budget
/// (`STREAM_IDLE_TIMEOUT` since the last meaningful event) applies. Only
/// meaningful events move the progress markers — keep-alive blank/comment
/// lines and partial line bytes must not extend these budgets, otherwise a
/// provider or proxy emitting periodic blank lines could stall the stream
/// forever without any timeout firing.
fn sse_progress_deadline(
    first_event_deadline: tokio::time::Instant,
    last_progress: tokio::time::Instant,
    events_received: usize,
) -> tokio::time::Instant {
    if events_received == 0 {
        first_event_deadline.min(last_progress + retry::STREAM_IDLE_TIMEOUT)
    } else {
        last_progress + retry::STREAM_IDLE_TIMEOUT
    }
}

/// Metadata-only summary of an outbound chat-completion request: round shape
/// and size, not content. This is what gets written to debug.log by default
/// in place of the full serialized payload (see `request_debug_log_line`).
#[cfg(test)]
pub(crate) fn request_log_summary(
    model: &str,
    message_count: usize,
    tool_count: usize,
    payload_bytes: usize,
) -> String {
    request_log_summary_with_protocol(
        model,
        message_count,
        tool_count,
        payload_bytes,
        "unspecified",
        "unspecified",
        tool_count,
        0,
        0,
    )
}

pub(crate) fn request_log_summary_with_protocol(
    model: &str,
    message_count: usize,
    tool_count: usize,
    payload_bytes: usize,
    tool_mode: &str,
    tool_protocol: &str,
    available_builtin_tools: usize,
    tool_schema_tokens: usize,
    textual_contract_tokens: usize,
) -> String {
    format!(
        "stream_request: sending model={model} tool_mode={tool_mode} tool_protocol={tool_protocol} messages={message_count} tools={tool_count} available_builtin_tools={available_builtin_tools} tool_schema_tokens={tool_schema_tokens} textual_contract_tokens={textual_contract_tokens} payload_bytes={payload_bytes}"
    )
}

fn tool_schema_tokens_for_protocol(
    protocol: crate::config::ToolProtocol,
    schemas: &[serde_json::Value],
    textual_contract_tokens: usize,
) -> usize {
    if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        crate::network::compaction::estimate_tool_schema_tokens(schemas)
    } else {
        textual_contract_tokens
    }
}

/// Choose what to write to the debug log for an outbound request: the cheap
/// structured `summary` by default, or the full serialized `payload`
/// (pretty-printed, exactly as it goes over the wire) only when
/// `verbose` (`config.debug_verbose_network_logging`) is explicitly set.
/// Kept pure/separate from the call site so both paths are unit-testable
/// without an app state, a request, or a file write.
pub(crate) fn request_debug_log_line(
    verbose: bool,
    summary: &str,
    payload: &serde_json::Value,
) -> String {
    if verbose {
        format!(
            "stream_request: Request payload: {}",
            serde_json::to_string_pretty(payload).unwrap_or_default()
        )
    } else {
        summary.to_string()
    }
}

fn invalid_argument_marker(raw: &str, kind: &str, error: impl Into<String>) -> serde_json::Value {
    let preview_end = raw.floor_char_boundary(raw.len().min(MAX_INVALID_ARGUMENT_PREVIEW_BYTES));
    serde_json::json!({
        "_invalid_arguments": {
            "kind": kind,
            "original_bytes": raw.len(),
            "preview": &raw[..preview_end],
            "truncated": raw.len() > preview_end,
            "execution": "rejected",
        },
        "_parse_error": error.into(),
        "_recovery": "No tool was executed. Emit one complete JSON object with the full arguments; do not rely on the preview.",
    })
}

fn append_bounded_native_arguments(target: &mut String, chunk: &str) -> bool {
    let remaining = MAX_NATIVE_TOOL_ARGUMENT_BYTES.saturating_sub(target.len());
    let keep = chunk.floor_char_boundary(chunk.len().min(remaining));
    target.push_str(&chunk[..keep]);
    keep < chunk.len()
}

/// Preserve bounded malformed-argument metadata for validation and model
/// feedback. Raw provider output is never replayed wholesale into history.
pub(crate) fn parse_native_tool_arguments(raw: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            invalid_argument_marker(raw, "invalid_shape", "tool arguments must be a JSON object")
        }
        Err(error) => invalid_argument_marker(
            raw,
            if error.is_eof() {
                "incomplete_syntax"
            } else {
                "invalid_syntax"
            },
            error.to_string(),
        ),
    }
}

/// Speculatively parse partial JSON argument fragments emitted chunk-by-chunk
/// by the model over SSE, allowing the TUI to project tool names and targets in real time.
pub(crate) fn parse_speculative_arguments(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return serde_json::json!({});
    }

    // 1. Exact parse if complete
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if value.is_object() {
            return value;
        }
    }

    // 2. Synthesize closing quotes / brackets / braces for in-flight streams
    let mut repaired = trimmed.to_string();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in repaired.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
        }
    }
    if in_quote {
        repaired.push('"');
    }
    let open_braces = repaired.chars().filter(|&c| c == '{').count();
    let close_braces = repaired.chars().filter(|&c| c == '}').count();
    for _ in close_braces..open_braces {
        repaired.push('}');
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&repaired) {
        if value.is_object() {
            return value;
        }
    }

    // 3. Fallback: Extract key arguments via pattern matching from partial stream
    let mut map = serde_json::Map::new();
    let keys = [
        "TargetFile",
        "AbsolutePath",
        "path",
        "DirectoryPath",
        "SearchPath",
        "CommandLine",
        "command",
        "pattern",
        "Query",
        "query",
        "name",
        "src",
        "dest",
    ];
    for key in keys {
        let pattern = format!(r#""{}"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)"#, key);
        if let Ok(re) = regex::Regex::new(&pattern) {
            if let Some(caps) = re.captures(trimmed) {
                if let Some(m) = caps.get(1) {
                    map.insert(
                        key.to_string(),
                        serde_json::Value::String(m.as_str().replace(r#"\""#, "\"")),
                    );
                }
            }
        }
    }
    serde_json::Value::Object(map)
}

/// Speculatively extract tool name and arguments from an in-flight text-fenced tool call (```tool ...).
pub(crate) fn parse_speculative_text_tool_call(
    content: &str,
) -> Option<(String, serde_json::Value)> {
    let pos = content
        .rfind("```tool\n")
        .or_else(|| content.rfind("```tool\r\n"))?;
    let tail = &content[pos + 7..];
    if tail.contains("\n```") {
        return None;
    }
    let parsed = parse_speculative_arguments(tail);
    let name = parsed.get("name").and_then(|n| n.as_str())?.to_string();
    let args = parsed
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    Some((name, args))
}

/// Speculatively extract a native [TOOL_CALLS]name[ARGS]{...} call. Native
/// tool output is streamed as plain text, so the name must become visible
/// before the argument object is complete.
pub(crate) fn parse_speculative_native_tool_call(
    content: &str,
) -> Option<(String, serde_json::Value)> {
    let marker = "[TOOL_CALLS]";
    let pos = content.rfind(marker)?;
    let tail = content[pos + marker.len()..].trim_start();
    let name_end = tail
        .char_indices()
        .find_map(|(index, character)| {
            (!character.is_ascii_alphanumeric() && character != '_' && character != '-')
                .then_some(index)
        })
        .unwrap_or(tail.len());
    if name_end == 0 {
        return None;
    }

    let name = tail[..name_end].to_owned();
    let arguments = tail[name_end..]
        .find('{')
        .map(|start| parse_speculative_arguments(&tail[name_end + start..]))
        .unwrap_or_else(|| serde_json::json!({}));
    Some((name, arguments))
}

pub(crate) async fn estimate_token_usage(
    messages: &[serde_json::Value],
    reply: &str,
) -> Option<TokenUsage> {
    estimate_token_usage_with_tool_schemas(messages, reply, &[]).await
}

pub(crate) async fn estimate_token_usage_with_tool_schemas(
    messages: &[serde_json::Value],
    reply: &str,
    tool_schemas: &[serde_json::Value],
) -> Option<TokenUsage> {
    let mut prompt_text = String::new();
    for msg in messages {
        if let Some(content) = msg.get("content") {
            if let Some(s) = content.as_str() {
                prompt_text.push_str(s);
                prompt_text.push('\n');
            } else if content.is_array() {
                if let Some(arr) = content.as_array() {
                    for item in arr {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            prompt_text.push_str(text);
                            prompt_text.push('\n');
                        }
                    }
                }
            } else {
                prompt_text.push_str(&content.to_string());
                prompt_text.push('\n');
            }
        }
        if let Some(tool_calls) = msg.get("tool_calls") {
            prompt_text.push_str(&tool_calls.to_string());
            prompt_text.push('\n');
        }
        if let Some(tool_call_id) = msg.get("tool_call_id").and_then(|id| id.as_str()) {
            prompt_text.push_str(tool_call_id);
            prompt_text.push('\n');
        }
    }
    let schema_tokens = crate::network::compaction::estimate_tool_schema_tokens(tool_schemas);
    let prompt = count_tokens(&prompt_text).saturating_add(schema_tokens as u32);
    let full = prompt_text + reply + "\n";
    let total = count_tokens(&full).saturating_add(schema_tokens as u32);
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: total.saturating_sub(prompt),
        total_tokens: total,
        cached_tokens: None,
        ..Default::default()
    })
}

fn prompt_estimation_delta_percent(estimated: u32, observed: u64) -> Option<i64> {
    (estimated > 0).then(|| {
        (((observed as i128 - i128::from(estimated)) * 100) / i128::from(estimated)) as i64
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_request(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    url: &str,
    model: &str,
    messages: Vec<serde_json::Value>,
    buffer: Arc<Mutex<StreamBuffer>>,
    quiet: bool,
    allow_tools: bool,
    thinking_mode: ThinkingMode,
    schema_policy: crate::tools::ToolSchemaPolicy,
    expected_session_id: Option<&str>,
    tool_output_limit_override: Option<u32>,
) -> Result<Option<String>, StreamFailure> {
    let profile = {
        state
            .lock()
            .await
            .config
            .models
            .iter()
            .find(|p| p.matches_request(url, model))
            .cloned()
    };
    let api_protocol = profile
        .as_ref()
        .map(crate::config::ModelProfile::resolved_api_protocol)
        .unwrap_or_else(|| {
            url.trim_end_matches('/')
                .ends_with("/responses")
                .then_some(crate::config::ApiProtocol::Responses)
                .unwrap_or_default()
        });
    let responses_api = matches!(api_protocol, crate::config::ApiProtocol::Responses);
    let aligned_messages = align_alternating_messages(messages);
    let message_count = aligned_messages.len();
    let (tool_protocol, agent_mode, workspace_root) = {
        let s = state.lock().await;
        (
            s.active_tool_protocol(),
            s.agent_mode,
            s.task_working_directory
                .clone()
                .or_else(|| s.workspace_root.clone())
                .or_else(|| std::env::current_dir().ok()),
        )
    };
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative)
        && let Err(diagnostic) =
            crate::network::history::validate_native_tool_messages(&aligned_messages)
    {
        crate::logger::operational_event(
            "turn.native_history_validation_failed",
            serde_json::json!({
                "request_boundary": "stream_request",
                "message_count": message_count,
                "diagnostic": diagnostic,
            }),
        );
        return Err(StreamFailure {
            kind: StreamFailureKind::ProviderError,
            status: None,
            detail: Some(format!(
                "Native tool history is incomplete after provider message alignment: {diagnostic}. The request was stopped locally; review the recent tool activity before retrying."
            )),
            bytes_received: 0,
            events_received: 0,
            partial_event_bytes: 0,
        });
    }
    let mut max_tokens = profile
        .as_ref()
        .map(|p| p.completion_token_limit(allow_tools))
        .unwrap_or_else(|| {
            crate::config::ModelProfile {
                name: model.to_string(),
                url: url.to_string(),
                model: model.to_string(),
                context_window: Some(crate::config::DEFAULT_CONTEXT_WINDOW),
                engine: None,
                api_key: None,
                env_key: None,
                tool_protocol: None,
                enable_thinking: None,
                reasoning_effort: None,
                thinking_budget: None,
                temperature: None,
                top_p: None,
                top_k: None,
                presence_penalty: None,
                frequency_penalty: None,
                force_sampling: None,
                preserve_thinking: None,
                max_tokens: None,
                supports_vision: None,
                ..Default::default()
            }
            .context_budget()
            .completion_reserve
        });
    max_tokens = clamp_request_max_tokens(max_tokens, thinking_mode);
    let output_token_field = profile
        .as_ref()
        .map(crate::config::ModelProfile::resolved_output_token_field)
        .unwrap_or(crate::config::OutputTokenField::MaxTokens);
    let mut output_token_limit = profile
        .as_ref()
        .and_then(|p| {
            p.output_token_limit(allow_tools, thinking_mode == ThinkingMode::BoundedRecovery)
        })
        .map(|limit| clamp_request_max_tokens(limit, thinking_mode))
        .or_else(|| {
            (allow_tools || thinking_mode == ThinkingMode::BoundedRecovery).then_some(max_tokens)
        });
    if allow_tools && thinking_mode == ThinkingMode::Normal {
        if let Some(override_limit) = tool_output_limit_override {
            let verified_ceiling = profile
                .as_ref()
                .map(|p| p.tool_output_ceiling())
                .unwrap_or(crate::config::DEFAULT_TOOL_ROUND_MAX_TOKENS);
            output_token_limit = Some(override_limit.min(verified_ceiling).max(1));
        }
    }
    // Recovery is action-oriented and therefore deliberately has no reasoning
    // budget. Normal configured thinking remains unchanged.
    let thinking_budget = if matches!(
        thinking_mode,
        ThinkingMode::BoundedRecovery | ThinkingMode::Disabled
    ) {
        None
    } else {
        profile
            .as_ref()
            .and_then(|p| p.client_reasoning_budget)
            .or_else(|| profile.as_ref().map(|p| p.context_budget().thinking_budget))
    };

    let text_surface = if !allow_tools {
        crate::tools::ToolSurface::default()
    } else if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        crate::tools::ToolSurface {
            agent: crate::tools::agent_tool_count(schema_policy, agent_mode),
            ..Default::default()
        }
    } else {
        crate::tools::textual_tool_surface(schema_policy, agent_mode)
    };
    let (native_tool_schemas, mcp_selection, tool_surface) =
        if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) && allow_tools {
            let (schemas, selection) = crate::network::prepare_native_tool_schemas(
                &state,
                schema_policy,
                &aligned_messages,
                workspace_root.as_deref(),
            )
            .await;
            let surface = crate::tools::ToolSurface {
                builtin: selection.builtin_available,
                mcp: selection.available,
                agent: text_surface.agent,
            };
            (schemas, selection, surface)
        } else {
            (
                Vec::new(),
                crate::tools::McpSchemaSelectionStats::default(),
                text_surface,
            )
        };

    // Estimate the actual continuation prompt before serializing the payload
    // so an adaptive ceiling cannot ask the provider for output that cannot
    // fit inside the profile's effective context window.
    let estimated_prompt_tokens =
        estimate_token_usage_with_tool_schemas(&aligned_messages, "", &native_tool_schemas)
            .await
            .map(|u| u.prompt_tokens)
            .unwrap_or(0);
    let context_output_capacity = profile.as_ref().map(|p| {
        p.context_budget()
            .hard_effective_limit
            .saturating_sub(estimated_prompt_tokens)
    });
    let provider_overhead_margin = profile
        .as_ref()
        .map(|p| p.context_budget().provider_overhead_margin)
        .unwrap_or_default();
    let accounted_prompt_tokens = estimated_prompt_tokens.saturating_add(provider_overhead_margin);
    if let Some(profile) = profile.as_ref() {
        let budget = profile.context_budget();
        if estimated_prompt_tokens.saturating_add(budget.completion_reserve)
            > budget.hard_effective_limit
        {
            crate::logger::operational_event(
                "context.preflight_rejected",
                serde_json::json!({
                    "model": model,
                    "estimated_prompt_tokens": estimated_prompt_tokens,
                    "completion_reserve": budget.completion_reserve,
                    "hard_effective_limit": budget.hard_effective_limit,
                    "reason": "final_projection_over_budget",
                }),
            );
            return Err(StreamFailure {
                kind: StreamFailureKind::ProviderError,
                status: None,
                detail: Some(format!(
                    "{CONTEXT_PREFLIGHT_STOP_PREFIX}final provider projection exceeds the configured context budget"
                )),
                bytes_received: 0,
                events_received: 0,
                partial_event_bytes: 0,
            });
        }
    }
    if let Some(capacity) = context_output_capacity {
        output_token_limit = output_token_limit.map(|limit| limit.min(capacity.max(1)));
    }
    {
        let mut buffer = buffer.lock().await;
        buffer.output_token_limit = output_token_limit;
    }
    let textual_contract_tokens =
        if allow_tools && !matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
            aligned_messages
                .iter()
                .filter_map(|message| {
                    (message.get("role").and_then(serde_json::Value::as_str) == Some("system"))
                        .then(|| message.get("content").and_then(serde_json::Value::as_str))
                        .flatten()
                })
                .filter_map(|content| content.find("# Tool Format").map(|start| &content[start..]))
                .map(count_tokens)
                .sum::<u32>() as usize
        } else {
            0
        };
    let mut payload = if responses_api {
        let mut payload = serde_json::json!({
            "model": model,
            "stream": true,
            "input": responses_input_from_messages(&aligned_messages),
        });
        if let Some(limit) = output_token_limit {
            payload["max_output_tokens"] = serde_json::json!(limit);
        }
        apply_responses_generation_options(&mut payload, profile.as_ref(), thinking_mode);
        apply_responses_sampling_options(&mut payload, profile.as_ref());
        if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative)
            && allow_tools
            && !native_tool_schemas.is_empty()
        {
            payload["tools"] =
                serde_json::Value::Array(responses_tool_schemas(&native_tool_schemas));
            payload["tool_choice"] = serde_json::json!("auto");
        }
        payload
    } else {
        let mut payload = serde_json::json!({
            "model": model,
            "stream": true,
            "stream_options": {
                "include_usage": true
            },
        });
        payload["messages"] = serde_json::Value::Array(aligned_messages);

        apply_output_token_limit(&mut payload, output_token_field, output_token_limit);

        apply_profile_generation_options(&mut payload, profile.as_ref(), thinking_mode);
        apply_profile_sampling_options(&mut payload, profile.as_ref());

        if !url.contains("generativelanguage.googleapis.com") {
            payload["frequency_penalty"] = serde_json::json!(
                profile
                    .as_ref()
                    .and_then(|p| p.frequency_penalty)
                    .unwrap_or(0.3)
            );
        }
        if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
            // A recovery turn may legitimately be the final answer for a
            // read-only task. Keep tools available, but do not require a tool call
            // after the recovery prompt explicitly asks for prose when no action
            // remains.
            apply_api_native_tools(&mut payload, native_tool_schemas.clone(), allow_tools);
        }
        payload
    };

    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        // A recovery turn may legitimately be the final answer for a
        // read-only task. Keep tools available, but do not require a tool call
        // after the recovery prompt explicitly asks for prose when no action
        // remains.
        if !responses_api {
            apply_api_native_tools(&mut payload, native_tool_schemas.clone(), allow_tools);
        }
        crate::logger::operational_event(
            "mcp.native_schema_selection",
            serde_json::json!({
                "available": mcp_selection.available,
                "selected": mcp_selection.selected,
                "relevant": mcp_selection.relevant,
                "previously_used": mcp_selection.previously_used,
                "fallback": mcp_selection.fallback,
                "omitted": mcp_selection.omitted,
                "selected_names": mcp_selection.selected_names,
                "max": crate::tools::MAX_MCP_NATIVE_SCHEMAS,
                "mcp_schema_bytes": mcp_selection.mcp_schema_bytes,
                "mcp_schema_budget_bytes": mcp_selection.mcp_schema_budget_bytes,
                "schema_budget_exhausted": mcp_selection.schema_budget_exhausted,
                "allow_tools": allow_tools,
                "phase": format!("{:?}", mcp_selection.phase),
                "builtin_available": mcp_selection.builtin_available,
                "builtin_selected": mcp_selection.builtin_selected,
            }),
        );
    }

    apply_provider_parallel_tool_call_policy(&mut payload, api_protocol, allow_tools);
    apply_openrouter_session_affinity(&mut payload, url, expected_session_id);

    let tool_count = if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        payload
            .get("tools")
            .and_then(|tools| tools.as_array())
            .map_or(0, |tools| tools.len())
    } else if allow_tools {
        tool_surface.total()
    } else {
        0
    };
    let tool_mode = if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        "native"
    } else {
        "textual"
    };
    let tool_protocol_name = match tool_protocol {
        crate::config::ToolProtocol::Json => "json",
        crate::config::ToolProtocol::Native => "native",
        crate::config::ToolProtocol::ApiNative => "api_native",
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|error| StreamFailure {
        kind: StreamFailureKind::ProviderError,
        status: None,
        detail: Some(format!("failed to serialize request payload: {error}")),
        bytes_received: 0,
        events_received: 0,
        partial_event_bytes: 0,
    })?;
    let payload_byte_count = payload_bytes.len();
    let verbose_network_logging = { state.lock().await.config.debug_verbose_network_logging };
    dbg_log!(
        "{}",
        request_debug_log_line(
            verbose_network_logging,
            &request_log_summary_with_protocol(
                model,
                message_count,
                tool_count,
                payload_byte_count,
                tool_mode,
                tool_protocol_name,
                tool_surface.builtin,
                tool_schema_tokens_for_protocol(
                    tool_protocol,
                    &native_tool_schemas,
                    textual_contract_tokens,
                ),
                textual_contract_tokens,
            ),
            &payload,
        )
    );

    let parallel_tool_calls_fallback_payload_bytes = payload
        .get("parallel_tool_calls")
        .is_some()
        .then(|| {
            let mut fallback_payload = payload.clone();
            fallback_payload
                .as_object_mut()
                .expect("request payload is an object")
                .remove("parallel_tool_calls");
            serde_json::to_vec(&fallback_payload).ok()
        })
        .flatten();
    let request_start_time = std::time::Instant::now();
    drop(payload);
    let tool_schema_tokens = tool_schema_tokens_for_protocol(
        tool_protocol,
        &native_tool_schemas,
        textual_contract_tokens,
    );

    if let Some((configured, provider)) = profile
        .as_ref()
        .and_then(crate::config::ModelProfile::context_window_mismatch)
    {
        crate::logger::operational_event(
            "context.window_mismatch",
            serde_json::json!({
                "model": model,
                "configured_context_window": configured,
                "provider_context_window": provider,
                "effective_context_window": provider,
                "action": "clamped_to_provider",
            }),
        );
    }

    crate::logger::operational_event(
        "context.request_composition",
        serde_json::json!({
            "model": model,
            "tool_mode": tool_mode,
            "tool_protocol": tool_protocol_name,
            "messages": message_count,
            "tools": tool_count,
            "available_builtin_tools": tool_surface.builtin,
            "available_mcp_tools": tool_surface.mcp,
            "available_agent_tools": tool_surface.agent,
            "builtin_schema_bytes": mcp_selection.builtin_schema_bytes,
            "mcp_schema_bytes": mcp_selection.mcp_schema_bytes,
            "mcp_schema_budget_bytes": mcp_selection.mcp_schema_budget_bytes,
            "schema_budget_exhausted": mcp_selection.schema_budget_exhausted,
            "payload_bytes": payload_byte_count,
            "tool_schema_tokens": tool_schema_tokens,
            "textual_contract_tokens": textual_contract_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "accounted_prompt_tokens": accounted_prompt_tokens,
            "total_estimated_prompt_tokens": accounted_prompt_tokens,
            "provider_overhead_margin": provider_overhead_margin,
            "output_token_field": output_token_limit.map(|_| output_token_field),
            "output_token_limit": output_token_limit,
            "tool_output_limit_override": tool_output_limit_override,
            "context_output_capacity": context_output_capacity,
            "output_token_limit_source":
                if tool_output_limit_override.is_some() {
                    "adaptive_profile_tool"
                } else if allow_tools
                    && profile
                        .as_ref()
                        .is_some_and(|p| p.verified_tool_output_ceiling().is_some())
                {
                    "profile_tool"
                } else if profile.as_ref().is_some_and(|p| {
                    p.max_output_tokens.is_some() || p.max_tokens.is_some()
                }) {
                    "profile"
                } else if output_token_limit.is_some() {
                    "client_safety"
                } else {
                    "provider_default"
                },
            "soft_context_target": profile.as_ref().map(|p| p.context_budget().soft_context_target),
            "hard_effective_limit": profile.as_ref().map(|p| p.context_budget().hard_effective_limit),
            "context_window": profile.as_ref().map(|p| p.context_budget().context_window),
            "configured_context_window": profile.as_ref().and_then(|p| p.context_window),
            "provider_context_window": profile.as_ref().and_then(|p| p.provider_context_window),
            "context_window_mismatch": profile.as_ref().and_then(|p| p.context_window_mismatch()).map(|(configured, provider)| serde_json::json!({
                "configured": configured,
                "provider": provider,
                "action": "clamped_to_provider"
            })),
            "thinking_budget": thinking_budget,
            "thinking_budget_wire_supported": profile.as_ref().map(|p| p.supports_thinking_budget_wire()),
            "minimum_answer_tokens": profile.as_ref().map(|p| p.context_budget().minimum_answer_tokens),
            "openrouter_session_affinity": is_openrouter_endpoint(url)
                && bounded_openrouter_session_id(expected_session_id).is_some(),
        }),
    );

    crate::logger::operational_event(
        "provider.request_start",
        serde_json::json!({
            "model": model,
            "tool_mode": tool_mode,
            "tool_protocol": tool_protocol_name,
            "messages": message_count,
            "tools": tool_count,
            "payload_bytes": payload_byte_count,
            "tool_schema_tokens": tool_schema_tokens,
            "textual_contract_tokens": textual_contract_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "accounted_prompt_tokens": accounted_prompt_tokens,
            "total_estimated_prompt_tokens": accounted_prompt_tokens,
            "provider_overhead_margin": provider_overhead_margin,
            "tool_schema_phase": format!("{:?}", mcp_selection.phase),
            "builtin_tools": if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
                mcp_selection.builtin_selected
            } else {
                tool_surface.builtin
            },
            "available_builtin_tools": tool_surface.builtin,
            "available_mcp_tools": tool_surface.mcp,
            "available_agent_tools": tool_surface.agent,
            "builtin_schema_bytes": mcp_selection.builtin_schema_bytes,
            "mcp_schema_bytes": mcp_selection.mcp_schema_bytes,
            "mcp_schema_budget_bytes": mcp_selection.mcp_schema_budget_bytes,
            "schema_budget_exhausted": mcp_selection.schema_budget_exhausted,
            "openrouter_session_affinity": is_openrouter_endpoint(url)
                && bounded_openrouter_session_id(expected_session_id).is_some(),
        }),
    );

    let resolved_url = profile
        .as_ref()
        .map(crate::config::ModelProfile::endpoint_url)
        .unwrap_or_else(|| {
            let trimmed = url.trim_end_matches('/');
            if responses_api {
                if trimmed.ends_with("/responses") {
                    trimmed.to_string()
                } else if let Some(base) = trimmed.strip_suffix("/chat/completions") {
                    format!("{base}/responses")
                } else if let Some(base) = trimmed.strip_suffix("/chats/completion") {
                    format!("{base}/responses")
                } else {
                    format!("{trimmed}/responses")
                }
            } else if trimmed.ends_with("/chat/completions")
                || trimmed.ends_with("/chats/completion")
            {
                trimmed.to_string()
            } else {
                format!("{trimmed}/chat/completions")
            }
        });

    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| {
                m.matches_request(url, model)
                    || m.url == url
                    || m.name == s.model_name
                    || m.endpoint_url() == resolved_url
            })
            .and_then(|m| m.resolved_api_key())
    };
    let (trace_session_id, assistant_turn) = {
        let s = state.lock().await;
        let assistant_turn = s
            .history
            .iter()
            .filter(|message| message.role == "assistant")
            .count()
            + 1;
        (
            expected_session_id
                .unwrap_or(&s.active_session_id)
                .to_owned(),
            assistant_turn,
        )
    };
    // The trace is deliberately tied to the existing verbose network-debug
    // switch. Its Drop implementation flushes a bounded summary even when a
    // stream exits through a timeout, cancellation, or parse error.
    let mut stream_trace = ProviderStreamTrace::new(
        verbose_network_logging,
        &trace_session_id,
        model,
        assistant_turn,
    );

    let mut request_payload_bytes = payload_bytes;
    let mut parallel_tool_calls_fallback_attempted = false;
    let mut attempt = 0usize;
    let response = loop {
        if cancel_token.is_cancelled() {
            return Err(StreamFailure::new(StreamFailureKind::Cancelled));
        }
        let mut req = client
            .post(&resolved_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_payload_bytes.clone());
        if let Some(ref key) = api_key {
            req = req
                .header("Authorization", format!("Bearer {key}"))
                .header("X-Api-Key", key);
        }
        let send_result = retry::race_cancellable(
            tokio::time::timeout(retry::HEADER_TIMEOUT, req.send()),
            &cancel_token,
        )
        .await;
        let send_result = match send_result {
            None => return Err(StreamFailure::new(StreamFailureKind::Cancelled)),
            Some(Err(_elapsed)) => {
                if attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: timed out waiting for response headers (attempt {}/{}), backing off {}ms",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err(StreamFailure::new(StreamFailureKind::Cancelled));
                    }
                    attempt += 1;
                    continue;
                }
                return Err(StreamFailure::new(StreamFailureKind::HeaderTimeout));
            }
            Some(Ok(r)) => r,
        };
        match send_result {
            Ok(resp) if resp.status().is_success() => {
                stream_trace.response_headers(resp.status().as_u16());
                dbg_log!(
                    "stream_request: Received response status: {}",
                    resp.status()
                );
                crate::logger::operational_event(
                    "provider.response_headers",
                    serde_json::json!({
                        "model": model,
                        "status": resp.status().as_u16(),
                        "elapsed_ms": request_start_time.elapsed().as_millis() as u64,
                    }),
                );
                break resp;
            }
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                stream_trace.response_headers(code);
                let err_body = match retry::race_cancellable(resp.text(), &cancel_token).await {
                    None => return Err(StreamFailure::new(StreamFailureKind::Cancelled)),
                    Some(body) => body.unwrap_or_default(),
                };
                if !parallel_tool_calls_fallback_attempted
                    && let Some(fallback_payload_bytes) =
                        parallel_tool_calls_fallback_payload_bytes.as_ref()
                    && is_parallel_tool_calls_rejection(code, &err_body)
                {
                    dbg_log!(
                        "stream_request: provider rejected parallel_tool_calls; retrying without optional field"
                    );
                    crate::logger::operational_event(
                        "provider.parallel_tool_calls_fallback",
                        serde_json::json!({
                            "model": model,
                            "status": code,
                            "action": "retry_without_optional_field",
                        }),
                    );
                    request_payload_bytes = fallback_payload_bytes.clone();
                    parallel_tool_calls_fallback_attempted = true;
                    continue;
                }
                if retry::is_retryable_status(code) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, code);
                    dbg_log!(
                        "stream_request: retryable status {} (attempt {}/{}), backing off {}ms",
                        status,
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err(StreamFailure::new(StreamFailureKind::Cancelled));
                    }
                    attempt += 1;
                    continue;
                }
                dbg_log!(
                    "stream_request: Request failed with status {} (error_body_bytes={})",
                    status,
                    err_body.len()
                );
                return Err(StreamFailure {
                    kind: StreamFailureKind::ProviderError,
                    status: Some(code),
                    detail: Some(err_body),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                });
            }
            Err(e) => {
                if retry::is_retryable_transport(&e) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    dbg_log!(
                        "stream_request: transient network error (attempt {}/{}), backing off {}ms: {}",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis(),
                        e
                    );
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err(StreamFailure::new(StreamFailureKind::Cancelled));
                    }
                    attempt += 1;
                    continue;
                }
                let kind = if e.is_timeout() || e.is_connect() {
                    StreamFailureKind::ConnectTimeout
                } else {
                    StreamFailureKind::ProviderError
                };
                let mut msg = format!("Request failed: {e}");
                let mut src = std::error::Error::source(&e);
                while let Some(cause) = src {
                    msg.push_str(&format!(": {cause}"));
                    src = cause.source();
                }
                return Err(StreamFailure {
                    kind,
                    status: None,
                    detail: Some(msg),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                });
            }
        }
    };

    let stream = response
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);
    let mut in_reasoning = false;
    let mut finish_reason: Option<String> = None;
    let mut stream_bytes_received = 0usize;
    let mut stream_events_received = 0usize;

    let mut accumulators = ToolAccumulatorSet::default();
    let mut response_call_ids = HashMap::new();
    let mut response_argument_deltas = HashSet::new();
    let mut tool_argument_limit_reached = false;
    let mut reasoning_detector = super::loop_detect::ReasoningLoopDetector::default();

    dbg_log!("stream_request: Starting SSE stream read loop");
    // Absolute progress deadlines anchored at headers. Keep-alive blank /
    // comment lines carry no meaningful event and must not extend them: the
    // per-fill timeout inside `read_sse_line_with_state` still tolerates a
    // slow partial line, but these bounds fire even while the wire stays
    // active. Without them a provider (or proxy) emitting periodic blank
    // lines could stall the first event forever.
    let stream_start = tokio::time::Instant::now();
    let first_event_deadline = stream_start + retry::FIRST_EVENT_TIMEOUT;
    let mut last_progress = stream_start;
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Err(StreamFailure::new(StreamFailureKind::Cancelled));
        }

        // Absolute deadline for this read (see `sse_progress_deadline`):
        // keep-alive blank/comment lines never move these markers.
        let absolute_deadline =
            sse_progress_deadline(first_event_deadline, last_progress, stream_events_received);

        tokio::select! {
            r = tokio::time::timeout_at(
                absolute_deadline,
                read_sse_line_with_state(
                    &mut reader,
                    &mut line_buf,
                    stream_events_received == 0,
                ),
            ) => {
                let r = match r {
                    Err(_) => {
                        let past_first_deadline = stream_events_received == 0
                            && tokio::time::Instant::now() >= first_event_deadline;
                        let kind = if past_first_deadline {
                            StreamFailureKind::FirstEventTimeout
                        } else {
                            StreamFailureKind::StreamIdleTimeout
                        };
                        dbg_log!(
                            "stream_request: SSE absolute progress deadline elapsed ({kind}, events={stream_events_received}, bytes={stream_bytes_received})"
                        );
                        crate::logger::operational_event(
                            "stream.progress_deadline",
                            serde_json::json!({
                                "model": model,
                                "kind": kind.to_string(),
                                "events_received": stream_events_received,
                                "bytes_received": stream_bytes_received,
                                "partial_event_bytes": line_buf.len(),
                                "elapsed_ms": stream_start.elapsed().as_millis() as u64,
                            }),
                        );
                        return Err(StreamFailure {
                            kind,
                            status: None,
                            detail: Some(format!(
                                "SSE stream {kind} with {stream_events_received} events after {stream_bytes_received} bytes"
                            )),
                            bytes_received: stream_bytes_received,
                            events_received: stream_events_received,
                            partial_event_bytes: line_buf.len(),
                        });
                    }
                    Ok(inner) => inner,
                };
                match r {
                    Ok(0) => {
                        dbg_log!("stream_request: SSE stream read EOF (0 bytes)");
                        if finish_reason.is_none() {
                            return Err(StreamFailure {
                                kind: StreamFailureKind::PrematureEof,
                                status: None,
                                detail: None,
                                bytes_received: stream_bytes_received,
                                events_received: stream_events_received,
                                partial_event_bytes: 0,
                            });
                        }
                        break;
                    }
                    Ok(_) => {
                        stream_bytes_received += line_buf.len();
                        let trimmed = line_buf.trim();
                        if trimmed == "data: [DONE]" {
                            buffer.lock().await.termination =
                                Some(StreamTermination::ProviderStop);
                            line_buf.clear();
                            break;
                        }
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                                stream_events_received += 1;
                                // Only meaningful events move the progress
                                // markers: keep-alive blank/comment lines and
                                // unparsable payloads must not extend the
                                // absolute first-event / idle budgets.
                                last_progress = tokio::time::Instant::now();
                                stream_trace.record(line_buf.len(), &value);
                                let val = if responses_api {
                                    normalize_responses_event(
                                        &value,
                                        &mut response_call_ids,
                                        &mut response_argument_deltas,
                                    )
                                    .unwrap_or_else(|| serde_json::json!({}))
                                } else {
                                    value
                                };
                                if let Some(error) = val.get("error") {
                                    let status = error
                                        .get("status_code")
                                        .and_then(|value| value.as_u64())
                                        .and_then(|value| u16::try_from(value).ok());
                                    let detail = error
                                        .get("message")
                                        .and_then(|value| value.as_str())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| error.to_string());
                                    return Err(StreamFailure {
                                        kind: StreamFailureKind::ProviderError,
                                        status,
                                        detail: Some(detail),
                                        bytes_received: stream_bytes_received,
                                        events_received: stream_events_received,
                                        partial_event_bytes: 0,
                                    });
                                }
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array())
                                    && !choices.is_empty() {
                                        let provider_stop = choices[0]
                                            .get("finish_reason")
                                            .and_then(|f| f.as_str())
                                            == Some("stop");
                                        if let Some(fr) = choices[0].get("finish_reason").and_then(|f| f.as_str()) {
                                            finish_reason = Some(fr.to_string());
                                        }
                                         let delta = choices[0].get("delta");
                                         let reasoning = delta
                                             .and_then(|d| {
                                                 d.get("reasoning")
                                                     .or_else(|| d.get("reasoning_content"))
                                                     .or_else(|| d.get("thought"))
                                                     .or_else(|| d.get("thinking"))
                                             })
                                             .and_then(|r| r.as_str());
                                         let content = delta
                                             .and_then(|d| d.get("content").or_else(|| d.get("text")))
                                             .and_then(|c| c.as_str());

                                         if let Some(tool_calls) = delta.and_then(|d| d.get("tool_calls")).and_then(|t| t.as_array()) {
                                             for tc in tool_calls {
                                                 let idx = accumulators.add_delta(tc);
                                                 buffer.lock().await.native_tool_call_checkpoint = accumulators.checkpoints();
                                                 let acc = &accumulators.calls[idx];
                                                 tool_argument_limit_reached |= acc.arguments_overflowed;
                                                 if !quiet && !acc.name.is_empty() {
                                                     let parsed_args = parse_speculative_arguments(&acc.arguments);
                                                     let id_opt = if acc.id.is_empty() { None } else { Some(acc.id.as_str()) };
                                                     let mut s = state.lock().await;
                                                     if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                         return Ok(None);
                                                     }
                                                     s.update_speculative_live_tool_call(id_opt, &acc.name, &parsed_args);
                                                 }
                                             }
                                         }
                                         if tool_argument_limit_reached {
                                             finish_reason = Some("tool_arguments_limit".to_string());
                                             line_buf.clear();
                                             break;
                                         }

                                         let mut chunk = String::new();
                                        let mut reasoning_loop_cut = false;
                                        let mut reasoning_budget_cut = false;
                                        if let Some(r_token) = reasoning {
                                            if !in_reasoning {
                                                in_reasoning = true;
                                                let started = std::time::Instant::now();
                                                {
                                                    let mut buffer = buffer.lock().await;
                                                    // A later reasoning segment means the prior
                                                    // content boundary is no longer final.
                                                    buffer.final_answer_boundary =
                                                        super::stream::FinalAnswerBoundary::None;
                                                    buffer.provider_final_answer_state =
                                                        super::stream::ProviderFinalAnswerState::None;
                                                    buffer.thought_started_at = Some(started);
                                                }
                                                if !quiet {
                                                    let mut s = state.lock().await;
                                                    if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                        return Ok(None);
                                                    }
                                                    s.current_thought_started_at = Some(started);
                                                }
                                                chunk.push_str("<think>\n");
                                            }
                                            let used_tokens =
                                                buffer.lock().await.thought_tokens_estimate();
                                            let bounded = bound_reasoning_chunk(
                                                r_token,
                                                used_tokens,
                                                thinking_budget,
                                            );
                                            let thought_delta = {
                                                let mut buffer = buffer.lock().await;
                                                buffer.thought_chars = buffer
                                                    .thought_chars
                                                    .saturating_add(bounded.text.len());
                                                let total = buffer.thought_tokens_estimate();
                                                let delta = total
                                                    .saturating_sub(buffer.thought_tokens);
                                                buffer.thought_tokens = total;
                                                delta
                                            };
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                    return Ok(None);
                                                }
                                                s.current_thought_tokens = s
                                                    .current_thought_tokens
                                                    .saturating_add(thought_delta);
                                            }
                                            chunk.push_str(&bounded.text);
                                            if let super::loop_detect::ReasoningLoopStatus::LoopDetected(reason) =
                                                reasoning_detector.feed_chunk(&bounded.text)
                                            {
                                                dbg_log!(
                                                    "stream_request: reasoning loop detected ({reason}) — stopping stream cleanly"
                                                );
                                                crate::logger::operational_event(
                                                    "stream.reasoning_loop_cut",
                                                    serde_json::json!({ "reason": reason }),
                                                );
                                                finish_reason = Some("reasoning_loop".to_string());
                                                reasoning_loop_cut = true;
                                            }
                                            if bounded.budget_exhausted {
                                                dbg_log!(
                                                    "stream_request: client reasoning budget reached ({} estimated tokens) — stopping stream cleanly",
                                                    thinking_budget.unwrap_or_default()
                                                );
                                                crate::logger::operational_event(
                                                    "stream.reasoning_budget_cut",
                                                    serde_json::json!({
                                                        "budget": thinking_budget,
                                                        "estimated_thought_tokens": used_tokens.saturating_add(thought_delta),
                                                    }),
                                                );
                                                finish_reason = Some("reasoning_budget".to_string());
                                                buffer.lock().await.termination =
                                                    Some(StreamTermination::ClientBudget);
                                                reasoning_budget_cut = true;
                                            }
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
                                                {
                                                    let mut buffer = buffer.lock().await;
                                                    buffer.final_answer_boundary = super::stream::FinalAnswerBoundary::ReasoningClosed;
                                                    buffer.finish_thought();
                                                }
                                                if !quiet {
                                                    let mut s = state.lock().await;
                                                    if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                        return Ok(None);
                                                    }
                                                    if let Some(started) = s.current_thought_started_at.take() {
                                                        s.current_thought_time_ms = s
                                                            .current_thought_time_ms
                                                            .saturating_add(started.elapsed().as_millis() as u64);
                                                    }
                                                }
                                                chunk.push_str("\n</think>\n\n");
                                            }
                                            chunk.push_str(c_token);
                                        }
                                        if provider_stop {
                                            let mut buffer = buffer.lock().await;
                                            buffer.termination =
                                                Some(StreamTermination::ProviderStop);
                                            if reasoning.is_none()
                                                && buffer.final_answer_boundary
                                                    == super::stream::FinalAnswerBoundary::ReasoningClosed
                                            {
                                                buffer.provider_final_answer_state =
                                                    super::stream::ProviderFinalAnswerState::Terminal;
                                            }
                                        }
                                        if !chunk.is_empty() {
                                            let tokens = (chunk.len() as f64 * crate::app::TOKENS_PER_CHAR_APPROX) as u32;
                                            let mut s = state.lock().await;
                                            if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                return Ok(None);
                                            }
                                            if let Some(ref mut tracker) = s.stream_tracker {
                                                tracker.tokens_so_far += tokens;
                                                tracker.record_chunk();
                                            }
                                            drop(s);

                                            buffer.lock().await.content.push_str(&chunk);
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                    return Ok(None);
                                                }
                                                s.append_current_response(&chunk);
                                                if s.raw_cli_mode {
                                                    use std::io::Write;
                                                    print!("{chunk}");
                                                    let _ = std::io::stdout().flush();
                                                }
                                            }
                                            if !quiet {
                                                let buf_content = { buffer.lock().await.content.clone() };
                                                if let Some((tool_name, tool_args)) = parse_speculative_text_tool_call(&buf_content) {
                                                    let mut s = state.lock().await;
                                                    if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                        return Ok(None);
                                                    }
                                                    s.update_speculative_live_tool_call(None, &tool_name, &tool_args);
                                                } else if let Some((tool_name, tool_args)) = parse_speculative_native_tool_call(&buf_content) {
                                                    let mut s = state.lock().await;
                                                    if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                        return Ok(None);
                                                    }
                                                    s.update_speculative_native_tool_call(&tool_name, &tool_args);
                                                }
                                            }
                                        }
                                        if reasoning_loop_cut || reasoning_budget_cut {
                                            line_buf.clear();
                                            break;
                                        }
                                    }
                                if let Some(usage) = val.get("usage").filter(|_| !quiet)
                                    && let (Some(p), Some(c), Some(t)) = (
                                        usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                        usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                        usage.get("total_tokens").and_then(|v| v.as_u64()),
                                    ) {
                                        let (cached, cache_write_tokens, cache_discount) =
                                            cache_usage_metrics(usage);
                                        let (provider_cache_status, provider_cache_reason) =
                                            provider_cache_observation(
                                                is_openrouter_endpoint(url)
                                                    && bounded_openrouter_session_id(
                                                        expected_session_id,
                                                    )
                                                    .is_some(),
                                                cached,
                                                cache_write_tokens,
                                            );
                                        let observed_reasoning_tokens = usage
                                            .get("completion_tokens_details")
                                            .and_then(|details| details.get("reasoning_tokens"))
                                            .and_then(|v| v.as_u64())
                                            .or_else(|| {
                                                usage
                                                    .get("reasoning_tokens")
                                                    .and_then(|v| v.as_u64())
                                            });
                                        let observed_answer_tokens = observed_reasoning_tokens
                                            .map(|reasoning| c.saturating_sub(reasoning));

                                        let mut s = state.lock().await;
                                        if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                            return Ok(None);
                                        }
                                        s.current_token_usage = Some(TokenUsage {
                                            prompt_tokens: p as u32,
                                            completion_tokens: c as u32,
                                            total_tokens: t as u32,
                                            cached_tokens: cached,
                                            cache_write_tokens,
                                            cache_discount,
                                        });

                                        let estimation_delta = if estimated_prompt_tokens > 0 {
                                            Some((p as i64) - (estimated_prompt_tokens as i64))
                                        } else {
                                            None
                                        };
                                        let estimation_delta_percent =
                                            prompt_estimation_delta_percent(estimated_prompt_tokens, p);

                                        crate::logger::operational_event(
                                            "provider.completion",
                                            serde_json::json!({
                                                "model": model,
                                                "prompt_tokens": p,
                                                "completion_tokens": c,
                                                "total_tokens": t,
                                                "cached_tokens": cached,
                                                "cache_write_tokens": cache_write_tokens,
                                                "cache_discount": cache_discount,
                                                "provider_cache_status": provider_cache_status,
                                                "provider_cache_reason": provider_cache_reason,
                                                "openrouter_session_affinity": is_openrouter_endpoint(url)
                                                    && bounded_openrouter_session_id(expected_session_id)
                                                        .is_some(),
                                                "requested_max_output_tokens": output_token_limit,
                                                "requested_max_tokens": output_token_limit,
                                                "requested_thinking_budget": thinking_budget,
                                                "thinking_budget": thinking_budget,
                                                "requested_minimum_answer_tokens": profile
                                                    .as_ref()
                                                    .map(|p| p.context_budget().minimum_answer_tokens),
                                                "observed_reasoning_tokens": observed_reasoning_tokens,
                                                "observed_answer_tokens": observed_answer_tokens,
                                                "completion_limit_reached": output_token_limit
                                                    .is_some_and(|limit| c >= u64::from(limit)),
                                                "estimated_prompt_tokens": estimated_prompt_tokens,
                                                "accounted_prompt_tokens": accounted_prompt_tokens,
                                                "tool_schema_tokens": tool_schema_tokens,
                                                "total_estimated_prompt_tokens": accounted_prompt_tokens,
                                                "provider_overhead_margin": provider_overhead_margin,
                                                "estimation_delta": estimation_delta,
                                                "estimation_delta_percent": estimation_delta_percent,
                                                "elapsed_ms": request_start_time.elapsed().as_millis() as u64,
                                            }),
                                        );
                                    }
                            } else {
                                stream_trace.record_malformed(line_buf.len());
                                dbg_log!(
                                    "stream_request: Failed to parse JSON from data payload (bytes={})",
                                    json_str.len()
                                );
                                return Err(StreamFailure {
                                    kind: StreamFailureKind::MalformedSse,
                                    status: None,
                                    detail: Some("invalid JSON data payload".to_string()),
                                    bytes_received: stream_bytes_received,
                                    events_received: stream_events_received,
                                    partial_event_bytes: 0,
                                });
                            }
                        }
                        line_buf.clear();
                    }
                    Err(e) => {
                        dbg_log!("stream_request: SSE read error: {}", e);
                        return Err(StreamFailure {
                            kind: e.kind(),
                            status: None,
                            detail: Some(e.to_string()),
                            bytes_received: stream_bytes_received,
                            events_received: stream_events_received,
                            partial_event_bytes: e.partial_event_bytes(),
                        });
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                dbg_log!("stream_request: Cancelled via select branch");
                return Err(StreamFailure::new(StreamFailureKind::Cancelled));
            }
        }
    }

    if in_reasoning {
        buffer.lock().await.finish_thought();
        if !quiet {
            let mut s = state.lock().await;
            if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                return Ok(None);
            }
            if let Some(started) = s.current_thought_started_at.take() {
                s.current_thought_time_ms = s
                    .current_thought_time_ms
                    .saturating_add(started.elapsed().as_millis() as u64);
            }
        }
        buffer.lock().await.content.push_str("\n</think>\n\n");
        if !quiet {
            let mut s = state.lock().await;
            if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                return Ok(None);
            }
            s.append_current_response("\n</think>\n\n");
            if s.raw_cli_mode {
                use std::io::Write;
                print!("\n</think>\n\n");
                let _ = std::io::stdout().flush();
            }
        }
    }

    let mut streamed_call_ids: Vec<String> = Vec::new();
    let mut native_tool_calls: Vec<crate::tools::ToolCallEnvelope> = Vec::new();
    let mut call_order: Vec<usize> = (0..accumulators.calls.len()).collect();
    // Indexed streams have a provider-defined response order. Unidentified
    // deltas retain arrival order and are placed after identified calls.
    call_order.sort_by_key(|&position| {
        let acc = &accumulators.calls[position];
        (
            acc.index.is_none(),
            acc.index.unwrap_or(usize::MAX),
            position,
        )
    });
    for (position, call_index) in call_order.into_iter().enumerate() {
        let acc = &accumulators.calls[call_index];
        if acc.name.is_empty() {
            continue;
        }

        let args_json = if acc.arguments_overflowed {
            serde_json::json!({
                "_invalid_arguments": {
                    "kind": "argument_limit",
                    "original_bytes_at_least": acc.argument_bytes,
                    "max_bytes": MAX_NATIVE_TOOL_ARGUMENT_BYTES,
                    "truncated": true,
                    "execution": "rejected",
                },
                "_parse_error": "tool arguments exceeded the local streaming limit",
                "_recovery": "No tool was executed. Emit one smaller complete call or use a focused edit; never continue from the truncated preview.",
            })
        } else {
            parse_native_tool_arguments(&acc.arguments)
        };

        let call_id = if acc.id.is_empty() {
            format!("call_{position}")
        } else {
            acc.id.clone()
        };
        streamed_call_ids.push(call_id.clone());
        native_tool_calls.push(crate::tools::ToolCallEnvelope {
            call_id,
            tool_name: acc.name.clone(),
            arguments: args_json,
        });
    }

    if !native_tool_calls.is_empty() {
        // Responses emits `response.completed` for both text and function-call
        // output. The downstream turn runner uses `tool_calls` to distinguish
        // an actionable response from a final prose response.
        if finish_reason.is_none() || finish_reason.as_deref() == Some("stop") {
            finish_reason = Some("tool_calls".to_string());
        }
        dbg_log!(
            "stream_request: preserving {} native tool call envelope(s)",
            native_tool_calls.len()
        );
        {
            let mut buf = buffer.lock().await;
            buf.tool_call_ids = streamed_call_ids;
            buf.native_tool_calls = native_tool_calls;
            buf.native_tool_call_checkpoint.clear();
        }
    }

    let mut buf = buffer.lock().await;
    buf.content = buf
        .content
        .trim_end_matches(char::is_whitespace)
        .to_string();
    dbg_log!(
        "stream_request: Stream request loop ended. Total content: {} chars",
        buf.content.len()
    );
    Ok(finish_reason)
}
