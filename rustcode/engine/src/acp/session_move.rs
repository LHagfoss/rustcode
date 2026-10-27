//! `_rustcode/session/move` (#2459): the macOS app's "Change Project Folder"
//! for one chat. The chat's recorded workspace moves to another folder; the
//! global default and other chats keep theirs. Only a closed chat moves, so
//! no live turn, tool or task is left running against the old folder: the
//! app closes it, moves it, then loads it again in the new folder.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/move", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MoveSessionRequest {
    pub session_id: String,
    /// The folder the chat works in from now on. Absolute.
    pub cwd: std::path::PathBuf,
}

/// Record `cwd` as the chat's workspace. Extra directories it was given stay;
/// a task worktree belonged to the old folder's repository and is dropped.
pub(crate) fn move_session(
    session_id: &str,
    cwd: std::path::PathBuf,
) -> Result<Value, agent_client_protocol::Error> {
    let invalid = |message: String| agent_client_protocol::Error::invalid_params().data(message);
    if crate::config::load_session_by_id(session_id).is_none() {
        return Err(invalid(format!("unknown session: {session_id}")));
    }
    let previous = crate::config::load_session_workspace(session_id);
    let workspace = rustcode_session::SessionWorkspace {
        cwd: cwd.clone(),
        additional_directories: previous
            .map(|workspace| workspace.additional_directories)
            .unwrap_or_default(),
        task_workspace_id: None,
    };
    crate::config::save_session_workspace(session_id, &workspace)
        .map_err(|error| invalid(format!("could not record the new folder: {error}")))?;
    Ok(json!({ "cwd": cwd }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_only_the_chats_recorded_folder() {
        let store = rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let extra = std::path::PathBuf::from("/tmp/extra");
        let history = vec![
            crate::app::ChatMessage::new("user", "Move me"),
            crate::app::ChatMessage::new("assistant", "Moved"),
        ];
        for id in ["acp-move-a", "acp-move-b"] {
            store.save_session_history(id, &history);
            store
                .save_session_workspace(
                    id,
                    &rustcode_session::SessionWorkspace {
                        cwd: old.path().to_path_buf(),
                        additional_directories: vec![extra.clone()],
                        task_workspace_id: Some("task-1".into()),
                    },
                )
                .unwrap();
        }
        rustcode_session::flush_history();

        assert_eq!(
            move_session("acp-move-a", new.path().to_path_buf()).unwrap(),
            json!({ "cwd": new.path() })
        );

        let moved = crate::config::load_session_workspace("acp-move-a").unwrap();
        assert_eq!(moved.cwd, new.path());
        assert_eq!(moved.additional_directories, vec![extra]);
        assert_eq!(moved.task_workspace_id, None);
        let other = crate::config::load_session_workspace("acp-move-b").unwrap();
        assert_eq!(other.cwd, old.path());
        assert!(move_session("acp-move-missing", new.path().to_path_buf()).is_err());
    }
}
