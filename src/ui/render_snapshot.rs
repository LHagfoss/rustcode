use crate::app::{
    AppStatus, ChatMessage, History, LiveToolCall, McpEditState, PendingQuestion, StreamTracker,
    SubAgent, SubAgentStatus, TokenUsage, ToolConfirmation, Verbosity,
};
use std::sync::Arc;

/// Immutable data captured for one UI render attempt.
#[allow(dead_code)]
pub(crate) struct RenderSnapshot {
    revision: u64,
    input_buffer: String,
    cursor_position: usize,
    history: History,
    history_display_start: usize,
    current_response: Arc<String>,
    current_token_usage: Option<TokenUsage>,
    current_thought_time_ms: u64,
    current_thought_tokens: u32,
    current_thought_started_at: Option<std::time::Instant>,
    model_quota_remaining: Option<f32>,
    pending_queue: Vec<String>,
    status: AppStatus,
    active_suggestion_index: Option<usize>,
    dismissed_completion: Option<String>,
    config: crate::config::AppConfig,
    model_name: String,
    api_base_url: String,
    cwd_and_branch: String,
    home_path: Option<String>,
    show_model_picker: bool,
    model_picker_index: usize,
    modal_picker_index: usize,
    model_picker_search: String,
    show_theme_picker: bool,
    theme_picker_index: usize,
    theme_picker_initial: String,
    show_command_picker: bool,
    command_picker_index: usize,
    command_picker_search: String,
    show_history_picker: bool,
    history_picker_index: usize,
    history_picker_sessions: Vec<crate::config::SessionMeta>,
    history_picker_truncated: bool,
    pending_delete_session_idx: Option<usize>,
    show_subagent_picker: bool,
    subagent_picker_index: usize,
    show_context_modal: bool,
    show_update_prompt: bool,
    update_check: crate::update::UpdateState,
    update_prompt_index: usize,
    show_mcp_config: bool,
    mcp_picker_index: usize,
    mcp_edit_state: Option<McpEditState>,
    generation_start_time: Option<std::time::Instant>,
    pending_tool_confirmation: Option<Vec<ToolConfirmation>>,
    modal_scroll_row: u16,
    tool_confirmation_selected: usize,
    pending_question: Option<PendingQuestion>,
    running_tools: Vec<String>,
    live_tool_calls: Arc<Vec<LiveToolCall>>,
    stream_tracker: Option<StreamTracker>,
    auto_confirm: bool,
    verbosity: Verbosity,
    delegation_active: bool,
    modal_open: bool,
    last_copy_text: Option<(String, std::time::Instant)>,
    expanded_thoughts: std::collections::HashSet<usize>,
    agent_mode: crate::config::AgentMode,
    subagents: Vec<SubAgentSnapshot>,
    selected_subagent_id: Option<u32>,
    active_context_window: u32,
    active_model_profile: Option<crate::config::ModelProfile>,
    active_tool_protocol: crate::config::ToolProtocol,
    command_suggestion: Option<String>,
    selected_subagent: Option<SelectedSubagentSnapshot>,
}

#[allow(dead_code)]
impl RenderSnapshot {
    pub(crate) fn new(state: &crate::app::AppState) -> Self {
        let subagents = state
            .subagents
            .iter()
            .map(SubAgentSnapshot::from)
            .collect::<Vec<_>>();
        let selected_subagent = state.selected_subagent_id.and_then(|id| {
            let agent = state.subagents.iter().find(|agent| agent.id == id)?;
            let history = subagents
                .iter()
                .find(|snapshot| snapshot.id == id)
                .map(|snapshot| Arc::clone(&snapshot.history))?;
            Some(SelectedSubagentSnapshot {
                id: agent.id,
                name: agent.name.clone(),
                history,
                status: agent.status,
                active_turn: agent.active_turn,
                parent_id: agent.parent_id,
            })
        });

        Self {
            revision: state.render_revision,
            input_buffer: state.input_buffer.clone(),
            cursor_position: state.cursor_position,
            history: state.history.snapshot(),
            history_display_start: state.history_display_start,
            current_response: Arc::clone(&state.current_response),
            current_token_usage: state.current_token_usage.clone(),
            current_thought_time_ms: state.current_thought_time_ms,
            current_thought_tokens: state.current_thought_tokens,
            current_thought_started_at: state.current_thought_started_at,
            model_quota_remaining: state.model_quota_remaining,
            pending_queue: state.pending_queue.clone(),
            status: state.status.clone(),
            active_suggestion_index: state.active_suggestion_index,
            dismissed_completion: state.dismissed_completion.clone(),
            config: state.config.clone(),
            model_name: state.model_name.clone(),
            api_base_url: state.api_base_url.clone(),
            cwd_and_branch: state.cwd_and_branch.clone(),
            home_path: std::env::var("HOME").ok(),
            show_model_picker: state.show_model_picker,
            model_picker_index: state.model_picker_index,
            modal_picker_index: state.modal_picker_index,
            model_picker_search: state.model_picker_search.clone(),
            show_theme_picker: state.show_theme_picker,
            theme_picker_index: state.theme_picker_index,
            theme_picker_initial: state.theme_picker_initial.clone(),
            show_command_picker: state.show_command_picker,
            command_picker_index: state.command_picker_index,
            command_picker_search: state.command_picker_search.clone(),
            show_history_picker: state.show_history_picker,
            history_picker_index: state.history_picker_index,
            history_picker_sessions: state.history_picker_sessions.clone(),
            history_picker_truncated: state.history_picker_truncated,
            pending_delete_session_idx: state.pending_delete_session_idx,
            show_subagent_picker: state.show_subagent_picker,
            subagent_picker_index: state.subagent_picker_index,
            show_context_modal: state.show_context_modal,
            show_update_prompt: state.show_update_prompt,
            update_check: state.update_check,
            update_prompt_index: state.update_prompt_index,
            show_mcp_config: state.show_mcp_config,
            mcp_picker_index: state.mcp_picker_index,
            mcp_edit_state: state.mcp_edit_state.clone(),
            generation_start_time: state.generation_start_time,
            pending_tool_confirmation: state.pending_tool_confirmation.clone(),
            modal_scroll_row: state.modal_scroll_row,
            tool_confirmation_selected: state.tool_confirmation_selected,
            pending_question: state.pending_question.clone(),
            running_tools: state.running_tools.clone(),
            live_tool_calls: Arc::clone(&state.live_tool_calls),
            stream_tracker: state.stream_tracker.clone(),
            auto_confirm: state.auto_confirm,
            verbosity: state.verbosity.clone(),
            delegation_active: state.delegation_active,
            modal_open: state.modal_open(),
            last_copy_text: state.last_copy_text.clone(),
            expanded_thoughts: state.expanded_thoughts.clone(),
            agent_mode: state.agent_mode,
            subagents,
            selected_subagent_id: state.selected_subagent_id,
            active_context_window: state.active_context_window(),
            active_model_profile: state.active_model_profile(),
            active_tool_protocol: state.active_tool_protocol(),
            command_suggestion: state.get_command_suggestion(),
            selected_subagent,
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn input_buffer(&self) -> &str {
        &self.input_buffer
    }
    pub(crate) fn cursor_position(&self) -> usize {
        self.cursor_position
    }
    pub(crate) fn history(&self) -> &History {
        &self.history
    }
    pub(crate) fn history_display_start(&self) -> usize {
        self.history_display_start
    }
    pub(crate) fn active_history(&self) -> &[ChatMessage] {
        self.selected_subagent
            .as_ref()
            .map(|agent| agent.history())
            .unwrap_or_else(|| self.history.as_slice())
    }
    pub(crate) fn active_history_display_start(&self) -> usize {
        if self.selected_subagent.is_some() {
            0
        } else {
            self.history_display_start
        }
    }
    pub(crate) fn current_response(&self) -> &str {
        self.current_response.as_str()
    }
    pub(crate) fn current_token_usage(&self) -> Option<&TokenUsage> {
        self.current_token_usage.as_ref()
    }
    pub(crate) fn current_thought_time_ms(&self) -> u64 {
        self.current_thought_time_ms
    }
    pub(crate) fn current_thought_tokens(&self) -> u32 {
        self.current_thought_tokens
    }
    pub(crate) fn current_thought_started_at(&self) -> Option<std::time::Instant> {
        self.current_thought_started_at
    }
    pub(crate) fn model_quota_remaining(&self) -> Option<f32> {
        self.model_quota_remaining
    }
    pub(crate) fn pending_queue(&self) -> &[String] {
        &self.pending_queue
    }
    pub(crate) fn status(&self) -> &AppStatus {
        &self.status
    }
    pub(crate) fn active_suggestion_index(&self) -> Option<usize> {
        self.active_suggestion_index
    }
    pub(crate) fn dismissed_completion(&self) -> Option<&str> {
        self.dismissed_completion.as_deref()
    }
    pub(crate) fn config(&self) -> &crate::config::AppConfig {
        &self.config
    }
    pub(crate) fn model_name(&self) -> &str {
        &self.model_name
    }
    pub(crate) fn api_base_url(&self) -> &str {
        &self.api_base_url
    }
    pub(crate) fn cwd_and_branch(&self) -> &str {
        &self.cwd_and_branch
    }
    pub(crate) fn home_path(&self) -> Option<&str> {
        self.home_path.as_deref()
    }
    pub(crate) fn running_tools(&self) -> &[String] {
        &self.running_tools
    }
    pub(crate) fn live_tool_calls(&self) -> &[LiveToolCall] {
        &self.live_tool_calls
    }
    pub(crate) fn pending_tool_confirmation(&self) -> Option<&[ToolConfirmation]> {
        self.pending_tool_confirmation.as_deref()
    }
    pub(crate) fn pending_question(&self) -> Option<&PendingQuestion> {
        self.pending_question.as_ref()
    }
    pub(crate) fn show_model_picker(&self) -> bool {
        self.show_model_picker
    }
    pub(crate) fn model_picker_index(&self) -> usize {
        self.model_picker_index
    }
    pub(crate) fn modal_picker_index(&self) -> usize {
        self.modal_picker_index
    }
    pub(crate) fn model_picker_search(&self) -> &str {
        &self.model_picker_search
    }
    pub(crate) fn show_theme_picker(&self) -> bool {
        self.show_theme_picker
    }
    pub(crate) fn theme_picker_index(&self) -> usize {
        self.theme_picker_index
    }
    pub(crate) fn theme_picker_initial(&self) -> &str {
        &self.theme_picker_initial
    }
    pub(crate) fn show_command_picker(&self) -> bool {
        self.show_command_picker
    }
    pub(crate) fn command_picker_index(&self) -> usize {
        self.command_picker_index
    }
    pub(crate) fn command_picker_search(&self) -> &str {
        &self.command_picker_search
    }
    pub(crate) fn show_history_picker(&self) -> bool {
        self.show_history_picker
    }
    pub(crate) fn history_picker_index(&self) -> usize {
        self.history_picker_index
    }
    pub(crate) fn history_picker_sessions(&self) -> &[crate::config::SessionMeta] {
        &self.history_picker_sessions
    }
    pub(crate) fn history_picker_truncated(&self) -> bool {
        self.history_picker_truncated
    }
    pub(crate) fn pending_delete_session_idx(&self) -> Option<usize> {
        self.pending_delete_session_idx
    }
    pub(crate) fn show_subagent_picker(&self) -> bool {
        self.show_subagent_picker
    }
    pub(crate) fn subagent_picker_index(&self) -> usize {
        self.subagent_picker_index
    }
    pub(crate) fn show_context_modal(&self) -> bool {
        self.show_context_modal
    }
    pub(crate) fn show_update_prompt(&self) -> bool {
        self.show_update_prompt
    }
    pub(crate) fn update_check(&self) -> crate::update::UpdateState {
        self.update_check
    }
    pub(crate) fn update_prompt_index(&self) -> usize {
        self.update_prompt_index
    }
    pub(crate) fn show_mcp_config(&self) -> bool {
        self.show_mcp_config
    }
    pub(crate) fn mcp_picker_index(&self) -> usize {
        self.mcp_picker_index
    }
    pub(crate) fn mcp_edit_state(&self) -> Option<&McpEditState> {
        self.mcp_edit_state.as_ref()
    }
    pub(crate) fn generation_start_time(&self) -> Option<std::time::Instant> {
        self.generation_start_time
    }
    pub(crate) fn modal_scroll_row(&self) -> u16 {
        self.modal_scroll_row
    }
    pub(crate) fn tool_confirmation_selected(&self) -> usize {
        self.tool_confirmation_selected
    }
    pub(crate) fn stream_tracker(&self) -> Option<&StreamTracker> {
        self.stream_tracker.as_ref()
    }
    pub(crate) fn auto_confirm(&self) -> bool {
        self.auto_confirm
    }
    pub(crate) fn verbosity(&self) -> &Verbosity {
        &self.verbosity
    }
    pub(crate) fn delegation_active(&self) -> bool {
        self.delegation_active
    }
    pub(crate) fn modal_open(&self) -> bool {
        self.modal_open
    }
    pub(crate) fn selected_subagent(&self) -> Option<&SelectedSubagentSnapshot> {
        self.selected_subagent.as_ref()
    }
    pub(crate) fn last_copy_text(&self) -> Option<&(String, std::time::Instant)> {
        self.last_copy_text.as_ref()
    }
    pub(crate) fn expanded_thoughts(&self) -> &std::collections::HashSet<usize> {
        &self.expanded_thoughts
    }
    pub(crate) fn agent_mode(&self) -> crate::config::AgentMode {
        self.agent_mode
    }
    pub(crate) fn subagents(&self) -> &[SubAgentSnapshot] {
        &self.subagents
    }
    pub(crate) fn selected_subagent_id(&self) -> Option<u32> {
        self.selected_subagent_id
    }
    pub(crate) fn active_context_window(&self) -> u32 {
        self.active_context_window
    }
    pub(crate) fn active_model_profile(&self) -> Option<&crate::config::ModelProfile> {
        self.active_model_profile.as_ref()
    }
    pub(crate) fn active_tool_protocol(&self) -> crate::config::ToolProtocol {
        self.active_tool_protocol
    }
    pub(crate) fn auto_confirm_status_text(&self) -> &'static str {
        if self.auto_confirm { "ON" } else { "OFF" }
    }
    pub(crate) fn completion_identity(&self) -> Option<String> {
        if let Some(command) = crate::app::suggestion::command_token(&self.input_buffer) {
            return Some(format!("command:{command}"));
        }
        crate::app::get_at_word_query(&self.input_buffer, self.cursor_position)
            .map(|(start, query)| format!("file:{start}:{query}"))
    }
    pub(crate) fn get_command_suggestion(&self) -> Option<&str> {
        self.command_suggestion.as_deref()
    }
}

#[derive(Clone)]
pub(crate) struct SubAgentSnapshot {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) task: String,
    pub(crate) history: Arc<Vec<ChatMessage>>,
    pub(crate) status: SubAgentStatus,
}

impl From<&SubAgent> for SubAgentSnapshot {
    fn from(agent: &SubAgent) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            task: agent.task.clone(),
            history: Arc::new(agent.history.clone()),
            status: agent.status,
        }
    }
}

/// The selected child context rendered in place of the root conversation.
pub(crate) struct SelectedSubagentSnapshot {
    id: u32,
    name: String,
    history: Arc<Vec<ChatMessage>>,
    status: SubAgentStatus,
    active_turn: bool,
    parent_id: Option<u32>,
}

impl SelectedSubagentSnapshot {
    pub(crate) fn id(&self) -> u32 {
        self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    pub(crate) fn history(&self) -> &[ChatMessage] {
        &self.history
    }
    pub(crate) fn status(&self) -> SubAgentStatus {
        self.status
    }
    pub(crate) fn active_turn(&self) -> bool {
        self.active_turn
    }
    pub(crate) fn parent_id(&self) -> Option<u32> {
        self.parent_id
    }
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
        state.replace_current_response("streamed response");
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

    #[test]
    fn render_snapshot_shares_response_storage_and_stays_stable_after_mutation() {
        let mut state = AppState::new();
        state.append_current_response("initial response");

        let snapshot = state.render_snapshot();
        assert!(std::ptr::eq(
            snapshot.current_response().as_ptr(),
            state.current_response.as_str().as_ptr()
        ));

        state.append_current_response(" after snapshot");

        assert_eq!(snapshot.current_response(), "initial response");
        assert_eq!(
            state.current_response.as_str(),
            "initial response after snapshot"
        );
    }

    #[test]
    fn response_mutations_invalidate_render_metrics() {
        let mut state = AppState::new();

        let append_revision = state.render_snapshot().revision();
        state.append_current_response("streamed output");
        assert_eq!(state.current_response.as_str(), "streamed output");
        assert!(!state.publish_render_metrics(
            append_revision,
            12,
            ratatui::layout::Rect::default()
        ));

        let clear_revision = state.render_snapshot().revision();
        state.clear_current_response();
        assert!(state.current_response.is_empty());
        assert!(!state.publish_render_metrics(
            clear_revision,
            12,
            ratatui::layout::Rect::default()
        ));
    }

    #[test]
    fn render_snapshot_captures_live_and_modal_render_data() {
        let mut state = AppState::new();
        state.pending_queue = vec!["queued prompt".to_owned()];
        state.dismissed_completion = Some("command:/help".to_owned());
        state.running_tools = vec!["run_command".to_owned()];
        std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
            "live",
            None,
            "run_command",
            "Ran",
            "cargo test",
        ));
        state.current_thought_time_ms = 42;
        state.current_thought_tokens = 7;
        state.pending_tool_confirmation = Some(vec![crate::app::ToolConfirmation {
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
        }]);
        state.pending_question = Some(crate::app::PendingQuestion::new(
            "Proceed?".to_owned(),
            vec!["yes".to_owned()],
            false,
        ));

        let snapshot = state.render_snapshot();

        assert_eq!(snapshot.pending_queue(), ["queued prompt"]);
        assert_eq!(snapshot.dismissed_completion(), Some("command:/help"));
        assert_eq!(snapshot.running_tools(), ["run_command"]);
        assert_eq!(snapshot.live_tool_calls()[0].target, "cargo test");
        assert_eq!(snapshot.current_thought_time_ms(), 42);
        assert_eq!(snapshot.current_thought_tokens(), 7);
        assert_eq!(
            snapshot.pending_tool_confirmation().unwrap()[0].tool_name,
            "run_command"
        );
        assert_eq!(snapshot.pending_question().unwrap().question, "Proceed?");
    }

    #[test]
    fn render_snapshot_shares_selected_subagent_and_live_tool_storage() {
        let mut state = AppState::new();
        state.subagents.push(SubAgent {
            id: 7,
            name: "reviewer".to_owned(),
            task: "review the patch".to_owned(),
            model: None,
            history: vec![ChatMessage::new("assistant", "subagent response")],
            status: SubAgentStatus::Running,
            active_turn: true,
            parent_id: None,
            write_access: false,
            allowed_paths: Vec::new(),
            verification_command: None,
            workspace_root: None,
            review_manifest: None,
        });
        state.selected_subagent_id = Some(7);
        std::sync::Arc::make_mut(&mut state.live_tool_calls).push(crate::app::LiveToolCall::new(
            "live",
            None,
            "run_command",
            "Ran",
            "cargo test",
        ));

        let snapshot = state.render_snapshot();
        let selected = snapshot.selected_subagent().expect("selected subagent");

        assert_eq!(
            snapshot.subagents()[0].history.as_ptr(),
            selected.history.as_ptr()
        );
        assert_eq!(
            snapshot.live_tool_calls.as_ptr(),
            state.live_tool_calls.as_ptr()
        );

        state.append_live_tool_output("live", b"new output", false);
        assert!(snapshot.live_tool_calls()[0].output.is_empty());
        assert_eq!(state.live_tool_calls[0].output[0].text, "new output");
    }

    #[test]
    fn input_and_cursor_mutations_invalidate_render_metrics() {
        let mut state = AppState::new();
        let input_revision = state.render_snapshot().revision();
        state.insert_char('x');
        assert!(!state.publish_render_metrics(input_revision, 1, ratatui::layout::Rect::default()));

        let cursor_revision = state.render_snapshot().revision();
        state.move_cursor_to_start();
        assert!(!state.publish_render_metrics(
            cursor_revision,
            1,
            ratatui::layout::Rect::default()
        ));

        state.pending_queue.push("queued".to_owned());
        let recall_revision = state.render_snapshot().revision();
        state.composer().pop_queued_prompt();
        assert!(!state.publish_render_metrics(
            recall_revision,
            1,
            ratatui::layout::Rect::default()
        ));

        state.input_buffer = "/he".to_owned();
        let autocomplete_revision = state.render_snapshot().revision();
        crate::app::actions::apply_autocomplete(&mut state);
        assert!(!state.publish_render_metrics(
            autocomplete_revision,
            1,
            ratatui::layout::Rect::default()
        ));
    }
}
