use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionUpdate, StopReason, TextContent, ToolCall as AcpToolCall,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};

pub(crate) struct AcpEventStream {
    streamed_prose: String,
    pending: String,
    in_thought: bool,
}

impl AcpEventStream {
    pub(crate) fn new() -> Self {
        Self {
            streamed_prose: String::new(),
            pending: String::new(),
            in_thought: false,
        }
    }

    fn text_update(text: String) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn thought_update(text: String) -> SessionUpdate {
        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn flush(&mut self) -> Vec<SessionUpdate> {
        let mut updates = Vec::new();
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            if self.in_thought {
                updates.push(Self::thought_update(pending));
            } else {
                self.streamed_prose.push_str(&pending);
                updates.push(Self::text_update(pending));
            }
        }
        updates
    }

    fn process_text_delta(&mut self, text: String) -> Vec<SessionUpdate> {
        const OPEN_TAG: &str = "<think>";
        const CLOSE_TAG: &str = "</think>";
        const OPEN_PREFIXES: &[&str] = &["<think", "<thin", "<thi", "<th", "<t", "<"];
        const CLOSE_PREFIXES: &[&str] = &["</think", "</thin", "</thi", "</th", "</t", "</", "<"];

        let mut updates = Vec::new();
        self.pending.push_str(&text);

        while !self.pending.is_empty() {
            if self.in_thought {
                if let Some(idx) = self.pending.find(CLOSE_TAG) {
                    let thought = self.pending[..idx].to_string();
                    if !thought.is_empty() {
                        updates.push(Self::thought_update(thought));
                    }
                    self.pending.drain(..idx + CLOSE_TAG.len());
                    self.in_thought = false;
                } else {
                    let matched_prefix_len = CLOSE_PREFIXES
                        .iter()
                        .find(|prefix| self.pending.ends_with(**prefix))
                        .map(|prefix| prefix.len())
                        .unwrap_or(0);

                    let emit_len = self.pending.len() - matched_prefix_len;
                    if emit_len > 0 {
                        let thought: String = self.pending.drain(..emit_len).collect();
                        updates.push(Self::thought_update(thought));
                    }
                    break;
                }
            } else if let Some(idx) = self.pending.find(OPEN_TAG) {
                let prose = self.pending[..idx].to_string();
                if !prose.is_empty() {
                    self.streamed_prose.push_str(&prose);
                    updates.push(Self::text_update(prose));
                }
                self.pending.drain(..idx + OPEN_TAG.len());
                self.in_thought = true;
            } else {
                let matched_prefix_len = OPEN_PREFIXES
                    .iter()
                    .find(|prefix| self.pending.ends_with(**prefix))
                    .map(|prefix| prefix.len())
                    .unwrap_or(0);

                let emit_len = self.pending.len() - matched_prefix_len;
                if emit_len > 0 {
                    let prose: String = self.pending.drain(..emit_len).collect();
                    self.streamed_prose.push_str(&prose);
                    updates.push(Self::text_update(prose));
                }
                break;
            }
        }

        updates
    }

    pub(crate) fn updates(&mut self, event: crate::network::AgentUiEvent) -> Vec<SessionUpdate> {
        match event {
            crate::network::AgentUiEvent::TextDelta { text } => self.process_text_delta(text),
            crate::network::AgentUiEvent::ToolStarted { name, id } => {
                let mut updates = self.flush();
                updates.push(SessionUpdate::ToolCall(
                    AcpToolCall::new(id, name).status(ToolCallStatus::InProgress),
                ));
                updates
            }
            crate::network::AgentUiEvent::ToolFinished { id, result } => {
                let status = if result.metadata.pending {
                    ToolCallStatus::InProgress
                } else if result.metadata.success {
                    ToolCallStatus::Completed
                } else {
                    ToolCallStatus::Failed
                };
                let mut updates = self.flush();
                updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    id,
                    ToolCallUpdateFields::new()
                        .status(status)
                        .raw_output(serde_json::json!({
                            "content": result.content,
                            "exitCode": result.metadata.exit_code,
                            "changedPaths": result.metadata.changed_paths,
                            "truncated": result.metadata.truncated,
                        })),
                )));
                updates
            }
            crate::network::AgentUiEvent::TurnRecovered { message } => {
                let mut updates = self.flush();
                updates.push(Self::thought_update(message));
                updates
            }
            crate::network::AgentUiEvent::TurnFinished { content, .. } => {
                let mut updates = self.flush();
                let promoted = crate::network::text::promote_bare_thought_markers(&content);
                let prose = crate::network::text::strip_think_blocks(&promoted);
                let trimmed_prose = prose.trim();
                if !trimmed_prose.is_empty()
                    && !self.streamed_prose.ends_with(&prose)
                    && !self.streamed_prose.ends_with(trimmed_prose)
                {
                    self.streamed_prose.push_str(&prose);
                    updates.push(Self::text_update(prose));
                }
                updates
            }
            crate::network::AgentUiEvent::PromptStarted { .. }
            | crate::network::AgentUiEvent::SubagentUpdated { .. }
            | crate::network::AgentUiEvent::ApprovalRequested { .. }
            | crate::network::AgentUiEvent::Cancelled { .. }
            | crate::network::AgentUiEvent::Error { .. } => Vec::new(),
        }
    }
}

pub(crate) fn acp_stop_reason(cancelled: bool, harness_reason: Option<&str>) -> StopReason {
    if cancelled {
        StopReason::Cancelled
    } else if harness_reason
        .is_some_and(|reason| reason.starts_with("budget:") || reason == "loop_escalation")
    {
        StopReason::MaxTurnRequests
    } else {
        StopReason::EndTurn
    }
}
