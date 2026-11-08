//! Thread actions the engine has no ACP method for, done as the SwiftUI app
//! does them (`RustCodeApp.swift`, `rename` and `trash`): the thread's
//! folder comes from `rustcode threads path <id>` and is accepted only when
//! it is that thread's folder inside the thread store. Rename writes the
//! folder's `title.txt`; Trash moves the folder to the user's Trash; Open
//! thread log opens its `logs/debug.log`.

use std::path::{Path, PathBuf};

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `dir` is thread `id`'s folder in the store.
pub fn is_thread_dir(dir: &Path, id: &str) -> bool {
    // Windows paths separate with `\`; the store's folder names are the same.
    let text = dir.to_string_lossy().replace('\\', "/");
    dir.is_absolute()
        && dir.file_name().is_some_and(|n| n == id)
        && (text.contains("/rustcode/threads/") || text.contains("/rustcode/sessions/"))
        && !dir
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn thread_dir(id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Not a thread id.".into());
    }
    let output = crate::process::command(crate::engine::resolve_engine())
        .args(["threads", "path", id])
        .output()
        .map_err(|e| format!("Could not run the RustCode server: {e}"))?;
    if !output.status.success() {
        return Err("Could not find this thread's folder.".into());
    }
    let dir = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !is_thread_dir(&dir, id) || !dir.is_dir() {
        return Err("Could not find this thread's folder.".into());
    }
    Ok(dir)
}

/// One line of at most 200 characters.
pub fn clean_title(title: &str) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    Some(title.chars().take(200).collect())
}

#[tauri::command(async)]
pub fn thread_rename(id: String, title: String) -> Result<String, String> {
    let title = clean_title(&title).ok_or("A title can't be empty.")?;
    let dir = thread_dir(&id)?;
    std::fs::write(dir.join("title.txt"), &title).map_err(|e| e.to_string())?;
    Ok(title)
}

/// Opens the thread's `logs/debug.log` in the text editor, or its folder when
/// the thread has not logged anything yet.
#[tauri::command(async)]
pub fn thread_log_open(id: String) -> Result<String, String> {
    let dir = thread_dir(&id)?;
    let log = dir.join("logs").join("debug.log");
    if log.is_file() {
        crate::native::open_checked(&log, false)?;
        return Ok(log.display().to_string());
    }
    crate::native::open_checked(&dir, true)?;
    Ok(dir.display().to_string())
}

/// A name in `trash` that is not taken: `id`, `id 2`, `id 3`, …
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
    let home = std::env::var_os("HOME").ok_or("No home folder.")?;
    let trash = PathBuf::from(home).join(".Trash");
    if !trash.is_dir() {
        return Err("No Trash folder.".into());
    }
    std::fs::rename(&dir, free_name(&trash, &id))
        .map_err(|e| format!("Could not move the thread to the Trash: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_a_thread_folder_in_the_store() {
        let ok = Path::new("/Users/a/.config/rustcode/threads/2026/11/02/abc-1");
        assert!(is_thread_dir(ok, "abc-1"));
        assert!(!is_thread_dir(ok, "abc-2"));
        assert!(!is_thread_dir(Path::new("/Users/a/abc-1"), "abc-1"));
        assert!(!is_thread_dir(
            Path::new("/Users/a/.config/rustcode/threads/../../abc-1"),
            "abc-1"
        ));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
        assert!(valid_id("019a2b-cd_3"));
    }

    #[cfg(windows)]
    #[test]
    fn accepts_a_windows_thread_folder() {
        let ok = Path::new(r"C:\Users\a\AppData\Roaming\rustcode\threads\2026\11\02\abc-1");
        assert!(is_thread_dir(ok, "abc-1"));
    }

    #[test]
    fn titles_are_one_line() {
        assert_eq!(clean_title("  a\n b  ").as_deref(), Some("a b"));
        assert_eq!(clean_title(" \n "), None);
    }
}
