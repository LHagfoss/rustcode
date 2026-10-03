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

#[cfg(test)]
mod tests;

pub use crate::provider_auth::{
    AuthMethod, CredentialRef, ProviderRateLimits, RateLimitWindow, provider_summary,
    provider_usage_summary,
};
pub use apply::{
    apply_approval_decision, apply_approval_decision_for_batch, apply_background_task_event,
    apply_question_answer, spawn_observed_orchestrator,
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
    filtered_commands, fuzzy_match_positions, fuzzy_matches, get_completion_len,
    list_project_file_paths, spinner_elapsed, spinner_frame, spinner_frame_index,
    summarize_tool_call,
};
pub use render_state::{RenderState, SubAgentView, render_state};
pub use snapshot::{
    ApprovalAction, ApprovalBatchPrompt, ApprovalDecision, ApprovalPrompt, Command,
    ControllerHandle, ControllerSnapshot, DraftSubmitMode, McpEditState, ModelChoice,
    PendingPrompt, PendingPromptKind, PendingQuestion, PromptSubmitMode, QuestionAnswer,
    QuestionPrompt, SessionChoice, ToolConfirmation, TranscriptItem, UiRect,
};
pub use tasks::{
    SubagentController, TaskDisplay, TurnContext, background_command_label,
    background_task_snapshots, has_background_tasks, spawn_background_task, stop_background_tasks,
};
pub use transcript::{
    AgentUiEvent, AppStatus, ChatMessage, CommandPanel, ExpandOutcome, History, LiveToolCall,
    LiveToolOutputChunk, PendingSteer, SettingsPicker, StreamTracker, SubAgent, SubAgentStatus,
    TokenUsage, ToolCallRef, ToolResult, ToolResultMetadata, ToolResultRecord, Verbosity,
    is_compaction_summary, mcp_tool_display_name, sanitize_recap_content,
    toggle_all_expanded_bodies, toggle_all_expanded_thoughts, toggle_expanded_bodies,
    toggle_expanded_thought,
};

pub use worker::InteractiveController;
