use crate::app::TokenUsage;
use crate::network::messages::RequestPrefixCache;
use crate::network::{ContextCheckpoint, events, lifecycle, loop_detect, verification};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct TurnContext {
    pub budget: BudgetState,
    pub recovery: RecoveryState,
    pub progress: ProgressState,
    pub verification: VerificationState,
    pub compiler: CompilerState,
    pub response: ResponseState,
    pub metrics: MetricsState,
    pub lifecycle: LifecycleState,
    pub(crate) shell_assessments: crate::tools::ShellAssessmentCache,
    pub(crate) request_prefix_cache: RequestPrefixCache,
}

pub struct BudgetState {
    pub tool_rounds: usize,
    pub max_tool_rounds: usize,
    pub max_total_tool_rounds: usize,
    /// Total rounds at which the current resumable segment began.
    pub segment_start_round: usize,
    pub segment_progress_checkpoint: usize,
    /// Number of segment boundaries crossed for this logical task.
    pub segment_count: usize,
    /// A productive segment may be continued by the queue orchestrator.
    pub continuation_pending: bool,
    pub tokens_used: u64,
    pub budget_stopped: Option<String>,
    pub round_budget_notice_sent: bool,
}

pub struct RecoveryState {
    pub oversized_batch_rejections: u8,
    pub loop_detector: loop_detect::LoopDetector,
    pub infrastructure_failures: loop_detect::InfrastructureFailureTracker,
    pub reasoning_loop_detector: loop_detect::ReasoningLoopDetector,
    pub loop_recovery_attempts: u8,
    pub reasoning_recovery_attempts: u8,
    pub reasoning_recovery_pending: bool,
    pub empty_response_recovery_attempts: u8,
    /// A provider/device failure after textual output gets one fresh,
    /// turn-scoped continuation. Keeping this separate from transport retries
    /// prevents a failed recovery from opening an unbounded loop.
    pub stream_recovery_attempts: u8,
    /// One optional, turn-local Laya credit. This state is intentionally not
    /// part of `SegmentCheckpoint`, so it cannot cross a turn segment.
    pub laya_read_only_recoveries_used: usize,
    pub laya_pending_recovery_advisory: Option<loop_detect::RecoveryAdvisory>,
    pub reasoning_loops_detected: usize,
    pub force_final: bool,
    pub completion_blocks: u8,
    pub finish_gate_retries: u32,
    pub consecutive_malformed_calls: usize,
    pub last_malformed_call: Option<String>,
}

pub struct ProgressState {
    pub ledger: loop_detect::ProgressLedger,
    pub file_evidence: loop_detect::FileEvidenceLedger,
    /// Compact evidence for a successful file mutation that the model later
    /// reports as malformed. This lets a failed repair recover against the
    /// known artifact instead of opening another whole-file inspection loop.
    pub grounded_artifact: Option<GroundedArtifactEvidence>,
    /// Complete read-only source results that can support a final review.
    /// Incomplete inspection results stay separately tracked so a truncated
    /// review can never be promoted to a successful headless completion.
    pub complete_inspection_results: usize,
    pub incomplete_inspection_results: usize,
    pub made_edits: bool,
    pub failed_mutations: usize,
    pub consecutive_no_progress: usize,
    pub consecutive_failed_mutations: usize,
    pub last_reason: Option<loop_detect::ProgressReason>,
    pub changed_paths: BTreeSet<String>,
    pub phase_checkpoint: Option<String>,
    /// Monotonic count of meaningful tool results. Segment boundaries use a
    /// checkpoint of this value so progress from an earlier segment cannot
    /// authorize an endless sequence of empty continuations.
    pub meaningful_events: usize,
}

pub struct GroundedArtifactEvidence {
    pub path: String,
    pub write_lines: Option<usize>,
    pub write_bytes: Option<usize>,
    pub read_range: Option<(usize, usize)>,
    pub repair_attempts: usize,
}

pub struct VerificationState {
    pub blocks: u8,
    pub ledger: verification::VerificationLedger,
}

pub struct CompilerState {
    pub edit_root: Option<PathBuf>,
    pub dirty: bool,
    pub cache: Option<(PathBuf, Option<String>)>,
    pub consecutive_error_gates: usize,
    pub consecutive_diagnostics: usize,
    pub last_diagnostic_fingerprint: Option<String>,
}

pub struct ResponseState {
    pub last_token_usage: Option<TokenUsage>,
    /// Sum of provider usage across every request in this logical turn,
    /// including continuation requests.  The per-response value remains in
    /// `last_token_usage` for the footer and transcript attribution.
    pub turn_token_usage: Option<TokenUsage>,
    pub last_stream_termination: Option<lifecycle::StreamTermination>,
    pub final_content: String,
    pub final_content_persisted: bool,
    pub streamed_call_ids: Vec<String>,
}

pub struct MetricsState {
    pub tool_calls: usize,
    pub mutating_tool_calls: usize,
    pub malformed_calls: usize,
    pub no_progress_results: usize,
    pub failure_replans: usize,
    pub evidence_recoveries: usize,
    pub grounded_recoveries: usize,
    pub provider_errors: usize,
    pub provider_429s: usize,
}

pub struct LifecycleState {
    pub turn_machine: events::TurnMachine,
    pub task_completed: bool,
    pub turn_started_at: Instant,
    pub user_wait_duration: Duration,
    pub stop_reason: Option<lifecycle::StopReason>,
}

/// Durable long-turn segment state. Only budget and progress counters cross
/// a restart: detectors rebuild from new observations and the transcript
/// keeps the completed-work evidence. Written when a turn ends with a pending
/// continuation or background turn, cleared otherwise.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SegmentCheckpoint {
    pub schema_version: u32,
    pub session_id: String,
    pub continuation_pending: bool,
    pub background_pending: bool,
    pub tool_rounds: usize,
    pub max_tool_rounds: usize,
    pub max_total_tool_rounds: usize,
    pub segment_start_round: usize,
    pub segment_progress_checkpoint: usize,
    pub segment_count: usize,
    pub meaningful_events: usize,
    pub made_edits: bool,
    pub failed_mutations: usize,
    pub changed_paths: Vec<String>,
    pub phase_checkpoint: Option<String>,
}

impl SegmentCheckpoint {
    pub const SCHEMA_VERSION: u32 = 1;
}

impl TurnContext {
    pub fn new() -> Self {
        Self::with_max_tool_rounds(crate::config::DEFAULT_MAX_TOOL_ROUNDS)
    }

    pub fn with_max_tool_rounds(max_tool_rounds: usize) -> Self {
        Self::with_budgets(
            max_tool_rounds,
            crate::config::DEFAULT_MAX_TOTAL_TOOL_ROUNDS,
        )
    }

    pub fn with_budgets(max_tool_rounds: usize, max_total_tool_rounds: usize) -> Self {
        Self {
            budget: BudgetState {
                tool_rounds: 0,
                max_tool_rounds: if max_tool_rounds == 0 {
                    usize::MAX
                } else {
                    max_tool_rounds
                },
                max_total_tool_rounds: if max_total_tool_rounds == 0 {
                    usize::MAX
                } else {
                    max_total_tool_rounds
                },
                segment_start_round: 0,
                segment_progress_checkpoint: 0,
                segment_count: 1,
                continuation_pending: false,
                tokens_used: 0,
                budget_stopped: None,
                round_budget_notice_sent: false,
            },
            recovery: RecoveryState {
                oversized_batch_rejections: 0,
                loop_detector: loop_detect::LoopDetector::new(6),
                infrastructure_failures: loop_detect::InfrastructureFailureTracker::default(),
                reasoning_loop_detector: loop_detect::ReasoningLoopDetector::default(),
                loop_recovery_attempts: 0,
                reasoning_recovery_attempts: 0,
                reasoning_recovery_pending: false,
                empty_response_recovery_attempts: 0,
                stream_recovery_attempts: 0,
                laya_read_only_recoveries_used: 0,
                laya_pending_recovery_advisory: None,
                reasoning_loops_detected: 0,
                force_final: false,
                completion_blocks: 0,
                finish_gate_retries: 0,
                consecutive_malformed_calls: 0,
                last_malformed_call: None,
            },
            progress: ProgressState {
                ledger: loop_detect::ProgressLedger::default(),
                file_evidence: loop_detect::FileEvidenceLedger::default(),
                grounded_artifact: None,
                complete_inspection_results: 0,
                incomplete_inspection_results: 0,
                made_edits: false,
                failed_mutations: 0,
                consecutive_no_progress: 0,
                consecutive_failed_mutations: 0,
                last_reason: None,
                changed_paths: BTreeSet::new(),
                phase_checkpoint: None,
                meaningful_events: 0,
            },
            verification: VerificationState {
                blocks: 0,
                ledger: verification::VerificationLedger::default(),
            },
            compiler: CompilerState {
                edit_root: None,
                dirty: true,
                cache: None,
                consecutive_error_gates: 0,
                consecutive_diagnostics: 0,
                last_diagnostic_fingerprint: None,
            },
            response: ResponseState {
                last_token_usage: None,
                turn_token_usage: None,
                last_stream_termination: None,
                final_content: String::new(),
                final_content_persisted: false,
                streamed_call_ids: Vec::new(),
            },
            metrics: MetricsState {
                tool_calls: 0,
                mutating_tool_calls: 0,
                malformed_calls: 0,
                no_progress_results: 0,
                failure_replans: 0,
                evidence_recoveries: 0,
                grounded_recoveries: 0,
                provider_errors: 0,
                provider_429s: 0,
            },
            lifecycle: LifecycleState {
                turn_machine: events::TurnMachine::new(),
                task_completed: false,
                turn_started_at: Instant::now(),
                user_wait_duration: Duration::ZERO,
                stop_reason: None,
            },
            shell_assessments: std::collections::HashMap::new(),
            request_prefix_cache: RequestPrefixCache::default(),
        }
    }

    pub(crate) fn segment_rounds(&self) -> usize {
        self.budget
            .tool_rounds
            .saturating_sub(self.budget.segment_start_round)
    }

    pub(crate) fn has_progress_in_current_segment(&self) -> bool {
        self.progress.meaningful_events > self.budget.segment_progress_checkpoint
    }

    /// Snapshot the restart-durable segment state for the session sidecar.
    pub(crate) fn segment_checkpoint(
        &self,
        session_id: &str,
        continuation_pending: bool,
        background_pending: bool,
    ) -> SegmentCheckpoint {
        SegmentCheckpoint {
            schema_version: SegmentCheckpoint::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            continuation_pending,
            background_pending,
            tool_rounds: self.budget.tool_rounds,
            max_tool_rounds: self.budget.max_tool_rounds,
            max_total_tool_rounds: self.budget.max_total_tool_rounds,
            segment_start_round: self.budget.segment_start_round,
            segment_progress_checkpoint: self.budget.segment_progress_checkpoint,
            segment_count: self.budget.segment_count,
            meaningful_events: self.progress.meaningful_events,
            made_edits: self.progress.made_edits,
            failed_mutations: self.progress.failed_mutations,
            changed_paths: self.progress.changed_paths.iter().cloned().collect(),
            phase_checkpoint: self.progress.phase_checkpoint.clone(),
        }
    }

    /// Hydrate a fresh context from a checkpoint. Detectors restart empty and
    /// rebuild from new observations; budgets and progress resume where the
    /// previous process left off. Returns false when the checkpoint is for
    /// another session or an unknown schema.
    pub(crate) fn restore_segment(
        &mut self,
        checkpoint: &SegmentCheckpoint,
        session_id: &str,
    ) -> bool {
        if checkpoint.schema_version != SegmentCheckpoint::SCHEMA_VERSION
            || checkpoint.session_id != session_id
        {
            return false;
        }
        self.budget.tool_rounds = checkpoint.tool_rounds;
        self.budget.max_tool_rounds = checkpoint.max_tool_rounds;
        self.budget.max_total_tool_rounds = checkpoint.max_total_tool_rounds;
        self.budget.segment_start_round = checkpoint.segment_start_round;
        self.budget.segment_progress_checkpoint = checkpoint
            .segment_progress_checkpoint
            .min(checkpoint.meaningful_events);
        self.budget.segment_count = checkpoint.segment_count.max(1);
        self.budget.continuation_pending = checkpoint.continuation_pending;
        self.progress.meaningful_events = checkpoint.meaningful_events;
        self.progress.made_edits = checkpoint.made_edits;
        self.progress.failed_mutations = checkpoint.failed_mutations;
        self.progress.changed_paths = checkpoint.changed_paths.iter().cloned().collect();
        self.progress.phase_checkpoint = checkpoint.phase_checkpoint.clone();
        true
    }

    pub(crate) fn begin_next_segment(&mut self) {
        self.budget.segment_start_round = self.budget.tool_rounds;
        self.budget.segment_progress_checkpoint = self.progress.meaningful_events;
        self.budget.segment_count = self.budget.segment_count.saturating_add(1);
        self.budget.continuation_pending = false;
        self.budget.round_budget_notice_sent = false;
        self.lifecycle.stop_reason = None;
        self.recovery.laya_read_only_recoveries_used = 0;
        self.recovery.laya_pending_recovery_advisory = None;
    }

    /// Add one provider response to the logical turn total. Provider usage is
    /// reported per request, so overwriting this value would undercount turns
    /// that execute tools or use response continuations.
    pub(crate) fn record_token_usage(&mut self, usage: Option<&TokenUsage>) {
        let Some(usage) = usage else {
            return;
        };
        let total = self
            .response
            .turn_token_usage
            .get_or_insert_with(TokenUsage::default);
        total.prompt_tokens = total.prompt_tokens.saturating_add(usage.prompt_tokens);
        total.completion_tokens = total
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
        total.cached_tokens = Some(
            total
                .cached_tokens
                .unwrap_or_default()
                .saturating_add(usage.cached_tokens.unwrap_or_default()),
        );
        total.cache_write_tokens = Some(
            total
                .cache_write_tokens
                .unwrap_or_default()
                .saturating_add(usage.cache_write_tokens.unwrap_or_default()),
        );
        total.cache_discount = usage.cache_discount.or(total.cache_discount);
    }

    /// Snapshot short-lived progress for the next request-local context tail.
    /// This is deliberately derived from turn state rather than persisted in
    /// history, so it cannot alter replay or compaction semantics.
    pub(crate) fn context_checkpoint(&self) -> ContextCheckpoint {
        let (edit_status, next_action) = if self.progress.made_edits {
            (
                "edits made; verification pending",
                "verify the changed files and run focused checks",
            )
        } else if self.progress.failed_mutations > 0 {
            (
                "no edits applied; the last mutation failed",
                "re-read the target and resolve the failed mutation",
            )
        } else {
            (
                "no edits made yet",
                "inspect the relevant files, then make the smallest scoped change",
            )
        };

        ContextCheckpoint {
            objective: self.progress.phase_checkpoint.clone(),
            edit_status,
            next_action,
        }
    }

    pub fn benchmark_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_rounds": self.budget.tool_rounds, "tool_calls": self.metrics.tool_calls,
            "segment_rounds": self.segment_rounds(),
            "segment_count": self.budget.segment_count,
            "effective_segment_limit": (self.budget.max_tool_rounds != usize::MAX)
                .then_some(self.budget.max_tool_rounds),
            "effective_total_round_limit": (self.budget.max_total_tool_rounds != usize::MAX)
                .then_some(self.budget.max_total_tool_rounds),
            "continuation_pending": self.budget.continuation_pending,
            "tokens_used": self.budget.tokens_used, "malformed_calls": self.metrics.malformed_calls,
            "no_progress_results": self.metrics.no_progress_results, "failure_replans": self.metrics.failure_replans,
            "evidence_recoveries": self.metrics.evidence_recoveries,
            "grounded_recoveries": self.metrics.grounded_recoveries,
            "progress_no_information_streak": self.progress.ledger.no_progress_streak(),
            "reasoning_loops_detected": self.recovery.reasoning_loops_detected,
            "infrastructure_failure_streak": self.recovery.infrastructure_failures.streak(),
            "reasoning_recovery_attempts": self.recovery.reasoning_recovery_attempts,
            "empty_response_recovery_attempts": self.recovery.empty_response_recovery_attempts,
            "last_stream_termination": self
                .response
                .last_stream_termination
                .map(|termination| termination.to_string()),
            "last_progress_reason": self.progress.last_reason.map(|reason| reason.label()),
            "compiler_diagnostic_streak": self.compiler.consecutive_diagnostics,
            "provider_errors": self.metrics.provider_errors, "provider_429s": self.metrics.provider_429s,
            "prefix_cache": self.request_prefix_cache.last_decision().label(),
            "prefix_context_updates": self.request_prefix_cache.context_updates(),
            "changed_paths": self.progress.changed_paths.iter().collect::<Vec<_>>(),
            "phase_checkpoint": self.progress.phase_checkpoint,
            "stop_reason": self.lifecycle.stop_reason.as_ref().map(ToString::to_string),
        })
    }
}

impl Default for TurnContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_checkpoint_round_trips_budgets_and_rejects_foreign_sessions() {
        let mut ctx = TurnContext::with_budgets(40, 200);
        ctx.budget.tool_rounds = 40;
        ctx.budget.segment_count = 2;
        ctx.progress.meaningful_events = 7;
        ctx.progress.made_edits = true;
        ctx.progress.changed_paths.insert("src/a.rs".to_string());
        ctx.progress.phase_checkpoint = Some("phase".to_string());

        let checkpoint = ctx.segment_checkpoint("session-1", true, false);
        // Sidecar must survive JSON serialization.
        let reparsed: SegmentCheckpoint =
            serde_json::from_str(&serde_json::to_string(&checkpoint).unwrap()).unwrap();
        assert_eq!(reparsed, checkpoint);

        let mut restored = TurnContext::with_budgets(40, 200);
        assert!(restored.restore_segment(&reparsed, "session-1"));
        assert_eq!(restored.budget.tool_rounds, 40);
        assert_eq!(restored.budget.segment_count, 2);
        assert!(restored.budget.continuation_pending);
        assert!(restored.has_progress_in_current_segment());
        assert!(restored.progress.made_edits);
        assert!(restored.progress.changed_paths.contains("src/a.rs"));

        let mut foreign = TurnContext::with_budgets(40, 200);
        assert!(!foreign.restore_segment(&reparsed, "session-2"));
        assert_eq!(foreign.budget.tool_rounds, 0);
    }

    #[test]
    fn turn_usage_accumulates_requests_without_changing_last_response_usage() {
        let mut context = TurnContext::new();
        let first = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
            cached_tokens: Some(80),
            cache_write_tokens: Some(4),
            cache_discount: Some(0.5),
        };
        let second = TokenUsage {
            prompt_tokens: 60,
            completion_tokens: 10,
            total_tokens: 70,
            cached_tokens: None,
            cache_write_tokens: None,
            cache_discount: None,
        };

        context.record_token_usage(Some(&first));
        context.record_token_usage(Some(&second));

        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .prompt_tokens,
            160
        );
        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .completion_tokens,
            30
        );
        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .total_tokens,
            190
        );
        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .cached_tokens,
            Some(80)
        );
        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .cache_write_tokens,
            Some(4)
        );
        assert_eq!(
            context
                .response
                .turn_token_usage
                .as_ref()
                .unwrap()
                .cache_discount,
            Some(0.5)
        );
    }
}
