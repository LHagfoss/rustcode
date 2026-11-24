//! The thread above a delegated one (`start_session`, `fan_out`).
//!
//! The app's link book prunes a finished link after 50 others, and the
//! sidebar used to read the parent from that link: a thread a parent started
//! lost its place and became a top-level row. The parent is written into the
//! child's `metadata.json` at `session/new` instead, beside its id and
//! creation time, where it lives as long as the thread does.
//!
//! Threads started before that record existed are found once: an agent hands
//! its task to a child with a heading, `From thread “<title>” (agent):`, as
//! the first user message (`delivered` in `apps/desktop/src/state/
//! orchestration.ts`). A background pass reads that heading, records the
//! parent (its id from a surviving link when there is one), and remembers it
//! ran in the data store's `store_meta`. No model is called and no transcript
//! is changed.

use rustcode_session::SessionRelationship;
use serde_json::Value;

use rustcode_core::ChatMessage;
// The heading every agent-delivered task starts with, and the title in it.
use crate::message_origin::{AGENT_HEADING as HEADING, delivered_title};

/// `store_meta` key and version of the one-time pass for threads that predate
/// the record. A later release that can read more bumps the version.
const BACKFILL_KEY: &str = "thread_parents_version";
const BACKFILL_VERSION: i64 = 2;

/// First-message prefix written by the scheduler daemon for both legacy Loops
/// and Jobs (`orchestration::local_threads::loop_prompt`).
const SCHEDULED_HEADING: &str = "This is a scheduled run of the ";

/// What the one-time pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Thread folders it looked at.
    pub(crate) threads: usize,
    /// Threads with no parent record that the heading showed were delegated.
    pub(crate) delegated: usize,
    /// Threads with no scheduled_job record that the first message showed were job runs.
    pub(crate) scheduled_runs: usize,
}

/// The parent `session/new` named in `_meta.rustcode.parent`, when it is a
/// usable session id, and how the new thread relates to it
/// (`_meta.rustcode.relationship`). Absent is a delegate, which is what every
/// caller before the field was; a name this release does not know is `Other`,
/// kept as a child but never given a delegate's powers.
pub(crate) fn requested(
    meta: Option<&serde_json::Map<String, Value>>,
) -> Option<(String, SessionRelationship)> {
    let rustcode = meta.and_then(|meta| meta.get("rustcode"))?;
    let id = rustcode
        .get("parent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| rustcode_session::valid_session_id(id))?;
    let relationship = match rustcode.get("relationship").and_then(Value::as_str) {
        None => SessionRelationship::Delegate,
        Some(name) => SessionRelationship::parse(name).unwrap_or(SessionRelationship::Other),
    };
    Some((id.to_owned(), relationship))
}

/// Record that `parent` started `child` as `relationship`, with the parent's
/// title as it is now.
pub(crate) fn record(child: &str, parent: &str, relationship: SessionRelationship) {
    if !rustcode_session::valid_session_id(child) {
        return;
    }
    let parent = rustcode_session::SessionParent {
        id: Some(parent.to_owned()),
        title: crate::sessions::load_session_title(parent).filter(|title| !title.is_empty()),
        relationship: Some(relationship),
    };
    crate::sessions::save_session_parent(child, &parent);
}

/// The first user message of `history`, with a delivered task's heading
/// removed: the agent's words, not the wrapper.
fn first_task(history: &[ChatMessage]) -> Option<&str> {
    history
        .iter()
        .find(|message| message.is_user_request())
        .map(|message| message.content.as_str())
}

/// The parent a thread's surviving link names, when the link book still has
/// one: a start link first, else the earliest link to it (only the thread that
/// started it, or one of its own descendants, ever messages it).
fn linked_parent(links: &[Value], child: &str) -> Option<(String, Option<String>)> {
    let mut best: Option<(&Value, bool, i64)> = None;
    for link in links {
        if link.get("target").and_then(Value::as_str) != Some(child) {
            continue;
        }
        let started = link
            .get("started")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let at = link.get("createdAt").and_then(Value::as_i64).unwrap_or(0);
        let better = match best {
            None => true,
            Some((_, was_started, was_at)) => match (started, was_started) {
                (true, false) => true,
                (false, true) => false,
                _ => at < was_at,
            },
        };
        if better {
            best = Some((link, started, at));
        }
    }
    let link = best?.0;
    let id = link.get("orchestrator").and_then(Value::as_str)?;
    if !rustcode_session::valid_session_id(id) {
        return None;
    }
    let title = link
        .get("orchestratorTitle")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned);
    Some((id.to_owned(), title))
}

/// The job name a scheduled run's first message opens with (`job “…”` or legacy `Loop “…”`).
fn scheduled_job_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(SCHEDULED_HEADING)?;
    let after_kind = rest
        .strip_prefix("job “")
        .or_else(|| rest.strip_prefix("Loop “"))?;
    let (name, _) = after_kind.split_once("”.")?;
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// Records the parent and scheduled-job marker of every thread an older release
/// started whose first message still shows it. Idempotent: recorded threads are
/// skipped, and transcripts without either heading's bytes are not parsed at all.
pub(crate) fn backfill() -> Report {
    let links = crate::data_store::load_link_book()
        .ok()
        .and_then(|book| book.get("book")?.get("links")?.as_array().cloned())
        .unwrap_or_default();
    let mut report = Report::default();
    for path in crate::sessions::session_history_paths() {
        report.threads += 1;
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let has_parent_heading = bytes
            .windows(HEADING.len())
            .any(|window| window == HEADING.as_bytes());
        let has_scheduled_heading = bytes
            .windows(SCHEDULED_HEADING.len())
            .any(|window| window == SCHEDULED_HEADING.as_bytes());
        if !has_parent_heading && !has_scheduled_heading {
            continue;
        }
        let Some(id) = crate::sessions::session_id_from_path(&path) else {
            continue;
        };
        let needs_parent =
            has_parent_heading && crate::sessions::load_session_parent(&id).is_none();
        let needs_scheduled =
            has_scheduled_heading && crate::sessions::load_session_scheduled_job(&id).is_none();
        if !needs_parent && !needs_scheduled {
            continue;
        }
        let Ok(history) = serde_json::from_slice::<Vec<ChatMessage>>(&bytes) else {
            continue;
        };
        let Some(first) = first_task(&history) else {
            continue;
        };
        if needs_parent && let Some(title) = delivered_title(first) {
            let (parent_id, parent_title) = match linked_parent(&links, &id) {
                Some((parent_id, parent_title)) => (Some(parent_id), parent_title),
                None => (None, None),
            };
            let parent = rustcode_session::SessionParent {
                id: parent_id,
                title: parent_title.or_else(|| Some(title.to_owned())),
                relationship: Some(SessionRelationship::Delegate),
            };
            crate::sessions::save_session_parent(&id, &parent);
            report.delegated += 1;
        }
        if needs_scheduled && let Some(job_name) = scheduled_job_name(first) {
            crate::sessions::save_session_scheduled_job(&id, job_name);
            report.scheduled_runs += 1;
        }
    }
    report
}

/// Runs the one-time pass at engine start, before the session list is read, so
/// a thread an older release started has its parent from the first list on.
/// After the first run, `store_meta` answers at once and nothing is read.
pub(crate) fn ensure_ready() {
    let done = crate::data_store::meta(BACKFILL_KEY)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if done >= BACKFILL_VERSION {
        return;
    }
    let report = backfill();
    if let Err(error) = crate::data_store::set_meta(BACKFILL_KEY, &BACKFILL_VERSION.to_string()) {
        crate::dbg_log!("thread parents: could not remember the pass ran: {error}");
    }
    crate::logger::operational_event(
        "thread_parents.backfill",
        serde_json::json!({
            "threads": report.threads,
            "delegated": report.delegated,
            "scheduled_runs": report.scheduled_runs,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_parent_named_by_session_new_is_read_from_the_meta() {
        let meta = serde_json::Map::from_iter([(
            "rustcode".to_owned(),
            json!({ "parent": " 018d6b28-0000-7000-8000-000000000002 " }),
        )]);
        assert_eq!(
            requested(Some(&meta)),
            Some((
                "018d6b28-0000-7000-8000-000000000002".to_owned(),
                SessionRelationship::Delegate
            ))
        );
        assert_eq!(requested(None), None);
        let blank = serde_json::Map::from_iter([("rustcode".to_owned(), json!({ "parent": " " }))]);
        assert_eq!(requested(Some(&blank)), None);
        let relative =
            serde_json::Map::from_iter([("rustcode".to_owned(), json!({ "parent": "../x" }))]);
        assert_eq!(requested(Some(&relative)), None);
    }

    #[test]
    fn the_relationship_named_by_session_new_is_read_and_an_unknown_one_is_never_a_delegate() {
        let id = "018d6b28-0000-7000-8000-000000000002";
        let relationship = |name: Value| {
            let meta = serde_json::Map::from_iter([(
                "rustcode".to_owned(),
                json!({ "parent": id, "relationship": name }),
            )]);
            requested(Some(&meta)).map(|(_, relationship)| relationship)
        };
        assert_eq!(relationship(json!("fork")), Some(SessionRelationship::Fork));
        assert_eq!(
            relationship(json!("side_chat")),
            Some(SessionRelationship::SideChat)
        );
        // A name this release does not know is never a delegate.
        assert_eq!(
            relationship(json!("mirror")),
            Some(SessionRelationship::Other)
        );
        // No relationship at all is what every caller before the field sent.
        let meta = serde_json::Map::from_iter([("rustcode".to_owned(), json!({ "parent": id }))]);
        assert_eq!(
            requested(Some(&meta)).map(|(_, relationship)| relationship),
            Some(SessionRelationship::Delegate)
        );
    }

    #[test]
    fn a_recorded_relationship_survives_in_the_thread_metadata() {
        let parent = crate::sessions::create_detached_session();
        let child = crate::sessions::create_detached_session();
        crate::sessions::save_session_title(&parent, "Update the sidebar");
        record(&child, &parent, SessionRelationship::SideChat);
        let saved = crate::sessions::load_session_parent(&child).expect("recorded parent");
        assert_eq!(saved.id.as_deref(), Some(parent.as_str()));
        assert_eq!(saved.title.as_deref(), Some("Update the sidebar"));
        assert_eq!(saved.relationship, Some(SessionRelationship::SideChat));
    }

    #[test]
    fn a_delivered_task_names_its_parent_in_the_heading() {
        assert_eq!(
            delivered_title("From thread “Research the sidebar” (agent):\n\nread these files"),
            Some("Research the sidebar")
        );
        assert_eq!(delivered_title("From thread “” (agent):\n\nx"), None);
        assert_eq!(delivered_title("read these files"), None);
        // A user quoting the heading later in a thread is not the first task.
        let history = vec![
            ChatMessage::new("user", "fix the build"),
            ChatMessage::new("assistant", "done"),
        ];
        assert_eq!(first_task(&history), Some("fix the build"));
        assert_eq!(first_task(&[]), None);
    }

    #[test]
    fn a_surviving_link_names_the_parent_for_an_old_thread() {
        let links = vec![
            json!({"target": "child", "orchestrator": "later", "createdAt": 20}),
            json!({"target": "child", "orchestrator": "parent", "started": true, "createdAt": 30}),
            json!({"target": "other", "orchestrator": "elsewhere", "createdAt": 1}),
        ];
        assert_eq!(
            linked_parent(&links, "child"),
            Some(("parent".to_owned(), None))
        );
        // Without a start link, the earliest one to the child.
        let links = vec![
            json!({"target": "child", "orchestrator": "later", "createdAt": 20}),
            json!({"target": "child", "orchestrator": "first", "createdAt": 10}),
        ];
        assert_eq!(
            linked_parent(&links, "child"),
            Some(("first".to_owned(), None))
        );
        assert_eq!(linked_parent(&links, "unknown"), None);
    }

    #[test]
    fn the_pass_records_a_parent_once() {
        let child = crate::sessions::create_detached_session();
        assert!(!child.is_empty());
        let dir = crate::sessions::get_active_session_dir(&child).expect("session dir");
        let history = vec![
            ChatMessage::new(
                "user",
                "From thread “Update the sidebar” (agent):\n\ndo the work",
            ),
            ChatMessage::new("assistant", "done"),
        ];
        std::fs::write(
            dir.join("history.json"),
            serde_json::to_string(&history).unwrap(),
        )
        .unwrap();
        crate::sessions::save_session_title(&child, "Research: the sidebar");

        assert_eq!(crate::sessions::load_session_parent(&child), None);
        let first = backfill();
        assert!(first.threads >= 1);
        assert!(first.delegated >= 1);
        let parent = crate::sessions::load_session_parent(&child).expect("recorded parent");
        // No link in this test's store: the heading is all there is.
        assert_eq!(parent.id, None);
        assert_eq!(parent.title.as_deref(), Some("Update the sidebar"));
        assert_eq!(parent.relationship, Some(SessionRelationship::Delegate));

        // The next pass leaves it alone.
        let before = crate::sessions::load_session_parent(&child);
        backfill();
        assert_eq!(crate::sessions::load_session_parent(&child), before);
    }

    #[test]
    fn scheduled_run_headings_recover_both_loop_and_job_names() {
        assert_eq!(
            scheduled_job_name(
                "This is a scheduled run of the Loop “Check Daily Dev”. Nobody is watching it run.\n\nCheck feed"
            ),
            Some("Check Daily Dev")
        );
        assert_eq!(
            scheduled_job_name(
                "This is a scheduled run of the job “CI watch”. Nobody is watching it run."
            ),
            Some("CI watch")
        );
        assert_eq!(
            scheduled_job_name("This is a scheduled run of the job “”. Nobody is watching."),
            None
        );
        assert_eq!(scheduled_job_name("Check Daily Dev"), None);
    }

    #[test]
    fn the_pass_marks_legacy_scheduled_runs_once() {
        let run = crate::sessions::create_detached_session();
        assert!(!run.is_empty());
        let dir = crate::sessions::get_active_session_dir(&run).expect("session dir");
        let history = vec![
            ChatMessage::new(
                "user",
                "This is a scheduled run of the Loop “Check Daily Dev”. Nobody is watching it run.\n\nCheck feed",
            ),
            ChatMessage::new("assistant", "Nothing to report."),
        ];
        std::fs::write(
            dir.join("history.json"),
            serde_json::to_string(&history).unwrap(),
        )
        .unwrap();

        assert_eq!(crate::sessions::load_session_scheduled_job(&run), None);
        let first = backfill();
        assert!(first.scheduled_runs >= 1);
        assert_eq!(
            crate::sessions::load_session_scheduled_job(&run).as_deref(),
            Some("Check Daily Dev")
        );

        let before = crate::sessions::load_session_scheduled_job(&run);
        backfill();
        assert_eq!(crate::sessions::load_session_scheduled_job(&run), before);
    }
}
