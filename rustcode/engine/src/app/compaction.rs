//! What the last compaction in this session actually did.
//!
//! Compaction used to be invisible to every frontend: the only record was an
//! `operational_event` line in the debug log. This type is the contract that
//! lets the TUI context modal, the macOS context sheet, and the `/context`
//! report show the same numbers, and lets a user tell an AI summary apart
//! from the reduced deterministic record a plan-backed profile gets.
//!
//! It is per session and in memory only: it describes the state of the live
//! history, not durable session data.

/// What triggered a compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    /// Ran automatically because the history crossed the budget.
    Auto,
    /// The user ran `/compact`.
    Manual,
}

impl CompactionTrigger {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "automatic",
            Self::Manual => "manual",
        }
    }
}

/// How much context the compaction had to replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionMode {
    /// An LLM summary replaced the head of the history.
    AiSummary,
    /// The budgeting fallback: keyword-extracted record instead of a summary.
    /// Plan-backed profiles use this for `/compact`, because their transport
    /// has no plain summarizer endpoint.
    DeterministicRecord,
}

impl CompactionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::AiSummary => "AI summary",
            Self::DeterministicRecord => "deterministic record",
        }
    }

    /// True when the result is a reduced record rather than a written summary,
    /// so frontends can say so instead of implying full fidelity.
    pub fn is_reduced(self) -> bool {
        matches!(self, Self::DeterministicRecord)
    }
}

/// The last compaction applied to the active history.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactionStats {
    pub trigger: CompactionTrigger,
    pub mode: CompactionMode,
    /// Estimated prompt tokens in the history before compaction.
    pub before_tokens: usize,
    /// Estimated tokens after compaction.
    pub after_tokens: usize,
    /// `before_tokens - after_tokens`, saturating at zero.
    pub reclaimed_tokens: usize,
    /// Messages the compactor considered before choosing a boundary.
    pub messages_considered: usize,
    /// How many compactions this session has performed, this one included.
    /// Repeated compaction is what degrades fidelity, so it is worth showing.
    pub count: u32,
}

impl CompactionStats {
    /// Build from a measured before/after pair. `previous_count` is the number
    /// of compactions already recorded for the session.
    pub fn new(
        trigger: CompactionTrigger,
        mode: CompactionMode,
        before_tokens: usize,
        after_tokens: usize,
        messages_considered: usize,
        previous_count: u32,
    ) -> Self {
        Self {
            trigger,
            mode,
            before_tokens,
            after_tokens,
            reclaimed_tokens: before_tokens.saturating_sub(after_tokens),
            messages_considered,
            count: previous_count.saturating_add(1),
        }
    }

    /// True once a session has compacted more than once. Mirrors the warning
    /// Codex shows: repeated compaction degrades accuracy over a long session.
    pub fn is_repeated(&self) -> bool {
        self.count > 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reclaim_is_the_measured_difference() {
        let stats = CompactionStats::new(
            CompactionTrigger::Auto,
            CompactionMode::AiSummary,
            120_000,
            40_000,
            30,
            0,
        );
        assert_eq!(stats.reclaimed_tokens, 80_000);
        assert_eq!(stats.count, 1);
        assert!(!stats.is_repeated());
        assert!(!stats.mode.is_reduced());
    }

    #[test]
    fn reclaim_saturates_when_compaction_does_not_shrink_history() {
        let stats = CompactionStats::new(
            CompactionTrigger::Manual,
            CompactionMode::DeterministicRecord,
            10_000,
            10_500,
            8,
            2,
        );
        assert_eq!(stats.reclaimed_tokens, 0);
        assert_eq!(stats.count, 3);
        assert!(stats.is_repeated());
        assert!(stats.mode.is_reduced());
        assert_eq!(stats.trigger.label(), "manual");
    }
}
