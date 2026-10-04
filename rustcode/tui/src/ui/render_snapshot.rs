use rustcode::controller::{
    AppStatus, ChatMessage, History, LiveToolCall, McpEditState, MonthlyUsage, PendingQuestion,
    RenderState, StreamTracker, SubAgentStatus, SubAgentView, TokenUsage, ToolConfirmation,
    Verbosity,
};
use std::sync::Arc;

/// Immutable data captured for one UI render attempt.
#[allow(dead_code)]
pub(crate) struct RenderSnapshot {
    revision: u64,
    input_buffer: String,
    ctrl_c_exit_armed: bool,
    cursor_position: usize,
    composer_selection_anchor: Option<usize>,
    history: History,
    tool_call_candidates: std::sync::OnceLock<Vec<usize>>,
    history_display_start: usize,
    current_response: Arc<String>,
    recap_loading: bool,
    current_token_usage: Option<TokenUsage>,
    current_turn_token_usage: Option<TokenUsage>,
    current_round_token_usage: Option<TokenUsage>,
    current_round_estimated_input_tokens: u32,
    current_round_estimated_output_tokens: u32,
    current_provider_request_prompt_estimate: u32,
    current_turn_token_usage_is_estimated: bool,
    token_usage_in_flight: bool,
    provider_request_in_flight: bool,
    response_time: Option<std::time::Duration>,
    current_thought_time_ms: u64,
    current_thought_tokens: u32,
    current_thought_started_at: Option<std::time::Instant>,
    model_quota_remaining: Option<f32>,
    provider_rate_limits: Option<rustcode::controller::ProviderRateLimits>,
    pending_queue: Vec<String>,
    pending_steers: Vec<String>,
    draft_submit_mode: rustcode::controller::DraftSubmitMode,
    steering_interruptible: bool,
    steering_escape_will_interrupt: bool,
    status: AppStatus,
    active_suggestion_index: Option<usize>,
    dismissed_completion: Option<String>,
    config: rustcode::controller::AppConfig,
    model_name: String,
    api_base_url: String,
    active_session_id: String,
    cwd_and_branch: String,
    home_path: Option<String>,
    overlay: OverlaySnapshot,
    generation_start_time: Option<std::time::Instant>,
    pending_tool_confirmation: Option<Vec<ToolConfirmation>>,
    pending_question: Option<PendingQuestion>,
    /// Chain position mirrors for the question modal header (`i/N`,
    /// unanswered count) without cloning the whole queue per frame.
    pending_question_chain_len: usize,
    pending_question_chain_position: usize,
    pending_question_chain_answered: usize,
    running_tools: Vec<String>,
    background_tasks: Vec<rustcode::controller::TaskDisplay>,
    waiting_for_background_terminal: bool,
    live_tool_calls: Arc<Vec<LiveToolCall>>,
    stream_tracker: Option<StreamTracker>,
    auto_confirm: bool,
    verbosity: Verbosity,
    delegation_active: bool,
    modal_open: bool,
    user_overlay_open: bool,
    last_copy_text: Option<(String, std::time::Instant)>,
    transient_notice: Option<String>,
    expanded_thoughts: std::collections::HashSet<usize>,
    agent_mode: rustcode::controller::AgentMode,
    subagents: Vec<SubAgentSnapshot>,
    selected_subagent_id: Option<u32>,
    active_context_window: u32,
    active_model_profile: Option<rustcode::controller::ModelProfile>,
    active_tool_protocol: rustcode::controller::ToolProtocol,
    command_suggestion: Option<String>,
    selected_subagent: Option<SelectedSubagentSnapshot>,
}

/// Capture the immutable view the renderer needs for this frame.
///
/// Free function (not a method): the view is a plain controller-owned data
/// type, so nothing in the core library needs to name the rendering types this
/// returns.
pub(crate) fn render_snapshot(view: &RenderState) -> RenderSnapshot {
    RenderSnapshot::new(view)
}

/// Seed a view's streamed-response buffer.
///
/// The view is a read projection: production only ever receives one from
/// `controller::render_state`. Render tests need to stand a frame up at a
/// given response, and the buffer is an `Arc` the snapshot shares, so the
/// helper writes the field the same way a projection would.
#[cfg(test)]
pub(crate) fn set_current_response(view: &mut RenderState, response: impl Into<String>) {
    view.current_response = Arc::new(response.into());
}

/// Data used exclusively by modal overlays. Large collections and editable
/// buffers are captured only while their owning overlay is visible.
struct OverlaySnapshot {
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
    history_picker_sessions: Vec<rustcode::controller::SessionMeta>,
    history_picker_truncated: bool,
    pending_delete_session_idx: Option<usize>,
    show_subagent_picker: bool,
    subagent_picker_index: usize,
    settings_picker: Option<rustcode::controller::SettingsPicker>,
    command_panel: Option<rustcode::controller::CommandPanel>,
    show_context_modal: bool,
    show_status_modal: bool,
    show_stats_modal: bool,
    show_session_modal: bool,
    stats_usage_history: std::collections::BTreeMap<String, MonthlyUsage>,
    show_update_prompt: bool,
    update_check: rustcode_core::update::UpdateState,
    update_prompt_index: usize,
    show_mcp_config: bool,
    mcp_picker_index: usize,
    mcp_edit_state: Option<McpEditState>,
    modal_scroll_row: u16,
    tool_confirmation_selected: usize,
}

impl OverlaySnapshot {
    fn new(view: &RenderState) -> Self {
        Self {
            show_model_picker: view.show_model_picker,
            model_picker_index: view.model_picker_index,
            modal_picker_index: view.modal_picker_index,
            model_picker_search: view.model_picker_search.clone(),
            show_theme_picker: view.show_theme_picker,
            theme_picker_index: view.theme_picker_index,
            theme_picker_initial: view.theme_picker_initial.clone(),
            show_command_picker: view.show_command_picker,
            command_picker_index: view.command_picker_index,
            command_picker_search: view.command_picker_search.clone(),
            show_history_picker: view.show_history_picker,
            history_picker_index: view.history_picker_index,
            history_picker_sessions: view.history_picker_sessions.clone(),
            history_picker_truncated: view.history_picker_truncated,
            pending_delete_session_idx: view.pending_delete_session_idx,
            show_subagent_picker: view.show_subagent_picker,
            subagent_picker_index: view.subagent_picker_index,
            settings_picker: view.settings_picker,
            command_panel: view.command_panel.clone(),
            show_context_modal: view.show_context_modal,
            show_status_modal: view.show_status_modal,
            show_stats_modal: view.show_stats_modal,
            show_session_modal: view.show_session_modal,
            stats_usage_history: view.stats_usage_history.clone(),
            show_update_prompt: view.show_update_prompt,
            update_check: view.update_check,
            update_prompt_index: view.update_prompt_index,
            show_mcp_config: view.show_mcp_config,
            mcp_picker_index: view.mcp_picker_index,
            mcp_edit_state: view.mcp_edit_state.clone(),
            modal_scroll_row: view.modal_scroll_row,
            tool_confirmation_selected: view.tool_confirmation_selected,
        }
    }
}

#[allow(dead_code)]
impl RenderSnapshot {
    pub(crate) fn new(view: &RenderState) -> Self {
        let selected_subagent =
            view.selected_subagent
                .as_ref()
                .map(|agent| SelectedSubagentSnapshot {
                    #[cfg(test)]
                    id: agent.id,
                    name: agent.name.clone(),
                    history: Arc::clone(&agent.history),
                    status: agent.status,
                    active_turn: agent.active_turn,
                    parent_id: agent.parent_id,
                });

        Self {
            revision: view.revision,
            input_buffer: view.input_buffer.clone(),
            ctrl_c_exit_armed: view.ctrl_c_exit_armed,
            cursor_position: view.cursor_position,
            composer_selection_anchor: view.composer_selection_anchor,
            history: view.history.snapshot(),
            tool_call_candidates: std::sync::OnceLock::new(),
            history_display_start: view.history_display_start,
            current_response: Arc::clone(&view.current_response),
            recap_loading: view.recap_loading,
            current_token_usage: view.current_token_usage.clone(),
            current_turn_token_usage: view.current_turn_token_usage.clone(),
            current_round_token_usage: view.current_round_token_usage.clone(),
            current_round_estimated_input_tokens: view.current_round_estimated_input_tokens,
            current_round_estimated_output_tokens: view.current_round_estimated_output_tokens,
            current_provider_request_prompt_estimate: view.current_provider_request_prompt_estimate,
            current_turn_token_usage_is_estimated: view.current_turn_token_usage_is_estimated,
            token_usage_in_flight: view.token_usage_in_flight,
            provider_request_in_flight: view.provider_request_in_flight,
            response_time: view.response_time,
            current_thought_time_ms: view.current_thought_time_ms,
            current_thought_tokens: view.current_thought_tokens,
            current_thought_started_at: view.current_thought_started_at,
            model_quota_remaining: view.model_quota_remaining,
            provider_rate_limits: view.provider_rate_limits.clone(),
            pending_queue: view.pending_queue.clone(),
            pending_steers: view.pending_steers.clone(),
            draft_submit_mode: view.draft_submit_mode,
            steering_interruptible: view.steering_interruptible,
            steering_escape_will_interrupt: view.steering_escape_will_interrupt,
            status: view.status.clone(),
            active_suggestion_index: view.active_suggestion_index,
            dismissed_completion: view.dismissed_completion.clone(),
            config: view.config.clone(),
            model_name: view.model_name.clone(),
            api_base_url: view.api_base_url.clone(),
            active_session_id: view.active_session_id.clone(),
            cwd_and_branch: view.cwd_and_branch.clone(),
            home_path: view.home_path.clone(),
            overlay: OverlaySnapshot::new(view),
            generation_start_time: view.generation_start_time,
            pending_tool_confirmation: view.pending_tool_confirmation.clone(),
            pending_question: view.pending_question.clone(),
            pending_question_chain_len: view.pending_question_chain_len,
            pending_question_chain_position: view.pending_question_chain_position,
            pending_question_chain_answered: view.pending_question_chain_answered,
            running_tools: view.running_tools.clone(),
            background_tasks: view.background_tasks.clone(),
            waiting_for_background_terminal: view.waiting_for_background_terminal,
            live_tool_calls: Arc::clone(&view.live_tool_calls),
            stream_tracker: view.stream_tracker.clone(),
            auto_confirm: view.auto_confirm,
            verbosity: view.verbosity.clone(),
            delegation_active: view.delegation_active,
            modal_open: view.modal_open(),
            user_overlay_open: view.user_overlay_open(),
            last_copy_text: view.last_copy_text.clone(),
            transient_notice: view.transient_notice.clone(),
            expanded_thoughts: view.expanded_thoughts.clone(),
            agent_mode: view.agent_mode,
            subagents: view
                .subagents
                .iter()
                .map(SubAgentSnapshot::new)
                .collect::<Vec<_>>(),
            selected_subagent_id: view.selected_subagent_id,
            active_context_window: view.active_context_window,
            active_model_profile: view.active_model_profile.clone(),
            active_tool_protocol: view.active_tool_protocol,
            command_suggestion: view.command_suggestion.clone(),
            selected_subagent,
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
    pub(crate) fn input_buffer(&self) -> &str {
        &self.input_buffer
    }
    pub(crate) fn ctrl_c_exit_armed(&self) -> bool {
        self.ctrl_c_exit_armed
    }
    pub(crate) fn transient_notice(&self) -> Option<&str> {
        self.transient_notice.as_deref()
    }
    pub(crate) fn cursor_position(&self) -> usize {
        self.cursor_position
    }
    pub(crate) fn composer_selection_range(&self) -> Option<(usize, usize)> {
        let mut anchor = self.composer_selection_anchor?.min(self.input_buffer.len());
        while !self.input_buffer.is_char_boundary(anchor) && anchor > 0 {
            anchor -= 1;
        }
        let mut cursor = self.cursor_position.min(self.input_buffer.len());
        while !self.input_buffer.is_char_boundary(cursor) && cursor > 0 {
            cursor -= 1;
        }
        let (start, end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        (start < end).then_some((start, end))
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
    /// Candidate assistant positions in this immutable displayed revision.
    /// Selection retains the snapshot, so legacy result correlation scans
    /// ordinary history once rather than once per exposed tool result.
    /// Only indices are retained; canonical messages and their ownership stay
    /// unchanged. Delimiters deliberately include tolerant/malformed envelopes.
    pub(crate) fn tool_call_candidate_indices(&self) -> &[usize] {
        self.tool_call_candidates.get_or_init(|| {
            self.active_history()
                .iter()
                .enumerate()
                .filter_map(|(index, message)| {
                    (message.role == "assistant"
                        && (!message.tool_calls.is_empty()
                            || message.content.contains(['{', '[', '<', '`'])))
                    .then_some(index)
                })
                .collect()
        })
    }
    pub(crate) fn active_history_display_start(&self) -> usize {
        if self.selected_subagent.is_some() {
            0
        } else {
            self.history_display_start
        }
    }
    pub(crate) fn recap_loading(&self) -> bool {
        self.recap_loading
    }
    pub(crate) fn current_response(&self) -> &str {
        self.current_response.as_str()
    }
    pub(crate) fn current_token_usage(&self) -> Option<&TokenUsage> {
        self.current_token_usage.as_ref()
    }
    pub(crate) fn current_turn_token_usage(&self) -> Option<&TokenUsage> {
        self.current_turn_token_usage.as_ref()
    }
    pub(crate) fn current_round_token_usage(&self) -> Option<&TokenUsage> {
        self.current_round_token_usage.as_ref()
    }
    pub(crate) fn current_round_estimated_input_tokens(&self) -> u32 {
        self.current_round_estimated_input_tokens
    }
    pub(crate) fn current_round_estimated_output_tokens(&self) -> u32 {
        self.current_round_estimated_output_tokens
    }
    pub(crate) fn current_provider_request_prompt_estimate(&self) -> u32 {
        self.current_provider_request_prompt_estimate
    }
    pub(crate) fn current_turn_token_usage_is_estimated(&self) -> bool {
        self.current_turn_token_usage_is_estimated
    }
    pub(crate) fn token_usage_in_flight(&self) -> bool {
        self.token_usage_in_flight
    }
    pub(crate) fn provider_request_in_flight(&self) -> bool {
        self.provider_request_in_flight
    }
    pub(crate) fn response_time(&self) -> Option<std::time::Duration> {
        self.response_time
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
    pub(crate) fn provider_rate_limits(&self) -> Option<&rustcode::controller::ProviderRateLimits> {
        self.provider_rate_limits.as_ref()
    }
    pub(crate) fn pending_queue(&self) -> &[String] {
        &self.pending_queue
    }
    pub(crate) fn pending_steers(&self) -> &[String] {
        &self.pending_steers
    }
    pub(crate) fn draft_submit_mode(&self) -> rustcode::controller::DraftSubmitMode {
        self.draft_submit_mode
    }
    pub(crate) fn steering_interruptible(&self) -> bool {
        self.steering_interruptible
    }
    pub(crate) fn steering_escape_will_interrupt(&self) -> bool {
        self.steering_escape_will_interrupt
    }
    pub(crate) fn show_steer_mode_hint(&self) -> bool {
        self.steering_interruptible
            && !self.input_buffer.trim().is_empty()
            && rustcode::controller::get_completion_len(&self.input_buffer, self.cursor_position)
                == 0
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
    pub(crate) fn config(&self) -> &rustcode::controller::AppConfig {
        &self.config
    }
    pub(crate) fn model_name(&self) -> &str {
        &self.model_name
    }
    pub(crate) fn api_base_url(&self) -> &str {
        &self.api_base_url
    }
    pub(crate) fn active_session_id(&self) -> &str {
        &self.active_session_id
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
    pub(crate) fn background_tasks(&self) -> &[rustcode::controller::TaskDisplay] {
        &self.background_tasks
    }
    pub(crate) fn waiting_for_background_terminal(&self) -> bool {
        self.waiting_for_background_terminal && !self.background_tasks.is_empty()
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
    pub(crate) fn pending_question_chain_len(&self) -> usize {
        self.pending_question_chain_len
    }
    pub(crate) fn pending_question_chain_position(&self) -> usize {
        self.pending_question_chain_position
    }
    pub(crate) fn pending_question_chain_answered(&self) -> usize {
        self.pending_question_chain_answered
    }
    pub(crate) fn show_model_picker(&self) -> bool {
        self.overlay.show_model_picker
    }
    pub(crate) fn model_picker_index(&self) -> usize {
        self.overlay.model_picker_index
    }
    pub(crate) fn modal_picker_index(&self) -> usize {
        self.overlay.modal_picker_index
    }
    pub(crate) fn model_picker_search(&self) -> &str {
        &self.overlay.model_picker_search
    }
    pub(crate) fn show_theme_picker(&self) -> bool {
        self.overlay.show_theme_picker
    }
    pub(crate) fn theme_picker_index(&self) -> usize {
        self.overlay.theme_picker_index
    }
    pub(crate) fn theme_picker_initial(&self) -> &str {
        &self.overlay.theme_picker_initial
    }
    pub(crate) fn show_command_picker(&self) -> bool {
        self.overlay.show_command_picker
    }
    pub(crate) fn command_picker_index(&self) -> usize {
        self.overlay.command_picker_index
    }
    pub(crate) fn command_picker_search(&self) -> &str {
        &self.overlay.command_picker_search
    }
    pub(crate) fn show_history_picker(&self) -> bool {
        self.overlay.show_history_picker
    }
    pub(crate) fn history_picker_index(&self) -> usize {
        self.overlay.history_picker_index
    }
    pub(crate) fn history_picker_sessions(&self) -> &[rustcode::controller::SessionMeta] {
        &self.overlay.history_picker_sessions
    }
    pub(crate) fn history_picker_truncated(&self) -> bool {
        self.overlay.history_picker_truncated
    }
    pub(crate) fn pending_delete_session_idx(&self) -> Option<usize> {
        self.overlay.pending_delete_session_idx
    }
    pub(crate) fn show_subagent_picker(&self) -> bool {
        self.overlay.show_subagent_picker
    }
    pub(crate) fn subagent_picker_index(&self) -> usize {
        self.overlay.subagent_picker_index
    }
    pub(crate) fn user_overlay_open(&self) -> bool {
        self.user_overlay_open
    }

    pub(crate) fn settings_picker(&self) -> Option<rustcode::controller::SettingsPicker> {
        self.overlay.settings_picker
    }

    pub(crate) fn command_panel(&self) -> Option<&rustcode::controller::CommandPanel> {
        self.overlay.command_panel.as_ref()
    }

    pub(crate) fn show_context_modal(&self) -> bool {
        self.overlay.show_context_modal
    }
    pub(crate) fn show_status_modal(&self) -> bool {
        self.overlay.show_status_modal
    }
    pub(crate) fn show_stats_modal(&self) -> bool {
        self.overlay.show_stats_modal
    }
    pub(crate) fn show_session_modal(&self) -> bool {
        self.overlay.show_session_modal
    }
    pub(crate) fn stats_usage_history(&self) -> &std::collections::BTreeMap<String, MonthlyUsage> {
        &self.overlay.stats_usage_history
    }
    pub(crate) fn show_update_prompt(&self) -> bool {
        self.overlay.show_update_prompt
    }
    pub(crate) fn update_check(&self) -> rustcode_core::update::UpdateState {
        self.overlay.update_check
    }
    pub(crate) fn update_prompt_index(&self) -> usize {
        self.overlay.update_prompt_index
    }
    pub(crate) fn show_mcp_config(&self) -> bool {
        self.overlay.show_mcp_config
    }
    pub(crate) fn mcp_picker_index(&self) -> usize {
        self.overlay.mcp_picker_index
    }
    pub(crate) fn mcp_edit_state(&self) -> Option<&McpEditState> {
        self.overlay.mcp_edit_state.as_ref()
    }
    pub(crate) fn generation_start_time(&self) -> Option<std::time::Instant> {
        self.generation_start_time
    }
    pub(crate) fn modal_scroll_row(&self) -> u16 {
        self.overlay.modal_scroll_row
    }
    pub(crate) fn tool_confirmation_selected(&self) -> usize {
        self.overlay.tool_confirmation_selected
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
    pub(crate) fn agent_mode(&self) -> rustcode::controller::AgentMode {
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
    pub(crate) fn active_model_profile(&self) -> Option<&rustcode::controller::ModelProfile> {
        self.active_model_profile.as_ref()
    }
    pub(crate) fn active_tool_protocol(&self) -> rustcode::controller::ToolProtocol {
        self.active_tool_protocol
    }
    pub(crate) fn auto_confirm_status_text(&self) -> &'static str {
        if self.auto_confirm { "ON" } else { "OFF" }
    }
    pub(crate) fn completion_identity(&self) -> Option<String> {
        if let Some(command) = rustcode::controller::command_token(&self.input_buffer) {
            return Some(format!("command:{command}"));
        }
        rustcode_core::input::get_at_word_query(&self.input_buffer, self.cursor_position)
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
    history_tokens: usize,
    last_message: String,
    pub(crate) status: SubAgentStatus,
    pub(crate) parent_id: Option<u32>,
    pub(crate) model: Option<String>,
    pub(crate) elapsed_ms: u64,
}

impl SubAgentSnapshot {
    fn new(agent: &SubAgentView) -> Self {
        let last_message = agent
            .history
            .last()
            .map(|message| {
                message
                    .content
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(48)
                    .collect()
            })
            .unwrap_or_default();
        let history_tokens = agent
            .history
            .iter()
            .map(rustcode::controller::estimate_message_tokens)
            .sum();
        Self {
            id: agent.id,
            name: agent.name.clone(),
            task: agent.task.clone(),
            history_tokens,
            last_message,
            status: agent.status,
            parent_id: agent.parent_id,
            model: agent.model.clone(),
            elapsed_ms: agent.elapsed_ms,
        }
    }
}

impl SubAgentSnapshot {
    pub(crate) fn history_tokens(&self) -> usize {
        self.history_tokens
    }

    pub(crate) fn last_message(&self) -> &str {
        &self.last_message
    }
}

/// The selected child context rendered in place of the root conversation.
pub(crate) struct SelectedSubagentSnapshot {
    #[cfg(test)]
    id: u32,
    name: String,
    history: Arc<Vec<ChatMessage>>,
    status: SubAgentStatus,
    active_turn: bool,
    parent_id: Option<u32>,
}

impl SelectedSubagentSnapshot {
    #[cfg(test)]
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
    use super::{render_snapshot, set_current_response};
    use rustcode::controller::{
        AppStatus, ChatMessage, RenderState, SubAgentStatus, SubAgentView, TaskDisplay,
    };
    use std::sync::Arc;

    #[test]
    fn tool_candidates_are_lazy_reused_and_isolated_to_displayed_revision() {
        let mut state = RenderState::new();
        state
            .history
            .push(ChatMessage::new("assistant", "plain prose"));
        state
            .history
            .push(ChatMessage::new("assistant", "{name: run_command}"));
        state.history.push(ChatMessage::new("tool", "{output}"));
        let snapshot = render_snapshot(&state);
        assert!(snapshot.tool_call_candidates.get().is_none());
        assert_eq!(snapshot.tool_call_candidate_indices(), &[1]);
        let pointer = snapshot.tool_call_candidate_indices().as_ptr();
        assert_eq!(pointer, snapshot.tool_call_candidate_indices().as_ptr());

        state
            .history
            .replace(vec![ChatMessage::new("assistant", "[TOOL_CALLS]new")]);
        let replaced = render_snapshot(&state);
        assert_eq!(replaced.tool_call_candidate_indices(), &[0]);
        assert_eq!(snapshot.tool_call_candidate_indices(), &[1]);
        state
            .history
            .push(ChatMessage::new("assistant", "```tool new"));
        assert_eq!(
            render_snapshot(&state).tool_call_candidate_indices(),
            &[0, 1]
        );

        state.selected_subagent = Some(subagent(
            9,
            "worker",
            "task",
            vec![ChatMessage::new("assistant", "<tool_call>new")],
            SubAgentStatus::Running,
            true,
            None,
        ));
        let selected = render_snapshot(&state);
        assert_eq!(selected.tool_call_candidate_indices(), &[0]);
        assert_eq!(snapshot.tool_call_candidate_indices(), &[1]);
    }

    fn subagent(
        id: u32,
        name: &str,
        task: &str,
        history: Vec<ChatMessage>,
        status: SubAgentStatus,
        active_turn: bool,
        parent_id: Option<u32>,
    ) -> SubAgentView {
        SubAgentView {
            id,
            name: name.to_owned(),
            task: task.to_owned(),
            history: Arc::new(history),
            status,
            active_turn,
            parent_id,
            model: None,
            elapsed_ms: 0,
        }
    }

    #[test]
    fn render_snapshot_captures_ui_state() {
        let mut state = RenderState::new();
        state.input_buffer = "draft input".to_owned();
        state.cursor_position = state.input_buffer.len();
        state.status = AppStatus::Streaming;
        state.history.push(ChatMessage::new("user", "root message"));
        state.history_display_start = 1;
        set_current_response(&mut state, "streamed response");
        state.show_model_picker = true;
        state.selected_subagent = Some(subagent(
            7,
            "reviewer",
            "review the patch",
            vec![ChatMessage::new("assistant", "subagent response")],
            SubAgentStatus::Running,
            true,
            Some(3),
        ));
        state.selected_subagent_id = Some(7);

        let snapshot = render_snapshot(&state);

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
    fn render_snapshot_captures_live_and_modal_render_data() {
        let mut state = RenderState::new();
        state.pending_queue = vec!["queued prompt".to_owned()];
        state.status = AppStatus::Streaming;
        state.steering_interruptible = true;
        state.pending_steers = vec!["first steer".to_owned(), "second steer".to_owned()];
        state.draft_submit_mode = rustcode::controller::DraftSubmitMode::Queue;
        state.dismissed_completion = Some("command:/help".to_owned());
        state.running_tools = vec!["run_command".to_owned()];
        Arc::make_mut(&mut state.live_tool_calls).push(rustcode::controller::LiveToolCall::new(
            "live",
            None,
            "run_command",
            "Ran",
            "cargo test",
        ));
        state.current_thought_time_ms = 42;
        state.current_thought_tokens = 7;
        state.pending_tool_confirmation = Some(vec![rustcode::controller::ToolConfirmation {
            request_id: None,
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }]);
        state.pending_question = Some(rustcode::controller::PendingQuestion::new(
            "Proceed?".to_owned(),
            vec!["yes".to_owned()],
            false,
        ));

        let snapshot = render_snapshot(&state);

        assert_eq!(snapshot.pending_queue(), ["queued prompt"]);
        assert_eq!(snapshot.pending_steers(), ["first steer", "second steer"]);
        assert_eq!(
            snapshot.draft_submit_mode(),
            rustcode::controller::DraftSubmitMode::Queue
        );
        // The snapshot projects the view's resolved turn flags; the engine
        // decides them (`controller::render_state`), see
        // `pending_confirmation_blocks_steering`.
        assert!(snapshot.steering_interruptible());
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
    fn render_snapshot_reports_steering_interruptibility_from_the_view() {
        let mut state = RenderState::new();
        state.status = AppStatus::Streaming;
        assert!(!render_snapshot(&state).steering_interruptible());

        state.steering_interruptible = true;
        assert!(render_snapshot(&state).steering_interruptible());
    }

    #[test]
    fn render_snapshot_shares_response_storage_and_stays_stable_after_mutation() {
        let mut state = RenderState::new();
        set_current_response(&mut state, "initial response");

        let snapshot = render_snapshot(&state);
        assert!(std::ptr::eq(
            snapshot.current_response().as_ptr(),
            state.current_response.as_str().as_ptr()
        ));

        Arc::make_mut(&mut state.current_response).push_str(" after snapshot");

        assert_eq!(snapshot.current_response(), "initial response");
        assert_eq!(
            state.current_response.as_str(),
            "initial response after snapshot"
        );
    }

    #[test]
    fn render_snapshot_keeps_selected_subagent_and_live_tool_storage_stable() {
        let mut state = RenderState::new();
        state.selected_subagent = Some(subagent(
            7,
            "reviewer",
            "review the patch",
            vec![ChatMessage::new("assistant", "subagent response")],
            SubAgentStatus::Running,
            true,
            None,
        ));
        state.selected_subagent_id = Some(7);
        Arc::make_mut(&mut state.live_tool_calls).push(rustcode::controller::LiveToolCall::new(
            "live",
            None,
            "run_command",
            "Ran",
            "cargo test",
        ));

        let snapshot = render_snapshot(&state);
        let selected = snapshot.selected_subagent().expect("selected subagent");

        assert_eq!(selected.history()[0].content, "subagent response");
        assert_eq!(
            selected.history().as_ptr(),
            state.selected_subagent.as_ref().unwrap().history.as_ptr()
        );

        Arc::make_mut(&mut state.selected_subagent.as_mut().unwrap().history)
            .push(ChatMessage::new("assistant", "later response"));
        assert_eq!(selected.history().len(), 1);
        assert_eq!(state.selected_subagent.as_ref().unwrap().history.len(), 2);
        assert_eq!(
            snapshot.live_tool_calls.as_ptr(),
            state.live_tool_calls.as_ptr()
        );

        Arc::make_mut(&mut state.live_tool_calls)[0]
            .output
            .push_back(rustcode::controller::LiveToolOutputChunk {
                stderr: false,
                text: "new output".into(),
            });
        assert!(snapshot.live_tool_calls()[0].output.is_empty());
        assert_eq!(state.live_tool_calls[0].output[0].text, "new output");
    }

    #[test]
    fn subagent_snapshots_keep_picker_metadata_and_selected_history() {
        let mut state = RenderState::new();
        state.show_subagent_picker = true;
        state.subagents = vec![
            subagent(
                1,
                "background",
                "check the background task",
                vec![ChatMessage::new("assistant", "background result")],
                SubAgentStatus::Completed,
                false,
                None,
            ),
            subagent(
                2,
                "selected",
                "inspect the selected context",
                vec![
                    ChatMessage::new("user", "inspect"),
                    ChatMessage::new("assistant", "selected result"),
                ],
                SubAgentStatus::Running,
                true,
                Some(1),
            ),
        ];
        state.selected_subagent = Some(state.subagents[1].clone());
        state.selected_subagent_id = Some(2);

        let snapshot = render_snapshot(&state);

        assert_eq!(snapshot.subagents()[0].last_message(), "background result");
        assert!(snapshot.subagents()[0].history_tokens() > 0);
        let selected = snapshot.selected_subagent().expect("selected subagent");
        assert_eq!(selected.history().len(), 2);
        assert_eq!(selected.history()[1].content, "selected result");
    }

    // Whether an overlay's payload is captured at all is the engine's call,
    // not the bridge's: `controller::render_state` only clones the session
    // list, the usage history, and the MCP buffer while their overlay is
    // open, so a frame never pays for a modal nobody is looking at. See
    // `controller::tests::render_state_tests::overlay_payloads_are_captured_only_while_their_overlay_is_open`.

    #[test]
    fn render_snapshot_captures_active_overlay_payloads() {
        let mut state = RenderState::new();
        state.show_history_picker = true;
        state.show_mcp_config = true;
        state
            .history_picker_sessions
            .push(rustcode::controller::SessionMeta {
                path: std::path::PathBuf::from("session.json"),
                title: "A session title".to_owned(),
                message_count: 3,
                when: "now".to_owned(),
                workspace_cwd: None,
            });
        state.mcp_edit_state = Some(rustcode::controller::McpEditState {
            is_add: true,
            edit_index: None,
            name_input: "server".to_owned(),
            command_input: "command".to_owned(),
            args_input: "--flag".to_owned(),
            active_field: 0,
            cursor_pos: 0,
        });

        let snapshot = render_snapshot(&state);

        assert_eq!(
            snapshot.history_picker_sessions()[0].title,
            "A session title"
        );
        assert_eq!(
            snapshot
                .mcp_edit_state()
                .map(|edit| edit.name_input.as_str()),
            Some("server")
        );
    }

    #[test]
    fn render_snapshot_reports_background_tasks_from_the_view() {
        let mut state = RenderState::new();
        assert!(snapshot_has_no_tasks(&render_snapshot(&state)));
        assert!(!state.waiting_for_background_terminal);

        state.background_tasks = vec![TaskDisplay {
            id: "task-1".to_owned(),
            command: "cargo test".to_owned(),
            started_at: std::time::Instant::now(),
            child_pid: Some(42),
        }];
        state.waiting_for_background_terminal = true;

        let snapshot = render_snapshot(&state);
        assert_eq!(snapshot.background_tasks().len(), 1);
        assert!(snapshot.waiting_for_background_terminal());
    }

    fn snapshot_has_no_tasks(snapshot: &super::RenderSnapshot) -> bool {
        snapshot.background_tasks().is_empty()
    }
}
