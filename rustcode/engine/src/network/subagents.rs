#[path = "subagents/agent_tools.rs"]
mod agent_tools;

pub(crate) use agent_tools::handle_agent_tool;
#[cfg(test)]
use agent_tools::{agent_id_arg, agent_ids_arg};
use agent_tools::{agent_notice, handle_agent_tool_for_parent};

use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::loop_detect;
use super::messages::{inject_system_reminder, trim_msgs_to_budget};
use super::runner;
use super::{
    StreamBuffer, compact_history_to_budget, confirm_and_execute, final_tool_diff,
    subagent_tool_history_message, tool_result_precludes_preview_fallback,
};
use rustcode_tool_protocol::text::{
    continuation_nudge_for_category, format_continuation_assistant_message, strip_leading_think,
};

fn subagent_model_result_content(message: &ChatMessage) -> String {
    let content = message
        .content
        .split_once(": ")
        .map(|(_, rest)| rest)
        .unwrap_or(&message.content);
    if let Some(metadata) = message.tool_result.as_ref() {
        let mut metadata = metadata.clone();
        metadata.completeness = metadata.resolved_completeness();
        format!(
            "{content}\n[result_metadata: {}]",
            serde_json::to_string(&metadata).unwrap_or_default()
        )
    } else {
        content.to_string()
    }
}

pub(crate) async fn set_subagent_status(
    state: &Arc<Mutex<AppState>>,
    agent_id: u32,
    status: crate::app::SubAgentStatus,
) {
    let mut state = state.lock().await;
    let _ = crate::app::SubagentController.set_status(
        &mut state,
        crate::app::SubagentId::from_raw(agent_id),
        status,
    );
}

fn subagent_history_message(
    message: &ChatMessage,
    protocol: crate::config::ToolProtocol,
) -> serde_json::Value {
    if protocol == crate::config::ToolProtocol::ApiNative
        && message.role == "assistant"
        && !message.tool_calls.is_empty()
    {
        return serde_json::json!({
            "role": "assistant",
            "content": if message.content.trim().is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(message.content.clone())
            },
            "tool_calls": message.tool_calls.iter().map(|call| serde_json::json!({
                "id": call.id,
                "type": "function",
                "function": {
                    "name": call.name,
                    "arguments": call.arguments,
                },
                "thought_signature": "context",
            })).collect::<Vec<_>>(),
        });
    }
    if protocol == crate::config::ToolProtocol::ApiNative
        && message.role == "tool"
        && let Some(call_id) = &message.tool_call_id
    {
        return serde_json::json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": subagent_model_result_content(message),
        });
    }
    if message.role == "tool" {
        serde_json::json!({
            "role": "user",
            "content": format!(
                "<tool_result>\n{}\n</tool_result>",
                subagent_model_result_content(message)
            ),
        })
    } else {
        serde_json::json!({"role": message.role, "content": message.content})
    }
}

fn prepare_subagent_tool_batch(
    parsed_calls: &mut Vec<(crate::tools::ToolCall, Option<String>)>,
    max_mutating_calls: usize,
) -> (usize, usize, Result<(), String>) {
    let requested_calls = parsed_calls.len();
    let policy_calls = parsed_calls
        .iter()
        .map(|(call, _)| call.clone())
        .collect::<Vec<_>>();
    let (kept_policy_calls, dropped_calls) =
        crate::tools::partition_tool_batch(policy_calls, max_mutating_calls);
    let mut retained_policy_calls = kept_policy_calls.clone();
    let retained = parsed_calls
        .drain(..)
        .filter(|(call, _)| {
            retained_policy_calls
                .iter()
                .position(|retained| retained == call)
                .map(|index| {
                    retained_policy_calls.remove(index);
                    true
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    *parsed_calls = retained;
    let validation = crate::tools::validate_tool_calls(&kept_policy_calls, max_mutating_calls);
    (requested_calls, dropped_calls.len(), validation)
}

fn parse_subagent_tool_calls(
    content: &str,
    protocol: crate::config::ToolProtocol,
    native_tool_calls: Vec<crate::tools::ToolCallEnvelope>,
) -> Vec<(crate::tools::ToolCall, Option<String>)> {
    if protocol == crate::config::ToolProtocol::ApiNative {
        return native_tool_calls
            .into_iter()
            .map(|call| {
                let call_id = call.call_id;
                (
                    crate::tools::ToolCall {
                        name: call.tool_name,
                        arguments: call.arguments,
                        call_id: Some(call_id.clone()),
                    },
                    Some(call_id),
                )
            })
            .collect();
    }

    crate::tools::parse_tool_call(content, protocol)
        .map(|call| (call, None))
        .into_iter()
        .collect()
}

pub(crate) async fn run_subagent(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: u32,
    owner_session_id: &str,
) -> Result<String, String> {
    let child_started = std::time::Instant::now();
    crate::logger::operational_event(
        "subagent.start",
        serde_json::json!({"agent_id": agent_id, "session_id": owner_session_id}),
    );
    {
        let mut s = state.lock().await;
        if s.active_session_id != owner_session_id {
            return Err("error: agent owner session changed".into());
        }
        let _ = crate::app::SubagentController.set_status(
            &mut s,
            crate::app::SubagentId::from_raw(agent_id),
            crate::app::SubAgentStatus::Running,
        );
        if let Some(agent) = s.subagents.iter().find(|agent| agent.id == agent_id) {
            crate::logger::operational_event(
                "subagent.queue.finish",
                serde_json::json!({"agent_id":agent_id,"session_id":owner_session_id,"queue_ms":agent.started_at_ms.unwrap_or(agent.created_at_ms).saturating_sub(agent.queued_at_ms),"depth":agent.depth}),
            );
        }
    }
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let mut rounds = 0usize;
    let (loop_abort, max_rounds, max_wall) = {
        let guard = state.lock().await.config.loop_guard;
        (
            guard.effective_loop_abort(),
            guard.effective_subagent_max_rounds(),
            guard.effective_subagent_max_wall(),
        )
    };
    let mut loop_detector = loop_detect::LoopDetector::new(loop_abort);
    let mut model_rounds = 0usize;
    let mut saved_history_len = None;
    loop {
        if cancel_token.is_cancelled() {
            crate::logger::operational_event(
                "subagent.finish",
                serde_json::json!({"agent_id": agent_id, "status": "cancelled", "session_id": owner_session_id}),
            );
            return Err("error: cancelled".to_string());
        }
        let over_limit = if model_rounds >= max_rounds {
            Some(format!("reached its limit of {max_rounds} model rounds"))
        } else if child_started.elapsed() >= max_wall {
            Some(format!("ran past its {}s time limit", max_wall.as_secs()))
        } else {
            None
        };
        if let Some(reason) = over_limit {
            crate::logger::operational_event(
                "subagent.finish",
                serde_json::json!({"agent_id": agent_id, "status": "failed", "reason": "limit", "rounds": model_rounds, "session_id": owner_session_id}),
            );
            return Err(failure_with_partial_answer(
                state,
                agent_id,
                format!("error: subagent {agent_id} {reason}"),
            )
            .await);
        }
        model_rounds += 1;
        let (base_history, pending_save) = {
            let mut s = state.lock().await;
            if s.active_session_id != owner_session_id {
                return Err("error: agent owner session changed".into());
            }
            if let Some(agent) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                let pending = agent.mailbox.drain(..).collect::<Vec<_>>();
                if !pending.is_empty() {
                    Arc::make_mut(&mut agent.history).extend(
                        pending
                            .into_iter()
                            .map(|message| ChatMessage::new("user", message)),
                    );
                }
            }
            let base_history = s
                .subagents
                .iter()
                .find(|a| a.id == agent_id)
                .map(|a| Arc::clone(&a.history))
                .unwrap_or_default();
            // Snapshot only when this child's history moved since the last
            // write; the file is written after the lock is released.
            let pending_save = (saved_history_len != Some(base_history.len()))
                .then(|| crate::app::subagent_persistence::snapshot(&s));
            (base_history, pending_save)
        };
        if let Some(pending_save) = pending_save {
            pending_save.write();
            saved_history_len = Some(base_history.len());
        }
        if base_history.is_empty() {
            return Err(format!("error: no subagent with id {agent_id}"));
        }
        let mut history_snapshot = base_history.as_ref().clone();

        let (
            profile_name,
            api_base_url,
            model_name,
            budget_token_limit,
            max_mutating_calls,
            workspace_root,
            schema_policy,
            protocol,
        ) = {
            let s = state.lock().await;
            let subagent = s
                .subagents
                .iter()
                .find(|a| a.id == agent_id)
                .ok_or_else(|| format!("error: no subagent with id {agent_id}"))?;
            let profile = subagent_request_profile(&s, subagent.model.as_deref());
            let (api_base_url, model_name, budget) = profile
                .as_ref()
                .map(|profile| {
                    (
                        profile.url.clone(),
                        profile.model.clone(),
                        profile.context_budget().history_tokens,
                    )
                })
                .unwrap_or_else(|| {
                    (
                        s.api_base_url.clone(),
                        s.model_name.clone(),
                        s.active_context_budget().history_tokens,
                    )
                });
            let max_mutating_calls = profile
                .as_ref()
                .map(|profile| profile.max_mutating_calls_per_response())
                .unwrap_or(crate::config::DEFAULT_MAX_MUTATING_CALLS_PER_RESPONSE);
            // The child's own endpoint decides its protocol, not the parent's.
            let protocol = s.tool_protocol_for(&api_base_url);
            (
                profile.map(|profile| profile.name),
                api_base_url,
                model_name,
                budget,
                max_mutating_calls,
                subagent_context_workspace_root(&s, subagent_execution_root(subagent)),
                crate::tools::ToolSchemaPolicy::for_subagent(subagent.write_access),
                protocol,
            )
        };
        if compact_history_to_budget(&mut history_snapshot, budget_token_limit).await {
            // Keep the compacted history, so later rounds start under budget
            // instead of compacting (and logging it) again every round. A
            // history changed meanwhile is left alone.
            let mut s = state.lock().await;
            if let Some(agent) = s.subagents.iter_mut().find(|a| a.id == agent_id)
                && Arc::ptr_eq(&agent.history, &base_history)
            {
                agent.history = Arc::new(history_snapshot.clone());
            }
        }
        drop(base_history);

        let agent_mode = { state.lock().await.agent_mode };
        let delegation_contract = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| {
                    format!(
                        "Delegation contract: write_access={}, allowed_paths={:?}, verification_command={:?}.{}",
                        agent.write_access,
                        agent.allowed_paths,
                        agent.verification_command,
                        agent.agent_type.instructions()
                    )
                })
                .unwrap_or_else(|| "Delegation contract unavailable; remain read-only.".to_string())
        };
        let mut system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense. {delegation_contract}\n\n{}",
            crate::tools::tool_system_prompt_for_policy(schema_policy, protocol, agent_mode),
            workspace_root
                .as_deref()
                .map(crate::context::environment_context_at)
                .unwrap_or_else(crate::context::environment_context)
        );
        crate::tools::append_tool_response_limit(&mut system_prompt, max_mutating_calls);
        crate::tools::append_mcp_catalogue(&mut system_prompt, schema_policy);
        let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt,
        })];
        msgs.extend(
            history_snapshot
                .iter()
                .map(|message| subagent_history_message(message, protocol)),
        );
        inject_system_reminder(&mut msgs);
        trim_msgs_to_budget(&mut msgs, budget_token_limit);

        stream_buffer.lock().await.reset();
        crate::dbg_log_for_session!(
            owner_session_id,
            "subagent {} round {}: requesting {}",
            agent_id,
            rounds,
            model_name
        );
        let request_client = client.clone();
        let request_state = Arc::clone(state);
        let request_cancel = cancel_token.clone();
        let request_buffer = Arc::clone(&stream_buffer);
        let request_api_url = api_base_url.clone();
        let request_model = model_name.clone();
        let request_profile = profile_name.clone();
        let request_msgs: Arc<[serde_json::Value]> = msgs.into();
        let request_started = std::time::Instant::now();
        let context_bytes = request_msgs
            .iter()
            .map(|message| message.to_string().len())
            .sum::<usize>();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let request_counter = Arc::clone(&requests);
        let remaining = max_wall.saturating_sub(child_started.elapsed());
        let collected = match tokio::time::timeout(
            remaining,
            runner::collect_response(runner::ContinuationPolicy::default(), move |request| {
                request_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut current_msgs = Vec::with_capacity(
                    request_msgs.len() + if request.previous.is_empty() { 0 } else { 2 },
                );
                current_msgs.extend(request_msgs.iter().cloned());
                if !request.previous.is_empty() {
                    let continuation_assistant =
                        format_continuation_assistant_message(&request.previous);
                    let nudge = continuation_nudge_for_category(&request.previous, None);
                    current_msgs.push(serde_json::json!({
                        "role": "assistant",
                        "content": continuation_assistant
                    }));
                    current_msgs.push(serde_json::json!({
                        "role": "user",
                        "content": nudge
                    }));
                }
                let request_client = request_client.clone();
                let request_state = Arc::clone(&request_state);
                let request_cancel = request_cancel.clone();
                let request_buffer = Arc::clone(&request_buffer);
                let request_api_url = request_api_url.clone();
                let request_model = request_model.clone();
                let request_profile = request_profile.clone();
                async move {
                    request_buffer.lock().await.reset();
                    let finish_reason = super::stream_request::stream_request_for_profile(
                        &request_client,
                        request_state,
                        request_cancel,
                        request_profile.as_deref(),
                        &request_api_url,
                        &request_model,
                        current_msgs,
                        Arc::clone(&request_buffer),
                        true,
                        true,
                        super::stream_request::ThinkingMode::Normal,
                        schema_policy,
                        Some(owner_session_id),
                        request.output_token_limit,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    let buffer = request_buffer.lock().await;
                    Ok::<_, super::runner::ResponseError>(super::runner::ResponseChunk {
                        content: buffer.content.clone(),
                        final_answer_boundary: buffer.final_answer_boundary,
                        provider_final_answer_state: buffer.provider_final_answer_state,
                        finish_reason,
                        has_native_tool_calls: !buffer.native_tool_calls.is_empty(),
                        output_token_limit: buffer.output_token_limit,
                        thought_time_ms: buffer.thought_time_ms,
                        thought_tokens: buffer.thought_tokens,
                        token_usage: None,
                    })
                }
            }),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return Err(failure_with_partial_answer(
                    state,
                    agent_id,
                    format!("error: subagent request failed: {e}"),
                )
                .await);
            }
            Err(_) => {
                crate::logger::operational_event(
                    "subagent.finish",
                    serde_json::json!({"agent_id": agent_id, "status": "failed", "reason": "limit", "rounds": model_rounds, "session_id": owner_session_id}),
                );
                return Err(failure_with_partial_answer(
                    state,
                    agent_id,
                    format!(
                        "error: subagent {agent_id} ran past its {}s time limit",
                        max_wall.as_secs()
                    ),
                )
                .await);
            }
        };
        let request_performance = stream_buffer.lock().await.performance.clone();
        {
            let mut s = state.lock().await;
            if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == agent_id) {
                agent.performance.model_us += crate::benchmark::elapsed_us(request_started);
                agent.performance.requests += requests.load(std::sync::atomic::Ordering::Relaxed);
                agent.performance.rounds += 1;
                agent.performance.context_bytes = context_bytes;
                agent.performance.ttft_us = request_performance.ttft_us;
                agent.performance.input_tokens = request_performance.input_tokens;
                agent.performance.output_tokens = request_performance.output_tokens;
                agent.performance.cached_input_tokens = request_performance.cached_input_tokens;
                agent.usage.add(&request_performance);
                agent.last_activity_ms = Some(crate::app::subagent_controller::now_ms());
            }
        }
        crate::logger::operational_event(
            "subagent.model.finish",
            serde_json::json!({"agent_id":agent_id,"session_id":owner_session_id,"model_us":crate::benchmark::elapsed_us(request_started),"context_bytes":context_bytes,"input_tokens":request_performance.input_tokens,"output_tokens":request_performance.output_tokens}),
        );
        let content = collected.content;
        let native_tool_calls = stream_buffer.lock().await.native_tool_calls.clone();

        if content.is_empty() && native_tool_calls.is_empty() {
            return Err(failure_with_partial_answer(
                state,
                agent_id,
                "error: subagent returned an empty reply".to_string(),
            )
            .await);
        }

        let mut parsed_calls = parse_subagent_tool_calls(&content, protocol, native_tool_calls);

        if !parsed_calls.is_empty() {
            let (requested_calls, dropped_calls, validation) =
                prepare_subagent_tool_batch(&mut parsed_calls, max_mutating_calls);
            let kept_calls = parsed_calls.len();
            if dropped_calls > 0 {
                crate::logger::operational_event(
                    "subagent.batch_truncated",
                    serde_json::json!({
                        "agent_id": agent_id,
                        "session_id": owner_session_id,
                        "requested": requested_calls,
                        "kept": kept_calls,
                        "dropped": dropped_calls,
                        "max_mutating_calls": max_mutating_calls,
                    }),
                );
            }
            let call_refs = parsed_calls
                .iter()
                .filter_map(|(call, call_id)| {
                    call_id.as_ref().map(|id| crate::app::ToolCallRef {
                        id: id.clone(),
                        name: call.name.clone(),
                        arguments: serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "null".to_string()),
                    })
                })
                .collect::<Vec<_>>();
            {
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    Arc::make_mut(&mut a.history)
                        .push(ChatMessage::new("assistant", &content).with_tool_calls(call_refs));
                }
            }

            if let Err(reason) = validation {
                let mut s = state.lock().await;
                for (tool_call, call_id) in &parsed_calls {
                    let execution = crate::tools::ToolExecutionOutput::failure_with_kind(
                        format!("error: tool batch rejected before execution: {reason}"),
                        crate::tools::ToolErrorKind::Validation,
                        false,
                    );
                    let message = subagent_tool_history_message(
                        &tool_call.name,
                        &tool_call.arguments,
                        execution,
                        None,
                        call_id.clone(),
                    );
                    if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                        Arc::make_mut(&mut a.history).push(message);
                    }
                }
                if dropped_calls > 0
                    && let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id)
                {
                    Arc::make_mut(&mut a.history).push(ChatMessage::new(
                        "system",
                        format!(
                            "[Subagent tool batch kept {kept_calls} of {requested_calls} calls and dropped {dropped_calls}; the kept batch failed validation: {reason}]"
                        ),
                    ));
                }
                continue;
            }

            if dropped_calls > 0 {
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    Arc::make_mut(&mut a.history).push(ChatMessage::new(
                        "system",
                        format!(
                            "[Subagent tool batch kept {kept_calls} of {requested_calls} calls and dropped {dropped_calls}; continue from the real results and emit the next action after them. The resolved mutation limit is {max_mutating_calls}.]"
                        ),
                    ));
                }
            }

            for (index, (tool_call, call_id)) in parsed_calls.iter().enumerate() {
                let name = &tool_call.name;
                let args = &tool_call.arguments;
                let (exact, category) = loop_detect::signatures(name, args);
                if let loop_detect::LoopStatus::Abort(repeats) =
                    loop_detector.check_tool(name, &exact, &category)
                {
                    let execution = crate::tools::ToolExecutionOutput::failure_with_kind(
                        format!("error: repeated tool action stopped after {repeats} attempts"),
                        crate::tools::ToolErrorKind::Internal,
                        false,
                    );
                    let message =
                        subagent_tool_history_message(name, args, execution, None, call_id.clone());
                    let mut s = state.lock().await;
                    if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                        Arc::make_mut(&mut a.history).push(message);
                        for (remaining_call, remaining_id) in parsed_calls.iter().skip(index + 1) {
                            let skipped = crate::tools::ToolExecutionOutput::failure_with_kind(
                                "error: tool call was not run because the subagent loop stopped"
                                    .to_string(),
                                crate::tools::ToolErrorKind::Internal,
                                false,
                            );
                            Arc::make_mut(&mut a.history).push(subagent_tool_history_message(
                                &remaining_call.name,
                                &remaining_call.arguments,
                                skipped,
                                None,
                                remaining_id.clone(),
                            ));
                        }
                    }
                    drop(s);
                    return Err(failure_with_partial_answer(
                        state,
                        agent_id,
                        format!(
                            "error: subagent {agent_id} stopped after {repeats} repeated '{name}' actions"
                        ),
                    )
                    .await);
                }
                rounds += 1;
                let (write_access, allowed_paths, execution_root) = {
                    let s = state.lock().await;
                    s.subagents
                        .iter()
                        .find(|agent| agent.id == agent_id)
                        .map(|agent| {
                            (
                                agent.write_access,
                                agent.allowed_paths.clone(),
                                subagent_execution_root(agent),
                            )
                        })
                        .unwrap_or((false, Vec::new(), None))
                };
                let path_outside_contract =
                    call_paths_outside_contract(args, execution_root.as_deref(), &allowed_paths);
                let tool_started = std::time::Instant::now();
                let (execution, diff_opt, user_wait) = if crate::tools::is_agent_tool(name) {
                    let execution = Box::pin(handle_agent_tool_for_parent(
                        client,
                        state,
                        cancel_token,
                        name,
                        args,
                        Some(agent_id),
                    ))
                    .await;
                    (execution, None, std::time::Duration::ZERO)
                } else if !write_access && !is_read_only_subagent_call(tool_call) {
                    (
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            READ_ONLY_REFUSAL.to_string(),
                            crate::tools::ToolErrorKind::PermissionDenied,
                            false,
                        ),
                        None,
                        std::time::Duration::ZERO,
                    )
                } else if write_access && path_outside_contract {
                    (
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            "error: requested path is outside the subagent allowed_paths contract"
                                .to_string(),
                            crate::tools::ToolErrorKind::PermissionDenied,
                            false,
                        ),
                        None,
                        std::time::Duration::ZERO,
                    )
                } else {
                    {
                        let mut s = state.lock().await;
                        let target = args
                            .get("path")
                            .or_else(|| args.get("command"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        agent_notice(&mut s, format!("agent-{agent_id} → {name} {target}"));
                    }
                    confirm_and_execute(
                        client,
                        state,
                        cancel_token,
                        name,
                        args,
                        &format!("agent-{agent_id} · {name}"),
                        false,
                        {
                            let s = state.lock().await;
                            s.subagents
                                .iter()
                                .find(|agent| agent.id == agent_id)
                                .and_then(subagent_execution_root)
                        },
                        None,
                    )
                    .await
                };
                {
                    let mut s = state.lock().await;
                    if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == agent_id) {
                        let elapsed = crate::benchmark::elapsed_us(tool_started);
                        let user_wait = crate::benchmark::duration_us(user_wait);
                        let work = elapsed.saturating_sub(user_wait);
                        agent.performance.tool_calls += 1;
                        agent
                            .performance
                            .record_tool_batch(elapsed, work, user_wait);
                        agent.last_activity_ms = Some(crate::app::subagent_controller::now_ms());
                    }
                }
                let preview_fallback = if tool_result_precludes_preview_fallback(&execution.content)
                {
                    None
                } else {
                    diff_opt
                };
                let final_diff = final_tool_diff(&execution.content, preview_fallback);
                let message = subagent_tool_history_message(
                    name,
                    args,
                    execution,
                    final_diff,
                    call_id.clone(),
                );
                // One guard for both appends: the state mutex is not reentrant.
                let cancelled = cancel_token.is_cancelled();
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    Arc::make_mut(&mut a.history).push(message);
                    if cancelled {
                        for (remaining_call, remaining_id) in parsed_calls.iter().skip(index + 1) {
                            let cancelled = crate::tools::ToolExecutionOutput::failure_with_kind(
                                "error: tool call was not run because the subagent was cancelled"
                                    .to_string(),
                                crate::tools::ToolErrorKind::Cancelled,
                                false,
                            );
                            Arc::make_mut(&mut a.history).push(subagent_tool_history_message(
                                &remaining_call.name,
                                &remaining_call.arguments,
                                cancelled,
                                None,
                                remaining_id.clone(),
                            ));
                        }
                    }
                }
                drop(s);
                if cancelled {
                    return Err("error: cancelled".to_string());
                }
            }
            continue;
        }

        let mut s = state.lock().await;
        if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
            Arc::make_mut(&mut a.history).push(ChatMessage::new("assistant", &content));
            // A message admitted during the request must be processed before completion.
            if !a.mailbox.is_empty() {
                continue;
            }
            // Close message admission atomically with the final history append.
            // The supervisor publishes terminal state after durable completion projection.
            a.active_turn = false;
            a.performance.wall_us = crate::benchmark::elapsed_us(child_started);
            a.performance.completed = true;
            crate::logger::operational_event(
                "subagent.summary",
                serde_json::json!({"agent_id":agent_id,"session_id":owner_session_id,"performance":a.performance}),
            );
        }
        crate::app::subagent_persistence::save(&s);
        crate::logger::operational_event(
            "subagent.finish",
            serde_json::json!({"agent_id": agent_id, "status": "completed", "rounds": rounds, "session_id": owner_session_id}),
        );
        return Ok(strip_leading_think(&content).to_string());
    }
}

async fn project_subagent_completion(
    state: &Arc<Mutex<AppState>>,
    completion: crate::app::SubagentCompletion,
    owner_session_id: &str,
) {
    // A cancelled parent is terminal only after its owned descendants cleaned up.
    if completion.status == crate::app::SubAgentStatus::Cancelled {
        let (supervisor, descendants) = {
            let s = state.lock().await;
            if s.active_session_id != owner_session_id {
                return;
            }
            let mut ids = vec![completion.id.raw()];
            for _ in 0..3 {
                for agent in &s.subagents {
                    if agent.parent_id.is_some_and(|parent| ids.contains(&parent))
                        && !ids.contains(&agent.id)
                    {
                        ids.push(agent.id);
                    }
                }
            }
            (
                s.subagent_supervisor.clone(),
                ids.into_iter().skip(1).collect::<Vec<_>>(),
            )
        };
        for id in descendants {
            let id = crate::app::SubagentId::from_raw(id);
            if supervisor.is_active(id) {
                let _ = supervisor.wait(id).await;
            }
        }
    }
    let review_manifest = {
        let state = state.lock().await;
        if state.active_session_id != owner_session_id {
            return;
        }
        state
            .subagents
            .iter()
            .find(|agent| agent.id == completion.id.raw())
            .and_then(|agent| agent.workspace_root.as_ref())
            .and_then(|workspace| {
                crate::sessions::write_subagent_review_manifest(workspace, completion.id.raw())
            })
    };
    let workspace_lifecycle = match completion.status {
        crate::app::SubAgentStatus::Completed => rustcode_session::WorkspaceLifecycle::Completed,
        crate::app::SubAgentStatus::Failed => rustcode_session::WorkspaceLifecycle::Failed,
        crate::app::SubAgentStatus::Cancelled => rustcode_session::WorkspaceLifecycle::Cancelled,
        crate::app::SubAgentStatus::Queued | crate::app::SubAgentStatus::Running => {
            rustcode_session::WorkspaceLifecycle::Running
        }
        crate::app::SubAgentStatus::Interrupted => rustcode_session::WorkspaceLifecycle::Cancelled,
    };
    let workspace_path = {
        let state = state.lock().await;
        if state.active_session_id != owner_session_id {
            return;
        }
        state
            .subagents
            .iter()
            .find(|agent| agent.id == completion.id.raw())
            .and_then(|agent| agent.workspace_root.clone())
    };
    if let Some(path) = workspace_path
        && let Some(manager) = crate::sessions::workspace_manager()
        && let Ok(Some(descriptor)) = manager.find_by_workspace_path(&path)
    {
        let _ = manager.set_lifecycle(&descriptor.id, workspace_lifecycle);
    }

    let mut state = state.lock().await;
    if state.active_session_id != owner_session_id {
        return;
    }
    let mut notify_root = false;
    if let Some(agent) = state
        .subagents
        .iter_mut()
        .find(|agent| agent.id == completion.id.raw())
    {
        if completion.status != crate::app::SubAgentStatus::Completed
            && agent.history.last().map(|message| message.content.as_str())
                != Some(completion.output.as_str())
        {
            Arc::make_mut(&mut agent.history).push(ChatMessage::new("system", &completion.output));
        }
        agent.review_manifest = review_manifest;
        agent.completion = Some(completion.output.clone());
        // A child of the root reports in on its own; a nested child's parent
        // is an agent that is waiting on it.
        if agent.parent_id.is_none() {
            notify_root = true;
        }
    }
    let _ = crate::app::SubagentController.set_status(&mut state, completion.id, completion.status);
    let status = match completion.status {
        crate::app::SubAgentStatus::Completed => "completed",
        crate::app::SubAgentStatus::Failed => "failed",
        crate::app::SubAgentStatus::Cancelled => "cancelled",
        crate::app::SubAgentStatus::Queued => "queued",
        crate::app::SubAgentStatus::Interrupted => "interrupted",
        crate::app::SubAgentStatus::Running => "running",
    };
    if notify_root {
        state.subagent_supervisor.send_completion_notice(
            completion.id.raw(),
            status,
            &completion.output,
        );
    }
    crate::app::subagent_persistence::save(&state);
    agent_notice(
        &mut state,
        format!("agent-{} {status}", completion.id.raw()),
    );
}

/// Messages waiting for the agent that called `wait_agent`: the root's
/// mailbox, or the calling child's own.
async fn waiter_mail_count(
    state: &Arc<Mutex<AppState>>,
    supervisor: &crate::app::SubagentSupervisor,
    waiter: Option<u32>,
) -> usize {
    match waiter {
        None => supervisor.root_agent_message_count(),
        Some(waiter) => state
            .lock()
            .await
            .subagents
            .iter()
            .find(|agent| agent.id == waiter)
            .map_or(0, |agent| agent.mailbox.len()),
    }
}

/// How long `cancel_agent` waits for the child to reach its terminal state,
/// so a prompt stop is reported as done rather than only requested.
const CANCEL_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// Cancel the child `id` and its descendants, as `cancel_agent` and the
/// macOS app's agent panel do. An interrupted child that is no longer running
/// is marked cancelled at once.
/// What a cancel did: the reply text, and whether the child has reached its
/// terminal state (false while it is still cancelling).
pub(crate) struct CancelOutcome {
    pub(crate) message: String,
    pub(crate) settled: bool,
}

pub(crate) async fn cancel_subagent_tree(
    state: &Arc<Mutex<AppState>>,
    id: u32,
) -> Result<CancelOutcome, String> {
    let supervisor = state.lock().await.subagent_supervisor.clone();
    if !supervisor.is_active(crate::app::SubagentId::from_raw(id)) {
        let mut s = state.lock().await;
        if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == id)
            && agent.status == crate::app::SubAgentStatus::Interrupted
        {
            agent.completion = Some("error: cancelled".into());
            let _ = crate::app::SubagentController.set_status(
                &mut s,
                crate::app::SubagentId::from_raw(id),
                crate::app::SubAgentStatus::Cancelled,
            );
            crate::app::subagent_persistence::save(&s);
            return Ok(CancelOutcome {
                message: format!("subagent {id} cancelled"),
                settled: true,
            });
        }
    }
    let descendants = {
        let s = state.lock().await;
        let mut ids = vec![id];
        for _ in 0..3 {
            for agent in &s.subagents {
                if agent.parent_id.is_some_and(|parent| ids.contains(&parent))
                    && !ids.contains(&agent.id)
                {
                    ids.push(agent.id);
                }
            }
        }
        ids
    };
    for child in descendants.iter().skip(1) {
        let _ = supervisor.cancel(crate::app::SubagentId::from_raw(*child));
    }
    let target = crate::app::SubagentId::from_raw(id);
    supervisor
        .cancel(target)
        .map_err(|error| error.to_string())?;
    // Cancellation aborts the child's model request, tool call or pending
    // approval, so it normally settles at once. Report what happened.
    let settled = tokio::time::timeout(
        CANCEL_SETTLE,
        supervisor.wait_result(target, || async { false }),
    )
    .await;
    Ok(match settled {
        Ok(Ok(Some(completion))) => {
            let status = match completion.status {
                crate::app::SubAgentStatus::Cancelled => "cancelled",
                crate::app::SubAgentStatus::Completed => "completed before the cancel took effect",
                crate::app::SubAgentStatus::Failed => "failed",
                crate::app::SubAgentStatus::Interrupted => "interrupted",
                crate::app::SubAgentStatus::Queued | crate::app::SubAgentStatus::Running => {
                    "stopped"
                }
            };
            CancelOutcome {
                message: format!("subagent {id} {status}"),
                settled: true,
            }
        }
        _ => CancelOutcome {
            message: format!(
                "cancellation requested for subagent {id}; it is still cancelling after {}s. \
                 Use wait_agent for its terminal result",
                CANCEL_SETTLE.as_secs()
            ),
            settled: false,
        },
    })
}

pub(crate) fn launch_subagent_turn(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    parent_cancel: &tokio_util::sync::CancellationToken,
    supervisor: &crate::app::SubagentSupervisor,
    agent_id: u32,
    owner_session_id: String,
) -> Result<(), crate::app::SubagentError> {
    let client = client.clone();
    let child_state = Arc::clone(state);
    let completion_state = Arc::downgrade(state);
    let mcp_registry = crate::mcp::get_mcp_registry();
    let completion_owner = owner_session_id.clone();
    supervisor.forget_completion_delivered(agent_id);
    supervisor.spawn_with_token_and_completion(
        crate::app::SubagentId::from_raw(agent_id),
        parent_cancel.clone(),
        move |child_cancel| async move {
            // Marked from the child task, which may wait for the state lock;
            // the caller can hold it, so it is not taken here.
            let workspace_path = child_state
                .lock()
                .await
                .subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .and_then(|agent| agent.workspace_root.clone());
            if let Some(path) = workspace_path
                && let Some(manager) = crate::sessions::workspace_manager()
                && let Ok(Some(descriptor)) = manager.find_by_workspace_path(&path)
            {
                let _ = manager.set_lifecycle(
                    &descriptor.id,
                    rustcode_session::WorkspaceLifecycle::Running,
                );
            }
            crate::mcp::DIRECT_MCP_REGISTRY
                .scope(
                    mcp_registry,
                    run_subagent(
                        &client,
                        &child_state,
                        &child_cancel,
                        agent_id,
                        &owner_session_id,
                    ),
                )
                .await
        },
        move |completion| async move {
            if let Some(state) = completion_state.upgrade() {
                if state.lock().await.active_session_id != completion_owner {
                    return;
                }
                project_subagent_completion(&state, completion, &completion_owner).await;
            }
        },
    )
}

/// Values of `spawn_agent`'s `model` that mean "the parent's model".
const INHERIT_MODEL_ALIASES: &[&str] = &["inherit", "default", "parent", "same"];

/// How many profiles a spawn error suggests.
const SUGGESTED_PROFILES: usize = 5;

/// The profile name a new child runs on, from `spawn_agent`'s `model`.
///
/// Omitted, empty or an alias in [`INHERIT_MODEL_ALIASES`]: the parent's
/// profile, with its URL and credentials. `None` when the parent runs on an
/// endpoint no profile describes; the child then follows the session.
/// Otherwise an exact profile name, or a model id that only one profile
/// serves (the parent's profile wins when it serves it). Anything else, or
/// a profile that cannot send requests, is an error for the caller.
pub(crate) fn resolve_subagent_profile(
    profiles: &[crate::config::ModelProfile],
    parent_profile: Option<&str>,
    requested: Option<&str>,
) -> Result<Option<String>, String> {
    let requested = requested.map(str::trim).unwrap_or_default();
    let parent = parent_profile.and_then(|name| profiles.iter().find(|p| p.name == name));
    if requested.is_empty()
        || INHERIT_MODEL_ALIASES
            .iter()
            .any(|alias| alias.eq_ignore_ascii_case(requested))
    {
        return Ok(parent.map(|profile| profile.name.clone()));
    }
    let omit_hint = match parent {
        Some(parent) => format!("Omit `model` to run on your own model ({})", parent.name),
        None => "Omit `model` to run on your own model".to_owned(),
    };
    let profile = match profiles.iter().find(|p| p.name == requested) {
        Some(profile) => profile,
        None => {
            let serving = profiles
                .iter()
                .filter(|p| p.model == requested)
                .collect::<Vec<_>>();
            match (serving.as_slice(), parent) {
                (_, Some(parent)) if parent.model == requested => parent,
                ([only], _) => only,
                ([], _) => {
                    return Err(format!(
                        "error: no model profile named '{requested}'. {omit_hint}, or pass a profile name such as: {}.",
                        suggested_profiles(profiles, parent, requested).join(", ")
                    ));
                }
                (several, _) => {
                    return Err(format!(
                        "error: several model profiles serve '{requested}': {}. {omit_hint}, or pass one of these names.",
                        several
                            .iter()
                            .take(SUGGESTED_PROFILES)
                            .map(|p| p.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
    };
    if super::stream_request::is_chatgpt_credential(Some(profile))
        && !super::stream_request::chatgpt_endpoint_is_supported(profile)
    {
        return Err(format!(
            "error: model profile '{}' cannot send requests: its ChatGPT plan login needs the https://api.openai.com/v1/responses endpoint. {omit_hint}.",
            profile.name
        ));
    }
    Ok(Some(profile.name.clone()))
}

/// A few profile names for a spawn error: the parent's first, then those
/// that share words with what was asked for (`chatgpt/gpt-5-mini` suggests
/// other `chatgpt/` and `gpt` profiles), then the rest in config order.
fn suggested_profiles<'a>(
    profiles: &'a [crate::config::ModelProfile],
    parent: Option<&'a crate::config::ModelProfile>,
    requested: &str,
) -> Vec<&'a str> {
    let words = |text: &str| {
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| word.len() > 1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let wanted = words(requested);
    let shared = |profile: &crate::config::ModelProfile| {
        words(&profile.name)
            .iter()
            .filter(|word| wanted.contains(word))
            .count()
    };
    let mut ranked = profiles
        .iter()
        .filter(|profile| parent.is_none_or(|parent| parent.name != profile.name))
        .enumerate()
        .map(|(index, profile)| (std::cmp::Reverse(shared(profile)), index, profile))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(shared, index, _)| (*shared, *index));
    parent
        .into_iter()
        .chain(ranked.into_iter().map(|(_, _, profile)| profile))
        .take(SUGGESTED_PROFILES)
        .map(|profile| profile.name.as_str())
        .collect()
}

/// The profile a child's request goes out on: the one it was spawned on by
/// name, else the session's own. A child persisted before profiles were
/// stored by name may hold a model id; it still finds a profile serving it.
fn subagent_request_profile(
    s: &AppState,
    model: Option<&str>,
) -> Option<crate::config::ModelProfile> {
    let Some(model) = model else {
        return s.active_model_profile();
    };
    let profiles = &s.config.models;
    profiles
        .iter()
        .find(|p| p.name == model)
        .or_else(|| profiles.iter().find(|p| p.model == model))
        .cloned()
}

/// Where a new child works.
enum SubagentWorkspacePlan {
    /// The thread's workspace (or the given cwd), fenced by allowed_paths
    /// when the child writes.
    Shared,
    /// A named workspace from the workspace manager at an explicit base.
    Named(Box<crate::sessions::WorkspaceRequest>),
    /// A detached worktree of this repository's HEAD.
    Worktree(std::path::PathBuf),
}

impl SubagentWorkspacePlan {
    fn source(&self) -> Option<&std::path::Path> {
        match self {
            Self::Shared => None,
            Self::Named(request) => Some(&request.source_cwd),
            Self::Worktree(repo) => Some(repo),
        }
    }

    /// An isolated plan needs a git repository. Outside one the child works
    /// in the source folder instead, still fenced by allowed_paths, and the
    /// returned note says so.
    fn or_shared_outside_git(
        self,
        working_directory: Option<std::path::PathBuf>,
    ) -> (Self, Option<std::path::PathBuf>, Option<String>) {
        match self.source() {
            Some(source) if !crate::sessions::is_git_work_tree(source) => {
                let note = format!(
                    "shared: {} is not a git repository, so the agent works there directly within allowed_paths",
                    source.display()
                );
                let source = source.to_path_buf();
                (Self::Shared, Some(source), Some(note))
            }
            _ => (self, working_directory, None),
        }
    }
}

/// What a read-only child hears when a call is not inspection. It cannot
/// fix this itself, so it names who can.
const READ_ONLY_REFUSAL: &str = "error: this subagent is read-only and the call was not recognised as read-only inspection. A subagent cannot grant itself write access: report what needs changing in your answer, and the parent can spawn a worker (agent_type \"worker\" with allowed_paths) to make the change.";

fn is_read_only_subagent_call(call: &crate::tools::ToolCall) -> bool {
    if call.name != "run_command" {
        // The same set the read-only tool schema advertises: reads, web,
        // skills, and MCP tools marked read-only.
        return crate::tools::read_only_subagent_runs(&call.name)
            || crate::tools::is_read_only_call(call);
    }
    if call.name == "run_command" {
        let explicit_enabled = |key: &str| {
            call.arguments.get(key).is_some_and(|value| {
                // Only a literal false is an unambiguous opt-out. Strings and
                // other schema-coerced values must not weaken this boundary.
                value != &serde_json::Value::Bool(false)
            })
        };
        if call.arguments.get("env").is_some()
            || ["background", "detached"]
                .iter()
                .any(|key| explicit_enabled(key))
            || call.arguments.get("network_access").is_some()
            || call.arguments.get("filesystem_write_path").is_some()
            || call
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .is_some_and(crate::tools::exec::has_shell_background_operator)
        {
            return false;
        }
        // `tool --help` only prints usage, even for a tool the command
        // policy does not know.
        if call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::tools::exec::is_help_or_version_command)
        {
            return true;
        }
    }
    crate::tools::is_read_only_call(call)
}

fn bounded_completion_output(
    id: u32,
    status: &str,
    output: &str,
    truncated: bool,
    handoff: &str,
) -> (String, bool) {
    let mut summary = output.to_owned();
    let handoff = crate::tools::truncate_bytes(handoff, 1024);
    let mut incomplete = truncated;
    let mut budget = output.len();
    loop {
        let result = format!(
            "subagent {id} {status}\n{}",
            serde_json::json!({"status":status,"summary":summary,"truncated":incomplete,"handoff":handoff})
        );
        if result.len() <= 8192 {
            return (result, incomplete);
        }
        // Keep the head and the tail: a child's conclusion is usually last.
        budget /= 2;
        summary = crate::tools::truncate_bytes_head_tail(output, budget);
        incomplete = true;
    }
}

/// Resolve and validate a `spawn_agent` `cwd` the way `run_command` checks its
/// own: relative to where the caller works, inside the workspace boundary
/// unless the sandbox is trusted, and an existing directory (#2560).
fn resolve_subagent_cwd(
    state: &crate::app::AppState,
    caller_directory: Option<std::path::PathBuf>,
    raw: &str,
) -> Result<std::path::PathBuf, String> {
    let session_root = state.effective_workspace_root();
    let context = rustcode_tools::ToolContext {
        workspace_root: if state.effective_sandbox_mode().is_trusted() {
            None
        } else {
            session_root.clone()
        },
        task_working_directory: caller_directory
            .or_else(|| state.task_working_directory.clone())
            .or(session_root),
        sandbox_dir: None,
        artifacts_dir: None,
        allow_task_scope_escape: true,
    };
    let path = rustcode_tools::validate_tool_path_with_context(raw, &context, false)?;
    if !path.is_dir() {
        return Err(format!("'{}' is not a directory", path.display()));
    }
    Ok(path.canonicalize().unwrap_or(path))
}

/// The directory a child's tools run in: its own workspace when it has one,
/// else the `cwd` it was spawned with, else none (the session workspace).
/// The latest non-empty assistant text in `history`: what a child that
/// failed had worked out so far.
fn last_partial_answer(history: &[ChatMessage]) -> Option<String> {
    history
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
        .map(|message| strip_leading_think(&message.content).trim())
        .find(|text| !text.is_empty())
        .map(str::to_string)
}

fn with_partial_answer(error: String, partial: Option<String>) -> String {
    match partial {
        Some(partial) => format!("{error}\nLast partial answer:\n{partial}"),
        None => error,
    }
}

/// `error` with the child's last partial answer, so a failed child still
/// hands its parent what it found.
async fn failure_with_partial_answer(
    state: &Arc<Mutex<AppState>>,
    agent_id: u32,
    error: String,
) -> String {
    let partial = state
        .lock()
        .await
        .subagents
        .iter()
        .find(|agent| agent.id == agent_id)
        .and_then(|agent| last_partial_answer(&agent.history));
    with_partial_answer(error, partial)
}

/// Argument keys that name a file or directory a tool reads or writes.
const CONTRACT_PATH_KEYS: &[&str] = &["path", "src", "dest", "file_path", "filesystem_write_path"];

/// `raw` as an absolute-or-root-relative path with `.` and `..` resolved
/// lexically; `None` when `..` climbs out of where it starts.
fn normalize_contract_path(
    root: Option<&std::path::Path>,
    raw: &str,
) -> Option<std::path::PathBuf> {
    use std::path::Component;
    let raw = std::path::Path::new(raw.trim());
    let joined = match root {
        Some(root) if raw.is_relative() => root.join(raw),
        _ => raw.to_path_buf(),
    };
    let mut normalized = std::path::PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    return None;
                }
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    Some(normalized)
}

/// Whether any path-like argument of a call falls outside `allowed`. Paths
/// are resolved against the child's working root, so `src/../x`, absolute
/// paths and `move_file`'s `src`/`dest` are all judged by where they land.
fn call_paths_outside_contract(
    args: &serde_json::Value,
    root: Option<&std::path::Path>,
    allowed: &[String],
) -> bool {
    let allowed = allowed
        .iter()
        .filter_map(|path| normalize_contract_path(root, path))
        .collect::<Vec<_>>();
    let paths = CONTRACT_PATH_KEYS
        .iter()
        .filter_map(|key| args.get(*key).and_then(serde_json::Value::as_str))
        .chain(
            args.get("paths")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str),
        );
    paths.into_iter().any(|path| {
        normalize_contract_path(root, path)
            .is_none_or(|path| !allowed.iter().any(|allowed| path.starts_with(allowed)))
    })
}

fn subagent_execution_root(agent: &crate::app::SubAgent) -> Option<std::path::PathBuf> {
    agent
        .workspace_root
        .clone()
        .or_else(|| agent.working_directory.clone())
}

fn subagent_context_workspace_root(
    state: &crate::app::AppState,
    agent_workspace: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    agent_workspace.or_else(|| state.effective_workspace_root())
}

#[cfg(test)]
#[path = "subagents/tests.rs"]
mod tests;
