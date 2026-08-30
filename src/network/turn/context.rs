use crate::app::TokenUsage;
use crate::network::{events, lifecycle, loop_detect, verification};
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
}

pub struct BudgetState {
    pub tool_rounds: usize,
    pub max_tool_rounds: usize,
    pub tokens_used: u64,
    pub budget_stopped: Option<String>,
}

pub struct RecoveryState {
    pub oversized_batch_rejections: u8,
    pub loop_detector: loop_detect::LoopDetector,
    pub reasoning_loop_detector: loop_detect::ReasoningLoopDetector,
    pub loop_recovery_attempts: u8,
    pub reasoning_recovery_attempts: u8,
    pub reasoning_recovery_pending: bool,
    pub empty_response_recovery_attempts: u8,
    pub reasoning_loops_detected: usize,
    pub force_final: bool,
    pub completion_blocks: u8,
    pub finish_gate_retries: u32,
    pub consecutive_malformed_calls: usize,
    pub last_malformed_call: Option<String>,
}

pub struct ProgressState {
    pub ledger: loop_detect::ProgressLedger,
    pub made_edits: bool,
    pub failed_mutations: usize,
    pub consecutive_no_progress: usize,
    pub consecutive_failed_mutations: usize,
    pub last_reason: Option<loop_detect::ProgressReason>,
    pub changed_paths: BTreeSet<String>,
    pub phase_checkpoint: Option<String>,
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
    pub final_content: String,
    pub final_content_persisted: bool,
    pub streamed_call_ids: Vec<String>,
}

pub struct MetricsState {
    pub tool_calls: usize,
    pub malformed_calls: usize,
    pub no_progress_results: usize,
    pub failure_replans: usize,
    pub evidence_recoveries: usize,
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

impl TurnContext {
    pub fn new() -> Self {
        Self::with_max_tool_rounds(crate::config::DEFAULT_MAX_TOOL_ROUNDS)
    }

    pub fn with_max_tool_rounds(max_tool_rounds: usize) -> Self {
        Self {
            budget: BudgetState {
                tool_rounds: 0,
                max_tool_rounds: max_tool_rounds.max(1),
                tokens_used: 0,
                budget_stopped: None,
            },
            recovery: RecoveryState {
                oversized_batch_rejections: 0,
                loop_detector: loop_detect::LoopDetector::new(6),
                reasoning_loop_detector: loop_detect::ReasoningLoopDetector::default(),
                loop_recovery_attempts: 0,
                reasoning_recovery_attempts: 0,
                reasoning_recovery_pending: false,
                empty_response_recovery_attempts: 0,
                reasoning_loops_detected: 0,
                force_final: false,
                completion_blocks: 0,
                finish_gate_retries: 0,
                consecutive_malformed_calls: 0,
                last_malformed_call: None,
            },
            progress: ProgressState {
                ledger: loop_detect::ProgressLedger::default(),
                made_edits: false,
                failed_mutations: 0,
                consecutive_no_progress: 0,
                consecutive_failed_mutations: 0,
                last_reason: None,
                changed_paths: BTreeSet::new(),
                phase_checkpoint: None,
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
                final_content: String::new(),
                final_content_persisted: false,
                streamed_call_ids: Vec::new(),
            },
            metrics: MetricsState {
                tool_calls: 0,
                malformed_calls: 0,
                no_progress_results: 0,
                failure_replans: 0,
                evidence_recoveries: 0,
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
        }
    }

    pub fn benchmark_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "tool_rounds": self.budget.tool_rounds, "tool_calls": self.metrics.tool_calls,
            "tokens_used": self.budget.tokens_used, "malformed_calls": self.metrics.malformed_calls,
            "no_progress_results": self.metrics.no_progress_results, "failure_replans": self.metrics.failure_replans,
            "evidence_recoveries": self.metrics.evidence_recoveries,
            "progress_no_information_streak": self.progress.ledger.no_progress_streak(),
            "reasoning_loops_detected": self.recovery.reasoning_loops_detected,
            "reasoning_recovery_attempts": self.recovery.reasoning_recovery_attempts,
            "empty_response_recovery_attempts": self.recovery.empty_response_recovery_attempts,
            "last_progress_reason": self.progress.last_reason.map(|reason| reason.label()),
            "compiler_diagnostic_streak": self.compiler.consecutive_diagnostics,
            "provider_errors": self.metrics.provider_errors, "provider_429s": self.metrics.provider_429s,
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
