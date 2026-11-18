//! Request payloads for the Responses API and the ChatGPT plan endpoint, request profile selection, and the request-prefix report.

use super::*;

pub(super) fn responses_input_from_messages(
    messages: &[serde_json::Value],
) -> Vec<serde_json::Value> {
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
        // Only a user turn carries images; assistant and system text stays text.
        let images = if role == "user" {
            responses_image_parts(message.get("content"))
        } else {
            Vec::new()
        };
        if !text.is_empty() || !images.is_empty() {
            let content_type = if role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            let mut content = Vec::new();
            if !text.is_empty() {
                content.push(serde_json::json!({"type": content_type, "text": text}));
            }
            content.extend(images);
            let item = serde_json::json!({
                "role": role,
                "content": content,
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

/// The image parts of a multi-part message, in the Responses dialect. Taking
/// only the text of a message used to drop every attached image before the
/// request was written, so a vision model was asked about a picture it never
/// received.
pub(super) fn responses_image_parts(content: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    content
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            let kind = part.get("type").and_then(serde_json::Value::as_str)?;
            if !matches!(kind, "image_url" | "input_image") {
                return None;
            }
            // Chat Completions nests the URL in an object; Responses takes it
            // as a string.
            let url = part
                .pointer("/image_url/url")
                .or_else(|| part.get("image_url"))
                .and_then(serde_json::Value::as_str)?;
            Some(serde_json::json!({"type": "input_image", "image_url": url}))
        })
        .collect()
}

/// ChatGPT plan requests reject Responses system-message items. Developer
/// messages carry the same local instructions while preserving their position
/// and content in the complete stateless history.
pub(super) fn chatgpt_plan_input_from_messages(
    messages: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    responses_input_from_messages(messages)
        .into_iter()
        .map(|mut item| {
            if item.get("role").and_then(serde_json::Value::as_str) == Some("system") {
                item["role"] = serde_json::json!("developer");
            }
            item
        })
        .collect()
}

pub(super) fn chatgpt_plan_additional_tools(
    schemas: &[serde_json::Value],
) -> Option<serde_json::Value> {
    let tools = responses_tool_schemas(schemas);
    (!tools.is_empty()).then(|| {
        serde_json::json!({
            "type": "additional_tools",
            "role": "developer",
            "tools": tools,
        })
    })
}

pub(super) fn response_message_text(content: Option<&serde_json::Value>) -> String {
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

pub(super) fn responses_tool_schemas(schemas: &[serde_json::Value]) -> Vec<serde_json::Value> {
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
            // The Responses API treats a function with no `strict` as strict,
            // which makes every property required: a model could not leave out
            // an optional MCP argument and sent `0` for it, which the server
            // rejected on every retry. Only a tool that asks for it is strict.
            response_schema["strict"] = function
                .get("strict")
                .cloned()
                .unwrap_or(serde_json::Value::Bool(false));
            Some(response_schema)
        })
        .collect()
}

pub(super) fn apply_responses_generation_options(
    payload: &mut serde_json::Value,
    profile: Option<&crate::config::ModelProfile>,
    thinking_mode: ThinkingMode,
) {
    if !thinking_mode.keeps_profile_reasoning() {
        return;
    }
    if let Some(p) = profile.filter(|p| p.supports_reasoning_effort_wire())
        && let Some(effort) = p.reasoning_effort.as_deref()
    {
        payload["reasoning"] =
            serde_json::json!({"effort": request_reasoning_effort(p, effort, thinking_mode)});
    }
}

pub(super) fn apply_responses_sampling_options(
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

pub(crate) fn is_chatgpt_credential(profile: Option<&crate::config::ModelProfile>) -> bool {
    profile
        .and_then(|profile| profile.credential.as_ref())
        .is_some_and(crate::provider_auth::CredentialRef::is_chatgpt)
}

/// The service tier Codex's Fast mode asks for. Codex's model catalog lists
/// it as `{"id": "priority", "name": "Fast"}` (#2582).
pub(crate) const FAST_SERVICE_TIER: &str = "priority";

/// Ask a ChatGPT-plan request for the fast (priority) tier when the session
/// turned fast mode on. Only the session's own turns and its subagents ask
/// for it: side requests (titles, recaps, summaries) are not worth the plan
/// usage, and no other transport knows the tier.
pub(crate) fn apply_fast_service_tier(
    payload: &mut serde_json::Value,
    chatgpt_plan: bool,
    fast_mode: bool,
    session_turn: bool,
) {
    if chatgpt_plan && fast_mode && session_turn {
        payload["service_tier"] = serde_json::json!(FAST_SERVICE_TIER);
    }
}

/// The tier a Responses `response.completed`/`response.incomplete` event
/// says the provider served, when it says one.
pub(crate) fn served_service_tier(event: &serde_json::Value) -> Option<&serde_json::Value> {
    event
        .pointer("/response/service_tier")
        .filter(|tier| !tier.is_null())
}

pub(crate) fn chatgpt_endpoint_is_supported(profile: &crate::config::ModelProfile) -> bool {
    profile.resolved_api_protocol() == crate::config::ApiProtocol::Responses
        && profile.endpoint_url() == "https://api.openai.com/v1/responses"
}

pub(crate) fn select_request_profile(
    profiles: &[crate::config::ModelProfile],
    selected_name: &str,
    url: &str,
    model: &str,
) -> Result<Option<crate::config::ModelProfile>, String> {
    let matches = profiles
        .iter()
        .filter(|profile| profile.matches_request(url, model))
        .collect::<Vec<_>>();
    if let Some(selected) = matches.iter().find(|profile| profile.name == selected_name) {
        return Ok(Some((*selected).clone()));
    }
    if matches.len() > 1 && matches.iter().any(|profile| profile.credential.is_some()) {
        return Err(format!(
            "multiple credential-bearing model profiles match {model} at {url}; select the intended profile with /model <profile> before sending"
        ));
    }
    Ok(matches.first().map(|profile| (*profile).clone()))
}

/// Fingerprint and size of each leading unit of a request, in the order a
/// provider reads them for prefix caching: the tool schemas, then every input
/// item or message.
pub(super) fn request_prefix_units(payload: &serde_json::Value) -> Vec<(u64, usize, String)> {
    use std::hash::{Hash, Hasher};
    let unit = |kind: String, value: &serde_json::Value| {
        let text = value.to_string();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        (hasher.finish(), text.len(), kind)
    };
    let mut units = Vec::new();
    if let Some(tools) = payload.get("tools") {
        units.push(unit("tools".to_owned(), tools));
    }
    let items = payload
        .get("input")
        .or_else(|| payload.get("messages"))
        .and_then(serde_json::Value::as_array);
    for item in items.into_iter().flatten() {
        let kind = item
            .get("type")
            .or_else(|| item.get("role"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("item");
        units.push(unit(kind.to_owned(), item));
    }
    units
}

/// Compare a request with the previous one sent for the same session and
/// report how far they agree from the start. A provider can only serve the
/// shared part from its prompt cache, so `shared_bytes` against the cached
/// tokens it reports shows whether a miss comes from the request changing or
/// from the provider. Only sizes, kinds and positions are recorded.
pub(super) fn request_prefix_report(
    session_id: &str,
    payload: &serde_json::Value,
) -> serde_json::Value {
    static PREVIOUS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, Vec<(u64, usize, String)>>>,
    > = std::sync::LazyLock::new(Default::default);
    const TRACKED_SESSIONS: usize = 16;

    let units = request_prefix_units(payload);
    let mut tracked = PREVIOUS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if tracked.len() >= TRACKED_SESSIONS && !tracked.contains_key(session_id) {
        tracked.clear();
    }
    let previous = tracked.insert(session_id.to_owned(), units.clone());
    drop(tracked);
    request_prefix_comparison(previous.as_deref(), &units)
}

pub(super) fn request_prefix_comparison(
    previous: Option<&[(u64, usize, String)]>,
    units: &[(u64, usize, String)],
) -> serde_json::Value {
    let bytes = |units: &[(u64, usize, String)]| units.iter().map(|unit| unit.1).sum::<usize>();
    let Some(previous) = previous else {
        return serde_json::json!({"units": units.len(), "bytes": bytes(units), "previous_units": null});
    };
    let shared = previous
        .iter()
        .zip(units)
        .take_while(|(before, now)| before.0 == now.0)
        .count();
    serde_json::json!({
        "units": units.len(),
        "bytes": bytes(units),
        "previous_units": previous.len(),
        "previous_bytes": bytes(previous),
        "shared_units": shared,
        "shared_bytes": bytes(&units[..shared]),
        // The first unit that differs, as it was in the previous request and
        // as it is now. Absent when the previous request is a full prefix.
        "first_changed": previous.get(shared).map(|before| serde_json::json!({
            "index": shared,
            "previous_kind": before.2,
            "previous_bytes": before.1,
            "kind": units.get(shared).map(|now| now.2.as_str()),
            "bytes": units.get(shared).map(|now| now.1),
        })),
    })
}

pub(super) fn chatgpt_stream_payload(
    model: &str,
    messages: &[serde_json::Value],
    native_tool_schemas: &[serde_json::Value],
    profile: Option<&crate::config::ModelProfile>,
    thinking_mode: ThinkingMode,
) -> serde_json::Value {
    let mut input = chatgpt_plan_input_from_messages(messages);
    if let Some(tools) = chatgpt_plan_additional_tools(native_tool_schemas) {
        // Directly after the instructions, on every request: placed before the
        // first function call it sat at the end of a turn's first request and
        // moved forward on the second, so no round could reuse the first
        // request's cached prefix. (#1796)
        let insert_at = input
            .iter()
            .take_while(|item| {
                item.get("role").and_then(serde_json::Value::as_str) == Some("developer")
            })
            .count();
        input.insert(insert_at, tools);
    }
    let mut payload = serde_json::json!({
        "model": model,
        "stream": true,
        "store": false,
        "input": input,
    });
    apply_responses_generation_options(&mut payload, profile, thinking_mode);
    // Without a summary the plan streams no reasoning text at all, so the
    // transcript showed no thinking even when a turn reasoned for thousands
    // of tokens.
    if let Some(reasoning) = payload.get_mut("reasoning").and_then(|r| r.as_object_mut()) {
        reasoning.insert("summary".into(), serde_json::json!("auto"));
    }
    payload
}

pub(super) fn responses_usage(value: &serde_json::Value) -> Option<serde_json::Value> {
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
pub(super) fn normalize_responses_event(
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
        // The model's own reasoning text (open-weight models served over
        // Responses).
        "response.reasoning_text.delta" => {
            delta.map(|delta| serde_json::json!({"choices": [{"delta": {"reasoning": delta}}]}))
        }
        // What OpenAI shows of hidden reasoning: a summary, often only a
        // bold heading. It is marked so clients label it as a summary.
        "response.reasoning_summary_text.delta" => delta.map(|delta| {
            serde_json::json!({"choices": [{"delta": {"reasoning": delta, "reasoning_summary": true}}]})
        }),
        // Summary parts arrive back to back, within one reasoning item and
        // across items (whose `summary_index` starts at 0 again); keep each
        // its own paragraph. The stream drops the separator when no reasoning
        // came before it, so it never opens an empty thought.
        "response.reasoning_summary_part.added" => Some(serde_json::json!({"choices": [{"delta": {
            "reasoning": "\n\n",
            "reasoning_summary": true,
            "reasoning_separator": true,
        }}]})),
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
        "response.incomplete" => {
            let reason = value
                .get("response")
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            if reason == "max_output_tokens" {
                let mut event = serde_json::json!({
                    "choices": [{"delta": {}, "finish_reason": "length"}],
                });
                if let Some(usage) = responses_usage(value) {
                    event["usage"] = usage;
                }
                Some(event)
            } else {
                Some(serde_json::json!({
                    "error": {
                        "code": "response_incomplete",
                        "message": format!("Responses API did not complete the response (reason: {reason})"),
                    }
                }))
            }
        }
        "response.failed" => {
            let error = value
                .get("response")
                .and_then(|response| response.get("error"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(serde_json::json!({
                "error": {
                    "code": error.get("code").cloned().unwrap_or(serde_json::Value::Null),
                    "message": error.get("message").and_then(serde_json::Value::as_str).unwrap_or("Responses API request failed"),
                    "param": error.get("param").cloned().unwrap_or(serde_json::Value::Null),
                    "status_code": error.get("status_code").cloned().unwrap_or(serde_json::Value::Null),
                }
            }))
        }
        "error" => {
            // The public API reports the fields on the event itself; the ChatGPT
            // backend can nest them under `error`.
            let error = value
                .get("error")
                .filter(|error| error.is_object())
                .unwrap_or(value);
            Some(serde_json::json!({
                "error": {
                    "code": error.get("code").cloned().unwrap_or(serde_json::Value::Null),
                    "message": error.get("message").and_then(serde_json::Value::as_str).unwrap_or("Responses API request failed"),
                    "param": error.get("param").cloned().unwrap_or(serde_json::Value::Null),
                    "status_code": error.get("status_code").cloned().unwrap_or(serde_json::Value::Null),
                }
            }))
        }
        _ => None,
    }
}
