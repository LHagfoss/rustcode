//! Owned, narrow view of everything a frontend needs to render one frame.
//!
//! This is the last convergence step of the TUI -> controller seam (issues
//! #1442 / #1431). The render layer used to read `rustcode::app::AppState`
//! directly: the snapshot bridge copied ~79 fields out of it, and the
//! frontend allow-list had to keep `rustcode::app` open because the render
//! tests also built an `AppState` to seed a frame. Frontends now render from
//! this view instead, so the render layer never names `AppState`.
//!
//! Shape: flat, one field per value the render layer reads, named after the
//! `AppState` field (or the `RenderSnapshot` accessor) it comes from so the
//! projection stays a mechanical copy. Three deliberate choices:
//!
//! * **Derived values are resolved here, not by the frontend.** Anything that
//!   needed an `AppState` method -- `ctrl_c_exit_armed`, `can_accept_steer`,
//!   the question-chain counters, the active model profile / context window /
//!   tool protocol, the command suggestion, the unexpired transient notice --
//!   is computed once per frame in [`render_state`] and stored pre-computed. A
//!   frontend that re-derived them would be a second copy of engine policy.
//!   [`RenderState::modal_open`] is the one exception: it is a pure function
//!   of overlay flags the view already carries, so deriving it on the view
//!   keeps a hand-built frame coherent.
//! * **Queued steers carry their text only.** The composer renders
//!   `pending_steers` as a list of strings; `session_id` is turn bookkeeping.
//! * **Overlay payloads are captured only while their overlay is visible**,
//!   preserving the bridge's allocation behaviour: opening a frame must not
//!   clone the session list, the usage history, or the MCP edit buffer for a
//!   modal nobody is looking at.
//!
//! Frontends that synthesize a view rather than projecting a live session
//! (render/golden tests today) build it with [`RenderState::new`], which is
//! exactly `render_state(&AppState::new())` so a seeded frame can never drift
//! from a real one.

use std::sync::Arc;

use super::{
    AgentMode, AppConfig, AppStatus, ChatMessage, DraftSubmitMode, History, LiveToolCall,
    McpEditState, ModelProfile, MonthlyUsage, PendingQuestion, SessionMeta, StreamTracker,
    SubAgentStatus, TaskDisplay, TokenUsage, ToolConfirmation, ToolProtocol, Verbosity,
};
use crate::app::AppState;

/// The render-visible projection of one subagent.
///
/// Narrower than the engine's `SubAgent`: the picker and context modal read
/// only these fields, so execution details (allowed paths, workspace root,
/// review manifest) never reach a frontend. `history` is an `Arc` so the
/// per-frame projection is O(1) per agent rather than a transcript copy.
#[derive(Clone)]
pub struct SubAgentView {
    pub id: u32,
    pub name: String,
    pub task: String,
    pub history: Arc<Vec<ChatMessage>>,
    pub status: SubAgentStatus,
    pub active_turn: bool,
    pub parent_id: Option<u32>,
    pub model: Option<String>,
    pub elapsed_ms: u64,
}

impl From<&crate::app::SubAgent> for SubAgentView {
    fn from(agent: &crate::app::SubAgent) -> Self {
        Self {
            id: agent.id,
            name: agent.name.clone(),
            task: agent.task.clone(),
            history: Arc::clone(&agent.history),
            status: agent.status,
            active_turn: agent.active_turn,
            parent_id: agent.parent_id,
            model: agent.model.clone(),
            elapsed_ms: agent
                .finished_at_ms
                .unwrap_or_else(crate::app::subagent_controller::now_ms)
                .saturating_sub(agent.created_at_ms),
        }
    }
}

/// One frame's worth of render-visible session state.
///
/// Every field is public because a frontend renders from the view and tests
/// seed it directly; nothing here is a mutation API. `AppState` stays the
/// engine's own mutable session, and this is the read-only projection of it.
pub struct RenderState {
    // --- composer and turn ------------------------------------------------
    /// Monotonic version of render-visible state; a frame may publish its
    /// layout metrics only while this is unchanged.
    pub revision: u64,
    pub status: AppStatus,
    pub input_buffer: String,
    pub cursor_position: usize,
    pub composer_selection_anchor: Option<usize>,
    /// True while a second Ctrl+C confirms application exit.
    pub ctrl_c_exit_armed: bool,
    pub draft_submit_mode: DraftSubmitMode,
    /// Whether the active turn accepts a steer right now.
    pub steering_interruptible: bool,
    /// Whether Escape interrupts the turn rather than closing a completion,
    /// a modal, or a selection.
    pub steering_escape_will_interrupt: bool,
    pub active_suggestion_index: Option<usize>,
    pub dismissed_completion: Option<String>,
    /// Completion suffix for the token under the cursor, if any.
    pub command_suggestion: Option<String>,

    // --- conversation -----------------------------------------------------
    pub history: History,
    pub history_display_start: usize,
    pub current_response: Arc<String>,
    pub recap_loading: bool,
    pub current_token_usage: Option<TokenUsage>,
    pub current_turn_token_usage: Option<TokenUsage>,
    pub current_round_token_usage: Option<TokenUsage>,
    pub current_round_estimated_input_tokens: u32,
    pub current_round_estimated_output_tokens: u32,
    pub current_provider_request_prompt_estimate: u32,
    pub current_turn_token_usage_is_estimated: bool,
    pub token_usage_in_flight: bool,
    pub provider_request_in_flight: bool,
    pub response_time: Option<std::time::Duration>,
    pub current_thought_time_ms: u64,
    pub current_thought_tokens: u32,
    pub current_thought_started_at: Option<std::time::Instant>,
    pub model_quota_remaining: Option<f32>,
    pub provider_rate_limits: Option<crate::provider_auth::ProviderRateLimits>,
    pub generation_start_time: Option<std::time::Instant>,
    pub pending_queue: Vec<String>,
    /// Text of each steer queued during the active turn.
    pub pending_steers: Vec<String>,
    pub pending_tool_confirmation: Option<Vec<ToolConfirmation>>,
    pub pending_question: Option<PendingQuestion>,
    pub pending_question_chain_len: usize,
    pub pending_question_chain_position: usize,
    pub pending_question_chain_answered: usize,
    pub running_tools: Vec<String>,
    pub live_tool_calls: Arc<Vec<LiveToolCall>>,
    pub stream_tracker: Option<StreamTracker>,
    pub expanded_thoughts: std::collections::HashSet<usize>,
    pub last_copy_text: Option<(String, std::time::Instant)>,
    /// Transient notice, already filtered for expiry.
    pub transient_notice: Option<String>,
    pub verbosity: Verbosity,
    pub delegation_active: bool,
    pub auto_confirm: bool,
    pub agent_mode: AgentMode,

    // --- session ----------------------------------------------------------
    pub config: AppConfig,
    pub model_name: String,
    pub api_base_url: String,
    pub active_session_id: String,
    pub cwd_and_branch: String,
    pub home_path: Option<String>,
    pub active_context_window: u32,
    pub active_model_profile: Option<ModelProfile>,
    pub active_tool_protocol: ToolProtocol,

    // --- background tasks -------------------------------------------------
    pub background_tasks: Vec<TaskDisplay>,
    pub pending_background_results: Vec<super::BackgroundResultDisplay>,
    /// True while a turn is waiting on a background task to reach a terminal
    /// state.
    pub waiting_for_background_terminal: bool,

    // --- subagents --------------------------------------------------------
    /// Subagent rows for the picker / context modal. Populated only while one
    /// of those surfaces is open.
    pub subagents: Vec<SubAgentView>,
    /// The subagent whose conversation replaces the root one, resolved even
    /// when no picker surface is open (the transcript renders it either way).
    pub selected_subagent: Option<SubAgentView>,
    pub selected_subagent_id: Option<u32>,

    // --- overlays ---------------------------------------------------------
    // Indexes and scroll offsets are cheap and always kept; the payloads
    // below (picker searches, session list, usage history, MCP buffer) are
    // captured only while their overlay is open.
    pub show_model_picker: bool,
    pub model_picker_index: usize,
    pub modal_picker_index: usize,
    pub model_picker_search: String,
    pub model_picker_search_cursor: usize,
    pub show_theme_picker: bool,
    pub theme_picker_index: usize,
    pub theme_picker_initial: String,
    pub show_command_picker: bool,
    pub command_picker_index: usize,
    pub command_picker_search: String,
    pub command_picker_search_cursor: usize,
    pub show_history_picker: bool,
    pub history_picker_index: usize,
    pub history_picker_sessions: Vec<SessionMeta>,
    pub history_picker_truncated: bool,
    pub pending_delete_session_idx: Option<usize>,
    pub show_subagent_picker: bool,
    pub subagent_picker_index: usize,
    pub settings_picker: Option<crate::app::SettingsPicker>,
    pub command_panel: Option<crate::app::CommandPanel>,
    pub show_context_modal: bool,
    pub show_status_modal: bool,
    pub show_stats_modal: bool,
    pub show_session_modal: bool,
    pub stats_usage_history: std::collections::BTreeMap<String, MonthlyUsage>,
    pub show_update_prompt: bool,
    pub update_check: rustcode_core::update::UpdateState,
    pub update_prompt_index: usize,
    pub show_mcp_config: bool,
    pub mcp_picker_index: usize,
    pub mcp_edit_state: Option<McpEditState>,
    pub modal_scroll_row: u16,
    pub tool_confirmation_selected: usize,
}

impl RenderState {
    /// Whether any overlay currently owns the screen.
    ///
    /// Derived rather than stored: the answer is a pure function of the
    /// overlay flags and `status`, both of which the view already carries, so
    /// a view built by hand (a render test, a future frontend's fixture)
    /// cannot hold an overlay open without the composer knowing about it.
    pub fn modal_open(&self) -> bool {
        self.user_overlay_open()
            || matches!(
                self.status,
                AppStatus::AwaitingToolConfirmation | AppStatus::AwaitingQuestion
            )
    }

    pub fn user_overlay_open(&self) -> bool {
        self.settings_picker.is_some()
            || self.command_panel.is_some()
            || self.show_model_picker
            || self.show_theme_picker
            || self.show_command_picker
            || self.show_history_picker
            || self.show_subagent_picker
            || self.show_context_modal
            || self.show_status_modal
            || self.show_stats_modal
            || self.show_session_modal
            || self.show_update_prompt
            || self.show_mcp_config
            || matches!(
                self.status,
                AppStatus::VerbosityPicker
                    | AppStatus::ThinkingPicker
                    | AppStatus::EffortPicker
                    | AppStatus::ProtocolPicker
                    | AppStatus::YoloPicker
            )
    }

    /// A populated view of a fresh session.
    ///
    /// Production-visible because a frontend crate cannot see the engine's
    /// `#[cfg(test)]` items, and render/golden tests need a frame to seed
    /// without naming `AppState`. This is `render_state(&AppState::new())`,
    /// so a test frame is identical to a real one.
    pub fn new() -> Self {
        render_state(&AppState::new())
    }

    /// Install a chained `ask_question` flow: the first question becomes
    /// active and the rest wait behind it, so the modal renders `i/N` for a
    /// freshly built view.
    ///
    /// Frontends that build a view by hand use this instead of setting the
    /// chain counters directly, so the derived position/length cannot drift
    /// from the questions they installed.
    pub fn set_question_chain(&mut self, questions: Vec<PendingQuestion>) {
        let mut questions = questions.into_iter();
        self.pending_question = questions.next();
        self.pending_question_chain_len =
            usize::from(self.pending_question.is_some()) + questions.count();
        self.pending_question_chain_position = usize::from(self.pending_question.is_some());
        self.pending_question_chain_answered = 0;
    }
}

impl Default for RenderState {
    fn default() -> Self {
        Self::new()
    }
}

/// Project a live session into the view a frontend renders from.
pub fn render_state(state: &AppState) -> RenderState {
    let capture_subagents = state.show_context_modal || state.show_subagent_picker;
    RenderState {
        revision: state.render_revision,
        // The queue orchestrator owns the full turn across provider-round
        // boundaries. Some intermediate callbacks set the live status to
        // Idle before the next round starts; keep the rendered activity
        // continuous while that owner is still running.
        status: if state.orchestrator_running && state.status == AppStatus::Idle {
            AppStatus::Streaming
        } else {
            state.status.clone()
        },
        input_buffer: state.input_buffer.clone(),
        cursor_position: state.cursor_position,
        composer_selection_anchor: state.composer_selection_anchor,
        ctrl_c_exit_armed: state.ctrl_c_exit_armed(),
        draft_submit_mode: state.draft_submit_mode,
        steering_interruptible: state.can_accept_steer(),
        steering_escape_will_interrupt: state.can_accept_steer()
            && !state.modal_open()
            && state.completion_identity().is_none()
            && state.sel_start.is_none()
            && state.sel_end.is_none(),
        active_suggestion_index: state.active_suggestion_index,
        dismissed_completion: state.dismissed_completion.clone(),
        command_suggestion: state.get_command_suggestion(),

        history: state.history.snapshot(),
        history_display_start: state.history_display_start,
        current_response: Arc::clone(&state.current_response),
        recap_loading: state.recap_request_id.is_some(),
        current_token_usage: state.current_token_usage.clone(),
        current_turn_token_usage: state.current_turn_token_usage.clone(),
        current_round_token_usage: state.current_round_token_usage.clone(),
        current_round_estimated_input_tokens: state.current_round_estimated_input_tokens,
        current_round_estimated_output_tokens: state.current_round_estimated_output_tokens,
        current_provider_request_prompt_estimate: state.current_provider_request_prompt_estimate,
        current_turn_token_usage_is_estimated: state.current_turn_token_usage_is_estimated,
        token_usage_in_flight: state.token_usage_in_flight,
        provider_request_in_flight: state.provider_request_in_flight,
        response_time: state.response_time,
        current_thought_time_ms: state.current_thought_time_ms,
        current_thought_tokens: state.current_thought_tokens,
        current_thought_started_at: state.current_thought_started_at,
        model_quota_remaining: state.model_quota_remaining,
        provider_rate_limits: state.provider_rate_limits.clone(),
        generation_start_time: state.generation_start_time,
        pending_queue: state.pending_queue.clone(),
        pending_steers: state
            .pending_steers
            .iter()
            .map(|steer| steer.text.clone())
            .collect(),
        pending_tool_confirmation: state.pending_tool_confirmation.clone(),
        pending_question: state.pending_question.clone(),
        pending_question_chain_len: state.question_chain_len(),
        pending_question_chain_position: state.question_chain_position(),
        pending_question_chain_answered: state.question_chain_answered(),
        running_tools: state.running_tools.clone(),
        live_tool_calls: Arc::clone(&state.live_tool_calls),
        stream_tracker: state.stream_tracker.clone(),
        expanded_thoughts: state.expanded_thoughts.clone(),
        last_copy_text: state.last_copy_text.clone(),
        transient_notice: state.active_transient_notice().map(str::to_owned),
        verbosity: state.verbosity.clone(),
        delegation_active: state.delegation_active,
        auto_confirm: state.auto_confirm,
        agent_mode: state.agent_mode,

        config: state.config.clone(),
        model_name: state.model_name.clone(),
        api_base_url: state.api_base_url.clone(),
        active_session_id: state.active_session_id.clone(),
        cwd_and_branch: state.cwd_and_branch.clone(),
        home_path: std::env::var("HOME").ok(),
        active_context_window: state.active_context_window(),
        active_model_profile: state.active_model_profile(),
        active_tool_protocol: state.active_tool_protocol(),

        background_tasks: super::background_task_snapshots(&state.active_session_id),
        pending_background_results: state
            .pending_background_outputs
            .iter()
            .map(|pending| super::BackgroundResultDisplay {
                id: pending.task_id.clone(),
                success: pending.output.success,
                cancelled: matches!(
                    pending.output.error_kind,
                    Some(rustcode_core::ToolErrorKind::Cancelled)
                ),
            })
            .collect(),
        waiting_for_background_terminal: state.background_turn_context.is_some(),

        subagents: capture_subagents
            .then(|| state.subagents.iter().map(SubAgentView::from).collect())
            .unwrap_or_default(),
        selected_subagent: state
            .selected_subagent_id
            .and_then(|id| state.subagents.iter().find(|agent| agent.id == id))
            .map(SubAgentView::from),
        selected_subagent_id: state.selected_subagent_id,

        show_model_picker: state.show_model_picker,
        model_picker_index: state.model_picker_index,
        modal_picker_index: state.modal_picker_index,
        model_picker_search: state
            .show_model_picker
            .then(|| state.model_picker_search.clone())
            .unwrap_or_default(),
        model_picker_search_cursor: state.model_picker_search_cursor,
        show_theme_picker: state.show_theme_picker,
        theme_picker_index: state.theme_picker_index,
        theme_picker_initial: state
            .show_theme_picker
            .then(|| state.theme_picker_initial.clone())
            .unwrap_or_default(),
        show_command_picker: state.show_command_picker,
        command_picker_index: state.command_picker_index,
        command_picker_search: state
            .show_command_picker
            .then(|| state.command_picker_search.clone())
            .unwrap_or_default(),
        command_picker_search_cursor: state.command_picker_search_cursor,
        show_history_picker: state.show_history_picker,
        history_picker_index: state.history_picker_index,
        history_picker_sessions: state
            .show_history_picker
            .then(|| state.history_picker_sessions.clone())
            .unwrap_or_default(),
        history_picker_truncated: state.history_picker_truncated,
        pending_delete_session_idx: state.pending_delete_session_idx,
        show_subagent_picker: state.show_subagent_picker,
        subagent_picker_index: state.subagent_picker_index,
        settings_picker: state.settings_picker,
        command_panel: state.command_panel.clone(),
        show_context_modal: state.show_context_modal,
        show_status_modal: state.show_status_modal,
        show_stats_modal: state.show_stats_modal,
        show_session_modal: state.show_session_modal,
        stats_usage_history: state
            .show_stats_modal
            .then(|| state.stats_usage_history.clone())
            .unwrap_or_default(),
        show_update_prompt: state.show_update_prompt,
        update_check: state.update_check,
        update_prompt_index: state.update_prompt_index,
        show_mcp_config: state.show_mcp_config,
        mcp_picker_index: state.mcp_picker_index,
        mcp_edit_state: state
            .show_mcp_config
            .then(|| state.mcp_edit_state.clone())
            .flatten(),
        modal_scroll_row: state.modal_scroll_row,
        tool_confirmation_selected: state.tool_confirmation_selected,
    }
}

#[cfg(test)]
mod tests {
    use super::render_state;
    use crate::app::{AppState, AppStatus};

    #[test]
    fn orchestrator_ownership_keeps_turn_activity_continuous_across_idle_gaps() {
        let mut state = AppState::new();
        state.status = AppStatus::Idle;
        state.orchestrator_running = true;

        assert_eq!(render_state(&state).status, AppStatus::Streaming);

        state.orchestrator_running = false;
        assert_eq!(render_state(&state).status, AppStatus::Idle);
    }
}
