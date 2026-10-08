//! Transcript presentation contract for frontends (issue #1442).
//!
//! The transcript/history-cell render layer projects session history and
//! live turn events. These re-exports and helpers are the seam it renders
//! through instead of reaching into `crate::app` / `crate::network`
//! directly.
//!
//! `AgentUiEvent` is re-exported as-is: it is the live-turn bridge the
//! engine orchestrator already produces, and the transcript consumes only
//! its prompt/delta/finished shapes. Native `TurnUpdate` production for
//! TUI-driven turns is turn-machinery endgame work; until then the render
//! side converges on the contract event type.

/// Session status the composer, status line, and footer render against.
pub use crate::app::AppStatus;
/// Subagent records the subagent picker and context modal list.
pub use crate::app::SubAgent;
/// Queued steer draft rendered by the composer.
pub use crate::app::state::PendingSteer;
/// Live turn shapes: in-flight tool calls, their output chunks, and the
/// token/thought stream tracker.
pub use crate::app::{
    CommandPanel, LiveToolCall, LiveToolFinish, LiveToolOutputChunk, SettingsPicker, StreamTracker,
};
pub use crate::app::{SubAgentStatus, TokenUsage, Verbosity};
pub use crate::network::events::{ToolResult, ToolResultMetadata};
pub use crate::network::ui_adapter::AgentUiEvent;
/// Persisted conversation and tool-record shapes the transcript renders.
pub use rustcode_core::{ChatMessage, History, ToolCallRef, ToolResultRecord};

/// True when `content` is a compaction summary: engine-internal history
/// bookkeeping that the transcript must not render as user-facing text.
///
/// Frontends get the classification, not the marker literal, so the render
/// protocol cannot drift from what compaction actually writes.
pub fn is_compaction_summary(content: &str) -> bool {
    content.starts_with(crate::network::compaction::SUMMARY_MARKER)
}

/// Strip recap framing to the plain text a history cell renders.
pub fn sanitize_recap_content(content: &str) -> String {
    crate::app::sanitize_recap_content(content)
}

/// What one expand/collapse request did to the collapsed tool bodies.
pub use crate::app::ExpandOutcome;

/// Expand every collapsible body, or collapse them all when all are expanded.
///
/// This is what `ctrl+o` does; [`toggle_expanded_thought`] is the single-entry
/// step behind `ctrl+shift+o`. The direction is decided by the visible set, so
/// the press is idempotent instead of depending on invisible focus (#1594).
pub fn toggle_all_expanded_thoughts(
    state: &mut crate::app::AppState,
    candidates: &[usize],
) -> ExpandOutcome {
    crate::app::toggle_all_expanded_thoughts(state, candidates)
}

/// Apply one whole-transcript expand/collapse press to bodies a frontend owns.
pub fn toggle_all_expanded_bodies(
    expanded: &mut std::collections::HashSet<usize>,
    candidates: &[usize],
) -> (ExpandOutcome, &'static str) {
    crate::app::toggle_all_expanded_bodies(expanded, candidates)
}

/// Toggle the collapsed tool body the expand key points at.
///
/// `candidates` are the message indices the frontend rendered with a collapsed
/// body, oldest first. The frontend owns that classification — only it knows
/// which rows it actually collapsed — so it passes the list in and the engine
/// owns the state transition and the feedback (#1541).
pub fn toggle_expanded_thought(
    state: &mut crate::app::AppState,
    candidates: &[usize],
) -> ExpandOutcome {
    crate::app::toggle_expanded_thought(state, candidates)
}

/// Apply one expand/collapse press to the collapsed bodies a frontend owns.
///
/// A production frontend presses through [`toggle_expanded_thought`] against the
/// session; a frontend that holds only the render-visible expand state drives
/// the same press here, so the transition and the feedback cannot drift between
/// the two. Returns what the press did plus the notice to install on the view
/// the next frame renders from (#1431).
pub fn toggle_expanded_bodies(
    expanded: &mut std::collections::HashSet<usize>,
    focus: &mut Option<usize>,
    candidates: &[usize],
) -> (ExpandOutcome, &'static str) {
    crate::app::toggle_expanded_bodies(expanded, focus, candidates)
}

/// The hash a tool result's record stores for the arguments of the call it
/// answers. A result without a call id is matched to its call through it.
pub fn tool_arguments_hash(arguments: &serde_json::Value) -> String {
    crate::network::tool_exec::stable_arguments_hash(arguments)
}

/// PascalCase display name for an MCP tool, if it follows the prefix convention.
pub fn mcp_tool_display_name(name: &str) -> Option<String> {
    crate::tools::mcp_tool_display_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recap_sanitizer_matches_the_engine_source() {
        for content in ["", "plain message", "[recap] prior facts\nmore"] {
            assert_eq!(
                sanitize_recap_content(content),
                crate::app::sanitize_recap_content(content)
            );
        }
    }

    #[test]
    fn transcript_types_unify_with_the_engine_sources() {
        fn assert_verbosity(_: Verbosity) {}
        fn assert_usage(_: TokenUsage) {}
        fn assert_subagent(_: SubAgentStatus) {}
        assert_verbosity(crate::app::Verbosity::Low);
        assert_usage(crate::app::TokenUsage::default());
        assert_subagent(crate::app::SubAgentStatus::Running);
    }
}
