//! The tools that change other threads, for an outside agent
//! (`docs/mcp-outside-agents.md`): `interrupt_session`, `update_session`,
//! `configure_session`, `merge_when_green`, `cleanup_worktree` and `create_loop`.
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
    let Some(meta) = crate::sessions::session_meta_by_id(target) else {
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
        super::CONFIGURE_SESSION => configure_session(caller, args).await,
        super::MERGE_WHEN_GREEN => merge_when_green(caller, cancel, args).await,
        super::CLEANUP_WORKTREE => cleanup_worktree(caller, args).await,
        super::LIST_SCHEDULED_TASKS => list_loops().await,
        super::CREATE_LOOP => create_loop(caller, args).await,
        super::UPDATE_LOOP => update_loop(caller, args).await,
        super::DELETE_SCHEDULED_TASK => delete_loop(args).await,
        super::RUN_SCHEDULED_TASK_NOW => run_loop(args).await,
        _ if crate::workflow::is_tool(tool) => match target_of(caller, args) {
            Ok(target) => {
                let mut workflow_args = args.clone();
                if let Some(fields) = workflow_args.as_object_mut() {
                    fields.remove("session_id");
                }
                match crate::workflow::answer(
                    &target,
                    None,
                    super::VERSION,
                    cancel,
                    tool,
                    &workflow_args,
                    false,
                )
                .await
                {
                    Ok(answer) => serde_json::from_str(&answer)
                        .unwrap_or_else(|reason| error("failed", reason.to_string())),
                    Err(reason) => error("failed", reason),
                }
            }
            Err(refusal) => refusal,
        },
        _ => return None,
    })
}

/// Save a Loop on this computer using the same scheduler as the desktop.
async fn create_loop(caller: &str, args: &Value) -> Value {
    // On the hub, a Loop may name a member: it is kept here and each run is
    // sent there, so its thread, harness and account are that machine's.
    let machine = super::outside_hub::hub_loop_machine(args);
    if machine.is_none()
        && let Err(refusal) = super::local::local_machine(args)
    {
        return refusal;
    }
    let field = |key: &str| args[key].as_str().map(|value| value.trim().to_owned());
    let Some(workspace) = field("project") else {
        return error(
            "invalid",
            "an outside agent must give the absolute project path",
        );
    };
    let thread_id = field("thread_id");
    if let Some(id) = thread_id.as_deref().filter(|_| machine.is_none()) {
        let Some(meta) = crate::sessions::session_meta_by_id(id) else {
            return error("not_found", "no local thread has that id");
        };
        if !may_reach(caller, id, &meta) {
            return error("not_allowed", reach_refused());
        }
        if super::local::project_of(id, &meta) != workspace {
            return error(
                "invalid",
                "thread_id must name a thread in the loop's project",
            );
        }
    }
    let mut harness = field("harness");
    let mut account = field("account");
    if harness.is_some() && machine.is_none() {
        let resolved = match super::local_threads::harness_for(harness.as_deref()) {
            Ok(resolved) => resolved,
            Err(refusal) => return refusal,
        };
        account = match super::local_threads::account_for(&resolved, account.as_deref()) {
            Ok(account) => account,
            Err(refusal) => return refusal,
        };
        harness = Some(resolved.id);
    }
    let request = crate::acp::loops::LoopsSaveRequest {
        id: None,
        name: field("name").unwrap_or_default(),
        workspace,
        prompt: field("prompt").unwrap_or_default(),
        schedule: args["schedule"].clone(),
        harness,
        account,
        model: field("model"),
        effort: field("effort"),
        read_only: args["read_only"].as_bool().unwrap_or(true),
        thread_id,
        script: None,
        // A webhook is turned on in the app, where its URL is shown once.
        webhook: false,
        rotate_webhook: false,
        machine: machine.clone(),
    };
    match crate::acp::loops::save(request).await {
        Ok(value) => {
            let saved = &value["loop"];
            json!({
                "id": saved["id"],
                "name": saved["name"],
                "schedule": saved["schedule"],
                "next_due_at": saved["nextDueAt"],
                "machine": saved["machine"],
            })
        }
        Err(reason) => error("target_unavailable", reason),
    }
}

/// List loops on this computer using the same scheduler as the desktop.
async fn list_loops() -> Value {
    match crate::acp::scheduled::list().await {
        Ok(value) => value,
        Err(reason) => error("target_unavailable", reason),
    }
}

/// Update, pause or resume an existing Loop on this computer.
async fn update_loop(caller: &str, args: &Value) -> Value {
    if let Err(refusal) = super::local::local_machine(args) {
        return refusal;
    }
    let field = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let Some(id) = field("id") else {
        return error("invalid", "id is required");
    };
    let thread_id = field("thread_id");
    if let Some(tid) = thread_id.as_deref() {
        let Some(meta) = crate::sessions::session_meta_by_id(tid) else {
            return error("not_found", "no local thread has that id");
        };
        if !may_reach(caller, tid, &meta) {
            return error("not_allowed", reach_refused());
        }
    }
    let mut harness = field("harness");
    let mut account = field("account");
    if harness.is_some() {
        let resolved = match super::local_threads::harness_for(harness.as_deref()) {
            Ok(resolved) => resolved,
            Err(refusal) => return refusal,
        };
        account = match super::local_threads::account_for(&resolved, account.as_deref()) {
            Ok(account) => account,
            Err(refusal) => return refusal,
        };
        harness = Some(resolved.id);
    }
    let schedule = args
        .get("schedule")
        .filter(|value| value.is_object())
        .cloned();
    match crate::acp::loops::patch(
        &id,
        args.get("paused").and_then(Value::as_bool),
        field("name"),
        field("prompt"),
        schedule,
        args.get("read_only").and_then(Value::as_bool),
        harness,
        account,
        field("model"),
        field("effort"),
        thread_id,
    )
    .await
    {
        Ok(value) => value,
        Err(reason) => error("target_unavailable", reason),
    }
}

/// Delete a Loop by id.
async fn delete_loop(args: &Value) -> Value {
    if let Err(refusal) = super::local::local_machine(args) {
        return refusal;
    }
    let Some(id) = args
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return error("invalid", "id is required");
    };
    match crate::acp::scheduled::delete(id.to_owned()).await {
        Ok(value) => value,
        Err(reason) => error("target_unavailable", reason),
    }
}

/// Trigger one run of a Loop now, keeping its schedule.
async fn run_loop(args: &Value) -> Value {
    if let Err(refusal) = super::local::local_machine(args) {
        return refusal;
    }
    let Some(id) = args
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return error("invalid", "id is required");
    };
    match crate::acp::scheduled::run(id.to_owned()).await {
        Ok(value) => value,
        Err(reason) => error("target_unavailable", reason),
    }
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
        crate::sessions::save_session_title(&target, title);
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
        "title": crate::sessions::load_session_title(&target).unwrap_or_default(),
        "pinned": crate::thread_status::pinned(&target),
        "status": status_word(&target),
    })
}

/// Reports the thread's harness, access, model and effort, and changes the
/// model or effort through the engine's own session controls, as the app's
/// pickers do (`local_threads::configure`).
async fn configure_session(caller: &str, args: &Value) -> Value {
    let target = match target_of(caller, args) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let mut args = args.clone();
    args["session_id"] = json!(target);
    super::local_threads::configure(caller, &args).await
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
