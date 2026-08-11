/// Accumulates text emitted by a provider stream while the network layer
/// processes the stream events.
pub(crate) struct StreamBuffer {
    pub content: String,
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
            tool_call_ids: Vec::new(),
            native_tool_calls: Vec::new(),
        }
    }

    /// Drops everything carried over from a previous request.
    pub fn reset(&mut self) {
        self.content.clear();
        self.tool_call_ids.clear();
        self.native_tool_calls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_typed_native_calls_and_provider_ids() {
        let mut buffer = StreamBuffer::new();
        buffer.tool_call_ids.push("call-1".to_string());
        buffer.native_tool_calls.push(crate::tools::ToolCallEnvelope {
            call_id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            arguments: serde_json::json!({"pattern": "x"}),
        });

        buffer.reset();

        assert!(buffer.tool_call_ids.is_empty());
        assert!(buffer.native_tool_calls.is_empty());
    }
}
