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
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            tool_call_ids: Vec::new(),
        }
    }

    /// Drops everything carried over from a previous request.
    pub fn reset(&mut self) {
        self.content.clear();
        self.tool_call_ids.clear();
    }
}
