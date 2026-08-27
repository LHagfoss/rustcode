use super::{ChatMessage, History};

#[test]
fn history_revision_tracks_structural_and_in_place_mutations() {
    let mut history = History::default();
    let initial_revision = history.revision();

    history.push(ChatMessage::new("user", "hello"));
    let after_push = history.revision();
    assert!(after_push > initial_revision);

    history[0].content = "changed".to_string();
    assert!(history.revision() > after_push);

    history.clear();
    assert!(history.revision() > after_push);
}

#[test]
fn history_clone_keeps_snapshot_revision_without_sharing_storage() {
    let mut history = History::default();
    history.push(ChatMessage::new("user", "hello"));
    let snapshot = history.clone();

    assert_eq!(snapshot.revision(), history.revision());
    history[0].content = "changed".to_string();
    assert_eq!(snapshot[0].content, "hello");
}

#[test]
fn history_snapshot_is_stable_after_live_mutation() {
    let mut history = History::default();
    history.push(ChatMessage::new("user", "hello"));
    let snapshot = history.snapshot();
    let snapshot_revision = snapshot.revision();

    history.push(ChatMessage::new("assistant", "answer"));

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].content, "hello");
    assert_eq!(snapshot.revision(), snapshot_revision);
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].content, "answer");
}
