use super::*;
use crate::data_store::{record_pull_request_at, record_thread_settled_at};

fn pull(number: u64, state: &str) -> PullRequestOutcome {
    PullRequestOutcome {
        url: format!("https://github.com/acme/app/pull/{number}"),
        number,
        title: format!("Change {number}"),
        state: state.to_owned(),
        head: "abc".to_owned(),
        ..PullRequestOutcome::default()
    }
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[test]
fn a_settled_thread_whose_pull_request_was_last_seen_open_is_owed_its_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let at = stamp(now());
    record_pull_request_at(&path, "settled", &pull(7, "open"), &at).unwrap();
    record_thread_settled_at(&path, "settled", true).unwrap();
    record_pull_request_at(&path, "active", &pull(8, "open"), &at).unwrap();
    record_pull_request_at(&path, "merged", &pull(9, "merged"), &at).unwrap();
    record_thread_settled_at(&path, "merged", true).unwrap();
    let owed = owed_at(&path, now()).unwrap();
    assert_eq!(owed.len(), 1);
    assert_eq!(owed[0].thread_id, "settled");
    assert_eq!(owed[0].repo, "acme/app");
    assert_eq!(owed[0].number, 7);
}

#[test]
fn a_pull_request_first_seen_long_ago_is_no_longer_followed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite");
    let old = stamp(now() - chrono::Duration::days(FOLLOW_DAYS + 1));
    record_pull_request_at(&path, "t", &pull(7, "open"), &old).unwrap();
    record_thread_settled_at(&path, "t", true).unwrap();
    assert!(owed_at(&path, now()).unwrap().is_empty());
}

fn owed(thread: &str, number: u64) -> Owed {
    Owed {
        thread_id: thread.to_owned(),
        repo: "acme/app".to_owned(),
        number,
        url: format!("https://github.com/acme/app/pull/{number}"),
        title: String::new(),
    }
}

#[test]
fn each_pull_request_is_read_once_a_round_and_not_again_until_its_recheck() {
    let rows = [
        owed("a", 1),
        owed("b", 1),
        owed("c", 2),
        owed("d", 3),
        owed("e", 4),
    ];
    let mut tried = HashMap::new();
    tried.insert(("acme/app".to_owned(), 2), 1_000);
    let due = due(&rows, &tried, 1_000 + RECHECK_SECS - 1);
    assert_eq!(
        due,
        vec![
            ("acme/app".to_owned(), 1),
            ("acme/app".to_owned(), 3),
            ("acme/app".to_owned(), 4),
        ]
    );
}

#[test]
fn a_merged_answer_records_the_merge() {
    let value = serde_json::json!({
        "number": 7,
        "title": "Fix it",
        "state": "MERGED",
        "headRefOid": "def",
        "mergedAt": "2026-11-17T10:24:25Z",
        "statusCheckRollup": [],
    });
    let outcome = outcome_of(&owed("t", 7), &value).unwrap();
    assert_eq!(outcome.state, "merged");
    assert_eq!(outcome.merged_at.as_deref(), Some("2026-11-17T10:24:25Z"));
    assert_eq!(outcome.title, "Fix it");
}
