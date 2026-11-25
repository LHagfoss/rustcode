//! Waking a parent thread with no app attached (#4382).
//!
//! The app wakes a thread when its delegates finish or a workflow run it
//! started ends (`apps/desktop/src/state/host.ts`). A thread on the background
//! server, a paired machine's engine or `rustcode -p` has no app, so the engine
//! does the same for what it owns: the links it ran (`engine: true`) and the
//! workflow runs it drove. It runs only while no app that hosts orchestration
//! is connected to this engine (`acp::clients::app_hosts_orchestration`), and
//! claims each event in the store first (`notifiedAt` on a link, `woken` on a
//! run), so an app that attaches later never repeats it.
//!
//! Finished links are gathered per parent for `wake-batch-seconds` from the
//! first one, then told in one turn, as the app batches them. A parent at Ask
//! access is not woken (nobody could approve its turn); it reads the result
//! with `wait_session` or `workflow_status`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const WAKE_MARK: &str = "<!-- rustcode:wake -->";
const WORKFLOW_MARK: &str = "<!-- rustcode:workflow -->";
/// The most of a delegate's answer a wake carries, as the app's
/// `WAKE_ANSWER_CHARS`.
const ANSWER_CHARS: usize = 1024;
const DEFAULT_BATCH_SECONDS: u64 = 60;

/// Links that settled and wait for their parent's wake, by parent.
static PENDING: LazyLock<Mutex<HashMap<String, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn app_hosts() -> bool {
    crate::acp::clients::app_hosts_orchestration()
}

fn batch_window() -> Duration {
    let seconds = crate::acp::settings::value("wake-batch-seconds")
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_BATCH_SECONDS)
        .min(600);
    Duration::from_secs(seconds)
}

fn find_link(id: &str) -> Option<Value> {
    super::local_threads::link_book()
        .get("links")
        .and_then(Value::as_array)?
        .iter()
        .find(|link| link.get("id").and_then(Value::as_str) == Some(id))
        .cloned()
}

/// Whether a settled link is news for its parent: it ended done or failed
/// and nobody told the parent yet.
pub(super) fn wakes_parent(link: &Value) -> bool {
    matches!(
        link.get("state").and_then(Value::as_str),
        Some("done" | "failed")
    ) && link.get("notifiedAt").is_none_or(Value::is_null)
}

/// An engine link settled: queue it for its parent's wake.
pub(super) fn link_settled(link_id: &str) {
    if app_hosts() || tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let Some(link) = find_link(link_id) else {
        return;
    };
    let Some(parent) = link.get("orchestrator").and_then(Value::as_str) else {
        return;
    };
    if !wakes_parent(&link) {
        return;
    }
    let parent = parent.to_owned();
    let first = {
        let mut pending = PENDING.lock().unwrap_or_else(|error| error.into_inner());
        let links = pending.entry(parent.clone()).or_default();
        links.push(link_id.to_owned());
        links.len() == 1
    };
    if first {
        tokio::spawn(async move {
            tokio::time::sleep(batch_window()).await;
            flush(&parent).await;
        });
    }
}

async fn flush(parent: &str) {
    let ids = PENDING
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(parent)
        .unwrap_or_default();
    if app_hosts() || !super::local_threads::wakes_without_app(parent) {
        return;
    }
    let claimed: Vec<Value> = ids.iter().filter_map(|id| claim(id)).collect();
    if claimed.is_empty() {
        return;
    }
    if let Err(why) = super::local_threads::deliver_notice(parent, &delegates_text(&claimed)).await
    {
        crate::dbg_log!("delegate wake for {parent} could not be delivered: {why}");
    }
}

/// Mark the link heard, once: the link as it was, when this call marked it.
fn claim(id: &str) -> Option<Value> {
    let at = now_ms();
    let claimed = std::cell::RefCell::new(None);
    let _ = crate::data_store::change_link(id, |mut link| {
        if wakes_parent(&link) {
            *claimed.borrow_mut() = Some(link.clone());
            link["notifiedAt"] = json!(at);
        }
        link
    });
    claimed.into_inner()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn duration(ms: i64) -> String {
    let seconds = (ms.max(0) / 1000) as u64;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {}s", seconds / 60, seconds % 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// The wake turn's text for settled delegate links, in the app's shape: a
/// first line naming the delegate, then one block per link.
pub(super) fn delegates_text(links: &[Value]) -> String {
    let text = |link: &Value, key: &str| {
        link.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let first = match links {
        [one] => format!("Delegate finished: {}", text(one, "targetTitle")),
        many => format!("Delegates finished: {}", many.len()),
    };
    let mut lines = vec![first];
    for link in links {
        let mut head = format!(
            "- {} · {} · link_id {} · session {}",
            text(link, "targetTitle"),
            text(link, "state"),
            text(link, "id"),
            text(link, "target"),
        );
        if let Some(worked) = link.get("workedMs").and_then(Value::as_i64) {
            head.push_str(&format!(" · worked {}", duration(worked)));
        }
        lines.push(head);
        let answer = match text(link, "state").as_str() {
            "failed" => text(link, "error"),
            _ => text(link, "answer"),
        };
        if !answer.is_empty() {
            lines.push(cut(&answer, ANSWER_CHARS));
        }
    }
    lines.push(
        "The rest is in read_session on each delegate's session or wait_session on its link_id. This message is the notification."
            .to_owned(),
    );
    format!("{}\n\n{WAKE_MARK}", lines.join("\n"))
}

/// The wake turn's text for an ended workflow run, from its summary, as the
/// app's `endNotice` writes it.
pub(super) fn workflow_text(run: &Value) -> String {
    let text = |key: &str| run.get(key).and_then(Value::as_str).unwrap_or("");
    let total = run
        .pointer("/calls/total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let failed = run
        .pointer("/calls/failed")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let name = match text("name") {
        "" => "run",
        name => name,
    };
    let status = text("status").replace('_', " ");
    let elapsed = run.get("elapsed_ms").and_then(Value::as_i64).unwrap_or(0);
    let workers = |n: u64| format!("{n} worker{}", if n == 1 { "" } else { "s" });
    let mut lines = vec![format!(
        "Workflow {name}: {status} · {} · {}",
        workers(total),
        duration(elapsed)
    )];
    if failed > 0 {
        lines.push(format!("{} failed.", workers(failed)));
    }
    if text("status") != "done" && !text("error").is_empty() {
        lines.push(format!("Error: {}", text("error")));
    }
    let what = if run.get("has_result").and_then(Value::as_bool) == Some(true) {
        "the result"
    } else {
        "the run"
    };
    lines.push(format!(
        "Read {what} with workflow_status (run_id {}).",
        text("run_id")
    ));
    format!("{}\n\n{WORKFLOW_MARK}", lines.join("\n"))
}

/// A workflow run of `thread` ended: tell the thread, when no app will.
pub(crate) fn run_ended(thread: &str, run_id: &str) {
    if app_hosts() || tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let Some(run) = crate::workflow::run_summary(thread, run_id) else {
        return;
    };
    if run.get("woken").and_then(Value::as_bool) == Some(true)
        || !super::local_threads::wakes_without_app(thread)
    {
        return;
    }
    let (thread, run_id) = (thread.to_owned(), run_id.to_owned());
    tokio::spawn(async move {
        if app_hosts() || !crate::workflow::claim_wake(&thread, &run_id).unwrap_or(false) {
            return;
        }
        if let Err(why) = super::local_threads::deliver_notice(&thread, &workflow_text(&run)).await
        {
            crate::dbg_log!("workflow wake for {thread} could not be delivered: {why}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_done_or_failed_link_nobody_was_told_of_wakes_its_parent() {
        assert!(wakes_parent(&json!({"state": "done"})));
        assert!(wakes_parent(
            &json!({"state": "failed", "notifiedAt": null})
        ));
        assert!(!wakes_parent(&json!({"state": "done", "notifiedAt": 5})));
        assert!(!wakes_parent(&json!({"state": "cancelled"})));
        assert!(!wakes_parent(&json!({"state": "working"})));
    }

    #[test]
    fn a_delegate_wake_names_each_link_and_ends_with_the_wake_mark() {
        let one = delegates_text(&[json!({
            "id": "p/1", "target": "t-1", "targetTitle": "Fix tests", "state": "done",
            "answer": "All green.", "workedMs": 125_000,
        })]);
        assert!(one.starts_with("Delegate finished: Fix tests\n"), "{one}");
        assert!(
            one.contains("link_id p/1 · session t-1 · worked 2m 5s"),
            "{one}"
        );
        assert!(one.contains("All green."));
        assert!(one.ends_with(WAKE_MARK));

        let long = "x".repeat(2000);
        let two = delegates_text(&[
            json!({"id": "p/1", "targetTitle": "A", "state": "done", "answer": long}),
            json!({"id": "p/2", "targetTitle": "B", "state": "failed", "error": "harness exited"}),
        ]);
        assert!(two.starts_with("Delegates finished: 2\n"), "{two}");
        assert!(two.contains("harness exited"));
        assert!(!two.contains(&"x".repeat(1100)));
    }

    #[test]
    fn a_workflow_wake_reads_like_the_apps_end_notice() {
        let text = workflow_text(&json!({
            "run_id": "wf-1-2", "name": "review", "status": "timed_out",
            "calls": {"total": 5, "failed": 1}, "elapsed_ms": 252_000,
            "error": "the run took longer than its limit", "has_result": false,
        }));
        assert!(
            text.starts_with("Workflow review: timed out · 5 workers · 4m 12s\n1 worker failed.\n"),
            "{text}"
        );
        assert!(text.contains("Error: the run took longer than its limit"));
        assert!(text.contains("Read the run with workflow_status (run_id wf-1-2)."));
        assert!(text.ends_with(WORKFLOW_MARK));
    }
}
