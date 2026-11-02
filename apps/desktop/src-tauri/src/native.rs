//! Native helpers behind narrow, validated commands.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Harness tools whose own login may be started in Terminal. Anything else is
/// refused, so a compromised page cannot run arbitrary commands.
const LOGIN_TOOLS: &[&str] = &[
    "codex",
    "claude",
    "gemini",
    "opencode",
    "claude-agent-acp",
    "codex-acp",
    "gh",
    "rustcode",
];

const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

fn absolute_dir(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(format!("Not a folder: {}", path.display()));
    }
    Ok(path)
}

#[tauri::command(async)]
pub fn open_url(url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
    {
        return Err("Only web and mail links open.".into());
    }
    open_with_system(&url)
}

#[cfg(target_os = "macos")]
fn open_with_system(target: &str) -> Result<(), String> {
    Command::new("open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn open_with_system(target: &str) -> Result<(), String> {
    Command::new("explorer")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_with_system(target: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.exists() {
        return Err("No such file.".into());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        open_with_system(&path.parent().unwrap_or(&path).display().to_string())
    }
}

/// POSIX single-quote a word for a shell command line.
fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Runs a harness's own login command in a new Terminal window, where the
/// user signs in directly with the harness. RustCode never sees the result.
#[tauri::command(async)]
pub fn open_login_terminal(command: Vec<String>) -> Result<(), String> {
    let tool = command.first().ok_or("No command.")?;
    let name = Path::new(tool)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !LOGIN_TOOLS.contains(&name) || tool.contains('/') {
        return Err(format!("{tool} is not a known harness login."));
    }
    if command.iter().any(|w| w.contains('\n') || w.contains('\r')) {
        return Err("Invalid command.".into());
    }
    let line = command
        .iter()
        .map(|w| shell_quote(w))
        .collect::<Vec<_>>()
        .join(" ");
    #[cfg(target_os = "macos")]
    {
        // AppleScript string: escape backslashes and quotes.
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            line.replace('\\', "\\\\").replace('"', "\\\"")
        );
        Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = line;
        Err(
            "Opening a terminal is only supported on macOS so far; copy the command instead."
                .into(),
        )
    }
}

#[derive(Serialize)]
pub struct FileChange {
    path: String,
    status: String,
    #[serde(rename = "oldPath", skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    added: u64,
    removed: u64,
    binary: bool,
}

#[derive(Serialize)]
pub struct GitChanges {
    branch: Option<String>,
    files: Vec<FileChange>,
    diff: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Uncommitted changes against HEAD, plus untracked files.
#[tauri::command(async)]
pub fn git_changes(cwd: String) -> Result<GitChanges, String> {
    let cwd = absolute_dir(&cwd)?;
    git(&cwd, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|_| "This folder is not a git repository.".to_string())?;
    let branch = git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string());
    let has_head = git(&cwd, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let base: &[&str] = if has_head { &["HEAD"] } else { &[] };

    let mut files = Vec::new();
    let status = git(&cwd, &[&["diff", "--name-status", "-M"], base].concat())?;
    let numstat = git(&cwd, &[&["diff", "--numstat", "-M"], base].concat())?;
    let counts: Vec<(Option<u64>, Option<u64>)> = numstat
        .lines()
        .map(|l| {
            let mut parts = l.split('\t');
            (
                parts.next().and_then(|a| a.parse().ok()),
                parts.next().and_then(|r| r.parse().ok()),
            )
        })
        .collect();
    for (i, line) in status.lines().enumerate() {
        let parts: Vec<&str> = line.split('\t').collect();
        let code = parts
            .first()
            .map(|c| c.chars().next().unwrap_or('M'))
            .unwrap_or('M');
        let (path, old_path) = match parts.as_slice() {
            [_, old, new] => (new.to_string(), Some(old.to_string())),
            [_, path] => (path.to_string(), None),
            _ => continue,
        };
        let (added, removed) = counts.get(i).copied().unwrap_or((None, None));
        files.push(FileChange {
            path,
            status: code.to_string(),
            old_path,
            added: added.unwrap_or(0),
            removed: removed.unwrap_or(0),
            binary: added.is_none(),
        });
    }
    for path in git(&cwd, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        let lines = std::fs::read(cwd.join(path))
            .ok()
            .filter(|bytes| !bytes.contains(&0))
            .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count() as u64);
        files.push(FileChange {
            path: path.to_string(),
            status: "U".into(),
            old_path: None,
            added: lines.unwrap_or(0),
            removed: 0,
            binary: lines.is_none(),
        });
    }
    let mut diff = git(&cwd, &[&["diff", "--no-color", "-M"], base].concat())?;
    if diff.len() > MAX_DIFF_BYTES {
        let mut cut = MAX_DIFF_BYTES;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
    }
    Ok(GitChanges {
        branch,
        files,
        diff,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_shell_words() {
        assert_eq!(shell_quote("codex"), "codex");
        assert_eq!(shell_quote("/login"), "/login");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn refuses_unknown_login_tools() {
        assert!(open_login_terminal(vec!["rm".into(), "-rf".into()]).is_err());
        assert!(open_login_terminal(vec!["/tmp/codex".into()]).is_err());
        assert!(open_login_terminal(vec![]).is_err());
    }

    #[test]
    fn refuses_non_web_links() {
        assert!(open_url("file:///etc/passwd".into()).is_err());
        assert!(open_url("javascript:alert(1)".into()).is_err());
    }
}
