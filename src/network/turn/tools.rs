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
    FORCE_ANSWER_PROMPT, LOOP_RECOVERY_PROMPT, LoopRecoveryAction, active_todo_checkpoint,
    cached_compiler_check, call_refs_for, compiler_diagnostic_fingerprint,
    completion_block_message, completion_claims_unapplied_work, failure_replan_message,
    is_mutating_tool, loop_recovery_action, mutation_made_progress, push_or_replace_loop_warning,
    truncated_batch_summary, unanswered_call_results, unanswered_call_results_with_kind,
    update_compiler_diagnostic_streak,
};
use super::recovery::record_malformed_call;
use super::{
    TurnContext, append_cancelled_batch_results, hydrate_explicit_verification_from_history,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolHandlingOutcome {
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_tool_response<P: policy::TurnPolicy + 'static>(
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
    let (protocol, max_mutating_calls, mutation_limit_source) = {
        let state = state.lock().await;
        let profile = state.active_model_profile();
        let mutation_limit_source = profile
            .as_ref()
            .and_then(|profile| profile.max_mutating_calls_per_response)
            .filter(|limit| *limit > 0)
            .map(|_| "profile")
            .unwrap_or("default");
        (
            state.active_tool_protocol(),
            profile
                .as_ref()
                .map(|profile| profile.max_mutating_calls_per_response())
                .unwrap_or(crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE),
            mutation_limit_source,
        )
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
    if response_finish_reason == Some("length") && !parsed_tool_calls.is_empty() {
        let call_refs = call_refs_for(&parsed_tool_calls, &ctx.response.streamed_call_ids);
        let mut s = state.lock().await;
        let mut message = ChatMessage::new("assistant", &ctx.response.final_content)
            .with_tool_calls(call_refs.clone());
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
    let (parsed_tool_calls, dropped_calls) =
        crate::tools::truncate_tool_batch(parsed_tool_calls, max_mutating_calls);
    if dropped_calls > 0 {
        dbg_log!(
            "Oversized batch: running {} of {} requested tool calls",
            parsed_tool_calls.len(),
            requested_calls
        );
        crate::logger::operational_event(
            "tools.batch_truncated",
            serde_json::json!({
                "requested": requested_calls,
                "kept": parsed_tool_calls.len(),
                "dropped": dropped_calls,
                "max_mutating_calls": max_mutating_calls,
                "max_mutating_calls_source": mutation_limit_source,
            }),
        );
        ctx.response.final_content = truncated_batch_summary(&parsed_tool_calls, dropped_calls);
    }
    let oversized_batch = dropped_calls > 0;
    if let Err(reason) = crate::tools::validate_tool_calls(&parsed_tool_calls, max_mutating_calls) {
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
        let guidance = if oversized_batch {
            ctx.recovery.oversized_batch_rejections =
                ctx.recovery.oversized_batch_rejections.saturating_add(1);
            if ctx.recovery.oversized_batch_rejections >= 2 {
                ctx.recovery.force_final = true;
            }
            format!(
                " This response contained {requested_calls} separate tool calls; the leading {} were kept and the rest dropped, then the remainder failed validation, so nothing ran and nothing it claimed about their results happened. Start again from the last real tool result. Reads may be issued together; keep calls that change the workspace to at most {} per response so each one is grounded in the previous result.",
                parsed_tool_calls.len(),
                max_mutating_calls
            )
        } else {
            ctx.recovery.oversized_batch_rejections = 0;
            String::new()
        };
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
                        "[Tool call rejected before execution: {reason}] Emit one corrected tool call.{guidance}{repeat_guidance}"
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
    let (tool_calls, deferred_tool_calls) =
        crate::tools::isolate_control_plane_call(parsed_tool_calls);
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
        for call in &tool_calls {
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
                match loop_recovery_action(ctx.recovery.loop_recovery_attempts) {
                    LoopRecoveryAction::Recover => {
                        ctx.recovery.loop_recovery_attempts += 1;
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
                        s.history
                            .push(ChatMessage::new("system", LOOP_RECOVERY_PROMPT));
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

            let approved = policy.should_approve(state, &tool_calls).await;

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
                if dropped_calls > 0 {
                    s.history.push(ChatMessage::new(
                                "system",
                                format!(
                                    "[{dropped_calls} of the {requested_calls} tool calls in that response were dropped; only the first {} ran. Their results follow — plan the next step from those, not from what the response predicted. Reads may be issued together; keep calls that change the workspace to at most {} per response.]",
                                    tool_calls.len(),
                                    max_mutating_calls
                                ),
                            ));
                }
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

            let deferred_notice = (deferred_tool_calls > 0).then(|| {
                format!(
                    "[harness: deferred {deferred_tool_calls} additional tool call(s) until the next model turn after skill loading]"
                )
            });
            // Phase 4: execute the accepted tool batch and record progress evidence.
            let results = execute_tool_batch(
                client,
                state,
                cancel_token,
                &tool_calls,
                ctx.lifecycle.turn_machine.state() == events::TurnState::ExecutingTools,
                &ctx.compiler.edit_root,
                &mut ctx.compiler.dirty,
                &mut ctx.compiler.cache,
                &mut ctx.lifecycle.user_wait_duration,
                deferred_notice,
            )
            .await;

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
                    "dropped": dropped_calls,
                    "max_mutating_calls": max_mutating_calls,
                    "max_mutating_calls_source": mutation_limit_source,
                    "successes": results.iter().filter(|result| result.metadata.success).count(),
                    "failed": results.iter().filter(|result| !result.metadata.success).count(),
                    "changed_paths": results.iter().map(|result| result.metadata.changed_paths.len()).sum::<usize>(),
                }),
            );

            if cancel_token.is_cancelled() {
                dbg_log!("Orchestrator: Cancelled during tool execution");
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Cancelled);
                let mut s = state.lock().await;
                append_cancelled_batch_results(s.history.as_mut_vec(), results, &call_refs);
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
            let mut cross_turn_made_progress = false;
            let mut cross_turn_had_edits = false;
            let mut cross_turn_target_files = Vec::new();
            let mut cross_turn_tool_count = 0;
            let executed = results.len();
            for (position, result) in results.into_iter().enumerate() {
                let call = tool_calls.get(position);
                let answered_call = call_refs.get(position).map(|call| call.id.clone());
                let name = result.tool_name;
                let mut metadata = result.metadata.clone();
                // The provider call id is attached at the orchestration
                // boundary, after parsing, and is then carried into the
                // history message that answers that exact call.
                metadata.call_id = answered_call.clone();
                let content = result.content;
                let diff_opt = result.diff;
                let file_preview = result.file_preview;
                if metadata.pending {
                    background_pending = true;
                    s.history.push(tool_result_history_message(
                        ToolResult {
                            tool_name: name,
                            content,
                            diff: diff_opt,
                            file_preview,
                            metadata,
                        },
                        answered_call,
                    ));
                    continue;
                }
                let mut verification_command = false;
                if (name == "run_command" || metadata.command.is_some())
                    && let Some(command) = call
                        .and_then(|call| call.arguments.get("command"))
                        .and_then(|command| command.as_str())
                        .or(metadata.command.as_deref())
                {
                    ctx.verification
                        .ledger
                        .record_command(command, metadata.exit_code);
                    if explicit_verification_requested {
                        ctx.verification
                            .ledger
                            .record_explicit_command(command, metadata.exit_code);
                    }
                    verification_command = verification::is_verification_command(command)
                        || loop_detect::is_stable_inspection_command(command);
                }
                dbg_log!(
                    "Tool '{}' finished with result length: {} chars",
                    name,
                    content.len()
                );
                if name == "complete_task" {
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
                    let failed = !metadata.success
                        || content
                            .trim_start()
                            .to_ascii_lowercase()
                            .starts_with("error");
                    let made_progress = mutation_made_progress(metadata.success, &content);
                    mutation_progress = made_progress && diff_opt.is_some();
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
                    }
                }

                // Keep authoritative write metadata separate from display
                // output. A later read can then prove that it is checking the
                // same revision without replaying source into recovery.
                if is_mutating_tool(&name) && metadata.success {
                    for path in &metadata.changed_paths {
                        let content = file_preview.as_ref().and_then(|(preview_path, content)| {
                            (preview_path == path).then_some(content.as_str())
                        });
                        ctx.progress.file_evidence.record_mutation(path, content);
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
                    && let Some((path, start_line, end_line)) =
                        loop_detect::read_target(&call.name, &call.arguments)
                    && let Some(recovery) = ctx.progress.file_evidence.record_read_with_kind(
                        &path,
                        start_line,
                        end_line,
                        !metadata.truncated,
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
                let failure_fingerprint = (!metadata.success).then(|| {
                    loop_detect::stable_hash(&format!(
                        "{name}:{}:{}",
                        metadata.exit_code.unwrap_or_default(),
                        loop_detect::stagnation_key(&content)
                    ))
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
                        success: metadata.success,
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
                if let Some(target_file) = target_file {
                    cross_turn_target_files.push(target_file.to_string());
                }
                ctx.progress.last_reason = Some(assessment.reason);
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
                if !assessment.suppress_stagnation {
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
                s.history.push(tool_result_history_message(
                    ToolResult {
                        tool_name: name,
                        content,
                        diff: diff_opt,
                        file_preview,
                        metadata,
                    },
                    answered_call,
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

            // `record_turn_evidence` models a complete model turn. Record the
            // batch once after all results are processed; calling it for each
            // result makes two read-only calls from one response look like two
            // repeated turns and triggers a false cross-turn loop.
            // Completion has its own evidence, verification, and compiler
            // gates below. Do not let generic loop recovery intercept a
            // `complete_task` request before those authoritative gates run.
            if !completed && cross_turn_tool_count > 0 {
                let target_file_refs = cross_turn_target_files
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let cross_turn_status = ctx.recovery.reasoning_loop_detector.record_turn_evidence(
                    &loop_detect::TurnEvidence {
                        reasoning: &ctx.response.final_content,
                        target_files: &target_file_refs,
                        made_progress: cross_turn_made_progress,
                        had_edits: cross_turn_had_edits,
                        tool_count: cross_turn_tool_count,
                        no_progress_streak: ctx.progress.ledger.no_progress_streak(),
                    },
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
            if executed < call_refs.len() {
                for message in
                    unanswered_call_results(&call_refs[executed..], "no result was produced")
                {
                    s.history.push(message);
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
                let recovery_guidance = if reason
                    == loop_detect::ProgressReason::RepeatedVerification
                {
                    "This verification already passed for the unchanged workspace. Do not run it again. Verify a different user-visible behavior, make a necessary edit, or finish."
                } else {
                    "Use a different, evidence-producing next step; do not repeat the same unchanged read, no-result search, no-op edit, or failed command."
                };
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
                match loop_recovery_action(ctx.recovery.loop_recovery_attempts) {
                    LoopRecoveryAction::Recover => {
                        ctx.recovery.loop_recovery_attempts += 1;
                        ctx.metrics.evidence_recoveries += 1;
                        ctx.recovery.loop_detector.reset();
                        s.history.push(ChatMessage::new(
                            "system",
                            format!("{evidence}\n{LOOP_RECOVERY_PROMPT}"),
                        ));
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
                s.history.push(ChatMessage::new("system", replan));
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
                ctx.lifecycle.stop_reason = Some(lifecycle::StopReason::Completed);
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
        let mut msg = ChatMessage::new("assistant", &ctx.response.final_content);
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
    use super::should_apply_loop_recovery;

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
}
