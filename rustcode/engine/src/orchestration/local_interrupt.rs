//! `interrupt_session` with no app attached (#4381): the engine stops a thread
//! it can reach and the links that thread is working through, as the app's
//! stop does (`docs/orchestrator.md` › *Stopping takes the tree*).
//!
//! A thread may stop a delegate it started itself at any access: that is how a
//! workflow run ends its workers on `stop_workflow` or a timeout. Stopping any
//! other thread changes work the caller does not own, so it needs Full access
//! like `message_session`. The target's own engine links, and theirs, are
//! stopped once each and settled `cancelled`; nothing is reported for them.

use std::collections::HashSet;

use serde_json::{Value, json};

/// A link's `state` while its turn may still run.
fn active(link: &Value) -> bool {
    matches!(
        link.get("state").and_then(Value::as_str),
        Some("queued" | "working" | "needs_user")
    )
}

fn text<'a>(link: &'a Value, key: &str) -> Option<&'a str> {
    link.get(key).and_then(Value::as_str)
}

fn links(book: &Value) -> &[Value] {
    book.get("links")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The active links `caller` holds on `target`.
pub(super) fn own_links(book: &Value, caller: &str, target: &str) -> Vec<String> {
    links(book)
        .iter()
        .filter(|link| {
            active(link)
                && text(link, "orchestrator") == Some(caller)
                && text(link, "target") == Some(target)
        })
        .filter_map(|link| text(link, "id").map(str::to_owned))
        .collect()
}

/// What stopping `target` takes with it: the threads below it that its active
/// engine links (and theirs) work through, each once, and those links.
pub(super) fn tree(book: &Value, target: &str) -> (Vec<String>, Vec<String>) {
    let mut seen: HashSet<String> = HashSet::from([target.to_owned()]);
    let mut queue = vec![target.to_owned()];
    let mut threads = Vec::new();
    let mut link_ids = Vec::new();
    while let Some(current) = queue.pop() {
        for link in links(book) {
            if !active(link)
                || link.get("engine").and_then(Value::as_bool) != Some(true)
                || text(link, "orchestrator") != Some(current.as_str())
            {
                continue;
            }
            if let Some(id) = text(link, "id") {
                link_ids.push(id.to_owned());
            }
            if let Some(below) = text(link, "target")
                && seen.insert(below.to_owned())
            {
                threads.push(below.to_owned());
                queue.push(below.to_owned());
            }
        }
    }
    (threads, link_ids)
}

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({ "error": { "code": code, "message": message.into() } })
}

pub(super) async fn answer(caller: &str, args: &Value) -> Value {
    if let Err(remote) = super::local::local_machine(args) {
        return remote;
    }
    let target = super::local::strip_own_host(
        args.get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim(),
    )
    .to_owned();
    if target.is_empty() {
        return error("invalid", "session_id is required");
    }
    if target == caller {
        return error("not_allowed", "a thread cannot stop itself");
    }
    let book = super::local_threads::link_book();
    let own = own_links(&book, caller, &target);
    if own.is_empty()
        && let Err(refusal) = super::local_threads::delegation(caller, "interrupt_session")
    {
        return refusal;
    }
    let mut answer = super::local_threads::interrupt(caller, args).await;
    if answer.get("error").is_some() {
        return answer;
    }
    let (below, below_links) = tree(&book, &target);
    if !below.is_empty()
        && let Err(refusal) = super::local_threads::cancel_turns(&below).await
    {
        return refusal;
    }
    for id in own.iter().chain(&below_links) {
        super::local_threads::stop_link(id);
    }
    answer["stopped"] = json!(below.len());
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(id: &str, from: &str, to: &str, state: &str, engine: bool) -> Value {
        json!({
            "id": id,
            "orchestrator": from,
            "target": to,
            "state": state,
            "engine": engine,
        })
    }

    #[test]
    fn a_caller_owns_only_its_active_links_on_the_target() {
        let book = json!({ "links": [
            link("p/1", "p", "w", "working", true),
            link("p/2", "p", "w", "done", true),
            link("q/1", "q", "w", "working", true),
            link("p/3", "p", "x", "working", true),
        ]});
        assert_eq!(own_links(&book, "p", "w"), vec!["p/1".to_owned()]);
        assert!(own_links(&book, "q", "x").is_empty());
        assert!(own_links(&json!({}), "p", "w").is_empty());
    }

    #[test]
    fn stopping_takes_the_engine_links_below_once_each() {
        let book = json!({ "links": [
            link("w/1", "w", "a", "working", true),
            link("w/2", "w", "b", "needs_user", true),
            link("a/1", "a", "c", "queued", true),
            link("b/1", "b", "c", "working", true),
            link("c/1", "c", "w", "working", true),
            link("w/3", "w", "d", "done", true),
            link("w/4", "w", "e", "working", false),
            link("x/1", "x", "f", "working", true),
        ]});
        let (mut threads, mut ids) = tree(&book, "w");
        threads.sort();
        ids.sort();
        assert_eq!(threads, vec!["a", "b", "c"]);
        assert_eq!(ids, vec!["a/1", "b/1", "c/1", "w/1", "w/2"]);
        assert_eq!(tree(&book, "d"), (Vec::new(), Vec::new()));
    }
}
