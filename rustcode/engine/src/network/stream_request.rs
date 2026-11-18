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
mod responses;
pub(crate) use responses::*;

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
    if let Some(p) = profile.filter(|p| p.supports_reasoning_effort_wire())
        && let Some(effort) = p.reasoning_effort.as_deref()
    {
        payload["reasoning_effort"] =
            serde_json::json!(request_reasoning_effort(p, effort, thinking_mode));
    }
    if let Some(thinking_budget) = profile
        .filter(|p| p.supports_thinking_budget_wire())
        .and_then(|p| p.thinking_budget)
    {
        payload["thinking_budget"] = serde_json::json!(thinking_budget);
    }
}

/// Attach auth headers to an outgoing provider request. `X-Api-Key` is
/// strictly opt-in (`send_x_api_key = true` on the profile): gateways such as
/// Splash reject any request carrying it alongside `Authorization`, even when
/// the key itself is valid.
pub(crate) fn apply_api_key_headers(
    req: reqwest::RequestBuilder,
    key: &str,
    send_x_api_key: bool,
) -> reqwest::RequestBuilder {
    let req = req.header("Authorization", format!("Bearer {key}"));
    if send_x_api_key {
        req.header("X-Api-Key", key)
    } else {
        req
    }
}

/// Send `frequency_penalty` only when the profile sets it. The historical
/// injected 0.3 default is rejected by backends such as Splash as an
/// unsupported output transformation.
pub(crate) fn apply_frequency_penalty(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
    url: &str,
) {
    if !url.contains("generativelanguage.googleapis.com")
        && let Some(penalty) = profile.and_then(|p| p.frequency_penalty)
    {
        payload["frequency_penalty"] = serde_json::json!(penalty);
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
pub enum ThinkingMode {
    Normal,
    BoundedRecovery,
    Disabled,
    /// Retry after the provider stopped at its output cap: lift RustCode's
    /// default cap to the profile's full completion budget and, when the
    /// stop produced nothing, lower the reasoning effort.
    OutputRecovery {
        reduce_reasoning: bool,
    },
}

impl ThinkingMode {
    /// Whether the profile's own reasoning settings apply to this request.
    fn keeps_profile_reasoning(self) -> bool {
        matches!(self, Self::Normal | Self::OutputRecovery { .. })
    }
}

/// The effort to send for `effort`, lowered for a retry whose previous
/// attempt spent the whole output cap reasoning. Uses `low` when the profile
/// lists it (or lists nothing), otherwise the cheapest listed effort that
/// still reasons.
fn request_reasoning_effort<'a>(
    profile: &'a crate::config::ModelProfile,
    effort: &'a str,
    thinking_mode: ThinkingMode,
) -> &'a str {
    if !matches!(
        thinking_mode,
        ThinkingMode::OutputRecovery {
            reduce_reasoning: true
        }
    ) || matches!(effort, "none" | "off" | "minimal" | "low")
    {
        return effort;
    }
    let listed = profile.reasoning_efforts.as_deref().unwrap_or_default();
    if listed.is_empty() || listed.iter().any(|e| e == "low") {
        return "low";
    }
    listed
        .iter()
        .map(String::as_str)
        .find(|e| !matches!(*e, "none" | "off"))
        .unwrap_or(effort)
}

/// Keep a wire `thinking_budget` below the request's output cap so the
/// provider cannot spend the whole cap reasoning and return no answer.
fn bound_wire_thinking_budget(payload: &mut serde_json::Value, output_token_limit: Option<u32>) {
    let (Some(limit), Some(budget)) = (
        output_token_limit,
        payload
            .get("thinking_budget")
            .and_then(serde_json::Value::as_u64),
    ) else {
        return;
    };
    if budget >= u64::from(limit) {
        let answer_room = (limit / 4).max(1);
        payload["thinking_budget"] = serde_json::json!(limit.saturating_sub(answer_room).max(1));
    }
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
    openrouter: bool,
    affinity_requested: bool,
    cached_tokens: Option<u32>,
    cache_write_tokens: Option<u32>,
) -> (&'static str, &'static str) {
    // Session affinity is an OpenRouter request option. Other providers cache
    // on their own, so what they report is the observation.
    if openrouter && !affinity_requested {
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
const RECOVERY_MAX_TOKENS: u32 = 1024;
/// Hard limit on one streamed tool call's JSON arguments. The write_to_file
/// spec states it, so the model sees it before generating a large file.
pub(crate) const MAX_NATIVE_TOOL_ARGUMENT_BYTES: usize = 40 * 1024;
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

/// The wire output cap for one request, before the context-capacity clamp.
fn request_output_token_limit(
    profile: Option<&crate::config::ModelProfile>,
    allow_tools: bool,
    thinking_mode: ThinkingMode,
    fallback_max_tokens: u32,
) -> Option<u32> {
    let bounded_recovery = thinking_mode == ThinkingMode::BoundedRecovery;
    match profile {
        Some(p) if matches!(thinking_mode, ThinkingMode::OutputRecovery { .. }) => {
            p.output_recovery_token_limit(allow_tools)
        }
        // The recovery clamp assumes reasoning can be switched off for the
        // retry. Where it cannot (hidden Responses reasoning, or a profile
        // without an `enable_thinking` control), the clamp would starve the
        // answer.
        Some(p) if p.reasoning_shares_output_cap() || p.enable_thinking.is_none() => {
            p.output_token_limit(allow_tools, bounded_recovery)
        }
        Some(p) => p
            .output_token_limit(allow_tools, bounded_recovery)
            .map(|limit| clamp_request_max_tokens(limit, thinking_mode)),
        None => (allow_tools || bounded_recovery).then_some(fallback_max_tokens),
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

/// Client-side cap on streamed reasoning for `profile`. A derived budget of
/// zero means the profile did not request thinking, not that the provider may
/// not think: a provider that reasons by default would otherwise be cut at its
/// first reasoning delta, every round, before it produced any answer.
fn client_thinking_budget(profile: &crate::config::ModelProfile) -> Option<u32> {
    profile
        .client_reasoning_budget
        .or_else(|| Some(profile.context_budget().thinking_budget).filter(|budget| *budget > 0))
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

fn request_workspace_root(state: &crate::app::AppState) -> Option<std::path::PathBuf> {
    state.effective_workspace_root()
}

#[cfg(test)]
#[path = "stream_request_tests.rs"]
mod tests;

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
    read_sse_line_with_state(reader, line_buf, false, retry::STREAM_IDLE_TIMEOUT).await
}

async fn read_sse_line_with_state<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut String,
    waiting_for_first_event: bool,
    timeout: std::time::Duration,
) -> Result<usize, SseReadError> {
    let mut bytes = Vec::new();

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
#[cfg(test)]
fn sse_progress_deadline(
    first_event_deadline: tokio::time::Instant,
    last_progress: tokio::time::Instant,
    events_received: usize,
) -> tokio::time::Instant {
    sse_progress_deadline_with_timeouts(
        first_event_deadline,
        last_progress,
        events_received,
        retry::STREAM_IDLE_TIMEOUT,
    )
}

fn sse_progress_deadline_with_timeouts(
    first_event_deadline: tokio::time::Instant,
    last_progress: tokio::time::Instant,
    events_received: usize,
    stream_idle_timeout: std::time::Duration,
) -> tokio::time::Instant {
    if events_received == 0 {
        first_event_deadline.min(last_progress + stream_idle_timeout)
    } else {
        last_progress + stream_idle_timeout
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
    for (key, re) in SPECULATIVE_ARGUMENT_PATTERNS.iter() {
        if let Some(caps) = re.captures(trimmed) {
            if let Some(m) = caps.get(1) {
                map.insert(
                    (*key).to_string(),
                    serde_json::Value::String(m.as_str().replace(r#"\""#, "\"")),
                );
            }
        }
    }
    serde_json::Value::Object(map)
}

/// One pattern per argument worth showing while a tool call is still
/// streaming. Compiled once: `parse_speculative_arguments` runs on every
/// argument delta, and a delta that ends mid-key or mid-escape always reaches
/// this fallback.
static SPECULATIVE_ARGUMENT_PATTERNS: std::sync::LazyLock<Vec<(&'static str, regex::Regex)>> =
    std::sync::LazyLock::new(|| {
        [
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
        ]
        .into_iter()
        .map(|key| {
            let pattern = format!(r#""{}"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)"#, key);
            let re = regex::Regex::new(&pattern).expect("speculative argument regex");
            (key, re)
        })
        .collect()
    });

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
    // Count the reply on its own. Encoding `prompt + reply` to subtract the
    // prompt again was a second full-context BPE pass on every round, and the
    // concatenation never hit the token memo.
    let completion = if reply.is_empty() {
        0
    } else {
        count_tokens(reply)
    };
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt.saturating_add(completion),
        cached_tokens: None,
        ..Default::default()
    })
}

fn prompt_estimation_delta_percent(estimated: u32, observed: u64) -> Option<i64> {
    (estimated > 0).then(|| {
        (((observed as i128 - i128::from(estimated)) * 100) / i128::from(estimated)) as i64
    })
}

// --------------------------------------------------------- attempt telemetry
//
// One row per actual send (`crate::data_store::ProviderRequestRow`) so a slow
// or failing turn can be read back instead of guessed at. Metadata is always
// recorded; the payloads are opt-in (`capture_provider_payloads`) and
// redacted, clipped and never a copy of an authorization header. See
// `docs/data-store.md`.

/// The most of a provider's own error text a row keeps (`response_summary_json`
/// is clipped to this before it reaches the store).
const MAX_ATTEMPT_ERROR_BYTES: usize = 512;

/// What every attempt of one provider request shares.
struct ProviderAttemptContext {
    thread_id: String,
    turn_id: Option<String>,
    round: Option<i64>,
    provider: Option<String>,
    model: String,
    endpoint_host: Option<String>,
    endpoint_path: Option<String>,
    capture_payloads: bool,
    /// The request body as it is sent, kept only when payload capture is on:
    /// otherwise a turn must not hold a second copy of it.
    payload: Option<serde_json::Value>,
}

/// One built-in provider HTTP attempt.
///
/// Created right before a send, updated as the attempt progresses, and
/// recorded by its `Drop`: a success, a retry, a provider error and a
/// cancellation therefore write exactly one row each, and no exit can skip
/// the write. The retry paths end the attempt before backing off, so the
/// recorded latency is the attempt's own and not the wait after it.
struct ProviderAttempt {
    row: crate::data_store::ProviderRequestRow,
    cancel_token: tokio_util::sync::CancellationToken,
    started: std::time::Instant,
    capture_payloads: bool,
    payload: Option<serde_json::Value>,
    finish_reason: Option<String>,
    error_message: Option<String>,
    /// Set when the attempt reached a clean end, so an interrupted one is
    /// classified by what was left behind instead of looking successful.
    reported_completion: bool,
}

impl ProviderAttempt {
    fn start(
        context: &ProviderAttemptContext,
        attempt: usize,
        request_bytes: usize,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> Self {
        Self {
            row: crate::data_store::ProviderRequestRow {
                thread_id: context.thread_id.clone(),
                turn_id: context.turn_id.clone(),
                round: context.round,
                provider: context.provider.clone(),
                model: Some(context.model.clone()),
                endpoint_host: context.endpoint_host.clone(),
                endpoint_path: context.endpoint_path.clone(),
                method: Some("POST".to_owned()),
                attempt: i64::try_from(attempt).ok(),
                started_at: now_rfc3339(),
                request_bytes: i64::try_from(request_bytes).ok(),
                ..Default::default()
            },
            cancel_token: cancel_token.clone(),
            started: std::time::Instant::now(),
            capture_payloads: context.capture_payloads,
            payload: context.payload.clone(),
            finish_reason: None,
            error_message: None,
            reported_completion: false,
        }
    }

    /// The response headers arrived, so the attempt is a response.
    fn headers(&mut self, status: u16) {
        self.row.headers_at = Some(now_rfc3339());
        self.row.status_code = Some(i64::from(status));
    }

    /// How much of the stream was read so far. Called as the stream is read
    /// so a timeout or a cancellation still reports its progress.
    fn progress(&mut self, events: usize, bytes: usize) {
        self.row.stream_event_count = i64::try_from(events).ok();
        self.row.response_bytes = i64::try_from(bytes).ok();
    }

    fn usage(&mut self, input: u64, output: u64, cached: Option<u64>) {
        self.row.input_tokens = i64::try_from(input).ok();
        self.row.output_tokens = i64::try_from(output).ok();
        self.row.cached_tokens = cached.and_then(|cached| i64::try_from(cached).ok());
    }

    /// The exit that knows why: `timeout`, `connect`, `stream`.
    ///
    /// The kind is a fixed label, never the provider's text.
    fn fail(&mut self, kind: &str) {
        if self.row.error_kind.is_none() {
            self.row.error_kind = Some(kind.to_owned());
        }
    }

    fn provider_message(&mut self, message: &str) {
        let lower = message.to_ascii_lowercase();
        let kind =
            if lower.contains("insufficient_balance") || lower.contains("insufficient balance") {
                Some("insufficient_balance")
            } else if lower.contains("rate_limit")
                || lower.contains("rate limit")
                || self.row.status_code == Some(429)
            {
                Some("rate_limit")
            } else {
                None
            };
        if let Some(kind) = kind {
            self.fail(kind);
        }
        self.error_message = Some(message.to_owned());
    }

    /// The stream reached its end, so the attempt itself succeeded.
    fn completed(&mut self, finish_reason: Option<String>) {
        self.reported_completion = true;
        self.finish_reason = finish_reason;
        self.row.error_kind = None;
    }

    /// The short kind of the attempt: what its exit said, else cancelled, a
    /// non-2xx status, a stream that stopped early, or a send that never
    /// reached the provider. `None` is a row that ended cleanly.
    fn derived_error_kind(&self) -> Option<String> {
        if self.reported_completion {
            return None;
        }
        if let Some(kind) = &self.row.error_kind {
            return Some(kind.clone());
        }
        if self.cancel_token.is_cancelled() {
            return Some("cancelled".to_owned());
        }
        match self.row.status_code {
            Some(code) if !(200..300).contains(&code) => Some(format!("http_{code}")),
            Some(_) => Some("stream".to_owned()),
            None => Some("connect".to_owned()),
        }
    }
}

impl Drop for ProviderAttempt {
    fn drop(&mut self) {
        let finished_at = now_rfc3339();
        let latency_ms = i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX);
        let error_kind = self.derived_error_kind();
        let mut row = std::mem::take(&mut self.row);
        row.finished_at = Some(finished_at);
        row.latency_ms = Some(latency_ms);
        row.error_kind = error_kind;
        if self.capture_payloads {
            if let Some(payload) = self.payload.as_ref() {
                let (prepared, clipped) = crate::data_store::redact::prepare_payload(
                    payload,
                    crate::data_store::MAX_PROVIDER_PAYLOAD_BYTES,
                );
                row.request_payload_json = Some(prepared);
                row.payload_truncated |= clipped;
            }
            let (summary, clipped) = attempt_summary_json(
                &row,
                self.finish_reason.take(),
                self.error_message.as_deref(),
            );
            row.response_summary_json = Some(summary);
            row.payload_truncated |= clipped;
        }
        record_provider_attempt(row);
    }
}

/// Records one attempt off the turn's thread: opening the store, migrating it
/// and inserting must not stall a stream. Outside a runtime (a unit test or a
/// one-shot CLI path) it writes in place.
fn record_provider_attempt(row: crate::data_store::ProviderRequestRow) {
    let Some(path) = crate::data_store::data_path() else {
        return;
    };
    let write = move || {
        if let Err(error) = crate::data_store::record_provider_request_at(&path, &row) {
            crate::dbg_log!(
                "data store: provider request of {} not recorded: {error}",
                row.thread_id
            );
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let _ = handle.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The host and path of an endpoint, with the query and fragment dropped: a
/// gateway key or a signed URL can carry a credential there.
fn endpoint_parts(url: &str) -> (Option<String>, Option<String>) {
    match reqwest::Url::parse(url) {
        Ok(parsed) => (
            parsed.host_str().map(str::to_owned),
            Some(parsed.path().to_owned()),
        ),
        Err(_) => (None, None),
    }
}

/// The provider a row names: the stored credential's provider
/// (`openai`, `claude`, `github-copilot`), else the profile's engine, else the
/// profile's own name.
fn provider_label(profile: Option<&crate::config::ModelProfile>) -> Option<String> {
    let profile = profile?;
    profile
        .credential
        .as_ref()
        .map(|credential| credential.provider.clone())
        .or_else(|| {
            profile
                .engine
                .clone()
                .filter(|engine| !engine.trim().is_empty())
        })
        .or_else(|| Some(profile.name.clone()).filter(|name| !name.trim().is_empty()))
}

/// A small summary of what an attempt's response was: its status, the finish
/// reason of a stream that ended, how many events it carried, and — clipped
/// and with any bearer token removed — the provider's own error message.
///
/// Never the body: a successful SSE stream is megabytes of model output, and
/// an error body can echo the request. The second value says the error text
/// was clipped.
fn attempt_summary_json(
    row: &crate::data_store::ProviderRequestRow,
    finish_reason: Option<String>,
    error_message: Option<&str>,
) -> (String, bool) {
    let mut summary = serde_json::Map::new();
    if let Some(code) = row.status_code {
        summary.insert("status".to_owned(), serde_json::json!(code));
    }
    if let Some(reason) = finish_reason.filter(|reason| !reason.trim().is_empty()) {
        summary.insert("finish_reason".to_owned(), serde_json::json!(reason));
    }
    if let Some(events) = row.stream_event_count {
        summary.insert("events".to_owned(), serde_json::json!(events));
    }
    let mut clipped = false;
    if let Some(message) = error_message.map(str::trim).filter(|text| !text.is_empty()) {
        let redacted = crate::data_store::redact::redact_bearer(message);
        let end = redacted.floor_char_boundary(redacted.len().min(MAX_ATTEMPT_ERROR_BYTES));
        clipped = end < redacted.len();
        summary.insert("error".to_owned(), serde_json::json!(&redacted[..end]));
    }
    (serde_json::Value::Object(summary).to_string(), clipped)
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
    stream_request_with_timeouts(
        client,
        state,
        cancel_token,
        url,
        model,
        messages,
        buffer,
        quiet,
        allow_tools,
        thinking_mode,
        schema_policy,
        expected_session_id,
        tool_output_limit_override,
        retry::FIRST_EVENT_TIMEOUT,
        retry::STREAM_IDLE_TIMEOUT,
        None,
        false,
    )
    .await
}

/// [`stream_request`] on the named profile rather than the session's own.
/// A subagent runs on a profile it resolved by name at spawn; picking the
/// profile again from `url` and `model` would be ambiguous when several
/// profiles serve one model at one URL (#2334).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_request_for_profile(
    client: &reqwest::Client,
    state: Arc<Mutex<AppState>>,
    cancel_token: tokio_util::sync::CancellationToken,
    profile_name: Option<&str>,
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
    stream_request_with_timeouts(
        client,
        state,
        cancel_token,
        url,
        model,
        messages,
        buffer,
        quiet,
        allow_tools,
        thinking_mode,
        schema_policy,
        expected_session_id,
        tool_output_limit_override,
        retry::FIRST_EVENT_TIMEOUT,
        retry::STREAM_IDLE_TIMEOUT,
        profile_name,
        true,
    )
    .await
}

fn resolve_request_session_id(
    expected_session_id: Option<&str>,
    active_session_id: &str,
) -> String {
    expected_session_id
        .filter(|session_id| !session_id.is_empty())
        .unwrap_or(active_session_id)
        .to_owned()
}

#[allow(clippy::too_many_arguments)]
/// Keep the newest subscription quota windows for `/usage` and mirror the
/// short window into the footer quota indicator.
async fn record_provider_rate_limits(
    state: &Arc<Mutex<AppState>>,
    limits: crate::provider_auth::ProviderRateLimits,
) {
    let mut state = state.lock().await;
    if let Some(primary) = &limits.primary {
        state.model_quota_remaining = Some((100.0 - primary.used_percent) as f32);
    }
    state.provider_rate_limits = Some(limits);
    state.request_redraw();
}

async fn stream_request_with_timeouts(
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
    first_event_timeout: std::time::Duration,
    stream_idle_timeout: std::time::Duration,
    selected_profile: Option<&str>,
    subagent: bool,
) -> Result<Option<String>, StreamFailure> {
    let request_started = std::time::Instant::now();
    let (profile, profile_error, active_session_id, fast_mode) = {
        let app = state.lock().await;
        let selected_name = selected_profile.unwrap_or(app.config.default.big());
        let (profile, error) =
            match select_request_profile(&app.config.models, selected_name, url, model) {
                Ok(profile) => (profile, None),
                Err(error) => (None, Some(error)),
            };
        (profile, error, app.active_session_id.clone(), app.fast_mode)
    };
    if let Some(detail) = profile_error {
        return Err(StreamFailure {
            kind: StreamFailureKind::ProviderError,
            status: None,
            detail: Some(detail),
            bytes_received: 0,
            events_received: 0,
            partial_event_bytes: 0,
        });
    }
    let request_session_id = resolve_request_session_id(expected_session_id, &active_session_id);
    macro_rules! request_event {
        ($event:expr, $fields:expr $(,)?) => {{
            let mut fields = $fields;
            if let Some(object) = fields.as_object_mut() {
                object.insert(
                    "session_id".to_owned(),
                    serde_json::Value::String(request_session_id.clone()),
                );
            }
            crate::logger::operational_event($event, fields);
        }};
    }
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
    let messages_api = matches!(api_protocol, crate::config::ApiProtocol::AnthropicMessages);
    let copilot = profile
        .as_ref()
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(crate::provider_auth::CredentialRef::is_copilot);
    let chatgpt_plan = is_chatgpt_credential(profile.as_ref());

    if chatgpt_plan && !profile.as_ref().is_some_and(chatgpt_endpoint_is_supported) {
        return Err(StreamFailure {
            kind: StreamFailureKind::ProviderError,
            status: None,
            detail: Some(
                "ChatGPT plan credentials require the public https://api.openai.com/v1/responses endpoint and the Responses protocol; request stopped before sending credentials.".to_owned(),
            ),
            bytes_received: 0,
            events_received: 0,
            partial_event_bytes: 0,
        });
    }
    let aligned_messages = if messages_api {
        messages
    } else {
        align_alternating_messages(messages)
    };
    let message_count = aligned_messages.len();
    let (tool_protocol, agent_mode, workspace_root) = {
        let s = state.lock().await;
        (
            s.active_tool_protocol(),
            s.agent_mode,
            request_workspace_root(&s),
        )
    };
    if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative)
        && let Err(diagnostic) =
            crate::network::history::validate_native_tool_messages(&aligned_messages)
    {
        request_event!(
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
    let mut output_token_limit =
        request_output_token_limit(profile.as_ref(), allow_tools, thinking_mode, max_tokens);
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
        profile.as_ref().and_then(client_thinking_budget)
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
                outside_table: text_surface.outside_table,
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
            request_event!(
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
    let mut payload = if chatgpt_plan {
        chatgpt_stream_payload(
            model,
            &aligned_messages,
            &native_tool_schemas,
            profile.as_ref(),
            thinking_mode,
        )
    } else if messages_api {
        crate::network::anthropic_messages::request_payload(
            model,
            &aligned_messages,
            &native_tool_schemas,
            output_token_limit.unwrap_or(max_tokens),
            true,
        )
        .map_err(|error| StreamFailure {
            kind: StreamFailureKind::ProviderError,
            status: None,
            detail: Some(format!("Messages history could not be serialized: {error}")),
            bytes_received: 0,
            events_received: 0,
            partial_event_bytes: 0,
        })?
    } else if responses_api {
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
        bound_wire_thinking_budget(&mut payload, output_token_limit);
        apply_profile_sampling_options(&mut payload, profile.as_ref());

        // Only send `frequency_penalty` when the profile sets it: backends
        // such as Splash reject the historical injected 0.3 default as an
        // unsupported output transformation.
        apply_frequency_penalty(&mut payload, profile.as_ref(), url);
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
        if !responses_api && !messages_api {
            apply_api_native_tools(&mut payload, native_tool_schemas.clone(), allow_tools);
        }
        request_event!(
            "mcp.native_schema_selection",
            serde_json::json!({
                "available": mcp_selection.available,
                "selected": mcp_selection.selected,
                "requested": mcp_selection.requested,
                "relevant": mcp_selection.relevant,
                "previously_used": mcp_selection.previously_used,
                "admitted_by": mcp_selection.admitted_by,
                "fallback": mcp_selection.fallback,
                "omitted": mcp_selection.omitted,
                "omitted_names": mcp_selection.omitted_names,
                "reserved_servers": mcp_selection.reserved_servers,
                "rejected_reservations": mcp_selection.rejected_reservations,
                "selected_names": mcp_selection.selected_names,
                "max": crate::tools::MAX_MCP_NATIVE_SCHEMAS,
                "mcp_schema_bytes": mcp_selection.mcp_schema_bytes,
                "mcp_schema_budget_bytes": mcp_selection.mcp_schema_budget_bytes,
                "schema_budget_exhausted": mcp_selection.schema_budget_exhausted,
                "allow_tools": allow_tools,
                "phase": format!("{:?}", mcp_selection.phase),
                "builtin_available": mcp_selection.builtin_available,
                "builtin_selected": mcp_selection.builtin_selected,
                "withheld_builtin_names": mcp_selection.withheld_builtin_names,
            }),
        );
    }

    if !chatgpt_plan {
        apply_provider_parallel_tool_call_policy(&mut payload, api_protocol, allow_tools);
    }
    apply_openrouter_session_affinity(&mut payload, url, expected_session_id);
    if chatgpt_plan {
        payload["prompt_cache_key"] = serde_json::json!(request_session_id);
    }
    // Quiet requests are side requests unless a subagent makes them.
    apply_fast_service_tier(&mut payload, chatgpt_plan, fast_mode, !quiet || subagent);
    let requested_service_tier = payload.get("service_tier").cloned();

    // Count the schemas selected for the request, not one payload field: a
    // ChatGPT-plan request carries them in an input item, and reading
    // `tools` reported zero for every such request.
    let tool_count = if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) {
        if allow_tools {
            native_tool_schemas.len()
        } else {
            0
        }
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
    buffer.lock().await.performance.schema_us = crate::benchmark::elapsed_us(request_started);
    let serialization_started = std::time::Instant::now();
    let payload_bytes = serde_json::to_vec(&payload).map_err(|error| StreamFailure {
        kind: StreamFailureKind::ProviderError,
        status: None,
        detail: Some(format!("failed to serialize request payload: {error}")),
        bytes_received: 0,
        events_received: 0,
        partial_event_bytes: 0,
    })?;
    buffer.lock().await.performance.serialization_us =
        crate::benchmark::elapsed_us(serialization_started);
    let payload_byte_count = payload_bytes.len();
    let (verbose_network_logging, capture_provider_payloads, attempt_turn_id, attempt_round) = {
        let app = state.lock().await;
        (
            app.config.debug_verbose_network_logging,
            app.config.capture_provider_payloads,
            app.active_turn_id.clone(),
            app.provider_request_round,
        )
    };
    crate::dbg_log_for_session!(
        request_session_id,
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
    let copilot_headers = copilot.then(|| {
        crate::provider_auth::github_copilot::request_headers(&payload, expected_session_id)
    });
    let request_start_time = std::time::Instant::now();
    // The side of a prompt-cache miss cannot be told from token counts alone:
    // record how much of this request repeats the previous one's leading items.
    request_event!(
        "provider.request_prefix",
        request_prefix_report(&request_session_id, &payload),
    );

    // Captured only on request: a turn must not hold a second copy of a
    // multi-megabyte payload that nobody asked to keep.
    let captured_request_payload = capture_provider_payloads.then(|| payload.clone());
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
        request_event!(
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

    request_event!(
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

    request_event!(
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

    let api_key = if let Some(profile) = profile.as_ref() {
        let credential = tokio::select! {
            _ = cancel_token.cancelled() => return Err(StreamFailure::new(StreamFailureKind::Cancelled)),
            result = tokio::time::timeout(
                // Long enough to answer an operating system password prompt
                // for the credential store; 30 seconds failed the turn while
                // the prompt was still open. A detached owner cannot show the
                // prompt and fails within seconds instead (#2021).
                crate::provider_auth::credential_resolution_timeout(),
                crate::provider_auth::resolve_profile_credential(profile),
            ) => match result {
                Err(_) => return Err(StreamFailure {
                    kind: StreamFailureKind::ProviderError,
                    status: None,
                    detail: Some(if crate::provider_auth::credential_prompts_allowed() {
                        "credential resolution timed out before the provider request was sent; if the operating system asked for a password to unlock the credential store, answer it and retry"
                    } else {
                        "credential resolution timed out before the provider request was sent; this background session cannot answer an operating system password prompt, so open `rustcode` in a terminal once or sign in again with /login, then retry"
                    }.to_owned()),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                }),
                Ok(Err(error)) => return Err(StreamFailure {
                    kind: StreamFailureKind::ProviderError,
                    status: None,
                    detail: Some(format!("credential resolution failed before the provider request was sent: {error:#}")),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                }),
                Ok(Ok(key)) => key,
            }
        };
        if profile.credential.is_some() && credential.is_none() {
            return Err(StreamFailure {
                kind: StreamFailureKind::ProviderError,
                status: None,
                detail: Some(
                    "the configured provider credential could not be resolved; no request was sent"
                        .to_owned(),
                ),
                bytes_received: 0,
                events_received: 0,
                partial_event_bytes: 0,
            });
        }
        credential
    } else {
        None
    };
    let chatgpt_client = if chatgpt_plan || copilot {
        Some(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| StreamFailure {
                    kind: StreamFailureKind::ProviderError,
                    status: None,
                    detail: Some(format!(
                        "could not initialize the protected ChatGPT transport: {error}"
                    )),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                })?,
        )
    } else {
        None
    };
    let (trace_session_id, assistant_turn) = {
        let s = state.lock().await;
        let assistant_turn = s
            .history
            .iter()
            .filter(|message| message.role == "assistant")
            .count()
            + 1;
        (request_session_id.clone(), assistant_turn)
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
    let chatgpt_turn_state = if chatgpt_plan {
        state.lock().await.provider_turn_state.clone()
    } else {
        None
    };
    let mut parallel_tool_calls_fallback_attempted = false;
    let mut attempt = 0usize;
    // The endpoint without its query: that is where a key travels, and the
    // row must be readable.
    let (attempt_endpoint_host, attempt_endpoint_path) = endpoint_parts(&resolved_url);
    let mut attempt_context = ProviderAttemptContext {
        thread_id: request_session_id.clone(),
        turn_id: attempt_turn_id,
        round: attempt_round,
        provider: provider_label(profile.as_ref()),
        model: model.to_owned(),
        endpoint_host: attempt_endpoint_host,
        endpoint_path: attempt_endpoint_path,
        capture_payloads: capture_provider_payloads,
        payload: captured_request_payload,
    };
    // The attempt in flight: a row is written when the guard is dropped, so
    // every exit from the loop below records exactly one attempt.
    let mut attempt_recorder: Option<ProviderAttempt>;
    let stream = loop {
        if cancel_token.is_cancelled() {
            return Err(StreamFailure::new(StreamFailureKind::Cancelled));
        }
        let request_client = chatgpt_client.as_ref().unwrap_or(client);
        let mut req = request_client
            .post(&resolved_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_payload_bytes.clone());
        if let Some(headers) = &copilot_headers {
            req = req.headers(headers.clone());
        }
        if messages_api {
            req = req.header("anthropic-version", "2023-06-01");
        }
        if chatgpt_plan {
            // The session identity the Codex client sends with every request.
            for name in ["session-id", "thread-id", "x-client-request-id"] {
                req = req.header(name, request_session_id.as_str());
            }
            if let Some(turn_state) = chatgpt_turn_state.as_deref() {
                req = req.header(crate::network::CHATGPT_TURN_STATE_HEADER, turn_state);
            }
        }
        if let Some(ref key) = api_key {
            req = apply_api_key_headers(
                req,
                key,
                !chatgpt_plan
                    && !copilot
                    && profile.as_ref().is_some_and(|p| p.send_x_api_key_header()),
            );
        }
        // One row per actual send, retries included.
        attempt_recorder = Some(ProviderAttempt::start(
            &attempt_context,
            attempt + 1,
            request_payload_bytes.len(),
            &cancel_token,
        ));
        let send_result = retry::race_cancellable(
            tokio::time::timeout(retry::header_timeout_for_attempt(attempt), req.send()),
            &cancel_token,
        )
        .await;
        let send_result = match send_result {
            None => return Err(StreamFailure::new(StreamFailureKind::Cancelled)),
            Some(Err(_elapsed)) => {
                if let Some(recorder) = attempt_recorder.as_mut() {
                    recorder.fail("timeout");
                }
                // Nothing was received, so replaying is safe on every
                // profile, plan logins included; the next attempt waits
                // longer for a slow cold prompt (#2669).
                if attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    crate::dbg_log_for_session!(
                        request_session_id,
                        "stream_request: timed out waiting for response headers (attempt {}/{}), backing off {}ms",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    // End the attempt before the backoff: its latency is the
                    // attempt's own, not the wait that follows it.
                    drop(attempt_recorder.take());
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
                if let Some(recorder) = attempt_recorder.as_mut() {
                    recorder.headers(resp.status().as_u16());
                }
                stream_trace.response_headers(resp.status().as_u16());
                crate::dbg_log_for_session!(
                    request_session_id,
                    "stream_request: Received response status: {}",
                    resp.status()
                );
                request_event!(
                    "provider.response_headers",
                    serde_json::json!({
                        "model": model,
                        "status": resp.status().as_u16(),
                        "elapsed_ms": request_start_time.elapsed().as_millis() as u64,
                        "rate_limit_headers":
                            crate::provider_auth::rate_limit_header_pairs(resp.headers()),
                    }),
                );
                if chatgpt_plan
                    && let Some(limits) = crate::provider_auth::ProviderRateLimits::from_headers(
                        resp.headers(),
                        chrono::Utc::now().timestamp(),
                    )
                {
                    record_provider_rate_limits(&state, limits).await;
                }
                // Keep the first token of the turn, as the Codex client does:
                // the backend answers every request with a new one, and only
                // replaying the first keeps the turn on one route.
                if chatgpt_plan
                    && chatgpt_turn_state.is_none()
                    && let Some(turn_state) = resp
                        .headers()
                        .get(crate::network::CHATGPT_TURN_STATE_HEADER)
                        .and_then(|value| value.to_str().ok())
                {
                    let mut s = state.lock().await;
                    if s.provider_turn_state.is_none() {
                        s.provider_turn_state = Some(turn_state.to_owned());
                    }
                    request_event!(
                        "provider.turn_state",
                        serde_json::json!({ "outcome": "stored", "bytes": turn_state.len() }),
                    );
                }
                break resp
                    .bytes_stream()
                    .map(|r| r.map_err(std::io::Error::other))
                    .boxed();
            }
            Ok(resp) => {
                let status = resp.status();
                let code = status.as_u16();
                if let Some(recorder) = attempt_recorder.as_mut() {
                    recorder.headers(code);
                }
                stream_trace.response_headers(code);
                let err_body = match retry::race_cancellable(resp.text(), &cancel_token).await {
                    None => return Err(StreamFailure::new(StreamFailureKind::Cancelled)),
                    Some(body) => body.unwrap_or_default(),
                };
                if let Some(recorder) = attempt_recorder.as_mut() {
                    recorder.progress(0, err_body.len());
                    recorder.provider_message(&err_body);
                }
                if !parallel_tool_calls_fallback_attempted
                    && let Some(fallback_payload_bytes) =
                        parallel_tool_calls_fallback_payload_bytes.as_ref()
                    && is_parallel_tool_calls_rejection(code, &err_body)
                {
                    crate::dbg_log_for_session!(
                        request_session_id,
                        "stream_request: provider rejected parallel_tool_calls; retrying without optional field"
                    );
                    request_event!(
                        "provider.parallel_tool_calls_fallback",
                        serde_json::json!({
                            "model": model,
                            "status": code,
                            "action": "retry_without_optional_field",
                        }),
                    );
                    request_payload_bytes = fallback_payload_bytes.clone();
                    // The captured body must be the one actually sent.
                    if let Some(object) = attempt_context
                        .payload
                        .as_mut()
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        object.remove("parallel_tool_calls");
                    }
                    parallel_tool_calls_fallback_attempted = true;
                    drop(attempt_recorder.take());
                    continue;
                }
                if !chatgpt_plan && retry::is_retryable_status(code) && attempt < retry::MAX_RETRIES
                {
                    let delay = retry::delay_for_attempt(attempt, code);
                    crate::dbg_log_for_session!(
                        request_session_id,
                        "stream_request: retryable status {} (attempt {}/{}), backing off {}ms",
                        status,
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis()
                    );
                    drop(attempt_recorder.take());
                    if retry::race_cancellable(tokio::time::sleep(delay), &cancel_token)
                        .await
                        .is_none()
                    {
                        return Err(StreamFailure::new(StreamFailureKind::Cancelled));
                    }
                    attempt += 1;
                    continue;
                }
                crate::dbg_log_for_session!(
                    request_session_id,
                    "stream_request: Request failed with status {} (error_body_bytes={})",
                    status,
                    err_body.len()
                );
                return Err(StreamFailure {
                    kind: StreamFailureKind::ProviderError,
                    status: Some(code),
                    detail: Some(if copilot {
                        match code { 401 => "Copilot rejected the GitHub credential; reconnect with /login github-copilot new (refresh gh auth login first for CLI credentials).".into(), 403 => "Copilot access or this model is disabled; check your plan and organization policy, then /refresh github-copilot.".into(), 404 => "Copilot no longer offers this model endpoint; /refresh github-copilot and select an available model.".into(), _ => err_body }
                    } else {
                        err_body
                    }),
                    bytes_received: 0,
                    events_received: 0,
                    partial_event_bytes: 0,
                });
            }
            Err(e) => {
                if let Some(recorder) = attempt_recorder.as_mut() {
                    recorder.fail(if e.is_timeout() {
                        "timeout"
                    } else if e.is_connect() {
                        "connect"
                    } else {
                        "stream"
                    });
                    recorder.provider_message(&format!("{e}"));
                }
                // Also for ChatGPT plans: no response arrived, so nothing ran,
                // and one reset connection ended the whole turn (session
                // 01a16b37, "Connection reset by peer" after 16 rounds).
                if retry::is_retryable_transport(&e) && attempt < retry::MAX_RETRIES {
                    let delay = retry::delay_for_attempt(attempt, 0);
                    crate::dbg_log_for_session!(
                        request_session_id,
                        "stream_request: transient network error (attempt {}/{}), backing off {}ms: {}",
                        attempt + 1,
                        retry::MAX_RETRIES,
                        delay.as_millis(),
                        e
                    );
                    drop(attempt_recorder.take());
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

    let wrapped = StreamReader::new(stream);
    let mut reader = BufReader::with_capacity(4096, wrapped);
    let mut line_buf = String::with_capacity(4096);
    let mut in_reasoning = false;
    if !quiet {
        // Until this response reasons, nothing it says is a summary.
        let mut s = state.lock().await;
        if expected_session_id.is_none_or(|expected| s.active_session_id == expected) {
            s.history.set_reasoning_summary(false);
        }
    }
    let mut finish_reason: Option<String> = None;
    let mut responses_completed = false;
    let mut stream_bytes_received = 0usize;
    let mut stream_events_received = 0usize;

    let mut accumulators = ToolAccumulatorSet::default();
    let mut response_call_ids = HashMap::new();
    let mut response_argument_deltas = HashSet::new();
    let mut messages_stream = crate::network::anthropic_messages::MessagesStream::default();
    let mut tool_argument_limit_reached = false;
    let mut reasoning_detector = super::loop_detect::ReasoningLoopDetector::default();

    crate::dbg_log_for_session!(
        request_session_id,
        "stream_request: Starting SSE stream read loop"
    );
    // Absolute progress deadlines anchored at headers. Keep-alive blank /
    // comment lines carry no meaningful event and must not extend them: the
    // per-fill timeout inside `read_sse_line_with_state` still tolerates a
    // slow partial line, but these bounds fire even while the wire stays
    // active. Without them a provider (or proxy) emitting periodic blank
    // lines could stall the first event forever.
    let stream_start = tokio::time::Instant::now();
    let first_event_deadline = stream_start + first_event_timeout;
    let mut last_progress = stream_start;
    loop {
        if cancel_token.is_cancelled() {
            crate::dbg_log_for_session!(
                request_session_id,
                "stream_request: Stream reading cancelled via token (events={stream_events_received}, bytes={stream_bytes_received})"
            );
            return Err(StreamFailure {
                kind: StreamFailureKind::Cancelled,
                status: None,
                detail: Some(format!(
                    "SSE stream cancelled with {stream_events_received} events after {stream_bytes_received} bytes"
                )),
                bytes_received: stream_bytes_received,
                events_received: stream_events_received,
                partial_event_bytes: line_buf.len(),
            });
        }

        // Absolute deadline for this read (see `sse_progress_deadline`):
        // keep-alive blank/comment lines never move these markers.
        let absolute_deadline = sse_progress_deadline_with_timeouts(
            first_event_deadline,
            last_progress,
            stream_events_received,
            stream_idle_timeout,
        );

        tokio::select! {
            r = tokio::time::timeout_at(
                absolute_deadline,
                read_sse_line_with_state(
                    &mut reader,
                    &mut line_buf,
                    stream_events_received == 0,
                    if stream_events_received == 0 {
                        first_event_timeout
                    } else {
                        stream_idle_timeout
                    },
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
                        crate::dbg_log_for_session!(request_session_id,
                            "stream_request: SSE absolute progress deadline elapsed ({kind}, events={stream_events_received}, bytes={stream_bytes_received})"
                        );
                        request_event!(
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
                        crate::dbg_log_for_session!(request_session_id,"stream_request: SSE stream read EOF (0 bytes)");
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
                        if let Some(recorder) = attempt_recorder.as_mut() {
                            recorder.progress(stream_events_received, stream_bytes_received);
                        }
                        let trimmed = line_buf.trim();
                        if trimmed == "data: [DONE]" {
                            if (responses_api && !responses_completed) || (messages_api && !messages_stream.completed) {
                                return Err(StreamFailure {
                                    kind: StreamFailureKind::PrematureEof,
                                    status: None,
                                    detail: Some(
                                        "Provider SSE stream ended with [DONE] before its completion event"
                                            .to_owned(),
                                    ),
                                    bytes_received: stream_bytes_received,
                                    events_received: stream_events_received,
                                    partial_event_bytes: 0,
                                });
                            }
                            buffer.lock().await.termination =
                                Some(StreamTermination::ProviderStop);
                            line_buf.clear();
                            break;
                        }
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                                stream_events_received += 1;
                                if let Some(recorder) = attempt_recorder.as_mut() {
                                    recorder.progress(stream_events_received, stream_bytes_received);
                                }
                                let generated_delta = value.pointer("/choices/0/delta").is_some_and(|delta| {
                                    ["content", "reasoning_content", "reasoning"].iter().any(|key| delta.get(key).and_then(serde_json::Value::as_str).is_some_and(|text| !text.is_empty()))
                                        || delta.get("tool_calls").and_then(serde_json::Value::as_array).is_some_and(|calls| !calls.is_empty())
                                }) || value.get("type").and_then(serde_json::Value::as_str).is_some_and(|kind| kind.ends_with(".delta"))
                                    && value.get("delta").is_some_and(|delta| delta.as_str().is_none_or(|text| !text.is_empty()));
                                if generated_delta {
                                    buffer.lock().await.performance.ttft_us.get_or_insert_with(|| crate::benchmark::elapsed_us(request_started));
                                }
                                // Only meaningful events move the progress
                                // markers: keep-alive blank/comment lines and
                                // unparsable payloads must not extend the
                                // absolute first-event / idle budgets.
                                last_progress = tokio::time::Instant::now();
                                stream_trace.record(line_buf.len(), &value);
                                if responses_api
                                    && value.get("type").and_then(serde_json::Value::as_str)
                                        .is_some_and(|kind| matches!(kind, "response.completed" | "response.incomplete"))
                                {
                                    responses_completed = true;
                                    // The tier the provider served, next to the one asked
                                    // for, so fast mode can be checked from the log (#2582).
                                    if let Some(served) = served_service_tier(&value) {
                                        request_event!(
                                            "provider.service_tier",
                                            serde_json::json!({
                                                "requested": requested_service_tier,
                                                "served": served,
                                            }),
                                        );
                                        state.lock().await.note_served_service_tier(requested_service_tier.as_ref(), served);
                                    }
                                }
                                if chatgpt_plan
                                    && let Some(limits) = value
                                        .get("rate_limits")
                                        .or_else(|| value.pointer("/response/rate_limits"))
                                        .and_then(|rate_limits| {
                                            crate::provider_auth::ProviderRateLimits::from_json(
                                                rate_limits,
                                                chrono::Utc::now().timestamp(),
                                            )
                                        })
                                {
                                    record_provider_rate_limits(&state, limits).await;
                                }
                                if responses_api
                                    && matches!(
                                        value.get("type").and_then(|v| v.as_str()),
                                        Some("response.failed" | "error")
                                    )
                                {
                                    let mut raw = value.clone();
                                    if let Some(response) = raw.get_mut("response").and_then(|r| r.as_object_mut()) {
                                        response.retain(|key, _| matches!(key.as_str(), "error" | "status" | "incomplete_details" | "id"));
                                    }
                                    crate::dbg_log_for_session!(request_session_id, "stream_request: provider failure event: {raw}");
                                }
                                let val = if responses_api {
                                    normalize_responses_event(
                                        &value,
                                        &mut response_call_ids,
                                        &mut response_argument_deltas,
                                    )
                                    .unwrap_or_else(|| serde_json::json!({}))
                                } else if messages_api {
                                    messages_stream.normalize(&value).unwrap_or_else(|| serde_json::json!({}))
                                } else {
                                    value
                                };
                                if let Some(error) = val.get("error") {
                                    let status = error
                                        .get("status_code")
                                        .and_then(|value| value.as_u64())
                                        .and_then(|value| u16::try_from(value).ok());
                                    let message = error
                                        .get("message")
                                        .and_then(|value| value.as_str())
                                        .map(str::to_owned)
                                        .unwrap_or_else(|| error.to_string());
                                    let code = error.get("code").and_then(|value| value.as_str());
                                    let detail = match code {
                                        Some(code @ "subscription_sharing_usage_limit_exceeded") => format!(
                                            "ChatGPT plan usage limit reached ({code}): {message}. Wait for the account limit to reset or select another eligible ChatGPT account. RustCode did not retry with an API key."
                                        ),
                                        Some(code @ "subscription_sharing_usage_unavailable") => format!(
                                            "ChatGPT plan usage is unavailable ({code}): {message}. Check the selected account's plan usage permission and sign in again if needed. RustCode did not retry with an API key."
                                        ),
                                        Some("response_incomplete") => format!(
                                            "ChatGPT Responses request ended without a completed response: {message}. Review the response limit and retry the request."
                                        ),
                                        Some(code) => format!("{message} ({code})"),
                                        None => message,
                                    };
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
                                         let reasoning_flag = |key: &str| {
                                             delta
                                                 .and_then(|d| d.get(key))
                                                 .and_then(serde_json::Value::as_bool)
                                                 .unwrap_or(false)
                                         };
                                         let reasoning_is_summary = reasoning_flag("reasoning_summary");
                                         // A paragraph break between summary parts means
                                         // nothing before any reasoning text.
                                         let reasoning = reasoning.filter(|_| {
                                             in_reasoning || !reasoning_flag("reasoning_separator")
                                         });
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
                                                    s.history.set_reasoning_summary(reasoning_is_summary);
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
                                                crate::dbg_log_for_session!(request_session_id,
                                                    "stream_request: reasoning loop detected ({reason}) — stopping stream cleanly"
                                                );
                                                request_event!(
                                                    "stream.reasoning_loop_cut",
                                                    serde_json::json!({ "reason": reason }),
                                                );
                                                finish_reason = Some("reasoning_loop".to_string());
                                                reasoning_loop_cut = true;
                                            }
                                            if bounded.budget_exhausted {
                                                crate::dbg_log_for_session!(request_session_id,
                                                    "stream_request: client reasoning budget reached ({} estimated tokens) — stopping stream cleanly",
                                                    thinking_budget.unwrap_or_default()
                                                );
                                                request_event!(
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
                                            if reasoning.is_none() {
                                                // Some providers omit explicit reasoning deltas.
                                                // Their terminal prose response is still a closed
                                                // final-answer boundary.
                                                buffer.final_answer_boundary =
                                                    super::stream::FinalAnswerBoundary::ReasoningClosed;
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
                                if let Some(usage) = val.get("usage") {
                                    let mut stream = buffer.lock().await;
                                    stream.performance.input_tokens = usage.get("prompt_tokens").and_then(serde_json::Value::as_u64);
                                    stream.performance.output_tokens = usage.get("completion_tokens").and_then(serde_json::Value::as_u64);
                                    stream.performance.cached_input_tokens = cache_usage_metrics(usage).0.map(u64::from);
                                }
                                // A subagent request is quiet but still logs its
                                // completion so its cost and cache use are visible
                                // (#2504); only the session's own request updates
                                // its token usage.
                                if let Some(usage) = val.get("usage").filter(|_| !quiet || subagent)
                                    && let (Some(p), Some(c), Some(t)) = (
                                        usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                        usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                        usage.get("total_tokens").and_then(|v| v.as_u64()),
                                    ) {
                                        let (cached, cache_write_tokens, cache_discount) =
                                            cache_usage_metrics(usage);
                                        // The tokens this response reported, kept
                                        // on the attempt's row.
                                        if let Some(recorder) = attempt_recorder.as_mut() {
                                            recorder.usage(p, c, cached.map(u64::from));
                                        }
                                        let (provider_cache_status, provider_cache_reason) =
                                            provider_cache_observation(
                                                is_openrouter_endpoint(url),
                                                bounded_openrouter_session_id(
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

                                        if !quiet {
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
                                        }

                                        let estimation_delta = if estimated_prompt_tokens > 0 {
                                            Some((p as i64) - (estimated_prompt_tokens as i64))
                                        } else {
                                            None
                                        };
                                        let estimation_delta_percent =
                                            prompt_estimation_delta_percent(estimated_prompt_tokens, p);

                                        request_event!(
                                            "provider.completion",
                                            serde_json::json!({
                                                "model": model,
                                                "subagent": subagent,
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
                                crate::dbg_log_for_session!(request_session_id,
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
                        crate::dbg_log_for_session!(request_session_id,"stream_request: SSE read error: {}", e);
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
                crate::dbg_log_for_session!(request_session_id,"stream_request: Cancelled via select branch (events={stream_events_received}, bytes={stream_bytes_received})");
                return Err(StreamFailure {
                    kind: StreamFailureKind::Cancelled,
                    status: None,
                    detail: Some(format!(
                        "SSE stream cancelled with {stream_events_received} events after {stream_bytes_received} bytes"
                    )),
                    bytes_received: stream_bytes_received,
                    events_received: stream_events_received,
                    partial_event_bytes: line_buf.len(),
                });
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
        crate::dbg_log_for_session!(
            request_session_id,
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
    crate::dbg_log_for_session!(
        request_session_id,
        "stream_request: Stream request loop ended. Total content: {} chars",
        buf.content.len()
    );
    // The attempt that carried this stream ends here; a stream that stops
    // early has already been classified by its guard.
    if let Some(mut recorder) = attempt_recorder.take() {
        recorder.completed(finish_reason.clone());
    }
    Ok(finish_reason)
}

#[cfg(test)]
mod request_prefix_tests {
    use super::{request_prefix_comparison, request_prefix_units};
    use serde_json::json;

    #[test]
    fn a_request_that_extends_the_previous_one_shares_all_but_its_changed_tail() {
        let first = json!({"input": [
            {"role": "developer", "content": "rules"},
            {"type": "additional_tools", "tools": [{"name": "read_file"}]},
            {"role": "user", "content": "hello"},
            {"role": "user", "content": "<rustcode_context>one"},
        ]});
        let second = json!({"input": [
            {"role": "developer", "content": "rules"},
            {"type": "additional_tools", "tools": [{"name": "read_file"}]},
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "<rustcode_context>two"},
        ]});
        let (before, now) = (request_prefix_units(&first), request_prefix_units(&second));

        let cold = request_prefix_comparison(None, &before);
        assert_eq!(cold["units"], 4);
        assert!(cold["previous_units"].is_null());

        let report = request_prefix_comparison(Some(&before), &now);
        assert_eq!(report["shared_units"], 3);
        assert_eq!(report["previous_units"], 4);
        assert_eq!(report["first_changed"]["index"], 3);
        assert_eq!(report["first_changed"]["previous_kind"], "user");
        assert_eq!(report["first_changed"]["kind"], "assistant");
        let shared: usize = now[..3].iter().map(|unit| unit.1).sum();
        assert_eq!(report["shared_bytes"], shared);

        // A changed tool list is caught as the very first unit.
        let retooled = json!({"tools": [{"name": "grep"}], "messages": [{"role": "user", "content": "hello"}]});
        let original = json!({"tools": [{"name": "read_file"}], "messages": [{"role": "user", "content": "hello"}]});
        let report = request_prefix_comparison(
            Some(&request_prefix_units(&original)),
            &request_prefix_units(&retooled),
        );
        assert_eq!(report["shared_units"], 0);
        assert_eq!(report["first_changed"]["previous_kind"], "tools");

        let identical = request_prefix_comparison(Some(&before), &before);
        assert_eq!(identical["shared_units"], 4);
        assert!(identical["first_changed"].is_null());
    }
}
