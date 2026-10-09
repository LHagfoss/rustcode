//! UI-neutral contract for frontends that control and observe a RustCode session.

mod apply;
mod attachments;
mod config;
mod context;
mod events;
mod input;
mod native_commands;
mod render_state;
mod snapshot;
mod tasks;
mod tasks_panel;
mod transcript;
mod worker;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_APPROVAL_BATCH_ID: AtomicU64 = AtomicU64::new(1);

/// Creates a unique identity for a controller-owned pending approval batch.
pub(crate) fn next_approval_batch_id() -> String {
    format!(
        "controller:{}:{}",
        std::process::id(),
        NEXT_APPROVAL_BATCH_ID.fetch_add(1, Ordering::Relaxed)
    )
}

static NEXT_TURN_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_QUESTION_ID: AtomicU64 = AtomicU64::new(1);

/// Creates a unique identity for one prompt the queue orchestrator runs, so a
/// cancel can name the turn it observed.
pub(crate) fn next_turn_id() -> String {
    format!(
        "turn:{}:{}",
        std::process::id(),
        NEXT_TURN_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// Creates a unique identity for one pending `ask_question` prompt, so an
/// answer can name the question it was written for.
pub(crate) fn next_question_id() -> String {
    format!(
        "question:{}:{}",
        std::process::id(),
        NEXT_QUESTION_ID.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests;

pub use crate::provider_auth::{
    AuthMethod, CredentialRef, ProviderRateLimits, RateLimitWindow, provider_summary,
    provider_usage_summary,
};
pub use apply::{
    QuestionRejection, QuestionReply, apply_approval_decision, apply_approval_decision_for_batch,
    apply_background_task_event, apply_question_answer, apply_question_answer_for_question,
    cancel_turn_for_turn, refresh_workspace_location_async, spawn_observed_orchestrator,
};
pub use attachments::save_image_attachment;
pub use config::{
    AgentMode, ApiProtocol, AppConfig, ModelProfile, MonthlyUsage, SandboxMode, SessionMeta,
    ToolProtocol, config_dir, sandbox_effective_description, save_config,
};
pub use context::{
    SkillInfo, discover_skills, estimate_message_tokens, estimate_tokens, tool_system_prompt,
};
pub use events::{
    ApprovalChoice, ControllerError, ControllerEvent, ControllerUpdate, TurnUpdate,
    accepts_generation,
};
pub use input::{
    ActivityKind, ActivitySnapshot, CommandInfo, build_help_text, classify_activity,
    classify_live_tools, command_token, copy_selection_binding, current_spinner_frame_index,
    filtered_commands, filtered_model_picker_profiles, fuzzy_match_positions, fuzzy_matches,
    get_completion_len, list_project_file_paths, spinner_elapsed, spinner_frame,
    spinner_frame_index, summarize_tool_call,
};
pub use render_state::{RenderState, SubAgentView, render_state};
/// Name of the shell `run_command` uses on this host, for labelling its calls.
pub use rustcode_command::shell_label;
pub(crate) use snapshot::transcript_tool_details;
pub use snapshot::{
    ApprovalAction, ApprovalBatchPrompt, ApprovalDecision, ApprovalPrompt, Command,
    ControllerHandle, ControllerSnapshot, DraftSubmitMode, McpEditState, ModelChoice,
    PendingPrompt, PendingPromptKind, PendingQuestion, PromptSubmitMode, QuestionAnswer,
    QuestionPrompt, SessionChoice, ToolConfirmation, TranscriptItem, UiRect,
};
pub use tasks::{
    BackgroundResultDisplay, SubagentController, TaskDisplay, TurnContext,
    background_command_label, background_task_snapshots, has_background_tasks,
    recent_background_results, spawn_background_task, stop_background_tasks,
};
pub use tasks_panel::{
    TaskLogView, TaskOutcome, TaskPanelRow, TaskRowState, TasksPanelInput, TasksPanelState,
    TasksPanelView, refresh_tasks_panel, show_tasks_panel, tasks_panel_input, tasks_panel_rows,
};
pub use transcript::{
    AgentUiEvent, AppStatus, ChatMessage, CommandPanel, ExpandOutcome, History, LiveToolCall,
    LiveToolFinish, LiveToolOutputChunk, PendingSteer, SettingsPicker, StreamTracker, SubAgent,
    SubAgentStatus, TokenUsage, ToolCallRef, ToolResult, ToolResultMetadata, ToolResultRecord,
    Verbosity, is_compaction_summary, mcp_tool_display_name, sanitize_recap_content,
    toggle_all_expanded_bodies, toggle_all_expanded_thoughts, toggle_expanded_bodies,
    toggle_expanded_thought, tool_arguments_hash,
};

pub use worker::InteractiveController;
