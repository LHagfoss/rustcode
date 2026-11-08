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

/// A lookup that only reads (which repository a folder belongs to) runs off
/// the create/remove path, so it must never hold a caller for long.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

fn git_line(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_line_within(dir, args, Duration::from_secs(60))
}

fn git_line_within(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    git_with(dir, args, &[], timeout).map(|out| out.trim().to_owned())
}

/// The top level of the git work tree holding `dir`.
pub(crate) fn toplevel(dir: &Path) -> Option<PathBuf> {
    toplevel_within(dir, Duration::from_secs(60))
}

fn toplevel_within(dir: &Path, timeout: Duration) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    let top = git_line_within(dir, &["rev-parse", "--show-toplevel"], timeout).ok()?;
    std::fs::canonicalize(top).ok()
}

/// The main checkout of the repository `dir` belongs to, so a worktree of
/// a worktree still files under the repository's name.
pub(crate) fn main_checkout(dir: &Path) -> Option<PathBuf> {
    main_checkout_within(dir, Duration::from_secs(60))
}

fn main_checkout_within(dir: &Path, timeout: Duration) -> Option<PathBuf> {
    // A folder that is gone is not a repository, and must not cost a git
    // process to learn that.
    if !dir.is_dir() {
        return None;
    }
    let common = git_line_within(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        timeout,
    )
    .ok()?;
    let common = std::fs::canonicalize(common).ok()?;
    if common.file_name().is_some_and(|name| name == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        toplevel_within(dir, timeout)
    }
}

/// How many folders `repo_of` remembers without asking git again.
const REPO_CACHE_LIMIT: usize = 512;

/// The repository a thread's folder belongs to, worktree or main checkout: the
/// one project a main checkout and every worktree of it share (#3150).
///
/// Asked once per folder, not once per thread, with a short timeout: this runs
/// when a thread is created, never from `session/list` (#3162). A folder that
/// is gone (or not a repository) answers `None` and the caller falls back to
/// the path.
pub(crate) fn repo_of(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    static SEEN: LazyLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if let Ok(seen) = SEEN.lock()
        && let Some(found) = seen.get(&key)
    {
        return found.clone();
    }
    let found = main_checkout_within(&key, LOOKUP_TIMEOUT);
    // A miss is not remembered: the folder may become a repository, and asking
    // again costs one short lookup rather than a permanently wrong answer.
    if found.is_some()
        && let Ok(mut seen) = SEEN.lock()
    {
        if seen.len() >= REPO_CACHE_LIMIT {
            seen.clear();
        }
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

/// Whether the worktree has uncommitted changes, and how far it is ahead of and
/// behind its upstream (or `base`). A git that cannot answer is an error, never
/// "clean": removal must not proceed on an unknown state.
fn status_counts(path: &Path, base: &str) -> Result<(bool, u64, u64), String> {
    let dirty = git(path, &["status", "--porcelain", "--untracked-files=normal"])
        .map(|out| !out.trim().is_empty())
        .map_err(|message| format!("git status failed: {message}"))?;
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
    .map_err(|message| format!("git rev-list failed: {message}"))
    .and_then(|line| {
        let mut parts = line.split_whitespace();
        match (parts.next(), parts.next()) {
            (Some(ahead), Some(behind)) => match (ahead.parse::<u64>(), behind.parse::<u64>()) {
                (Ok(ahead), Ok(behind)) => Ok((ahead, behind)),
                _ => Err(format!("git rev-list answered '{line}'")),
            },
            _ => Err(format!("git rev-list answered '{line}'")),
        }
    })?;
    Ok((dirty, ahead, behind))
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
        // A worktree whose git state cannot be read (a prunable folder, a
        // broken checkout) still exists and still lists: the row says its
        // state is unknown. Removal is where unknown refuses.
        let counts = status_counts(&path, &worktree.base);
        let (dirty, ahead, behind) = match &counts {
            Ok((dirty, ahead, behind)) => (json!(dirty), json!(ahead), json!(behind)),
            Err(message) => {
                crate::dbg_log!(
                    "[worktrees] {} has no readable state: {message}",
                    path.display()
                );
                (Value::Null, Value::Null, Value::Null)
            }
        };
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
            object.insert("dirty".into(), dirty);
            object.insert("ahead".into(), ahead);
            object.insert("behind".into(), behind);
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
        let (dirty, _, _) =
            status_counts(&path, base.as_deref().unwrap_or("HEAD")).map_err(|message| {
                WorktreeError {
                    reason: "unknown",
                    message,
                }
            })?;
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

// ---------------------------------------------------------- cleanup ----

/// A commit `git cherry` marks `+` for the base branch: its sha and subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CherryCommit {
    pub sha: String,
    pub subject: String,
}

/// What the worktree's state says about removing it once its pull request is
/// done (#3153). `safe` is only true when nothing in the worktree would be
/// lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CleanupDecision {
    pub safe: bool,
    /// `dirty` or `unmerged` when it is not safe.
    pub reason: &'static str,
    /// The commits `git cherry` says the base branch does not have.
    pub unique: Vec<CherryCommit>,
}

/// Whether a merged delegate's worktree may go. A dirty worktree never may.
/// The commits `git cherry` marks `+` are unique to the branch, so they need one
/// of two proofs that their content is already on the base: the branch's tree
/// matches it (`git diff --quiet main branch`, which is what a squash merge
/// leaves when nothing else landed), or `merged_safe` says every one of them is
/// part of the merged pull request's head (a squash merge whose local `main`
/// has not caught up). A commit made after the pull request merged is in
/// neither, so it still blocks the cleanup.
pub(crate) fn cleanup_decision(
    dirty: bool,
    unique: Vec<CherryCommit>,
    same_content: bool,
    merged_safe: bool,
) -> CleanupDecision {
    if dirty {
        return CleanupDecision {
            safe: false,
            reason: "dirty",
            unique,
        };
    }
    if unique.is_empty() || same_content || merged_safe {
        return CleanupDecision {
            safe: true,
            reason: "",
            unique,
        };
    }
    CleanupDecision {
        safe: false,
        reason: "unmerged",
        unique,
    }
}

/// Whether `branch` is one cleanup must never delete: the base it is compared
/// against, or the repository's default branch. The short forms catch
/// `origin/main` standing for `main`.
pub(crate) fn protected_branch(branch: &str, base: &str, default: Option<&str>) -> bool {
    fn short(name: &str) -> &str {
        name.rsplit('/').next().unwrap_or(name)
    }
    branch == base
        || branch == short(base)
        || default.is_some_and(|default| branch == default || branch == short(default))
}

/// Ignored paths that are ordinary build output, which cleanup may delete.
const IGNORED_BUILD_DIRS: [&str; 9] = [
    "target",
    "node_modules",
    "dist",
    "build",
    ".turbo",
    ".next",
    ".cache",
    "coverage",
    ".venv",
];
/// Ignored entries one refusal names; the rest are counted.
const MAX_IGNORED_LISTED: usize = 20;

/// The ref a worktree's branch is compared against for cleanup: the branch it
/// was made from when that still exists, else `main` or `master`, else the base
/// commit it recorded.
fn cleanup_base(repo: &Path, record: Option<&Record>) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(record) = record {
        if let Some(base_ref) = record.worktree.base_ref.clone() {
            candidates.push(base_ref);
        }
    }
    candidates.push("main".to_owned());
    candidates.push("master".to_owned());
    if let Some(record) = record {
        candidates.push(record.worktree.base.clone());
    }
    candidates.into_iter().find(|candidate| {
        !candidate.is_empty()
            && git_line(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{candidate}^{{commit}}"),
                ],
            )
            .is_ok()
    })
}

/// The commits `git cherry -v <base> <branch>` marks `+`: the ones the base
/// branch does not have, with their subjects. A git that cannot answer is an
/// error, never "no unique work".
fn cherry_commits(repo: &Path, base: &str, branch: &str) -> Result<Vec<CherryCommit>, String> {
    let lines = git(repo, &["cherry", "-v", base, branch])
        .map_err(|message| format!("git cherry failed: {message}"))?;
    let mut unique = Vec::new();
    for line in lines.lines() {
        let Some(rest) = line.strip_prefix("+ ") else {
            continue;
        };
        let mut parts = rest.splitn(2, ' ');
        let sha = parts.next().unwrap_or("");
        if sha.is_empty() {
            return Err(format!("git cherry answered '{line}'"));
        }
        unique.push(CherryCommit {
            sha: sha.to_owned(),
            subject: parts.next().unwrap_or("").trim().to_owned(),
        });
    }
    Ok(unique)
}

/// Whether the branch's tree is already the base's tree.
fn same_tree(repo: &Path, base: &str, branch: &str) -> Result<bool, String> {
    let tree = |reference: &str| git_line(repo, &["rev-parse", &format!("{reference}^{{tree}}")]);
    Ok(
        tree(base).map_err(|message| format!("git rev-parse failed: {message}"))?
            == tree(branch).map_err(|message| format!("git rev-parse failed: {message}"))?,
    )
}

/// Whether `sha` is part of `head`. A head git cannot resolve is not proof of
/// anything, so it answers false.
fn commit_reachable(repo: &Path, sha: &str, head: &str) -> bool {
    git_line(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{head}^{{commit}}"),
        ],
    )
    .is_ok()
        && git(repo, &["merge-base", "--is-ancestor", sha, head]).is_ok()
}

/// Ignored files in the worktree that are not known build output, which
/// `git worktree remove` would delete silently. `target/`, `node_modules/` and
/// the like are expected; `.env.local`, notes and credentials are not.
fn ignored_files(path: &Path) -> Result<Vec<String>, String> {
    let out = git(path, &["status", "--porcelain", "--ignored=matching"])
        .map_err(|message| format!("git status --ignored failed: {message}"))?;
    let mut others = std::collections::BTreeSet::new();
    let mut more = false;
    for line in out.lines() {
        let Some(entry) = line.strip_prefix("!! ") else {
            continue;
        };
        let entry = entry.trim_end_matches('/');
        // Any component counts: `apps/desktop/node_modules` and
        // `apps/desktop/src-tauri/target` are the same build output as the
        // root's, not a user file to refuse.
        if entry
            .split('/')
            .any(|component| IGNORED_BUILD_DIRS.contains(&component))
        {
            continue;
        }
        if others.len() >= MAX_IGNORED_LISTED {
            more = true;
            continue;
        }
        others.insert(entry.to_owned());
    }
    let mut listed: Vec<String> = others.into_iter().collect();
    if more {
        listed.push("...".to_owned());
    }
    Ok(listed)
}

/// The branch a worktree is on. A detached HEAD is its own refusal; a git that
/// cannot answer is `unknown`, never "not on a branch".
fn worktree_branch(path: &Path) -> Result<String, WorktreeError> {
    match git_line(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(branch) if !branch.is_empty() => Ok(branch),
        // `--quiet` exits 1 with no stderr for a detached HEAD.
        Ok(_) => Err(WorktreeError {
            reason: "detached",
            message: "the worktree is on a detached HEAD, so there is no branch to clean up"
                .to_owned(),
        }),
        Err(error) if error.trim().is_empty() => Err(WorktreeError {
            reason: "detached",
            message: "the worktree is on a detached HEAD, so there is no branch to clean up"
                .to_owned(),
        }),
        Err(error) => Err(WorktreeError {
            reason: "unknown",
            message: format!("git symbolic-ref failed: {error}"),
        }),
    }
}

/// Remove a merged delegate's worktree and branch (#3153), refusing when the
/// worktree is dirty, its commits are not on the base branch, it is on a
/// different branch than the caller named, or it holds ignored files that are
/// not build output. Unlike [`remove`], a squash merge is not mistaken for lost
/// work: the pull request's merged state (read by the caller) counts only for
/// the commits that are part of its head. The branch is deleted with `-D`,
/// which the decision above makes safe.
pub(crate) fn cleanup(
    path: &Path,
    expected_branch: Option<&str>,
    pr_head: Option<&str>,
    pr_merged: bool,
) -> Result<Value, WorktreeError> {
    let path = std::fs::canonicalize(path)
        .map_err(|error| WorktreeError::failed(format!("invalid path: {error}")))?;
    let inside = root()
        .and_then(|root| std::fs::canonicalize(root).ok())
        .is_some_and(|root| path.starts_with(&root) && path != root);
    if !inside {
        return Err(WorktreeError {
            reason: "not_managed",
            message: "only worktrees under RustCode's worktrees folder can be cleaned up"
                .to_owned(),
        });
    }
    let record = load_record(&path);
    let repo = main_checkout(&path).ok_or_else(|| WorktreeError {
        reason: "not_git",
        message: format!("{} is not a git worktree", path.display()),
    })?;
    let branch = worktree_branch(&path)?;
    // The caller cleaned up the worktree it read the pull request for. A
    // delegate that switched branches must not take the new one down with it.
    if let Some(expected) = expected_branch.filter(|branch| !branch.trim().is_empty())
        && expected != branch
    {
        return Err(WorktreeError {
            reason: "branch_changed",
            message: format!("the worktree is on {branch}, not {expected}; nothing was removed"),
        });
    }
    let Some(base) = cleanup_base(&repo, record.as_ref()) else {
        return Err(WorktreeError {
            reason: "no_base",
            message: "no base branch to compare the worktree against".to_owned(),
        });
    };
    let default_branch = git_line(&repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    if protected_branch(&branch, &base, default_branch.as_deref()) {
        return Err(WorktreeError {
            reason: "protected_branch",
            message: format!("{branch} is the base or default branch; it is never deleted"),
        });
    }
    let unknown = |message: String| WorktreeError {
        reason: "unknown",
        message,
    };
    let (dirty, _, _) = status_counts(&path, &base).map_err(unknown)?;
    let unique = cherry_commits(&repo, &base, &branch).map_err(unknown)?;
    let same_content = same_tree(&repo, &base, &branch).map_err(unknown)?;
    // The PR merged proves its content is on the base, not that a commit made
    // after its head is. Without a head to check against, only the other proofs count.
    let merged_safe = pr_merged
        && match pr_head.filter(|head| !head.trim().is_empty()) {
            Some(head) => unique
                .iter()
                .all(|commit| commit_reachable(&repo, &commit.sha, head)),
            None => unique.is_empty(),
        };
    let decision = cleanup_decision(dirty, unique, same_content, merged_safe);
    if !decision.safe {
        let message = match decision.reason {
            "dirty" => {
                "the worktree has uncommitted changes; commit or discard them first".to_owned()
            }
            _ => format!(
                "{} commit(s) are not on {base} and are not part of the merged pull request: {}",
                decision.unique.len(),
                decision
                    .unique
                    .iter()
                    .map(|commit| commit.subject.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };
        return Err(WorktreeError {
            reason: decision.reason,
            message,
        });
    }
    let ignored = ignored_files(&path).map_err(unknown)?;
    if !ignored.is_empty() {
        return Err(WorktreeError {
            reason: "ignored_files",
            message: format!(
                "the worktree holds ignored files that removing it would delete: {}",
                ignored.join(", ")
            ),
        });
    }
    let path_text = path.to_string_lossy().into_owned();
    if path.exists() {
        git(&repo, &["worktree", "remove", &path_text]).map_err(|message| {
            WorktreeError::failed(format!("git worktree remove failed: {message}"))
        })?;
    }
    let _ = git(&repo, &["worktree", "prune"]);
    // The threads that ran here move back to the repository before the branch goes.
    if let Some(record) = record.as_ref() {
        move_threads_to_repo(&record.session_ids, &repo);
    }
    let branch_removed = git(&repo, &["branch", "-D", &branch]).is_ok();
    if let Some(record) = record_path(&path) {
        let _ = std::fs::remove_file(record);
    }
    Ok(json!({
        "removed": true,
        "branch": branch,
        "branch_removed": branch_removed,
        "base": base,
        "commit_count": decision.unique.len(),
    }))
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
    // The worktree knows its repository without asking git, so record it on
    // the thread now: `session/list` reads it later (#3162).
    crate::config::save_session_repo(session_id, Some(&worktree.repo));
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

/// The repository recorded on a thread, without asking git: its session
/// metadata, else the worktree record beside it. `session/list` calls this for
/// every listed thread, so it must stay a file read (#3162).
pub(crate) fn recorded_repo(
    session_id: &str,
    metadata: Option<&rustcode_session::SessionMetadata>,
) -> Option<PathBuf> {
    if let Some(repo) = metadata.and_then(|metadata| metadata.repo.clone()) {
        return Some(repo);
    }
    of_session(session_id).map(|worktree| worktree.repo)
}

/// The repository recorded on a thread, resolving and recording it once when it
/// is missing: at creation, or the first load of a thread older than this
/// record. `session/list` never calls this.
pub(crate) fn session_repo(session_id: &str) -> Option<PathBuf> {
    if let Some(repo) = crate::config::load_session_repo(session_id) {
        return Some(repo);
    }
    if let Some(worktree) = of_session(session_id) {
        crate::config::save_session_repo(session_id, Some(&worktree.repo));
        return Some(worktree.repo);
    }
    let repo = repo_of(&crate::checkpoints::thread_cwd(session_id)?)?;
    crate::config::save_session_repo(session_id, Some(&repo));
    Some(repo)
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
    // worktree of it are one project for the orchestration tools (#3150). It
    // is recorded once here, so `session/list` reads it instead of running git
    // per thread (#3162).
    if let Some(repo) = session_repo(session_id) {
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

    fn cherry(subject: &str) -> CherryCommit {
        CherryCommit {
            sha: format!("{:040x}", subject.len()),
            subject: subject.to_owned(),
        }
    }

    fn tip_of(repo: &Path, branch: &str) -> String {
        git_line(repo, &["rev-parse", &format!("refs/heads/{branch}")]).unwrap()
    }

    #[test]
    fn a_cleanup_refuses_dirty_or_unmerged_work_and_allows_a_squash_merge() {
        let unique = || vec![cherry("feat(mcp): add the tool")];
        // Nothing to lose: nothing unique, an identical tree, or a merged PR.
        assert!(cleanup_decision(false, Vec::new(), false, false).safe);
        assert!(cleanup_decision(false, unique(), true, false).safe);
        let squash = cleanup_decision(false, unique(), false, true);
        assert!(squash.safe, "{squash:?}");
        assert_eq!(squash.unique, unique());
        // Uncommitted work never goes, whatever the PR says.
        let dirty = cleanup_decision(true, unique(), true, true);
        assert!(!dirty.safe);
        assert_eq!(dirty.reason, "dirty");
        // Unique commits, no matching content and no merged PR: refused, and it says which.
        let unmerged = cleanup_decision(false, unique(), false, false);
        assert!(!unmerged.safe);
        assert_eq!(unmerged.reason, "unmerged");
        assert_eq!(unmerged.unique.len(), 1);
    }

    #[test]
    fn a_cleanup_refuses_the_base_and_default_branches() {
        assert!(protected_branch("main", "main", None));
        assert!(protected_branch("main", "origin/main", Some("main")));
        assert!(protected_branch("trunk", "origin/main", Some("trunk")));
        assert!(!protected_branch("feat/login", "origin/main", Some("main")));
        assert!(!protected_branch("feat/main", "main", Some("main")));
    }

    #[test]
    fn a_cleanup_refuses_what_it_cannot_read_and_says_unknown() {
        let (_dir, repo) = repo();
        // A git that cannot answer is an error, never "no unique work".
        let error = cherry_commits(&repo, "no-such-base", "main").unwrap_err();
        assert!(error.contains("git cherry failed"), "{error}");
        assert!(same_tree(&repo, "no-such-base", "main").is_err());
    }

    #[test]
    fn a_cleanup_takes_the_worktree_it_was_asked_for_or_none() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        let refused =
            cleanup(&worktree.path, Some("feat/somewhere-else"), None, false).unwrap_err();
        assert_eq!(refused.reason, "branch_changed");
        assert!(worktree.path.exists());
        // The branch it is on is fine.
        cleanup(
            &worktree.path,
            Some(&worktree.branch),
            Some(&tip_of(&repo, &worktree.branch)),
            false,
        )
        .unwrap();
    }

    #[test]
    fn a_cleanup_allows_a_squash_merge_only_for_commits_in_the_pr_head() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        std::fs::write(worktree.path.join("login.txt"), "login\n").unwrap();
        git(&worktree.path, &["add", "-A"]).unwrap();
        git(&worktree.path, &["commit", "-q", "-m", "add login"]).unwrap();
        let head = tip_of(&repo, &worktree.branch);
        // The PR merged at this head, and its commit is part of it: the squash
        // merge GitHub did is enough, even with a stale local main.
        let answer = cleanup(&worktree.path, Some(&worktree.branch), Some(&head), true).unwrap();
        assert_eq!(answer["removed"], true);
        assert_eq!(answer["branch_removed"], true);
        assert_eq!(answer["commit_count"], 1);
        assert!(!worktree.path.exists());
        assert!(
            git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", worktree.branch)
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn a_cleanup_refuses_a_commit_made_after_the_pull_request_merged() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        std::fs::write(worktree.path.join("login.txt"), "login\n").unwrap();
        git(&worktree.path, &["add", "-A"]).unwrap();
        git(&worktree.path, &["commit", "-q", "-m", "add login"]).unwrap();
        let head = tip_of(&repo, &worktree.branch);
        std::fs::write(worktree.path.join("after.txt"), "after\n").unwrap();
        git(&worktree.path, &["add", "-A"]).unwrap();
        git(&worktree.path, &["commit", "-q", "-m", "after the merge"]).unwrap();
        // The PR merged at `head`; the newer commit is not part of it.
        let refused =
            cleanup(&worktree.path, Some(&worktree.branch), Some(&head), true).unwrap_err();
        assert_eq!(refused.reason, "unmerged");
        assert!(
            refused.message.contains("after the merge"),
            "{}",
            refused.message
        );
        assert!(worktree.path.exists());
    }

    #[test]
    fn a_cleanup_refuses_ignored_files_that_are_not_build_output() {
        let (_dir, repo) = repo();
        // Build output is ignored too, but cleanup may delete it.
        let gitignore = std::fs::read_to_string(repo.join(".gitignore")).unwrap();
        std::fs::write(
            repo.join(".gitignore"),
            format!("{gitignore}target/\nnode_modules/\ndist/\n"),
        )
        .unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["commit", "-q", "-m", "ignore target"]).unwrap();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        // A local secret is not build output and would be deleted silently.
        std::fs::write(worktree.path.join("ignored.txt"), "secret\n").unwrap();
        let refused = cleanup(&worktree.path, Some(&worktree.branch), None, false).unwrap_err();
        assert_eq!(refused.reason, "ignored_files");
        assert!(
            refused.message.contains("ignored.txt"),
            "{}",
            refused.message
        );
        assert!(worktree.path.exists());
        // Remove it and leave only build output: that may go.
        std::fs::remove_file(worktree.path.join("ignored.txt")).unwrap();
        for (dir, file) in [
            ("target/debug", "app"),
            ("apps/desktop/node_modules/left-pad", "index.js"),
            ("apps/desktop/dist/assets", "app.js"),
            ("apps/desktop/src-tauri/target/debug", "build"),
        ] {
            std::fs::create_dir_all(worktree.path.join(dir)).unwrap();
            std::fs::write(worktree.path.join(dir).join(file), "built\n").unwrap();
        }
        let answer = cleanup(
            &worktree.path,
            Some(&worktree.branch),
            Some(&tip_of(&repo, &worktree.branch)),
            false,
        )
        .unwrap();
        assert_eq!(answer["removed"], true);
    }

    #[test]
    fn a_list_keeps_a_worktree_whose_state_cannot_be_read() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        // A prunable checkout: git still knows the worktree, but its state
        // cannot be read. It is listed with null state rather than failing
        // the whole list; removal is where it refuses.
        std::fs::remove_dir_all(&worktree.path).unwrap();
        let rows = list(&repo).unwrap();
        let row = rows
            .iter()
            .find(|row| row["branch"] == worktree.branch.as_str())
            .unwrap_or_else(|| panic!("{} is still listed: {rows:?}", worktree.branch));
        assert_eq!(row["dirty"], Value::Null);
        assert_eq!(row["ahead"], Value::Null);
        assert_eq!(row["behind"], Value::Null);
    }

    #[test]
    fn a_cleanup_tells_a_detached_head_from_a_git_error() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        git(&worktree.path, &["checkout", "--detach", "-q"]).unwrap();
        let refused = cleanup(&worktree.path, Some(&worktree.branch), None, false).unwrap_err();
        assert_eq!(refused.reason, "detached");
        // A folder git cannot read is an unknown state, not a detached HEAD.
        let other = tempfile::tempdir().unwrap();
        assert_eq!(worktree_branch(other.path()).unwrap_err().reason, "unknown");
    }

    #[test]
    fn a_cleanup_refuses_unmerged_commits_and_a_dirty_worktree() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the logout form".into()),
            ..Default::default()
        })
        .unwrap();
        std::fs::write(worktree.path.join("logout.txt"), "logout\n").unwrap();
        git(&worktree.path, &["add", "-A"]).unwrap();
        git(&worktree.path, &["commit", "-q", "-m", "add logout"]).unwrap();
        let refused = cleanup(&worktree.path, Some(&worktree.branch), None, false).unwrap_err();
        assert_eq!(refused.reason, "unmerged");
        assert!(
            refused.message.contains("add logout"),
            "{}",
            refused.message
        );
        // A dirty worktree is refused even when the PR merged.
        std::fs::write(worktree.path.join("notes.txt"), "wip\n").unwrap();
        let refused = cleanup(
            &worktree.path,
            Some(&worktree.branch),
            Some(&tip_of(&repo, &worktree.branch)),
            true,
        )
        .unwrap_err();
        assert_eq!(refused.reason, "dirty");
        // Nothing was removed: the worktree and its branch are still there.
        assert!(worktree.path.exists());
        assert!(
            git(
                &repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{}", worktree.branch)
                ]
            )
            .is_ok()
        );
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
    fn a_folder_that_becomes_a_repository_is_asked_again() {
        // A miss is not cached for good: the folder may become a repository.
        let dir = tempfile::tempdir().unwrap();
        let path = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(repo_of(&path), None);
        git(&path, &["init", "-q", "-b", "main"]).unwrap();
        assert_eq!(repo_of(&path), Some(path.clone()));
    }

    #[test]
    fn a_new_thread_records_its_repository_so_listing_needs_no_git() {
        let (_dir, repo) = repo();
        let id = crate::config::create_detached_session();
        assert!(!id.is_empty(), "a test config dir holds sessions");
        crate::config::save_session_workspace(
            &id,
            &rustcode_session::SessionWorkspace {
                cwd: repo.clone(),
                ..Default::default()
            },
        )
        .unwrap();
        let answer = finish_new(
            agent_client_protocol::schema::v1::NewSessionResponse::new(id.clone()),
            None,
        );
        assert_eq!(crate::config::load_session_repo(&id), Some(repo.clone()));
        let meta = answer.meta.expect("thread meta");
        assert_eq!(meta["rustcode"]["repo"], json!(repo));
        // The record answers on its own, when the repository is gone and git
        // has nothing left to ask.
        std::fs::remove_dir_all(&repo).unwrap();
        assert_eq!(
            recorded_repo(&id, crate::config::load_session_metadata(&id).as_ref()),
            Some(repo)
        );
    }

    #[test]
    fn attaching_a_worktree_records_its_repository_on_the_thread() {
        let (_dir, repo) = repo();
        let worktree = create(&CreateRequest {
            repo: repo.clone(),
            title: Some("Add the login form".into()),
            ..Default::default()
        })
        .unwrap();
        let id = crate::config::create_detached_session();
        assert!(!id.is_empty(), "a test config dir holds sessions");
        attach(&id, &worktree);
        assert_eq!(crate::config::load_session_repo(&id), Some(repo));
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
