use super::*;
use crate::pull_watch::Snapshot;

fn watch(link: bool, state: Option<&str>) -> Watch {
    let mut watch: Watch = serde_json::from_value(json!({
        "sessionId": "s1",
        "cwd": "/work/app",
        "number": 12,
        "repo": "acme/app",
        "link": link,
        "status": "watching",
        "createdAt": 0,
        "nextCheckAt": 0,
    }))
    .unwrap();
    watch.snapshot = state.map(|state| Snapshot {
        state: state.to_owned(),
        ..Snapshot::default()
    });
    watch
}

#[test]
fn a_worktree_threads_open_link_merges_when_the_project_turned_it_on() {
    assert!(wanted(Some(&json!(true)), true, &watch(true, Some("open"))));
    assert!(wanted(Some(&json!(true)), true, &watch(true, None)));
}

#[test]
fn nothing_merges_on_its_own_without_the_setting_a_worktree_or_a_link() {
    assert!(!wanted(
        Some(&json!(false)),
        true,
        &watch(true, Some("open"))
    ));
    assert!(!wanted(None, true, &watch(true, Some("open"))));
    assert!(!wanted(
        Some(&json!(true)),
        false,
        &watch(true, Some("open"))
    ));
    assert!(!wanted(
        Some(&json!(true)),
        true,
        &watch(false, Some("open"))
    ));
}

#[test]
fn a_merged_or_closed_pull_request_is_left_alone() {
    assert!(!wanted(
        Some(&json!(true)),
        true,
        &watch(true, Some("merged"))
    ));
    assert!(!wanted(
        Some(&json!(true)),
        true,
        &watch(true, Some("closed"))
    ));
}
