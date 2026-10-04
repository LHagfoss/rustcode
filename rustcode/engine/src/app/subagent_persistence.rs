//! Atomic, session-addressed agent tree snapshots. Runtime handles are never serialized.
use super::{AppState, ChatMessage, SubAgent, SubAgentStatus};
use std::{io::Write, path::Path, sync::Arc};

pub(crate) mod arc_history {
    use super::*;
    use serde::{Deserialize, Serialize};
    pub fn serialize<S: serde::Serializer>(
        history: &Arc<Vec<ChatMessage>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        history.as_ref().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<Vec<ChatMessage>>, D::Error> {
        Vec::<ChatMessage>::deserialize(deserializer).map(Arc::new)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AgentSnapshot {
    agents: Vec<SubAgent>,
    #[serde(default)]
    root_messages: Vec<super::subagent_controller::RootAgentMessage>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RecordedSnapshot {
    Current(AgentSnapshot),
    Legacy(Vec<SubAgent>),
}

fn snapshot_path(session_id: &str) -> Option<std::path::PathBuf> {
    if session_id.is_empty()
        || session_id.contains(['/', '\\'])
        || session_id == "."
        || session_id == ".."
    {
        return None;
    }
    let store = rustcode_session::SessionStore::new(crate::config::get_config_dir()?);
    Some(store.ensure_session(session_id).join("agents.json"))
}

pub(crate) fn save(state: &AppState) {
    let Some(path) = snapshot_path(&state.active_session_id) else {
        return;
    };
    if let Err(error) = save_at(
        &path,
        &state.subagents,
        &state.subagent_supervisor.root_messages(),
    ) {
        crate::logger::operational_event(
            "subagent.persistence_error",
            serde_json::json!({"session_id": state.active_session_id, "error": error.to_string()}),
        );
    }
}

fn save_at(
    path: &Path,
    agents: &[SubAgent],
    root_messages: &[super::subagent_controller::RootAgentMessage],
) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing parent"))?;
    std::fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(
        temporary.as_file_mut(),
        &serde_json::json!({"agents":agents,"root_messages":root_messages}),
    )?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn restore(state: &mut AppState) {
    let Some(path) = snapshot_path(&state.active_session_id) else {
        return;
    };
    let Ok(snapshot) = load_at(&path) else {
        return;
    };
    state
        .subagent_supervisor
        .restore_root_messages(snapshot.root_messages);
    let agents = snapshot.agents;
    state.next_subagent_id = agents
        .iter()
        .map(|agent| agent.id)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    state.subagents = agents;
}

fn load_at(path: &Path) -> Result<AgentSnapshot, Box<dyn std::error::Error>> {
    let recorded: RecordedSnapshot = serde_json::from_reader(std::fs::File::open(path)?)?;
    let snapshot = match recorded {
        RecordedSnapshot::Current(snapshot) => snapshot,
        RecordedSnapshot::Legacy(agents) => AgentSnapshot {
            agents,
            root_messages: Vec::new(),
        },
    };
    if snapshot.root_messages.len() > 32
        || snapshot
            .root_messages
            .iter()
            .any(|mail| mail.message.len() > 8192)
    {
        return Err("invalid root mailbox".into());
    }
    let mut agents = snapshot.agents;
    // Validate recorded ownership before exposing a tree. Never resurrect live tasks.
    let ids = agents
        .iter()
        .map(|agent| agent.id)
        .collect::<std::collections::HashSet<_>>();
    if ids.len() != agents.len() || agents.len() > 64 || ids.contains(&0) || ids.contains(&u32::MAX)
    {
        return Err("invalid agent registry".into());
    }
    for agent in &mut agents {
        if agent
            .parent_id
            .is_some_and(|parent| parent == agent.id || !ids.contains(&parent))
        {
            return Err("invalid agent parent".into());
        }
        if agent.active_turn
            || matches!(
                agent.status,
                SubAgentStatus::Running | SubAgentStatus::Queued
            )
        {
            agent.status = SubAgentStatus::Interrupted;
            agent.active_turn = false;
            agent.completion =
                Some("Interrupted by process/session restart; send a follow-up to resume".into());
        }
    }
    // Reject cycles even when every referenced ID exists.
    for agent in &agents {
        let mut parent = agent.parent_id;
        for depth in 0..=agents.len() {
            let Some(id) = parent else {
                break;
            };
            if depth == agents.len() {
                return Err("cyclic agent tree".into());
            }
            parent = agents
                .iter()
                .find(|candidate| candidate.id == id)
                .and_then(|candidate| candidate.parent_id);
        }
    }
    let parents = agents
        .iter()
        .map(|agent| (agent.id, agent.parent_id))
        .collect::<std::collections::HashMap<_, _>>();
    for agent in &mut agents {
        let mut depth = 1;
        let mut root = agent.id;
        while let Some(parent) = parents.get(&root).copied().flatten() {
            depth += 1;
            root = parent;
        }
        if depth > 3 {
            return Err("agent depth exceeds configured limit".into());
        }
        agent.depth = depth;
        agent.root_id = (depth > 1).then_some(root);
    }
    Ok(AgentSnapshot {
        agents,
        root_messages: snapshot.root_messages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_snapshot_restores_relationships_history_and_interruption() {
        let mut state = AppState::new();
        let controller = super::super::SubagentController;
        let parent = controller.spawn(
            &mut state,
            "parent",
            None,
            None,
            false,
            Vec::new(),
            None,
            None,
        );
        controller.spawn(
            &mut state,
            "child",
            None,
            Some(parent),
            false,
            Vec::new(),
            None,
            None,
        );
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("agents.json");
        save_at(
            &path,
            &state.subagents,
            &state.subagent_supervisor.root_messages(),
        )
        .unwrap();
        let restored = load_at(&path).unwrap().agents;
        assert_eq!(restored[1].parent_id, Some(parent.raw()));
        assert_eq!(restored[1].history, state.subagents[1].history);
        assert!(
            restored
                .iter()
                .all(|agent| agent.status == SubAgentStatus::Interrupted && !agent.active_turn)
        );
        state.subagents[0].parent_id = Some(state.subagents[1].id);
        save_at(
            &path,
            &state.subagents,
            &state.subagent_supervisor.root_messages(),
        )
        .unwrap();
        assert!(load_at(&path).is_err());
    }
}
