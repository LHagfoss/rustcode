use super::*;

pub(super) fn agent_notice(state: &mut AppState, message: String) {
    state.set_notice(message);
}

/// Longest a single `wait_agent` call may block. The parent turn's
/// cancellation still ends it at once.
const MAX_WAIT_AGENT_TIMEOUT_MS: u64 = 3_600_000;

/// Shortest a `wait_agent` call waits. A child's turn takes model rounds, so a
/// one-second wait only times out and spends a parent round per poll; a
/// finished child or a message for the waiter still ends the wait early.
pub(super) const MIN_WAIT_AGENT_TIMEOUT_MS: u64 = 30_000;

/// The targets of `wait_agent`: `ids`, or the single `id`.
pub(super) fn agent_ids_arg(args: &serde_json::Value) -> Vec<u32> {
    let parse = |value: &serde_json::Value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .and_then(|id| u32::try_from(id).ok())
    };
    let mut ids = Vec::new();
    for id in args
        .get("ids")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse)
        .chain(args.get("id").and_then(parse))
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

pub(super) fn agent_id_arg(args: &serde_json::Value) -> Option<u32> {
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
    handle_agent_tool_for_parent(client, state, cancel_token, name, args, None).await
}

pub(super) async fn handle_agent_tool_for_parent(
    client: &reqwest::Client,
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    name: &str,
    args: &serde_json::Value,
    parent_id: Option<u32>,
) -> crate::tools::ToolExecutionOutput {
    if parent_id.is_some() && matches!(name, "set_goal" | "todo_write") {
        return crate::tools::ToolExecutionOutput::failure(
            "error: child agents cannot modify root goals or plans".into(),
        );
    }
    match name {
        "list_agents" | "inspect_agent" => {
            let s = state.lock().await;
            let id = agent_id_arg(args);
            if name == "inspect_agent" && !s.subagents.iter().any(|agent| Some(agent.id) == id) {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: unknown agent id".into(),
                );
            }
            let agents = s.subagents.iter().filter(|agent| name != "inspect_agent" || Some(agent.id) == id).map(|agent| {
                let transcript = if name == "inspect_agent" { agent.history.iter().rev().take(8).map(|message| serde_json::json!({"role":message.role,"content":message.content.chars().take(768).collect::<String>()})).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>() } else { Vec::new() };
                serde_json::json!({"id":agent.id,"parent_id":agent.parent_id,"root_id":agent.root_id,"depth":agent.depth,"name":agent.name,"agent_type":agent.agent_type,"task":agent.task.chars().take(256).collect::<String>(),"status":agent.status,"model":agent.model.as_deref().unwrap_or(&s.model_name),"context_inheritance":agent.context_inheritance,"elapsed_ms":agent.finished_at_ms.unwrap_or_else(crate::app::subagent_controller::now_ms).saturating_sub(agent.created_at_ms),"pending_messages":agent.mailbox.len(),"performance":agent.performance,"transcript_tail":transcript,"completion":agent.completion})
            }).collect::<Vec<_>>();
            crate::tools::ToolExecutionOutput::success(
                serde_json::json!({"agents":agents}).to_string(),
            )
        }
        "spawn_agent" => {
            let spawn_started = std::time::Instant::now();
            if parent_id.is_none() && !state.lock().await.delegation_active {
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
            // Resolved to a profile name now, so an unusable choice fails
            // here instead of after the child reports `queued` (#2334).
            let model = {
                let s = state.lock().await;
                let parent_profile = match parent_id {
                    Some(id) => s
                        .subagents
                        .iter()
                        .find(|agent| agent.id == id)
                        .and_then(|agent| agent.model.clone()),
                    None => None,
                }
                .or_else(|| s.active_model_profile().map(|profile| profile.name));
                match resolve_subagent_profile(
                    &s.config.models,
                    parent_profile.as_deref(),
                    args.get("model").and_then(|m| m.as_str()),
                ) {
                    Ok(model) => model,
                    Err(error) => return crate::tools::ToolExecutionOutput::failure(error),
                }
            };
            // A role only presets `write_access`; the checks below apply to
            // a worker exactly as they do to an explicit `write_access`.
            let (agent_type, write_access) = match args
                .get("agent_type")
                .and_then(|value| value.as_str())
                .map(crate::app::subagent_context::AgentRole::parse)
                .transpose()
                .and_then(|role| {
                    let role = role.unwrap_or_default();
                    role.write_access(args.get("write_access").and_then(|value| value.as_bool()))
                        .map(|write_access| (role, write_access))
                }) {
                Ok(resolved) => resolved,
                Err(error) => {
                    return crate::tools::ToolExecutionOutput::failure(format!("error: {error}"));
                }
            };
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
            let workspace_mode = args
                .get("workspace_mode")
                .and_then(|value| value.as_str())
                .unwrap_or("shared");
            if !matches!(workspace_mode, "shared" | "isolated") {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: workspace_mode must be `shared` or `isolated`".to_string(),
                );
            }
            if workspace_mode == "isolated" && !write_access {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: isolated workspaces require write_access=true and allowed_paths"
                        .to_string(),
                );
            }
            let requested_cwd = args
                .get("cwd")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let verification_label = verification_command
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let (reserved_id, owner_session_id, inherited, strategy, working_directory, plan) = {
                let mut s = state.lock().await;
                let owner_session_id = s.active_session_id.clone();
                // Finished agents stay listed for inspection; only live ones
                // count toward the limit.
                let live_agents = s
                    .subagents
                    .iter()
                    .filter(|agent| {
                        agent.active_turn
                            || matches!(
                                agent.status,
                                crate::app::SubAgentStatus::Queued
                                    | crate::app::SubAgentStatus::Running
                            )
                    })
                    .count();
                if live_agents >= 64 {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: session agent limit is 64".into(),
                    );
                }
                let parent =
                    parent_id.and_then(|id| s.subagents.iter().find(|agent| agent.id == id));
                if parent_id.is_some() && parent.is_none() {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: parent agent unavailable".into(),
                    );
                }
                if parent.is_some_and(|agent| agent.depth >= 3) {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: maximum agent depth is 3".into(),
                    );
                }
                if parent.is_some_and(|agent| agent.write_access) {
                    return crate::tools::ToolExecutionOutput::failure("error: write-enabled agents cannot delegate; return their verified handoff first".into());
                }
                if parent_id.is_some() && write_access {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: nested delegation is read-only".into(),
                    );
                }
                let strategy = match crate::app::subagent_context::ContextInheritance::parse(
                    args.get("context_inheritance")
                        .and_then(|v| v.as_str())
                        .unwrap_or("minimal"),
                ) {
                    Ok(strategy) => strategy,
                    Err(error) => {
                        return crate::tools::ToolExecutionOutput::failure(format!(
                            "error: {error}"
                        ));
                    }
                };
                let evidence = args.get("evidence").and_then(|value| value.as_str());
                if evidence.is_some_and(|text| text.len() > 8192) {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: selected evidence exceeds 8192 bytes".into(),
                    );
                }
                let inherited = crate::app::subagent_context::inherit_context(
                    parent
                        .map(|agent| agent.history.as_slice())
                        .unwrap_or(&s.history),
                    task,
                    strategy,
                    evidence,
                );
                let working_directory = match requested_cwd {
                    Some(cwd) => match resolve_subagent_cwd(
                        &s,
                        parent.and_then(subagent_execution_root),
                        cwd,
                    ) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            return crate::tools::ToolExecutionOutput::failure(format!(
                                "error: invalid cwd: {error}"
                            ));
                        }
                    },
                    None => None,
                };
                // A shared child, read-only or not, works in the thread's
                // workspace (or the cwd it was given); a writer is fenced by
                // allowed_paths there rather than by a copy.
                let working_directory = working_directory.or_else(|| {
                    (write_access && workspace_mode == "shared")
                        .then(|| s.effective_workspace_root())
                        .flatten()
                });
                let id = s.next_subagent_id;
                let plan = 'plan: {
                    if workspace_mode == "isolated" {
                        let source_cwd = working_directory
                            .clone()
                            .or_else(|| s.effective_workspace_root())
                            .ok_or_else(|| "cannot determine source workspace".to_string());
                        let Ok(source_cwd) = source_cwd else {
                            return crate::tools::ToolExecutionOutput::failure(
                                "error: cannot determine source workspace".to_string(),
                            );
                        };
                        // Without a base, the child gets a worktree of the
                        // workspace's current HEAD.
                        let Some(base_sha) = args
                            .get("base_sha")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.trim().is_empty())
                        else {
                            break 'plan SubagentWorkspacePlan::Worktree(source_cwd);
                        };
                        let name = args
                            .get("workspace_name")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or(&format!("agent-{id}"))
                            .to_string();
                        let task_id = args
                            .get("task_id")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or(&id.to_string())
                            .to_string();
                        let mut request = crate::sessions::WorkspaceRequest::for_task(
                            source_cwd,
                            name,
                            base_sha,
                            format!("session:{}", s.active_session_id),
                            s.active_session_id.clone(),
                            task_id,
                        );
                        request.branch = args
                            .get("branch")
                            .and_then(|value| value.as_str())
                            .map(str::to_string);
                        SubagentWorkspacePlan::Named(Box::new(request))
                    } else {
                        SubagentWorkspacePlan::Shared
                    }
                };
                let reserved_id = crate::app::SubagentController.reserve_id(&mut s);
                debug_assert_eq!(reserved_id.raw(), id);
                (
                    reserved_id,
                    owner_session_id,
                    inherited,
                    strategy,
                    working_directory,
                    plan,
                )
            };
            // Creating a workspace runs git and copies files; do it without
            // the state lock so the session stays responsive meanwhile.
            let (plan, working_directory, workspace_note) =
                plan.or_shared_outside_git(working_directory);
            let workspace_root = match plan {
                SubagentWorkspacePlan::Named(request) => match crate::sessions::workspace_manager()
                    .ok_or_else(|| "workspace persistence unavailable".to_string())
                    .and_then(|manager| manager.create(&request).map_err(|error| error.to_string()))
                {
                    Ok(descriptor) => Some(descriptor.workspace_path),
                    Err(error) => {
                        return crate::tools::ToolExecutionOutput::failure(format!(
                            "error: unable to create named isolated workspace: {error}"
                        ));
                    }
                },
                SubagentWorkspacePlan::Worktree(repo) => {
                    match crate::sessions::create_subagent_workspace(
                        &owner_session_id,
                        reserved_id.raw(),
                        &repo,
                    ) {
                        Ok(path) => Some(path),
                        Err(error) => {
                            return crate::tools::ToolExecutionOutput::failure(format!(
                                "error: unable to create isolated subagent workspace: {error}"
                            ));
                        }
                    }
                }
                SubagentWorkspacePlan::Shared => None,
            };
            let (agent_id, supervisor, owner_session_id, child_model) = {
                let mut s = state.lock().await;
                if s.active_session_id != owner_session_id {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: the session changed while the agent was being set up".into(),
                    );
                }
                let id = crate::app::SubagentController
                    .spawn_reserved(
                        &mut s,
                        reserved_id,
                        task,
                        model,
                        parent_id.map(crate::app::SubagentId::from_raw),
                        write_access,
                        allowed_paths,
                        verification_command,
                        workspace_root,
                    )
                    .raw();
                if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == id) {
                    agent.history = Arc::new(inherited);
                    agent.context_inheritance = strategy;
                    agent.agent_type = agent_type;
                    if agent.workspace_root.is_none() {
                        agent.working_directory = working_directory;
                    }
                    agent.status = crate::app::SubAgentStatus::Queued;
                }
                crate::app::subagent_persistence::save(&s);
                crate::logger::operational_event(
                    "subagent.spawn",
                    serde_json::json!({"agent_id":id,"session_id":owner_session_id,"parent_id":parent_id,"agent_type":agent_type,"context_inheritance":strategy,"inherited_messages":s.subagents.last().map(|agent|agent.history.len()).unwrap_or(0)}),
                );
                let brief: String = task.chars().take(60).collect();
                agent_notice(
                    &mut s,
                    format!(
                        "agent-{id} spawned: {brief} (write_access={write_access}, workspace_mode={workspace_mode}, verify={})",
                        verification_label
                    ),
                );
                let child_model = s
                    .subagents
                    .iter()
                    .find(|agent| agent.id == id)
                    .and_then(|agent| agent.model.clone())
                    .unwrap_or_else(|| s.model_name.clone());
                (
                    id,
                    s.subagent_supervisor.clone(),
                    owner_session_id,
                    child_model,
                )
            };
            if let Err(error) = launch_subagent_turn(
                client,
                state,
                cancel_token,
                &supervisor,
                agent_id,
                owner_session_id,
            ) {
                set_subagent_status(state, agent_id, crate::app::SubAgentStatus::Failed).await;
                return crate::tools::ToolExecutionOutput::failure(format!(
                    "error: unable to start subagent {agent_id}: {error}"
                ));
            }
            crate::logger::operational_event(
                "subagent.spawn.finish",
                serde_json::json!({"agent_id":agent_id,"spawn_us":crate::benchmark::elapsed_us(spawn_started)}),
            );
            // The id is what every other agent tool takes; the nickname is the
            // name the child goes by in notices and the agent picker.
            let status = state
                .lock()
                .await
                .subagents
                .iter()
                .find(|agent| agent.id == agent_id)
                .map(|agent| agent.status)
                .unwrap_or(crate::app::SubAgentStatus::Queued);
            let mut receipt = serde_json::json!({
                "agent_id": agent_id,
                "nickname": format!("agent-{agent_id}"),
                "agent_type": agent_type,
                "model": child_model,
                "status": status,
            });
            if let Some(note) = workspace_note {
                receipt["workspace"] = serde_json::Value::String(note);
            }
            crate::tools::ToolExecutionOutput::success(receipt.to_string())
        }
        "send_agent" | "send_message" | "followup_task" => {
            if parent_id.is_none() && !state.lock().await.delegation_active {
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
            if id == 0 {
                if name != "send_message" || parent_id.is_none() {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: root accepts queue-only messages from child agents".into(),
                    );
                }
                let s = state.lock().await;
                if let Err(error) = s
                    .subagent_supervisor
                    .send_root_message(parent_id.unwrap(), message.to_owned())
                {
                    return crate::tools::ToolExecutionOutput::failure(format!("error: {error}"));
                }
                crate::app::subagent_persistence::save(&s);
                return crate::tools::ToolExecutionOutput::success(
                    "message queued for main agent at its next safe boundary".into(),
                );
            }
            let interrupt = args
                .get("interrupt")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if interrupt && name == "send_message" {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: send_message only queues; use followup_task with interrupt".into(),
                );
            }
            // Checked before the turn is stopped, so a message that cannot be
            // delivered never costs the child its work.
            if interrupt && message.len() > 8192 {
                return crate::tools::ToolExecutionOutput::failure(format!(
                    "error: {}",
                    crate::app::SubagentError::MailboxFull(crate::app::SubagentId::from_raw(id))
                ));
            }
            let interrupted = if interrupt {
                match interrupt_subagent_turn(state, cancel_token, id, parent_id).await {
                    Ok(interrupted) => interrupted,
                    Err(error) => {
                        return crate::tools::ToolExecutionOutput::failure(format!(
                            "error: {error}"
                        ));
                    }
                }
            } else {
                false
            };
            let (supervisor, owner_session_id) = {
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
                let active = s
                    .subagents
                    .iter()
                    .find(|agent| agent.id == id)
                    .is_some_and(|agent| agent.active_turn);
                if name == "send_message" && !active {
                    if s.subagent_supervisor
                        .is_active(crate::app::SubagentId::from_raw(id))
                    {
                        return crate::tools::ToolExecutionOutput::failure(
                            "error: child is finishing; retry after its terminal result".into(),
                        );
                    }
                    let agent = s
                        .subagents
                        .iter_mut()
                        .find(|agent| agent.id == id)
                        .expect("checked above");
                    if agent.mailbox.len() >= 32 || message.len() > 8192 {
                        return crate::tools::ToolExecutionOutput::failure(
                            "error: mailbox limit reached".into(),
                        );
                    }
                    agent.mailbox.push_back(message.to_owned());
                    s.subagent_supervisor.notify_activity();
                    crate::app::subagent_persistence::save(&s);
                    return crate::tools::ToolExecutionOutput::success(format!(
                        "message queued for subagent {id}; idle agent remains idle"
                    ));
                }
                if let Err(error) = crate::app::SubagentController.send_input(
                    &mut s,
                    crate::app::SubagentId::from_raw(id),
                    message,
                ) {
                    return crate::tools::ToolExecutionOutput::failure(format!("error: {error}"));
                }
                crate::app::subagent_persistence::save(&s);
                if active {
                    return crate::tools::ToolExecutionOutput::success(format!(
                        "message queued for active subagent {id}"
                    ));
                }
                agent_notice(&mut s, format!("agent-{id} ← follow-up ({task})"));
                (s.subagent_supervisor.clone(), s.active_session_id.clone())
            };
            if let Err(error) = launch_subagent_turn(
                client,
                state,
                cancel_token,
                &supervisor,
                id,
                owner_session_id,
            ) {
                set_subagent_status(state, id, crate::app::SubAgentStatus::Failed).await;
                return crate::tools::ToolExecutionOutput::failure(format!(
                    "error: unable to start subagent {id} follow-up: {error}"
                ));
            }
            crate::tools::ToolExecutionOutput::success(if interrupted {
                format!(
                    "subagent {id} interrupted and redirected; use wait_agent for its terminal result"
                )
            } else {
                format!("subagent {id} follow-up started; use wait_agent for its terminal result")
            })
        }
        "wait_agent" => {
            if parent_id.is_none() && !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let ids = agent_ids_arg(args);
            if ids.is_empty() {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' or 'ids' argument".to_string(),
                );
            }
            let supervisor = state.lock().await.subagent_supervisor.clone();
            let timeout_ms = args
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(60_000)
                .clamp(MIN_WAIT_AGENT_TIMEOUT_MS, MAX_WAIT_AGENT_TIMEOUT_MS);
            // Several targets: wait for whichever finishes first, then report
            // that one exactly as a single-target wait would.
            let id = if let [id] = ids[..] {
                id
            } else {
                let (known, restored) = {
                    let s = state.lock().await;
                    let mut ancestors = Vec::new();
                    let mut ancestor = parent_id;
                    while let Some(candidate) = ancestor {
                        ancestors.push(candidate);
                        ancestor = s
                            .subagents
                            .iter()
                            .find(|agent| agent.id == candidate)
                            .and_then(|agent| agent.parent_id);
                    }
                    if ids.iter().any(|id| ancestors.contains(id)) {
                        return crate::tools::ToolExecutionOutput::failure(
                            "error: an agent cannot wait on itself or an ancestor".into(),
                        );
                    }
                    (
                        ids.iter()
                            .all(|id| s.subagents.iter().any(|agent| agent.id == *id)),
                        // A result restored with the session lives on the
                        // agent record, not in the supervisor.
                        ids.iter().copied().find(|id| {
                            !supervisor.is_active(crate::app::SubagentId::from_raw(*id))
                                && s.subagents
                                    .iter()
                                    .any(|agent| agent.id == *id && agent.completion.is_some())
                        }),
                    )
                };
                if !known {
                    return crate::tools::ToolExecutionOutput::failure(
                        "error: unknown agent id in 'ids'".into(),
                    );
                }
                if let Some(id) = restored {
                    id
                } else {
                    let targets = ids
                        .iter()
                        .map(|id| crate::app::SubagentId::from_raw(*id))
                        .collect::<Vec<_>>();
                    let first = supervisor
                        .yield_while_waiting(parent_id.map(crate::app::SubagentId::from_raw), async {
                            tokio::select! {
                                result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), supervisor.wait_any(&targets)) => result.ok(),
                                _ = cancel_token.cancelled() => Some(Err(crate::app::SubagentError::WaitCancelled(targets[0]))),
                            }
                        })
                        .await;
                    match first {
                        Some(Ok(id)) => id.raw(),
                        Some(Err(error)) => {
                            return crate::tools::ToolExecutionOutput::failure(format!(
                                "error: {error}"
                            ));
                        }
                        None => {
                            return crate::tools::ToolExecutionOutput::success(format!(
                                "none of subagents {ids:?} finished before the wait timed out; their tasks remain available"
                            ));
                        }
                    }
                }
            };
            if !supervisor.is_active(crate::app::SubagentId::from_raw(id)) {
                let s = state.lock().await;
                if let Some(agent) = s.subagents.iter().find(|agent| agent.id == id)
                    && !supervisor.has_result(crate::app::SubagentId::from_raw(id))
                    && let Some(summary) = &agent.completion
                {
                    let status = match agent.status {
                        crate::app::SubAgentStatus::Completed => "completed",
                        crate::app::SubAgentStatus::Failed => "failed",
                        crate::app::SubAgentStatus::Cancelled => "cancelled",
                        crate::app::SubAgentStatus::Interrupted => "interrupted",
                        crate::app::SubAgentStatus::Queued => "queued",
                        crate::app::SubAgentStatus::Running => "running",
                    };
                    let (content, truncated) = bounded_completion_output(
                        id,
                        status,
                        summary,
                        false,
                        "restored session result",
                    );
                    let mut result = if agent.status == crate::app::SubAgentStatus::Completed {
                        crate::tools::ToolExecutionOutput::success(content)
                    } else {
                        crate::tools::ToolExecutionOutput::failure(content)
                    };
                    if truncated {
                        result.truncated = true;
                        result.completeness = rustcode_core::ToolResultCompleteness::ByteTruncated;
                    }
                    return result;
                }
            }
            if let Some(caller) = parent_id {
                let s = state.lock().await;
                let mut ancestor = Some(caller);
                while let Some(candidate) = ancestor {
                    if candidate == id {
                        return crate::tools::ToolExecutionOutput::failure(
                            "error: an agent cannot wait on itself or an ancestor".into(),
                        );
                    }
                    ancestor = s
                        .subagents
                        .iter()
                        .find(|agent| agent.id == candidate)
                        .and_then(|agent| agent.parent_id);
                }
            }
            // Other children's activity wakes the wait; only the result, a
            // new message for the waiter, the timeout or a cancel end it
            // (#2559). Before, any wake-up returned without the result.
            let mail_before = waiter_mail_count(state, &supervisor, parent_id).await;
            let wait = supervisor.yield_while_waiting(parent_id.map(crate::app::SubagentId::from_raw), async {
                let result = supervisor.wait_result(crate::app::SubagentId::from_raw(id), || async {
                    waiter_mail_count(state, &supervisor, parent_id).await > mail_before
                });
                tokio::select! {
                    result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), result) => result,
                    _ = cancel_token.cancelled() => Ok(Err(crate::app::SubagentError::WaitCancelled(crate::app::SubagentId::from_raw(id)))),
                }
            }).await;
            let completion = match wait {
                Ok(result) => match result {
                    Ok(Some(completion)) => completion,
                    Ok(None) => {
                        return crate::tools::ToolExecutionOutput::success(format!(
                            "a message for you arrived while waiting; subagent {id} is still working. Read the message, then call wait_agent again for its result"
                        ));
                    }
                    Err(error) => {
                        return crate::tools::ToolExecutionOutput::failure(format!(
                            "error: {error}"
                        ));
                    }
                },
                Err(_) => {
                    return crate::tools::ToolExecutionOutput::success(format!(
                        "subagent {id} wait timed out; task remains available"
                    ));
                }
            };
            if parent_id.is_none() {
                supervisor.mark_completion_delivered(id);
            }
            let status = match completion.status {
                crate::app::SubAgentStatus::Completed => "completed",
                crate::app::SubAgentStatus::Failed => "failed",
                crate::app::SubAgentStatus::Cancelled => "cancelled",
                crate::app::SubAgentStatus::Queued => "queued",
                crate::app::SubAgentStatus::Interrupted => "interrupted",
                crate::app::SubAgentStatus::Running => "running",
            };
            let handoff = {
                let workspace = state
                    .lock()
                    .await
                    .subagents
                    .iter()
                    .find(|agent| agent.id == id)
                    .and_then(|agent| agent.workspace_root.clone());
                workspace.and_then(|path| {
                    crate::sessions::workspace_manager().and_then(|manager| {
                        manager.handoff_for_workspace_path(&path).ok().flatten()
                    })
                })
            };
            let handoff = handoff
                .map(|value| format!("\n\n{value}"))
                .unwrap_or_default();
            let (content, output_truncated) = bounded_completion_output(
                id,
                status,
                &completion.output,
                completion.truncated,
                &handoff,
            );
            if completion.status == crate::app::SubAgentStatus::Completed {
                let mut output = crate::tools::ToolExecutionOutput::success(content);
                if output_truncated {
                    output.truncated = true;
                    output.completeness = rustcode_core::ToolResultCompleteness::ByteTruncated;
                    output.error_kind = Some(crate::tools::ToolErrorKind::OutputLimit);
                }
                output
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
            if parent_id.is_none() && !state.lock().await.delegation_active {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: subagents are disabled for this task. Run /delegate before starting the task.".to_string(),
                );
            }
            let Some(id) = agent_id_arg(args) else {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: missing or invalid 'id' argument".to_string(),
                );
            };
            if parent_id == Some(id) {
                return crate::tools::ToolExecutionOutput::failure(
                    "error: an agent cannot cancel itself".into(),
                );
            }
            match cancel_subagent_tree(state, id).await {
                Ok(outcome) => crate::tools::ToolExecutionOutput::success(outcome.message),
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
            let mut s = state.lock().await;
            let todos = match crate::app::apply_todo_write(&s.todos, &args) {
                Ok(todos) => todos,
                Err(error) => return crate::tools::ToolExecutionOutput::failure(error),
            };
            let summary = format!(
                "Plan updated ({} item(s)): {}",
                todos.len(),
                todos
                    .iter()
                    .map(|t| format!("{} [{}] {}", t.id, t.status, t.content))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            s.todos = todos;
            drop(s);
            crate::tools::ToolExecutionOutput::success(summary)
        }
        _ => crate::tools::ToolExecutionOutput::failure(format!(
            "error: unknown agent tool '{name}'"
        )),
    }
}

/// Stop a child's running turn so a follow-up can redirect it, and return
/// once that turn has cleaned up. Children it started are cancelled with it,
/// as they are with any parent turn. A child that is idle or still queued is
/// left alone (`Ok(false)`): the ordinary follow-up already reaches it before
/// its next request.
async fn interrupt_subagent_turn(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &tokio_util::sync::CancellationToken,
    id: u32,
    caller: Option<u32>,
) -> Result<bool, String> {
    let target = crate::app::SubagentId::from_raw(id);
    let supervisor = {
        let s = state.lock().await;
        // Stopping an ancestor would stop the caller that waits for it.
        let mut ancestor = caller;
        while let Some(candidate) = ancestor {
            if candidate == id {
                return Err("an agent cannot interrupt itself or an ancestor".into());
            }
            ancestor = s
                .subagents
                .iter()
                .find(|agent| agent.id == candidate)
                .and_then(|agent| agent.parent_id);
        }
        let running = s
            .subagents
            .iter()
            .any(|agent| agent.id == id && agent.status != crate::app::SubAgentStatus::Queued);
        if !running || !s.subagent_supervisor.is_active(target) {
            return Ok(false);
        }
        s.subagent_supervisor.clone()
    };
    if supervisor.cancel(target).is_err() {
        return Ok(false);
    }
    let completion = tokio::select! {
        result = supervisor.wait(target) => result.map_err(|error| error.to_string())?,
        _ = cancel_token.cancelled() => {
            return Err(crate::app::SubagentError::WaitCancelled(target).to_string());
        }
    };
    // The turn finished by itself first; its result stands and the follow-up
    // proceeds as it would for any finished child.
    if completion.status != crate::app::SubAgentStatus::Cancelled {
        return Ok(false);
    }
    let mut s = state.lock().await;
    // The caller asked for this stop, so it is neither news for the root nor
    // a result the child should read back before its new instructions.
    supervisor.mark_completion_delivered(id);
    if let Some(agent) = s.subagents.iter_mut().find(|agent| agent.id == id)
        && agent
            .history
            .last()
            .is_some_and(|last| last.role == "system" && last.content == completion.output)
    {
        Arc::make_mut(&mut agent.history).pop();
    }
    let _ = crate::app::SubagentController.set_status(
        &mut s,
        target,
        crate::app::SubAgentStatus::Interrupted,
    );
    Ok(true)
}
