//! Native Messages wire adapter. Canonical history and tool dispatch stay in
//! RustCode's existing provider-independent representation.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) fn content_blocks(content: Option<&Value>) -> Result<Vec<Value>> {
    match content {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(text)) => Ok(if text.is_empty() { Vec::new() } else { vec![json!({"type":"text","text":text})] }),
        Some(Value::Array(parts)) => parts.iter().map(|part| {
            match part.get("type").and_then(Value::as_str) {
                Some("text" | "input_text") => Ok(json!({"type":"text","text":part.get("text").and_then(Value::as_str).unwrap_or("")})),
                Some("image_url" | "input_image") => {
                    let url = part.pointer("/image_url/url").or_else(|| part.get("image_url")).and_then(Value::as_str).ok_or_else(|| anyhow!("image input has no URL"))?;
                    let source = if let Some(data) = url.strip_prefix("data:") {
                        let (mime, encoded) = data.split_once(";base64,").ok_or_else(|| anyhow!("Messages image input must be a base64 data URL"))?;
                        if !matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp") { bail!("Messages image format is unsupported"); }
                        json!({"type":"base64","media_type":mime,"data":encoded})
                    } else {
                        json!({"type":"url","url":url})
                    };
                    Ok(json!({"type":"image","source":source}))
                }
                _ => bail!("Messages history contains an unsupported content block"),
            }
        }).collect(),
        _ => bail!("Messages history contains invalid content"),
    }
}

pub(crate) fn request_payload(
    model: &str,
    history: &[Value],
    schemas: &[Value],
    max_tokens: u32,
    stream: bool,
) -> Result<Value> {
    let mut system = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for message in history {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user");
        if matches!(role, "system" | "developer") {
            system.extend(content_blocks(message.get("content"))?);
            continue;
        }
        let (role, mut blocks) = if role == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Messages tool result has no call ID"))?;
            (
                "user",
                vec![
                    json!({"type":"tool_result","tool_use_id":id,"content":content_blocks(message.get("content"))?}),
                ],
            )
        } else {
            (
                if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                },
                content_blocks(message.get("content"))?,
            )
        };
        if role == "assistant" {
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Messages tool call has no ID"))?;
                let name = call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Messages tool call has no name"))?;
                let args = call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input: Value = serde_json::from_str(args)
                    .map_err(|_| anyhow!("Messages tool call contains invalid JSON arguments"))?;
                if !input.is_object() {
                    bail!("Messages tool arguments must be an object");
                }
                blocks.push(json!({"type":"tool_use","id":id,"name":name,"input":input}));
            }
        }
        if blocks.is_empty() {
            continue;
        }
        if let Some(last) = messages.last_mut().filter(|last| last["role"] == role) {
            last["content"].as_array_mut().unwrap().extend(blocks);
        } else {
            messages.push(json!({"role":role,"content":blocks}));
        }
    }
    let mut payload =
        json!({"model":model,"messages":messages,"max_tokens":max_tokens.max(1),"stream":stream});
    if !system.is_empty() {
        payload["system"] = json!(system);
    }
    if !schemas.is_empty() {
        payload["tools"] = json!(schemas.iter().filter_map(|schema| {
            let function = schema.get("function")?;
            Some(json!({"name":function.get("name")?,"description":function.get("description").unwrap_or(&Value::Null),"input_schema":function.get("parameters")?}))
        }).collect::<Vec<_>>());
    }
    // Extended/adaptive thinking requires signed history blocks which the
    // canonical representation does not retain yet. Do not enable it here.
    Ok(payload)
}

#[derive(Default)]
pub(crate) struct MessagesStream {
    pub(crate) completed: bool,
    tools: HashMap<usize, (Value, bool)>,
    input: u64,
    output: u64,
    cached: u64,
    cache_write: u64,
    stop_reason: Option<String>,
}

impl MessagesStream {
    fn usage(&self) -> Value {
        let prompt = self
            .input
            .saturating_add(self.cached)
            .saturating_add(self.cache_write);
        json!({"prompt_tokens":prompt,"completion_tokens":self.output,"total_tokens":prompt.saturating_add(self.output),"prompt_tokens_details":{"cached_tokens":self.cached,"cache_write_tokens":self.cache_write}})
    }

    pub(crate) fn normalize(&mut self, event: &Value) -> Option<Value> {
        let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match event.get("type").and_then(Value::as_str)? {
            "message_start" => {
                let usage = event.pointer("/message/usage")?;
                self.input = usage
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.cached = usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.cache_write = usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.output = usage
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                Some(json!({"usage":self.usage()}))
            }
            "content_block_start" => {
                let block = event.get("content_block")?;
                match block.get("type").and_then(Value::as_str)? {
                    "tool_use" => {
                        self.tools.insert(
                            index,
                            (
                                block.get("input").cloned().unwrap_or_else(|| json!({})),
                                false,
                            ),
                        );
                        Some(
                            json!({"choices":[{"delta":{"tool_calls":[{"index":index,"id":block.get("id")?,"type":"function","function":{"name":block.get("name")?,"arguments":""}}]}}]}),
                        )
                    }
                    "text" => Some(json!({"choices":[{"delta":{"content":block.get("text")?}}]})),
                    _ => None,
                }
            }
            "content_block_delta" => {
                let delta = event.get("delta")?;
                match delta.get("type").and_then(Value::as_str)? {
                    "text_delta" => {
                        Some(json!({"choices":[{"delta":{"content":delta.get("text")?}}]}))
                    }
                    "thinking_delta" => {
                        Some(json!({"choices":[{"delta":{"reasoning":delta.get("thinking")?}}]}))
                    }
                    "input_json_delta" => {
                        let fragment = delta.get("partial_json")?.as_str()?;
                        if fragment.is_empty() {
                            return None;
                        }
                        self.tools.get_mut(&index)?.1 = true;
                        Some(
                            json!({"choices":[{"delta":{"tool_calls":[{"index":index,"function":{"arguments":delta.get("partial_json")?}}]}}]}),
                        )
                    }
                    _ => None,
                }
            }
            "content_block_stop" => {
                let (input, seen) = self.tools.remove(&index)?;
                (!seen).then(|| json!({"choices":[{"delta":{"tool_calls":[{"index":index,"function":{"arguments":input.to_string()}}]}}]}))
            }
            "message_delta" => {
                self.stop_reason = event
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(output) = event
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                {
                    self.output = output;
                }
                Some(json!({"usage":self.usage()}))
            }
            "message_stop" => {
                self.completed = true;
                let reason = match self.stop_reason.as_deref() {
                    Some("tool_use") => "tool_calls",
                    Some("max_tokens" | "model_context_window_exceeded") => "length",
                    Some("end_turn" | "stop_sequence" | "refusal") => "stop",
                    _ => {
                        return Some(
                            json!({"error":{"message":"Messages stream ended without a supported stop reason"}}),
                        );
                    }
                };
                Some(json!({"choices":[{"delta":{},"finish_reason":reason}],"usage":self.usage()}))
            }
            "error" => Some(json!({"error":event.get("error")?})),
            _ => None,
        }
    }
}

pub(crate) fn response_text(body: &Value) -> Option<String> {
    let text = body
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_payload_preserves_system_images_and_parallel_tool_transactions() {
        let payload = request_payload("claude", &[
            json!({"role":"system","content":"Be helpful"}),
            json!({"role":"user","content":[{"type":"text","text":"Inspect"},{"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}]}),
            json!({"role":"assistant","content":"Checking", "tool_calls":[
                {"id":"call_a","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.rs\"}"}},
                {"id":"call_b","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"b.rs\"}"}}
            ]}),
            json!({"role":"tool","tool_call_id":"call_a","content":"file a"}),
            json!({"role":"tool","tool_call_id":"call_b","content":"file b"}),
            json!({"role":"user","content":"Continue"})
        ], &[json!({"type":"function","function":{"name":"read_file","description":"Read","parameters":{"type":"object"}}})], 8192, true).unwrap();
        assert_eq!(payload["system"][0]["text"], "Be helpful");
        assert_eq!(payload["messages"].as_array().unwrap().len(), 3);
        assert_eq!(
            payload["messages"][0]["content"][1],
            json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}})
        );
        assert_eq!(
            payload["messages"][1]["content"][1],
            json!({"type":"tool_use","id":"call_a","name":"read_file","input":{"path":"a.rs"}})
        );
        assert_eq!(
            payload["messages"][2]["content"][0]["tool_use_id"],
            "call_a"
        );
        assert_eq!(
            payload["messages"][2]["content"][1]["tool_use_id"],
            "call_b"
        );
        assert_eq!(payload["messages"][2]["content"][2]["text"], "Continue");
        assert_eq!(
            payload["tools"][0]["input_schema"],
            json!({"type":"object"})
        );
        assert!(payload.get("parallel_tool_calls").is_none());
        assert!(payload.get("stream_options").is_none());
        assert_eq!(payload["max_tokens"], 8192);
    }

    #[test]
    fn messages_payload_rejects_invalid_tool_arguments_without_fabricating_input() {
        assert!(request_payload("claude", &[json!({"role":"assistant","tool_calls":[{"id":"a","function":{"name":"read","arguments":"{bad"}}]})], &[], 8192, true).is_err());
    }

    #[test]
    fn messages_stream_keeps_tool_identity_fragmented_arguments_and_cache_usage() {
        let mut stream = MessagesStream::default();
        stream.normalize(&json!({"type":"message_start","message":{"usage":{"input_tokens":11,"output_tokens":0,"cache_read_input_tokens":20,"cache_creation_input_tokens":3}}}));
        let start = stream.normalize(&json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_x","name":"read_file","input":{}}})).unwrap();
        assert_eq!(
            start["choices"][0]["delta"]["tool_calls"][0]["id"],
            "call_x"
        );
        let delta = stream.normalize(&json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})).unwrap();
        assert_eq!(
            delta["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":"
        );
        stream.normalize(&json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"x\"}"}}));
        assert!(
            stream
                .normalize(&json!({"type":"content_block_stop","index":1}))
                .is_none()
        );
        let usage = stream.normalize(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}})).unwrap();
        assert_eq!(usage["usage"]["prompt_tokens"], 34);
        assert_eq!(usage["usage"]["total_tokens"], 43);
        assert_eq!(usage["usage"]["prompt_tokens_details"]["cached_tokens"], 20);
        assert!(!stream.completed);
        let stop = stream.normalize(&json!({"type":"message_stop"})).unwrap();
        assert_eq!(stop["choices"][0]["finish_reason"], "tool_calls");
        assert!(stream.completed);
    }

    #[test]
    fn empty_argument_delta_keeps_provider_valid_empty_object_input() {
        let mut stream = MessagesStream::default();
        stream.normalize(&json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"empty","name":"list","input":{}}}));
        // Empty JSON fragments must not count as argument content; the stop
        // envelope still carries a provider-valid empty object.
        assert!(stream.normalize(&json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":""}})).is_none());
        let stop = stream
            .normalize(&json!({"type":"content_block_stop","index":0}))
            .unwrap();
        let args = stop["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(args).unwrap(), json!({}));
        stream.normalize(&json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}));
        let done = stream.normalize(&json!({"type":"message_stop"})).unwrap();
        assert_eq!(done["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn refusal_and_context_exhaustion_are_successful_terminal_reasons() {
        for (reason, expected) in [
            ("refusal", "stop"),
            ("model_context_window_exceeded", "length"),
        ] {
            let mut stream = MessagesStream::default();
            stream.normalize(&json!({"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}));
            let text = stream.normalize(&json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Partial"}})).unwrap();
            assert_eq!(text["choices"][0]["delta"]["content"], "Partial");
            let delta = stream.normalize(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" answer"}})).unwrap();
            assert_eq!(delta["choices"][0]["delta"]["content"], " answer");
            let usage = stream.normalize(&json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":3}})).unwrap();
            assert_eq!(usage["usage"]["prompt_tokens"], 10);
            assert_eq!(usage["usage"]["completion_tokens"], 3);
            let stop = stream.normalize(&json!({"type":"message_stop"})).unwrap();
            assert!(stop.get("error").is_none());
            assert_eq!(stop["choices"][0]["finish_reason"], expected);
            assert_eq!(stop["usage"]["completion_tokens"], 3);
        }
    }

    #[test]
    fn messages_stream_emits_empty_object_once_when_tool_has_no_argument_deltas() {
        let mut stream = MessagesStream::default();
        stream.normalize(&json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"empty","name":"list","input":{}}}));
        let stop = stream
            .normalize(&json!({"type":"content_block_stop","index":0}))
            .unwrap();
        assert_eq!(
            stop["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
    }
}
