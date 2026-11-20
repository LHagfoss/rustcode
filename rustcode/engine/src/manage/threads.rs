//! `rc threads move|rename|pin|unpin` and `rc worktree …`
//! (#3821): the thread and worktree operations the app's own controls use,
//! from a terminal. Moving a thread needs the running server, which owns the
//! thread's harness process; the rest change the files the app watches.

use serde_json::{Value, json};
use std::path::Path;

/// Move a thread to another harness (and account or model) through the
/// running server, as the app's harness picker does.
pub async fn move_thread(
    session_id: &str,
    harness: &str,
    account: Option<&str>,
    model: Option<&str>,
) -> Result<Value, String> {
    let params = json!({
        "sessionId": session_id,
        "harness": harness,
        "account": account,
        "model": model,
    });
    match crate::server::ask_running("_rustcode/session/harness", params).await {
        crate::server::Asked::Answer(answer) => Ok(answer),
        crate::server::Asked::Refused(message) => Err(message),
        crate::server::Asked::NoServer(_) => Err(
            "no RustCode server is running, and a thread moves in the server that runs it; start the app or `rc server install`, then retry".to_owned(),
        ),
    }
}

/// Pin or unpin a thread in the app's list.
pub fn set_pinned(session_id: &str, pinned: bool) -> Result<(), String> {
    crate::thread_status::set_pinned(session_id, pinned)
}

/// The RustCode-managed worktrees of the repository at `repo`.
pub fn worktrees(repo: &Path) -> Result<Value, String> {
    crate::worktrees::list(repo)
        .map(|rows| json!({ "worktrees": rows }))
        .map_err(|error| error.message)
}

/// One worktree's branch, distance from what it tracks, changed files and
/// last commit.
pub fn worktree_state(path: &Path) -> Value {
    crate::worktrees::state(path)
}

/// Remove a RustCode-managed worktree. Without `force`, a worktree with
/// uncommitted or unpushed work is refused and kept.
pub fn remove_worktree(path: &Path, force: bool) -> Result<Value, String> {
    crate::worktrees::remove(path, force).map_err(|error| error.message)
}

/// `git worktree prune` in the repository: drop the records of worktrees whose
/// folders are gone. Files are never touched.
pub fn prune_worktrees(repo: &Path) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "prune", "--verbose"])
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(said.trim().to_owned())
}
