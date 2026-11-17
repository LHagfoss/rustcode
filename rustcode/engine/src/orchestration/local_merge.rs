//! Engine-owned merges for hosted threads. Approval precedes the background job;
//! the job outlives the tool call and delivers one outcome as a follow-up turn.
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({"error": {"code": code, "message": message.into()}})
}

pub(super) async fn answer(
    caller: &str,
    args: &Value,
    asker: Option<super::ask_user::Asker>,
    cancel: &CancellationToken,
) -> Value {
    if let Err(why) = super::validate(super::MERGE_WHEN_GREEN, args) {
        return error("invalid", why);
    }
    if let Err(refusal) = super::local::local_machine(args) {
        return refusal;
    }
    let target = args
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or(caller);
    let Some(meta) = crate::config::session_meta_by_id(target) else {
        return error("not_found", "no thread has that id");
    };
    if !super::local::may_reach(caller, target, &meta) {
        return error("not_allowed", super::local::reach_refused());
    }
    let path = meta
        .workspace_cwd
        .clone()
        .unwrap_or_else(|| super::local::project_of(target, &meta).into());
    let probe = path.clone();
    let Some(branch) =
        tokio::task::spawn_blocking(move || crate::pull_requests::current_branch(&probe))
            .await
            .ok()
            .flatten()
    else {
        return error("not_found", "cannot read this thread's current branch");
    };
    let pull = match crate::pull_requests::for_branch(&path, &branch).await {
        Ok(Some(pull)) => pull,
        Ok(None) => return error("not_found", "no pull request matches this thread's branch"),
        Err(why) => return error(why.code, why.message),
    };
    if args
        .get("number")
        .and_then(Value::as_u64)
        .is_some_and(|number| number != pull.number)
    {
        return error(
            "not_allowed",
            "the PR number must match this thread's current branch",
        );
    }
    let Some(asker) = asker else {
        return error(
            "unavailable",
            "no permission channel is available to approve this merge; nothing was changed",
        );
    };
    if !crate::manage::approved(
        &asker,
        caller,
        &format!(
            "Squash-merge pull request #{} in {}",
            pull.number,
            path.display()
        ),
        args,
        cancel,
    )
    .await
    {
        return error("not_allowed", "merge was not approved; nothing was changed");
    }
    let number = pull.number;
    let wait = std::time::Duration::from_millis(
        args.get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(super::DEFAULT_MERGE_WAIT_MS)
            .clamp(1000, super::MAX_MERGE_WAIT_MS),
    );
    // Delegate calls retain their existing synchronous result contract. Only the
    // caller's numbered PR is handed off as a background merge.
    if args.get("number").is_none() {
        return match crate::pull_requests::merge_branch_when_green(
            &path,
            Some(number),
            Some(branch),
            wait,
            cancel,
        )
        .await
        {
            Ok(outcome) => {
                json!({"session_id": target, "number": outcome.number, "state": outcome.state, "merged": outcome.merged, "detail": outcome.message, "failing": outcome.failing, "url": outcome.url})
            }
            Err(why) => error(why.code, why.message),
        };
    }
    let caller = caller.to_owned();
    tokio::spawn(async move {
        let result = crate::pull_requests::merge_branch_when_green(
            &path,
            Some(number),
            Some(branch),
            wait,
            &CancellationToken::new(),
        )
        .await;
        let result = match result {
            Ok(outcome) => {
                json!({"number": outcome.number, "state": outcome.state, "merged": outcome.merged, "detail": outcome.message, "failing": outcome.failing, "url": outcome.url})
            }
            Err(why) => error(why.code, why.message),
        };
        super::local_threads::merge_notice(&caller, number, result).await;
    });
    json!({"number": number, "state": "watching", "detail": "Merge approved and handed to RustCode. End the turn; the result will wake this thread."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_merge_is_refused_before_approval_or_git() {
        let result = answer(
            "missing",
            &json!({"number": 0}),
            None,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result["error"]["code"], "invalid");
    }

    #[tokio::test]
    async fn missing_thread_does_not_start_a_merge() {
        let result = answer(
            "missing",
            &json!({"number": 3930}),
            None,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result["error"]["code"], "not_found");
    }
}
