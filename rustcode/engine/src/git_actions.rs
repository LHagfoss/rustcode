//! Git writes the app runs in a thread's folder without asking the agent:
//! commit, push, pull and opening a pull request (#3724).
//!
//! `_rustcode/git/state` reads what the header's git button needs to pick its
//! action, and `_rustcode/git/run` runs one action. Every write goes through
//! the user's own `git` (hooks and signing apply) and `gh`; nothing is
//! stashed, reset or forced.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::worktrees::{git, git_timed};

/// A commit runs the repository's hooks, which may lint or test.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(300);
/// A push or pull talks to the remote, and a pre-push hook may run checks.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(300);
/// The most output an answer carries, from its end.
const OUTPUT_TAIL: usize = 4_000;

/// Read a folder's git state for the header's git button: its branch, the
/// default branch, the changed files, and how far it is from its upstream.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/git/state", response = serde_json::Value)]
pub(crate) struct StateRequest {
    pub path: PathBuf,
}

/// Run one git action in a folder. `action` is `commit`, `push`, `commit_push`,
/// `pull` or `create_pr`. A commit names its `message` and `files`; on the
/// default branch it needs `newBranch` or `allowDefaultBranch`. A pull request
/// takes `title`, `body`, `base` and `draft`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/git/run", response = serde_json::Value)]
pub(crate) struct RunRequest {
    pub path: PathBuf,
    pub action: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub new_branch: Option<String>,
    #[serde(default)]
    pub allow_default_branch: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub draft: bool,
}

/// Why an action did not run or failed, with the command's output tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitError {
    pub reason: &'static str,
    pub message: String,
}

impl GitError {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: tail(&message.into()),
        }
    }

    pub(crate) fn into_acp(self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::invalid_params()
            .data(json!({ "reason": self.reason, "message": self.message }))
    }
}

/// One changed file: its path and a one-letter status (`M`, `A`, `D`, `R`,
/// `?` untracked, `U` conflicted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ChangedFile {
    pub path: String,
    pub status: char,
}

/// What `git status --porcelain=v2 --branch -z` says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Status {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<ChangedFile>,
}

/// The last `OUTPUT_TAIL` bytes of `text`, cut at a character boundary.
fn tail(text: &str) -> String {
    let text = text.trim();
    if text.len() <= OUTPUT_TAIL {
        return text.to_owned();
    }
    let mut start = text.len() - OUTPUT_TAIL;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Parse `git status --porcelain=v2 --branch -z --untracked-files=all`.
pub(crate) fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut fields = text.split('\0');
    while let Some(entry) = fields.next() {
        if let Some(header) = entry.strip_prefix("# ") {
            if let Some(head) = header.strip_prefix("branch.head ") {
                if head != "(detached)" {
                    status.branch = Some(head.to_owned());
                }
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                status.upstream = Some(upstream.to_owned());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        status.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        status.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let (kind, rest) = entry.split_at(entry.len().min(1));
        let (letter, path) = match kind {
            "1" => {
                let parts: Vec<&str> = rest.trim_start().splitn(8, ' ').collect();
                (xy_letter(parts.first().copied()), parts.get(7).copied())
            }
            "2" => {
                let parts: Vec<&str> = rest.trim_start().splitn(9, ' ').collect();
                // The renamed-from path follows as its own field.
                fields.next();
                (Some('R'), parts.get(8).copied())
            }
            "u" => {
                let parts: Vec<&str> = rest.trim_start().splitn(10, ' ').collect();
                (Some('U'), parts.get(9).copied())
            }
            "?" => (Some('?'), Some(rest.trim_start())),
            _ => (None, None),
        };
        if let (Some(status_letter), Some(path)) = (letter, path)
            && !path.is_empty()
        {
            status.files.push(ChangedFile {
                path: path.to_owned(),
                status: status_letter,
            });
        }
    }
    status
}

/// The letter a `1` entry shows: the staged change, else the unstaged one.
fn xy_letter(xy: Option<&str>) -> Option<char> {
    let mut chars = xy?.chars();
    let (x, y) = (chars.next()?, chars.next()?);
    Some(if x != '.' { x } else { y })
}

fn status(path: &Path) -> Result<Status, GitError> {
    git(
        path,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .map(|text| parse_status(&text))
    .map_err(|message| GitError::new("not_git", message))
}

fn line(path: &Path, args: &[&str]) -> Option<String> {
    git(path, args)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The branch the remote is on (`origin/HEAD`), else a local `main` or `master`.
fn default_branch(path: &Path) -> Option<String> {
    if let Some(name) = line(
        path,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        return Some(name.strip_prefix("origin/").unwrap_or(&name).to_owned());
    }
    ["main", "master"].into_iter().find_map(|name| {
        line(
            path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        )
        .map(|_| name.to_owned())
    })
}

/// The remote a first push goes to: `origin`, else the only remote.
fn push_remote(path: &Path) -> Option<String> {
    let remotes = line(path, &["remote"])?;
    let remotes: Vec<&str> = remotes.lines().map(str::trim).collect();
    if remotes.contains(&"origin") {
        return Some("origin".to_owned());
    }
    remotes.first().map(|name| (*name).to_owned())
}

/// The state `_rustcode/git/state` answers.
pub(crate) fn state(path: &Path) -> Result<Value, GitError> {
    if !path.is_dir() {
        return Err(GitError::new(
            "missing",
            format!("{} does not exist", path.display()),
        ));
    }
    let status = status(path)?;
    let default = default_branch(path);
    let remote = push_remote(path);
    Ok(json!({
        "path": path,
        "branch": status.branch,
        "defaultBranch": default,
        "onDefault": status.branch.is_some() && status.branch == default,
        "upstream": status.upstream,
        "ahead": status.ahead,
        "behind": status.behind,
        "hasRemote": remote.is_some(),
        "files": status.files,
    }))
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

fn commit(path: &Path, request: &RunRequest) -> Result<String, GitError> {
    let message = non_empty(request.message.as_deref())
        .ok_or_else(|| GitError::new("invalid", "A commit needs a message"))?;
    let files: Vec<&str> = request
        .files
        .iter()
        .map(|file| file.trim())
        .filter(|file| !file.is_empty())
        .collect();
    if files.is_empty() {
        return Err(GitError::new(
            "invalid",
            "Choose at least one file to commit",
        ));
    }
    if files.iter().any(|file| file.starts_with('-')) {
        return Err(GitError::new("invalid", "A file name cannot start with -"));
    }
    let current = status(path)?;
    let mut output = String::new();
    if let Some(branch) = non_empty(request.new_branch.as_deref()) {
        if branch.starts_with('-') || git(path, &["check-ref-format", "--branch", branch]).is_err()
        {
            return Err(GitError::new(
                "invalid",
                format!("'{branch}' is not a valid branch name"),
            ));
        }
        git(path, &["switch", "-c", branch]).map_err(|error| GitError::new("failed", error))?;
        output.push_str(&format!("Switched to a new branch '{branch}'\n"));
    } else if current.branch.is_some()
        && current.branch == default_branch(path)
        && !request.allow_default_branch
    {
        return Err(GitError::new(
            "default_branch",
            format!(
                "This commits to {}. Confirm it, or commit to a new branch.",
                current.branch.as_deref().unwrap_or_default()
            ),
        ));
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(files.iter().copied());
    git(path, &add).map_err(|error| GitError::new("failed", error))?;
    let mut commit = vec!["commit", "-m", message, "--"];
    commit.extend(files.iter().copied());
    let out = git_timed(path, &commit, COMMIT_TIMEOUT)
        .map_err(|error| GitError::new("hook_failed", error))?;
    output.push_str(&out);
    Ok(output)
}

fn push(path: &Path) -> Result<String, GitError> {
    let current = status(path)?;
    let Some(branch) = current.branch else {
        return Err(GitError::new("detached", "HEAD is not on a branch"));
    };
    if current.upstream.is_some() {
        return git_timed(path, &["push"], REMOTE_TIMEOUT)
            .map_err(|error| GitError::new("push_rejected", error));
    }
    let remote = push_remote(path)
        .ok_or_else(|| GitError::new("no_remote", "This repository has no remote to push to"))?;
    git_timed(
        path,
        &[
            "push",
            "--set-upstream",
            &remote,
            &format!("HEAD:refs/heads/{branch}"),
        ],
        REMOTE_TIMEOUT,
    )
    .map_err(|error| GitError::new("push_rejected", error))
}

fn pull(path: &Path) -> Result<String, GitError> {
    git_timed(path, &["pull", "--ff-only"], REMOTE_TIMEOUT)
        .map_err(|error| GitError::new("pull_failed", error))
}

async fn create_pr(path: &Path, request: &RunRequest) -> Result<(String, String), GitError> {
    let current = status(path)?;
    let mut output = String::new();
    if current.upstream.is_none() || current.ahead > 0 {
        let path = path.to_path_buf();
        output = tokio::task::spawn_blocking(move || push(&path))
            .await
            .map_err(|error| GitError::new("failed", error.to_string()))??;
    }
    let mut args: Vec<String> = vec!["pr".into(), "create".into()];
    match non_empty(request.title.as_deref()) {
        Some(title) => {
            args.extend(["--title".into(), title.to_owned()]);
            args.extend(["--body".into(), request.body.clone().unwrap_or_default()]);
        }
        None => args.push("--fill".into()),
    }
    if let Some(base) = non_empty(request.base.as_deref()) {
        args.extend(["--base".into(), base.to_owned()]);
    }
    if request.draft {
        args.push("--draft".into());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let printed = crate::pull_requests::gh(path, &args, None)
        .await
        .map_err(|error| GitError::new(error.code, error.message))?;
    let printed = String::from_utf8_lossy(&printed).into_owned();
    let url = printed
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .unwrap_or_default()
        .to_owned();
    output.push_str(&printed);
    Ok((output, url))
}

/// Run `request.action` and answer `{ action, output, url?, state }`.
pub(crate) async fn run(request: RunRequest) -> Result<Value, GitError> {
    let path = request.path.clone();
    if !path.is_dir() {
        return Err(GitError::new(
            "missing",
            format!("{} does not exist", path.display()),
        ));
    }
    let action = request.action.trim().to_owned();
    let mut url = None;
    let output = match action.as_str() {
        "commit" | "push" | "commit_push" | "pull" => {
            let request = request.clone();
            let action = action.clone();
            tokio::task::spawn_blocking(move || -> Result<String, GitError> {
                let path = request.path.as_path();
                match action.as_str() {
                    "commit" => commit(path, &request),
                    "push" => push(path),
                    "pull" => pull(path),
                    _ => {
                        let mut out = commit(path, &request)?;
                        out.push('\n');
                        out.push_str(&push(path)?);
                        Ok(out)
                    }
                }
            })
            .await
            .map_err(|error| GitError::new("failed", error.to_string()))??
        }
        "create_pr" => {
            let (out, link) = create_pr(&path, &request).await?;
            url = Some(link).filter(|link| !link.is_empty());
            out
        }
        other => {
            return Err(GitError::new(
                "invalid",
                format!("Unknown git action '{other}'"),
            ));
        }
    };
    let state = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || state(&path))
            .await
            .ok()
            .and_then(Result::ok)
    };
    Ok(json!({
        "action": action,
        "output": tail(&output),
        "url": url,
        "state": state,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_reads_branch_distance_and_files() {
        let text = [
            "# branch.oid 0123",
            "# branch.head feat/x",
            "# branch.upstream origin/feat/x",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 aaa bbb src/a file.ts",
            "1 A. N... 000000 100644 100644 000 ccc new.ts",
            "2 R. N... 100644 100644 100644 aaa bbb R100 to.ts",
            "from.ts",
            "u UU N... 100644 100644 100644 100644 a b c conflict.ts",
            "? untracked.txt",
            "",
        ]
        .join("\0");
        let status = parse_status(&text);
        assert_eq!(status.branch.as_deref(), Some("feat/x"));
        assert_eq!(status.upstream.as_deref(), Some("origin/feat/x"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        let files: Vec<(&str, char)> = status
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.status))
            .collect();
        assert_eq!(
            files,
            vec![
                ("src/a file.ts", 'M'),
                ("new.ts", 'A'),
                ("to.ts", 'R'),
                ("conflict.ts", 'U'),
                ("untracked.txt", '?'),
            ]
        );
    }

    #[test]
    fn a_detached_head_names_no_branch() {
        let status = parse_status("# branch.oid 0123\0# branch.head (detached)\0");
        assert_eq!(status.branch, None);
        assert!(status.files.is_empty());
    }

    #[test]
    fn long_output_keeps_its_end() {
        let long = "x".repeat(OUTPUT_TAIL + 10) + "END";
        let cut = tail(&long);
        assert!(cut.ends_with("END"));
        assert!(cut.starts_with('…'));
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "T"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(path, &args).unwrap();
        }
        std::fs::write(path.join("a.txt"), "a").unwrap();
        git(path, &["add", "a.txt"]).unwrap();
        git(path, &["commit", "-qm", "init"]).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_commit_to_the_default_branch_needs_confirmation() {
        let dir = repo();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        let request = RunRequest {
            path: dir.path().to_path_buf(),
            action: "commit".into(),
            message: Some("add b".into()),
            files: vec!["b.txt".into()],
            ..RunRequest::default()
        };
        let refused = run(request.clone()).await.unwrap_err();
        assert_eq!(refused.reason, "default_branch");
        let answer = run(RunRequest {
            allow_default_branch: true,
            ..request
        })
        .await
        .unwrap();
        assert_eq!(answer["state"]["files"], json!([]));
    }

    #[tokio::test]
    async fn a_commit_on_a_new_branch_takes_only_the_chosen_files() {
        let dir = repo();
        std::fs::write(dir.path().join("b.txt"), "b").unwrap();
        std::fs::write(dir.path().join("c.txt"), "c").unwrap();
        let answer = run(RunRequest {
            path: dir.path().to_path_buf(),
            action: "commit".into(),
            message: Some("add b".into()),
            files: vec!["b.txt".into()],
            new_branch: Some("feat/b".into()),
            ..RunRequest::default()
        })
        .await
        .unwrap();
        assert_eq!(answer["state"]["branch"], "feat/b");
        assert_eq!(
            answer["state"]["files"],
            json!([{ "path": "c.txt", "status": "?" }])
        );
    }

    #[tokio::test]
    async fn pushing_without_a_remote_says_so() {
        let dir = repo();
        let refused = run(RunRequest {
            path: dir.path().to_path_buf(),
            action: "push".into(),
            ..RunRequest::default()
        })
        .await
        .unwrap_err();
        assert_eq!(refused.reason, "no_remote");
    }
}
