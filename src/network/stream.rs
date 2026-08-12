/// Accumulates text emitted by a provider stream while the network layer
/// processes the stream events.
pub(crate) struct StreamBuffer {
    pub content: String,
    pub thought_time_ms: u64,
    pub thought_tokens: u32,
    pub thought_started_at: Option<std::time::Instant>,
    /// Provider-assigned ids for the structured tool calls in this response, in
    /// the order the calls appear. Empty for the text protocols, where a call is
    /// prose the model wrote and has no identity of its own.
    ///
    /// The ids matter because the provider requires each result to name the call
    /// it answers; without them a tool result is just another message and the
    /// model is free to misattribute it.
    pub tool_call_ids: Vec<String>,
    /// Structured native calls kept separate from display text. ApiNative
    /// responses must not be serialized into fenced Markdown and parsed back.
    pub native_tool_calls: Vec<crate::tools::ToolCallEnvelope>,
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            thought_time_ms: 0,
            thought_tokens: 0,
            thought_started_at: None,
            tool_call_ids: Vec::new(),
            native_tool_calls: Vec::new(),
        }
    }

    /// Drops everything carried over from a previous request.
    pub fn reset(&mut self) {
        self.content.clear();
        self.thought_time_ms = 0;
        self.thought_tokens = 0;
        self.thought_started_at = None;
        self.tool_call_ids.clear();
        self.native_tool_calls.clear();
    }

    pub fn finish_thought(&mut self) {
        if let Some(started) = self.thought_started_at.take() {
            self.thought_time_ms = self
                .thought_time_ms
                .saturating_add(started.elapsed().as_millis() as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_typed_native_calls_and_provider_ids() {
        let mut buffer = StreamBuffer::new();
        buffer.thought_time_ms = 12;
        buffer.thought_tokens = 4;
        buffer.thought_started_at = Some(std::time::Instant::now());
        buffer.tool_call_ids.push("call-1".to_string());
        buffer.native_tool_calls.push(crate::tools::ToolCallEnvelope {
            call_id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": "x"}),
        });

        buffer.reset();

        assert!(buffer.tool_call_ids.is_empty());
        assert!(buffer.native_tool_calls.is_empty());
        assert_eq!(buffer.thought_time_ms, 0);
        assert_eq!(buffer.thought_tokens, 0);
        assert!(buffer.thought_started_at.is_none());
    }
}
