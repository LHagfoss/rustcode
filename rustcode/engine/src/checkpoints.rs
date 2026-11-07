//! Per-turn checkpoints (`docs/orchestrator.md`, *Checkpoints*).
//!
//! Before every turn of a thread, built-in or external, the engine commits
//! the thread's work tree (tracked and untracked, non-ignored files) to the
//! hidden ref `refs/rustcode/checkpoints/<session_id>/<turn>`. The commit is
//! built with a temporary index (`GIT_INDEX_FILE` + `git add -A` +
//! `write-tree` + `commit-tree` + `update-ref`), so the user's index, HEAD
//! and stash are never touched. Outside a git repository, past
//! [`SNAPSHOT_BUDGET`], or in a repository whose index is huge, the turn
//! simply has no checkpoint and a debug line says why.

use crate::worktrees::{git_with, toplevel};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const REF_PREFIX: &str = "refs/rustcode/checkpoints";
/// Checkpoints kept per thread; older ones are pruned.
pub(crate) const KEEP: usize = 50;
/// How long a snapshot may hold the turn's start.
pub(crate) const SNAPSHOT_BUDGET: Duration = Duration::from_secs(3);
/// An index this large means a repository too big to snapshot every turn.
const MAX_INDEX_BYTES: u64 = 48 * 1024 * 1024;
/// How long a diff or restore, which the user asked for, may take.
const REQUEST_BUDGET: Duration = Duration::from_secs(60);
const PROMPT_BYTES: usize = 4096;
const PATCH_BYTES: usize = 256 * 1024;
const PATCH_TOTAL_BYTES: usize = 2 * 1024 * 1024;
/// How long keeping the state a rewind abandons may take: the user asked for it.
const KEEP_BUDGET: Duration = Duration::from_secs(20);
const USER_INDEX_KEY: &str = "rustcode-user-index: ";
/// Marks a checkpoint that holds the state a rewind abandoned, not the state before a turn.
const ATTEMPT_KEY: &str = "rustcode-attempt: ";

/// One checkpoint, as `_rustcode/checkpoints/list` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Checkpoint {
    pub turn: u64,
    #[serde(rename = "ref")]
    pub reference: String,
    pub created_at: String,
    pub prompt: String,
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    /// Kept when a rewind abandoned the state this holds, rather than taken before a turn; it
    /// belongs to no turn of the chat, so nothing pairs a turn's changes with it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub attempt: bool,
    /// Which user message of the chat the turn's prompt became, for rewind.
    #[serde(skip)]
    pub user_index: Option<usize>,
    #[serde(skip)]
    pub full_prompt: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The repository a thread's cwd belongs to.
struct Repo {
    top: PathBuf,
    index: PathBuf,
}

fn ref_name(session_id: &str, turn: u64) -> String {
    format!("{REF_PREFIX}/{session_id}/{turn}")
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err("out of time".to_owned())
    } else {
        Ok(left)
    }
}

fn run(dir: &Path, args: &[&str], deadline: Instant) -> Result<String, String> {
    git_with(dir, args, &[], remaining(deadline)?)
}

fn run_indexed(
    dir: &Path,
    index: &Path,
    args: &[&str],
    deadline: Instant,
) -> Result<String, String> {
    git_with(
        dir,
        args,
        &[("GIT_INDEX_FILE", index.as_os_str())],
        remaining(deadline)?,
    )
}

fn repo(cwd: &Path, deadline: Instant) -> Option<Repo> {
    let out = run(
        cwd,
        &["rev-parse", "--show-toplevel", "--git-path", "index"],
        deadline,
    )
    .ok()?;
    let mut lines = out.lines();
    let top = std::fs::canonicalize(lines.next()?).ok()?;
    let index = PathBuf::from(lines.next()?);
    let index = if index.is_absolute() {
        index
    } else {
        cwd.join(index)
    };
    Some(Repo { top, index })
}

/// The tree of the work tree as it is now, built in a temporary index that
/// starts as a copy of the user's (so unchanged files are not re-hashed).
fn work_tree(repo: &Repo, deadline: Instant) -> Result<String, String> {
    let temporary = tempfile::Builder::new()
        .prefix("rustcode-checkpoint-")
        .tempfile()
        .map_err(|error| error.to_string())?;
    let index = temporary.path().to_path_buf();
    if repo.index.is_file() {
        std::fs::copy(&repo.index, &index).map_err(|error| error.to_string())?;
    } else {
        // An unborn repository: start from an empty index.
        let _ = std::fs::remove_file(&index);
    }
    run_indexed(&repo.top, &index, &["add", "-A", "--", "."], deadline)?;
    let tree = run_indexed(&repo.top, &index, &["write-tree"], deadline)?;
    let _ = std::fs::remove_file(&index);
    drop(temporary);
    Ok(tree.trim().to_owned())
}

/// The turns `session_id` has checkpoints for, oldest first.
fn turns(top: &Path, session_id: &str, deadline: Instant) -> Vec<u64> {
    let mut turns: Vec<u64> = run(
        top,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=4)",
            &format!("{REF_PREFIX}/{session_id}/"),
        ],
        deadline,
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|line| line.trim().parse().ok())
    .collect();
    turns.sort_unstable();
    turns
}

fn clean_prompt(prompt: &str) -> String {
    let mut text: String = prompt
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect();
    if text.len() > PROMPT_BYTES {
        let mut cut = PROMPT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    text.trim().to_owned()
}

/// Snapshot `cwd` as the checkpoint before the next turn of `session_id`.
/// `Ok(None)` when there is nothing to snapshot (not a git repository).
pub(crate) fn snapshot(
    cwd: &Path,
    session_id: &str,
    prompt: &str,
    user_index: Option<usize>,
    budget: Duration,
) -> Result<Option<u64>, String> {
    snapshot_with(cwd, session_id, prompt, user_index, false, budget)
}

/// Keep the work tree as the state a rewind is about to abandon, so the attempt it leaves can be
/// returned to (`_rustcode/checkpoints/restore` with `keep`). `Ok(None)` outside a repository.
pub(crate) fn keep(cwd: &Path, session_id: &str, budget: Duration) -> Result<Option<u64>, String> {
    snapshot_with(cwd, session_id, "", None, true, budget)
}

/// [`snapshot`], with whether it holds an abandoned attempt rather than the start of a turn.
fn snapshot_with(
    cwd: &Path,
    session_id: &str,
    prompt: &str,
    user_index: Option<usize>,
    attempt: bool,
    budget: Duration,
) -> Result<Option<u64>, String> {
    if !rustcode_session::valid_session_id(session_id) {
        return Ok(None);
    }
    let deadline = Instant::now() + budget;
    let Some(repo) = repo(cwd, deadline) else {
        return Ok(None);
    };
    if std::fs::metadata(&repo.index).is_ok_and(|meta| meta.len() > MAX_INDEX_BYTES) {
        return Err("the repository is too large to checkpoint".to_owned());
    }
    let tree = work_tree(&repo, deadline)?;
    let existing = turns(&repo.top, session_id, deadline);
    let turn = existing.last().map_or(1, |last| last + 1);
    let mut message = format!("rustcode checkpoint {turn}\n\n");
    if attempt {
        message.push_str(&format!("{ATTEMPT_KEY}1\n"));
    }
    let prompt = clean_prompt(prompt);
    if !prompt.is_empty() {
        message.push_str(&prompt);
        message.push_str("\n\n");
    }
    if let Some(user_index) = user_index {
        message.push_str(&format!("{USER_INDEX_KEY}{user_index}\n"));
    }
    let head = run(
        &repo.top,
        &["rev-parse", "--verify", "-q", "HEAD"],
        deadline,
    )
    .ok()
    .map(|head| head.trim().to_owned())
    .filter(|head| !head.is_empty());
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(head) = head.as_deref() {
        args.extend(["-p", head]);
    }
    args.extend(["-m", message.as_str()]);
    let identity: [(&str, &OsStr); 4] = [
        ("GIT_AUTHOR_NAME", OsStr::new("RustCode")),
        (
            "GIT_AUTHOR_EMAIL",
            OsStr::new("checkpoints@rustcode.invalid"),
        ),
        ("GIT_COMMITTER_NAME", OsStr::new("RustCode")),
        (
            "GIT_COMMITTER_EMAIL",
            OsStr::new("checkpoints@rustcode.invalid"),
        ),
    ];
    let commit = git_with(&repo.top, &args, &identity, remaining(deadline)?)?;
    let commit = commit.trim();
    // The zero old value: never overwrite a checkpoint that exists.
    run(
        &repo.top,
        &[
            "update-ref",
            &ref_name(session_id, turn),
            commit,
            "0000000000000000000000000000000000000000",
        ],
        deadline,
    )?;
    let mut kept = existing;
    kept.push(turn);
    // Pruning is housekeeping: it gets its own short budget.
    let prune_deadline = Instant::now() + Duration::from_secs(2);
    while kept.len() > KEEP {
        let oldest = kept.remove(0);
        let _ = run(
            &repo.top,
            &["update-ref", "-d", &ref_name(session_id, oldest)],
            prune_deadline,
        );
    }
    Ok(Some(turn))
}

/// [`snapshot`] off the async runtime, logged, never failing the turn.
pub(crate) async fn before_turn(
    session_id: &str,
    cwd: &Path,
    prompt: &str,
    user_index: Option<usize>,
) {
    // A test turn running in this checkout must not leave refs in it.
    #[cfg(test)]
    if !cwd.starts_with(std::env::temp_dir())
        && !std::fs::canonicalize(cwd).is_ok_and(|cwd| {
            cwd.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap_or_default())
        })
    {
        return;
    }
    let (session_id, cwd, prompt) = (session_id.to_owned(), cwd.to_path_buf(), prompt.to_owned());
    let started = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let result = snapshot(&cwd, &session_id, &prompt, user_index, SNAPSHOT_BUDGET);
        (session_id, result)
    })
    .await;
    match result {
        Ok((session_id, Ok(Some(turn)))) => crate::dbg_log!(
            "checkpoint {turn} of {session_id} in {} ms",
            started.elapsed().as_millis()
        ),
        Ok((_, Ok(None))) => {}
        Ok((session_id, Err(message))) => {
            crate::dbg_log!("checkpoint of {session_id} skipped: {message}")
        }
        Err(error) => crate::dbg_log!("checkpoint task failed: {error}"),
    }
}

/// How many user messages of `history` a replay shows: the index the next
/// prompt's message gets, which `_rustcode/rewind` takes.
pub(crate) fn next_user_index(history: &[crate::app::ChatMessage]) -> usize {
    history
        .iter()
        .filter(|message| message.role == "user" && !message.content.is_empty())
        .count()
}

// ------------------------------------------------------------- reading ---

fn numstat_totals(top: &Path, from: &str, to: &str, deadline: Instant) -> (u64, u64, u64) {
    let out = run(
        top,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            "--numstat",
            "-z",
            from,
            to,
        ],
        deadline,
    )
    .unwrap_or_default();
    numstat(&out)
        .iter()
        .fold((0, 0, 0), |(files, ins, del), (i, d)| {
            (files + 1, ins + i, del + d)
        })
}

/// `(insertions, deletions)` per file of `git diff --numstat -z`; binary
/// files count as 0.
fn numstat(out: &str) -> Vec<(u64, u64)> {
    let mut rows = Vec::new();
    let mut fields = out.split('\0').peekable();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, '\t');
        let insertions = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let deletions = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        // A rename leaves the path empty and names old and new next.
        if parts.next().is_some_and(str::is_empty) {
            fields.next();
            fields.next();
        }
        rows.push((insertions, deletions));
    }
    rows
}

fn parse_body(body: &str) -> (String, Option<usize>, bool) {
    let mut prompt = Vec::new();
    let mut user_index = None;
    let mut attempt = false;
    for line in body.lines() {
        if let Some(index) = line.strip_prefix(USER_INDEX_KEY) {
            user_index = index.trim().parse().ok();
        } else if line.strip_prefix(ATTEMPT_KEY).is_some() {
            attempt = true;
        } else {
            prompt.push(line);
        }
    }
    (prompt.join("\n").trim().to_owned(), user_index, attempt)
}

fn first_chars(text: &str, count: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(count).collect()
}

/// The thread's checkpoints, oldest first. Empty outside a git repository.
pub(crate) fn list(cwd: &Path, session_id: &str) -> Result<Vec<Checkpoint>, String> {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let Some(repo) = repo(cwd, deadline) else {
        return Ok(Vec::new());
    };
    let out = run(
        &repo.top,
        &[
            "for-each-ref",
            "--format=%(refname:lstrip=4)%00%(refname)%00%(committerdate:iso-strict)%00%(contents:body)%00%1e",
            &format!("{REF_PREFIX}/{session_id}/"),
        ],
        deadline,
    )?;
    let mut checkpoints: Vec<Checkpoint> = out
        .split('\u{1e}')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').split('\0');
            let turn = fields.next()?.trim().parse().ok()?;
            let reference = fields.next()?.to_owned();
            let created_at = fields.next()?.to_owned();
            let (full_prompt, user_index, attempt) = parse_body(fields.next().unwrap_or_default());
            Some(Checkpoint {
                turn,
                reference,
                created_at,
                prompt: first_chars(&full_prompt, 80),
                files_changed: 0,
                insertions: 0,
                deletions: 0,
                attempt,
                user_index,
                full_prompt,
            })
        })
        .collect();
    checkpoints.sort_by_key(|checkpoint| checkpoint.turn);
    let now = if checkpoints.is_empty() {
        None
    } else {
        work_tree(&repo, deadline).ok()
    };
    for index in 0..checkpoints.len() {
        let next = checkpoints
            .get(index + 1)
            .map(|next| next.reference.clone())
            .or_else(|| now.clone());
        if let Some(next) = next {
            let (files, insertions, deletions) =
                numstat_totals(&repo.top, &checkpoints[index].reference, &next, deadline);
            let checkpoint = &mut checkpoints[index];
            checkpoint.files_changed = files;
            checkpoint.insertions = insertions;
            checkpoint.deletions = deletions;
        }
    }
    Ok(checkpoints)
}

fn find(cwd: &Path, session_id: &str, turn: u64) -> Result<(Repo, Vec<u64>), String> {
    let deadline = Instant::now() + REQUEST_BUDGET;
    let repo =
        repo(cwd, deadline).ok_or_else(|| "the thread is not in a git repository".to_owned())?;
    let turns = turns(&repo.top, session_id, deadline);
    if !turns.contains(&turn) {
        return Err(format!("no checkpoint for turn {turn}"));
    }
    Ok((repo, turns))
}

fn status_word(code: &str) -> &'static str {
    match code.chars().next() {
        Some('A' | 'C') => "added",
        Some('D') => "deleted",
        Some('R') => "renamed",
        _ => "modified",
    }
}

/// What turn `turn` changed: its checkpoint against the next one, or the
/// work tree for the latest turn.
pub(crate) fn diff(cwd: &Path, session_id: &str, turn: u64) -> Result<Vec<Value>, String> {
    let (repo, turns) = find(cwd, session_id, turn)?;
    let deadline = Instant::now() + REQUEST_BUDGET;
    let from = ref_name(session_id, turn);
    let to = match turns.iter().find(|later| **later > turn) {
        Some(next) => ref_name(session_id, *next),
        None => work_tree(&repo, deadline)?,
    };
    let base = ["diff", "--no-ext-diff", "--no-textconv", "--no-color", "-M"];
    let with = |extra: &[&str]| {
        let mut args = base.to_vec();
        args.extend_from_slice(extra);
        args.extend([from.as_str(), to.as_str()]);
        run(&repo.top, &args, deadline)
    };
    let names = with(&["--name-status", "-z"])?;
    let stats = numstat(&with(&["--numstat", "-z"])?);
    let patch = with(&["--src-prefix=a/", "--dst-prefix=b/"])?;
    let mut patches = Vec::new();
    let mut current = String::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") && !current.is_empty() {
            patches.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        patches.push(current);
    }
    let mut fields = names.split('\0').filter(|field| !field.is_empty());
    let mut files = Vec::new();
    let mut total = 0usize;
    while let Some(code) = fields.next() {
        let renamed = code.starts_with('R') || code.starts_with('C');
        let first = fields.next().unwrap_or_default();
        let (path, old_path) = if renamed {
            (fields.next().unwrap_or_default(), Some(first))
        } else {
            (first, None)
        };
        let index = files.len();
        let (insertions, deletions) = stats.get(index).copied().unwrap_or((0, 0));
        let mut text = if patches.len() > index {
            patches[index].clone()
        } else {
            String::new()
        };
        let mut truncated = false;
        let limit = PATCH_BYTES.min(PATCH_TOTAL_BYTES.saturating_sub(total));
        if text.len() > limit {
            let mut cut = limit;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            truncated = true;
        }
        total += text.len();
        let mut file = json!({
            "path": path,
            "status": status_word(code),
            "insertions": insertions,
            "deletions": deletions,
            "patch": text,
        });
        if let Some(old_path) = old_path {
            file["old_path"] = json!(old_path);
        }
        if truncated {
            file["truncated"] = json!(true);
        }
        files.push(file);
    }
    Ok(files)
}

/// A path git reported, joined to the top only when it stays inside it.
fn inside(top: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then(|| top.join(relative))
}

/// Put the work tree back to the checkpoint before turn `turn`: files the
/// checkpoint has are written back, files it did not have (and that are not
/// ignored) are deleted. Ignored files, the index, HEAD and the stash are
/// left alone.
pub(crate) fn restore(cwd: &Path, session_id: &str, turn: u64) -> Result<(), String> {
    let (repo, _) = find(cwd, session_id, turn)?;
    let deadline = Instant::now() + REQUEST_BUDGET;
    let checkpoint = ref_name(session_id, turn);
    let now = work_tree(&repo, deadline)?;
    let names = |filter: &str| -> Result<Vec<String>, String> {
        let out = run(
            &repo.top,
            &[
                "diff",
                "--no-ext-diff",
                "--no-renames",
                "--name-only",
                "-z",
                filter,
                &checkpoint,
                &now,
            ],
            deadline,
        )?;
        Ok(out
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect())
    };
    let added = names("--diff-filter=A")?;
    let changed = names("--diff-filter=MDT")?;
    for name in &added {
        if let Some(path) = inside(&repo.top, name) {
            let _ = std::fs::remove_file(&path);
            // Directories the turn created and left empty go too.
            let mut parent = path.parent();
            while let Some(dir) = parent {
                if dir == repo.top || std::fs::remove_dir(dir).is_err() {
                    break;
                }
                parent = dir.parent();
            }
        }
    }
    if !changed.is_empty() {
        let temporary = tempfile::Builder::new()
            .prefix("rustcode-restore-")
            .tempfile()
            .map_err(|error| error.to_string())?;
        let index = temporary.path().to_path_buf();
        let _ = std::fs::remove_file(&index);
        run_indexed(&repo.top, &index, &["read-tree", &checkpoint], deadline)?;
        for chunk in changed.chunks(200) {
            let mut args = vec!["checkout-index", "-f", "--"];
            args.extend(chunk.iter().map(String::as_str));
            run_indexed(&repo.top, &index, &args, deadline)?;
        }
        let _ = std::fs::remove_file(&index);
    }
    Ok(())
}

/// Delete every checkpoint of `session_id` in the repository of `cwd`.
pub(crate) fn delete_session(cwd: &Path, session_id: &str) {
    if !rustcode_session::valid_session_id(session_id) {
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let Some(top) = toplevel(cwd) else {
        return;
    };
    for turn in turns(&top, session_id, deadline) {
        let _ = run(
            &top,
            &["update-ref", "-d", &ref_name(session_id, turn)],
            deadline,
        );
    }
}

/// Whether `cwd` can have checkpoints.
pub(crate) fn available(cwd: &Path) -> bool {
    toplevel(cwd).is_some()
}

// ------------------------------------------------------------ requests ---

/// The working directory a thread's checkpoints are taken in.
pub(crate) fn thread_cwd(session_id: &str) -> Option<PathBuf> {
    if !rustcode_session::valid_session_id(session_id) {
        return None;
    }
    crate::config::load_session_workspace(session_id)
        .map(|workspace| workspace.cwd)
        .or_else(|| crate::harness::store::load(session_id).map(|record| record.cwd))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/checkpoints/list", response = serde_json::Value)]
pub(crate) struct ListRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/checkpoints/diff", response = serde_json::Value)]
pub(crate) struct DiffRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub turn: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/checkpoints/restore", response = serde_json::Value)]
pub(crate) struct RestoreRequest {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub turn: u64,
    #[serde(default)]
    pub rewind: bool,
    /// Keep the work tree as an attempt checkpoint before restoring, so the state this abandons
    /// can be returned to; the answer names it in `kept`.
    #[serde(default)]
    pub keep: bool,
}

fn invalid(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.into())
}

fn cwd_of(session_id: &str) -> Result<PathBuf, agent_client_protocol::Error> {
    thread_cwd(session_id).ok_or_else(|| invalid(format!("unknown session: {session_id}")))
}

pub(crate) fn answer_list(request: &ListRequest) -> Result<Value, agent_client_protocol::Error> {
    let cwd = cwd_of(&request.session_id)?;
    let checkpoints = list(&cwd, &request.session_id).map_err(invalid)?;
    Ok(json!({ "checkpoints": checkpoints, "available": available(&cwd) }))
}

pub(crate) fn answer_diff(request: &DiffRequest) -> Result<Value, agent_client_protocol::Error> {
    let cwd = cwd_of(&request.session_id)?;
    Ok(json!({ "files": diff(&cwd, &request.session_id, request.turn).map_err(invalid)? }))
}

/// Restore the files, then rewind the chat when asked and `rewind` can:
/// it is given the open built-in thread's state, `None` for an external or
/// closed thread, whose conversation is kept. With `keep`, the state it is about to abandon is
/// checkpointed first, and answered as `kept`.
pub(crate) async fn answer_restore(
    request: &RestoreRequest,
    state: Option<std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>>,
) -> Result<Value, agent_client_protocol::Error> {
    if crate::thread_status::is_running(&request.session_id) {
        return Err(invalid("the thread is busy; restore once its turn ends"));
    }
    let cwd = cwd_of(&request.session_id)?;
    let checkpoint = list(&cwd, &request.session_id)
        .map_err(invalid)?
        .into_iter()
        .find(|checkpoint| checkpoint.turn == request.turn)
        .ok_or_else(|| invalid(format!("no checkpoint for turn {}", request.turn)))?;
    // Kept before anything changes: a failure here leaves the work tree and the chat as they are,
    // so the app can ask before restoring without a way back.
    let kept = if request.keep {
        let (cwd, session_id) = (cwd.clone(), request.session_id.clone());
        match tokio::task::spawn_blocking(move || keep(&cwd, &session_id, KEEP_BUDGET)).await {
            Ok(Ok(turn)) => turn,
            Ok(Err(message)) => {
                return Err(invalid(format!(
                    "the state this would abandon could not be kept: {message}"
                )));
            }
            Err(error) => {
                return Err(invalid(format!(
                    "the state this would abandon could not be kept: {error}"
                )));
            }
        }
    } else {
        None
    };
    let mut conversation = "kept";
    let mut rewind_error = None;
    if request.rewind
        && let Some(state) = state.as_ref()
        && let Some(user_index) = checkpoint.user_index
    {
        // Rewind first: it refuses a busy chat, and then nothing changed.
        let mut state = state.lock().await;
        match crate::acp::rewind_chat(&mut state, user_index, &checkpoint.full_prompt) {
            Ok(_) => conversation = "rewound",
            Err(error) => rewind_error = Some(error.to_string()),
        }
    }
    let (session_id, turn) = (request.session_id.clone(), request.turn);
    tokio::task::spawn_blocking(move || restore(&cwd, &session_id, turn))
        .await
        .map_err(|error| invalid(error.to_string()))?
        .map_err(invalid)?;
    let mut answer = json!({ "restored": true, "conversation": conversation });
    if let Some(turn) = kept {
        answer["kept"] = json!({ "turn": turn, "ref": ref_name(&request.session_id, turn) });
    }
    if let Some(error) = rewind_error {
        answer["rewind_error"] = json!(error);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktrees::{git, tests::repo};

    fn index_bytes(repo: &Path) -> Vec<u8> {
        std::fs::read(repo.join(".git/index")).unwrap()
    }

    fn take(repo: &Path, session: &str, prompt: &str) -> u64 {
        snapshot(repo, session, prompt, Some(0), Duration::from_secs(30))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn snapshots_list_and_diff_without_touching_the_index() {
        let (_dir, repo) = repo();
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&repo, &["add", "staged.txt"]).unwrap();
        let index_before = index_bytes(&repo);
        let head_before = git(&repo, &["rev-parse", "HEAD"]).unwrap();

        std::fs::write(repo.join("untracked.txt"), "one\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "secret\n").unwrap();
        assert_eq!(take(&repo, "cp-a", "first prompt"), 1);
        // The turn edits a file and adds one.
        std::fs::write(repo.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(repo.join("added.txt"), "new\n").unwrap();
        assert_eq!(take(&repo, "cp-a", "second prompt"), 2);
        std::fs::remove_file(repo.join("untracked.txt")).unwrap();

        assert_eq!(
            index_bytes(&repo),
            index_before,
            "the user's index is untouched"
        );
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap(), head_before);
        assert!(git(&repo, &["stash", "list"]).unwrap().is_empty());

        let listed = list(&repo, "cp-a").unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].prompt, "first prompt");
        assert_eq!(listed[0].user_index, Some(0));
        assert_eq!(listed[0].files_changed, 2);
        assert_eq!(listed[0].insertions, 2);
        // The latest turn is against the work tree: untracked.txt went.
        assert_eq!(listed[1].files_changed, 1);
        assert_eq!(listed[1].deletions, 1);

        let files = diff(&repo, "cp-a", 1).unwrap();
        let paths: Vec<_> = files
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["README.md", "added.txt"]);
        assert_eq!(files[0]["status"], "modified");
        assert_eq!(files[1]["status"], "added");
        assert!(files[0]["patch"].as_str().unwrap().contains("+world"));
        assert!(
            files[1]["patch"]
                .as_str()
                .unwrap()
                .starts_with("diff --git a/added.txt")
        );
        // Ignored files are never in a checkpoint.
        let tree = git(
            &repo,
            &["ls-tree", "-r", "--name-only", &ref_name("cp-a", 1)],
        )
        .unwrap();
        assert!(tree.contains("untracked.txt") && tree.contains("staged.txt"));
        assert!(!tree.contains("ignored.txt"));
        assert!(diff(&repo, "cp-a", 9).is_err());
    }

    #[test]
    fn restore_puts_files_back_and_keeps_ignored_ones() {
        let (_dir, repo) = repo();
        std::fs::write(repo.join("untracked.txt"), "keep me\n").unwrap();
        let index_before = index_bytes(&repo);
        take(&repo, "cp-b", "go");
        std::fs::write(repo.join("README.md"), "changed\n").unwrap();
        std::fs::remove_file(repo.join("untracked.txt")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join("src/deep/new.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.join("ignored.txt"), "build output\n").unwrap();

        restore(&repo, "cp-b", 1).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "hello\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("untracked.txt")).unwrap(),
            "keep me\n"
        );
        assert!(
            !repo.join("src").exists(),
            "new files and their empty dirs go"
        );
        assert!(
            repo.join("ignored.txt").exists(),
            "ignored files are never deleted"
        );
        assert_eq!(index_bytes(&repo), index_before);
    }

    #[test]
    fn keeps_the_last_fifty_and_deletes_with_the_thread() {
        let (_dir, repo) = repo();
        for _ in 0..(KEEP + 2) {
            take(&repo, "cp-c", "");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let kept = turns(&repo, "cp-c", deadline);
        assert_eq!(kept.len(), KEEP);
        assert_eq!(kept.first(), Some(&3));
        delete_session(&repo, "cp-c");
        assert!(turns(&repo, "cp-c", deadline).is_empty());
    }

    #[test]
    fn outside_git_there_is_nothing_to_do() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            snapshot(dir.path(), "cp-d", "x", None, SNAPSHOT_BUDGET).unwrap(),
            None
        );
        assert!(list(dir.path(), "cp-d").unwrap().is_empty());
        assert!(!available(dir.path()));
        assert!(restore(dir.path(), "cp-d", 1).is_err());
    }

    #[test]
    fn keeps_the_abandoned_state_and_restores_both_ways() {
        let (_dir, repo) = repo();
        std::fs::write(repo.join("README.md"), "v1\n").unwrap();
        assert_eq!(take(&repo, "cp-g", "one"), 1);
        std::fs::write(repo.join("README.md"), "v2\n").unwrap();
        assert_eq!(take(&repo, "cp-g", "two"), 2);
        std::fs::write(repo.join("README.md"), "v3\n").unwrap();

        // A rewind that keeps the state it leaves: the work tree as it is now.
        let kept = keep(&repo, "cp-g", Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert_eq!(kept, 3);
        // Back to before the first turn, which abandons both of them.
        restore(&repo, "cp-g", 1).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "v1\n"
        );

        // The kept state is listed, but belongs to no turn: it is the attempt's head.
        let listed = list(&repo, "cp-g").unwrap();
        let attempt = listed.iter().find(|c| c.turn == kept).unwrap();
        assert!(attempt.attempt);
        assert!(attempt.prompt.is_empty());
        assert!(listed.iter().filter(|c| c.turn < kept).all(|c| !c.attempt));

        // Going back to the attempt puts the files where the rewind left them.
        restore(&repo, "cp-g", kept).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "v3\n"
        );
    }

    #[test]
    fn a_spent_budget_skips_instead_of_waiting() {
        let (_dir, repo) = repo();
        let result = snapshot(&repo, "cp-e", "x", None, Duration::from_nanos(1));
        assert!(matches!(result, Ok(None) | Err(_)));
        assert!(turns(&repo, "cp-e", Instant::now() + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn unborn_repositories_and_numstat_renames() {
        let dir = tempfile::tempdir().unwrap();
        let repo = std::fs::canonicalize(dir.path()).unwrap();
        git(&repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        assert_eq!(take(&repo, "cp-f", "first"), 1);
        assert_eq!(numstat("1\t2\tpath\0-\t-\tbin\0"), vec![(1, 2), (0, 0)]);
        assert_eq!(numstat("3\t0\t\0old\0new\0"), vec![(3, 0)]);
        assert_eq!(
            parse_body("hello\nthere\n\nrustcode-user-index: 4\n"),
            ("hello\nthere".into(), Some(4), false)
        );
        assert_eq!(
            parse_body("rustcode-attempt: 1\n"),
            (String::new(), None, true)
        );
    }
}
