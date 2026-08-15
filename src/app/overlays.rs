use crate::app::state::{AppState, AppStatus};

pub(crate) struct OverlayState<'a> {
    status: &'a mut AppStatus,
    show_model_picker: &'a mut bool,
    show_theme_picker: &'a mut bool,
    show_command_picker: &'a mut bool,
    show_history_picker: &'a mut bool,
    show_mcp_config: &'a mut bool,
    pending_delete_session_idx: &'a mut Option<usize>,
    mcp_edit_state: &'a mut Option<crate::app::state::McpEditState>,
    pending_tool_confirmation: &'a mut Option<Vec<crate::app::state::ToolConfirmation>>,
    pending_question: &'a mut Option<crate::app::state::PendingQuestion>,
}

impl<'a> OverlayState<'a> {
    pub(crate) fn new(state: &'a mut AppState) -> Self {
        Self {
            status: &mut state.status,
            show_model_picker: &mut state.show_model_picker,
            show_theme_picker: &mut state.show_theme_picker,
            show_command_picker: &mut state.show_command_picker,
            show_history_picker: &mut state.show_history_picker,
            show_mcp_config: &mut state.show_mcp_config,
            pending_delete_session_idx: &mut state.pending_delete_session_idx,
            mcp_edit_state: &mut state.mcp_edit_state,
            pending_tool_confirmation: &mut state.pending_tool_confirmation,
            pending_question: &mut state.pending_question,
        }
    }

    pub(crate) fn any_open(&self) -> bool {
        *self.show_model_picker
            || *self.show_theme_picker
            || *self.show_command_picker
            || *self.show_history_picker
            || *self.show_mcp_config
            || self.pending_delete_session_idx.is_some()
            || self.mcp_edit_state.is_some()
            || self.pending_tool_confirmation.is_some()
            || self.pending_question.is_some()
            || matches!(
                *self.status,
                AppStatus::VerbosityPicker | AppStatus::ThinkingPicker | AppStatus::ProtocolPicker
            )
    }

    pub(crate) fn close_all(&mut self) {
        *self.show_model_picker = false;
        *self.show_theme_picker = false;
        *self.show_command_picker = false;
        *self.show_history_picker = false;
        *self.show_mcp_config = false;
        *self.pending_delete_session_idx = None;
        *self.mcp_edit_state = None;
        if matches!(
            *self.status,
            AppStatus::VerbosityPicker | AppStatus::ThinkingPicker | AppStatus::ProtocolPicker
        ) {
            *self.status = AppStatus::Idle;
        }
    }
}

impl AppState {
    pub(crate) fn overlays(&mut self) -> OverlayState<'_> {
        OverlayState::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::OverlayState;
    use crate::app::AppState;

    #[test]
    fn overlay_view_can_close_all_picker_surfaces() {
        let mut state = AppState::new();
        state.show_model_picker = true;
        state.show_command_picker = true;

        {
            let mut overlays = OverlayState::new(&mut state);
            assert!(overlays.any_open());
            overlays.close_all();
        }

        assert!(!state.show_model_picker);
        assert!(!state.show_command_picker);
    }
}
