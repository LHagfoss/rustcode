use crate::app::events::Overlay;
use crate::app::state::{AppState, AppStatus};

pub(crate) struct OverlayState<'a> {
    status: &'a mut AppStatus,
    show_model_picker: &'a mut bool,
    show_theme_picker: &'a mut bool,
    show_command_picker: &'a mut bool,
    show_history_picker: &'a mut bool,
    show_subagent_picker: &'a mut bool,
    show_mcp_config: &'a mut bool,
    pending_delete_session_idx: &'a mut Option<usize>,
    mcp_edit_state: &'a mut Option<crate::app::state::McpEditState>,
    pending_tool_confirmation: &'a mut Option<Vec<crate::app::state::ToolConfirmation>>,
    tool_confirmation_selected: &'a mut usize,
    auto_confirm: &'a mut bool,
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
            show_subagent_picker: &mut state.show_subagent_picker,
            show_mcp_config: &mut state.show_mcp_config,
            pending_delete_session_idx: &mut state.pending_delete_session_idx,
            mcp_edit_state: &mut state.mcp_edit_state,
            pending_tool_confirmation: &mut state.pending_tool_confirmation,
            tool_confirmation_selected: &mut state.tool_confirmation_selected,
            auto_confirm: &mut state.auto_confirm,
            pending_question: &mut state.pending_question,
        }
    }

    pub(crate) fn any_open(&self) -> bool {
        *self.show_model_picker
            || *self.show_theme_picker
            || *self.show_command_picker
            || *self.show_history_picker
            || *self.show_subagent_picker
            || *self.show_mcp_config
            || self.pending_delete_session_idx.is_some()
            || self.mcp_edit_state.is_some()
            || self.pending_tool_confirmation.is_some()
            || self.pending_question.is_some()
            || matches!(
                *self.status,
                AppStatus::VerbosityPicker
                    | AppStatus::ThinkingPicker
                    | AppStatus::EffortPicker
                    | AppStatus::ProtocolPicker
            )
    }

    pub(crate) fn close_all(&mut self) {
        *self.show_model_picker = false;
        *self.show_theme_picker = false;
        *self.show_command_picker = false;
        *self.show_history_picker = false;
        *self.show_subagent_picker = false;
        *self.show_mcp_config = false;
        *self.pending_delete_session_idx = None;
        *self.mcp_edit_state = None;
        if matches!(
            *self.status,
            AppStatus::VerbosityPicker
                | AppStatus::ThinkingPicker
                | AppStatus::EffortPicker
                | AppStatus::ProtocolPicker
        ) {
            *self.status = AppStatus::Idle;
        }
    }

    pub(crate) fn open(&mut self, overlay: Overlay) {
        match overlay {
            Overlay::CommandPalette => *self.show_command_picker = true,
            Overlay::History => *self.show_history_picker = true,
            Overlay::Subagents => *self.show_subagent_picker = true,
            Overlay::Model => *self.show_model_picker = true,
            Overlay::Theme => *self.show_theme_picker = true,
            Overlay::McpConfig => *self.show_mcp_config = true,
            Overlay::Verbosity => *self.status = AppStatus::VerbosityPicker,
            Overlay::Thinking => *self.status = AppStatus::ThinkingPicker,
            Overlay::Effort => *self.status = AppStatus::EffortPicker,
            Overlay::Protocol => *self.status = AppStatus::ProtocolPicker,
            Overlay::ToolConfirmation => {
                if self.pending_tool_confirmation.is_some() {
                    *self.status = AppStatus::AwaitingToolConfirmation;
                }
            }
            Overlay::Question => {
                if self.pending_question.is_some() {
                    *self.status = AppStatus::AwaitingQuestion;
                }
            }
        }
    }

    pub(crate) fn approval_selected(&self) -> usize {
        *self.tool_confirmation_selected
    }

    pub(crate) fn move_approval_selection(&mut self, direction: i8) {
        *self.tool_confirmation_selected = if direction < 0 { 0 } else { 1 };
    }

    pub(crate) fn toggle_auto_confirm(&mut self) {
        *self.auto_confirm = !*self.auto_confirm;
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
    use crate::app::events::Overlay;
    use crate::app::{AppState, AppStatus};

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

    #[test]
    fn overlay_view_owns_approval_selection_and_opening() {
        let mut state = AppState::new();
        state.pending_tool_confirmation = Some(Vec::new());

        {
            let mut overlays = OverlayState::new(&mut state);
            overlays.open(Overlay::ToolConfirmation);
            overlays.move_approval_selection(1);
            overlays.toggle_auto_confirm();
            assert_eq!(overlays.approval_selected(), 1);
        }

        assert_eq!(state.status, AppStatus::AwaitingToolConfirmation);
        assert!(state.auto_confirm);
    }
}
