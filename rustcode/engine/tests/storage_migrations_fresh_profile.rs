//! The Windows fresh-profile case, as a test target of its own so the Windows
//! portability job can run it natively.
//!
//! On Windows a fresh profile ran every migration and then failed to record
//! them: the checkpoint was flushed through a reopened read-only handle, and
//! `FlushFileBuffers` there fails with `Access is denied. (os error 5)`, so the
//! next start repeated the steps and warned. This target links only the
//! library and its public migration API, which is what lets it build on
//! Windows; the library's own test modules hold unix-only assertions.

use std::fs;
use std::path::Path;

use rustcode::storage_migrations::{self, CheckpointState, RunOutcome, checkpoint_path, status};

fn shipped_ids() -> Vec<String> {
    storage_migrations::MIGRATIONS
        .iter()
        .map(|migration| migration.id.to_owned())
        .collect()
}

fn recorded_ids(checkpoint: &serde_json::Value) -> Vec<String> {
    checkpoint["completed"]
        .as_array()
        .expect("completed array")
        .iter()
        .map(|done| done["id"].as_str().expect("completed id").to_owned())
        .collect()
}

fn leftover_temporary_files(storage: &Path) -> Vec<String> {
    fs::read_dir(storage)
        .expect("read storage directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.contains(".tmp"))
        .collect()
}

#[test]
fn a_fresh_profile_records_its_checkpoint_and_a_restart_reruns_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let fresh = parent.path().join("fresh-profile");
    fs::create_dir_all(&fresh).unwrap();
    let shipped = shipped_ids();

    let applied = match storage_migrations::run_pending(&fresh) {
        RunOutcome::Applied(applied) => applied,
        other => panic!("a fresh profile must run the shipped steps, got {other:?}"),
    };
    assert_eq!(applied, shipped);

    let path = checkpoint_path(&fresh);
    let recorded = fs::read_to_string(&path).unwrap();
    let checkpoint: serde_json::Value = serde_json::from_str(&recorded).unwrap();
    assert_eq!(checkpoint["format"], 1);
    assert_eq!(recorded_ids(&checkpoint), shipped);

    let reported = status(&fresh);
    assert_eq!(reported.checkpoint, CheckpointState::Current);
    assert_eq!(reported.pending().count(), 0);

    // Published, not left next to the checkpoint as a temporary file.
    let storage = path.parent().unwrap();
    let leftovers = leftover_temporary_files(storage);
    assert!(leftovers.is_empty(), "left behind {leftovers:?}");

    // A clean restart on the same profile: no step runs and nothing is
    // rewritten, so the checkpoint did persist.
    assert_eq!(
        storage_migrations::run_pending(&fresh),
        RunOutcome::UpToDate
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), recorded);
    assert_eq!(status(&fresh).pending().count(), 0);
}
