use crate::app::{AppStatus, ChatMessage, History, SubAgentStatus};

/// Immutable data captured for one UI render attempt.
pub(crate) struct RenderSnapshot {
    revision: u64,
    input_buffer: String,
    cursor_position: usize,
    history: History,
    history_display_start: usize,
    current_response: String,
    status: AppStatus,
    modal_open: bool,
    selected_subagent: Option<SelectedSubagentSnapshot>,
}

impl RenderSnapshot {
    pub(crate) fn new(state: &crate::app::AppState) -> Self {
        Self {
            revision: state.render_revision,
            input_buffer: state.input_buffer.clone(),
            cursor_position: state.cursor_position,
            history: state.history.snapshot(),
            history_display_start: state.history_display_start,
            current_response: state.current_response.clone(),
            status: state.status.clone(),
            modal_open: state.modal_open(),
            selected_subagent: state.selected_subagent().map(SelectedSubagentSnapshot::from),
        }
    }

    pub(crate) fn revision(&self) -> u64 { self.revision }
    pub(crate) fn input_buffer(&self) -> &str { &self.input_buffer }
    pub(crate) fn cursor_position(&self) -> usize { self.cursor_position }
    pub(crate) fn history(&self) -> &History { &self.history }
    pub(crate) fn history_display_start(&self) -> usize { self.history_display_start }
    pub(crate) fn active_history(&self) -> &[ChatMessage] {
        self.selected_subagent
            .as_ref()
            .map(|agent| agent.history())
            .unwrap_or_else(|| self.history.as_slice())
    }
    pub(crate) fn active_history_display_start(&self) -> usize {
        if self.selected_subagent.is_some() { 0 } else { self.history_display_start }
    }
    pub(crate) fn current_response(&self) -> &str { &self.current_response }
    pub(crate) fn status(&self) -> &AppStatus { &self.status }
    pub(crate) fn modal_open(&self) -> bool { self.modal_open }
    pub(crate) fn selected_subagent(&self) -> Option<&SelectedSubagentSnapshot> {
        self.selected_subagent.as_ref()
    }
}

/// The selected child context rendered in place of the root conversation.
pub(crate) struct SelectedSubagentSnapshot {
    id: u32,
    name: String,
    history: Vec<ChatMessage>,
    status: SubAgentStatus,
    active_turn: bool,
    parent_id: Option<u32>,
}

impl From<&crate::app::SubAgent> for SelectedSubagentSnapshot {
    fn from(agent: &crate::app::SubAgent) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            history: agent.history.clone(),
            status: agent.status,
            active_turn: agent.active_turn,
            parent_id: agent.parent_id,
        }
    }
}

impl SelectedSubagentSnapshot {
    pub(crate) fn id(&self) -> u32 { self.id }
    pub(crate) fn name(&self) -> &str { &self.name }
    pub(crate) fn history(&self) -> &[ChatMessage] { &self.history }
    pub(crate) fn status(&self) -> SubAgentStatus { self.status }
    pub(crate) fn active_turn(&self) -> bool { self.active_turn }
    pub(crate) fn parent_id(&self) -> Option<u32> { self.parent_id }
}

#[cfg(test)]
mod tests {
    use crate::app::{AppState, AppStatus, ChatMessage, SubAgent, SubAgentStatus};

    #[test]
    fn render_snapshot_captures_ui_state() {
        let mut state = AppState::new();
        state.input_buffer = "draft input".to_owned();
        state.cursor_position = state.input_buffer.len();
        state.status = AppStatus::Streaming;
        state.history.push(ChatMessage::new("user", "root message"));
        state.history_display_start = 1;
        state.current_response = "streamed response".to_owned();
        state.show_model_picker = true;
        state.subagents.push(SubAgent {
            id: 7,
            name: "reviewer".to_owned(),
            task: "review the patch".to_owned(),
            model: Some("test-model".to_owned()),
            history: vec![ChatMessage::new("assistant", "subagent response")],
            status: SubAgentStatus::Running,
            active_turn: true,
            parent_id: Some(3),
            write_access: false,
            allowed_paths: Vec::new(),
            verification_command: None,
            workspace_root: None,
            review_manifest: None,
        });
        state.selected_subagent_id = Some(7);

        let snapshot = state.render_snapshot();

        assert_eq!(snapshot.input_buffer(), "draft input");
        assert_eq!(snapshot.cursor_position(), "draft input".len());
        assert_eq!(snapshot.status(), &AppStatus::Streaming);
        assert_eq!(snapshot.history().as_slice(), state.history.as_slice());
        assert_eq!(snapshot.history_display_start(), 1);
        assert_eq!(snapshot.current_response(), "streamed response");
        assert!(snapshot.modal_open());
        let selected = snapshot.selected_subagent().expect("selected subagent");
        assert_eq!(selected.id(), 7);
        assert_eq!(selected.name(), "reviewer");
        assert_eq!(selected.history()[0].content, "subagent response");
        assert_eq!(snapshot.active_history()[0].content, "subagent response");
        assert_eq!(snapshot.active_history_display_start(), 0);
        assert_eq!(selected.status(), SubAgentStatus::Running);
        assert!(selected.active_turn());
        assert_eq!(selected.parent_id(), Some(3));
    }

    #[test]
    fn render_metrics_reject_stale_revision() {
        let mut state = AppState::new();
        let revision = state.render_snapshot().revision();
        let input_area = ratatui::layout::Rect::new(2, 3, 40, 4);

        assert!(state.publish_render_metrics(revision, 12, input_area));
        assert_eq!(state.conversation_content_height, 12);
        assert_eq!(state.input_text_area, Some(input_area));

        state.request_redraw();
        assert!(!state.publish_render_metrics(revision, 99, ratatui::layout::Rect::default()));
        assert_eq!(state.conversation_content_height, 12);
        assert_eq!(state.input_text_area, Some(input_area));
    }
}
