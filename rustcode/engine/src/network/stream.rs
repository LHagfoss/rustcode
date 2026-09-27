/// Accumulates text emitted by a provider stream while the network layer
/// processes the stream events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FinalAnswerBoundary {
    #[default]
    None,
    ReasoningClosed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProviderFinalAnswerState {
    #[default]
    None,
    Terminal,
}

/// Bounded diagnostic state for a native tool call that was still streaming
/// when the provider failed. This is deliberately not a tool-call envelope:
/// it can never be dispatched or rendered as a completed provider message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeToolCallCheckpoint {
    pub index: Option<usize>,
    pub call_id: Option<String>,
    pub tool_name: String,
    pub argument_bytes: usize,
    pub arguments_complete: bool,
    pub arguments_overflowed: bool,
    pub argument_fingerprint: String,
    pub diagnostic: String,
}

pub(crate) struct StreamBuffer {
    pub content: String,
    /// Classification of a successful stream termination. Failures are
    /// carried by `StreamFailure` and classified by the turn layer.
    pub termination: Option<crate::network::lifecycle::StreamTermination>,
    pub final_answer_boundary: FinalAnswerBoundary,
    pub provider_final_answer_state: ProviderFinalAnswerState,
    pub thought_time_ms: u64,
    pub thought_tokens: u32,
    /// Total kept reasoning chars this response. The integer estimate is
    /// derived once from this total (see `thought_tokens_estimate`); summing
    /// per-SSE-delta ceils instead overcounts token-piece streams by ~0.5
    /// tokens per event and cuts thinking early.
    pub thought_chars: usize,
    pub thought_started_at: Option<std::time::Instant>,
    /// Effective output ceiling used for this request, recorded so the turn
    /// runner can make an evidence-based continuation decision.
    pub output_token_limit: Option<u32>,
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
    /// Native call identity and argument diagnostics observed before a
    /// failed stream. Kept separate from `native_tool_calls` so partial data
    /// cannot reach the dispatcher or durable structured history.
    pub native_tool_call_checkpoint: Vec<NativeToolCallCheckpoint>,
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            termination: None,
            final_answer_boundary: FinalAnswerBoundary::None,
            provider_final_answer_state: ProviderFinalAnswerState::None,
            thought_time_ms: 0,
            thought_tokens: 0,
            thought_chars: 0,
            thought_started_at: None,
            output_token_limit: None,
            tool_call_ids: Vec::new(),
            native_tool_calls: Vec::new(),
            native_tool_call_checkpoint: Vec::new(),
        }
    }

    /// Drops everything carried over from a previous request.
    pub fn reset(&mut self) {
        self.content.clear();
        self.termination = None;
        self.final_answer_boundary = FinalAnswerBoundary::None;
        self.provider_final_answer_state = ProviderFinalAnswerState::None;
        self.thought_time_ms = 0;
        self.thought_tokens = 0;
        self.thought_chars = 0;
        self.thought_started_at = None;
        self.output_token_limit = None;
        self.tool_call_ids.clear();
        self.native_tool_calls.clear();
        self.native_tool_call_checkpoint.clear();
    }

    /// Integer thinking estimate derived once from the total kept
    /// reasoning chars, instead of summing per-SSE-delta ceils.
    pub fn thought_tokens_estimate(&self) -> u32 {
        (self.thought_chars as f64 * crate::app::TOKENS_PER_CHAR_APPROX).ceil() as u32
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
        buffer.thought_chars = 16;
        buffer.thought_started_at = Some(std::time::Instant::now());
        buffer.final_answer_boundary = FinalAnswerBoundary::ReasoningClosed;
        buffer.provider_final_answer_state = ProviderFinalAnswerState::Terminal;
        buffer.tool_call_ids.push("call-1".to_string());
        buffer
            .native_tool_calls
            .push(crate::tools::ToolCallEnvelope {
                call_id: "call-1".to_string(),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({"pattern": "x"}),
            });

        buffer.reset();

        assert!(buffer.tool_call_ids.is_empty());
        assert!(buffer.termination.is_none());
        assert!(buffer.native_tool_calls.is_empty());
        assert!(buffer.native_tool_call_checkpoint.is_empty());
        assert_eq!(buffer.thought_time_ms, 0);
        assert_eq!(buffer.thought_tokens, 0);
        assert_eq!(buffer.thought_chars, 0);
        assert_eq!(buffer.thought_tokens_estimate(), 0);
        assert!(buffer.thought_started_at.is_none());
        assert_eq!(buffer.final_answer_boundary, FinalAnswerBoundary::None);
        assert_eq!(
            buffer.provider_final_answer_state,
            ProviderFinalAnswerState::None
        );
    }
}
