use crate::app::{AppState, TokenUsage};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio_util::io::StreamReader;

use super::lifecycle::{StreamFailure, StreamFailureKind};
use super::retry;
use super::stream::StreamBuffer;
use super::{align_alternating_messages, count_tokens, parse_sse_line};

/// Tracks streamed tool-fence markers, including markers split across chunks.
#[derive(Default)]
struct ToolFenceCounter {
    seen: usize,
    tail: String,
}

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
    if thinking_mode == ThinkingMode::Disabled
        || profile.is_some_and(|p| p.enable_thinking == Some(false))
    {
        payload["enable_thinking"] = serde_json::json!(false);
        // oMLX exposes reasoning controls through OpenAI-compatible
        // `chat_template_kwargs`; keep the top-level field for servers that
        // implement the Qwen extension directly.
        payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(false);
        return;
    }
    if thinking_mode == ThinkingMode::BoundedRecovery {
        payload["enable_thinking"] = serde_json::json!(true);
        payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(true);
        payload["reasoning_effort"] = serde_json::json!("low");
        payload["thinking_budget"] = serde_json::json!(RECOVERY_THINKING_BUDGET);
        return;
    }
    if let Some(enable_thinking) = profile.and_then(|p| p.enable_thinking) {
        payload["enable_thinking"] = serde_json::json!(enable_thinking);
        payload["chat_template_kwargs"]["enable_thinking"] = serde_json::json!(enable_thinking);
    }
    if let Some(effort) = profile.and_then(|p| p.reasoning_effort.as_ref()) {
        payload["reasoning_effort"] = serde_json::json!(effort);
    }
    if let Some(thinking_budget) = profile.and_then(|p| p.thinking_budget) {
        payload["thinking_budget"] = serde_json::json!(thinking_budget);
    }
}

fn apply_profile_sampling_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
) {
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
    if let Some(preserve_thinking) = profile.and_then(|p| p.preserve_thinking) {
        payload["chat_template_kwargs"]["preserve_thinking"] = serde_json::json!(preserve_thinking);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeToolChoice {
    Auto,
    Required,
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
    choice: NativeToolChoice,
) {
    if allow_tools && !schema.is_empty() {
        payload["tools"] = serde_json::Value::Array(schema);
        payload["tool_choice"] = serde_json::json!(match choice {
            NativeToolChoice::Auto => "auto",
            NativeToolChoice::Required => "required",
        });
    }
}

const RECOVERY_THINKING_BUDGET: u32 = 1024;
const RECOVERY_MAX_TOKENS: u32 = 1024;
const MAX_NATIVE_TOOL_ARGUMENT_BYTES: usize = 40 * 1024;
const MAX_INVALID_ARGUMENT_PREVIEW_BYTES: usize = 1024;
const MAX_TOOL_CALL_INDEX: usize = 127;
const MAX_PROVIDER_TRACE_EVENTS: usize = 256;
const MAX_PROVIDER_TRACE_BYTES: usize = 64 * 1024;
const PROVIDER_TRACE_METADATA_RESERVE: usize = 4096;
const MAX_PROVIDER_TRACE_CHOICES_PER_EVENT: usize = 8;
const MAX_PROVIDER_TRACE_TOOL_CALLS_PER_CHOICE: usize = 64;

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
                "cached_tokens": usage.get("prompt_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
                    .and_then(|value| value.as_u64())
                    .or_else(|| number("cached_tokens")),
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
        let index = index.map(|index| index.min(MAX_TOOL_CALL_INDEX));
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
            acc.index = Some(index.min(MAX_TOOL_CALL_INDEX));
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
    fn profile_generation_options_include_hard_thinking_budget() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("low".to_string()),
            thinking_budget: Some(4096),
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
    fn recovery_keeps_thinking_on_with_a_small_one_request_budget() {
        let profile = crate::config::ModelProfile {
            enable_thinking: Some(true),
            reasoning_effort: Some("medium".to_string()),
            thinking_budget: Some(4096),
            ..crate::config::ModelProfile::default()
        };
        let mut payload = serde_json::json!({});

        apply_profile_generation_options(
            &mut payload,
            Some(&profile),
            ThinkingMode::BoundedRecovery,
        );

        assert_eq!(payload["enable_thinking"], true);
        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(payload["reasoning_effort"], "low");
        assert_eq!(payload["thinking_budget"], RECOVERY_THINKING_BUDGET);
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
    fn bounded_recovery_keeps_output_cap_separate_and_small() {
        let profile = crate::config::ModelProfile {
            context_window: Some(128_000),
            max_tokens: Some(16_000),
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

        apply_api_native_tools(&mut payload, schema, false, NativeToolChoice::Auto);

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

        apply_api_native_tools(&mut payload, schema, true, NativeToolChoice::Auto);

        assert_eq!(payload["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(payload["tool_choice"], "auto");
    }

    #[test]
    fn recovery_tool_requests_require_a_tool_call() {
        let mut payload = serde_json::json!({});
        let schema = vec![serde_json::json!({
            "type": "function",
            "function": {"name": "view_file"}
        })];

        apply_api_native_tools(&mut payload, schema, true, NativeToolChoice::Required);

        assert_eq!(payload["tool_choice"], "required");
    }

    #[test]
    fn recovery_thinking_budget_is_small_and_deterministic() {
        assert_eq!(RECOVERY_THINKING_BUDGET, 1024);
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
            Self::Io(error) if error.contains("invalid UTF-8") => StreamFailureKind::MalformedSse,
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

/// Backwards-compatible helper used by focused parser tests.  The streaming
/// request uses [`read_sse_line_with_state`] so the first meaningful event has
/// its own deadline.
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

/// Metadata-only summary of an outbound chat-completion request: round shape
/// and size, not content. This is what gets written to debug.log by default
/// in place of the full serialized payload (see `request_debug_log_line`).
pub(crate) fn request_log_summary(
    model: &str,
    message_count: usize,
    tool_count: usize,
    payload_bytes: usize,
) -> String {
    format!(
        "stream_request: sending model={model} messages={message_count} tools={tool_count} payload_bytes={payload_bytes}"
    )
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

fn invalid_argument_marker(raw: &str, error: impl Into<String>) -> serde_json::Value {
    let preview_end = raw.floor_char_boundary(raw.len().min(MAX_INVALID_ARGUMENT_PREVIEW_BYTES));
    serde_json::json!({
        "_invalid_arguments": {
            "original_bytes": raw.len(),
            "preview": &raw[..preview_end],
            "truncated": raw.len() > preview_end,
        },
        "_parse_error": error.into(),
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
        Ok(_) => invalid_argument_marker(raw, "tool arguments must be a JSON object"),
        Err(error) => invalid_argument_marker(raw, error.to_string()),
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
) -> Result<Option<String>, StreamFailure> {
    let aligned_messages = align_alternating_messages(messages);
    let message_count = aligned_messages.len();

    let profile = {
        state
            .lock()
            .await
            .config
            .models
            .iter()
            .find(|p| {
                (p.model == model || p.name == model) && (p.url == url || p.endpoint_url() == url)
            })
            .cloned()
    };
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
    let output_token_limit = profile
        .as_ref()
        .and_then(|p| {
            p.output_token_limit(
                allow_tools,
                thinking_mode == ThinkingMode::BoundedRecovery,
            )
        })
        .map(|limit| clamp_request_max_tokens(limit, thinking_mode))
        .or_else(|| {
            (allow_tools || thinking_mode == ThinkingMode::BoundedRecovery)
                .then_some(max_tokens)
        });
    // Recovery keeps reasoning enabled but applies a small client-side bound:
    // it exists to produce the next action, not another full planning turn.
    let thinking_budget = if thinking_mode == ThinkingMode::BoundedRecovery {
        Some(RECOVERY_THINKING_BUDGET)
    } else if thinking_mode == ThinkingMode::Disabled {
        None
    } else {
        profile.as_ref().and_then(|p| p.thinking_budget)
    };

    let (tool_protocol, native_tool_schemas, mcp_selection) = {
        let mut s = state.lock().await;
        let tool_protocol = s.active_tool_protocol();
        if matches!(tool_protocol, crate::config::ToolProtocol::ApiNative) && allow_tools {
            let session_id = s.active_session_id.clone();
            let workspace_root = s
                .workspace_root
                .clone()
                .or_else(|| std::env::current_dir().ok());
            let (schemas, selection) = s.prompt_cache.native_tool_schemas(
                schema_policy,
                &aligned_messages,
                &session_id,
                workspace_root.as_deref(),
            );
            (tool_protocol, schemas, selection)
        } else {
            (
                tool_protocol,
                Vec::new(),
                crate::tools::McpSchemaSelectionStats::default(),
            )
        }
    };
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
        let tool_choice = if thinking_mode == ThinkingMode::BoundedRecovery && allow_tools {
            NativeToolChoice::Required
        } else {
            NativeToolChoice::Auto
        };
        apply_api_native_tools(
            &mut payload,
            native_tool_schemas.clone(),
            allow_tools,
            tool_choice,
        );
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
                "allow_tools": allow_tools,
                "phase": format!("{:?}", mcp_selection.phase),
                "builtin_available": mcp_selection.builtin_available,
                "builtin_selected": mcp_selection.builtin_selected,
            }),
        );
    }

    let tool_count = payload
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
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
            &request_log_summary(model, message_count, tool_count, payload_byte_count),
            &payload,
        )
    );

    let request_start_time = std::time::Instant::now();
    let provider_messages = payload["messages"]
        .as_array()
        .expect("request messages were constructed as an array");
    let estimated_prompt_tokens =
        estimate_token_usage_with_tool_schemas(provider_messages, "", &native_tool_schemas)
            .await
            .map(|u| u.prompt_tokens)
            .unwrap_or(0);
    drop(payload);
    let tool_schema_tokens =
        crate::network::compaction::estimate_tool_schema_tokens(&native_tool_schemas);

    crate::logger::operational_event(
        "context.request_composition",
        serde_json::json!({
            "model": model,
            "messages": message_count,
            "tools": tool_count,
            "payload_bytes": payload_byte_count,
            "tool_schema_tokens": tool_schema_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "total_estimated_prompt_tokens": estimated_prompt_tokens,
            "output_token_field": output_token_limit.map(|_| output_token_field),
            "output_token_limit": output_token_limit,
            "output_token_limit_source":
                if profile.as_ref().is_some_and(|p| p.max_tokens.is_some()) {
                    "profile"
                } else if output_token_limit.is_some() {
                    "client_safety"
                } else {
                    "provider_default"
                },
            "soft_context_target": profile.as_ref().map(|p| p.context_budget().soft_context_target),
            "hard_effective_limit": profile.as_ref().map(|p| p.context_budget().hard_effective_limit),
            "context_window": profile.as_ref().map(|p| p.context_budget().context_window),
        }),
    );

    crate::logger::operational_event(
        "provider.request_start",
        serde_json::json!({
            "model": model,
            "messages": message_count,
            "tools": tool_count,
            "payload_bytes": payload_byte_count,
            "tool_schema_tokens": tool_schema_tokens,
            "estimated_prompt_tokens": estimated_prompt_tokens,
            "total_estimated_prompt_tokens": estimated_prompt_tokens,
            "tool_schema_phase": format!("{:?}", mcp_selection.phase),
            "builtin_tools": mcp_selection.builtin_selected,
            "available_builtin_tools": mcp_selection.builtin_available,
        }),
    );

    let resolved_url = {
        let trimmed = url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") || trimmed.ends_with("/chats/completion") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/chat/completions")
        }
    };

    let api_key = {
        let s = state.lock().await;
        s.config
            .models
            .iter()
            .find(|m| m.url == url || m.name == s.model_name || m.endpoint_url() == resolved_url)
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

    let mut attempt = 0usize;
    let response = loop {
        if cancel_token.is_cancelled() {
            return Err(StreamFailure::new(StreamFailureKind::Cancelled));
        }
        let mut req = client
            .post(&resolved_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(payload_bytes.clone());
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
    let mut tool_argument_limit_reached = false;
    let mut fences = ToolFenceCounter::default();
    let mut reasoning_detector = super::loop_detect::ReasoningLoopDetector::default();
    let runaway_limit = crate::tools::MAX_TOOL_CALLS_PER_RESPONSE;

    dbg_log!("stream_request: Starting SSE stream read loop");
    loop {
        if cancel_token.is_cancelled() {
            dbg_log!("stream_request: Stream reading cancelled via token");
            return Ok(None);
        }

        tokio::select! {
            r = read_sse_line_with_state(
                &mut reader,
                &mut line_buf,
                stream_events_received == 0,
            ) => {
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
                            line_buf.clear();
                            break;
                        }
                        if let Some(json_str) = parse_sse_line(trimmed) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                                stream_events_received += 1;
                                stream_trace.record(line_buf.len(), &val);
                                if let Some(choices) = val.get("choices").and_then(|c| c.as_array())
                                    && !choices.is_empty() {
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
                                                buffer.lock().await.thought_started_at = Some(started);
                                                if !quiet {
                                                    let mut s = state.lock().await;
                                                    if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                        return Ok(None);
                                                    }
                                                    s.current_thought_started_at = Some(started);
                                                }
                                                chunk.push_str("<think>\n");
                                            }
                                            let used_tokens = buffer.lock().await.thought_tokens;
                                            let bounded = bound_reasoning_chunk(
                                                r_token,
                                                used_tokens,
                                                thinking_budget,
                                            );
                                            let thought_tokens = bounded.estimated_tokens;
                                            {
                                                let mut buffer = buffer.lock().await;
                                                buffer.thought_tokens = buffer
                                                    .thought_tokens
                                                    .saturating_add(thought_tokens);
                                            }
                                            if !quiet {
                                                let mut s = state.lock().await;
                                                if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                                    return Ok(None);
                                                }
                                                s.current_thought_tokens = s
                                                    .current_thought_tokens
                                                    .saturating_add(thought_tokens);
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
                                                        "estimated_thought_tokens": used_tokens.saturating_add(thought_tokens),
                                                    }),
                                                );
                                                finish_reason = Some("reasoning_budget".to_string());
                                                reasoning_budget_cut = true;
                                            }
                                        } else if let Some(c_token) = content {
                                            if in_reasoning {
                                                in_reasoning = false;
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
                                                chunk.push_str("\n</think>\n\n");
                                            }
                                            chunk.push_str(c_token);
                                        }
                                        let runaway = fences.push(&chunk) > runaway_limit
                                            || accumulators
                                                .calls
                                                .iter()
                                                .filter(|acc| !acc.name.is_empty())
                                                .count()
                                                > runaway_limit;
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
                                                }
                                            }
                                        }
                                        if reasoning_loop_cut || reasoning_budget_cut {
                                            line_buf.clear();
                                            break;
                                        }
                                        if runaway {
                                            dbg_log!(
                                                "stream_request: past {} tool calls in one response — cutting the stream",
                                                runaway_limit
                                            );
                                            crate::logger::operational_event(
                                                "stream.runaway_cut",
                                                serde_json::json!({ "limit": runaway_limit }),
                                            );
                                            accumulators.calls.truncate(runaway_limit);
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
                                        let cached = usage.get("prompt_tokens_details")
                                            .and_then(|details| details.get("cached_tokens"))
                                            .and_then(|v| v.as_u64())
                                            .or_else(|| usage.get("cached_tokens").and_then(|v| v.as_u64()))
                                            .map(|n| n as u32);

                                        let mut s = state.lock().await;
                                        if expected_session_id.is_some_and(|expected| s.active_session_id != expected) {
                                            return Ok(None);
                                        }
                                        s.current_token_usage = Some(TokenUsage {
                                            prompt_tokens: p as u32,
                                            completion_tokens: c as u32,
                                            total_tokens: t as u32,
                                            cached_tokens: cached,
                                        });

                                        let estimation_delta = if estimated_prompt_tokens > 0 {
                                            Some((p as i64) - (estimated_prompt_tokens as i64))
                                        } else {
                                            None
                                        };

                                        crate::logger::operational_event(
                                            "provider.completion",
                                            serde_json::json!({
                                                "model": model,
                                                "prompt_tokens": p,
                                                "completion_tokens": c,
                                                "total_tokens": t,
                                                "cached_tokens": cached,
                                                "requested_max_tokens": output_token_limit,
                                                "thinking_budget": thinking_budget,
                                                "completion_limit_reached": output_token_limit
                                                    .is_some_and(|limit| c >= u64::from(limit)),
                                                "estimated_prompt_tokens": estimated_prompt_tokens,
                                                "tool_schema_tokens": tool_schema_tokens,
                                                "total_estimated_prompt_tokens": estimated_prompt_tokens,
                                                "estimation_delta": estimation_delta,
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
                return Ok(None);
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
                    "original_bytes_at_least": acc.argument_bytes,
                    "max_bytes": MAX_NATIVE_TOOL_ARGUMENT_BYTES,
                    "truncated": true,
                },
                "_parse_error": "tool arguments exceeded the local streaming limit",
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
        dbg_log!(
            "stream_request: preserving {} native tool call envelope(s)",
            native_tool_calls.len()
        );
        {
            let mut buf = buffer.lock().await;
            buf.tool_call_ids = streamed_call_ids;
            buf.native_tool_calls = native_tool_calls;
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
