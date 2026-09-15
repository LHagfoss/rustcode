use std::sync::Arc;

use tokio::sync::Mutex;

use crate::app::{AppState, AppStatus, ChatMessage, StreamTracker, TokenUsage};

use super::super::events::{self, ToolResult};
use super::super::lifecycle;
use super::super::loop_detect;
use super::super::policy;
use super::super::text::has_intended_tool_call;
use super::super::tool_exec::{
    execute_tool_batch, get_tool_project_root, tool_result_history_message,
};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolHandlingOutcome {
    Continue,
    Stop,
    NotHandled,
}

fn should_apply_loop_recovery(
    completion_requested: bool,
    output_abort: bool,
    has_evidence_recovery: bool,
) -> bool {
    !completion_requested && (output_abort || has_evidence_recovery)
}

fn batch_invalidates_read_recovery(
    made_progress: bool,
    recovery: Option<&(loop_detect::ProgressReason, usize, String)>,
) -> bool {
    made_progress
        && recovery
            .is_some_and(|(reason, _, _)| *reason == loop_detect::ProgressReason::NoNewInformation)
}

fn mutation_batch_guidance(policy: &crate::config::ToolSchedulingPolicy) -> String {
    if policy.allow_batching {
        format!(
            "This trusted profile permits a bounded batch of up to {} read-only and {} workspace-changing tool calls per response. Keep control-plane calls alone, keep mutations grounded and sequential, and never assume an unexecuted call ran.",
            policy.max_read_only_calls, policy.max_mutating_calls
        )
    } else {
        format!(
            "Emit exactly one tool call per response and wait for its result before choosing the next action. The mutation budget remains {} for provider policy compatibility, but it is not a reason to batch calls. Read-only inspection never consumes it.",
            policy.max_mutating_calls
        )
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

fn selected_tool_call_indices(
    calls: &[crate::tools::ToolCall],
    validation_errors: &[Option<String>],
    policy: crate::config::ToolSchedulingPolicy,
) -> Vec<usize> {
    let first_control = calls.iter().enumerate().find(|(index, call)| {
        validation_errors[*index].is_none()
            && matches!(
                crate::tools::tool_safety(&call.name),
                crate::tools::ToolSafety::ControlPlane
            )
    });
    if let Some((index, _)) = first_control {
        return vec![index];
    }

    let first_valid = calls
        .iter()
        .enumerate()
        .find(|(index, _)| validation_errors[*index].is_none())
        .map(|(index, _)| index);
    let Some(first_valid) = first_valid else {
        return if calls.is_empty() {
            Vec::new()
        } else {
            vec![0]
        };
    };
    if !policy.allow_batching {
        return vec![first_valid];
    }

    let mut selected = Vec::new();
    let mut read_only = 0;
    let mut mutating = 0;
    let mut seen = std::collections::HashSet::new();
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
        if crate::tools::is_read_only_call(call) {
            if read_only >= policy.max_read_only_calls {
                continue;
            }
            read_only += 1;
        } else {
            if mutating >= policy.max_mutating_calls {
                continue;
            }
            mutating += 1;
        }
        selected.push(index);
    }

    selected
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
    content: &str,
) -> bool {
    let Some(call) = call else {
        return false;
    };
    if call.name != "run_command"
        || metadata.success
        || content.to_ascii_lowercase().contains("error")
    {
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
    let inspection = metadata.inspection.as_ref()?;
    if !metadata.success
        || content.trim().is_empty()
        || !loop_detect::read_returns_content(&call.name, &call.arguments)
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
        s.history.push(ChatMessage::new(
            "system",
            "[Tool calls rejected: the provider stopped at the output limit before the complete tool request was available. No tool ran. Reissue one smaller, complete tool call.]",
        ));
        crate::config::save_history(&s.history);
        s.clear_current_response();
        s.status = AppStatus::Streaming;
        s.stream_tracker = Some(StreamTracker::new());
        drop(s);
        ctx.budget.tool_rounds += 1;
        return ToolHandlingOutcome::Continue;
    }

    let requested_calls = parsed_tool_calls.len();
    let validation_errors = crate::tools::validation_errors_by_call(&parsed_tool_calls);
    let scheduling_policy = {
        let s = state.lock().await;
        s.active_model_profile()
            .map(|profile| profile.tool_scheduling_policy())
            .unwrap_or_default()
    };
    // Preserve control-plane priority (for example, load a requested skill
    // before acting). The default policy selects one valid call; explicitly
    // trusted profiles may select a bounded read/mutation batch. Invalid or
    // over-budget calls remain in the transcript as non-executed results.
    let selected_call_indices =
        selected_tool_call_indices(&parsed_tool_calls, &validation_errors, scheduling_policy);
    let selected_call_index = selected_call_indices.first().copied();
    let executable_tool_calls = selected_call_indices
        .iter()
        .map(|index| parsed_tool_calls[*index].clone())
        .collect::<Vec<_>>();
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
        s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "[Tool call rejected before execution: {reason}] Emit one corrected tool call. {}{}",
                        mutation_batch_guidance(&scheduling_policy),
                        repeat_guidance
                    ),
                ));
        crate::config::save_history(&s.history);
        s.clear_current_response();
        s.status = AppStatus::Streaming;
        drop(s);
        ctx.budget.tool_rounds += 1;
        return ToolHandlingOutcome::Continue;
    }
    ctx.recovery.oversized_batch_rejections = 0;
    let tool_calls = parsed_tool_calls;
    let deferred_call_count = tool_calls.len().saturating_sub(executable_tool_calls.len());
    let unexecuted_call_count = deferred_call_count;
    let read_only_batch = !selected_call_indices.is_empty()
        && selected_call_indices.iter().all(|index| {
            tool_calls
                .get(*index)
                .is_some_and(|call| loop_detect::is_read_only_call(&call.name, &call.arguments))
        });
    let call_refs = call_refs_for(&tool_calls, &ctx.response.streamed_call_ids);
    let turn_action = match ctx.lifecycle.turn_machine.model_finished(
        cancel_token.is_cancelled(),
        ctx.recovery.force_final,
        !tool_calls.is_empty(),
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
            if is_mutating_tool(&call.name) {
                if let Some(root) = get_tool_project_root(&call.name, &call.arguments) {
                    ctx.compiler.edit_root = Some(root);
                    ctx.compiler.dirty = true;
                }
            }
        }
        match loop_status {
            loop_detect::LoopStatus::Abort(n) => {
                match loop_recovery_action_for(ctx.recovery.loop_recovery_attempts, read_only_batch)
                {
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
                        crate::config::save_history(&s.history);
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
                        crate::config::save_history(&s.history);
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
                let mut s = state.lock().await;
                let action = loop_offender.as_deref().unwrap_or("the last tool action");
                let warning_text = format!(
                    "[Loop warning: '{action}' has repeated {n} times. If a tool edit or view is failing, stop retrying the same inputs — if an edit failed to match, view a wider line range or use grep to verify exact target content.]"
                );
                push_or_replace_loop_warning(s.history.as_mut_vec(), warning_text);
                drop(s);
            }
            loop_detect::LoopStatus::Ok => {}
        }

        if !cancel_token.is_cancelled() {
            ctx.budget.tool_rounds += 1;

            let approved = policy.should_approve(state, &executable_tool_calls).await;

            {
                let mut s = state.lock().await;
                s.pending_tool_confirmation = None;
                s.status = AppStatus::Streaming;
                s.stream_tracker = Some(StreamTracker::new());
                let mut msg = ChatMessage::new("assistant", &ctx.response.final_content)
                    .with_tool_calls(call_refs.clone());
                msg.response_time_ms = Some(turn_response_time_ms);
                msg.token_usage = turn_token_usage.clone();
                msg.thought_time_ms = thought_time_ms;
                msg.thought_tokens = thought_tokens;
                s.history.push(msg);
                ctx.response.final_content_persisted = true;
                crate::config::save_history(&s.history);
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
            let results = execute_tool_batch(
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
            )
            .await;
            let mut executed_results = results.into_iter();
            let results = selected_call_indices
                .iter()
                .map(|index| {
                    executed_results.next().unwrap_or_else(|| ToolResult {
                        tool_name: tool_calls[*index].name.clone(),
                        content: format!("error: tool execution missing for this call ({index})"),
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
            let mutation_batch = results
                .iter()
                .any(|result| is_mutating_tool(&result.tool_name));
            if mutation_batch {
                let diagnostics = results
                    .iter()
                    .find_map(|result| compiler_diagnostic_fingerprint(&result.content));
                if diagnostics.is_some() || !ctx.compiler.dirty {
                    update_compiler_diagnostic_streak(ctx, diagnostics);
                }
            }

            crate::logger::operational_event(
                "tools.batch.finish",
                serde_json::json!({
                    "count": results.len(),
                    "requested": requested_calls,
                    "executed": results.len(),
                    "deferred": unexecuted_call_count,
                    "successes": results.iter().filter(|result| result.metadata.success).count(),
                    "failed": results.iter().filter(|result| !result.metadata.success).count(),
                    "changed_paths": results.iter().map(|result| result.metadata.changed_paths.len()).sum::<usize>(),
                }),
            );

            if cancel_token.is_cancelled() {
                dbg_log!("Orchestrator: Cancelled during tool execution");
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
                let mut s = state.lock().await;
                let selected_refs = selected_call_indices
                    .iter()
                    .filter_map(|index| call_refs.get(*index).cloned())
                    .collect::<Vec<_>>();
                append_cancelled_batch_results(s.history.as_mut_vec(), results, &selected_refs);
                for (index, call_ref) in call_refs.iter().enumerate() {
                    if !selected_call_indices.contains(&index) {
                        s.history.extend(unanswered_call_results_with_kind(
                            std::slice::from_ref(call_ref),
                            "not executed because the model round was cancelled",
                            crate::tools::ToolErrorKind::Cancelled,
                        ));
                    }
                }
                if call_refs.is_empty() {
                    s.history
                        .push(ChatMessage::new("system", "Request cancelled by user"));
                }
                crate::config::save_history(&s.history);
                ctx.lifecycle.turn_machine.finish_tools_if_executing();
                return ToolHandlingOutcome::Stop;
            }

            let mut s = state.lock().await;
            s.status = AppStatus::Streaming;
            let mut completed = false;
            // Completion is valid only when every result in the same batch
            // was actually available and complete. A later turn may recover
            // by re-issuing a bounded read, so this is intentionally scoped
            // to the batch that requested completion.
            let mut batch_incomplete = unexecuted_call_count > 0;
            let mut background_pending = false;
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
            let mut cross_tool_inspection_cycle = None;
            let mut cross_turn_made_progress = false;
            let mut cross_turn_had_edits = false;
            let mut cross_turn_authoritative_progress = false;
            let mut cross_turn_target_files = Vec::new();
            let mut cross_turn_tool_count = 0;
            let mut result_messages = Vec::with_capacity(results.len() + deferred_call_count);
            for (position, result) in results.into_iter().enumerate() {
                let call_position = selected_call_indices
                    .get(position)
                    .copied()
                    .unwrap_or(position);
                let call = tool_calls.get(call_position);
                let answered_call = call_refs.get(call_position).map(|call| call.id.clone());
                let name = result.tool_name;
                let mut metadata = result.metadata.clone();
                // The provider call id is attached at the orchestration
                // boundary, after parsing, and is then carried into the
                // history message that answers that exact call.
                metadata.call_id = answered_call.clone();
                let content = result.content;
                let diff_opt = result.diff;
                let file_preview = result.file_preview;
                batch_incomplete |= incomplete_tool_result(&metadata);
                if let Some(complete) = content_bearing_inspection_status(call, &metadata, &content)
                {
                    if !complete {
                        ctx.progress.incomplete_inspection_results =
                            ctx.progress.incomplete_inspection_results.saturating_add(1);
                    } else {
                        ctx.progress.complete_inspection_results =
                            ctx.progress.complete_inspection_results.saturating_add(1);
                    }
                }
                if metadata.pending {
                    background_pending = true;
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
                if is_mutating_tool(&name) {
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
                        ctx.progress.consecutive_failed_mutations = 0;
                        if !metadata.changed_paths.is_empty() || name != "run_command" {
                            ctx.verification.ledger.record_edit();
                        }
                    }
                    if made_progress {
                        ctx.recovery.loop_detector.reset();
                        ctx.recovery.reasoning_loop_detector.reset();
                        ctx.recovery.reasoning_recovery_attempts = 0;
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
                let action = call
                    .map(|call| {
                        let (exact, category) =
                            loop_detect::signatures(&call.name, &call.arguments);
                        if verification_command || loop_detect::is_read_only(&call.name) {
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
                let assessment = ctx
                    .progress
                    .ledger
                    .observe(&loop_detect::ProgressObservation {
                        action,
                        output_fingerprint,
                        state_fingerprint,
                        failure_fingerprint,
                        changed_workspace,
                        fresh_read: loop_detect::is_read_only(&name) && !metadata.replayed,
                        search_result,
                        no_result,
                        verification: verification_command,
                        read_only: loop_detect::is_read_only(&name),
                        replayed: metadata.replayed,
                        success: metadata.success && semantic_failure.is_none(),
                    });
                let target_file = call.and_then(|c| {
                    c.arguments
                        .get("path")
                        .or_else(|| c.arguments.get("target_file"))
                        .or_else(|| c.arguments.get("TargetFile"))
                        .and_then(|v| v.as_str())
                });
                cross_turn_tool_count += 1;
                // Runtime probes, browser checks, and fresh reads can provide
                // decisive evidence without changing a file. The progress
                // ledger already classifies that evidence; carry its result
                // into the cross-turn reasoning detector instead of treating
                // "no workspace diff" as "no progress".
                cross_turn_made_progress |= assessment.meaningful;
                cross_turn_had_edits |= is_mutating_tool(&name);
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
                    ctx.progress.consecutive_no_progress = 0;
                } else if !assessment.suppress_stagnation {
                    ctx.progress.consecutive_no_progress += 1;
                    ctx.metrics.no_progress_results += 1;
                }
                if !assessment.suppress_stagnation
                    && (assessment.reason == loop_detect::ProgressReason::Churn
                        || assessment.streak >= loop_detect::ProgressLedger::RECOVERY_STREAK)
                {
                    evidence_recovery = Some((assessment.reason, assessment.streak, name.clone()));
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
                if !assessment.suppress_stagnation && !benign_shell_failure {
                    match ctx
                        .recovery
                        .loop_detector
                        .record_output(loop_detect::stagnation_key(&content))
                    {
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

            for (index, call_ref) in call_refs.iter().enumerate() {
                if selected_call_indices.contains(&index) {
                    continue;
                }
                let (reason, error_kind) = validation_errors[index]
                    .as_deref()
                    .map(|reason| (reason, crate::tools::ToolErrorKind::Validation))
                    .unwrap_or((
                        "not executed in this model round; reissue it only if still needed",
                        crate::tools::ToolErrorKind::Internal,
                    ));
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
            for (_, message) in result_messages {
                s.history.push(message);
            }
            if deferred_call_count > 0 {
                let deferred = tool_calls
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !selected_call_indices.contains(index))
                    .map(|(index, call)| {
                        call_refs
                            .get(index)
                            .map(|call_ref| format!("{} ({})", call.name, call_ref.id))
                            .unwrap_or_else(|| call.name.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                s.history.push(ChatMessage::new(
                    "system",
                    format!(
                        "[The model emitted {requested_calls} tool calls. {} were executed this round; the remaining calls ({deferred}) were not executed or scheduled. Reissue deferred calls only after reviewing the real results.]",
                        selected_call_indices.len()
                    ),
                ));
            }

            if background_pending {
                crate::config::save_history(&s.history);
                s.clear_current_response();
                drop(s);
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::BackgroundPending);
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

            let output_abort = matches!(stagnation, loop_detect::LoopStatus::Abort(_));
            if should_apply_loop_recovery(completed, output_abort, evidence_recovery.is_some()) {
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
                let recovery_action = if targeted_recovery_exhausted {
                    LoopRecoveryAction::ForceFinal
                } else {
                    loop_recovery_action_for(ctx.recovery.loop_recovery_attempts, read_only_batch)
                };
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
                        crate::config::save_history(&s.history);
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
                        crate::config::save_history(&s.history);
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
                crate::config::save_history(&s.history);
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
                crate::config::save_history(&s.history);
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
                crate::config::save_history(&s.history);
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
                    crate::config::save_history(&s.history);
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
                        .unwrap_or_else(|| {
                            "No verification command was run after the latest edit.".to_string()
                        });
                    s.history.push(ChatMessage::new(
                        "system",
                        format!(
                            "[Finish blocked — {reason} Run the relevant project verification command after the latest edit, inspect its result, then report completion.]"
                        ),
                    ));
                    crate::config::save_history(&s.history);
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
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                    drop(s);
                    let compiler_errors = cached_compiler_check(
                        &root,
                        &mut ctx.compiler.dirty,
                        &mut ctx.compiler.cache,
                        cancel_token,
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
                            crate::config::save_history(&s.history);
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
            crate::config::save_history(&s.history);
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
        crate::config::save_history(&s.history);
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
mod tests {
    use super::super::GroundedArtifactEvidence;
    use super::{
        batch_invalidates_read_recovery, benign_shell_wrapper_failure,
        bounded_malformed_tool_history, content_bearing_inspection_status,
        grounded_artifact_recovery_message, incomplete_tool_result, mutation_batch_guidance,
        selected_tool_call_indices, should_apply_loop_recovery, targeted_no_progress_guidance,
    };
    use crate::network::events::ToolResultMetadata;
    use crate::tools::ToolCall;
    use rustcode_core::{InspectionRange, InspectionResultMetadata, ToolResultCompleteness};

    fn inspection_metadata(complete: bool) -> InspectionResultMetadata {
        InspectionResultMetadata {
            requested_path: Some("src/app.ts".to_string()),
            requested_range: Some(InspectionRange {
                start: Some(1),
                end: Some(20),
            }),
            returned_path: Some("src/app.ts".to_string()),
            returned_range: Some(InspectionRange {
                start: Some(1),
                end: Some(20),
            }),
            complete,
            fingerprint: "read:src/app.ts:1:20".to_string(),
            ..Default::default()
        }
    }

    fn read_call(command: &str) -> ToolCall {
        ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": command}),
            call_id: None,
        }
    }

    #[test]
    fn completion_request_reaches_finish_gates_before_loop_recovery() {
        assert!(!should_apply_loop_recovery(true, true, true));
        assert!(!should_apply_loop_recovery(true, false, true));
    }

    #[test]
    fn ordinary_tool_rounds_still_apply_loop_recovery() {
        assert!(should_apply_loop_recovery(false, true, false));
        assert!(should_apply_loop_recovery(false, false, true));
        assert!(!should_apply_loop_recovery(false, false, false));
    }

    #[test]
    fn grounded_artifact_recovery_is_bounded_and_targeted() {
        let evidence = GroundedArtifactEvidence {
            path: "src/index.html".to_string(),
            write_lines: Some(699),
            write_bytes: Some(23_785),
            read_range: Some((1, 699)),
            repair_attempts: 0,
        };
        let message = grounded_artifact_recovery_message(
            &evidence,
            &format!("error: {}", "x".repeat(10_000)),
            false,
        );

        assert!(message.contains("src/index.html"));
        assert!(message.contains("699 lines / 23785 bytes"));
        assert!(message.contains("complete read of lines 1-699"));
        assert!(message.contains("last repair attempt made no change"));
        assert!(message.contains("exactly one narrow replace_file_content"));
        assert!(message.contains("Do not reread the whole file"));
        assert!(message.len() < 2_000, "recovery notice was not bounded");
    }

    #[test]
    fn single_call_guidance_preserves_mutation_policy() {
        for limit in [1, 3] {
            let guidance = mutation_batch_guidance(&crate::config::ToolSchedulingPolicy {
                max_mutating_calls: limit,
                ..Default::default()
            });
            assert!(guidance.contains("exactly one tool call per response"));
            assert!(guidance.contains(&format!("mutation budget remains {limit}")));
            assert!(guidance.contains("Read-only inspection never consumes it"));
            assert!(!guidance.contains("parallel"));
        }
    }

    #[test]
    fn default_scheduler_keeps_one_valid_call() {
        let calls = vec![read_call("git status"), read_call("git diff")];
        let errors = vec![None, None];
        let selected = selected_tool_call_indices(
            &calls,
            &errors,
            crate::config::ToolSchedulingPolicy::default(),
        );
        assert_eq!(selected, vec![0]);
    }

    #[test]
    fn trusted_scheduler_applies_separate_read_and_mutation_limits() {
        let calls = vec![
            read_call("git status"),
            read_call("git diff"),
            ToolCall {
                name: "write_to_file".to_string(),
                arguments: serde_json::json!({"path": "a", "content": "a"}),
                call_id: None,
            },
            ToolCall {
                name: "write_to_file".to_string(),
                arguments: serde_json::json!({"path": "b", "content": "b"}),
                call_id: None,
            },
        ];
        let errors = vec![None, None, None, None];
        let selected = selected_tool_call_indices(
            &calls,
            &errors,
            crate::config::ToolSchedulingPolicy {
                allow_batching: true,
                max_read_only_calls: 1,
                max_mutating_calls: 1,
                ..Default::default()
            },
        );
        assert_eq!(selected, vec![0, 2]);
    }

    #[test]
    fn scheduler_isolates_control_plane_calls() {
        let calls = vec![
            read_call("git status"),
            ToolCall {
                name: "use_skill".to_string(),
                arguments: serde_json::json!({"name": "rust"}),
                call_id: None,
            },
            read_call("git diff"),
        ];
        let errors = vec![None, None, None];
        let selected = selected_tool_call_indices(
            &calls,
            &errors,
            crate::config::ToolSchedulingPolicy {
                allow_batching: true,
                ..Default::default()
            },
        );
        assert_eq!(selected, vec![1]);
    }

    #[test]
    fn read_only_inspection_stays_executable_while_loop_recovery_is_pending() {
        // #984 must not weaken #983: read-only classification is evaluated
        // before the mutating cap in every batch path, including recovery, so
        // inspection is never dropped or reprimanded for budget reasons while
        // the harness nudges the model back on track.
        use crate::tools::{is_read_only_call, partition_tool_batch, validate_tool_calls};

        let shell = |command: &str| ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": command}),
            call_id: None,
        };
        let batch = vec![
            ToolCall {
                name: "replace_file_content".to_string(),
                arguments: serde_json::json!({
                    "path": "src/a.ts",
                    "edits": [{"old_string": "a", "new_string": "b"}]
                }),
                call_id: None,
            },
            shell("git status --short"),
            shell("ls src"),
            shell("cat src/app.ts"),
        ];
        assert!(!is_read_only_call(&batch[0]));
        assert!(batch.iter().skip(1).all(is_read_only_call));
        let limit = crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE;
        let (kept, dropped) = partition_tool_batch(batch, limit);
        assert!(
            dropped.is_empty(),
            "recovery must not drop inspection: {dropped:?}"
        );
        assert_eq!(kept.len(), 4);
        assert!(validate_tool_calls(&kept, limit).is_ok());
    }

    fn read_observation(action: &str) -> super::loop_detect::ProgressObservation {
        super::loop_detect::ProgressObservation {
            action: action.to_owned(),
            output_fingerprint: super::loop_detect::stable_hash(action),
            state_fingerprint: None,
            failure_fingerprint: None,
            changed_workspace: false,
            fresh_read: true,
            search_result: false,
            no_result: false,
            verification: false,
            read_only: true,
            replayed: false,
            success: true,
        }
    }

    #[test]
    fn fresh_evidence_invalidates_repeated_read_recovery_in_either_batch_order() {
        use super::loop_detect::{ProgressLedger, ProgressReason};

        let old = read_observation("read:macros.rs:1:80");
        let fresh = read_observation("read:service.rs:1:100");
        for batch in [[&old, &old, &old, &fresh], [&fresh, &old, &old, &old]] {
            let mut ledger = ProgressLedger::default();
            ledger.observe(&old);
            let mut made_progress = false;
            let mut recovery = None;
            for observation in batch {
                let assessment = ledger.observe(observation);
                made_progress |= assessment.meaningful;
                if assessment.streak >= ProgressLedger::RECOVERY_STREAK {
                    recovery = Some((
                        assessment.reason,
                        assessment.streak,
                        observation.action.clone(),
                    ));
                }
            }
            assert_eq!(
                recovery.as_ref().unwrap().0,
                ProgressReason::NoNewInformation
            );
            assert!(batch_invalidates_read_recovery(
                made_progress,
                recovery.as_ref()
            ));
        }
    }

    #[test]
    fn no_progress_recovery_names_the_repeated_action_and_preserves_resumability() {
        let guidance = targeted_no_progress_guidance(
            super::loop_detect::ProgressReason::NoNewInformation,
            3,
            "read:src/main.rs:1:80",
        );
        assert!(guidance.contains("no_new_information"));
        assert!(guidance.contains("read:src/main.rs:1:80"));
        assert!(guidance.contains("Do not replay"));
        assert!(guidance.contains("preserve completed tool results"));
    }

    #[test]
    fn all_repeated_reads_still_request_recovery() {
        use super::loop_detect::ProgressLedger;

        let old = read_observation("read:macros.rs:1:80");
        let mut ledger = ProgressLedger::default();
        ledger.observe(&old);
        let mut made_progress = false;
        let mut recovery = None;
        for _ in 0..ProgressLedger::RECOVERY_STREAK {
            let assessment = ledger.observe(&old);
            made_progress |= assessment.meaningful;
            recovery = Some((assessment.reason, assessment.streak, old.action.clone()));
        }
        assert!(!batch_invalidates_read_recovery(
            made_progress,
            recovery.as_ref()
        ));
        assert!(should_apply_loop_recovery(false, false, recovery.is_some()));
    }

    #[test]
    fn fresh_reads_do_not_clear_failure_or_churn_recovery() {
        use super::loop_detect::ProgressReason;

        for reason in [
            ProgressReason::RepeatedFailure,
            ProgressReason::Churn,
            ProgressReason::RepeatedVerification,
        ] {
            let recovery = (reason, 3, "run_command".to_owned());
            assert!(!batch_invalidates_read_recovery(true, Some(&recovery)));
        }
    }

    #[test]
    fn expected_shell_wrapper_statuses_do_not_become_recovery_failures() {
        let no_match = read_call("rg missing_symbol src | head -20");
        let no_match_metadata = ToolResultMetadata {
            success: false,
            exit_code: Some(1),
            ..Default::default()
        };
        assert!(benign_shell_wrapper_failure(
            Some(&no_match),
            &no_match_metadata,
            "exit code: 1\n(no output)"
        ));

        let sigpipe = read_call("rg symbol src | head -20");
        let sigpipe_metadata = ToolResultMetadata {
            success: false,
            exit_code: Some(141),
            ..Default::default()
        };
        assert!(benign_shell_wrapper_failure(
            Some(&sigpipe),
            &sigpipe_metadata,
            "exit code: 141\n(no output)"
        ));

        let real_failure = read_call("rg symbol src && cargo test");
        assert!(!benign_shell_wrapper_failure(
            Some(&real_failure),
            &no_match_metadata,
            "exit code: 1\nerror: cargo test failed"
        ));
    }

    #[test]
    fn truncated_results_block_same_batch_completion() {
        let metadata = ToolResultMetadata {
            completeness: ToolResultCompleteness::LineTruncated,
            truncated: true,
            ..Default::default()
        };
        assert!(incomplete_tool_result(&metadata));
    }

    #[test]
    fn complete_and_user_limited_results_do_not_block_completion() {
        assert!(!incomplete_tool_result(&ToolResultMetadata::default()));
        assert!(!incomplete_tool_result(&ToolResultMetadata {
            completeness: ToolResultCompleteness::UserLimited,
            ..Default::default()
        }));
    }

    #[test]
    fn malformed_tool_history_is_bounded_and_keeps_a_diagnostic_marker() {
        let content = format!("<tool_call>\n{}", "x".repeat(20_000));
        let bounded = bounded_malformed_tool_history(&content);

        assert!(bounded.len() <= super::MAX_MALFORMED_TOOL_HISTORY_BYTES);
        assert!(bounded.starts_with("<tool_call>"));
        assert!(bounded.contains("malformed tool response truncated by harness"));
        assert!(bounded.contains("bytes omitted"));
    }

    #[test]
    fn malformed_tool_history_does_not_split_utf8() {
        let content = format!("<tool_call>{}", "🙂".repeat(4_000));
        let bounded = bounded_malformed_tool_history(&content);

        assert!(bounded.len() <= super::MAX_MALFORMED_TOOL_HISTORY_BYTES);
        assert!(std::str::from_utf8(bounded.as_bytes()).is_ok());
    }

    #[test]
    fn content_bearing_complete_inspection_requires_typed_metadata() {
        let call = read_call("cat src/app.ts");
        let mut metadata = ToolResultMetadata {
            success: true,
            inspection: Some(inspection_metadata(true)),
            ..Default::default()
        };
        assert_eq!(
            content_bearing_inspection_status(Some(&call), &metadata, "1: export const app = 1;"),
            Some(true)
        );

        metadata.inspection = None;
        assert_eq!(
            content_bearing_inspection_status(Some(&call), &metadata, "source"),
            None
        );
    }

    #[test]
    fn wc_and_od_are_not_content_bearing_source_inspections() {
        for command in ["wc -l src/app.ts", "od -An -tx1 src/app.ts"] {
            let call = read_call(command);
            let metadata = ToolResultMetadata {
                success: true,
                inspection: Some(inspection_metadata(true)),
                ..Default::default()
            };
            assert_eq!(
                content_bearing_inspection_status(Some(&call), &metadata, "20"),
                None,
                "{command} must not count as source content"
            );
        }
    }

    #[test]
    fn incomplete_typed_content_inspection_cannot_support_completion() {
        let call = read_call("cat src/app.ts");
        let metadata = ToolResultMetadata {
            success: true,
            completeness: ToolResultCompleteness::ByteTruncated,
            truncated: true,
            inspection: Some(inspection_metadata(false)),
            ..Default::default()
        };
        assert_eq!(
            content_bearing_inspection_status(Some(&call), &metadata, "partial source"),
            Some(false)
        );
    }
}
