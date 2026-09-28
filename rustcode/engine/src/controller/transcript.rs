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

pub use crate::app::{SubAgentStatus, TokenUsage, Verbosity};
pub use crate::network::ui_adapter::AgentUiEvent;

/// Strip recap framing to the plain text a history cell renders.
pub fn sanitize_recap_content(content: &str) -> String {
    crate::app::sanitize_recap_content(content)
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
