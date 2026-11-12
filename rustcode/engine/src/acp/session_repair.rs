//! `_rustcode/session/repair`: the app's "Repair Thread" for a thread that
//! will not open. It checks what `session/load` checks and puts right what it
//! safely can: an external harness thread whose record names another folder
//! than the thread's workspace (the "Invalid params" a move could leave
//! behind) gets the workspace's folder. A folder that no longer exists is
//! reported, not guessed: the app offers to move the thread. Only a closed
//! thread is repaired, like a move.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/session/repair", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepairSessionRequest {
    pub session_id: String,
}

/// `{cwd, fixed: [..], problems: [..]}`: the folder the thread now opens in,
/// what was put right, and what is left for the user.
pub(crate) fn repair_session(session_id: &str) -> Result<Value, agent_client_protocol::Error> {
    let invalid = |message: String| agent_client_protocol::Error::invalid_params().data(message);
    let record = crate::harness::store::load(session_id);
    let history = crate::config::load_session_by_id(session_id);
    if record.is_none() && history.is_none() {
        return Err(invalid(format!("unknown session: {session_id}")));
    }
    let workspace = crate::config::load_session_workspace(session_id);
    let mut fixed = Vec::new();
    let mut problems = Vec::new();
    // The workspace is what a move writes first, so it wins over the record.
    let cwd = workspace
        .as_ref()
        .map(|w| w.cwd.clone())
        .or_else(|| record.as_ref().map(|r| r.cwd.clone()));
    if let (Some(mut record), Some(cwd)) = (record, cwd.as_ref())
        && !same_folder(&record.cwd, cwd)
    {
        fixed.push(format!(
            "The {} record pointed at {}; it now uses {}.",
            record.harness,
            record.cwd.display(),
            cwd.display()
        ));
        record.cwd = cwd.clone();
        record.updated_at = chrono::Utc::now().to_rfc3339();
        crate::harness::store::save(session_id, &record);
    }
    match cwd.as_ref() {
        Some(cwd) if !cwd.is_dir() => problems.push(format!(
            "The folder {} no longer exists. Move the thread to another folder.",
            cwd.display()
        )),
        None => problems.push("The thread has no folder recorded.".to_owned()),
        _ => {}
    }
    if let Some((meta, _)) = &history
        && std::fs::metadata(&meta.path).is_err()
    {
        problems.push("The thread's history file cannot be read.".to_owned());
    }
    Ok(json!({ "cwd": cwd, "fixed": fixed, "problems": problems }))
}

fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(id: &str) {
        let store = rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
        store.save_session_history(
            id,
            &vec![
                crate::app::ChatMessage::new("user", "Fix me"),
                crate::app::ChatMessage::new("assistant", "Fixed"),
            ],
        );
        rustcode_session::flush_history();
    }

    #[test]
    fn points_a_stray_harness_record_at_the_threads_folder() {
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let id = "acp-repair-record";
        seed(id);
        crate::config::save_session_workspace(
            id,
            &rustcode_session::SessionWorkspace {
                cwd: new.path().to_path_buf(),
                additional_directories: vec![],
                task_workspace_id: None,
            },
        )
        .unwrap();
        crate::harness::store::save(
            id,
            &crate::harness::store::HarnessRecord::new("claude", old.path().into()),
        );

        let answer = repair_session(id).unwrap();

        assert_eq!(answer["fixed"].as_array().unwrap().len(), 1);
        assert!(answer["problems"].as_array().unwrap().is_empty());
        assert_eq!(crate::harness::store::load(id).unwrap().cwd, new.path());
        // A second pass finds nothing left to do.
        assert!(
            repair_session(id).unwrap()["fixed"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reports_a_missing_folder_and_an_unknown_thread() {
        let id = "acp-repair-gone";
        seed(id);
        let gone = tempfile::tempdir().unwrap().path().to_path_buf();
        crate::config::save_session_workspace(
            id,
            &rustcode_session::SessionWorkspace {
                cwd: gone,
                additional_directories: vec![],
                task_workspace_id: None,
            },
        )
        .unwrap();
        let answer = repair_session(id).unwrap();
        assert_eq!(answer["problems"].as_array().unwrap().len(), 1);
        assert!(repair_session("acp-repair-missing").is_err());
    }
}

#[cfg(test)]
mod opens_in_tests {
    #[test]
    fn a_recorded_folder_opens_in_the_same_folder() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = rustcode_session::strip_verbatim(&std::fs::canonicalize(dir.path()).unwrap());
        assert!(crate::acp::opens_in(dir.path(), &cwd));
        assert!(!crate::acp::opens_in(&dir.path().join("missing"), &cwd));
    }

    /// A thread recorded with the verbatim prefix opens in the plain folder
    /// the app asks for (a Windows thread that would not open).
    #[cfg(windows)]
    #[test]
    fn a_verbatim_record_opens_in_the_plain_folder() {
        let dir = tempfile::tempdir().unwrap();
        let verbatim = std::fs::canonicalize(dir.path()).unwrap();
        assert!(verbatim.to_string_lossy().starts_with(r"\\?\"));
        let plain = rustcode_session::strip_verbatim(&verbatim);
        assert!(crate::acp::opens_in(&verbatim, &plain));
        assert!(crate::acp::opens_in(&plain, &plain));
    }
}
