use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::pull_watch::Watch;

/// The project setting that merges a worktree thread's pull request once its
/// checks pass.
pub const SETTING: &str = "merge-when-green";

/// How many times a merge waits again after a failing check, for the thread
/// to push a fix.
pub const FIX_ROUNDS: u32 = 3;

/// How long one round waits for a fix to be pushed after a failing check.
pub const FIX_WAIT_SECS: u64 = 2 * 60 * 60;

/// How often a round waiting for a fix reads the pull request's head.
pub const FIX_POLL_SECS: u64 = 60;

/// Whether a newly linked pull request merges on its own: the thread's project
/// turned [`SETTING`] on, the thread runs in a worktree (which RustCode removes
/// and settles once its pull request merged), and the pull request is open.
pub fn wanted(setting: Option<&Value>, in_worktree: bool, watch: &Watch) -> bool {
    setting == Some(&Value::Bool(true))
        && in_worktree
        && watch.link
        && watch
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.state == "open")
}

/// The `_rustcode/pulls/watch/start` answer for a started watch. A link that
/// [`wanted`] says merges on its own starts that merge in the background; the
/// link tells the thread how it went.
pub fn linked(watch: Watch) -> Value {
    let setting = crate::acp::settings::value_for_session(SETTING, &watch.session_id);
    let in_worktree = crate::worktrees::of_session(&watch.session_id).is_some();
    if wanted(setting.as_ref(), in_worktree, &watch) {
        tokio::spawn(merge(watch.clone()));
    }
    json!({ "watch": watch })
}

/// Merge once every check passes. After a failing check, wait for a new head
/// (the thread's fix) and try again, up to [`FIX_ROUNDS`] times.
async fn merge(watch: Watch) {
    let cancel = tokio_util::sync::CancellationToken::new();
    let wait = Duration::from_secs(crate::pull_requests::MAX_MERGE_WAIT_SECS);
    for _ in 0..=FIX_ROUNDS {
        let before = head(&watch.cwd, &watch.repo, watch.number).await;
        let outcome = crate::pull_requests::merge_branch_when_green(
            &watch.cwd,
            Some(watch.number),
            None,
            wait,
            &cancel,
        )
        .await;
        match outcome {
            Ok(outcome) if outcome.state == "failing" || outcome.state == "pending" => {
                crate::dbg_log!(
                    "merge when green: #{} {}: {}",
                    watch.number,
                    outcome.state,
                    outcome.message
                );
                if outcome.state == "failing" && !new_head(&watch, before).await {
                    return;
                }
            }
            Ok(outcome) => {
                crate::dbg_log!("merge when green: #{} {}", watch.number, outcome.state);
                return;
            }
            Err(error) => {
                crate::dbg_log!(
                    "merge when green: #{} not merged: {}",
                    watch.number,
                    error.message
                );
                return;
            }
        }
    }
}

/// Wait for the pull request's head to move past `before`, at most
/// [`FIX_WAIT_SECS`]. False when it never moved or the pull request closed.
async fn new_head(watch: &Watch, before: Option<String>) -> bool {
    let polls = FIX_WAIT_SECS / FIX_POLL_SECS;
    for _ in 0..polls {
        tokio::time::sleep(Duration::from_secs(FIX_POLL_SECS)).await;
        let value =
            crate::pull_requests::watch_view_on(&watch.cwd, &watch.repo, watch.number).await;
        let Some(detail) = value.ok().as_ref().and_then(crate::pull_requests::detail) else {
            continue;
        };
        if detail.summary.state != crate::pull_requests::PullState::Open {
            return false;
        }
        if before.as_deref() != Some(detail.summary.head_sha.as_str()) {
            return true;
        }
    }
    false
}

async fn head(cwd: &Path, repo: &str, number: u64) -> Option<String> {
    let value = crate::pull_requests::watch_view_on(cwd, repo, number)
        .await
        .ok()?;
    crate::pull_requests::detail(&value).map(|detail| detail.summary.head_sha)
}

#[cfg(test)]
#[path = "pull_auto_merge_tests.rs"]
mod tests;
