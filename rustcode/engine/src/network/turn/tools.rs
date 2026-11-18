use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage};

use super::super::compiler::{CompilerCheckOutcome, update_compiler_outcome_streak};
use super::super::events::{self, ToolResult};
use super::super::lifecycle;
use super::super::loop_detect;
use super::super::policy;
use super::super::tool_exec::{get_tool_project_root, tool_result_history_message};
use super::super::verification;
use super::super::{
    FORCE_ANSWER_PROMPT, LoopRecoveryAction, active_todo_checkpoint, cached_compiler_check,
    call_refs_for, compiler_diagnostic_fingerprint, completion_block_message,
    completion_claims_unapplied_work, failure_replan_message, is_mutating_tool,
    log_recovery_decision, loop_recovery_action_for, mutation_made_progress,
    push_or_replace_loop_warning, push_or_replace_recovery_notice,
    unanswered_call_results_with_kind, update_compiler_diagnostic_streak,
};
use super::recovery::{loop_recovery_prompt, record_malformed_call};
use super::{
    GroundedArtifactEvidence, TurnContext, append_cancelled_batch_results,
    hydrate_explicit_verification_from_history,
};
use rustcode_tool_protocol::text::has_intended_tool_call;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolHandlingOutcome {
    Continue,
    Stop,
    NotHandled,
}

fn should_apply_loop_recovery(
    completion_requested: bool,
    output_abort: bool,
    round_had_meaningful: bool,
    has_evidence_recovery: bool,
) -> bool {
    !completion_requested && (has_evidence_recovery || (output_abort && !round_had_meaningful))
}

/// The system notice for a round that did not run every call, or `None` when
/// every unexecuted call was rejected by validation: each of those is already
/// answered by its own tool result.
fn unexecuted_calls_notice(
    requested_calls: usize,
    executed_calls: usize,
    queued: &[usize],
    reissue: &[usize],
    closed: &[usize],
    is_known_mutation: impl Fn(usize) -> bool,
    describe: impl Fn(&[usize]) -> String,
) -> Option<String> {
    if queued.is_empty() && reissue.is_empty() && closed.is_empty() {
        return None;
    }
    let mut notice = format!(
        "[The model emitted {requested_calls} tool calls. {executed_calls} were executed this round."
    );
    // Only a known mutation is described as a workspace change: an
    // unannotated MCP call is held because it may have side effects, not
    // because it changes the workspace (#1970).
    let (queued_changes, queued_other): (Vec<usize>, Vec<usize>) =
        queued.iter().partition(|index| is_known_mutation(**index));
    if !queued_changes.is_empty() {
        notice.push_str(&format!(
            " The scheduler held {} over the per-response workspace-change limit and queued them for automatic execution in a later round ({}); do not reissue them, their real results arrive without another request from you.",
            queued_changes.len(),
            describe(&queued_changes),
        ));
    }
    if !queued_other.is_empty() {
        notice.push_str(&format!(
            " The scheduler held {} call(s) that may have side effects over the per-response limit on such calls and queued them for automatic execution in a later round ({}); do not reissue them, their real results arrive without another request from you.",
            queued_other.len(),
            describe(&queued_other),
        ));
    }
    if !reissue.is_empty() {
        notice.push_str(&format!(
            " The harness did not schedule {} call(s) ({}); reissue them only after reviewing the real results.",
            reissue.len(),
            describe(reissue),
        ));
    }
    if !closed.is_empty() {
        notice.push_str(&format!(
            " The remaining {} call(s) ({}) were not executed: review the real results above.",
            closed.len(),
            describe(closed),
        ));
    }
    notice.push(']');
    Some(notice)
}

/// Write targets that do not exist yet, so a successful write can be recorded
/// as a creation rather than a change to an existing file.
fn absent_write_targets(
    calls: &[crate::tools::ToolCall],
    workspace_root: Option<&std::path::Path>,
) -> std::collections::BTreeMap<String, std::path::PathBuf> {
    let Some(root) = workspace_root else {
        return Default::default();
    };
    calls
        .iter()
        .filter(|call| matches!(call.name.as_str(), "write_to_file" | "write_file_chunk"))
        .filter_map(|call| call.arguments.get("path")?.as_str())
        .map(|path| (path.to_owned(), root.join(path)))
        .filter(|(_, resolved)| !resolved.exists())
        .collect()
}

/// A builtin call that changes the workspace, a process, or delegated work.
/// Unknown (MCP) tools are not known mutations even when they are held.
fn is_known_mutation_call(name: &str) -> bool {
    matches!(
        crate::tools::tool_safety(name),
        crate::tools::ToolSafety::WorkspaceMutation
            | crate::tools::ToolSafety::ProcessControl
            | crate::tools::ToolSafety::Delegation
    )
}

fn tool_changes_workspace(name: &str, arguments: Option<&serde_json::Value>) -> bool {
    if name == "spawn_agent" {
        return arguments
            .is_some_and(crate::app::subagent_context::AgentRole::spawn_requests_write);
    }
    is_mutating_tool(name)
}

fn loop_signal_event(
    output_stagnation: Option<&loop_detect::LoopStatus>,
    round_had_meaningful: Option<bool>,
    evidence_recovery: Option<loop_detect::ProgressReason>,
    recovery_taken: bool,
    recovery_suppressed_reason: Option<&str>,
    cancelled: bool,
) -> serde_json::Value {
    let (status, repeats) = match output_stagnation {
        Some(loop_detect::LoopStatus::Ok) => ("ok", serde_json::json!(0)),
        Some(loop_detect::LoopStatus::Warning(repeats)) => ("warning", serde_json::json!(repeats)),
        Some(loop_detect::LoopStatus::Abort(repeats)) => ("abort", serde_json::json!(repeats)),
        None => ("not_evaluated", serde_json::Value::Null),
    };

    serde_json::json!({
        "output_stagnation": {
            "status": status,
            "repeats": repeats,
        },
        "round_had_meaningful": round_had_meaningful,
        "evidence_recovery": evidence_recovery.map(loop_detect::ProgressReason::label),
        "recovery_taken": recovery_taken,
        "recovery_suppressed_reason": recovery_suppressed_reason,
        "cancelled": cancelled,
    })
}

fn batch_invalidates_read_recovery(
    made_progress: bool,
    recovery: Option<&(loop_detect::ProgressReason, usize, String)>,
) -> bool {
    made_progress
        && recovery
            .is_some_and(|(reason, _, _)| *reason == loop_detect::ProgressReason::NoNewInformation)
}

/// Append a completed tool batch and atomically follow it with any steering
/// text accepted for the same turn session. Callers hold the AppState mutex so
/// the result/steer ordering cannot interleave with queue or session changes.
fn append_finalized_tool_batch(
    state: &mut AppState,
    turn_session_id: &str,
    result_messages: Vec<ChatMessage>,
) {
    state.history.extend(result_messages);
    let screenshots = std::mem::take(&mut state.pending_preview_images);
    if state.active_session_id == turn_session_id
        && let Some(text) = crate::preview::screenshot_attachment_message(&screenshots)
    {
        state.history.push(ChatMessage::new("user", text));
    }
    append_pending_steers(state, turn_session_id);
}

/// Consume only steers that still belong to the captured turn and active
/// session. A session replacement or an Esc promotion wins by making this a
/// no-op, so no accepted text can leak into another session or appear twice.
fn append_pending_steers(state: &mut AppState, turn_session_id: &str) {
    if state.active_session_id != turn_session_id {
        return;
    }
    for steer in state.take_steer_records_for_history(turn_session_id) {
        let mut message = ChatMessage::new("user", steer.text);
        message.origin = steer.origin;
        state.history.push(message);
    }
}

/// Fold one model round's stagnation signals into the consecutive counter.
/// A batched round contributes at most one step no matter how many of its
/// calls stagnated, and any meaningful result clears the streak.
fn apply_round_stagnation(
    consecutive_no_progress: usize,
    had_meaningful: bool,
    hits: usize,
) -> usize {
    if had_meaningful {
        0
    } else if hits > 0 {
        consecutive_no_progress.saturating_add(1)
    } else {
        consecutive_no_progress
    }
}

/// Feed one result into output stagnation, counting each distinct output once
/// per model round. Parallel calls in one round that return the same thing
/// (four different searches that each find nothing) are one observation, not
/// a streak; the streak builds only across rounds (#2563).
fn record_round_output(
    detector: &mut loop_detect::LoopDetector,
    round_outputs: &mut std::collections::HashSet<u64>,
    output: &str,
) -> loop_detect::LoopStatus {
    let key = loop_detect::stagnation_key(output);
    if round_outputs.insert(loop_detect::stable_hash(key)) {
        detector.record_output(key)
    } else {
        loop_detect::LoopStatus::Ok
    }
}

/// Recovery action label for a sustained run of stale re-verification rounds.
const REVERIFICATION_ACTION: &str = "re-verification of unchanged files";

/// Fold one round into the consecutive stale-round counter. A round is stale
/// when at least half of its results were successful read-only re-reads that
/// added no information, and it changed nothing. Mixed rounds matter because one fresh read clears both the
/// per-action ledger streak and `consecutive_no_progress`. Without this, a
/// model that re-reads the same docs alongside one new range every round
/// never arms recovery.
fn apply_stale_round(
    consecutive: usize,
    stale_rereads: usize,
    assessed_results: usize,
    lasting_progress: bool,
) -> usize {
    if !lasting_progress && stale_rereads > 0 && stale_rereads * 2 >= assessed_results {
        consecutive.saturating_add(1)
    } else {
        0
    }
}

fn mutation_batch_guidance(policy: &crate::config::ToolSchedulingPolicy) -> String {
    if policy.allow_batching {
        format!(
            "This trusted profile permits a bounded batch of up to {} read-only and {} workspace-changing tool calls per response. Control-plane calls run first with read-only calls following in order; keep mutations grounded and sequential, and never assume an unexecuted call ran.",
            policy.max_read_only_calls, policy.max_mutating_calls
        )
    } else {
        format!(
            "Batch independent read-only calls freely in one response and wait for their results before choosing the next action. Up to {} workspace-changing calls per response run in the order given; control-plane calls run first with read-only calls following in order. Read-only inspection never consumes the mutation budget. Never assume an unexecuted call ran.",
            policy.max_mutating_calls
        )
    }
}

async fn populate_shell_assessments(
    state: &Arc<Mutex<AppState>>,
    ctx: &mut TurnContext,
    calls: &[crate::tools::ToolCall],
) {
    let (mode, auto_confirm) = {
        let state = state.lock().await;
        (state.agent_mode, state.auto_confirm)
    };
    for call in calls {
        if call.name != "run_command" {
            continue;
        }
        let key = crate::tools::shell_assessment_cache_key(call);
        if ctx
            .shell_assessments
            .get(&key)
            .is_some_and(|assessment| crate::tools::shell_assessment_matches_call(assessment, call))
        {
            continue;
        }
        if let Some(assessment) = crate::tools::assess_shell_call(call, mode, auto_confirm) {
            ctx.shell_assessments.insert(key, assessment);
        }
    }
}

fn targeted_no_progress_guidance(
    reason: loop_detect::ProgressReason,
    streak: usize,
    action: &str,
) -> String {
    let action = action
        .char_indices()
        .nth(160)
        .map_or(action, |(end, _)| &action[..end]);
    // A discovery call that keeps failing never listed anything: point at its
    // error, not at an inventory it did not return (#2540).
    if action == "list_mcp_tools"
        && matches!(
            reason,
            loop_detect::ProgressReason::Failure | loop_detect::ProgressReason::RepeatedFailure
        )
    {
        return format!(
            "list_mcp_tools failed {streak} time(s) in a row with the same error, so no tools were listed. Read the error in its result and fix what it names (for example remove an argument it rejects) before calling it again. A failed discovery call says nothing about which MCP tools exist; do not tell the user a server's tools are unavailable on that basis."
        );
    }
    if action == "list_mcp_tools" {
        return "MCP discovery is repeating already-known tools. Stop listing the inventory. Invoke the discovered callable_name using its bound argument schema (a listed tool without input_schema needs one list_mcp_tools call with select:[callable_name]); if invocation is unavailable, explain the concrete blocker or ask a useful task question. Do not claim a tool ran without its result.".to_string();
    }
    if action == REVERIFICATION_ACTION {
        return format!(
            "The harness observed {streak} consecutive rounds that mostly re-read files already in the transcript, with no edits in between. Stop re-verifying: the results already in the transcript are current. Do the one concrete step that is still missing, or give the final summary now. Do not re-read a file unless an edit changed it or a specific missing line is identified. This recovery is bounded and preserves completed tool results."
        );
    }
    let repeated_verification = action.starts_with("verification:");
    if repeated_verification {
        return format!(
            "The harness observed a repeated successful verification for `{action}`. That check already passed after the latest edit; do not rerun it unless the workspace changes or new evidence invalidates it. Continue from the result already in the transcript, or explain the blocker. This recovery is bounded and preserves completed tool results."
        );
    }
    format!(
        "The harness observed {} for `{action}` {streak} consecutive result(s) without a state change. Do not replay that action. Continue from the result already in the transcript: choose one different, bounded evidence-producing step or explain the blocker. This recovery is bounded; preserve completed tool results and do not claim an action ran unless its result is present.",
        reason.label(),
    )
}

fn shell_call_is_read_only(call: &crate::tools::ToolCall) -> bool {
    crate::tools::is_read_only_call(call)
}

/// Outcome of scheduling one model response: the calls to execute now, the
/// valid calls the harness takes over so they run in a later round, and the
/// ones it refuses to take over.
///
/// Dropping an over-budget call and telling the model to reissue it cost a
/// whole provider round trip for work the harness already held (#1590). Queued
/// calls keep their identity, execute under their own budget, and never ask
/// the model for them again. Invalid and duplicate calls are neither queued nor
/// reissued: they are answered in this round instead.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledCalls {
    selected: Vec<usize>,
    queued: Vec<usize>,
    reissue: Vec<usize>,
}

impl ScheduledCalls {
    fn selected_only(selected: Vec<usize>) -> Self {
        Self {
            selected,
            queued: Vec::new(),
            reissue: Vec::new(),
        }
    }
}

/// Per-round mutation allowance for one scheduling decision. A drain of the
/// deferred queue uses the same cap so it can never re-enter a round's budget.
fn mutation_cap(policy: crate::config::ToolSchedulingPolicy) -> usize {
    policy.max_mutating_calls.max(1)
}

/// Whether the harness may run this call later instead of asking the model to
/// reissue it (#1590).
///
/// A completion request answers the model rather than the workspace, and it
/// arrives over budget because `complete_task` is deliberately not
/// read-only. Queueing it would park the end of the turn behind a queued edit
/// and — with the "do not reissue" notice — leave the task unable to finish, so
/// it stays with the model.
fn is_queueable(call: &crate::tools::ToolCall) -> bool {
    call.name != "complete_task"
}

/// The calls the harness is holding for this round (#1590), in queue order,
/// together with the length of the queue prefix they cover.
///
/// The prefix is always a strict prefix and the batch is bounded by a fresh
/// mutation allowance, so a non-empty queue always shrinks and the drain can
/// never re-enter the current round's budget or spin. A queued call the model
/// re-issued anyway is covered by that round's execution instead of running
/// twice. Only calls queued by *earlier* rounds are eligible: the point of the
/// mutation limit is that the model reads one mutation's result before the
/// next one runs.
fn take_deferred_batch(
    queued: &[crate::tools::ToolCall],
    policy: crate::config::ToolSchedulingPolicy,
    selected: &[crate::tools::ToolCall],
) -> (Vec<crate::tools::ToolCall>, usize) {
    let cap = mutation_cap(policy);
    let mut seen = selected
        .iter()
        .map(crate::tools::duplicate_tool_call_key)
        .collect::<std::collections::HashSet<_>>();
    let mut mutating = 0;
    let mut batch = Vec::new();
    let mut prefix = 0;
    for call in queued {
        if mutating >= cap && !shell_call_is_read_only(call) {
            break;
        }
        prefix += 1;
        if shell_call_is_read_only(call) {
            if seen.insert(crate::tools::duplicate_tool_call_key(call)) {
                batch.push(call.clone());
            }
            continue;
        }
        mutating += 1;
        if seen.insert(crate::tools::duplicate_tool_call_key(call)) {
            batch.push(call.clone());
        }
    }
    (batch, prefix)
}

/// One executable unit of a round's batch.
///
/// `announced` is the call's index in the model's announcement, which is both
/// its transcript position and the provider call id its result must answer.
/// Harness-queued calls (#1590) have no announcement of their own: their
/// earlier announcement was already closed, so their result is harness-authored
/// and carries no call id.
struct BatchEntry {
    call: crate::tools::ToolCall,
    answered_call: Option<String>,
    announced: usize,
}

impl BatchEntry {
    fn from_announcement(
        call: &crate::tools::ToolCall,
        index: usize,
        call_refs: &[crate::app::ToolCallRef],
    ) -> Self {
        Self {
            call: call.clone(),
            answered_call: call_refs.get(index).map(|call_ref| call_ref.id.clone()),
            announced: index,
        }
    }

    fn from_deferred(call: crate::tools::ToolCall) -> Self {
        Self {
            call,
            answered_call: None,
            announced: usize::MAX,
        }
    }
}

#[cfg(test)]
fn selected_tool_call_indices(
    calls: &[crate::tools::ToolCall],
    validation_errors: &[Option<String>],
    policy: crate::config::ToolSchedulingPolicy,
) -> Vec<usize> {
    scheduled_tool_calls(calls, validation_errors, policy).selected
}

fn scheduled_tool_calls(
    calls: &[crate::tools::ToolCall],
    validation_errors: &[Option<String>],
    policy: crate::config::ToolSchedulingPolicy,
) -> ScheduledCalls {
    let first_control = calls.iter().enumerate().find(|(index, call)| {
        validation_errors[*index].is_none()
            && matches!(
                crate::tools::tool_safety(&call.name),
                crate::tools::ToolSafety::ControlPlane
            )
    });
    if let Some((index, control)) = first_control {
        // Control-plane calls run first for ordering, but valid read-only
        // companions follow in the same batch (issue #1230) instead of
        // costing a whole extra model round-trip for reissue. The executor
        // runs the batch sequentially, so the control result still lands
        // before any companion starts. Mutating companions, duplicates, and
        // invalid calls never ride along: the mutating ones are queued for the
        // harness instead (#1590).
        let mut selected = vec![index];
        let mut queued = Vec::new();
        let mut reissue = Vec::new();
        let mut seen = std::collections::HashSet::new();
        seen.insert(crate::tools::duplicate_tool_call_key(control));
        for (other, call) in calls.iter().enumerate() {
            if other == index || validation_errors[other].is_some() {
                continue;
            }
            if matches!(
                crate::tools::tool_safety(&call.name),
                crate::tools::ToolSafety::ControlPlane
            ) {
                // The control-plane barrier runs one call per round; a second
                // one waits for the model rather than for the harness.
                reissue.push(other);
                continue;
            }
            if !seen.insert(crate::tools::duplicate_tool_call_key(call)) {
                continue;
            }
            if shell_call_is_read_only(call) {
                selected.push(other);
            } else if is_queueable(call) {
                queued.push(other);
            } else {
                reissue.push(other);
            }
        }
        return ScheduledCalls {
            selected,
            queued,
            reissue,
        };
    }

    let first_valid = calls
        .iter()
        .enumerate()
        .find(|(index, _)| validation_errors[*index].is_none())
        .map(|(index, _)| index);
    let Some(first_valid) = first_valid else {
        return ScheduledCalls::selected_only(if calls.is_empty() {
            Vec::new()
        } else {
            vec![0]
        });
    };
    // Permissive scheduling: every valid read-only call runs — batching
    // independent inspection is the fast path, not a policy violation.
    // Workspace mutations stay ordered at one per round by default (more
    // only for explicitly trusted batching profiles), and control-plane
    // calls still execute alone above. Over-budget calls are queued for the
    // harness (#1590); invalid and duplicate calls remain in the transcript as
    // non-executed results.
    let mut selected = Vec::new();
    let mut queued = Vec::new();
    let mut reissue = Vec::new();
    let mut mutating = 0;
    let mut seen = std::collections::HashSet::new();
    let cap = mutation_cap(policy);
    for (index, call) in calls.iter().enumerate() {
        if validation_errors[index].is_some()
            || matches!(
                crate::tools::tool_safety(&call.name),
                crate::tools::ToolSafety::ControlPlane
            )
        {
            continue;
        }
        if !seen.insert(crate::tools::duplicate_tool_call_key(call)) {
            continue;
        }
        if !shell_call_is_read_only(call) {
            if mutating >= cap {
                if is_queueable(call) {
                    queued.push(index);
                } else {
                    reissue.push(index);
                }
                continue;
            }
            mutating += 1;
        }
        selected.push(index);
    }
    if selected.is_empty() {
        return ScheduledCalls::selected_only(vec![first_valid]);
    }

    ScheduledCalls {
        selected,
        queued,
        reissue,
    }
}

const MAX_MALFORMED_TOOL_HISTORY_BYTES: usize = 4096;

/// Keep malformed provider output useful for diagnostics without replaying a
/// potentially megabyte-sized partial tool argument into every retry.
fn bounded_malformed_tool_history(content: &str) -> String {
    if content.len() <= MAX_MALFORMED_TOOL_HISTORY_BYTES {
        return content.to_owned();
    }

    // Leave ample room for the marker, including the decimal byte count.
    let preview_limit = MAX_MALFORMED_TOOL_HISTORY_BYTES.saturating_sub(128);
    let mut preview_end = preview_limit.min(content.len());
    while preview_end > 0 && !content.is_char_boundary(preview_end) {
        preview_end -= 1;
    }
    let marker = format!(
        "\n[malformed tool response truncated by harness; {} bytes omitted]",
        content.len().saturating_sub(preview_end)
    );
    format!("{}{}", &content[..preview_end], marker)
}

/// How a finished batch's results split for `tools.batch.finish`. A pending
/// result (a background start that reports on completion) has not failed, so
/// it is counted apart instead of inflating `failed` (#2501).
#[derive(Debug, Default, PartialEq, Eq)]
struct BatchOutcomeCounts {
    successes: usize,
    failed: usize,
    pending: usize,
}

impl BatchOutcomeCounts {
    fn of(results: &[ToolResult]) -> Self {
        let mut counts = Self::default();
        for result in results {
            if result.metadata.pending {
                counts.pending += 1;
            } else if result.metadata.success {
                counts.successes += 1;
            } else {
                counts.failed += 1;
            }
        }
        counts
    }
}

fn incomplete_tool_result(metadata: &crate::network::events::ToolResultMetadata) -> bool {
    metadata.truncated
        || matches!(
            metadata.completeness,
            rustcode_core::ToolResultCompleteness::LineTruncated
                | rustcode_core::ToolResultCompleteness::ByteTruncated
        )
}

fn benign_shell_wrapper_failure(
    call: Option<&crate::tools::ToolCall>,
    metadata: &crate::network::events::ToolResultMetadata,
    _content: &str,
) -> bool {
    let Some(call) = call else {
        return false;
    };
    // Benign-ness is structural: grep/rg exit 1 *is* the understood no-match
    // signal, and 141 with `head` in the pipeline is SIGPIPE after enough
    // data. Never scan output text for "error" here: a no-match report for a
    // pattern like "error" (or harness text quoting it) is still a legitimate
    // negative result, not a failure worth loop-recovery evidence.
    if call.name != "run_command" || metadata.success {
        return false;
    }
    let Some(command) = call
        .arguments
        .get("command")
        .and_then(|value| value.as_str())
    else {
        return false;
    };
    let command = command.trim();
    let stages = command.split('|').map(str::trim).collect::<Vec<_>>();
    let has_search_stage = stages.iter().any(|stage| {
        let executable = stage.split_whitespace().next().unwrap_or_default();
        matches!(executable, "grep" | "rg")
            || (executable == "git" && stage.split_whitespace().nth(1) == Some("grep"))
    });
    match metadata.exit_code {
        // grep/rg use 1 for a understood no-match result. Restrict this to a
        // simple pipeline so a later command failure is never hidden.
        Some(1) => has_search_stage && !command.contains("&&") && !command.contains(';'),
        // `head` commonly closes a pipe after it has enough data, leaving the
        // upstream producer with SIGPIPE. This remains a failed command in
        // the transcript; it is only excluded from loop-recovery evidence.
        Some(141) => {
            stages.len() > 1
                && stages.iter().any(|stage| {
                    let executable = stage.split_whitespace().next().unwrap_or_default();
                    executable == "head"
                })
        }
        _ => false,
    }
}

fn content_bearing_inspection_status(
    call: Option<&crate::tools::ToolCall>,
    metadata: &crate::network::events::ToolResultMetadata,
    content: &str,
) -> Option<bool> {
    let call = call?;
    if !metadata.success || metadata.pending || content.trim().is_empty() {
        return None;
    }
    // Archive reads establish historical conversation facts, without posing
    // as fresh source-file inspections. Previews carry no completion evidence;
    // a complete requested message page can support a recall-only answer.
    if call.name == "zoom_context" {
        return call
            .arguments
            .get("message")
            .and_then(serde_json::Value::as_u64)
            .map(|_| !incomplete_tool_result(metadata) && !metadata.payload_truncated);
    }
    // MCP reads do not carry the file-range inspection metadata emitted by
    // source tools. Count only known read-only MCP results (or tools with an
    // explicit readOnlyHint); a deferred call has success=false above.
    let mail_read = matches!(
        call.name.rsplit("__").next().unwrap_or(&call.name),
        "list_emails" | "search_emails" | "read_email" | "list_attachments"
    );
    if metadata.inspection.is_none()
        && matches!(
            crate::tools::tool_safety(&call.name),
            crate::tools::ToolSafety::Unknown
        )
        && (mail_read || crate::tools::is_read_only_call(call))
    {
        return Some(!incomplete_tool_result(metadata) && !metadata.payload_truncated);
    }
    let inspection = metadata.inspection.as_ref()?;
    if !loop_detect::read_returns_content(&call.name, &call.arguments)
        || inspection.fingerprint.trim().is_empty()
        || inspection.requested_path.is_none()
        || inspection.returned_path.is_none()
    {
        return None;
    }
    Some(inspection.complete && !incomplete_tool_result(metadata))
}

fn bounded_repair_failure(content: &str) -> String {
    const MAX_FAILURE_CHARS: usize = 512;
    let first_line = content.lines().next().unwrap_or(content).trim();
    if first_line.len() <= MAX_FAILURE_CHARS {
        return first_line.to_string();
    }
    let mut end = MAX_FAILURE_CHARS;
    while end > 0 && !first_line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &first_line[..end])
}

fn grounded_artifact_recovery_message(
    evidence: &GroundedArtifactEvidence,
    failure: &str,
    exhausted: bool,
) -> String {
    let failure = bounded_repair_failure(failure);
    let write_evidence = match (evidence.write_lines, evidence.write_bytes) {
        (Some(lines), Some(bytes)) => format!("{lines} lines / {bytes} bytes"),
        (Some(lines), None) => format!("{lines} lines"),
        (None, Some(bytes)) => format!("{bytes} bytes"),
        (None, None) => "known successful mutation".to_string(),
    };
    let read_evidence = evidence
        .read_range
        .map(|(start, end)| format!("complete read of lines {start}-{end}"))
        .unwrap_or_else(|| "complete read evidence".to_string());
    if exhausted {
        format!(
            "[Grounded artifact recovery exhausted: '{}' was successfully written ({write_evidence}); {read_evidence} confirmed the artifact needs repair; the last repair attempt made no change ({failure}). No safe targeted repair has succeeded. Do not reread the whole file or replay the successful write. Stop with an actionable incomplete/recoverable response.]",
            evidence.path
        )
    } else {
        format!(
            "[Grounded artifact recovery: '{}' was successfully written ({write_evidence}); {read_evidence} was returned and the artifact was reported malformed; the last repair attempt made no change ({failure}). Use that existing evidence to issue exactly one narrow replace_file_content for the malformed suffix. Do not reread the whole file or replay the successful write; if the exact replacement is not provable, report the blocker instead of guessing.]",
            evidence.path
        )
    }
}

fn same_recovery_path(left: &str, right: &str) -> bool {
    left == right || crate::tools::resolve_tool_path(left) == crate::tools::resolve_tool_path(right)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_tool_response<P: policy::TurnPolicy + 'static>(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    policy: &Arc<P>,
    ctx: &mut TurnContext,
    response_finish_reason: Option<&str>,
    turn_response_time_ms: u64,
    turn_token_usage: Option<TokenUsage>,
    thought_time_ms: Option<u64>,
    thought_tokens: Option<u32>,
    native_tool_calls: Vec<crate::tools::ToolCallEnvelope>,
    turn_session_id: &str,
) -> ToolHandlingOutcome {
    // Phase 3: normalize provider output into protocol-independent events.
    let protocol = {
        let state = state.lock().await;
        state.active_tool_protocol()
    };
    let model_response = if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        let typed_calls = native_tool_calls
            .into_iter()
            .map(|call| crate::tools::ToolCall {
                name: call.tool_name,
                arguments: call.arguments,
                call_id: Some(call.call_id),
            })
            .collect();
        events::native_response(
            &ctx.response.final_content,
            response_finish_reason,
            typed_calls,
        )
    } else {
        events::normalize_response(
            &ctx.response.final_content,
            response_finish_reason,
            protocol,
        )
    };
    dbg_log!(
        "Model response normalized from {:?}; raw length={} chars",
        model_response.source,
        model_response.raw_content.len()
    );
    let response_events = model_response.events;
    let parsed_tool_calls = response_events
        .iter()
        .filter_map(|event| match event {
            events::AgentEvent::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let response_was_output_truncated = response_events.iter().any(|event| {
        matches!(
            event,
            events::AgentEvent::Finished(events::FinishReason::Length)
        )
    });
    let parsed_tool_calls = parsed_tool_calls
        .into_iter()
        .map(|mut call| {
            if let Some(canonical) = crate::tools::resolve_builtin_tool_alias(&call.name) {
                crate::logger::operational_event(
                    "tools.alias_applied",
                    serde_json::json!({
                        "alias": call.name,
                        "canonical": canonical,
                    }),
                );
                call.name = canonical.to_string();
            }
            call
        })
        .collect::<Vec<_>>();

    // A provider length stop is not evidence that a structured call is
    // complete. Some providers emit syntactically valid JSON before cutting
    // the response, leaving arguments silently truncated. Fail every call in
    // this response closed and persist the complete assistant/call + result
    // transaction so the next request can safely re-issue a smaller call.
    if response_was_output_truncated && !parsed_tool_calls.is_empty() {
        let call_refs = call_refs_for(&parsed_tool_calls, &ctx.response.streamed_call_ids);
        let recovery = &mut ctx.recovery.output_limit;
        recovery.truncated_tool_calls = recovery.truncated_tool_calls.saturating_add(1);
        let retry =
            recovery.truncated_tool_calls <= super::recovery::MAX_TRUNCATED_TOOL_CALL_RETRIES;
        if retry {
            // More room for the reissued call; the guidance asks for a
            // smaller one.
            recovery.pending = Some(super::context::OutputRecoveryRequest {
                reduce_reasoning: false,
            });
        }
        crate::logger::operational_event(
            if retry {
                "turn.output_budget_continuation"
            } else {
                "turn.output_budget_exhausted"
            },
            serde_json::json!({
                "kind": "truncated_native_tool_call",
                "attempt": ctx.recovery.output_limit.truncated_tool_calls,
                "calls": parsed_tool_calls.len(),
                "request_output_limit": ctx.recovery.output_limit.last_request_limit,
                "output_tokens": ctx.recovery.output_limit.last_output_tokens,
                "outcome": if retry { "continuation_requested" } else { "incomplete" },
            }),
        );
        let mut s = state.lock().await;
        let mut message = ChatMessage::new("assistant", &ctx.response.final_content)
            .with_tool_calls(call_refs.clone())
            .as_unexecuted_tool_call_checkpoint();
        message.response_time_ms = Some(turn_response_time_ms);
        message.token_usage = turn_token_usage;
        message.thought_time_ms = thought_time_ms;
        message.thought_tokens = thought_tokens;
        s.history.push(message);
        ctx.response.final_content_persisted = true;
        s.history.extend(unanswered_call_results_with_kind(
            &call_refs,
            "provider stopped at the output limit; the call was not executed",
            crate::tools::ToolErrorKind::OutputLimit,
        ));
        if !retry {
            let reason =
                super::recovery::output_limit_stop_reason(s.active_model_profile().as_ref(), ctx);
            ctx.response.final_content = super::recovery::output_limit_final_response(&reason);
            ctx.response.final_content_persisted = false;
            ctx.lifecycle.task_completed = false;
            ctx.lifecycle.stop_reason = Some(reason);
            crate::config::save_session_history(&s.active_session_id, &s.history);
            s.clear_current_response();
            drop(s);
            ctx.lifecycle.turn_machine.abandon_tool_phase();
            return ToolHandlingOutcome::Stop;
        }
        s.history.push(ChatMessage::new(
            "system",
            super::recovery::TRUNCATED_TOOL_CALL_GUIDANCE,
        ));
        crate::config::save_session_history(&s.active_session_id, &s.history);
        s.clear_current_response();
        s.status = AppStatus::Streaming;
        s.stream_tracker = Some(StreamTracker::new());
        drop(s);
        ctx.budget.tool_rounds += 1;
        return ToolHandlingOutcome::Continue;
    }

    if !parsed_tool_calls.is_empty() {
        // A complete tool request arrived: earlier cut-off calls are behind.
        ctx.recovery.output_limit.truncated_tool_calls = 0;
    }
    let requested_calls = parsed_tool_calls.len();
    let validation_errors = crate::tools::validation_errors_by_call(&parsed_tool_calls);
    let scheduling_policy = {
        let s = state.lock().await;
        s.active_model_profile()
            .map(|profile| profile.tool_scheduling_policy())
            .unwrap_or_default()
    };
    populate_shell_assessments(state, ctx, &parsed_tool_calls).await;
    // Preserve control-plane priority (for example, load a requested skill
    // before acting). The default policy selects one valid call; explicitly
    // trusted profiles may select a bounded read/mutation batch. Invalid calls
    // remain in the transcript as non-executed results, and over-budget calls
    // are handed to the harness queue instead of being dropped (#1590).
    let scheduled = scheduled_tool_calls(&parsed_tool_calls, &validation_errors, scheduling_policy);
    let selected_call_indices = scheduled.selected;
    let selected_call_index = selected_call_indices.first().copied();
    if let Some(reason) = selected_call_index.and_then(|index| validation_errors[index].clone()) {
        if lifecycle::is_unavailable_tool_error(&reason) {
            ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::UnavailableTool);
        }
        let raw_content = ctx.response.final_content.clone();
        let repeated_malformed = record_malformed_call(ctx, &raw_content, &parsed_tool_calls);
        dbg_log!("Tool-call validation rejected response: {}", reason);
        let mut s = state.lock().await;
        let rejected_refs = call_refs_for(&parsed_tool_calls, &ctx.response.streamed_call_ids);
        let mut msg = ChatMessage::new("assistant", ctx.response.final_content.clone())
            .with_tool_calls(rejected_refs.clone());
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        msg.thought_time_ms = thought_time_ms;
        msg.thought_tokens = thought_tokens;
        s.history.push(msg);
        ctx.response.final_content_persisted = true;
        for message in unanswered_call_results_with_kind(
            &rejected_refs,
            &reason,
            crate::tools::ToolErrorKind::Validation,
        ) {
            s.history.push(message);
        }
        ctx.recovery.oversized_batch_rejections = 0;
        let repeat_guidance = if repeated_malformed {
            format!(
                " This is the same invalid tool request repeated {} times. Stop retrying this exact shape; re-read the schema and re-plan, or respond with text explaining what remains.",
                ctx.recovery.consecutive_malformed_calls
            )
        } else {
            String::new()
        };
        // The reason is already the tool result above; repeating it here
        // reported one rejection twice (#1886).
        s.history.push(ChatMessage::new(
            "system",
            format!(
                "[Tool call rejected before execution: see the tool result above] Emit one corrected tool call. {}{}",
                mutation_batch_guidance(&scheduling_policy),
                repeat_guidance
            ),
        ));
        crate::config::save_session_history(&s.active_session_id, &s.history);
        s.clear_current_response();
        s.status = AppStatus::Streaming;
        drop(s);
        ctx.budget.tool_rounds += 1;
        return ToolHandlingOutcome::Continue;
    }
    ctx.recovery.oversized_batch_rejections = 0;
    let tool_calls = parsed_tool_calls;
    let deferred_call_count = tool_calls.len().saturating_sub(selected_call_indices.len());
    let unexecuted_call_count = deferred_call_count;
    let read_only_batch = !selected_call_indices.is_empty()
        && selected_call_indices.iter().all(|index| {
            tool_calls
                .get(*index)
                .is_some_and(|call| shell_call_is_read_only(call))
        });
    let call_refs = call_refs_for(&tool_calls, &ctx.response.streamed_call_ids);
    // Hand the calls this round's mutation budget could not take to the
    // harness queue. They keep their identity and are executed automatically
    // in a later round, so the model never pays a round trip to reissue them
    // (#1590). A new user prompt or turn end releases the queue.
    let queued_call_count = scheduled.queued.len();
    let queued_before = {
        let mut s = state.lock().await;
        let before = s.deferred_tool_calls.len();
        s.deferred_tool_calls.extend(
            scheduled
                .queued
                .iter()
                .map(|index| tool_calls[*index].clone()),
        );
        before
    };
    if queued_call_count > 0 {
        dbg_log!(
            "Scheduler queued {} over-budget call(s) for a later round",
            queued_call_count
        );
    }
    // A response without calls would end the turn and release the queue, but
    // the model was told the held calls run without another request from it.
    // Treat the queue as work for this round so they run and their results
    // reach the model before it can finish (#1857).
    let held_calls_pending =
        tool_calls.is_empty() && !state.lock().await.deferred_tool_calls.is_empty();
    if held_calls_pending {
        dbg_log!("Model answered without calls while the harness still holds queued calls");
    }
    let turn_action = match ctx.lifecycle.turn_machine.model_finished(
        cancel_token.is_cancelled(),
        ctx.recovery.force_final,
        !tool_calls.is_empty() || held_calls_pending,
        ctx.lifecycle.task_completed,
    ) {
        Ok(action) => action,
        Err(invalid) => {
            dbg_log!("Turn machine rejected model_finished: {invalid}");
            crate::logger::operational_event(
                "turn.invalid_transition",
                serde_json::json!({
                    "stage": "model_finished",
                    "detail": invalid.to_string(),
                }),
            );
            events::TurnAction::FinishResponse
        }
    };
    if turn_action == events::TurnAction::Cancel {
        return ToolHandlingOutcome::Stop;
    }
    if matches!(turn_action, events::TurnAction::ExecuteTools) {
        ctx.recovery.consecutive_malformed_calls = 0;
        ctx.recovery.last_malformed_call = None;
        dbg_log!("Parsed {} tool call requests", tool_calls.len());

        let mut loop_status = loop_detect::LoopStatus::Ok;
        let mut loop_offender: Option<String> = None;
        // The pre-execution call-repetition warning is parked here and only
        // pushed after the results are in, when no stronger recovery speaks.
        // This keeps one model-visible guidance per round.
        let mut pending_loop_warning: Option<String> = None;
        for index in &selected_call_indices {
            let call = &tool_calls[*index];
            let (exact, category) = loop_detect::signatures(&call.name, &call.arguments);
            let s = ctx
                .recovery
                .loop_detector
                .check_tool(&call.name, &exact, &category);
            if s.rank() > loop_status.rank() {
                loop_status = s;
                loop_offender = Some(format!("{} ({category})", call.name));
            }
            if tool_changes_workspace(&call.name, Some(&call.arguments)) {
                if let Some(root) = get_tool_project_root(&call.name, &call.arguments) {
                    ctx.compiler.edit_root = Some(root);
                    ctx.compiler.dirty = true;
                }
            }
        }
        match loop_status {
            loop_detect::LoopStatus::Abort(n) => {
                let local_recovery_action =
                    loop_recovery_action_for(ctx.recovery.loop_recovery_attempts, read_only_batch);
                match local_recovery_action {
                    LoopRecoveryAction::Recover => {
                        ctx.recovery.loop_recovery_attempts =
                            ctx.recovery.loop_recovery_attempts.saturating_add(1);
                        log_recovery_decision(ctx, "tool_loop", "recover", "loop_detector_abort");
                        ctx.recovery.loop_detector.reset();
                        dbg_log!(
                            "Loop detector: abort after {} repeats — allowing bounded recovery turn",
                            n
                        );
                        let mut s = state.lock().await;
                        let mut msg = ChatMessage::new("assistant", &ctx.response.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        msg.thought_time_ms = thought_time_ms;
                        msg.thought_tokens = thought_tokens;
                        s.history.push(msg);
                        ctx.response.final_content_persisted = true;
                        let recovery_prompt = loop_recovery_prompt(
                            &s.history,
                            ctx.progress.made_edits,
                            ctx.compiler.consecutive_diagnostics > 0
                                || ctx.compiler.consecutive_error_gates > 0,
                        );
                        push_or_replace_recovery_notice(
                            s.history.as_mut_vec(),
                            recovery_prompt.to_string(),
                        );
                        crate::config::save_session_history(&s.active_session_id, &s.history);
                        s.clear_current_response();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.lifecycle.turn_machine.abandon_tool_phase();
                        ctx.budget.tool_rounds += 1;
                        return ToolHandlingOutcome::Continue;
                    }
                    LoopRecoveryAction::ForceFinal => {
                        log_recovery_decision(
                            ctx,
                            "tool_loop",
                            "force_final",
                            "loop_detector_abort",
                        );
                        dbg_log!(
                            "Loop detector: abort after {} repeats — forcing wrap-up turn",
                            n
                        );
                        let mut s = state.lock().await;
                        let mut msg = ChatMessage::new("assistant", &ctx.response.final_content);
                        msg.response_time_ms = Some(turn_response_time_ms);
                        msg.token_usage = turn_token_usage.clone();
                        msg.thought_time_ms = thought_time_ms;
                        msg.thought_tokens = thought_tokens;
                        s.history.push(msg);
                        ctx.response.final_content_persisted = true;
                        s.history
                            .push(ChatMessage::new("system", FORCE_ANSWER_PROMPT));
                        crate::config::save_session_history(&s.active_session_id, &s.history);
                        s.clear_current_response();
                        drop(s);
                        ctx.lifecycle.turn_machine.abandon_tool_phase();
                        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                        ctx.recovery.force_final = true;
                        return ToolHandlingOutcome::Continue;
                    }
                }
            }
            loop_detect::LoopStatus::Warning(n) => {
                dbg_log!("Loop detector: warning at {} repeats", n);
                let action = loop_offender.as_deref().unwrap_or("the last tool action");
                pending_loop_warning = Some(format!(
                    "[Loop warning: '{action}' has repeated {n} times. If it keeps failing or returning the same thing, stop retrying the same inputs — for an edit that failed to match, view a wider line range or use grep to verify exact target content. If you are deliberately re-checking a value that changes, space the checks out with other work.]"
                ));
            }
            loop_detect::LoopStatus::Ok => {}
        }

        if !cancel_token.is_cancelled() {
            ctx.budget.tool_rounds += 1;

            // Run the calls the harness has been holding since an earlier round
            // (#1590) after this round's own selections, under a fresh mutation
            // allowance. Peeking here — after every early return above — means
            // an abandoned round leaves the queue intact instead of dropping
            // calls it never ran.
            let announced_calls = selected_call_indices
                .iter()
                .map(|index| tool_calls[*index].clone())
                .collect::<Vec<_>>();
            let (deferred_batch, deferred_prefix) = {
                let s = state.lock().await;
                // A cancellation can clear the queue between rounds; never read
                // past whatever is still held.
                let held = queued_before.min(s.deferred_tool_calls.len());
                take_deferred_batch(
                    &s.deferred_tool_calls[..held],
                    scheduling_policy,
                    &announced_calls,
                )
            };
            let mut batch = announced_calls
                .iter()
                .zip(&selected_call_indices)
                .map(|(call, index)| BatchEntry::from_announcement(call, *index, &call_refs))
                .collect::<Vec<_>>();
            let auto_executed_count = deferred_batch.len();
            batch.extend(deferred_batch.into_iter().map(BatchEntry::from_deferred));
            let executable_tool_calls = batch
                .iter()
                .map(|entry| entry.call.clone())
                .collect::<Vec<_>>();
            if auto_executed_count > 0 {
                populate_shell_assessments(state, ctx, &executable_tool_calls).await;
                dbg_log!(
                    "Executing {} harness-queued call(s) from an earlier round",
                    auto_executed_count
                );
            }

            let approved = policy
                .should_approve_with_assessments(
                    state,
                    &executable_tool_calls,
                    &ctx.shell_assessments,
                )
                .await;

            {
                let mut s = state.lock().await;
                s.pending_tool_confirmation = None;
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                s.note_turn_progress();
                let mut msg = ChatMessage::new("assistant", &ctx.response.final_content)
                    .with_tool_calls(call_refs.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.response.final_content_persisted = true;
                crate::config::save_session_history(&s.active_session_id, &s.history);
                // An approval prompt for the batch waited on the user, not on
                // the harness (#1891).
                let approval_wait = std::mem::take(&mut s.approval_wait);
                ctx.lifecycle.user_wait_duration += approval_wait;
                ctx.performance.user_wait_us += crate::benchmark::duration_us(approval_wait);
            }

            let transition = if approved {
                ctx.lifecycle.turn_machine.approval_granted()
            } else {
                ctx.lifecycle.turn_machine.approval_denied()
            };
            if let Err(invalid) = transition {
                dbg_log!("Turn machine rejected approval transition: {invalid}");
                crate::logger::operational_event(
                    "turn.invalid_transition",
                    serde_json::json!({
                        "stage": "approval",
                        "approved": approved,
                        "detail": invalid.to_string(),
                    }),
                );
            }

            // Phase 4: execute only the selected, complete calls and record
            // progress evidence. The other calls are closed below, never
            // silently dropped or described as if they ran.
            let tools_started = std::time::Instant::now();
            let user_wait_before = ctx.lifecycle.user_wait_duration;
            let workspace_root = {
                let mut s = state.lock().await;
                // Rows of calls that finish stay listed until the batch is
                // recorded below.
                s.retain_finished_live_tool_calls();
                s.effective_workspace_root()
            };
            let absent_before =
                absent_write_targets(&executable_tool_calls, workspace_root.as_deref());
            let results = super::super::tool_exec::execute_tool_batch_with_assessments(
                client,
                state,
                cancel_token,
                &executable_tool_calls,
                ctx.lifecycle.turn_machine.state() == events::TurnState::ExecutingTools,
                &ctx.compiler.edit_root,
                &mut ctx.compiler.dirty,
                &mut ctx.compiler.cache,
                &mut ctx.lifecycle.user_wait_duration,
                None,
                &ctx.shell_assessments,
            )
            .await;
            if executable_tool_calls.len() > 1
                && executable_tool_calls
                    .iter()
                    .all(super::super::tool_exec::parallel_inspection)
            {
                ctx.performance.parallel_groups += 1;
            }
            ctx.performance.record_tool_batch(
                crate::benchmark::elapsed_us(tools_started),
                results
                    .iter()
                    .map(|result| result.metadata.execution_us)
                    .sum::<u64>(),
                crate::benchmark::duration_us(
                    ctx.lifecycle
                        .user_wait_duration
                        .saturating_sub(user_wait_before),
                ),
            );
            ctx.performance.replayed_reads += results
                .iter()
                .filter(|result| result.metadata.replayed)
                .count();
            // The peeked prefix is answered one way or another — result,
            // denial, or cancellation — so retire it now. A cancellation may
            // have released the queue while the batch ran.
            if deferred_prefix > 0 {
                let mut s = state.lock().await;
                let retire = deferred_prefix.min(s.deferred_tool_calls.len());
                s.deferred_tool_calls.drain(..retire);
            }
            let mut executed_results = results.into_iter();
            let results = batch
                .iter()
                .map(|entry| {
                    executed_results.next().unwrap_or_else(|| ToolResult {
                        tool_name: entry.call.name.clone(),
                        content: format!(
                            "error: tool execution missing for this call ({})",
                            entry.announced
                        ),
                        diff: None,
                        file_preview: None,
                        metadata: crate::network::events::ToolResultMetadata {
                            success: false,
                            error_kind: Some(crate::tools::ToolErrorKind::Internal),
                            retryable: true,
                            ..Default::default()
                        },
                    })
                })
                .collect::<Vec<_>>();

            ctx.metrics.tool_calls += results.len();
            ctx.metrics.mutating_tool_calls += batch
                .iter()
                .filter(|entry| !crate::tools::is_read_only_call(&entry.call))
                .count();
            let mutation_batch = results
                .iter()
                .any(|result| is_mutating_tool(&result.tool_name));
            if mutation_batch {
                let diagnostic = results
                    .iter()
                    .find_map(|result| compiler_diagnostic_fingerprint(&result.content));
                if let Some(fingerprint) = diagnostic {
                    update_compiler_diagnostic_streak(ctx, Some(fingerprint));
                } else if !ctx.compiler.dirty {
                    update_compiler_diagnostic_streak(ctx, None);
                } else if let Some(reason) = results.iter().find_map(|result| {
                    result
                        .content
                        .strip_prefix("__BUILD_UNVERIFIED__:")
                        .map(str::trim)
                }) {
                    update_compiler_outcome_streak(
                        ctx,
                        &CompilerCheckOutcome::UnverifiedInfrastructure {
                            reason: reason.to_string(),
                        },
                    );
                }
            }

            let outcomes = BatchOutcomeCounts::of(&results);
            crate::logger::operational_event(
                "tools.batch.finish",
                serde_json::json!({
                    "count": results.len(),
                    "requested": requested_calls,
                    "executed": results.len(),
                    "deferred": unexecuted_call_count,
                    // Calls handed to the harness queue this round, and how many
                    // of the previously queued ones ran automatically (#1590).
                    "queued": queued_call_count,
                    "auto_executed": auto_executed_count,
                    "successes": outcomes.successes,
                    "failed": outcomes.failed,
                    "pending": outcomes.pending,
                    "changed_paths": results.iter().map(|result| result.metadata.changed_paths.len()).sum::<usize>(),
                }),
            );

            if cancel_token.is_cancelled() {
                dbg_log!("Orchestrator: Cancelled during tool execution");
                crate::logger::operational_event(
                    "turn.loop_signal",
                    loop_signal_event(None, None, None, false, None, true),
                );
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
                // A cancelled round returns before the per-result loop, so
                // `last_reason` and `no_progress_results` still describe an
                // earlier round of this turn. Clearing the per-round reason and
                // counting the cancelled calls keeps the turn summary a
                // measurement of this turn instead of a carry-over (#1592).
                ctx.progress.last_reason = None;
                ctx.metrics.cancelled_tool_calls = ctx
                    .metrics
                    .cancelled_tool_calls
                    .saturating_add(results.len());
                let mut s = state.lock().await;
                s.clear_finished_live_tool_calls();
                // Pair by call id rather than position: harness-queued results
                // (#1590) share the batch but own no provider announcement.
                let mut announced_refs = Vec::new();
                let mut announced_results = Vec::new();
                let mut harness_results = Vec::new();
                for (entry, result) in batch.iter().zip(results) {
                    match entry.answered_call.clone() {
                        Some(call_id) => {
                            announced_refs.push(crate::app::ToolCallRef {
                                id: call_id,
                                name: entry.call.name.clone(),
                                arguments: entry.call.arguments.to_string(),
                            });
                            announced_results.push(result);
                        }
                        None => harness_results.push(result),
                    }
                }
                append_cancelled_batch_results(
                    s.history.as_mut_vec(),
                    announced_results,
                    &announced_refs,
                );
                for result in harness_results {
                    s.history.push(tool_result_history_message(result, None));
                }
                for (index, call_ref) in call_refs.iter().enumerate() {
                    if !selected_call_indices.contains(&index) {
                        s.history.extend(unanswered_call_results_with_kind(
                            std::slice::from_ref(call_ref),
                            "not executed because the model round was cancelled",
                            crate::tools::ToolErrorKind::Cancelled,
                        ));
                    }
                }
                append_pending_steers(&mut s, turn_session_id);
                if call_refs.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "Request cancelled by user"));
                }
                crate::config::save_session_history(&s.active_session_id, &s.history);
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Stop;
            }

            let mut s = state.lock().await;
            // The results are recorded under this lock, so the finished rows
            // hand over to their committed rows in one frame.
            s.clear_finished_live_tool_calls();
            s.status = AppStatus::Streaming;
            let mut completed = false;
            // Completion is valid only when every result in the same batch
            // was actually available and complete. A later turn may recover
            // by re-issuing a bounded read, so this is intentionally scoped
            // to the batch that requested completion.
            let mut batch_incomplete = unexecuted_call_count > 0;
            let explicit_verification_user_index = s
                .history
                .iter()
                .enumerate()
                .rev()
                .find(|(_, message)| message.role == "user")
                .map(|(index, _)| index);
            let explicit_verification_requested =
                explicit_verification_user_index.is_some_and(|index| {
                    verification::is_explicit_verification_request(&s.history[index].content)
                });
            if explicit_verification_requested {
                if let Some(index) = explicit_verification_user_index {
                    hydrate_explicit_verification_from_history(
                        &mut ctx.verification.ledger,
                        &s.history,
                        index,
                    );
                }
            }
            let mut stagnation = loop_detect::LoopStatus::Ok;
            let mut failure_replan = None;
            let mut evidence_recovery = None;
            let mut grounded_recovery = None;
            let mut targeted_recovery = false;
            let mut targeted_recovery_exhausted = false;
            // Batch-level stagnation: one model round contributes at most one
            // no-progress step no matter how many calls it batched, and any
            // meaningful result in the batch clears the streak.
            let mut round_had_meaningful = false;
            let mut round_no_progress_hits = 0usize;
            let mut round_outputs = std::collections::HashSet::new();
            let mut round_stale_rereads = 0usize;
            let mut round_assessed_results = 0usize;
            let mut round_workspace_changed = false;
            let mut cross_tool_inspection_cycle = None;
            let mut cross_turn_made_progress = false;
            let mut cross_turn_had_edits = false;
            let mut cross_turn_authoritative_progress = false;
            let mut cross_turn_target_files = Vec::new();
            let mut cross_turn_tool_count = 0;
            let mut infrastructure_stop: Option<(String, String, usize)> = None;
            let mut result_messages = Vec::with_capacity(results.len() + deferred_call_count);
            for (position, result) in results.into_iter().enumerate() {
                // `results` is built one entry per `batch` element, so the
                // lookup always succeeds. Harness-queued calls (#1590) occupy
                // the tail of `batch`, which keeps every announced position
                // mapped to its announcement and sorts their results after the
                // round's own answers.
                let entry = &batch[position];
                let call_position = entry.announced;
                let call = Some(&entry.call);
                let answered_call = entry.answered_call.clone();
                let name = result.tool_name;
                let mut metadata = result.metadata.clone();
                // The provider call id is attached at the orchestration
                // boundary, after parsing, and is then carried into the
                // history message that answers that exact call.
                metadata.call_id = answered_call.clone();
                let content = result.content;
                let diff_opt = result.diff;
                let file_preview = result.file_preview;
                // Set when the current result is a workspace-boundary
                // rejection: a harness guardrail hit, not model stagnation.
                let mut boundary_failure = false;
                let infrastructure_decision = ctx.recovery.infrastructure_failures.observe(
                    &name,
                    metadata.error_kind.map(|kind| kind.as_str()),
                    metadata.retryable,
                    metadata.success,
                    &content,
                );
                let infrastructure_stop_reached = matches!(
                    &infrastructure_decision,
                    loop_detect::InfrastructureFailureDecision::Stop { .. }
                );
                match infrastructure_decision {
                    loop_detect::InfrastructureFailureDecision::Allowed { failure, streak }
                    | loop_detect::InfrastructureFailureDecision::Stop { failure, streak } => {
                        crate::logger::operational_event(
                            "turn.infrastructure_failure",
                            serde_json::json!({
                                "round": ctx.budget.tool_rounds,
                                "tool": name,
                                "fingerprint": &failure.fingerprint,
                                "dependency": &failure.dependency,
                                "class": &failure.class,
                                "streak": streak,
                            }),
                        );
                        // Workspace-boundary rejections are harness
                        // guardrails, not model stagnation: they ride the
                        // infrastructure guard (reset + hint) and never feed
                        // evidence-based recovery.
                        if failure.fingerprint == loop_detect::WORKSPACE_BOUNDARY_FINGERPRINT {
                            boundary_failure = true;
                        }
                        if infrastructure_stop_reached {
                            infrastructure_stop =
                                Some((failure.fingerprint, failure.dependency, streak));
                        }
                    }
                    loop_detect::InfrastructureFailureDecision::NotInfrastructure
                    | loop_detect::InfrastructureFailureDecision::Cleared => {}
                }
                batch_incomplete |= incomplete_tool_result(&metadata);
                if let Some(complete) = content_bearing_inspection_status(call, &metadata, &content)
                {
                    if let Some(fingerprint) = metadata
                        .inspection
                        .as_ref()
                        .map(|inspection| inspection.fingerprint.as_str())
                        .filter(|fingerprint| !fingerprint.is_empty())
                    {
                        ctx.progress.record_inspection_result(fingerprint, complete);
                    } else if !complete {
                        ctx.progress.incomplete_inspection_results =
                            ctx.progress.incomplete_inspection_results.saturating_add(1);
                    } else {
                        ctx.progress.complete_inspection_results =
                            ctx.progress.complete_inspection_results.saturating_add(1);
                    }
                }
                if metadata.pending {
                    result_messages.push((
                        call_position,
                        tool_result_history_message(
                            ToolResult {
                                tool_name: name,
                                content,
                                diff: diff_opt,
                                file_preview,
                                metadata,
                            },
                            answered_call,
                        ),
                    ));
                    continue;
                }
                let mut verification_command = false;
                let mut repeated_successful_verification = false;
                if (name == "run_command" || metadata.command.is_some())
                    && let Some(command) = call
                        .and_then(|call| call.arguments.get("command"))
                        .and_then(|command| command.as_str())
                        .or(metadata.command.as_deref())
                {
                    verification_command = verification::is_verification_command(command)
                        || loop_detect::is_stable_inspection_command(command);
                    repeated_successful_verification = ctx
                        .verification
                        .ledger
                        .is_repeated_successful_command(command, metadata.exit_code)
                        && verification_command;
                    ctx.verification
                        .ledger
                        .set_scope(verification::VerificationScope::new(
                            workspace_root.as_deref(),
                            ctx.compiler.edit_root.as_deref(),
                            metadata.workspace_generation,
                        ));
                    ctx.verification
                        .ledger
                        .record_command(command, metadata.exit_code);
                    if explicit_verification_user_index.is_some_and(|index| {
                        verification::is_explicit_verification_command(
                            &s.history[index].content,
                            command,
                        )
                    }) {
                        ctx.verification
                            .ledger
                            .record_explicit_command(command, metadata.exit_code);
                    }
                }
                dbg_log!(
                    "Tool '{}' finished with result length: {} chars",
                    name,
                    content.len()
                );
                if name == "complete_task" && metadata.success {
                    completed = true;
                }
                for path in &metadata.changed_paths {
                    if !ctx.progress.changed_paths.contains(path)
                        && let Some(resolved) = absent_before.get(path)
                    {
                        ctx.progress
                            .created_paths
                            .insert(path.clone(), resolved.clone());
                    }
                }
                ctx.progress
                    .changed_paths
                    .extend(metadata.changed_paths.iter().cloned());
                if name == "todo_write" && metadata.success {
                    ctx.progress.phase_checkpoint = active_todo_checkpoint(&s.todos);
                    if let Some(phase) = ctx.progress.phase_checkpoint.as_deref() {
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("[phase checkpoint: {phase}]"),
                        ));
                    }
                }
                let mut mutation_progress = false;
                if tool_changes_workspace(&name, call.map(|call| &call.arguments)) {
                    let made_progress = mutation_made_progress(metadata.success, &content);
                    let failed = !made_progress;
                    mutation_progress =
                        made_progress && (!metadata.changed_paths.is_empty() || diff_opt.is_some());
                    if failed {
                        ctx.progress.failed_mutations += 1;
                        ctx.progress.consecutive_failed_mutations += 1;
                        if let Some(call) = call {
                            let (exact, category) =
                                loop_detect::signatures(&call.name, &call.arguments);
                            if let loop_detect::LoopStatus::Abort(repeats) = ctx
                                .recovery
                                .loop_detector
                                .record_failed_tool(&exact, &category)
                            {
                                failure_replan =
                                    Some(failure_replan_message(&call.name, &category, repeats));
                            }
                            if let Some(evidence) = ctx.progress.grounded_artifact.as_mut()
                                && evidence.read_range.is_some()
                                && call
                                    .arguments
                                    .get("path")
                                    .and_then(|value| value.as_str())
                                    .is_some_and(|path| same_recovery_path(path, &evidence.path))
                            {
                                let exhausted = evidence.repair_attempts > 0;
                                evidence.repair_attempts =
                                    evidence.repair_attempts.saturating_add(1);
                                grounded_recovery = Some(grounded_artifact_recovery_message(
                                    evidence,
                                    &bounded_repair_failure(&content),
                                    exhausted,
                                ));
                                targeted_recovery = true;
                                targeted_recovery_exhausted = exhausted;
                            }
                        }
                    } else {
                        ctx.progress.made_edits = true;
                        if metadata.changed_paths.is_empty()
                            || matches!(name.as_str(), "move_file" | "copy_file")
                        {
                            ctx.progress.untracked_edits = true;
                        }
                        ctx.progress.consecutive_failed_mutations = 0;
                        if metadata.changed_paths.is_empty() {
                            if name != "run_command" {
                                ctx.verification.ledger.record_edit();
                            }
                        } else {
                            let base = workspace_root.clone().unwrap_or_default();
                            let edited: Vec<_> = metadata
                                .changed_paths
                                .iter()
                                .map(|path| base.join(path))
                                .collect();
                            let project_roots: Vec<_> = workspace_root
                                .iter()
                                .chain(&ctx.compiler.edit_root)
                                .cloned()
                                .collect();
                            ctx.verification
                                .ledger
                                .record_edit_to(&edited, &project_roots);
                        }
                    }
                    if made_progress {
                        ctx.recovery.loop_detector.reset();
                        ctx.recovery.reasoning_loop_detector.reset();
                        ctx.recovery.reasoning_recovery_attempts = 0;
                        ctx.recovery.output_limit.record_progress();
                        ctx.progress.grounded_artifact = None;
                    }
                }

                // Keep authoritative write metadata separate from display
                // output. A later read can then prove that it is checking the
                // same revision without replaying source into recovery.
                if is_mutating_tool(&name) && mutation_progress {
                    for path in &metadata.changed_paths {
                        let content = file_preview.as_ref().and_then(|(preview_path, content)| {
                            (preview_path == path).then_some(content.as_str())
                        });
                        ctx.progress.file_evidence.record_mutation(path, content);
                        ctx.progress.grounded_artifact = Some(GroundedArtifactEvidence {
                            path: path.clone(),
                            write_lines: content.map(|content| content.lines().count()),
                            write_bytes: content.map(str::len),
                            read_range: None,
                            repair_attempts: 0,
                        });
                    }
                }

                let no_result = loop_detect::stagnation_key(&content) == "grep:no-matches";
                let search_result = loop_detect::is_search_tool(&name)
                    || call.is_some_and(|call| {
                        let (_, category) = loop_detect::signatures(&call.name, &call.arguments);
                        category.starts_with("search:")
                    });
                let changed_workspace = mutation_progress && metadata.success;
                let state_fingerprint = changed_workspace.then(|| {
                    let mut state = metadata.changed_paths.join("\n");
                    if let Some(diff) = diff_opt.as_deref() {
                        state.push('\n');
                        state.push_str(diff);
                    }
                    loop_detect::stable_hash(&state)
                });
                let output_fingerprint = compiler_diagnostic_fingerprint(&content)
                    .as_deref()
                    .map(loop_detect::stable_hash)
                    .unwrap_or_else(|| {
                        loop_detect::stable_hash(loop_detect::stagnation_key(&content))
                    });
                let call_is_read_only = call
                    .map(|call| shell_call_is_read_only(call))
                    .unwrap_or_else(|| loop_detect::is_read_only(&name));
                let action = call
                    .map(|call| {
                        let (exact, category) =
                            loop_detect::signatures(&call.name, &call.arguments);
                        if verification_command || call_is_read_only {
                            category
                        } else {
                            exact
                        }
                    })
                    .unwrap_or_else(|| name.clone());
                if metadata.success
                    && let Some(call) = call
                    && let Some(path) = loop_detect::inspection_target(&call.name, &call.arguments)
                {
                    let (_, category) = loop_detect::signatures(&call.name, &call.arguments);
                    if category.starts_with("read:") {
                        if let loop_detect::ReasoningLoopStatus::LoopDetected(reason) = ctx
                            .recovery
                            .reasoning_loop_detector
                            .record_inspection_evidence(&loop_detect::InspectionEvidence {
                                tool_name: &call.name,
                                target: &path,
                                turn_id: ctx.budget.tool_rounds as u64,
                                unchanged: !is_mutating_tool(&name),
                                incomplete: metadata.truncated
                                    || matches!(
                                        metadata.completeness,
                                        rustcode_core::ToolResultCompleteness::LineTruncated
                                            | rustcode_core::ToolResultCompleteness::ByteTruncated
                                    ),
                                corruption_claim:
                                    loop_detect::claims_corrupt_or_incomplete_inspection(
                                        &ctx.response.final_content,
                                    ),
                            })
                        {
                            cross_tool_inspection_cycle = Some((reason, path));
                        }
                    }
                }
                if metadata.success
                    && let Some(call) = call
                    && let Some((path, start_line, end_line)) =
                        loop_detect::read_target(&call.name, &call.arguments)
                {
                    let inspection = metadata.inspection.as_ref();
                    let returned_start = inspection
                        .and_then(|inspection| inspection.returned_range.as_ref())
                        .and_then(|range| range.start)
                        .and_then(|line| usize::try_from(line).ok())
                        .unwrap_or(start_line);
                    let returned_end = inspection
                        .and_then(|inspection| inspection.returned_range.as_ref())
                        .and_then(|range| range.end)
                        .and_then(|line| usize::try_from(line).ok())
                        .or(end_line);
                    let complete_content_read =
                        content_bearing_inspection_status(Some(call), &metadata, &content)
                            == Some(true);
                    if complete_content_read
                        && loop_detect::claims_corrupt_or_incomplete_inspection(
                            &ctx.response.final_content,
                        )
                        && let Some(evidence) = ctx.progress.grounded_artifact.as_mut()
                        && same_recovery_path(&path, &evidence.path)
                        && returned_start == 1
                        && returned_end.is_some_and(|end| {
                            evidence
                                .write_lines
                                .is_none_or(|write_lines| end >= write_lines)
                        })
                    {
                        evidence.read_range = returned_end.map(|end| (returned_start, end));
                    }
                }
                if metadata.success
                    && let Some(call) = call
                    && let Some((path, start_line, end_line)) =
                        loop_detect::read_target(&call.name, &call.arguments)
                    && let Some(recovery) = ctx.progress.file_evidence.record_read_with_kind(
                        &path,
                        start_line,
                        metadata
                            .inspection
                            .as_ref()
                            .and_then(|inspection| inspection.returned_range.as_ref())
                            .and_then(|range| range.end)
                            .and_then(|line| usize::try_from(line).ok())
                            .or(end_line),
                        !incomplete_tool_result(&metadata),
                        loop_detect::read_returns_content(&call.name, &call.arguments),
                    )
                {
                    grounded_recovery = Some(recovery.message());
                    evidence_recovery = Some((
                        loop_detect::ProgressReason::NoNewInformation,
                        recovery.repeated_reads,
                        format!("read:{}#{}", path, start_line / 200),
                    ));
                }
                let benign_shell_failure = benign_shell_wrapper_failure(call, &metadata, &content);
                let semantic_failure = (!benign_shell_failure)
                    .then(|| loop_detect::semantic_failure_class(&content))
                    .flatten();
                let failure_fingerprint = semantic_failure
                    .map(|class| loop_detect::stable_hash(&format!("semantic_failure:{class}")))
                    .or_else(|| {
                        (!metadata.success && !benign_shell_failure).then(|| {
                            loop_detect::stable_hash(&format!(
                                "{name}:{}:{}",
                                metadata.exit_code.unwrap_or_default(),
                                loop_detect::stagnation_key(&content)
                            ))
                        })
                    });
                let read_range = call
                    .filter(|_| metadata.success)
                    .and_then(|call| loop_detect::read_target(&call.name, &call.arguments))
                    .and_then(|(path, start_line, end_line)| {
                        let returned = metadata
                            .inspection
                            .as_ref()
                            .and_then(|inspection| inspection.returned_range.as_ref());
                        let line =
                            |line: Option<_>| line.and_then(|line| usize::try_from(line).ok());
                        let start =
                            line(returned.and_then(|range| range.start)).unwrap_or(start_line);
                        let end = line(returned.and_then(|range| range.end)).or(end_line)?;
                        Some((path, start, end))
                    });
                let observation = loop_detect::ProgressObservation {
                    action,
                    output_fingerprint,
                    state_fingerprint,
                    failure_fingerprint,
                    changed_workspace,
                    fresh_read: call_is_read_only && !metadata.replayed,
                    search_result,
                    no_result,
                    verification: verification_command,
                    read_only: call_is_read_only,
                    replayed: metadata.replayed,
                    success: metadata.success && semantic_failure.is_none(),
                };
                let assessment = if name == "list_mcp_tools" {
                    let inventory = serde_json::from_str::<serde_json::Value>(
                        content.strip_prefix("list_mcp_tools: ").unwrap_or(&content),
                    )
                    .ok();
                    ctx.progress
                        .ledger
                        .observe_mcp_discovery(&observation, inventory.as_ref())
                } else {
                    ctx.progress.ledger.observe_with_read_range(
                        &observation,
                        read_range
                            .as_ref()
                            .map(|(path, start, end)| (path.as_str(), *start, *end)),
                    )
                };
                let target_file = call.and_then(|c| {
                    c.arguments
                        .get("path")
                        .or_else(|| c.arguments.get("target_file"))
                        .or_else(|| c.arguments.get("TargetFile"))
                        .and_then(|v| v.as_str())
                });
                cross_turn_tool_count += 1;
                // Stale-round accounting covers only the model's own reads.
                // Guardrail rejections are left out entirely, and failures and
                // search misses have their own replan and stagnation paths.
                if !assessment.suppress_stagnation && !boundary_failure {
                    round_assessed_results += 1;
                    if call_is_read_only
                        && metadata.success
                        && !no_result
                        && assessment.reason == loop_detect::ProgressReason::NoNewInformation
                    {
                        round_stale_rereads += 1;
                    }
                }
                if assessment.reason == loop_detect::ProgressReason::WorkspaceChanged {
                    round_workspace_changed = true;
                    ctx.progress.workspace_change_events =
                        ctx.progress.workspace_change_events.saturating_add(1);
                }
                // Runtime probes, browser checks, and fresh reads can provide
                // decisive evidence without changing a file. The progress
                // ledger already classifies that evidence; carry its result
                // into the cross-turn reasoning detector instead of treating
                // "no workspace diff" as "no progress".
                cross_turn_made_progress |= assessment.meaningful;
                cross_turn_had_edits |=
                    tool_changes_workspace(&name, call.map(|call| &call.arguments));
                // A successful verification or other novel command result is
                // authoritative evidence even when it leaves the workspace
                // unchanged. Do not compare its reasoning with a stale plan
                // from before the evidence was produced.
                cross_turn_authoritative_progress |= matches!(
                    assessment.reason,
                    loop_detect::ProgressReason::Verification
                        | loop_detect::ProgressReason::NewInformation
                );
                if let Some(target_file) = target_file {
                    cross_turn_target_files.push(target_file.to_string());
                }
                ctx.progress.last_reason = Some(assessment.reason);
                if boundary_failure {
                    // Harness guardrail hits are not model stagnation: keep
                    // them out of the evidence-recovery streak entirely. The
                    // rejection text itself plus the infrastructure guard
                    // carry the hint.
                    ctx.progress.ledger.reset_streak();
                }
                crate::logger::operational_event(
                    "turn.progress",
                    serde_json::json!({
                        "round": ctx.budget.tool_rounds,
                        "tool": name,
                        "meaningful": assessment.meaningful,
                        "reason": assessment.reason.label(),
                        "streak": assessment.streak,
                        "replayed": metadata.replayed,
                        "success": metadata.success && semantic_failure.is_none(),
                        "repeated_successful_verification": repeated_successful_verification,
                        "failure_class": semantic_failure,
                    }),
                );
                if assessment.meaningful {
                    ctx.progress.meaningful_events =
                        ctx.progress.meaningful_events.saturating_add(1);
                    round_had_meaningful = true;
                } else if !assessment.suppress_stagnation {
                    round_no_progress_hits = round_no_progress_hits.saturating_add(1);
                    ctx.metrics.no_progress_results += 1;
                }
                if !assessment.suppress_stagnation
                    && !boundary_failure
                    && (assessment.reason == loop_detect::ProgressReason::Churn
                        || assessment.streak >= ctx.progress.evidence_recovery_streak)
                {
                    // Name the action that opened the streak (the dominating
                    // tool), not whatever ran most recently: with corroborated
                    // streaks the last tool misattributes the blame.
                    let dominant = ctx
                        .progress
                        .ledger
                        .streak_action()
                        .map(str::to_string)
                        .unwrap_or_else(|| name.clone());
                    evidence_recovery = Some((assessment.reason, assessment.streak, dominant));
                }
                if repeated_successful_verification && metadata.success {
                    let action = format!(
                        "verification:{}",
                        call.and_then(|call| call.arguments.get("command"))
                            .and_then(|command| command.as_str())
                            .or(metadata.command.as_deref())
                            .unwrap_or(name.as_str())
                    );
                    evidence_recovery.get_or_insert((
                        loop_detect::ProgressReason::NoNewInformation,
                        1,
                        action,
                    ));
                }
                if !assessment.suppress_stagnation && !benign_shell_failure && !boundary_failure {
                    match record_round_output(
                        &mut ctx.recovery.loop_detector,
                        &mut round_outputs,
                        &content,
                    ) {
                        status @ (loop_detect::LoopStatus::Warning(n)
                        | loop_detect::LoopStatus::Abort(n)) => {
                            dbg_log!("Loop detector: output stagnation x{} for '{}'", n, name);
                            if status.rank() > stagnation.rank() {
                                stagnation = status;
                            }
                        }
                        loop_detect::LoopStatus::Ok => {}
                    }
                }
                result_messages.push((
                    call_position,
                    tool_result_history_message(
                        ToolResult {
                            tool_name: name,
                            content,
                            diff: diff_opt,
                            file_preview,
                            metadata,
                        },
                        answered_call,
                    ),
                ));
            }

            // Stagnation is one signal per model round: a batched round of
            // repeated reads is one stagnant step, not N budget-consuming
            // failures, and any meaningful result clears the streak.
            ctx.progress.consecutive_no_progress = apply_round_stagnation(
                ctx.progress.consecutive_no_progress,
                round_had_meaningful,
                round_no_progress_hits,
            );

            // Per-result no-information signals are provisional: a mixed
            // batch can repeat an old read and also discover new evidence.
            // Decide after the whole batch, independently of result order.
            // Keep failure/churn signals and the separate cross-turn guard;
            // fresh reads must not license an endless inspect/same-plan loop.
            if batch_invalidates_read_recovery(cross_turn_made_progress, evidence_recovery.as_ref())
            {
                evidence_recovery = None;
                grounded_recovery = None;
            }

            // A successful verification in the same batch is authoritative
            // progress and invalidates any stale inspection-cycle signal.
            if cross_turn_authoritative_progress {
                cross_tool_inspection_cycle = None;
            }

            // Sustained re-verification: rounds that are mostly stale re-reads
            // stay invisible to the checks above as long as each one also has
            // a single fresh read. Feed a sustained run into the same bounded
            // evidence recovery, so it escalates to a forced final answer
            // well before the token budget runs out.
            ctx.progress.consecutive_stale_rounds = apply_stale_round(
                ctx.progress.consecutive_stale_rounds,
                round_stale_rereads,
                round_assessed_results,
                round_workspace_changed || cross_turn_authoritative_progress,
            );
            if ctx.progress.consecutive_stale_rounds >= ctx.progress.evidence_recovery_streak {
                if evidence_recovery.is_none() {
                    evidence_recovery = Some((
                        loop_detect::ProgressReason::NoNewInformation,
                        ctx.progress.consecutive_stale_rounds,
                        REVERIFICATION_ACTION.to_string(),
                    ));
                }
                ctx.progress.consecutive_stale_rounds = 0;
            }
            if let Some((reason, path)) = cross_tool_inspection_cycle {
                ctx.recovery.reasoning_loops_detected += 1;
                crate::logger::operational_event(
                    reason,
                    serde_json::json!({
                        "round": ctx.budget.tool_rounds,
                        "reason": reason,
                        "target": path,
                    }),
                );
                if evidence_recovery.is_none() {
                    evidence_recovery = Some((
                        loop_detect::ProgressReason::NoNewInformation,
                        ctx.recovery.reasoning_recovery_attempts as usize + 1,
                        format!("cross-tool inspection cycle: {reason}"),
                    ));
                }
            }

            let output_abort = matches!(stagnation, loop_detect::LoopStatus::Abort(_));

            for (index, call_ref) in call_refs.iter().enumerate() {
                if selected_call_indices.contains(&index) {
                    continue;
                }
                let (reason, error_kind) = match validation_errors[index].as_deref() {
                    Some(reason) => (reason, crate::tools::ToolErrorKind::Validation),
                    // Queued calls stay closed with a typed result so the
                    // announcement keeps its provider pairing, but the model is
                    // never asked to reissue what the harness already holds
                    // (#1590).
                    None if scheduled.queued.contains(&index) => (
                        "held by the harness and queued for automatic execution in a later round; do not reissue it",
                        crate::tools::ToolErrorKind::Deferred,
                    ),
                    None if scheduled.reissue.contains(&index) => (
                        "not scheduled by the harness; reissue it only if still needed after reviewing the executed results",
                        crate::tools::ToolErrorKind::Deferred,
                    ),
                    None => (
                        "an identical call in this response already ran; its result above answers this call too",
                        crate::tools::ToolErrorKind::Deferred,
                    ),
                };
                if let Some(message) = unanswered_call_results_with_kind(
                    std::slice::from_ref(call_ref),
                    reason,
                    error_kind,
                )
                .into_iter()
                .next()
                {
                    result_messages.push((index, message));
                }
            }
            result_messages.sort_by_key(|(index, _)| *index);
            append_finalized_tool_batch(
                &mut s,
                turn_session_id,
                result_messages
                    .into_iter()
                    .map(|(_, message)| message)
                    .collect(),
            );
            if let Some(workspace) = s.effective_workspace_root() {
                let session_dirs = crate::config::get_active_session_dir(turn_session_id)
                    .into_iter()
                    .collect::<Vec<_>>();
                for index in &selected_call_indices {
                    if let Some(call) = tool_calls.get(*index) {
                        ctx.progress.outside_workspace.record(
                            &call.arguments,
                            &workspace,
                            &session_dirs,
                        );
                    }
                }
                if let Some(note) = ctx
                    .progress
                    .outside_workspace
                    .take_note(&workspace, &s.history)
                {
                    crate::logger::operational_event(
                        "turn.outside_workspace_note",
                        serde_json::json!({"session_id": turn_session_id}),
                    );
                    s.history.push(ChatMessage::new("system", note));
                }
            }
            if deferred_call_count > 0 {
                let describe = |indices: &[usize]| {
                    indices
                        .iter()
                        .map(|index| {
                            call_refs
                                .get(*index)
                                .map(|call_ref| {
                                    format!("{} ({})", tool_calls[*index].name, call_ref.id)
                                })
                                .unwrap_or_else(|| tool_calls[*index].name.clone())
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                // A call rejected by validation already carries its reason
                // as its own tool result, so it is not reported again (#1886).
                let closed = (0..tool_calls.len())
                    .filter(|index| {
                        !selected_call_indices.contains(index)
                            && !scheduled.queued.contains(index)
                            && validation_errors[*index].is_none()
                    })
                    .collect::<Vec<_>>();
                if let Some(notice) = unexecuted_calls_notice(
                    requested_calls,
                    selected_call_indices.len(),
                    &scheduled.queued,
                    &scheduled.reissue,
                    &closed,
                    |index| {
                        tool_calls
                            .get(index)
                            .is_some_and(|call| is_known_mutation_call(&call.name))
                    },
                    describe,
                ) {
                    dbg_log!("Deferred tool-call diagnostic: {notice}");
                    s.history.push(ChatMessage::new("system", notice));
                }
                // Deferred calls never adopt their speculative projections;
                // drop them so they cannot linger under an active group (#1495).
                s.clear_speculative_live_tool_calls();
            }

            if let Some((fingerprint, dependency, streak)) = infrastructure_stop {
                let stop_reason = fingerprint;
                let boundary_stop = stop_reason == loop_detect::WORKSPACE_BOUNDARY_FINGERPRINT;
                crate::logger::operational_event(
                    "turn.loop_signal",
                    loop_signal_event(
                        Some(&stagnation),
                        Some(round_had_meaningful),
                        evidence_recovery.as_ref().map(|(reason, _, _)| *reason),
                        false,
                        Some("infrastructure_failure"),
                        false,
                    ),
                );
                s.history.push(ChatMessage::new(
                    "system",
                    if boundary_stop {
                        format!(
                            "[Workspace boundary guard: a tool path outside the workspace root was rejected across {streak} consecutive attempts. Do not retry out-of-workspace paths; choose a path under the workspace root or explain the blocker.]"
                        )
                    } else {
                        format!(
                            "[Infrastructure failure guard: {dependency} has failed across multiple tool attempts ({streak} consecutive matching failures). Further retries are paused.]"
                        )
                    },
                ));
                crate::config::save_session_history(&s.active_session_id, &s.history);
                s.clear_current_response();
                drop(s);
                ctx.response.final_content = if boundary_stop {
                    format!(
                        "I could not complete the task because the requested path lies outside the workspace root ({streak} consecutive rejections; {stop_reason}). Choose a path under the workspace root, then resume the session."
                    )
                } else {
                    format!(
                        "I could not complete the task because the {dependency} dependency remained unavailable across {streak} diagnostic attempts. The harness stopped further retries ({stop_reason}). Check that service or MCP connection, then resume the session."
                    )
                };
                ctx.response.final_content_persisted = false;
                ctx.lifecycle.task_completed = false;
                ctx.lifecycle.stop_reason =
                    Some(lifecycle::StopReason::InfrastructureFailure(stop_reason));
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Stop;
            }

            // `record_turn_evidence` models a complete model turn. Record it
            // once after the selected result is processed.
            // Completion has its own evidence, verification, and compiler
            // gates below. Do not let generic loop recovery intercept a
            // `complete_task` request before those authoritative gates run.
            if !completed && cross_turn_tool_count > 0 {
                let target_file_refs = cross_turn_target_files
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let cross_turn_status = ctx
                    .recovery
                    .reasoning_loop_detector
                    .record_turn_evidence_after_progress(
                        &loop_detect::TurnEvidence {
                            reasoning: &ctx.response.final_content,
                            target_files: &target_file_refs,
                            made_progress: cross_turn_made_progress,
                            had_edits: cross_turn_had_edits,
                            tool_count: cross_turn_tool_count,
                            no_progress_streak: ctx.progress.ledger.no_progress_streak(),
                        },
                        cross_turn_authoritative_progress,
                    );
                if let loop_detect::ReasoningLoopStatus::LoopDetected(reason) = cross_turn_status {
                    ctx.recovery.reasoning_loops_detected += 1;
                    dbg_log!("Cross-turn reasoning loop detected: {reason}");
                    crate::logger::operational_event(
                        reason,
                        serde_json::json!({
                            "round": ctx.budget.tool_rounds,
                            "reason": reason,
                        }),
                    );
                    if evidence_recovery.is_none() {
                        evidence_recovery = Some((
                            loop_detect::ProgressReason::NoNewInformation,
                            ctx.recovery.reasoning_recovery_attempts as usize + 1,
                            format!("reasoning loop: {reason}"),
                        ));
                    }
                }
            }
            if !completed
                && let loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) =
                    stagnation
            {
                push_or_replace_loop_warning(
                    s.history.as_mut_vec(),
                    format!(
                        "[Loop warning: the last {n} tool results were identical in kind (e.g. repeated \"no matches\"). Re-phrasing the same search is not progress — the answer is not where you are looking. View the relevant file directly or change approach.]"
                    ),
                );
            }

            if targeted_recovery && evidence_recovery.is_none() {
                evidence_recovery = Some((
                    loop_detect::ProgressReason::NoNewInformation,
                    1,
                    "grounded malformed-artifact repair".to_string(),
                ));
            }

            // Single-guidance arbiter: at most one model-visible recovery
            // notice per round. The evidence-recovery and failure-replan paths
            // below return early with their own notice, so parked warnings are
            // only pushed when neither of them will fire. The output-stagnation
            // warning wins over the parked call-repetition one; they share one
            // replaceable slot either way.
            let recovery_will_fire = ctx.progress.evidence_recovery_enabled
                && should_apply_loop_recovery(
                    completed,
                    output_abort,
                    round_had_meaningful,
                    evidence_recovery.is_some(),
                );
            let recovery_suppressed_reason =
                (output_abort && round_had_meaningful && !completed && evidence_recovery.is_none())
                    .then_some("same_round_meaningful_progress");
            crate::logger::operational_event(
                "turn.loop_signal",
                loop_signal_event(
                    Some(&stagnation),
                    Some(round_had_meaningful),
                    evidence_recovery.as_ref().map(|(reason, _, _)| *reason),
                    recovery_will_fire,
                    recovery_suppressed_reason,
                    false,
                ),
            );
            if !completed && !recovery_will_fire && failure_replan.is_none() {
                match stagnation {
                    loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) => {
                        push_or_replace_loop_warning(
                            s.history.as_mut_vec(),
                            format!(
                                "[Loop warning: the last {n} tool results were identical in kind (e.g. repeated \"no matches\"). Re-phrasing the same search is not progress — the answer is not where you are looking. View the relevant file directly or change approach.]"
                            ),
                        );
                    }
                    loop_detect::LoopStatus::Ok => {
                        if let Some(warning) = pending_loop_warning {
                            push_or_replace_loop_warning(s.history.as_mut_vec(), warning);
                        }
                    }
                }
            }

            if recovery_will_fire {
                let (reason, streak, action) = evidence_recovery.unwrap_or((
                    loop_detect::ProgressReason::NoNewInformation,
                    match stagnation {
                        loop_detect::LoopStatus::Warning(n) | loop_detect::LoopStatus::Abort(n) => {
                            n
                        }
                        loop_detect::LoopStatus::Ok => 0,
                    },
                    "repeated tool output".to_string(),
                ));
                let recovery_guidance = targeted_no_progress_guidance(reason, streak, &action);
                let evidence = grounded_recovery.clone().map_or_else(
                    || {
                        format!(
                            "[Evidence-based recovery: signal={} streak={} action={}]. {recovery_guidance}",
                            reason.label(),
                            streak,
                            action
                        )
                    },
                    |notice| format!("[Evidence-based recovery: {notice}]"),
                );
                let local_recovery_action = if targeted_recovery_exhausted {
                    LoopRecoveryAction::ForceFinal
                } else {
                    loop_recovery_action_for(ctx.recovery.loop_recovery_attempts, read_only_batch)
                };
                let recovery_action = local_recovery_action;
                match recovery_action {
                    LoopRecoveryAction::Recover => {
                        ctx.recovery.loop_recovery_attempts =
                            ctx.recovery.loop_recovery_attempts.saturating_add(1);
                        ctx.metrics.evidence_recoveries += 1;
                        if targeted_recovery {
                            ctx.metrics.grounded_recoveries += 1;
                        }
                        log_recovery_decision(ctx, "evidence", "recover", reason.label());
                        ctx.recovery.loop_detector.reset();
                        ctx.recovery.reasoning_loop_detector.reset();
                        let recovery_prompt = loop_recovery_prompt(
                            &s.history,
                            ctx.progress.made_edits,
                            ctx.compiler.consecutive_diagnostics > 0
                                || ctx.compiler.consecutive_error_gates > 0,
                        );
                        push_or_replace_recovery_notice(
                            s.history.as_mut_vec(),
                            format!("{evidence}\n{recovery_prompt}"),
                        );
                        crate::config::save_session_history(&s.active_session_id, &s.history);
                        s.clear_current_response();
                        s.status = AppStatus::Streaming;
                        s.stream_tracker = Some(StreamTracker::new());
                        drop(s);
                        ctx.lifecycle.turn_machine.finish_tools_if_executing();
                        ctx.budget.tool_rounds += 1;
                        return ToolHandlingOutcome::Continue;
                    }
                    LoopRecoveryAction::ForceFinal => {
                        log_recovery_decision(ctx, "evidence", "force_final", reason.label());
                        if targeted_recovery {
                            ctx.metrics.grounded_recoveries += 1;
                        }
                        crate::logger::operational_event(
                            loop_detect::DIAG_RECOVERY_EXHAUSTED,
                            serde_json::json!({
                                "recovery_attempts": ctx.recovery.loop_recovery_attempts,
                                "reason": reason.label(),
                            }),
                        );
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("{evidence}\n{FORCE_ANSWER_PROMPT}"),
                        ));
                        crate::config::save_session_history(&s.active_session_id, &s.history);
                        s.clear_current_response();
                        drop(s);
                        ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::LoopEscalation);
                        ctx.recovery.force_final = true;
                        ctx.lifecycle.turn_machine.finish_tools_if_executing();
                        return ToolHandlingOutcome::Continue;
                    }
                }
            }

            if completed && batch_incomplete {
                dbg_log!(
                    "Completion blocked: tool batch contained dropped, unresolved, or incomplete results"
                );
                s.history.push(ChatMessage::new(
                    "system",
                    "[Finish blocked — this response included a dropped, unresolved, or incomplete tool result. Re-issue the affected tool call and inspect a complete result before reporting completion.]",
                ));
                crate::config::save_session_history(&s.active_session_id, &s.history);
                s.clear_current_response();
                drop(s);
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Continue;
            }

            if let Some(replan) = failure_replan {
                ctx.metrics.failure_replans += 1;
                // A replan is a recovery opportunity, not a terminal state.
                // The old path set `force_final` immediately, so the next
                // response's tool call was discarded and a recoverable pair
                // of edit mismatches ended the entire task. Reset the
                // equivalence detector and let the model inspect or choose a
                // different mutation method. The consecutive-failure budget
                // remains intact as the hard backstop.
                ctx.recovery.loop_detector.reset();
                push_or_replace_recovery_notice(s.history.as_mut_vec(), replan);
                crate::config::save_session_history(&s.active_session_id, &s.history);
                s.clear_current_response();
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                drop(s);
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Continue;
            }

            if completed
                && completion_claims_unapplied_work(
                    ctx.progress.made_edits,
                    ctx.progress.failed_mutations,
                    ctx.recovery.completion_blocks,
                )
            {
                ctx.recovery.completion_blocks += 1;
                dbg_log!(
                    "Completion blocked: {} failed edits, none applied",
                    ctx.progress.failed_mutations
                );
                crate::logger::operational_event(
                    "turn.completion_blocked",
                    serde_json::json!({ "failed_mutations": ctx.progress.failed_mutations }),
                );
                s.history.push(ChatMessage::new(
                    "system",
                    completion_block_message(ctx.progress.failed_mutations),
                ));
                crate::config::save_session_history(&s.active_session_id, &s.history);
                s.clear_current_response();
                drop(s);
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Continue;
            }

            const MAX_VERIFICATION_BLOCKS: u8 = 2;
            if completed {
                if let Some(evidence) = ctx.verification.ledger.explicit_last_failure() {
                    ctx.verification.blocks = ctx.verification.blocks.saturating_add(1);
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "[Finish blocked — the explicitly requested command failed: {} (exit code {:?}). Run it again and inspect its result before reporting completion.]",
                            evidence.command, evidence.exit_code
                        ),
                    ));
                    crate::config::save_session_history(&s.active_session_id, &s.history);
                    s.clear_current_response();
                    drop(s);
                    ctx.lifecycle.turn_machine.finish_tools_if_executing();
                    return ToolHandlingOutcome::Continue;
                }
                let requires_verification = ctx.progress.made_edits
                    && (verification::requires_verification(&ctx.progress.changed_paths)
                        || ctx.verification.ledger.last_failure().is_some());
                if requires_verification
                    && !ctx.verification.ledger.has_fresh_successful_verification()
                    // Unchanged since it passed: running it again would only
                    // be answered from the verification cache.
                    && !ctx.verification.ledger.verified_workspace_is_unchanged(|root| {
                        crate::workspace_intelligence::snapshot(root)
                            .ok()
                            .map(|snapshot| snapshot.generation)
                    })
                    && ctx.verification.blocks < MAX_VERIFICATION_BLOCKS
                {
                    ctx.verification.blocks += 1;
                    let reason = ctx
                        .verification
                        .ledger
                        .last_failure()
                        .map(|evidence| {
                            format!(
                                "The latest verification failed: {} (exit code {:?}).",
                                evidence.command, evidence.exit_code
                            )
                        })
                        .unwrap_or_else(|| ctx.verification.ledger.missing_verification_reason());
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "[Finish blocked — {reason} Run the relevant project verification command after the latest edit, inspect its result, then report completion.]"
                        ),
                    ));
                    crate::config::save_session_history(&s.active_session_id, &s.history);
                    s.clear_current_response();
                    drop(s);
                    ctx.lifecycle.turn_machine.finish_tools_if_executing();
                    return ToolHandlingOutcome::Continue;
                }
                let mut build_status = if ctx.progress.made_edits
                    && verification::requires_verification(&ctx.progress.changed_paths)
                {
                    "pending"
                } else {
                    "not run (no workspace code edits detected)"
                };
                if ctx.progress.made_edits
                    && verification::requires_verification(&ctx.progress.changed_paths)
                {
                    // `s` already owns the application-state mutex here. Do
                    // not lock it recursively: Tokio's mutex is not reentrant
                    // and that used to deadlock every accepted complete_task
                    // which reached the compiler finish gate. Release state
                    // while the potentially slow compiler check runs, then
                    // reacquire it for the history/status updates below.
                    s.status = AppStatus::Streaming;
                    let root = ctx
                        .compiler
                        .edit_root
                        .clone()
                        .or_else(|| s.effective_workspace_root())
                        .unwrap_or_default();
                    let sandbox_mode = s.effective_sandbox_mode();
                    drop(s);
                    let compiler_errors = cached_compiler_check(
                        &root,
                        &mut ctx.compiler.dirty,
                        &mut ctx.compiler.cache,
                        cancel_token,
                        sandbox_mode,
                    )
                    .await;
                    s = state.lock().await;
                    if let Some(errors) = compiler_errors {
                        if errors.starts_with("__BUILD_UNVERIFIED__") {
                            dbg_log!("complete_task finish gate: build unverified — {errors}");
                            build_status = "unverified";
                            s.history.push(ChatMessage::new(
                                "system",
                                format!("[⚠ Build could not be verified — {errors}]"),
                            ));
                        } else {
                            dbg_log!("complete_task finish gate failed with compiler errors");
                            ctx.compiler.consecutive_error_gates += 1;
                            s.history.push(ChatMessage::new(
                                        "system",
                                        format!(
                                            "[Finish blocked — the build does not compile. You cannot report this \
                                             task as done while there are compiler errors. Fix them, then finish. \
                                             Compiler errors:\n{errors}]"
                                        ),
                                    ));
                            crate::config::save_session_history(&s.active_session_id, &s.history);
                            s.clear_current_response();
                            drop(s);
                            ctx.lifecycle.turn_machine.finish_tools_if_executing();
                            return ToolHandlingOutcome::Continue;
                        }
                    } else {
                        build_status = "passed";
                        ctx.compiler.consecutive_error_gates = 0;
                    }
                }

                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
                dbg_log!("complete_task accepted; finalizing the turn");
                let task_result_summary = tool_calls
                    .iter()
                    .find(|call| call.name == "complete_task")
                    .and_then(|call| call.arguments.get("result").and_then(|r| r.as_str()))
                    .map(|s| s.to_string());

                if let Some(mut summary_text) = task_result_summary
                    && !summary_text.is_empty()
                {
                    let mut changed_paths = std::collections::BTreeSet::new();
                    for message in &s.history {
                        if let Some(metadata) = &message.tool_result {
                            changed_paths.extend(metadata.changed_paths.iter().cloned());
                        }
                    }
                    let paths = if changed_paths.is_empty() {
                        "none recorded".to_string()
                    } else {
                        changed_paths.into_iter().collect::<Vec<_>>().join(", ")
                    };
                    summary_text.push_str(&format!(
                        "\n\n[harness verification: build={build_status}; tool_verification={}; changed_paths={paths}]",
                        ctx.verification.ledger.summary()
                    ));
                    if !ctx.progress.made_edits && ctx.progress.failed_mutations > 0 {
                        summary_text.push_str(&format!(
                            "\n[harness warning: {} edit(s) failed and none were applied — \
    nothing in this summary was written to disk by this task]",
                            ctx.progress.failed_mutations
                        ));
                    }
                    s.history.push(ChatMessage::new("assistant", summary_text));
                }
                drop(s);
                ctx.lifecycle.task_completed = true;
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Stop;
            }
            crate::config::save_session_history(&s.active_session_id, &s.history);
            s.clear_current_response();
            drop(s);
            ctx.lifecycle.turn_machine.finish_tools_if_executing();
            dbg_log!("Tool round finished, looping back");
            return ToolHandlingOutcome::Continue;
        } else {
            dbg_log!("Tool execution cancelled");
            ctx.lifecycle.turn_machine.finish_tools_if_executing();
            return ToolHandlingOutcome::Stop;
        }
    } else if has_intended_tool_call(&ctx.response.final_content) {
        dbg_log!("Orchestrator: Detected malformed tool call, auto-correcting and retrying...");
        ctx.budget.tool_rounds += 1;
        let raw_content = ctx.response.final_content.clone();
        let repeated_malformed = record_malformed_call(ctx, &raw_content, &[]);
        let mut s = state.lock().await;
        let bounded_history_content = bounded_malformed_tool_history(&ctx.response.final_content);
        let mut msg = ChatMessage::new("assistant", bounded_history_content)
            .as_unexecuted_tool_call_checkpoint();
        msg.response_time_ms = Some(turn_response_time_ms);
        msg.token_usage = turn_token_usage.clone();
        msg.thought_time_ms = thought_time_ms;
        msg.thought_tokens = thought_tokens;
        s.history.push(msg);
        ctx.response.final_content_persisted = true;

        let reason = crate::tools::diagnose_failed_tool_call(&ctx.response.final_content)
            .map(|r| format!("{r}\n\n"))
            .unwrap_or_default();
        let correction = match protocol {
            crate::config::ToolProtocol::ApiNative => {
                "Invoke one function through the native tool interface. Do not print XML, JSON, or a fenced tool block as assistant prose."
            }
            crate::config::ToolProtocol::Native => {
                "Output one complete call using the active native text-tool format; do not mix it with JSON fencing."
            }
            crate::config::ToolProtocol::Json => {
                "Output one complete JSON call inside a ```tool fenced block with exactly the keys `name` and `arguments`."
            }
        };
        let feedback = format!(
            "tool_error: The attempted tool call was malformed or could not be parsed. {reason}{correction} Ensure argument numbers and booleans use their schema types.{}",
            if repeated_malformed {
                format!(
                    " This malformed request has repeated {} times; stop emitting the same block and re-plan or answer with text.",
                    ctx.recovery.consecutive_malformed_calls
                )
            } else {
                String::new()
            }
        );

        s.history.push(ChatMessage::new("tool", feedback));
        crate::config::save_session_history(&s.active_session_id, &s.history);
        s.clear_current_response();
        s.status = AppStatus::Streaming;
        s.stream_tracker = Some(StreamTracker::new());
        drop(s);
        let _ = ctx.lifecycle.turn_machine.retry_for_finish_gate();
        dbg_log!("Retrying agent loop round due to malformed tool call");
        return ToolHandlingOutcome::Continue;
    }
    ToolHandlingOutcome::NotHandled
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
