use crate::app::{ChatMessage, TokenUsage};
use crate::ui::scrollback::TranscriptCursor;
use ratatui::text::Line;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HistoryCell {
    User(String),
    Assistant {
        content: String,
        token_usage: Option<TokenUsage>,
        response_time_ms: Option<u64>,
        thought_time_ms: Option<u64>,
        thought_tokens: Option<u32>,
    },
    Tool(String),
    System(String),
    Error(String),
    Plan(String),
}

impl HistoryCell {
    fn from_message(message: &ChatMessage) -> Self {
        match message.role.as_str() {
            "user" => Self::User(message.content.clone()),
            "assistant" => Self::Assistant {
                content: message.content.clone(),
                token_usage: message.token_usage.clone(),
                response_time_ms: message.response_time_ms,
                thought_time_ms: message.thought_time_ms,
                thought_tokens: message.thought_tokens,
            },
            "tool" => Self::Tool(message.content.clone()),
            "error" => Self::Error(message.content.clone()),
            "plan" => Self::Plan(message.content.clone()),
            _ => Self::System(message.content.clone()),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::User(text)
            | Self::Tool(text)
            | Self::System(text)
            | Self::Error(text)
            | Self::Plan(text) => text,
            Self::Assistant { content, .. } => content,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptEvent {
    TextDelta(String),
    CommitLive,
    Resize,
}

#[derive(Default)]
pub(crate) struct TranscriptModel {
    committed: Vec<HistoryCell>,
    live: Option<HistoryCell>,
    cursor: TranscriptCursor,
    replay_revision: u64,
}

impl TranscriptModel {
    pub(crate) fn from_history(history: &[ChatMessage]) -> Self {
        Self {
            committed: history.iter().map(HistoryCell::from_message).collect(),
            ..Self::default()
        }
    }

    pub(crate) fn committed(&self) -> &[HistoryCell] {
        &self.committed
    }

    /// Synchronize the durable portion of the presentation model with the
    /// canonical session history. The UI owns the projection; `ChatMessage`
    /// remains the persistence and provider boundary.
    pub(crate) fn sync_history(&mut self, history: &[ChatMessage]) {
        let next = history
            .iter()
            .map(HistoryCell::from_message)
            .collect::<Vec<_>>();
        if self.committed != next {
            self.committed = next;
        }
    }

    /// Replace the one mutable assistant cell with the current cumulative
    /// stream. Provider deltas are cumulative in `AppState`, so appending here
    /// would duplicate content on every redraw.
    pub(crate) fn replace_live_text(&mut self, text: &str) {
        if text.is_empty() {
            self.live = None;
        } else if let Some(HistoryCell::Assistant { content, .. }) = self.live.as_mut() {
            content.clear();
            content.push_str(text);
        } else {
            self.live = Some(HistoryCell::Assistant {
                content: text.to_owned(),
                token_usage: None,
                response_time_ms: None,
                thought_time_ms: None,
                thought_tokens: None,
            });
        }
    }

    pub(crate) fn live_text(&self) -> Option<&str> {
        self.live.as_ref().map(HistoryCell::text)
    }

    pub(crate) fn apply(&mut self, event: TranscriptEvent) {
        match event {
            TranscriptEvent::TextDelta(text) => self.apply_text_delta(&text),
            TranscriptEvent::CommitLive => self.commit_live(),
            TranscriptEvent::Resize => self.reset_for_resize(),
        }
    }

    pub(crate) fn apply_agent_event(&mut self, event: &crate::network::ui_adapter::AgentUiEvent) {
        match event {
            crate::network::ui_adapter::AgentUiEvent::PromptStarted { .. } => {
                self.live = None;
            }
            crate::network::ui_adapter::AgentUiEvent::SubagentUpdated { .. } => {}
            crate::network::ui_adapter::AgentUiEvent::TextDelta { text } => {
                self.apply_text_delta(text);
            }
            crate::network::ui_adapter::AgentUiEvent::TurnFinished { content, .. } => {
                if !content.is_empty() {
                    self.replace_live_text(content);
                }
                self.commit_live();
            }
            crate::network::ui_adapter::AgentUiEvent::Cancelled { .. }
            | crate::network::ui_adapter::AgentUiEvent::Error { .. }
            | crate::network::ui_adapter::AgentUiEvent::TurnRecovered { .. }
            | crate::network::ui_adapter::AgentUiEvent::ToolStarted { .. }
            | crate::network::ui_adapter::AgentUiEvent::ApprovalRequested { .. }
            | crate::network::ui_adapter::AgentUiEvent::ToolFinished { .. } => {}
        }
    }

    pub(crate) fn apply_text_delta(&mut self, text: &str) {
        match self.live.as_mut() {
            Some(HistoryCell::Assistant { content, .. }) => content.push_str(text),
            Some(_) => {
                self.live = Some(HistoryCell::Assistant {
                    content: text.to_owned(),
                    token_usage: None,
                    response_time_ms: None,
                    thought_time_ms: None,
                    thought_tokens: None,
                });
            }
            None => {
                self.live = Some(HistoryCell::Assistant {
                    content: text.to_owned(),
                    token_usage: None,
                    response_time_ms: None,
                    thought_time_ms: None,
                    thought_tokens: None,
                });
            }
        }
    }

    pub(crate) fn commit_live(&mut self) {
        if let Some(live) = self.live.take() {
            self.committed.push(live);
        }
    }

    pub(crate) fn reset_for_resize(&mut self) {
        self.cursor.reset();
        self.replay_revision = self.replay_revision.saturating_add(1);
    }

    #[allow(dead_code)]
    pub(crate) fn replay_revision(&self) -> u64 {
        self.replay_revision
    }

    #[allow(dead_code)]
    pub(crate) fn render(&self, _width: u16, height: u16) -> Vec<Line<'static>> {
        self.committed
            .iter()
            .chain(self.live.iter())
            .flat_map(|cell| cell.text().lines().map(|line| Line::from(line.to_owned())))
            .take(usize::from(height))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryCell, TranscriptEvent, TranscriptModel};
    use crate::app::ChatMessage;

    #[test]
    fn history_converts_to_owned_cells_and_keeps_roles() {
        let history = vec![
            ChatMessage::new("user", "hello"),
            ChatMessage::new("assistant", "answer"),
            ChatMessage::new("tool", "output"),
        ];

        let model = TranscriptModel::from_history(&history);

        assert!(matches!(&model.committed()[0], HistoryCell::User(text) if text == "hello"));
        assert!(
            matches!(&model.committed()[1], HistoryCell::Assistant { content, .. } if content == "answer")
        );
        assert!(matches!(&model.committed()[2], HistoryCell::Tool(text) if text == "output"));
    }

    #[test]
    fn text_deltas_replace_one_mutable_live_cell_until_commit() {
        let mut model = TranscriptModel::default();
        model.apply(TranscriptEvent::TextDelta("first".to_owned()));
        model.apply(TranscriptEvent::TextDelta(" second".to_owned()));

        assert_eq!(model.live_text(), Some("first second"));
        assert_eq!(model.committed().len(), 0);

        model.apply(TranscriptEvent::CommitLive);

        assert_eq!(model.live_text(), None);
        assert!(matches!(
            &model.committed()[0],
            HistoryCell::Assistant { content, .. } if content == "first second"
        ));
    }

    #[test]
    fn resize_replay_rebuilds_from_canonical_cells() {
        let history = vec![ChatMessage::new("assistant", "stable")];
        let mut model = TranscriptModel::from_history(&history);
        model.apply_text_delta("live");

        model.apply(TranscriptEvent::Resize);

        assert_eq!(model.committed().len(), 1);
        assert_eq!(model.live_text(), Some("live"));
    }

    #[test]
    fn agent_ui_events_update_one_live_cell_and_commit_it() {
        let mut model = TranscriptModel::default();
        model.apply_agent_event(&crate::network::ui_adapter::AgentUiEvent::TextDelta {
            text: "answer".to_owned(),
        });
        model.apply_agent_event(&crate::network::ui_adapter::AgentUiEvent::TurnFinished {
            content: "answer".to_owned(),
            completed: true,
        });

        assert_eq!(model.live_text(), None);
        assert!(matches!(
            &model.committed()[0],
            HistoryCell::Assistant { content, .. } if content == "answer"
        ));
    }
}
