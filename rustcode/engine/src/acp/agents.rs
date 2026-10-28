//! The session's subagents for ACP clients such as the macOS app: the list
//! the terminal's agent picker shows, a child's conversation as the session
//! updates a replay sends, and the picker's two actions, sending a child more
//! instructions and cancelling it. The client polls the list while a turn
//! runs or a child is active.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::app::{AppState, SubAgent, SubAgentStatus, SubagentId};

pub(crate) const AGENTS_CAPABILITY_VERSION: u64 = 1;

/// Bytes of a child's task and completion the list carries.
const SUMMARY_BYTES: usize = 600;

/// `_rustcode/agents`: one method for the panel's four needs, so the ACP
/// router's handler chain stays short.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/agents", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentsRequest {
    pub session_id: String,
    #[serde(flatten)]
    pub action: AgentsAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub(crate) enum AgentsAction {
    /// Every child of the session.
    List,
    /// One child's conversation.
    History { id: u32 },
    /// More instructions for a child.
    Send { id: u32, text: String },
    /// Stop a child and its descendants.
    Cancel { id: u32 },
}

/// Answer an `_rustcode/agents` request for the session's `state`.
pub(crate) async fn answer(
    state: &Arc<Mutex<AppState>>,
    cwd: &std::path::Path,
    client: &reqwest::Client,
    action: AgentsAction,
) -> Result<Value, agent_client_protocol::Error> {
    match action {
        AgentsAction::List => Ok(list(&*state.lock().await)),
        AgentsAction::History { id } => history(&*state.lock().await, id, cwd),
        AgentsAction::Send { id, text } => send(state, client, id, text).await,
        AgentsAction::Cancel { id } => cancel(state, id).await,
    }
}

fn status_name(status: SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Queued => "queued",
        SubAgentStatus::Running => "running",
        SubAgentStatus::Interrupted => "interrupted",
        SubAgentStatus::Completed => "completed",
        SubAgentStatus::Failed => "failed",
        SubAgentStatus::Cancelled => "cancelled",
    }
}

fn role_name(role: crate::app::subagent_context::AgentRole) -> &'static str {
    use crate::app::subagent_context::AgentRole;
    match role {
        AgentRole::Default => "default",
        AgentRole::Explorer => "explorer",
        AgentRole::Worker => "worker",
    }
}

fn bounded(text: &str) -> String {
    if text.len() <= SUMMARY_BYTES {
        return text.to_owned();
    }
    let end = (0..=SUMMARY_BYTES)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    format!("{}…", &text[..end])
}

/// The child's lifecycle as the client shows it: a running child whose
/// cancellation was requested but has not finished cleaning up is
/// `cancelling`, so a stop is never shown as still working or as done.
fn lifecycle_name(agent: &SubAgent, cancel_requested: bool) -> &'static str {
    if cancel_requested
        && matches!(
            agent.status,
            SubAgentStatus::Queued | SubAgentStatus::Running
        )
    {
        "cancelling"
    } else {
        status_name(agent.status)
    }
}

fn agent_json(agent: &SubAgent, supervisor: &crate::app::SubagentSupervisor) -> Value {
    let cancel_requested = supervisor.cancel_requested(SubagentId::from_raw(agent.id));
    json!({
        "id": agent.id,
        "name": agent.name,
        "task": bounded(&agent.task),
        "model": agent.model,
        "agentType": role_name(agent.agent_type),
        "status": lifecycle_name(agent, cancel_requested),
        "activeTurn": agent.active_turn,
        "parentId": agent.parent_id,
        "depth": agent.depth,
        "writeAccess": agent.write_access,
        "messageCount": agent.history.len(),
        "startedAtMs": agent.started_at_ms,
        "finishedAtMs": agent.finished_at_ms,
        "lastActivityMs": agent.last_activity_ms.or(agent.started_at_ms),
        "toolCalls": agent.performance.tool_calls,
        "completion": agent.completion.as_deref().map(bounded),
    })
}

/// Answer `_rustcode/agents`: every child of the session, oldest first.
pub(crate) fn list(state: &AppState) -> Value {
    json!({ "agents": state
        .subagents
        .iter()
        .map(|agent| agent_json(agent, &state.subagent_supervisor))
        .collect::<Vec<_>>() })
}

/// The child `id`'s conversation, replayed as the session updates a loaded
/// chat sends, so the client renders it with its own transcript.
pub(crate) fn history(
    state: &AppState,
    id: u32,
    cwd: &std::path::Path,
) -> Result<Value, agent_client_protocol::Error> {
    let agent = state
        .subagents
        .iter()
        .find(|agent| agent.id == id)
        .ok_or_else(|| unknown_agent(id))?;
    let updates = super::replay_history_updates(&agent.history, Some(cwd));
    Ok(json!({
        "agent": agent_json(agent, &state.subagent_supervisor),
        "updates": serde_json::to_value(updates).unwrap_or_else(|_| json!([])),
    }))
}

fn unknown_agent(id: u32) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(format!("unknown subagent: {id}"))
}

/// Answer `_rustcode/agents/send` as the terminal does when a child is
/// selected: the text joins the child's mailbox, and an idle child starts a
/// turn on it. Returns whether the text was queued behind a running turn.
pub(crate) async fn send(
    state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    id: u32,
    text: String,
) -> Result<Value, agent_client_protocol::Error> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(agent_client_protocol::Error::invalid_params().data("empty message"));
    }
    let agent_id = SubagentId::from_raw(id);
    let (supervisor, owner_session_id) = {
        let mut s = state.lock().await;
        if !s.subagents.iter().any(|agent| agent.id == id) {
            return Err(unknown_agent(id));
        }
        crate::app::SubagentController
            .send_input(&mut s, agent_id, text)
            .map_err(|error| {
                agent_client_protocol::Error::invalid_params().data(error.to_string())
            })?;
        crate::app::subagent_persistence::save(&s);
        if s.subagent_supervisor.is_active(agent_id) {
            return Ok(json!({ "queued": true }));
        }
        (s.subagent_supervisor.clone(), s.active_session_id.clone())
    };
    // The child outlives the request; cancelling it is `agents/cancel`.
    let cancel = tokio_util::sync::CancellationToken::new();
    if let Err(error) = crate::network::subagents::launch_subagent_turn(
        client,
        state,
        &cancel,
        &supervisor,
        id,
        owner_session_id,
    ) {
        let mut s = state.lock().await;
        let _ = crate::app::SubagentController.set_status(&mut s, agent_id, SubAgentStatus::Failed);
        return Err(agent_client_protocol::Error::internal_error().data(error.to_string()));
    }
    Ok(json!({ "queued": false }))
}

/// Answer `_rustcode/agents/cancel` as `cancel_agent` does: the child and
/// its descendants stop.
pub(crate) async fn cancel(
    state: &Arc<Mutex<AppState>>,
    id: u32,
) -> Result<Value, agent_client_protocol::Error> {
    if !state
        .lock()
        .await
        .subagents
        .iter()
        .any(|agent| agent.id == id)
    {
        return Err(unknown_agent(id));
    }
    match crate::network::subagents::cancel_subagent_tree(state, id).await {
        Ok(message) => Ok(json!({ "message": message })),
        Err(error) => Err(agent_client_protocol::Error::invalid_params().data(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(id: u32, status: SubAgentStatus) -> SubAgent {
        let mut agent: SubAgent = serde_json::from_value(json!({
            "id": id,
            "name": format!("agent-{id}"),
            "task": "find the bug",
            "model": "small",
            "history": [],
            "status": "queued",
            "active_turn": false,
            "parent_id": null,
            "write_access": false,
            "allowed_paths": [],
            "verification_command": null,
            "workspace_root": null,
            "review_manifest": null,
        }))
        .expect("subagent");
        agent.status = status;
        agent.history = Arc::new(vec![
            crate::app::ChatMessage::new("user", "find the bug"),
            crate::app::ChatMessage::new("assistant", "It is in parse()."),
        ]);
        agent
    }

    #[test]
    fn list_names_each_child_with_its_status_and_counts() {
        let mut state = AppState::new();
        assert_eq!(list(&state), json!({ "agents": [] }));
        state.subagents.push(child(1, SubAgentStatus::Running));
        state.subagents.push(child(2, SubAgentStatus::Completed));
        let listed = list(&state);
        let agents = listed["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0]["name"], "agent-1");
        assert_eq!(agents[0]["status"], "running");
        assert_eq!(agents[0]["agentType"], "default");
        assert_eq!(agents[0]["messageCount"], 2);
        assert_eq!(agents[1]["status"], "completed");
    }

    #[test]
    fn a_child_whose_cancel_was_requested_lists_as_cancelling() {
        let mut state = AppState::new();
        let mut running = child(1, SubAgentStatus::Running);
        running.last_activity_ms = Some(42);
        assert_eq!(lifecycle_name(&running, true), "cancelling");
        assert_eq!(lifecycle_name(&running, false), "running");
        // A finished child reports its terminal state even if a cancel
        // request raced its completion.
        let cancelled = child(2, SubAgentStatus::Cancelled);
        assert_eq!(lifecycle_name(&cancelled, true), "cancelled");
        state.subagents.push(running);
        let listed = list(&state);
        assert_eq!(listed["agents"][0]["lastActivityMs"], 42);
        assert_eq!(listed["agents"][0]["toolCalls"], 0);
    }

    #[test]
    fn history_replays_the_childs_conversation() {
        let mut state = AppState::new();
        state.subagents.push(child(3, SubAgentStatus::Completed));
        let answer = history(&state, 3, std::path::Path::new("/repo")).unwrap();
        let updates = answer["updates"].as_array().unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0]["sessionUpdate"], "user_message_chunk");
        assert_eq!(updates[1]["sessionUpdate"], "agent_message_chunk");
        assert_eq!(updates[1]["content"]["text"], "It is in parse().");
        assert!(history(&state, 9, std::path::Path::new("/repo")).is_err());
    }

    #[test]
    fn a_long_task_is_bounded_on_a_character_boundary() {
        let text = "é".repeat(SUMMARY_BYTES);
        let short = bounded(&text);
        assert!(short.len() <= SUMMARY_BYTES + '…'.len_utf8());
        assert!(short.ends_with('…'));
    }

    #[test]
    fn requests_name_their_action() {
        let request: AgentsRequest =
            serde_json::from_value(json!({"sessionId": "s", "action": "list"})).unwrap();
        assert_eq!(request.session_id, "s");
        assert_eq!(request.action, AgentsAction::List);
        let request: AgentsRequest = serde_json::from_value(
            json!({"sessionId": "s", "action": "send", "id": 2, "text": "t"}),
        )
        .unwrap();
        assert_eq!(
            request.action,
            AgentsAction::Send {
                id: 2,
                text: "t".into()
            }
        );
    }
}
