//! The tools that change other threads, for an outside agent
//! (`docs/mcp-outside-agents.md`): `interrupt_session`, `update_session`,
//! `configure_session`, `merge_when_green` and `cleanup_worktree`.
//!
//! The app hosts these for a thread because it holds the thread's worktree,
//! pull request and card. An outside agent has no thread and no app behind
//! it, so the engine runs them itself on the same operations the app asks of
//! it: `thread_status`, `pull_requests::merge_branch_when_green` and
//! `worktrees::cleanup_resolving_rename` (the bodies of `_rustcode/pulls/merge`
//! and `_rustcode/worktrees/cleanup`), and the loopback `session/cancel` the
//! app's stop button sends. The grant's scope and the user's approval are
//! decided before a call gets here (`outside::answer_with`), and every call is
//! recorded in the outside-agent audit there.
//!
//! An outside agent always names the thread (`session_id`): it has none of its
//! own to default to. Reach follows the thread tools': any thread of this
//! computer when "Let agents read threads in other projects" is on.

use std::time::Duration;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::local::{may_reach, reach_refused};

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

fn needs_app(what: &str) -> Value {
    error(
        "unavailable",
        format!(
            "{what} needs the RustCode app open; the engine cannot do it for an outside agent. Nothing was changed."
        ),
    )
}

/// The thread a call names, when this outside agent may act on it.
fn target_of(caller: &str, args: &Value) -> Result<String, Value> {
    let target = args
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let Some(target) = target else {
        return Err(error(
            "invalid",
            "session_id is required: an outside agent has no thread of its own to default to",
        ));
    };
    if let Err(remote) = super::local::local_machine(args) {
        return Err(remote);
    }
    let Some(meta) = crate::config::session_meta_by_id(target) else {
        return Err(error(
            "not_found",
            "no thread has that id; use list_sessions",
        ));
    };
    if !may_reach(caller, target, &meta) {
        return Err(error("not_allowed", reach_refused()));
    }
    Ok(target.to_owned())
}

/// Run one of the management tools. `None` when `tool` is not one of them.
pub(crate) async fn answer(
    caller: &str,
    cancel: &CancellationToken,
    tool: &str,
    args: &Value,
) -> Option<Value> {
    Some(match tool {
        super::INTERRUPT_SESSION => super::local_threads::interrupt(caller, args).await,
        super::UPDATE_SESSION => update_session(caller, args),
        super::CONFIGURE_SESSION => configure_session(caller, args),
        super::MERGE_WHEN_GREEN => merge_when_green(caller, cancel, args).await,
        super::CLEANUP_WORKTREE => cleanup_worktree(caller, args).await,
        _ => return None,
    })
}

/// The status words `update_session` speaks, from what the engine stores.
fn status_word(session_id: &str) -> &'static str {
    match crate::thread_status::current(session_id) {
        crate::thread_status::Status::Review => "review",
        crate::thread_status::Status::Done => "settled",
        _ => "active",
    }
}

fn update_session(caller: &str, args: &Value) -> Value {
    // Read state and snoozes live in the app's own store.
    if !matches!(args.get("unread"), None | Some(Value::Null)) {
        return needs_app("update_session with unread");
    }
    if args.get("snoozed_until").is_some() {
        return needs_app("update_session with snoozed_until");
    }
    let target = match target_of(caller, args) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    if let Some(title) = args.get("title").and_then(Value::as_str) {
        let title = title.trim();
        if title.is_empty() {
            return error("invalid", "title must not be empty");
        }
        crate::config::save_session_title(&target, title);
    }
    if let Some(pinned) = args.get("pinned").and_then(Value::as_bool)
        && let Err(reason) = crate::thread_status::set_pinned(&target, pinned)
    {
        return error("failed", format!("the pin was not changed: {reason}"));
    }
    if let Some(status) = args.get("status").and_then(Value::as_str) {
        let stored = match status {
            "settled" => "done",
            "review" => "review",
            _ => "idle",
        };
        if let Err(reason) = crate::thread_status::set(&target, stored) {
            return error("failed", format!("the status was not changed: {reason}"));
        }
    }
    json!({
        "session_id": target,
        "title": crate::config::load_session_title(&target).unwrap_or_default(),
        "pinned": crate::thread_status::pinned(&target),
        "status": status_word(&target),
    })
}

/// Reports the thread's harness, access, model and effort. Changing them
/// goes through the app's session controls, which the engine does not drive
/// for an outside agent.
fn configure_session(caller: &str, args: &Value) -> Value {
    let changes = ["model", "effort"]
        .iter()
        .any(|key| !matches!(args.get(*key), None | Some(Value::Null)));
    if changes {
        return needs_app("configure_session with model or effort");
    }
    let target = match target_of(caller, args) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let harness = crate::harness::store::load(&target)
        .map(|record| record.harness)
        .unwrap_or_else(|| crate::harness::BUILTIN_ID.to_owned());
    let access = crate::config::load_session_access(&target).unwrap_or_else(|| "ask".to_owned());
    let mut answer = json!({ "session_id": target, "harness": harness, "access": access });
    if let Some(model) = crate::config::load_session_model(&target) {
        answer["model"] = json!(model.profile);
        if let Some(effort) = model.reasoning_effort {
            answer["effort"] = json!(effort);
        }
    }
    answer
}

/// The worktree a thread works in: its path and the branch recorded for it.
fn worktree_of(target: &str) -> Result<(std::path::PathBuf, String), Value> {
    let Some(worktree) = crate::worktrees::list_meta(target) else {
        return Err(error(
            "not_found",
            "that thread has no RustCode worktree to merge or clean up",
        ));
    };
    let path = worktree
        .get("path")
        .and_then(Value::as_str)
        .map(std::path::PathBuf::from);
    let branch = worktree
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    match path {
        Some(path) => Ok((path, branch)),
        None => Err(error("not_found", "that thread's worktree has no path")),
    }
}

async fn merge_when_green(caller: &str, cancel: &CancellationToken, args: &Value) -> Value {
    if args.get("number").is_some() {
        return error(
            "invalid",
            "an outside agent merges the pull request of a thread: give session_id",
        );
    }
    let target = match target_of(caller, args) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let (path, branch) = match worktree_of(&target) {
        Ok(found) => found,
        Err(refusal) => return refusal,
    };
    let wait = Duration::from_millis(
        args.get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(super::DEFAULT_MERGE_WAIT_MS)
            .clamp(1000, super::MAX_MERGE_WAIT_MS),
    );
    // A pull request on the branch the worktree is on now, which may be one
    // the delegate renamed to after the worktree was recorded.
    let now_on = {
        let probe = path.clone();
        tokio::task::spawn_blocking(move || crate::pull_requests::current_branch(&probe))
            .await
            .ok()
            .flatten()
    };
    let asked = now_on.or(Some(branch).filter(|branch| !branch.is_empty()));
    match crate::pull_requests::merge_branch_when_green(&path, None, asked, wait, cancel).await {
        Ok(outcome) => {
            let mut answer = json!({
                "session_id": target,
                "number": outcome.number,
                "state": outcome.state,
                "merged": outcome.merged,
                "waited_secs": outcome.waited_secs,
                "detail": outcome.message,
            });
            if !outcome.url.is_empty() {
                answer["url"] = json!(outcome.url);
            }
            if !outcome.failing.is_empty() {
                answer["failing"] = json!(outcome.failing);
            }
            answer
        }
        Err(pull) => {
            let code = match pull.code {
                "not_found" => "not_found",
                "cancelled" => "cancelled",
                _ => "failed",
            };
            error(code, pull.message)
        }
    }
}

async fn cleanup_worktree(caller: &str, args: &Value) -> Value {
    let target = match target_of(caller, args) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let (path, branch) = match worktree_of(&target) {
        Ok(found) => found,
        Err(refusal) => return refusal,
    };
    // What the app reads before it asks for the cleanup: the pull request of
    // the branch the worktree is on now, so a squash merge is not mistaken
    // for lost work. When it cannot be read, cleanup decides from git alone.
    let probe = path.clone();
    let now_on = tokio::task::spawn_blocking(move || crate::pull_requests::current_branch(&probe))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| branch.clone());
    let pull = crate::pull_requests::for_branch(&path, &now_on)
        .await
        .ok()
        .flatten();
    let (head_sha, merged) = match &pull {
        Some(pull) => (Some(pull.head_sha.clone()), pull.merged),
        None => (None, false),
    };
    let recorded = Some(branch).filter(|branch| !branch.is_empty());
    let cleaned =
        crate::worktrees::cleanup_resolving_rename(path, recorded, head_sha, merged).await;
    let mut answer = match cleaned {
        Ok(Ok(value)) => value,
        Ok(Err(refusal)) => {
            let code = if refusal.reason == "unknown" {
                "unavailable"
            } else {
                "conflict"
            };
            return error(code, format!("{}: {}", refusal.reason, refusal.message));
        }
        Err(failure) => return error("failed", failure.to_string()),
    };
    answer["session_id"] = json!(target);
    let settle = args.get("settle").and_then(Value::as_bool).unwrap_or(true);
    if settle && crate::thread_status::set(&target, "done").is_ok() {
        answer["settled"] = json!(true);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(value: &Value) -> Option<&str> {
        value.pointer("/error/code").and_then(Value::as_str)
    }

    #[tokio::test]
    async fn every_tool_names_its_thread() {
        let cancel = CancellationToken::new();
        for tool in [
            super::super::INTERRUPT_SESSION,
            super::super::UPDATE_SESSION,
            super::super::CONFIGURE_SESSION,
            super::super::MERGE_WHEN_GREEN,
            super::super::CLEANUP_WORKTREE,
        ] {
            let answer = answer("outside:abcd1234", &cancel, tool, &json!({"title": "x"}))
                .await
                .expect("a management tool");
            assert_eq!(failure(&answer), Some("invalid"), "{tool}");
        }
    }

    #[tokio::test]
    async fn a_merge_by_number_alone_is_refused() {
        let answer = merge_when_green(
            "outside:abcd1234",
            &CancellationToken::new(),
            &json!({"number": 12}),
        )
        .await;
        assert_eq!(failure(&answer), Some("invalid"));
    }

    #[tokio::test]
    async fn an_unknown_thread_is_not_found() {
        let answer = answer(
            "outside:abcd1234",
            &CancellationToken::new(),
            super::super::CLEANUP_WORKTREE,
            &json!({"session_id": "no-such-thread-0000"}),
        )
        .await
        .expect("a management tool");
        assert_eq!(failure(&answer), Some("not_found"));
    }

    #[test]
    fn changes_only_the_app_can_make_are_said_so() {
        for args in [
            json!({"session_id": "t", "unread": true}),
            json!({"session_id": "t", "snoozed_until": "1h"}),
        ] {
            let answer = update_session("outside:abcd1234", &args);
            assert_eq!(failure(&answer), Some("unavailable"));
        }
        let answer = configure_session(
            "outside:abcd1234",
            &json!({"session_id": "t", "model": "x"}),
        );
        assert_eq!(failure(&answer), Some("unavailable"));
    }

    #[tokio::test]
    async fn other_tools_are_not_ours() {
        assert!(
            answer(
                "outside:abcd1234",
                &CancellationToken::new(),
                super::super::LIST_SESSIONS,
                &json!({}),
            )
            .await
            .is_none()
        );
    }
}
