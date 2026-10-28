//! What the last compaction in this session actually did.
//!
//! Compaction used to be invisible to every frontend: the only record was an
//! `operational_event` line in the debug log. This type is the contract that
//! lets the TUI context modal, the macOS context sheet, and the `/context`
//! report show the same numbers, and lets a user tell an AI summary apart
//! from the reduced deterministic record a plan-backed profile gets.
//!
//! It is per session. The live value describes the active history; the last
//! one is also recorded in the session's metadata, so the count and the
//! chat's compaction card survive a reload.

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

    /// True when an automatic compaction reclaimed under a tenth of the
    /// history. Repeating the summarizer then would refill and recompact on
    /// every turn without freeing room.
    pub fn was_ineffective_auto(&self) -> bool {
        self.trigger == CompactionTrigger::Auto
            && self.reclaimed_tokens.saturating_mul(10) < self.before_tokens
    }

    /// The record kept in the session's metadata, stamped `at_ms`.
    pub fn to_session_record(&self, at_ms: u64) -> rustcode_session::SessionCompaction {
        rustcode_session::SessionCompaction {
            count: self.count,
            trigger: snake_case_name(&self.trigger),
            mode: snake_case_name(&self.mode),
            before_tokens: self.before_tokens as u64,
            after_tokens: self.after_tokens as u64,
            messages_considered: self.messages_considered as u64,
            at_ms,
        }
    }

    /// The stats a recorded compaction describes; `None` for a record whose
    /// trigger or mode this engine does not know.
    pub fn from_session_record(record: &rustcode_session::SessionCompaction) -> Option<Self> {
        let mut stats = Self::new(
            parse_snake_case_name(&record.trigger)?,
            parse_snake_case_name(&record.mode)?,
            record.before_tokens as usize,
            record.after_tokens as usize,
            record.messages_considered as usize,
            0,
        );
        stats.count = record.count;
        Some(stats)
    }
}

/// The summary the latest compaction left at the head of `history`: the
/// handoff an AI summary wrote, or the deterministic record.
pub fn compaction_summary(history: &[crate::app::ChatMessage]) -> Option<String> {
    history
        .iter()
        .rev()
        .find_map(|message| message.compaction_boundary.as_ref())
        .map(|boundary| boundary.summary.clone())
        .filter(|summary| !summary.trim().is_empty())
}

fn parse_snake_case_name<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::from(name)).ok()
}

fn snake_case_name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
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
        assert!(
            !stats.was_ineffective_auto(),
            "manual runs are the user's call"
        );
    }

    #[test]
    fn the_session_record_round_trips() {
        let stats = CompactionStats::new(
            CompactionTrigger::Manual,
            CompactionMode::AiSummary,
            161_495,
            4_000,
            116,
            2,
        );
        let record = stats.to_session_record(42);
        assert_eq!(record.trigger, "manual");
        assert_eq!(record.mode, "ai_summary");
        assert_eq!(record.count, 3);
        assert_eq!(record.at_ms, 42);
        assert_eq!(CompactionStats::from_session_record(&record), Some(stats));
        let unknown = rustcode_session::SessionCompaction {
            trigger: "later".into(),
            ..record
        };
        assert_eq!(CompactionStats::from_session_record(&unknown), None);
    }

    #[test]
    fn an_automatic_pass_that_frees_little_is_ineffective() {
        let weak = CompactionStats::new(
            CompactionTrigger::Auto,
            CompactionMode::AiSummary,
            100_000,
            95_000,
            40,
            0,
        );
        assert!(weak.was_ineffective_auto());
        let strong = CompactionStats::new(
            CompactionTrigger::Auto,
            CompactionMode::AiSummary,
            100_000,
            40_000,
            40,
            0,
        );
        assert!(!strong.was_ineffective_auto());
    }
}
