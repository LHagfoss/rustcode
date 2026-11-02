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

/// A ref or commit the webview names as the base of a branch diff: no
/// option-like or odd characters, and it must resolve to a commit.
fn valid_base(base: &str) -> bool {
    !base.is_empty()
        && base.len() <= 200
        && !base.starts_with('-')
        && !base.contains("..")
        && base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/@~^-".contains(c))
}

/// Uncommitted changes against HEAD, plus untracked files. With `base`
/// (a worktree's base commit or branch), everything since the branch left
/// it: its commits and the working tree, against their merge base.
#[tauri::command(async)]
pub fn git_changes(cwd: String, base: Option<String>) -> Result<GitChanges, String> {
    let cwd = absolute_dir(&cwd)?;
    git(&cwd, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|_| "This folder is not a git repository.".to_string())?;
    let branch = git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string());
    let has_head = git(&cwd, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let since = match base.as_deref() {
        Some(base) => {
            if !valid_base(base) {
                return Err("Not a valid base revision.".into());
            }
            let commit = format!("{base}^{{commit}}");
            git(&cwd, &["rev-parse", "--verify", "--quiet", &commit])
                .map_err(|_| format!("{base} is not a commit here."))?;
            Some(git(&cwd, &["merge-base", base, "HEAD"])?.trim().to_string())
        }
        None => None,
    };
    let base: Vec<&str> = match &since {
        Some(sha) => vec![sha.as_str()],
        None if has_head => vec!["HEAD"],
        None => vec![],
    };
    let base = base.as_slice();

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

/// Opens a Terminal window in an existing folder (a worktree's, from its
/// thread menu).
#[tauri::command(async)]
pub fn open_terminal_at(path: String) -> Result<(), String> {
    let path = absolute_dir(&path)?;
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Opening a terminal is only supported on macOS so far.".into())
    }
}

/// Whether the system's Reduce transparency setting is on, which the web
/// view does not report through `prefers-reduced-transparency`.
#[tauri::command(async)]
pub fn reduce_transparency() -> bool {
    #[cfg(target_os = "macos")]
    {
        Command::new("defaults")
            .args(["read", "com.apple.universalaccess", "reduceTransparency"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The engine refuses larger images (`MAX_ENCODED_IMAGE_LEN`).
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// An image file the composer attaches, as an ACP image block takes it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFile {
    pub name: String,
    pub mime_type: String,
    pub data: String,
}

/// The MIME type the engine accepts for an image path, if any.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Read a dropped or picked image for the composer: an absolute path to a
/// PNG, JPEG, GIF or WebP file of at most 20 MB, nothing else.
#[tauri::command(async)]
pub fn read_image(path: String) -> Result<ImageFile, String> {
    use base64::Engine as _;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("Not an absolute path.".into());
    }
    let mime = image_mime(&path).ok_or("Only PNG, JPEG, GIF and WebP images attach as images.")?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file.".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("Images must be 20 MB or smaller.".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(ImageFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mime_type: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_images() {
        assert!(read_image("relative.png".into()).is_err());
        assert!(read_image("/etc/passwd".into()).is_err());
        assert_eq!(image_mime(Path::new("/a/b.JPG")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("/a/b.txt")), None);
    }

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
    fn checks_branch_bases() {
        assert!(valid_base("main"));
        assert!(valid_base("origin/main"));
        assert!(valid_base("abc123"));
        assert!(valid_base("HEAD~2"));
        assert!(!valid_base("--output=/tmp/x"));
        assert!(!valid_base("a..b"));
        assert!(!valid_base("main; rm"));
        assert!(!valid_base(""));
    }

    #[test]
    fn refuses_terminal_outside_folders() {
        assert!(open_terminal_at("relative".into()).is_err());
        assert!(open_terminal_at("/no/such/folder/here".into()).is_err());
    }

    #[test]
    fn refuses_non_web_links() {
        assert!(open_url("file:///etc/passwd".into()).is_err());
        assert!(open_url("javascript:alert(1)".into()).is_err());
    }
}
