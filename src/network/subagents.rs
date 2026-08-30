use crate::app::{AppState, ChatMessage};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::loop_detect;
use super::messages::{inject_system_reminder, trim_msgs_to_budget};
use super::runner;
use super::stream_request::stream_request;
use super::text::{
    continuation_nudge_for_category, format_continuation_assistant_message, strip_leading_think,
};
use super::{
    StreamBuffer, compact_history_to_budget, confirm_and_execute, final_tool_diff,
    is_read_only_tool, push_status_line, subagent_tool_history_message,
    tool_result_precludes_preview_fallback,
};

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
            "content": message
                .content
                .split_once(": ")
                .map(|(_, rest)| rest)
                .unwrap_or(&message.content),
        });
    }
    if message.role == "tool" {
        serde_json::json!({
            "role": "user",
            "content": format!("<tool_result>\n{}\n</tool_result>", message.content),
        })
    } else {
        serde_json::json!({"role": message.role, "content": message.content})
    }
}

pub(crate) async fn run_subagent(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    agent_id: u32,
) -> Result<String, String> {
    crate::logger::operational_event("subagent.start", serde_json::json!({"agent_id": agent_id}));
    let stream_buffer = Arc::new(Mutex::new(StreamBuffer::new()));
    let mut rounds = 0usize;
    let mut loop_detector = loop_detect::LoopDetector::new(6);
    loop {
        if cancel_token.is_cancelled() {
            crate::logger::operational_event(
                "subagent.finish",
                serde_json::json!({"agent_id": agent_id, "status": "cancelled"}),
            );
            return Err("error: cancelled".to_string());
        }
        let mut history_snapshot: Vec<ChatMessage> = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|a| a.id == agent_id)
                .map(|a| a.history.as_ref().clone())
                .unwrap_or_default()
        };
        if history_snapshot.is_empty() {
            return Err(format!("error: no subagent with id {agent_id}"));
        }

        let (api_base_url, model_name, budget_token_limit, workspace_root) = {
            let s = state.lock().await;
            let subagent = s
                .subagents
                .iter()
                .find(|a| a.id == agent_id)
                .expect("Subagent not found");
            let target_model_name = subagent.model.as_deref().unwrap_or(&s.model_name);
            let profile = s
                .config
                .models
                .iter()
                .find(|p| p.name == target_model_name || p.model == target_model_name)
                .cloned();
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
            (api_base_url, model_name, budget, s.workspace_root.clone())
        };
        compact_history_to_budget(&mut history_snapshot, budget_token_limit).await;

        let protocol = { state.lock().await.active_tool_protocol() };
        let agent_mode = { state.lock().await.agent_mode };
        let delegation_contract = {
            let s = state.lock().await;
            s.subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| {
                    format!(
                        "Delegation contract: write_access={}, allowed_paths={:?}, verification_command={:?}.",
                        agent.write_access, agent.allowed_paths, agent.verification_command
                    )
                })
                .unwrap_or_else(|| "Delegation contract unavailable; remain read-only.".to_string())
        };
        let system_prompt = format!(
            "{}\n\nYou are subagent {agent_id}, working for a main agent in the same \
rustcode session. Complete the task you were given, then reply in plain text \
with NO tool call — that reply is returned to the main agent. Keep the final \
reply compact and information-dense. {delegation_contract}\n\n{}",
            crate::tools::tool_system_prompt_for_policy(
                crate::tools::ToolSchemaPolicy::subagent(),
                protocol,
                agent_mode,
            ),
            workspace_root
                .as_deref()
                .map(crate::context::environment_context_at)
                .unwrap_or_else(crate::context::environment_context)
        );
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
        dbg_log!(
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
        let request_msgs: Arc<[serde_json::Value]> = msgs.into();
        let collected = match runner::collect_response(move |previous| {
            let mut current_msgs =
                Vec::with_capacity(request_msgs.len() + if previous.is_empty() { 0 } else { 2 });
            current_msgs.extend(request_msgs.iter().cloned());
            if !previous.is_empty() {
                let continuation_assistant = format_continuation_assistant_message(&previous);
                let nudge = continuation_nudge_for_category(&previous, None);
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
            async move {
                request_buffer.lock().await.reset();
                let finish_reason = stream_request(
                    &request_client,
                    request_state,
                    request_cancel,
                    &request_api_url,
                    &request_model,
                    current_msgs,
                    Arc::clone(&request_buffer),
                    true,
                    true,
                    super::stream_request::ThinkingMode::Normal,
                    crate::tools::ToolSchemaPolicy::subagent(),
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
                let buffer = request_buffer.lock().await;
                Ok(super::runner::ResponseChunk {
                    content: buffer.content.clone(),
                    finish_reason,
                    has_native_tool_calls: !buffer.native_tool_calls.is_empty(),
                    thought_time_ms: buffer.thought_time_ms,
                    thought_tokens: buffer.thought_tokens,
                })
            }
        })
        .await
        {
            Ok(result) => result,
            Err(e) => return Err(format!("error: subagent request failed: {e}")),
        };
        let content = collected.content;
        let native_tool_calls = stream_buffer.lock().await.native_tool_calls.clone();

        if content.is_empty() && native_tool_calls.is_empty() {
            return Err("error: subagent returned an empty reply".to_string());
        }

        let protocol = { state.lock().await.active_tool_protocol() };
        let parsed_calls = if native_tool_calls.is_empty() {
            crate::tools::parse_tool_call(&content, protocol)
                .map(|call| (call, None))
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            native_tool_calls
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
                .collect::<Vec<_>>()
        };

        if !parsed_calls.is_empty() {
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

            for (index, (tool_call, call_id)) in parsed_calls.iter().enumerate() {
                let name = &tool_call.name;
                let args = &tool_call.arguments;
                if let Err(reason) =
                    crate::tools::validate_tool_calls(std::slice::from_ref(tool_call))
                {
                    let execution = crate::tools::ToolExecutionOutput::failure_with_kind(
                        format!("error: tool call rejected before execution: {reason}"),
                        crate::tools::ToolErrorKind::Validation,
                        false,
                    );
                    let message =
                        subagent_tool_history_message(name, args, execution, None, call_id.clone());
                    let mut s = state.lock().await;
                    if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                        Arc::make_mut(&mut a.history).push(message);
                    }
                    continue;
                }
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
                    return Err(format!(
                        "error: subagent {agent_id} stopped after {repeats} repeated '{name}' actions"
                    ));
                }
                rounds += 1;
                let (write_access, allowed_paths) = {
                    let s = state.lock().await;
                    s.subagents
                        .iter()
                        .find(|agent| agent.id == agent_id)
                        .map(|agent| (agent.write_access, agent.allowed_paths.clone()))
                        .unwrap_or((false, Vec::new()))
                };
                let path_outside_contract = args
                    .get("path")
                    .and_then(|value| value.as_str())
                    .is_some_and(|path| {
                        !allowed_paths.iter().any(|allowed| {
                            path == allowed
                                || path.starts_with(&format!("{}/", allowed.trim_end_matches('/')))
                        })
                    });
                let (execution, diff_opt, _user_wait) = if !write_access && !is_read_only_tool(name)
                {
                    (
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            "error: subagents are read-only by default; request write_access with allowed_paths explicitly".to_string(),
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
                } else if crate::tools::is_agent_tool(name) {
                    (
                        crate::tools::ToolExecutionOutput::failure_with_kind(
                            "error: subagents cannot spawn, message, wait on, or cancel other agents"
                                .to_string(),
                            crate::tools::ToolErrorKind::UnavailableDependency,
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
                        push_status_line(&mut s, format!("agent-{agent_id} → {name} {target}"));
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
                                .and_then(|agent| agent.workspace_root.clone())
                        },
                        None,
                    )
                    .await
                };
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
                let mut s = state.lock().await;
                if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
                    Arc::make_mut(&mut a.history).push(message);
                }
                if cancel_token.is_cancelled() {
                    let mut s = state.lock().await;
                    if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
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
                    return Err("error: cancelled".to_string());
                }
            }
            continue;
        }

        let mut s = state.lock().await;
        if let Some(a) = s.subagents.iter_mut().find(|a| a.id == agent_id) {
            Arc::make_mut(&mut a.history).push(ChatMessage::new("assistant", &content));
        }
        crate::logger::operational_event(
            "subagent.finish",
            serde_json::json!({"agent_id": agent_id, "status": "completed", "rounds": rounds}),
        );
        return Ok(strip_leading_think(&content).to_string());
    }
}

async fn project_subagent_completion(
    state: &Arc<Mutex<AppState>>,
    completion: crate::app::SubagentCompletion,
) {
    let review_manifest = {
        let state = state.lock().await;
        state
            .subagents
            .iter()
            .find(|agent| agent.id == completion.id.raw())
            .and_then(|agent| agent.workspace_root.as_ref())
            .and_then(|workspace| {
                crate::config::write_subagent_review_manifest(workspace, completion.id.raw())
            })
    };

    let mut state = state.lock().await;
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
    }
    let _ = crate::app::SubagentController.set_status(&mut state, completion.id, completion.status);
    push_status_line(
        &mut state,
        format!(
            "agent-{} {}",
            completion.id.raw(),
            match completion.status {
                crate::app::SubAgentStatus::Completed => "completed",
                crate::app::SubAgentStatus::Failed => "failed",
                crate::app::SubAgentStatus::Cancelled => "cancelled",
                crate::app::SubAgentStatus::Running => "running",
            }
        ),
    );
}

fn launch_subagent_turn(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    parent_cancel: &tokio_util::sync::CancellationToken,
    supervisor: &crate::app::SubagentSupervisor,
    agent_id: u32,
) -> Result<(), crate::app::SubagentError> {
    let client = client.clone();
    let child_state = Arc::clone(state);
    let completion_state = Arc::downgrade(state);
    supervisor.spawn_with_token_and_completion(
        crate::app::SubagentId::from_raw(agent_id),
        parent_cancel.clone(),
        move |child_cancel| async move {
            run_subagent(&client, &child_state, &child_cancel, agent_id).await
        },
        move |completion| async move {
            if let Some(state) = completion_state.upgrade() {
                project_subagent_completion(&state, completion).await;
            }
        },
    )
}

fn agent_id_arg(args: &serde_json::Value) -> Option<u32> {
    args.get("id")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .and_then(|id| u32::try_from(id).ok())
}

pub(crate) async fn handle_agent_tool(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
) -> crate::tools::ToolExecutionOutput {
    match name {
        "spawn_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(task) = args
                .get("task")
                .and_then(|t| t.as_str())
                .filter(|t| !t.trim().is_empty())
            else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'task' argument".to_string(),
                );
            };
            let model = args
                .get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());
            let write_access = args
                .get("write_access")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let allowed_paths = args
                .get("allowed_paths")
                .and_then(|value| value.as_array())
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if write_access && allowed_paths.is_empty() {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: write-enabled subagents require at least one allowed_paths entry"
                        .to_string(),
                );
            }
            let verification_command = args
                .get("verification_command")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            let verification_label = verification_command
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let (agent_id, supervisor) = {
                let mut s = state.lock().await;
                let id = s.next_subagent_id;
                let workspace_root = if write_access {
                    match crate::config::create_subagent_workspace(&s.active_session_id, id) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            return crate::tools::ToolExecutionOutput::failure(format!(
                                "error: unable to create isolated subagent workspace: {error}"
                            ));
                        }
                    }
                } else {
                    None
                };
                let id = crate::app::SubagentController
                    .spawn(
                        &mut s,
                        task,
                        model,
                        None,
                        write_access,
                        allowed_paths,
                        verification_command,
                        workspace_root,
                    )
                    .raw();
                let brief: String = task.chars().take(60).collect();
                push_status_line(
                    &mut s,
                    format!(
                        "agent-{id} spawned: {brief} (write_access={write_access}, verify={})",
                        verification_label
                    ),
                );
                (id, s.subagent_supervisor.clone())
            };
            if let Err(error) =
                launch_subagent_turn(client, state, cancel_token, &supervisor, agent_id)
            {
                set_subagent_status(state, agent_id, crate::app::SubAgentStatus::Failed).await;
                return crate::tools::ToolExecutionOutput::failure(format!(
                    "error: unable to start subagent {agent_id}: {error}"
                ));
            }
            crate::tools::ToolExecutionOutput::success(format!(
                "subagent {agent_id} started; use wait_agent to receive its terminal result"
            ))
        }
        "send_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(id) = agent_id_arg(args) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' argument".to_string(),
                );
            };
            let Some(message) = args
                .get("message")
                .and_then(|m| m.as_str())
                .filter(|m| !m.trim().is_empty())
            else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'message' argument".to_string(),
                );
            };
            let supervisor = {
                let mut s = state.lock().await;
                let Some(task) = s
                    .subagents
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.task.chars().take(40).collect::<String>())
                else {
                    let known: Vec<String> = s.subagents.iter().map(|a| a.id.to_string()).collect();
                    return crate::tools::ToolExecutionOutput::failure(if known.is_empty() {
                        "error: no subagents exist — use spawn_agent first".to_string()
                    } else {
                        format!(
                            "error: no subagent with id {id}. Known ids: {}",
                            known.join(", ")
                        )
                    });
                };
                if let Err(error) = crate::app::SubagentController.send_input(
                    &mut s,
                    crate::app::SubagentId::from_raw(id),
                    message,
                ) {
                    return crate::tools::ToolExecutionOutput::failure(format!("error: {error}"));
                }
                push_status_line(&mut s, format!("agent-{id} ← follow-up ({task})"));
                s.subagent_supervisor.clone()
            };
            if let Err(error) = launch_subagent_turn(client, state, cancel_token, &supervisor, id) {
                set_subagent_status(state, id, crate::app::SubAgentStatus::Failed).await;
                return crate::tools::ToolExecutionOutput::failure(format!(
                    "error: unable to start subagent {id} follow-up: {error}"
                ));
            }
            crate::tools::ToolExecutionOutput::success(format!(
                "subagent {id} follow-up started; use wait_agent for its terminal result"
            ))
        }
        "wait_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(id) = agent_id_arg(args) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' argument".to_string(),
                );
            };
            let supervisor = state.lock().await.subagent_supervisor.clone();
            let completion = match supervisor.wait(crate::app::SubagentId::from_raw(id)).await {
                Ok(completion) => completion,
                Err(error) => {
                    return crate::tools::ToolExecutionOutput::failure(format!("error: {error}"));
                }
            };
            let status = match completion.status {
                crate::app::SubAgentStatus::Completed => "completed",
                crate::app::SubAgentStatus::Failed => "failed",
                crate::app::SubAgentStatus::Cancelled => "cancelled",
                crate::app::SubAgentStatus::Running => "running",
            };
            let truncation = if completion.truncated {
                "\n[result truncated; full output remains in the child history/artifact]"
            } else {
                ""
            };
            let content = format!("subagent {id} {status}\n{}{truncation}", completion.output);
            if completion.status == crate::app::SubAgentStatus::Completed {
                crate::tools::ToolExecutionOutput::success(content)
            } else if completion.status == crate::app::SubAgentStatus::Cancelled {
                crate::tools::ToolExecutionOutput::failure_with_kind(
                    content,
                    crate::tools::ToolErrorKind::Cancelled,
                    false,
                )
            } else {
                crate::tools::ToolExecutionOutput::failure(content)
            }
        }
        "cancel_agent" => {
            if !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(id) = agent_id_arg(args) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' argument".to_string(),
                );
            };
            let supervisor = state.lock().await.subagent_supervisor.clone();
            match supervisor.cancel(crate::app::SubagentId::from_raw(id)) {
                Ok(()) => crate::tools::ToolExecutionOutput::success(format!(
                    "cancellation requested for subagent {id}; use wait_agent for its terminal result"
                )),
                Err(error) => crate::tools::ToolExecutionOutput::failure(format!("error: {error}")),
            }
        }
        "set_goal" => {
            let goal = args.get("goal").and_then(|g| g.as_str()).unwrap_or("");
            if goal.is_empty() {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'goal' argument".to_string(),
                );
            }
            let mut s = state.lock().await;
            s.continuous_mode = true;
            s.input_buffer.clear();
            s.cursor_position = 0;
            crate::tools::ToolExecutionOutput::success(format!(
                "Success: Goal set to '{}'. You are now in continuous autoloop mode. Continue executing tools to complete this goal, and call the 'complete_task' tool when fully done.",
                goal
            ))
        }
        "todo_write" => {
            let Some(arr) = args.get("todos").and_then(|t| t.as_array()) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing 'todos' array argument".to_string(),
                );
            };
            let mut todos = Vec::with_capacity(arr.len());
            for item in arr {
                let Some(content) = item
                    .get("content")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.trim().is_empty())
                else {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: each todo needs a non-empty 'content'".to_string(),
                    );
                };
                let status = item
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                let priority = item
                    .get("priority")
                    .and_then(|s| s.as_str())
                    .unwrap_or("medium")
                    .to_string();
                todos.push(crate::app::TodoItem {
                    content: content.to_string(),
                    status,
                    priority,
                });
            }
            let summary = format!(
                "Plan updated ({} item(s)): {}",
                todos.len(),
                todos
                    .iter()
                    .map(|t| format!("[{}] {}", t.status, t.content))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            let mut s = state.lock().await;
            s.todos = todos;
            drop(s);
            crate::tools::ToolExecutionOutput::success(summary)
        }
        _ => crate::tools::ToolExecutionOutput::failure(format!(
            "error: unknown agent tool '{name}'"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn gated_subagent_server() -> (
        String,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let _ = accepted_tx.send(());
            let _ = release_rx.await;
            let chunk = serde_json::json!({
                "choices": [{"delta": {"content": "child finished"}, "finish_reason": "stop"}]
            });
            let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        (format!("http://{address}"), accepted_rx, release_tx)
    }

    async fn delegated_test_state(url: String) -> Arc<Mutex<AppState>> {
        let mut state = AppState::new();
        state.api_base_url = url;
        state.model_name = "subagent-test-model".to_owned();
        state.delegation_active = true;
        Arc::new(Mutex::new(state))
    }

    #[test]
    fn native_subagent_history_keeps_call_and_result_ids_structured() {
        let assistant = ChatMessage::new("assistant", "").with_tool_calls(vec![
            crate::app::ToolCallRef {
                id: "call-1".to_string(),
                name: "view_file".to_string(),
                arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
            },
            crate::app::ToolCallRef {
                id: "call-2".to_string(),
                name: "grep".to_string(),
                arguments: r#"{"pattern":"TODO"}"#.to_string(),
            },
        ]);
        let result =
            ChatMessage::new("tool", "view_file: content").answering(Some("call-1".to_string()));

        let assistant_message =
            subagent_history_message(&assistant, crate::config::ToolProtocol::ApiNative);
        let result_message =
            subagent_history_message(&result, crate::config::ToolProtocol::ApiNative);

        assert_eq!(assistant_message["role"], "assistant");
        assert_eq!(assistant_message["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(assistant_message["tool_calls"][1]["id"], "call-2");
        assert_eq!(result_message["role"], "tool");
        assert_eq!(result_message["tool_call_id"], "call-1");
        assert_eq!(result_message["content"], "content");
    }

    #[tokio::test]
    async fn spawn_tool_returns_while_child_runs_and_wait_delivers_once() {
        let (url, accepted, release) = gated_subagent_server().await;
        let state = delegated_test_state(url).await;
        let cancel = tokio_util::sync::CancellationToken::new();

        let spawned = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "spawn_agent",
            &serde_json::json!({"task": "inspect the code"}),
        )
        .await;
        assert!(spawned.success);
        assert!(spawned.content.contains("subagent 1 started"));
        tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
            .await
            .unwrap()
            .unwrap();
        assert!(state.lock().await.subagents[0].active_turn);

        release.send(()).unwrap();
        let waited = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "wait_agent",
            &serde_json::json!({"id": 1}),
        )
        .await;
        let replayed = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "wait_agent",
            &serde_json::json!({"id": 1}),
        )
        .await;
        assert!(waited.success, "{}", waited.content);
        assert_eq!(waited.content, replayed.content);
        assert!(waited.content.contains("child finished"));
    }

    #[tokio::test]
    async fn running_child_rejects_send_and_cancel_tool_stops_it() {
        let (url, accepted, release) = gated_subagent_server().await;
        let state = delegated_test_state(url).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "spawn_agent",
            &serde_json::json!({"task": "keep working"}),
        )
        .await;
        tokio::time::timeout(std::time::Duration::from_secs(5), accepted)
            .await
            .unwrap()
            .unwrap();

        let sent = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "send_agent",
            &serde_json::json!({"id": 1, "message": "duplicate turn"}),
        )
        .await;
        assert!(!sent.success);
        assert!(sent.content.contains("already running"));

        let cancelled = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "cancel_agent",
            &serde_json::json!({"id": 1}),
        )
        .await;
        assert!(cancelled.success);
        let waited = handle_agent_tool(
            &reqwest::Client::new(),
            &state,
            &cancel,
            "wait_agent",
            &serde_json::json!({"id": 1}),
        )
        .await;
        assert!(!waited.success);
        assert!(waited.content.contains("cancelled"));
        let _ = release.send(());
        assert_eq!(
            state.lock().await.subagents[0].status,
            crate::app::SubAgentStatus::Cancelled
        );
    }
}
