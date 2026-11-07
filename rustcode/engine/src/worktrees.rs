//! Worktree threads (`docs/orchestrator.md`, *Worktree threads*): a thread
//! can run in its own git worktree, kept under the config directory at
//! `worktrees/<repo-name>/<slug>`, so parallel threads never edit the same
//! checkout. Git is the source of truth for which worktrees exist; a small
//! record beside each one (`worktrees/<repo-name>/.<slug>.json`) keeps the
//! base it started from and the threads that use it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The file in a thread's directory naming the worktree it runs in.
const THREAD_FILE: &str = "worktree.json";

/// One worktree, as `_rustcode/worktrees/*` answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub repo: PathBuf,
    /// The commit it branched from.
    pub base: String,
    /// The branch (or ref) `base` was resolved from, for the merged check on
    /// remove.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    worktree: Worktree,
    #[serde(default)]
    session_ids: Vec<String>,
}

/// Why a request failed; `reason` goes to `error.data.reason`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeError {
    pub reason: &'static str,
    pub message: String,
}

impl WorktreeError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            reason: "failed",
            message: message.into(),
        }
    }

    pub(crate) fn into_acp(self) -> agent_client_protocol::Error {
        agent_client_protocol::Error::invalid_params()
            .data(json!({ "reason": self.reason, "message": self.message }))
    }
}

// ---------------------------------------------------------------- git ----

/// Run git in `dir`, with the caller's environment cleared of anything that
/// would point it at another index or repository, and without taking the
/// optional index lock (`git status` would otherwise refresh the user's
/// index). Fails past `timeout`.
pub(crate) fn git_with(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &std::ffi::OsStr)],
    timeout: Duration,
) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("git did not start: {error}"))?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut text);
        }
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut text);
        }
        text
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {} timed out", args.first().unwrap_or(&"")));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(3)),
            Err(error) => return Err(error.to_string()),
        }
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if status.success() {
        Ok(stdout)
    } else {
        Err(stderr.trim().to_owned())
    }
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_with(dir, args, &[], Duration::from_secs(60))
}

fn git_line(dir: &Path, args: &[&str]) -> Result<String, String> {
    git(dir, args).map(|out| out.trim().to_owned())
}

/// The top level of the git work tree holding `dir`.
pub(crate) fn toplevel(dir: &Path) -> Option<PathBuf> {
    let top = git_line(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    std::fs::canonicalize(top).ok()
}

/// The main checkout of the repository `dir` belongs to, so a worktree of
/// a worktree still files under the repository's name.
pub(crate) fn main_checkout(dir: &Path) -> Option<PathBuf> {
    let common = git_line(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let common = std::fs::canonicalize(common).ok()?;
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        toplevel(dir)
    }
}

/// The repository a thread's folder belongs to, worktree or main checkout: the
/// one project a main checkout and every worktree of it share (#3150).
///
/// Asked once per folder, not once per thread: `session/list` lists threads and
/// a repository does not move, so the answer is cached. A folder that is gone
/// (or not a repository) answers `None` and the caller falls back to the path.
pub(crate) fn repo_of(dir: &Path) -> Option<PathBuf> {
    static SEEN: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if let Ok(seen) = SEEN.lock()
        && let Some(found) = seen.get(&key)
    {
        return found.clone();
    }
    let found = main_checkout(&key);
    if let Ok(mut seen) = SEEN.lock() {
        seen.insert(key, found.clone());
    }
    found
}

// ------------------------------------------------------------ storage ----

pub(crate) fn root() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join("worktrees"))
}

fn repo_name(repo: &Path) -> String {
    let name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let slug = slugify(&name, 60);
    if slug.is_empty() {
        "repo".to_owned()
    } else {
        slug
    }
}

fn record_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some(path.parent()?.join(format!(".{name}.json")))
}

fn load_record(path: &Path) -> Option<Record> {
    let text = std::fs::read_to_string(record_path(path)?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_record(record: &Record) {
    let Some(path) = record_path(&record.worktree.path) else {
        return;
    };
    if let Ok(text) = serde_json::to_string_pretty(record) {
        let _ = write_atomic(&path, text.as_bytes());
    }
}

/// Write `bytes` to `path` through a temporary file beside it.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

// ------------------------------------------------------------ naming -----

/// Lowercase ASCII words joined by `-`, at most `limit` bytes, cut at a word.
fn slugify(text: &str, limit: usize) -> String {
    let mut slug = String::new();
    for word in text
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        let extra = word.len() + usize::from(!slug.is_empty());
        if slug.len() + extra > limit {
            if slug.is_empty() {
                slug.push_str(&word[..limit.min(word.len())]);
            }
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word);
    }
    slug
}

/// `<type>/<slug>` for a thread titled `title`: the type from the title's
/// leading verb, the slug from its other words, short filler dropped.
pub(crate) fn branch_from_title(title: &str) -> String {
    const FILLER: &[&str] = &[
        "a", "an", "the", "to", "of", "for", "and", "in", "on", "with", "into", "from", "by", "at",
        "please", "some",
    ];
    let words: Vec<String> = title
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let (kind, skip) = match words.first().map(String::as_str) {
        Some("fix" | "fixes" | "fixed" | "repair" | "resolve" | "bug") => ("fix", 1),
        Some("doc" | "docs" | "document" | "documentation") => ("docs", 1),
        Some("refactor" | "clean" | "cleanup" | "simplify" | "rename" | "move") => ("refactor", 1),
        Some("test" | "tests") => ("test", 1),
        Some("perf" | "speed" | "optimize" | "optimise") => ("perf", 1),
        Some("chore" | "bump" | "update" | "upgrade") => ("chore", 1),
        Some("add" | "adds" | "implement" | "create" | "make" | "build" | "support" | "feat") => {
            ("feat", 1)
        }
        _ => ("feat", 0),
    };
    let rest: Vec<&str> = words
        .iter()
        .skip(skip)
        .map(String::as_str)
        .filter(|word| !FILLER.contains(word))
        .collect();
    let mut slug = slugify(&rest.join(" "), 40);
    if slug.is_empty() {
        slug = "thread".to_owned();
    }
    format!("{kind}/{slug}")
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// `base`, else `base-2`, `base-3`, … the first that `taken` refuses.
fn deduplicate(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..1000)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| format!("{base}-{}", chrono::Utc::now().timestamp()))
}

/// The branch a new worktree of `repo` titled `title` gets.
pub(crate) fn suggest_branch(repo: &Path, title: &str) -> String {
    deduplicate(&branch_from_title(title), |name| branch_exists(repo, name))
}

fn valid_branch(repo: &Path, branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && git(repo, &["check-ref-format", "--branch", branch]).is_ok()
}

// ---------------------------------------------------------- requests -----

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/worktrees/create", response = serde_json::Value)]
pub(crate) struct CreateRequest {
    pub repo: PathBuf,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/worktrees/list", response = serde_json::Value)]
pub(crate) struct ListRequest {
    pub repo: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/worktrees/remove", response = serde_json::Value)]
pub(crate) struct RemoveRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub force: bool,
}

/// Create a worktree of `repo` on a new branch (or an existing one that no
/// worktree has checked out).
pub(crate) fn create(request: &CreateRequest) -> Result<Worktree, WorktreeError> {
    let repo = std::fs::canonicalize(&request.repo)
        .map_err(|error| WorktreeError::failed(format!("invalid repo: {error}")))?;
    let repo = main_checkout(&repo).ok_or_else(|| WorktreeError {
        reason: "not_git",
        message: format!("{} is not in a git repository", repo.display()),
    })?;
    let base_ref = request
        .base
        .clone()
        .filter(|base| !base.trim().is_empty())
        .unwrap_or_else(|| "HEAD".to_owned());
    if base_ref.starts_with('-') {
        return Err(WorktreeError::failed("invalid base"));
    }
    let base = git_line(
        &repo,
        &["rev-parse", "--verify", &format!("{base_ref}^{{commit}}")],
    )
    .map_err(|_| WorktreeError {
        reason: "no_base",
        message: format!("{base_ref} does not name a commit"),
    })?;
    // `HEAD` is remembered as the branch it pointed at, so remove can tell
    // whether the thread's branch landed there since.
    let named_base = if base_ref == "HEAD" {
        git_line(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
    } else {
        Some(base_ref.clone())
    };
    let (branch, existing) = match request.branch.as_deref().map(str::trim) {
        Some(branch) if !branch.is_empty() => {
            if !valid_branch(&repo, branch) {
                return Err(WorktreeError::failed(format!(
                    "'{branch}' is not a valid branch name"
                )));
            }
            (branch.to_owned(), branch_exists(&repo, branch))
        }
        _ => (
            suggest_branch(&repo, request.title.as_deref().unwrap_or("")),
            false,
        ),
    };
    let parent = root()
        .ok_or_else(|| WorktreeError::failed("no config directory"))?
        .join(repo_name(&repo));
    std::fs::create_dir_all(&parent).map_err(|error| WorktreeError::failed(error.to_string()))?;
    let slug = deduplicate(&slugify(&branch, 80), |slug| {
        parent.join(slug).exists() || parent.join(format!(".{slug}.json")).exists()
    });
    let path = parent.join(&slug);
    let path_text = path.to_string_lossy().into_owned();
    // Remembered so a failed add undoes only what it made itself.
    let path_existed = path.exists();
    if !existing && let Err(message) = git(&repo, &["branch", "--no-track", &branch, &base]) {
        return Err(WorktreeError::failed(format!(
            "git branch failed: {message}"
        )));
    }
    if let Err(message) = git(&repo, &["worktree", "add", &path_text, &branch]) {
        // A failed add can leave its new branch and a half-made checkout
        // behind; clearing them keeps the name free for the retry.
        abandon_create(&repo, &path, path_existed, &branch, !existing, &base);
        return Err(WorktreeError::failed(format!(
            "git worktree add failed: {message}"
        )));
    }
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let worktree = Worktree {
        path,
        branch,
        repo,
        base,
        base_ref: named_base,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    save_record(&Record {
        worktree: worktree.clone(),
        session_ids: Vec::new(),
    });
    Ok(worktree)
}

/// Undo what a failed `git worktree add` made, so the next attempt gets the
/// same branch name and leaves no half-made checkout behind.
///
/// `git worktree add` registers the worktree and checks it out before the step
/// that can fail (a failing post-checkout hook, say), so a failure can leave a
/// live worktree holding the branch. Only what this call made is touched:
/// `path_existed` says whether the path was there before the add, `branch` is
/// the one the add asked for, and `created_branch` whether this call made it.
fn abandon_create(
    repo: &Path,
    path: &Path,
    path_existed: bool,
    branch: &str,
    created_branch: bool,
    base: &str,
) {
    if !path_existed && let Err(error) = remove_leftover_worktree(repo, path, branch) {
        crate::dbg_log!(
            "[worktrees] a failed add left {} and it could not be removed: {error}",
            path.display()
        );
    }
    // The worktree has to be gone first: git refuses to delete a branch that
    // one still has checked out.
    if created_branch && let Err(error) = delete_created_branch(repo, branch, base) {
        crate::dbg_log!(
            "[worktrees] a failed add left the branch {branch} and it could not be deleted: {error}"
        );
    }
}

/// Remove the worktree a failed add registered at `path`, if git registered
/// one there for `branch`. Nothing else at that path is touched: a worktree on
/// another branch is someone else's, however the paths came to collide.
fn remove_leftover_worktree(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    let listed = git(repo, &["worktree", "list", "--porcelain"])?;
    let expected = format!("refs/heads/{branch}");
    let field = |block: &str, prefix: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::to_owned)
    };
    let ours = listed.split("\n\n").any(|block| {
        field(block, "worktree ").is_some_and(|listed| same_path(Path::new(&listed), path))
            && field(block, "branch ").as_deref() == Some(expected.as_str())
    });
    if !ours {
        return Ok(());
    }
    let path_text = path.to_string_lossy();
    // Plain first, then `--force` for a half-made checkout git would otherwise
    // keep: its files are this call's checkout of a branch nothing has
    // committed to, so there is nothing of anyone's in them to lose.
    for force in [false, true] {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path_text.as_ref());
        if git(repo, &args).is_ok() {
            return Ok(());
        }
    }
    // A worktree `worktree remove` will not take still holds a registration;
    // drop the directory and prune that entry.
    if let Err(error) = std::fs::remove_dir_all(path)
        && path.exists()
    {
        return Err(format!("could not remove {}: {error}", path.display()));
    }
    git(repo, &["worktree", "prune"]).map(|_| ())
}

/// Delete `branch`, which this call made, once its failed add is undone.
/// It still points at `base`, so no commit can be lost; a branch whose tip
/// moved is left alone, since something else is working on it.
fn delete_created_branch(repo: &Path, branch: &str, base: &str) -> Result<(), String> {
    // A branch that is already gone is nothing to undo.
    let Some(tip) = branch_tip(repo, branch) else {
        return Ok(());
    };
    if tip != base {
        return Err("its tip is no longer the base commit".to_owned());
    }
    // `-d` refuses a branch `HEAD` does not contain, which a base that is not
    // an ancestor of `HEAD` legitimately is; the tip is still `base`,
    // checked again below, so forcing the delete loses nothing.
    if git(repo, &["branch", "-d", branch]).is_ok() {
        return Ok(());
    }
    if branch_tip(repo, branch).as_deref() != Some(base) {
        return Err("its tip is no longer the base commit".to_owned());
    }
    git(repo, &["branch", "-D", branch]).map(|_| ())
}

fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
    git_line(
        repo,
        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
    )
    .ok()
}

/// Whether two paths name the same place, for a path git printed and one this
/// process built (a worktree git registered may be gone, so canonicalizing can
/// fail for either).
fn same_path(left: &Path, right: &Path) -> bool {
    fn canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
    canonical(left) == canonical(right)
}

fn status_counts(path: &Path, base: &str) -> (bool, u64, u64) {
    let dirty = git(path, &["status", "--porcelain", "--untracked-files=normal"])
        .map(|out| !out.trim().is_empty())
        .unwrap_or(false);
    let against = git_line(path, &["rev-parse", "--abbrev-ref", "@{upstream}"])
        .ok()
        .unwrap_or_else(|| base.to_owned());
    let (ahead, behind) = git_line(
        path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{against}"),
        ],
    )
    .ok()
    .and_then(|line| {
        let mut parts = line.split_whitespace();
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    })
    .unwrap_or((0, 0));
    (dirty, ahead, behind)
}

/// The worktrees RustCode made for `repo`, with their state.
pub(crate) fn list(repo: &Path) -> Result<Vec<Value>, WorktreeError> {
    let repo = std::fs::canonicalize(repo)
        .ok()
        .and_then(|repo| main_checkout(&repo))
        .ok_or_else(|| WorktreeError {
            reason: "not_git",
            message: format!("{} is not in a git repository", repo.display()),
        })?;
    let Some(root) = root().and_then(|root| std::fs::canonicalize(root).ok()) else {
        return Ok(Vec::new());
    };
    let porcelain =
        git(&repo, &["worktree", "list", "--porcelain"]).map_err(WorktreeError::failed)?;
    let mut rows = Vec::new();
    for block in porcelain.split("\n\n") {
        let Some(path) = block
            .lines()
            .find_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
        else {
            continue;
        };
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if !path.starts_with(&root) {
            continue;
        }
        let branch = block
            .lines()
            .find_map(|line| line.strip_prefix("branch refs/heads/"))
            .unwrap_or_default()
            .to_owned();
        let record = load_record(&path);
        let worktree = record
            .as_ref()
            .map(|record| record.worktree.clone())
            .unwrap_or_else(|| Worktree {
                path: path.clone(),
                branch: branch.clone(),
                repo: repo.clone(),
                base: String::new(),
                base_ref: None,
                created_at: String::new(),
            });
        let (dirty, ahead, behind) = status_counts(&path, &worktree.base);
        let mut row = serde_json::to_value(Worktree {
            branch: if branch.is_empty() {
                worktree.branch.clone()
            } else {
                branch
            },
            ..worktree
        })
        .unwrap_or_default();
        if let Some(object) = row.as_object_mut() {
            object.insert("dirty".into(), json!(dirty));
            object.insert("ahead".into(), json!(ahead));
            object.insert("behind".into(), json!(behind));
            object.insert(
                "session_ids".into(),
                json!(record.map(|record| record.session_ids).unwrap_or_default()),
            );
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Remove a worktree RustCode made. Refuses one with uncommitted changes
/// (`dirty`) or commits no remote branch has (`unpushed`) unless `force`;
/// deletes its branch only when the branch is merged into its base.
pub(crate) fn remove(path: &Path, force: bool) -> Result<Value, WorktreeError> {
    let path = std::fs::canonicalize(path)
        .map_err(|error| WorktreeError::failed(format!("invalid path: {error}")))?;
    let inside = root()
        .and_then(|root| std::fs::canonicalize(root).ok())
        .is_some_and(|root| path.starts_with(&root) && path != root);
    if !inside {
        return Err(WorktreeError {
            reason: "not_managed",
            message: "only worktrees RustCode created can be removed".to_owned(),
        });
    }
    let record = load_record(&path);
    let repo = main_checkout(&path).ok_or_else(|| WorktreeError {
        reason: "not_git",
        message: format!("{} is not a git worktree", path.display()),
    })?;
    let branch = git_line(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let base = record
        .as_ref()
        .map(|record| record.worktree.base.clone())
        .filter(|base| !base.is_empty());
    if !force {
        let (dirty, _, _) = status_counts(&path, base.as_deref().unwrap_or("HEAD"));
        if dirty {
            return Err(WorktreeError {
                reason: "dirty",
                message: "the worktree has uncommitted changes".to_owned(),
            });
        }
        let mut args = vec!["rev-list", "--count", "HEAD", "--not", "--remotes"];
        if let Some(base) = base.as_deref() {
            args.push(base);
        }
        let unpushed = git_line(&path, &args)
            .ok()
            .and_then(|count| count.parse::<u64>().ok())
            .unwrap_or(0);
        if unpushed > 0 {
            return Err(WorktreeError {
                reason: "unpushed",
                message: format!("{unpushed} commit(s) are not on any remote branch"),
            });
        }
    }
    let path_text = path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_text);
    git(&repo, &args).map_err(|message| {
        WorktreeError::failed(format!("git worktree remove failed: {message}"))
    })?;
    let _ = git(&repo, &["worktree", "prune"]);
    // A thread whose folder is gone cannot be opened again, so every thread
    // that ran here moves to the repository the worktree came from.
    if let Some(record) = record.as_ref() {
        move_threads_to_repo(&record.session_ids, &repo);
    }
    let mut branch_removed = false;
    if let Some(branch) = branch {
        let into = record
            .as_ref()
            .and_then(|record| record.worktree.base_ref.clone())
            .or(base);
        let merged = into.is_some_and(|into| {
            git(&repo, &["merge-base", "--is-ancestor", &branch, &into]).is_ok()
        });
        if merged {
            branch_removed = git(&repo, &["branch", "-D", &branch]).is_ok();
        }
    }
    if let Some(record) = record_path(&path) {
        let _ = std::fs::remove_file(record);
    }
    Ok(json!({ "removed": true, "branch_removed": branch_removed }))
}

// ------------------------------------------------------------ threads ----

/// `_meta.rustcode.worktree` of a `session/new`, when it asks for one.
pub(crate) fn requested(meta: Option<&serde_json::Map<String, Value>>) -> Option<CreateRequest> {
    let asked = meta?.get("rustcode")?.get("worktree")?;
    let repo = asked.get("repo")?.as_str()?;
    let text = |key: &str| asked.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(CreateRequest {
        repo: PathBuf::from(repo),
        branch: text("branch"),
        title: text("title"),
        base: text("base"),
    })
}

/// Record that `session_id` runs in `worktree`, on both sides.
pub(crate) fn attach(session_id: &str, worktree: &Worktree) {
    if let Some(dir) = crate::config::get_active_session_dir(session_id)
        && let Ok(text) = serde_json::to_string_pretty(worktree)
    {
        let _ = std::fs::create_dir_all(&dir);
        let _ = write_atomic(&dir.join(THREAD_FILE), text.as_bytes());
    }
    let mut record = load_record(&worktree.path).unwrap_or_else(|| Record {
        worktree: worktree.clone(),
        session_ids: Vec::new(),
    });
    if !record.session_ids.iter().any(|id| id == session_id) {
        record.session_ids.push(session_id.to_owned());
        save_record(&record);
    }
}

/// Move the threads of a removed worktree back to its repository and forget
/// the worktree on them, so each still opens.
fn move_threads_to_repo(session_ids: &[String], repo: &Path) {
    for session_id in session_ids {
        if crate::acp::session_move::move_session(session_id, repo.to_path_buf()).is_ok() {
            detach(session_id);
        }
    }
}

/// Forget the worktree `session_id` ran in: its folder is gone, or the thread
/// moved to the repository it came from.
pub(crate) fn detach(session_id: &str) {
    if let Some(dir) = crate::config::get_active_session_dir(session_id) {
        let _ = std::fs::remove_file(dir.join(THREAD_FILE));
    }
}

/// A thread may still point at a worktree an older release removed without
/// moving it. When the folder is gone and the worktree's repository is still
/// there, move the thread back to it and answer the folder to open instead.
/// Nothing else is guessed: a missing folder with no living repository stays
/// the error it was.
pub(crate) fn recover_missing_worktree(session_id: &str, cwd: &Path) -> Option<std::path::PathBuf> {
    if cwd.is_dir() {
        return None;
    }
    let worktree = of_session(session_id)?;
    let same = rustcode_session::canonicalize_best_effort(&worktree.path)
        == rustcode_session::canonicalize_best_effort(cwd);
    if !same || !worktree.repo.is_dir() {
        return None;
    }
    crate::acp::session_move::move_session(session_id, worktree.repo.clone()).ok()?;
    detach(session_id);
    Some(worktree.repo)
}

/// The worktree `session_id` runs in, if it was started in one.
pub(crate) fn of_session(session_id: &str) -> Option<Worktree> {
    if !rustcode_session::valid_session_id(session_id) {
        return None;
    }
    let dir = crate::config::get_active_session_dir(session_id)?;
    let text = std::fs::read_to_string(dir.join(THREAD_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// `{path, branch, repo}` of a thread's worktree, for `session/list`.
pub(crate) fn list_meta(session_id: &str) -> Option<Value> {
    of_session(session_id).map(|worktree| {
        json!({ "path": worktree.path, "branch": worktree.branch, "repo": worktree.repo })
    })
}

/// `session/new` with `_meta.rustcode.worktree`: create the worktree and
/// make its path the session's cwd. Other requests pass through.
pub(crate) fn prepare_new(
    mut request: agent_client_protocol::schema::v1::NewSessionRequest,
) -> Result<
    (
        agent_client_protocol::schema::v1::NewSessionRequest,
        Option<Worktree>,
    ),
    agent_client_protocol::Error,
> {
    let Some(asked) = requested(request.meta.as_ref()) else {
        return Ok((request, None));
    };
    let worktree = create(&asked).map_err(WorktreeError::into_acp)?;
    request.cwd = worktree.path.clone();
    Ok((request, Some(worktree)))
}

/// The thread metadata v2 adds to `session/new` and `session/load`
/// answers: `worktree`, `status`, `checkpoints` (whether the cwd is in a git
/// repository), the last also in the harness capabilities.
fn thread_meta(
    session_id: &str,
    meta: Option<serde_json::Map<String, Value>>,
) -> serde_json::Map<String, Value> {
    let mut meta = meta;
    if let Some(worktree) = of_session(session_id) {
        meta = Some(merge_rustcode_meta(meta, "worktree", json!(worktree)));
    }
    let status = crate::thread_status::current(session_id).as_str();
    let mut meta = merge_rustcode_meta(meta, "status", json!(status));
    let pinned = crate::thread_status::pinned(session_id);
    meta = merge_rustcode_meta(Some(meta), "pinned", json!(pinned));
    // The repository the thread's folder belongs to: a main checkout and every
    // worktree of it are one project for the orchestration tools (#3150).
    if let Some(repo) = crate::checkpoints::thread_cwd(session_id)
        .as_deref()
        .and_then(repo_of)
    {
        meta = merge_rustcode_meta(Some(meta), "repo", json!(repo));
    }
    if let Some(cwd) = crate::checkpoints::thread_cwd(session_id) {
        let available = crate::checkpoints::available(&cwd);
        meta = merge_rustcode_meta(Some(meta), "checkpoints", json!(available));
        if let Some(capabilities) = meta
            .get_mut("rustcode")
            .and_then(|rustcode| rustcode.get_mut("harness"))
            .and_then(|harness| harness.get_mut("capabilities"))
            .and_then(Value::as_object_mut)
        {
            capabilities.insert("checkpoints".into(), json!(available));
        }
    }
    meta
}

/// The `session/new` answer, with the worktree recorded on the thread.
pub(crate) fn finish_new(
    mut response: agent_client_protocol::schema::v1::NewSessionResponse,
    worktree: Option<&Worktree>,
) -> agent_client_protocol::schema::v1::NewSessionResponse {
    let session_id = response.session_id.to_string();
    if let Some(worktree) = worktree {
        attach(&session_id, worktree);
    }
    response.meta = Some(thread_meta(&session_id, response.meta.take()));
    response
}

/// The `session/load` answer, with the thread's worktree and status.
pub(crate) fn finish_load(
    mut response: agent_client_protocol::schema::v1::LoadSessionResponse,
    session_id: &str,
) -> agent_client_protocol::schema::v1::LoadSessionResponse {
    response.meta = Some(thread_meta(session_id, response.meta.take()));
    response
}

/// Put `key: value` under `_meta.rustcode` of `meta`.
pub(crate) fn merge_rustcode_meta(
    meta: Option<serde_json::Map<String, Value>>,
    key: &str,
    value: Value,
) -> serde_json::Map<String, Value> {
    let mut meta = meta.unwrap_or_default();
    let rustcode = meta
        .entry("rustcode".to_owned())
        .or_insert_with(|| json!({}));
    if !rustcode.is_object() {
        *rustcode = json!({});
    }
    if let Some(object) = rustcode.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
    meta
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A repository with one commit, in a fresh temporary directory.
    pub(crate) fn repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join(format!(
            "repo-{}",
            std::process::id() as u64 * 1000 + rand_suffix()
        ));
        std::fs::create_dir_all(&repo).unwrap();
        let repo = std::fs::canonicalize(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(&repo, &args).unwrap();
        }
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "init"]).unwrap();
        (dir, repo)
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
            + std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos() as u64
    }

    #[test]
    fn branch_names_come_from_the_title() {
        assert_eq!(branch_from_title("Add the login form"), "feat/login-form");
        assert_eq!(branch_from_title("Fix crash on save!"), "fix/crash-save");
        assert_eq!(branch_from_title("Docs for the API"), "docs/api");
        assert_eq!(branch_from_title("login form"), "feat/login-form");
        assert_eq!(branch_from_title(""), "feat/thread");
        assert_eq!(branch_from_title("Ärger über 💥"), "feat/rger-ber");
        assert!(branch_from_title(&"word ".repeat(40)).len() <= 45);
    }

    /// A repository's main checkout and every worktree of it are one project: a
    /// thread in a worktree has to reach the repository's other threads (#3150).
    #[test]
    fn a_worktree_and_its_main_checkout_are_one_repository() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo_of(&repo), Some(repo.clone()));
        assert_eq!(repo_of(&worktree.path), Some(repo.clone()));
        // A folder inside either is the same repository, and git resolves it again
        // for a nested directory.
        std::fs::create_dir_all(worktree.path.join("src")).unwrap();
        assert_eq!(repo_of(&worktree.path.join("src")), Some(repo.clone()));
        // Two worktrees of one repository meet here too.
        let sibling = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Fix the logout form".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(repo_of(&sibling.path), Some(repo.clone()));
        // A folder that is not a repository, or is gone, has none: the caller
        // falls back to comparing the paths.
        assert_eq!(repo_of(_dir.path()), None);
        assert_eq!(repo_of(&_dir.path().join("absent")), None);
    }

    #[test]
    fn create_list_and_deduplicate() {
        let (_dir, repo) = repo();
        let first = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(first.branch, "feat/login-form");
        assert!(first.path.join("README.md").is_file());
        assert!(
            first
                .path
                .starts_with(std::fs::canonicalize(root().unwrap()).unwrap())
        );
        assert_eq!(first.base_ref.as_deref(), Some("main"));
        let second = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(second.branch, "feat/login-form-2");
        assert_ne!(first.path, second.path);

        attach("worktree-test-session", &first);
        assert_eq!(of_session("worktree-test-session"), Some(first.clone()));
        std::fs::write(first.path.join("new.txt"), "x").unwrap();
        let rows = list(&repo).unwrap();
        assert_eq!(rows.len(), 2);
        let row = rows
            .iter()
            .find(|row| row["branch"] == "feat/login-form")
            .unwrap();
        assert_eq!(row["dirty"], true);
        assert_eq!(row["session_ids"], json!(["worktree-test-session"]));
        // Listing from inside a worktree finds the same repository.
        assert_eq!(list(&first.path).unwrap().len(), 2);
    }

    /// How many worktrees git has registered for `repo`: the main checkout
    /// counts as one.
    fn worktree_count(repo: &Path) -> usize {
        git(repo, &["worktree", "list", "--porcelain"])
            .unwrap()
            .split("\n\n")
            .filter(|block| !block.trim().is_empty())
            .count()
    }

    /// Make the checkout step of `git worktree add` fail in `repo`: git
    /// registers the worktree and checks it out, then the hook exits 1. A real
    /// setup failure (a hook, a checkout that cannot finish) leaves the same
    /// pieces behind: the new branch, the registration and the directory.
    #[cfg(unix)]
    fn fail_checkout(repo: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo.join(".git/hooks/post-checkout");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_add_removes_the_branch_and_worktree_it_made() {
        let (_dir, repo) = repo();
        fail_checkout(&repo);
        let error = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap_err();
        // git's own failure text, not the cleanup's.
        assert!(
            error.message.starts_with("git worktree add failed: "),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("Preparing worktree"),
            "{}",
            error.message
        );
        // Both the branch and the worktree its failed checkout held are gone.
        assert!(!branch_exists(&repo, "feat/login-form"));
        assert_eq!(worktree_count(&repo), 1);
        let parent = root().unwrap().join(repo_name(&repo));
        let leftovers: Vec<String> = std::fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // The failed attempt freed the name, so the retry is not `-2`.
        std::fs::remove_file(repo.join(".git/hooks/post-checkout")).unwrap();
        let retried = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(retried.branch, "feat/login-form");
        assert!(retried.path.join("README.md").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_branch_that_existed_before_survives_a_failed_add() {
        let (_dir, repo) = repo();
        git(&repo, &["branch", "feat/taken"]).unwrap();
        let tip = git_line(&repo, &["rev-parse", "feat/taken"]).unwrap();
        fail_checkout(&repo);
        let error = create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/taken".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(error.message.starts_with("git worktree add failed: "));
        assert!(branch_exists(&repo, "feat/taken"));
        assert_eq!(git_line(&repo, &["rev-parse", "feat/taken"]).unwrap(), tip);
        assert_eq!(worktree_count(&repo), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_branch_with_a_commit_survives_a_failed_add() {
        let (_dir, repo) = repo();
        git(&repo, &["switch", "-q", "-c", "feat/keep"]).unwrap();
        std::fs::write(repo.join("keep.txt"), "keep\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "keep"]).unwrap();
        git(&repo, &["switch", "-q", "main"]).unwrap();
        let tip = git_line(&repo, &["rev-parse", "feat/keep"]).unwrap();
        assert_ne!(tip, git_line(&repo, &["rev-parse", "main"]).unwrap());
        fail_checkout(&repo);
        create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/keep".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(branch_exists(&repo, "feat/keep"));
        assert_eq!(git_line(&repo, &["rev-parse", "feat/keep"]).unwrap(), tip);
        assert_eq!(worktree_count(&repo), 1);
    }

    #[test]
    fn cleanup_leaves_another_branch_worktree_at_the_path() {
        let (dir, repo) = repo();
        let path = dir.path().join("shared-path");
        git(&repo, &["branch", "feat/other"]).unwrap();
        git(
            &repo,
            &["worktree", "add", path.to_str().unwrap(), "feat/other"],
        )
        .unwrap();
        // A colliding slug is not enough: the worktree has to be on our
        // branch to be ours.
        remove_leftover_worktree(&repo, &path, "feat/mine").unwrap();
        assert!(path.exists());
        assert_eq!(worktree_count(&repo), 2);
        remove_leftover_worktree(&repo, &path, "feat/other").unwrap();
        assert!(!path.exists());
        assert_eq!(worktree_count(&repo), 1);
        assert!(branch_exists(&repo, "feat/other"));
    }

    #[test]
    fn cleanup_never_deletes_a_branch_that_moved() {
        let (_dir, repo) = repo();
        let base = git_line(&repo, &["rev-parse", "HEAD"]).unwrap();
        git(&repo, &["switch", "-q", "-c", "feat/moved"]).unwrap();
        std::fs::write(repo.join("moved.txt"), "m\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "moved"]).unwrap();
        git(&repo, &["switch", "-q", "main"]).unwrap();
        let moved = git_line(&repo, &["rev-parse", "feat/moved"]).unwrap();
        assert!(delete_created_branch(&repo, "feat/moved", &base).is_err());
        assert_eq!(
            git_line(&repo, &["rev-parse", "feat/moved"]).unwrap(),
            moved
        );
        // A branch that is not there is nothing to undo.
        assert!(delete_created_branch(&repo, "feat/absent", &base).is_ok());
    }

    #[test]
    fn cleanup_deletes_a_base_head_does_not_contain() {
        let (_dir, repo) = repo();
        // A commit on a branch `main` does not contain: `git branch -d`
        // refuses it, and the forced delete has to be safe by the base check.
        git(&repo, &["switch", "-q", "-c", "side"]).unwrap();
        std::fs::write(repo.join("side.txt"), "s\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "side"]).unwrap();
        let base = git_line(&repo, &["rev-parse", "HEAD"]).unwrap();
        git(&repo, &["switch", "-q", "main"]).unwrap();
        git(&repo, &["branch", "feat/side", &base]).unwrap();
        assert!(git(&repo, &["branch", "-d", "feat/side"]).is_err());
        delete_created_branch(&repo, "feat/side", &base).unwrap();
        assert!(!branch_exists(&repo, "feat/side"));
        assert!(branch_exists(&repo, "side"));
    }

    #[test]
    fn remove_is_guarded() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/guarded".into()),
            ..Default::default()
        })
        .unwrap();
        std::fs::write(worktree.path.join("wip.txt"), "x").unwrap();
        assert_eq!(remove(&worktree.path, false).unwrap_err().reason, "dirty");
        git(&worktree.path, &["add", "-A"]).unwrap();
        git(&worktree.path, &["commit", "-q", "-m", "wip"]).unwrap();
        assert_eq!(
            remove(&worktree.path, false).unwrap_err().reason,
            "unpushed"
        );
        // Forced: gone, and the unmerged branch is kept.
        assert_eq!(
            remove(&worktree.path, true).unwrap()["branch_removed"],
            false
        );
        assert!(!worktree.path.exists());
        assert!(branch_exists(&repo, "feat/guarded"));

        // A clean worktree with nothing new goes, and so does its branch.
        let clean = create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/clean".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(remove(&clean.path, false).unwrap()["branch_removed"], true);
        assert!(!branch_exists(&repo, "feat/clean"));
        // Never something RustCode did not make.
        assert_eq!(remove(&repo, true).unwrap_err().reason, "not_managed");
    }

    /// A history for `id`, so it is a thread the store knows, as every real
    /// one is before it can run in a worktree.
    fn seed_thread(id: &str) {
        let store = rustcode_session::SessionStore::new(crate::config::get_config_dir().unwrap());
        store.save_session_history(
            id,
            &vec![
                crate::app::ChatMessage::new("user", "Run in a worktree"),
                crate::app::ChatMessage::new("assistant", "Done"),
            ],
        );
        rustcode_session::flush_history();
    }

    #[test]
    fn removing_a_worktree_moves_its_threads_to_the_repository() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/moved-threads".into()),
            ..Default::default()
        })
        .unwrap();
        let session_id = crate::config::create_detached_session();
        seed_thread(&session_id);
        crate::config::save_session_workspace(
            &session_id,
            &rustcode_session::SessionWorkspace {
                cwd: worktree.path.clone(),
                additional_directories: Vec::new(),
                task_workspace_id: None,
            },
        )
        .unwrap();
        attach(&session_id, &worktree);

        assert_eq!(remove(&worktree.path, false).unwrap()["removed"], true);

        // The thread moved to the repository and no longer names the worktree.
        let moved = crate::config::load_session_workspace(&session_id).unwrap();
        assert_eq!(moved.cwd, repo);
        assert_eq!(of_session(&session_id), None);
        let dir = crate::config::get_active_session_dir(&session_id).unwrap();
        assert!(!dir.join(THREAD_FILE).exists());
    }

    #[test]
    fn a_thread_whose_worktree_is_gone_falls_back_to_its_repository() {
        let (dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            branch: Some("feat/broken-thread".into()),
            ..Default::default()
        })
        .unwrap();
        let session_id = crate::config::create_detached_session();
        seed_thread(&session_id);
        crate::config::save_session_workspace(
            &session_id,
            &rustcode_session::SessionWorkspace {
                cwd: worktree.path.clone(),
                additional_directories: Vec::new(),
                task_workspace_id: None,
            },
        )
        .unwrap();
        attach(&session_id, &worktree);
        // A worktree removed before this release: the folder is gone, the
        // thread still points at it.
        std::fs::remove_dir_all(&worktree.path).unwrap();

        // Only the worktree's own folder falls back, not any missing folder.
        assert!(recover_missing_worktree(&session_id, &dir.path().join("elsewhere")).is_none());
        assert_eq!(
            recover_missing_worktree(&session_id, &worktree.path),
            Some(repo.clone())
        );
        assert_eq!(
            crate::config::load_session_workspace(&session_id)
                .unwrap()
                .cwd,
            repo
        );
        assert_eq!(of_session(&session_id), None);
        // A folder that is still there is opened as it is.
        assert!(recover_missing_worktree(&session_id, &repo).is_none());
    }

    #[test]
    fn session_new_starts_in_a_worktree_and_answers_it() {
        use agent_client_protocol::schema::v1::{NewSessionRequest, NewSessionResponse};
        let (_dir, repo) = repo();
        let request = NewSessionRequest::new(repo.clone()).meta(serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            json!({ "worktree": { "repo": repo, "title": "Fix the flaky test" } }),
        )]));
        let (request, worktree) = prepare_new(request).unwrap();
        let worktree = worktree.unwrap();
        assert_eq!(request.cwd, worktree.path);
        assert_eq!(worktree.branch, "fix/flaky-test");
        let session_id = crate::config::create_detached_session();
        let _ = crate::config::save_session_workspace(
            &session_id,
            &rustcode_session::SessionWorkspace {
                cwd: worktree.path.clone(),
                additional_directories: Vec::new(),
                task_workspace_id: None,
            },
        );
        let response = finish_new(NewSessionResponse::new(session_id.clone()), Some(&worktree));
        let meta = response.meta.unwrap();
        assert_eq!(meta["rustcode"]["worktree"]["branch"], "fix/flaky-test");
        assert_eq!(meta["rustcode"]["status"], "idle");
        assert_eq!(meta["rustcode"]["checkpoints"], true);
        assert_eq!(
            list_meta(&session_id).unwrap()["path"],
            json!(worktree.path)
        );
        // A plain session/new is left alone.
        let (_, none) = prepare_new(NewSessionRequest::new(repo)).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn session_new_meta_and_merge() {
        let meta = serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            json!({ "worktree": { "repo": "/r", "title": "T" }, "harness": "codex" }),
        )]);
        let asked = requested(Some(&meta)).unwrap();
        assert_eq!(asked.repo, PathBuf::from("/r"));
        assert_eq!(asked.title.as_deref(), Some("T"));
        assert!(requested(None).is_none());
        let merged = merge_rustcode_meta(Some(meta), "status", json!("idle"));
        assert_eq!(merged["rustcode"]["harness"], "codex");
        assert_eq!(merged["rustcode"]["status"], "idle");
    }
}
