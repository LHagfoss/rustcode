use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, SessionUpdate, StopReason, TextContent, ToolCall as AcpToolCall,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};

pub(crate) struct AcpEventStream {
    streamed_prose: String,
    pending: String,
    in_thought: bool,
    /// The open thought has shown text; whitespace before that is dropped,
    /// so a client never gets a thought with nothing in it.
    thought_has_text: bool,
    /// Thoughts are the provider's reasoning summary, not reasoning text.
    reasoning_summary: bool,
    /// How long the reasoning took, sent with replayed thoughts.
    thought_ms: Option<u64>,
    /// The session directory, for edit paths a receipt does not resolve.
    cwd: Option<std::path::PathBuf>,
}

impl AcpEventStream {
    pub fn new() -> Self {
        Self {
            streamed_prose: String::new(),
            pending: String::new(),
            in_thought: false,
            thought_has_text: false,
            reasoning_summary: false,
            thought_ms: None,
            cwd: None,
        }
    }

    pub fn with_cwd(mut self, cwd: std::path::PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    fn text_update(text: String) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    /// A piece of reasoning. `_meta.rustcode.thoughtSource` says what it is:
    /// `summary`, the provider's summary of reasoning it keeps hidden, or
    /// `raw`, reasoning text the model streamed. Replayed thoughts also carry
    /// `thoughtMs`, how long the reasoning took.
    fn thought_update(&mut self, text: String) -> Option<SessionUpdate> {
        let text = if self.thought_has_text {
            text
        } else {
            text.trim_start().to_owned()
        };
        if text.is_empty() {
            return None;
        }
        self.thought_has_text = true;
        let mut meta = serde_json::Map::new();
        let source = if self.reasoning_summary {
            "summary"
        } else {
            "raw"
        };
        meta.insert("thoughtSource".into(), source.into());
        if let Some(ms) = self.thought_ms {
            meta.insert("thoughtMs".into(), ms.into());
        }
        Some(SessionUpdate::AgentThoughtChunk(
            ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
                .meta(rustcode_meta(serde_json::Value::Object(meta))),
        ))
    }

    /// A stored assistant message as the live stream sent it: prose as
    /// message chunks, each `<think>` span as thought chunks labelled with
    /// the message's reasoning source, so a replayed turn reads like the
    /// live one.
    pub(crate) fn replay_assistant_text(
        content: &str,
        reasoning_summary: bool,
        thought_ms: Option<u64>,
    ) -> Vec<SessionUpdate> {
        let mut stream = Self::new();
        stream.reasoning_summary = reasoning_summary;
        stream.thought_ms = thought_ms;
        let mut updates = stream.process_text_delta(content.to_owned());
        updates.extend(stream.flush());
        updates
    }

    fn flush(&mut self) -> Vec<SessionUpdate> {
        let mut updates = Vec::new();
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            if self.in_thought {
                updates.extend(self.thought_update(pending));
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
                    updates.extend(self.thought_update(thought));
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
                        updates.extend(self.thought_update(thought));
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
                self.thought_has_text = false;
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
            crate::network::AgentUiEvent::ReasoningSource { summary } => {
                self.reasoning_summary = summary;
                Vec::new()
            }
            crate::network::AgentUiEvent::ToolStarted {
                name,
                id,
                detail,
                summary,
            } => {
                let mut updates = self.flush();
                let mut call = AcpToolCall::new(id, name).status(ToolCallStatus::InProgress);
                // What the call acts on (a command, a path) and what the model
                // said it does, so a client can say what is running before it
                // finishes.
                let mut meta = serde_json::Map::new();
                if let Some(detail) = detail {
                    meta.insert("detail".into(), detail.into());
                }
                if let Some(summary) = summary {
                    meta.insert("summary".into(), summary.into());
                }
                if !meta.is_empty() {
                    call = call.meta(rustcode_meta(serde_json::Value::Object(meta)));
                }
                updates.push(SessionUpdate::ToolCall(call));
                updates
            }
            crate::network::AgentUiEvent::ToolOutput { id, text } => {
                // A delta to append, not ACP `content` (which replaces), so
                // a long command does not resend everything it printed.
                let mut updates = self.flush();
                updates.push(SessionUpdate::ToolCallUpdate(
                    ToolCallUpdate::new(id, ToolCallUpdateFields::new())
                        .meta(rustcode_meta(serde_json::json!({ "outputDelta": text }))),
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
                let diffs = super::edit_diff::edit_diff_content(
                    &result.tool_name,
                    status == ToolCallStatus::Completed,
                    &result.content,
                    result.diff.as_deref(),
                    &result.metadata.changed_paths,
                    self.cwd.as_deref(),
                );
                let mut updates = self.flush();
                let mut fields = ToolCallUpdateFields::new().status(status);
                if !diffs.is_empty() {
                    fields = fields.content(diffs);
                }
                updates.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    id,
                    fields.raw_output(serde_json::json!({
                        "content": result.content,
                        "exitCode": result.metadata.exit_code,
                        "changedPaths": result.metadata.changed_paths,
                        "truncated": result.metadata.truncated,
                        "completeness": result.metadata.completeness.as_str(),
                        "commandStatus": result.metadata.command_status,
                    })),
                )));
                updates
            }
            #[cfg(test)]
            crate::network::AgentUiEvent::TurnRecovered { message } => {
                let mut updates = self.flush();
                updates.extend(self.thought_update(message));
                updates
            }
            crate::network::AgentUiEvent::TurnFinished { content, .. } => {
                let mut updates = self.flush();
                // A harness stop marker is not part of the answer. The prompt's
                // stop reason already says the user cancelled; any other stop
                // goes out as a notice the client can toast.
                if let Some(notice) = crate::network::lifecycle::stop_marker_notice(&content) {
                    if !crate::network::lifecycle::is_turn_cancelled_marker(&content) {
                        // A stop `continue` can pick up is flagged, so the
                        // client can offer to continue instead of a toast.
                        let resumable =
                            crate::network::lifecycle::stop_marker_is_resumable(&content);
                        updates.push(SessionUpdate::AgentMessageChunk(
                            ContentChunk::new(ContentBlock::Text(TextContent::new(notice))).meta(
                                rustcode_meta(serde_json::json!({
                                    "kind": "notice",
                                    "resumable": resumable,
                                })),
                            ),
                        ));
                    }
                    return updates;
                }
                // The turn already put reasoning a model wrote behind bare
                // `thought` markers into `<think>` spans (`turn::run_round`).
                let prose = rustcode_tool_protocol::text::strip_think_blocks(&content);
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
            | crate::network::AgentUiEvent::QuestionRequested { .. }
            | crate::network::AgentUiEvent::Cancelled { .. } => Vec::new(),
            #[cfg(test)]
            crate::network::AgentUiEvent::Error { .. } => Vec::new(),
        }
    }
}

/// How long a finished turn took and when it ended, for the client to show
/// under the turn's last reply. A replayed message carries it on its last
/// prose chunk as `_meta.rustcode.turn`: `{id, startedAt, endedAt,
/// elapsedMs, outcome}`. A turn still open has no end to show, so it sends
/// nothing.
pub(crate) fn attach_turn_timing(updates: &mut [SessionUpdate], turn: &crate::app::TurnTiming) {
    let (Some(ended_at), Some(outcome)) = (&turn.ended_at, turn.outcome) else {
        return;
    };
    let Some(SessionUpdate::AgentMessageChunk(chunk)) = updates
        .iter_mut()
        .rev()
        .find(|update| matches!(update, SessionUpdate::AgentMessageChunk(_)))
    else {
        return;
    };
    let mut timing = serde_json::Map::new();
    timing.insert("id".into(), turn.turn_id.clone().into());
    if let Some(started_at) = &turn.started_at {
        timing.insert("startedAt".into(), started_at.clone().into());
    }
    timing.insert("endedAt".into(), ended_at.clone().into());
    if let Some(ms) = turn.elapsed_work_ms {
        timing.insert("elapsedMs".into(), ms.into());
    }
    let outcome = match outcome {
        crate::app::TurnOutcome::Completed => "completed",
        crate::app::TurnOutcome::Cancelled => "cancelled",
        crate::app::TurnOutcome::Failed => "failed",
    };
    timing.insert("outcome".into(), outcome.into());
    let mut meta = chunk.meta.take().unwrap_or_default();
    let mut rustcode = meta
        .remove("rustcode")
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    rustcode.insert("turn".into(), serde_json::Value::Object(timing));
    meta.insert("rustcode".into(), serde_json::Value::Object(rustcode));
    chunk.meta = Some(meta);
}

fn rustcode_meta(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([("rustcode".to_owned(), value)])
}

pub(crate) fn acp_stop_reason(cancelled: bool, harness_reason: Option<&str>) -> StopReason {
    if cancelled {
        StopReason::Cancelled
    } else if harness_reason.is_some_and(|reason| reason.starts_with("output_limit:")) {
        StopReason::MaxTokens
    } else if harness_reason
        .is_some_and(|reason| reason.starts_with("budget:") || reason == "loop_escalation")
    {
        StopReason::MaxTurnRequests
    } else {
        StopReason::EndTurn
    }
}
