//! Thread actions the engine has no ACP method for, done as the SwiftUI app
//! does them (`RustCodeApp.swift`, `rename` and `trash`): the thread's
//! folder comes from `rustcode threads path <id>` and is accepted only when
//! it is that thread's folder inside the thread store. Rename writes the
//! folder's `title.txt`; Trash moves the folder to the user's Trash; Open
//! thread log opens its `logs/debug.log`.

use std::path::{Component, Path, PathBuf};

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Resolves a thread directory only when it is inside this config folder's
/// thread store. Canonical paths also reject symlinks out of the store.
fn thread_dir_in_config(dir: &Path, id: &str, config: &Path) -> Option<PathBuf> {
    if !valid_id(id) || !dir.is_absolute() || dir.file_name().is_none_or(|name| name != id) {
        return None;
    }
    let config = config.canonicalize().ok()?;
    let dir = dir.canonicalize().ok()?;
    if !dir.is_dir() || dir.file_name().is_none_or(|name| name != id) {
        return None;
    }
    let relative = dir.strip_prefix(&config).ok()?;
    let store = relative.components().next()?;
    if !matches!(store, Component::Normal(name) if name == "threads" || name == "sessions") {
        return None;
    }
    Some(dir)
}

/// Resolves an existing file to its canonical location inside config.
fn file_in_config(path: &Path, config: &Path) -> Option<PathBuf> {
    let config = config.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    (path.starts_with(config) && path.is_file()).then_some(path)
}

fn thread_dir(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Not a thread id.".into());
    }
    let config = crate::diagnostics::config_dir().ok_or("No config folder.")?;
    let output = crate::process::command(crate::engine::resolve_engine())
        .args(["threads", "path", id])
        .output()
        .map_err(|e| format!("Could not run the RustCode server: {e}"))?;
    if !output.status.success() {
        return Err("Could not find this thread's folder.".into());
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    thread_dir_in_config(&dir, id, &config)
        .ok_or_else(|| "Could not find this thread's folder.".to_string())
}

/// One line of at most 200 characters.
pub fn clean_title(title: &str) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(200).collect())
}

/// Renames a thread. `user` is a name the user typed: it is recorded in the
/// thread directory, so an agent naming its own thread
/// leaves it alone (issue 3599). A rename an agent asked for leaves the record as
/// it is.
#[tauri::command(async)]
pub fn thread_rename(id: String, title: String, user: Option<bool>) -> Result<String, String> {
    let title = clean_title(&title).ok_or("A title can't be empty.")?;
    let dir = thread_dir(&id)?;
    write_title(&dir, &title, user == Some(true)).map_err(|e| e.to_string())?;
    Ok(title)
}

fn write_title(dir: &Path, title: &str, user: bool) -> std::io::Result<()> {
    if user {
        mark_user_titled(dir)?;
    }
    std::fs::write(dir.join("title.txt"), title)
}

/// Whether the user named the thread themselves (`thread_rename` with `user`,
/// or `/change_title` in the engine).
#[tauri::command(async)]
pub fn thread_user_titled(id: String) -> Result<bool, String> {
    user_titled(&thread_dir(&id)?).map_err(|e| e.to_string())
}

const METADATA_FILE: &str = "metadata.json";

fn user_titled(dir: &Path) -> std::io::Result<bool> {
    if dir.join("user-titled").try_exists()? {
        return Ok(true);
    }
    let raw = match std::fs::read(dir.join(METADATA_FILE)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let metadata: serde_json::Value = serde_json::from_slice(&raw)?;
    Ok(metadata
        .get("user_titled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Record the user's choice separately from engine metadata, which can be
/// updated concurrently by the engine.
fn mark_user_titled(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join("user-titled"), [])
}

/// Opens the thread's `logs/debug.log` in the text editor, or its folder when
/// the thread has not logged anything yet.
#[tauri::command(async)]
pub fn thread_log_open(id: String) -> Result<String, String> {
    let config = crate::diagnostics::config_dir().ok_or("No config folder.")?;
    let dir = thread_dir(&id)?;
    let log = dir.join("logs").join("debug.log");
    if log.exists() {
        let path = file_in_config(&log, &config)
            .filter(|path| path.starts_with(&dir))
            .ok_or("The thread log is outside this thread's folder.")?;
        crate::native::open_checked(&path, false)?;
        return Ok(path.display().to_string());
    }
    crate::native::open_checked(&dir, true)?;
    Ok(dir.display().to_string())
}

/// A name in `trash` that is not taken: `id`, `id 2`, `id 3`, …
#[cfg(not(windows))]
fn free_name(trash: &Path, id: &str) -> PathBuf {
    let mut candidate = trash.join(id);
    let mut n = 2;
    while candidate.exists() {
        candidate = trash.join(format!("{id} {n}"));
        n += 1;
    }
    candidate
}

#[tauri::command(async)]
pub fn thread_trash(id: String) -> Result<(), String> {
    let dir = thread_dir(&id)?;
    move_thread_to_trash(&dir, &id)
}

#[cfg(windows)]
fn move_thread_to_trash(dir: &Path, _id: &str) -> Result<(), String> {
    trash::delete(dir).map_err(recycle_bin_error)
}

#[cfg(any(windows, test))]
fn recycle_bin_error(error: impl std::fmt::Display) -> String {
    format!("Could not move the thread to the Recycle Bin: {error}")
}

#[cfg(not(windows))]
fn move_thread_to_trash(dir: &Path, id: &str) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("No home folder.")?;
    let trash = PathBuf::from(home).join(".Trash");
    if !trash.is_dir() {
        return Err("No Trash folder.".into());
    }
    std::fs::rename(dir, free_name(&trash, id))
        .map_err(|e| format!("Could not move the thread to the Trash: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_the_user_typed_is_recorded_beside_the_engines_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "rustcode-user-titled-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Older threads without metadata still retain the user's choice.
        mark_user_titled(&dir).unwrap();
        assert!(user_titled(&dir).unwrap());
        assert!(!dir.join(METADATA_FILE).exists());
        std::fs::remove_file(dir.join("user-titled")).unwrap();

        std::fs::write(
            dir.join(METADATA_FILE),
            r#"{"schema_version":1,"id":"abc-1","created_at_ms":1,"yolo":true}"#,
        )
        .unwrap();
        assert!(!user_titled(&dir).unwrap());
        mark_user_titled(&dir).unwrap();
        assert!(user_titled(&dir).unwrap());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(METADATA_FILE)).unwrap()).unwrap();
        assert_eq!(saved["yolo"], true);
        assert_eq!(saved["id"], "abc-1");
        std::fs::remove_file(dir.join("user-titled")).unwrap();
        std::fs::write(dir.join(METADATA_FILE), b"invalid").unwrap();
        assert!(user_titled(&dir).is_err());
        std::fs::create_dir(dir.join("user-titled")).unwrap();
        std::fs::write(dir.join("title.txt"), "original").unwrap();
        assert!(write_title(&dir, "new", true).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("title.txt")).unwrap(),
            "original"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn accepts_only_a_thread_folder_in_the_configured_store() {
        let base = std::env::temp_dir().join(format!(
            "rustcode-thread-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config = base.join("custom-config");
        let ok = config.join("threads/2026/11/02/abc-1");
        let outside = base.join("elsewhere/abc-1");
        std::fs::create_dir_all(&ok).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            thread_dir_in_config(&ok, "abc-1", &config),
            Some(ok.canonicalize().unwrap())
        );
        assert!(thread_dir_in_config(&ok, "abc-2", &config).is_none());
        assert!(thread_dir_in_config(&outside, "abc-1", &config).is_none());
        assert!(thread_dir_in_config(&config.join("skills/abc-1"), "abc-1", &config).is_none());

        let log = ok.join("logs/debug.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "safe log").unwrap();
        assert_eq!(
            file_in_config(&log, &config),
            Some(log.canonicalize().unwrap())
        );
        let external_log = base.join("outside.log");
        std::fs::write(&external_log, "not a thread log").unwrap();
        assert!(file_in_config(&external_log, &config).is_none());
        #[cfg(unix)]
        {
            let escaping = config.join("threads/2026/11/03/abc-1");
            std::fs::create_dir_all(escaping.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside, &escaping).unwrap();
            assert!(thread_dir_in_config(&escaping, "abc-1", &config).is_none());

            std::fs::remove_file(&log).unwrap();
            std::os::unix::fs::symlink(&external_log, &log).unwrap();
            assert!(file_in_config(&log, &config).is_none());
        }
        let _ = std::fs::remove_dir_all(base);

        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert!(valid_id("019a2b-cd_3"));
    }

    #[test]
    fn recycle_bin_failure_keeps_the_system_error() {
        assert_eq!(
            recycle_bin_error("Access is denied."),
            "Could not move the thread to the Recycle Bin: Access is denied."
        );
    }

    #[test]
    fn titles_are_one_line() {
        assert_eq!(clean_title("  a\n b  ").as_deref(), Some("a b"));
        assert_eq!(clean_title(" \n "), None);
    }
}
