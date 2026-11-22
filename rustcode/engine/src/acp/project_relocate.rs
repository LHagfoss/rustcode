//! Move a closed project's folder and its recorded paths, preserving transcripts.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/project/relocate", response = serde_json::Value)]
pub(crate) struct RelocateRequest {
    pub path: String,
    pub destination: String,
}

fn remap(value: &mut Value, old: &Path, new: &Path) -> bool {
    let Some(path) = value.as_str() else {
        return false;
    };
    let path = rustcode_session::strip_verbatim(Path::new(path));
    let Ok(rest) = path.strip_prefix(old) else {
        return false;
    };
    *value = json!(if rest.as_os_str().is_empty() {
        new.to_path_buf()
    } else {
        new.join(rest)
    });
    true
}

/// Only path-bearing records are edited; history, ids and native resume ids stay intact.
fn rewrite(record: &mut Value, old: &Path, new: &Path) -> bool {
    let mut changed = false;
    for field in ["cwd", "repo"] {
        if let Some(value) = record.get_mut(field) {
            changed |= remap(value, old, new);
        }
    }
    if let Some(dirs) = record
        .get_mut("additional_directories")
        .and_then(Value::as_array_mut)
    {
        for dir in dirs {
            changed |= remap(dir, old, new);
        }
    }
    changed
}

fn rewrite_projects(record: &mut Value, old: &Path, new: &Path) -> bool {
    let Some(projects) = record.get_mut("projects").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut changed = false;
    let mut moved = serde_json::Map::new();
    for (key, value) in std::mem::take(projects) {
        let mut path = json!(key);
        changed |= remap(&mut path, old, new);
        moved.insert(path.as_str().unwrap_or(&key).to_owned(), value);
    }
    *projects = moved;
    changed
}

// Include empty threads: session_ids() enumerates transcripts, but a freshly
// created thread already has metadata/workspace before its first history write.
fn record_ids() -> Result<Vec<String>, String> {
    let config = crate::config::get_config_dir().ok_or("thread storage unavailable")?;
    let mut ids = crate::sessions::session_ids();
    let mut pending = vec![(rustcode_session::threads_dir(&config), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot enumerate threads: {e}")),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.join("metadata.json").is_file() && rustcode_session::valid_session_id(&name) {
                ids.push(name);
            } else if depth < 3 {
                pending.push((path, depth + 1));
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

pub(crate) fn relocate(
    request: RelocateRequest,
    is_open: impl Fn(&str) -> bool,
) -> Result<Value, String> {
    let old = rustcode_session::strip_verbatim(
        &std::fs::canonicalize(&request.path).map_err(|e| e.to_string())?,
    );
    if !Path::new(&request.path).is_absolute() || !old.is_dir() || old.parent().is_none() {
        return Err("path must be an absolute project folder, not a filesystem root".into());
    }
    if crate::config::get_config_dir()
        .and_then(|dir| std::fs::canonicalize(dir).ok())
        .is_some_and(|dir| rustcode_session::strip_verbatim(&dir).starts_with(&old))
    {
        return Err("cannot move a folder containing RustCode thread storage".into());
    }
    let asked = PathBuf::from(&request.destination);
    if !asked.is_absolute() || asked.file_name().is_none() {
        return Err("destination must be an absolute new folder path".into());
    }
    let parent = std::fs::canonicalize(asked.parent().ok_or("destination needs a parent")?)
        .map_err(|e| format!("destination parent: {e}"))?;
    let new = rustcode_session::strip_verbatim(
        &parent.join(asked.file_name().ok_or("destination needs a folder name")?),
    );
    if new.starts_with(&old) || std::fs::symlink_metadata(&new).is_ok() {
        return Err("destination must not exist or be inside the project".into());
    }
    // A linked worktree's gitfile, and a main checkout's linked worktrees, contain
    // absolute paths that a plain directory rename cannot safely repair.
    if std::fs::symlink_metadata(old.join(".git")).is_ok_and(|m| m.file_type().is_symlink())
        || old.join(".git").is_file()
        || old.join(".git/worktrees").exists()
        || old.join(".git/modules").exists()
        || old.join(".git/objects/info/alternates").exists()
    {
        return Err(
            "relocation of linked worktrees, submodules or shared git objects is not supported"
                .into(),
        );
    }
    if let Some(main) = crate::worktrees::main_checkout(&old) {
        if std::fs::canonicalize(main)
            .ok()
            .map(|path| rustcode_session::strip_verbatim(&path))
            .as_ref()
            != Some(&old)
        {
            return Err("move the repository root, not a folder inside it".into());
        }
    }
    if crate::worktrees::git(&old, &["rev-parse", "--is-bare-repository"])
        .is_ok_and(|answer| answer.trim() == "true")
        || crate::worktrees::git(&old, &["config", "--get", "core.worktree"]).is_ok()
    {
        return Err("bare repositories and custom git worktree paths cannot be relocated".into());
    }
    let mut edits = Vec::new();
    let mut ids = Vec::new();
    for id in record_ids()? {
        let dir =
            crate::sessions::get_active_session_dir(&id).ok_or("thread storage unavailable")?;
        let mut affected = false;
        let mut unreadable = Vec::new();
        for name in ["workspace.json", "metadata.json", "harness.json"] {
            let path = dir.join(name);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
            };
            let mut record: Value = match serde_json::from_slice(&bytes) {
                Ok(record) => record,
                Err(e) => {
                    unreadable.push(format!("cannot read {}: {e}", path.display()));
                    continue;
                }
            };
            if rewrite(&mut record, &old, &new) {
                affected = true;
                edits.push((
                    path,
                    bytes,
                    serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
                ));
            }
        }
        if affected {
            if !unreadable.is_empty() {
                return Err(unreadable.join("; "));
            }
            if is_open(&id) {
                return Err(format!("close thread {id} before moving its project"));
            }
            ids.push(id);
        }
    }
    // Per-project permissions and defaults live outside the folder, alongside
    // thread storage. Include that file in the same rollback set.
    let settings = crate::config::get_config_dir()
        .ok_or("thread storage unavailable")?
        .join("settings.json");
    match std::fs::read(&settings) {
        Ok(bytes) => {
            let mut record: Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("cannot read settings: {e}"))?;
            if rewrite_projects(&mut record, &old, &new) {
                edits.push((
                    settings,
                    bytes,
                    serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot read settings: {e}")),
    }
    commit_move(&old, &new, &edits, atomic_write)?;
    Ok(json!({ "path": new, "previous_path": old, "moved": true, "thread_ids": ids }))
}

type RecordEdit = (PathBuf, Vec<u8>, Vec<u8>);

fn commit_move(
    old: &Path,
    new: &Path,
    edits: &[RecordEdit],
    mut write: impl FnMut(&Path, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    std::fs::rename(old, new)
        .map_err(|e| format!("folder move failed (moves must stay on one filesystem): {e}"))?;
    for (index, (path, _, updated)) in edits.iter().enumerate() {
        if let Err(error) = write(path, updated) {
            let mut failures = Vec::new();
            for (path, original, _) in edits[..index].iter().rev() {
                if let Err(e) = write(path, original) {
                    failures.push(e.to_string());
                }
            }
            if let Err(e) = std::fs::rename(new, old) {
                failures.push(e.to_string());
            }
            return Err(format!(
                "could not update thread paths: {error}; rollback errors: {failures:?}"
            ));
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("relocate.tmp");
    std::fs::write(&temp, bytes)?;
    let result = std::fs::rename(&temp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remaps_paths_at_component_boundaries_without_touching_history() {
        let mut record = json!({"cwd":"/code/app/src", "repo":"/code/app", "additional_directories":["/code/app/docs","/code/application"], "native_session_id":"native", "history":"/code/app"});
        assert!(rewrite(
            &mut record,
            Path::new("/code/app"),
            Path::new("/new/app")
        ));
        assert_eq!(record["cwd"], json!(Path::new("/new/app").join("src")));
        assert_eq!(record["repo"], json!(Path::new("/new/app")));
        assert_eq!(
            record["additional_directories"],
            json!([Path::new("/new/app").join("docs"), "/code/application"])
        );
        assert_eq!(record["native_session_id"], "native");
        assert_eq!(record["history"], "/code/app");
    }
    #[test]
    fn moves_a_folder_and_rejects_an_occupied_destination() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("file"), "kept").unwrap();
        let request = || RelocateRequest {
            path: old.to_string_lossy().into(),
            destination: new.to_string_lossy().into(),
        };
        std::fs::create_dir(&new).unwrap();
        assert!(relocate(request(), |_| false).is_err());
        assert!(old.join("file").exists());
        std::fs::remove_dir(&new).unwrap();
        relocate(request(), |_| false).unwrap();
        assert_eq!(std::fs::read_to_string(new.join("file")).unwrap(), "kept");
        assert!(!old.exists());
    }
    #[test]
    fn closed_threads_keep_their_identity_history_and_harness_record() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        std::fs::create_dir(&old).unwrap();
        let old = rustcode_session::strip_verbatim(&std::fs::canonicalize(old).unwrap());
        let id = crate::sessions::create_detached_session();
        let workspace = rustcode_session::SessionWorkspace {
            cwd: old.clone(),
            additional_directories: vec![old.join("docs")],
            task_workspace_id: None,
        };
        crate::sessions::save_session_workspace(&id, &workspace).unwrap();
        crate::sessions::save_session_repo(&id, Some(&old));
        let mut harness = crate::harness::store::HarnessRecord::new("codex", old.clone());
        harness.native_session_id = Some("native-kept".into());
        crate::harness::store::save(&id, &harness);
        assert!(record_ids().unwrap().contains(&id));
        let history_path = crate::sessions::get_active_session_dir(&id)
            .unwrap()
            .join("history.json");
        let history = serde_json::to_vec(&vec![rustcode_core::ChatMessage::new(
            "user",
            "Keep this history",
        )])
        .unwrap();
        std::fs::write(&history_path, &history).unwrap();
        let request = || RelocateRequest {
            path: old.to_string_lossy().into(),
            destination: new.to_string_lossy().into(),
        };
        assert!(relocate(request(), |thread| thread == id).is_err());
        assert!(old.exists());
        assert!(!new.exists());
        assert_eq!(
            crate::sessions::load_session_workspace(&id).unwrap().cwd,
            old
        );
        let answer = relocate(request(), |_| false).unwrap();
        let new = rustcode_session::strip_verbatim(&std::fs::canonicalize(new).unwrap());
        assert!(
            answer["thread_ids"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        let moved = crate::sessions::load_session_workspace(&id).unwrap();
        assert_eq!(moved.cwd, new);
        assert_eq!(crate::sessions::load_session_repo(&id), Some(new.clone()));
        assert_eq!(moved.additional_directories, vec![new.join("docs")]);
        assert_eq!(std::fs::read(&history_path).unwrap(), history);
        let moved = crate::harness::store::load(&id).unwrap();
        assert_eq!(moved.cwd, new);
        assert_eq!(moved.native_session_id.as_deref(), Some("native-kept"));
    }

    #[test]
    fn a_failed_record_write_restores_the_folder_and_prior_records() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        std::fs::create_dir(&old).unwrap();
        let a = dir.path().join("a.json");
        let b = dir.path().join("b.json");
        std::fs::write(&a, b"before").unwrap();
        std::fs::write(&b, b"before").unwrap();
        let edits = vec![
            (a.clone(), b"before".to_vec(), b"after".to_vec()),
            (b.clone(), b"before".to_vec(), b"after".to_vec()),
        ];
        let mut calls = 0;
        assert!(
            commit_move(&old, &new, &edits, |path, bytes| {
                calls += 1;
                if calls == 2 {
                    return Err(std::io::Error::other("disk full"));
                }
                atomic_write(path, bytes)
            })
            .is_err()
        );
        assert!(old.exists());
        assert!(!new.exists());
        assert_eq!(std::fs::read(&a).unwrap(), b"before");
        assert_eq!(std::fs::read(&b).unwrap(), b"before");
    }

    #[test]
    fn project_settings_keep_their_values_at_the_new_path() {
        let mut settings = json!({"version": 1, "values": {"resume-after-restart": true}, "projects": {"/old/app": {"default-permission-mode": "ask"}, "/old/application": {"model": "unchanged"}}});
        assert!(rewrite_projects(
            &mut settings,
            Path::new("/old/app"),
            Path::new("/new/app")
        ));
        assert_eq!(
            settings["projects"][Path::new("/new/app").to_string_lossy().as_ref()]["default-permission-mode"],
            "ask"
        );
        assert_eq!(
            settings["projects"]["/old/application"]["model"],
            "unchanged"
        );
        assert_eq!(settings["values"]["resume-after-restart"], true);
    }
}
