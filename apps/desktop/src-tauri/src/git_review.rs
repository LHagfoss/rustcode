//! A worktree branch one commit at a time, for the Review screen: the commits
//! since the branch left its base, and one commit's changes against its first
//! parent, so a merge of the base shows only what it brought in.

use crate::native::{absolute_dir, capped_diff, diff_files, git, valid_base, GitChanges};
use serde::Serialize;
use std::path::Path;

/// The most commits a branch lists.
const MAX_COMMITS: usize = 100;
/// The tree with nothing in it: what a root commit is compared against.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const FIELD: char = '\u{1f}';

#[derive(Serialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct GitCommit {
    sha: String,
    subject: String,
    author: String,
    /// Commit time, seconds since the epoch.
    time: i64,
    /// More than one for a merge.
    parents: usize,
}

/// `rev` as a full commit id, refused when it is not a plain revision or not a commit here.
fn resolve_commit(cwd: &Path, rev: &str) -> Result<String, String> {
    if !valid_base(rev) {
        return Err("Not a valid commit.".into());
    }
    git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .map(|sha| sha.trim().to_string())
    .map_err(|_| format!("{rev} is not a commit here."))
}

fn parse_commit(line: &str) -> Option<GitCommit> {
    let mut parts = line.split(FIELD);
    let sha = parts.next()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    Some(GitCommit {
        sha,
        subject: parts.next().unwrap_or_default().to_string(),
        author: parts.next().unwrap_or_default().to_string(),
        time: parts
            .next()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(0),
        parents: parts.next().unwrap_or_default().split_whitespace().count(),
    })
}

/// The commits HEAD made since it left `base`, newest first.
#[tauri::command(async)]
pub fn git_commits(cwd: String, base: String) -> Result<Vec<GitCommit>, String> {
    let cwd = absolute_dir(&cwd)?;
    let base = resolve_commit(&cwd, &base)?;
    let since = git(&cwd, &["merge-base", &base, "HEAD"])?
        .trim()
        .to_string();
    let log = git(
        &cwd,
        &[
            "log",
            "--topo-order",
            &format!("-n{MAX_COMMITS}"),
            "--format=%H%x1f%s%x1f%an%x1f%ct%x1f%P",
            &format!("{since}..HEAD"),
        ],
    )?;
    Ok(log.lines().filter_map(parse_commit).collect())
}

/// One commit's changes against its first parent, or against nothing for a root commit.
#[tauri::command(async)]
pub fn git_commit_changes(cwd: String, sha: String) -> Result<GitChanges, String> {
    let cwd = absolute_dir(&cwd)?;
    let sha = resolve_commit(&cwd, &sha)?;
    let parent = git(
        &cwd,
        &["rev-parse", "--verify", "--quiet", &format!("{sha}^1")],
    )
    .map(|p| p.trim().to_string())
    .unwrap_or_else(|_| EMPTY_TREE.to_string());
    let range = [parent.as_str(), sha.as_str()];
    Ok(GitChanges {
        branch: None,
        files: diff_files(&cwd, &range)?,
        diff: capped_diff(&cwd, &range)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> (std::path::PathBuf, impl Fn(&[&str]) -> String) {
        let dir = std::env::temp_dir().join(format!("rc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.clone();
        let run = move |args: &[&str]| {
            let out = crate::process::command("git")
                .arg("-C")
                .arg(&path)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        (dir, run)
    }

    #[test]
    fn a_branch_lists_its_commits_and_shows_each_alone() {
        let (dir, run) = repo("git-commits");
        let cwd = dir.to_string_lossy().into_owned();
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        let root = run(&["rev-parse", "HEAD"]);
        run(&["switch", "-q", "-c", "work"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        run(&["commit", "-q", "-am", "add two"]);
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "add b"]);
        run(&["switch", "-q", "main"]);
        std::fs::write(dir.join("c.txt"), "main\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "on main"]);
        run(&["switch", "-q", "work"]);
        run(&["merge", "-q", "--no-edit", "main"]);

        let commits = git_commits(cwd.clone(), "main".into()).unwrap();
        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects.len(), 3, "{subjects:?}");
        assert!(subjects.contains(&"add b") && subjects.contains(&"add two"));
        assert!(!subjects.contains(&"on main"));
        assert_eq!(commits[0].parents, 2);
        let two = commits.iter().find(|c| c.subject == "add two").unwrap();
        assert_eq!(two.parents, 1);

        let one = git_commit_changes(cwd.clone(), two.sha.clone()).unwrap();
        assert!(one.diff.contains("+two"));
        assert!(!one.diff.contains("b.txt"));
        let merge = git_commit_changes(cwd.clone(), commits[0].sha.clone()).unwrap();
        assert!(merge.diff.contains("c.txt"));
        assert!(!merge.diff.contains("+two"));
        let first = git_commit_changes(cwd.clone(), root).unwrap();
        assert!(first.diff.contains("+one"));

        assert!(git_commit_changes(cwd.clone(), "--all".into()).is_err());
        assert!(git_commit_changes(cwd.clone(), "a..b".into()).is_err());
        assert!(git_commit_changes(cwd, "0123456789abcdef".into()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_log_line_reads_as_a_commit() {
        let line = format!("abc{FIELD}Fix it{FIELD}Ann{FIELD}1700000000{FIELD}p1 p2");
        assert_eq!(
            parse_commit(&line),
            Some(GitCommit {
                sha: "abc".into(),
                subject: "Fix it".into(),
                author: "Ann".into(),
                time: 1_700_000_000,
                parents: 2,
            })
        );
        assert_eq!(parse_commit(""), None);
    }
}
