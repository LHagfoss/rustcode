//! Skills discovery and token estimation for frontends (issue #1441).
//!
//! Thin contract over the engine estimators: frontends call these instead of
//! reaching into `crate::skills` / `crate::network::compaction` directly, so
//! the context modal, `/skills`, and the future `serve` transport share one
//! accounting rule. Numbers are identical by construction — same functions,
//! same inputs.
//!
//! Decision record: per-section snapshot-carried counts (the issue's option
//! 1 in full) are deferred to #1442. The TUI render path owns `AppState`
//! and holds no `ControllerSnapshot`, so snapshot-carried values cannot
//! reach the context modal without the render convergence #1442 owns.

use std::path::PathBuf;

use crate::config::{AgentMode, ToolProtocol};

/// Owned display data for one discovered skill: the fields frontends
/// render (picker rows, `/skills` output, context accounting).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

impl From<crate::skills::SkillMetadata> for SkillInfo {
    fn from(skill: crate::skills::SkillMetadata) -> Self {
        Self {
            name: skill.name,
            description: skill.description,
            path: skill.path,
        }
    }
}

/// Skills from `.rustcode/skills`, `.agents/skills`, and the user config
/// dir. Filesystem reads stay in the engine; frontends render this list.
pub fn discover_skills() -> Vec<SkillInfo> {
    crate::skills::discover_skills()
        .into_iter()
        .map(SkillInfo::from)
        .collect()
}

/// Exact token count for `text` under `cl100k_base` (memoized in the engine).
pub fn estimate_tokens(text: &str) -> usize {
    crate::network::compaction::estimate_tokens(text)
}

/// Provider-visible cost of a persisted chat message, including native
/// tool-call arguments stored outside `content`.
pub fn estimate_message_tokens(message: &crate::app::ChatMessage) -> usize {
    crate::network::compaction::estimate_message_tokens(message)
}

/// System-prompt text the context modal accounts for: tool definitions for
/// `protocol`/`agent_mode`, including agent tools when a delegation is active.
pub fn tool_system_prompt(
    delegation_active: bool,
    protocol: ToolProtocol,
    agent_mode: AgentMode,
) -> String {
    crate::tools::tool_system_prompt(delegation_active, protocol, agent_mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimates_match_the_engine_source() {
        for text in [
            "",
            "child task",
            "cargo test --locked",
            "line one\nline two",
        ] {
            assert_eq!(
                estimate_tokens(text),
                crate::network::compaction::estimate_tokens(text)
            );
        }
        let message = crate::app::ChatMessage::new("assistant", "subagent response");
        assert_eq!(
            estimate_message_tokens(&message),
            crate::network::compaction::estimate_message_tokens(&message)
        );
    }
    #[test]
    fn skill_discovery_maps_every_engine_skill() {
        let discovered = discover_skills();
        let expected = crate::skills::discover_skills();
        assert_eq!(discovered.len(), expected.len());
        for (info, skill) in discovered.iter().zip(expected.iter()) {
            assert_eq!(info.name, skill.name);
            assert_eq!(info.description, skill.description);
            assert_eq!(info.path, skill.path);
        }
    }

    #[test]
    fn system_prompt_matches_the_engine_source() {
        let prompt = tool_system_prompt(false, ToolProtocol::Json, AgentMode::Build);
        assert_eq!(
            prompt,
            crate::tools::tool_system_prompt(false, ToolProtocol::Json, AgentMode::Build)
        );
        assert!(!prompt.is_empty());
    }
}
