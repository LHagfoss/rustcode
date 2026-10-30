//! `get_changes` (#2650): a session's uncommitted changes, read for a device
//! to review. Read-only: it runs `git status` and `git diff`, never a Git
//! command that writes, and bounds what it sends so the answer fits one
//! frame.

use std::path::Path;
use std::time::Duration;

use super::protocol::{RemoteChangedFile, RemoteChanges};

/// Files listed at most; the rest are counted.
pub const MAX_FILES: usize = 100;
/// One file's diff at most.
pub const MAX_FILE_DIFF_BYTES: usize = 32 * 1024;
/// All diffs together at most, well inside one frame.
pub const MAX_TOTAL_DIFF_BYTES: usize = 160 * 1024;
/// Untracked files whose contents are shown as added.
const MAX_UNTRACKED_DIFFS: usize = 20;
/// The whole read at most.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The changes in `workspace` as of `captured_at`.
pub async fn read(workspace: &Path, captured_at: String) -> RemoteChanges {
    let empty = |reason: &str| RemoteChanges {
        captured_at: captured_at.clone(),
        workspace: workspace.display().to_string(),
        available: false,
        reason: Some(reason.to_owned()),
        head: None,
        branch: None,
        files: Vec::new(),
        omitted_files: 0,
    };
    match tokio::time::timeout(TIMEOUT, read_files(workspace)).await {
        Err(_) => empty("Git took too long to read the changes"),
        Ok(Err(reason)) => empty(&reason),
        Ok(Ok((head, branch, files, omitted))) => RemoteChanges {
            captured_at,
            workspace: workspace.display().to_string(),
            available: true,
            reason: None,
            head,
            branch,
            files,
            omitted_files: omitted,
        },
    }
}

type Read = (Option<String>, Option<String>, Vec<RemoteChangedFile>, u32);

async fn read_files(workspace: &Path) -> Result<Read, String> {
    let inside = git(workspace, &["rev-parse", "--is-inside-work-tree"])
        .await
        .map_err(|_| "The workspace is not a Git repository".to_owned())?;
    if inside.trim() != "true" {
        return Err("The workspace is not a Git repository".to_owned());
    }
    let head = git(workspace, &["rev-parse", "--short", "HEAD"])
        .await
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    let branch = git(workspace, &["branch", "--show-current"])
        .await
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    let status = git(
        workspace,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .await
    .map_err(|error| format!("git status failed: {error}"))?;
    let mut entries = parse_status(&status);
    let omitted = entries.len().saturating_sub(MAX_FILES) as u32;
    entries.truncate(MAX_FILES);

    // Tracked changes against HEAD, staged or not; a repository without a
    // commit yet has only untracked and added files.
    let base = if head.is_some() { "HEAD" } else { "--cached" };
    let diff = git(
        workspace,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--find-renames",
            base,
        ],
    )
    .await
    .unwrap_or_default();
    let mut diffs = split_diff(&diff);

    let mut budget = MAX_TOTAL_DIFF_BYTES;
    let mut untracked_shown = 0;
    let mut files = Vec::with_capacity(entries.len());
    for (status, path, old_path) in entries {
        let mut text = diffs.remove(&path);
        if text.is_none() && status == "untracked" && untracked_shown < MAX_UNTRACKED_DIFFS {
            untracked_shown += 1;
            // `--no-index` exits 1 when the files differ, which they do.
            text = git_any(
                workspace,
                &["diff", "--no-color", "--no-index", "--", "/dev/null", &path],
            )
            .await
            .ok();
        }
        let binary = text.as_deref().is_some_and(|diff| {
            diff.contains("\nBinary files ") || diff.starts_with("Binary files ")
        });
        let (additions, deletions) = text.as_deref().map(count_lines).unwrap_or((0, 0));
        let (diff, diff_truncated) = match text.filter(|_| !binary) {
            None => (None, false),
            Some(text) => bound(text, &mut budget),
        };
        files.push(RemoteChangedFile {
            path,
            old_path,
            status,
            additions,
            deletions,
            binary,
            diff,
            diff_truncated,
        });
    }
    Ok((head, branch, files, omitted))
}

/// `text` within the per-file cap and what is left of the frame's budget.
fn bound(text: String, budget: &mut usize) -> (Option<String>, bool) {
    let limit = MAX_FILE_DIFF_BYTES.min(*budget);
    if limit == 0 {
        return (None, true);
    }
    if text.len() <= limit {
        *budget -= text.len();
        return (Some(text), false);
    }
    // Cut at a line end, so no half line shows.
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let end = text[..end].rfind('\n').map_or(end, |index| index + 1);
    *budget -= end;
    (Some(text[..end].to_owned()), true)
}

/// `(status, path, old path)` per entry of `git status --porcelain=v1 -z`.
pub fn parse_status(output: &str) -> Vec<(String, String, Option<String>)> {
    let mut entries = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let (code, path) = field.split_at(3);
        let code = code.trim_end();
        let status = if code == "??" {
            "untracked"
        } else if code.contains('R') {
            "renamed"
        } else if code.contains('D') {
            "deleted"
        } else if code.contains('A') {
            "added"
        } else {
            "modified"
        };
        // A rename names its old path in the next field.
        let old_path = (status == "renamed")
            .then(|| fields.next().map(str::to_owned))
            .flatten();
        entries.push((status.to_owned(), path.to_owned(), old_path));
    }
    entries
}

/// Each file's part of a unified diff, by its new path.
pub fn split_diff(diff: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for part in diff.split("diff --git ").skip(1) {
        let text = format!("diff --git {part}");
        let path = part
            .lines()
            .find_map(|line| line.strip_prefix("+++ b/"))
            .or_else(|| {
                // A deletion has no `+++ b/` line; its header names it.
                part.lines()
                    .next()
                    .and_then(|line| line.rsplit_once(" b/").map(|(_, path)| path))
            })
            .map(str::to_owned);
        if let Some(path) = path {
            out.insert(path, text);
        }
    }
    out
}

/// Added and removed lines of one file's diff.
pub fn count_lines(diff: &str) -> (u32, u32) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed)
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(cwd, args).await?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// As `git`, taking any exit status that printed something.
async fn git_any(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(cwd, args).await?;
    if output.stdout.is_empty() {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

async fn run(cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("could not run git: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_entries_and_renames_parse() {
        let status = " M src/a.rs\0?? new.txt\0R  b.rs\0old_b.rs\0 D gone.rs\0A  added.rs\0";
        let entries = parse_status(status);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0], ("modified".into(), "src/a.rs".into(), None));
        assert_eq!(entries[1].0, "untracked");
        assert_eq!(
            entries[2],
            ("renamed".into(), "b.rs".into(), Some("old_b.rs".into()))
        );
        assert_eq!(entries[3].0, "deleted");
        assert_eq!(entries[4].0, "added");
    }

    #[test]
    fn a_diff_splits_by_file_and_counts_lines() {
        let diff = "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1,2 @@\n-old\n+new\n+more\ndiff --git a/y.rs b/y.rs\ndeleted file mode 100644\n--- a/y.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let parts = split_diff(diff);
        assert_eq!(count_lines(&parts["x.rs"]), (2, 1));
        assert_eq!(count_lines(&parts["y.rs"]), (0, 1));
    }

    #[test]
    fn diffs_are_cut_at_a_line_and_the_budget_runs_out() {
        let mut budget = 30;
        let (text, cut) = bound(
            "line one\nline two\nline three\nline four\n".into(),
            &mut budget,
        );
        assert!(cut);
        assert!(text.unwrap().ends_with('\n'));
        let (none, cut) = bound("more".into(), &mut 0);
        assert!(none.is_none() && cut);
    }

    #[tokio::test]
    async fn a_repository_reports_its_changes_and_a_folder_says_why_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap()
        };
        let plain = read(root, "now".into()).await;
        assert!(!plain.available);
        assert!(plain.reason.unwrap().contains("not a Git repository"));

        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-qm", "first"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("new.txt"), "fresh\n").unwrap();

        let changes = read(root, "now".into()).await;
        assert!(changes.available, "{:?}", changes.reason);
        assert!(changes.head.is_some());
        let a = changes
            .files
            .iter()
            .find(|file| file.path == "a.txt")
            .unwrap();
        assert_eq!(
            (a.status.as_str(), a.additions, a.deletions),
            ("modified", 1, 0)
        );
        assert!(a.diff.as_deref().unwrap().contains("+two"));
        let new = changes
            .files
            .iter()
            .find(|file| file.path == "new.txt")
            .unwrap();
        assert_eq!((new.status.as_str(), new.additions), ("untracked", 1));
        // Reading changed nothing.
        let status = run(&["status", "--porcelain"]);
        assert!(String::from_utf8_lossy(&status.stdout).contains("?? new.txt"));
    }
}
