use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

const NO_WAIT: Duration = Duration::ZERO;

fn checkpoint_text(dir: &Path) -> String {
    fs::read_to_string(checkpoint_path(dir)).unwrap()
}

fn completed_ids(dir: &Path) -> Vec<String> {
    let checkpoint: Checkpoint = serde_json::from_str(&checkpoint_text(dir)).unwrap();
    checkpoint
        .completed
        .into_iter()
        .map(|done| done.id)
        .collect()
}

fn write_raw_checkpoint(dir: &Path, text: &str) {
    fs::create_dir_all(dir.join(STORAGE_DIR)).unwrap();
    fs::write(checkpoint_path(dir), text).unwrap();
}

#[test]
fn the_shipped_registry_has_unique_ids() {
    for (index, migration) in MIGRATIONS.iter().enumerate() {
        assert!(
            MIGRATIONS[..index].iter().all(|m| m.id != migration.id),
            "duplicate migration id {}",
            migration.id
        );
    }
}

#[test]
fn an_install_with_nothing_registered_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        run_pending_with(dir.path(), &[], NO_WAIT),
        RunOutcome::UpToDate
    );
    assert!(!dir.path().join(STORAGE_DIR).exists());
}

#[test]
fn a_fresh_install_runs_every_step_in_order_and_records_it() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn first(_: &Path) -> Result<(), String> {
        assert_eq!(RUNS.fetch_add(1, Ordering::SeqCst), 0);
        Ok(())
    }
    fn second(_: &Path) -> Result<(), String> {
        assert_eq!(RUNS.fetch_add(1, Ordering::SeqCst), 1);
        Ok(())
    }
    let registry = [
        Migration {
            id: "0001-first",
            description: "first",
            run: first,
        },
        Migration {
            id: "0002-second",
            description: "second",
            run: second,
        },
    ];
    let dir = tempfile::tempdir().unwrap();

    let before = status_with(dir.path(), &registry).0;
    assert_eq!(before.checkpoint, CheckpointState::Missing);
    assert_eq!(before.pending().count(), 2);

    assert_eq!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Applied(vec!["0001-first".into(), "0002-second".into()])
    );
    assert_eq!(RUNS.load(Ordering::SeqCst), 2);
    assert_eq!(completed_ids(dir.path()), ["0001-first", "0002-second"]);
    let after = status_with(dir.path(), &registry).0;
    assert_eq!(after.checkpoint, CheckpointState::Current);
    assert_eq!(after.pending().count(), 0);
}

#[test]
fn an_already_current_install_reruns_nothing_and_leaves_the_checkpoint_alone() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn step(_: &Path) -> Result<(), String> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();
    let recorded = r#"{"format":1,"completed":[{"id":"0001-step","completed_at":"2026-01-01T00:00:00+00:00","version":"0.1.0"}]}"#;
    write_raw_checkpoint(dir.path(), recorded);

    assert_eq!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::UpToDate
    );
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);
    assert_eq!(checkpoint_text(dir.path()), recorded);
    assert!(!lock_path(dir.path()).exists());
}

#[test]
fn a_repeated_run_does_not_rerun_completed_steps() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn step(_: &Path) -> Result<(), String> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();

    assert!(matches!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Applied(_)
    ));
    let recorded = checkpoint_text(dir.path());
    assert_eq!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::UpToDate
    );
    assert_eq!(RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(checkpoint_text(dir.path()), recorded);
}

#[test]
fn a_failed_step_is_not_recorded_and_is_retried_next_time() {
    static FIRST_RUNS: AtomicUsize = AtomicUsize::new(0);
    static FLAKY_RUNS: AtomicUsize = AtomicUsize::new(0);
    static LAST_RUNS: AtomicUsize = AtomicUsize::new(0);
    fn first(_: &Path) -> Result<(), String> {
        FIRST_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn flaky(_: &Path) -> Result<(), String> {
        if FLAKY_RUNS.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("disk full".into())
        } else {
            Ok(())
        }
    }
    fn last(_: &Path) -> Result<(), String> {
        LAST_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    let registry = [
        Migration {
            id: "0001-first",
            description: "first",
            run: first,
        },
        Migration {
            id: "0002-flaky",
            description: "flaky",
            run: flaky,
        },
        Migration {
            id: "0003-last",
            description: "last",
            run: last,
        },
    ];
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Failed {
            applied: vec!["0001-first".into()],
            id: "0002-flaky".into(),
            error: "disk full".into(),
        }
    );
    // Later steps wait for the failed one.
    assert_eq!(LAST_RUNS.load(Ordering::SeqCst), 0);
    assert_eq!(completed_ids(dir.path()), ["0001-first"]);
    let ids: Vec<_> = status_with(dir.path(), &registry)
        .0
        .pending()
        .map(|entry| entry.id.clone())
        .collect();
    assert_eq!(ids, ["0002-flaky", "0003-last"]);

    assert_eq!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Applied(vec!["0002-flaky".into(), "0003-last".into()])
    );
    assert_eq!(FIRST_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(FLAKY_RUNS.load(Ordering::SeqCst), 2);
    assert_eq!(LAST_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(
        completed_ids(dir.path()),
        ["0001-first", "0002-flaky", "0003-last"]
    );
}

#[test]
fn concurrent_runs_apply_each_step_once() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn slow(_: &Path) -> Result<(), String> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }
    static REGISTRY: [Migration; 1] = [Migration {
        id: "0001-slow",
        description: "slow",
        run: slow,
    }];
    let dir = tempfile::tempdir().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let outcomes: Vec<RunOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let barrier = barrier.clone();
                let path = dir.path();
                scope.spawn(move || {
                    barrier.wait();
                    run_pending_with(path, &REGISTRY, Duration::from_secs(10))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    assert_eq!(RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RunOutcome::Applied(_)))
            .count(),
        1
    );
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, RunOutcome::Applied(_) | RunOutcome::UpToDate))
    );
    assert_eq!(completed_ids(dir.path()), ["0001-slow"]);
}

#[test]
fn a_held_lock_leaves_the_work_to_its_holder() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn step(_: &Path) -> Result<(), String> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(STORAGE_DIR)).unwrap();
    let held = acquire_lock(dir.path(), NO_WAIT).unwrap().unwrap();

    assert_eq!(
        run_pending_with(dir.path(), &registry, Duration::from_millis(100)),
        RunOutcome::Busy
    );
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);
    drop(held);
    assert!(matches!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Applied(_)
    ));
}

#[test]
fn a_checkpoint_from_a_newer_format_blocks_without_rewriting() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    fn step(_: &Path) -> Result<(), String> {
        RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();
    let newer = r#"{"format":2,"steps":{"0001-step":"done"}}"#;
    write_raw_checkpoint(dir.path(), newer);

    assert_eq!(
        status_with(dir.path(), &registry).0.checkpoint,
        CheckpointState::Newer { format: 2 }
    );
    assert!(matches!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Blocked(_)
    ));
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);
    assert_eq!(checkpoint_text(dir.path()), newer);
}

#[test]
fn an_unreadable_checkpoint_blocks_without_rewriting() {
    let dir = tempfile::tempdir().unwrap();
    fn step(_: &Path) -> Result<(), String> {
        panic!("must not run");
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    write_raw_checkpoint(dir.path(), "{not json");

    assert!(matches!(
        status_with(dir.path(), &registry).0.checkpoint,
        CheckpointState::Unreadable { .. }
    ));
    assert!(matches!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Blocked(_)
    ));
    assert_eq!(checkpoint_text(dir.path()), "{not json");
}

#[test]
fn steps_recorded_by_a_newer_release_are_reported_and_kept() {
    fn step(_: &Path) -> Result<(), String> {
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "step",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();
    write_raw_checkpoint(
        dir.path(),
        r#"{"format":1,"completed":[{"id":"0009-future","completed_at":"2027-01-01T00:00:00+00:00","version":"9.0.0"}]}"#,
    );

    let status = status_with(dir.path(), &registry).0;
    assert_eq!(status.unknown_completed, ["0009-future"]);
    assert_eq!(status.pending().count(), 1);
    assert!(status.render().contains("0009-future"));

    assert!(matches!(
        run_pending_with(dir.path(), &registry, NO_WAIT),
        RunOutcome::Applied(_)
    ));
    assert_eq!(completed_ids(dir.path()), ["0009-future", "0001-step"]);
}

#[test]
fn status_reports_structured_output_without_touching_files() {
    fn step(_: &Path) -> Result<(), String> {
        Ok(())
    }
    let registry = [Migration {
        id: "0001-step",
        description: "Move things",
        run: step,
    }];
    let dir = tempfile::tempdir().unwrap();

    let status = status_with(dir.path(), &registry).0;
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["checkpoint"]["state"], "missing");
    assert_eq!(json["migrations"][0]["id"], "0001-step");
    assert_eq!(json["migrations"][0]["state"], "pending");
    assert!(
        status
            .render()
            .contains("pending    0001-step  Move things")
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

mod sessions_folder_to_threads {
    use super::super::sessions_folder_to_threads;
    use std::fs;
    use std::path::Path;

    fn thread(root: &Path, folder: &str, id: &str) -> std::path::PathBuf {
        let dir = root.join(folder).join("2026/10/30").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("history.json"), "[]").unwrap();
        dir
    }

    #[test]
    fn a_fresh_install_has_nothing_to_move() {
        let dir = tempfile::tempdir().unwrap();
        sessions_folder_to_threads(dir.path()).unwrap();
        assert!(!dir.path().join("threads").exists());
        assert!(fs::symlink_metadata(dir.path().join("sessions")).is_err());
    }

    #[test]
    fn the_folder_is_renamed_and_the_old_name_links_to_it() {
        let dir = tempfile::tempdir().unwrap();
        thread(dir.path(), "sessions", "a");
        sessions_folder_to_threads(dir.path()).unwrap();
        assert!(
            dir.path()
                .join("threads/2026/10/30/a/history.json")
                .is_file()
        );
        #[cfg(unix)]
        {
            let link = dir.path().join("sessions");
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(link.join("2026/10/30/a/history.json").is_file());
        }
        // Running again changes nothing.
        sessions_folder_to_threads(dir.path()).unwrap();
        assert!(
            dir.path()
                .join("threads/2026/10/30/a/history.json")
                .is_file()
        );
    }

    #[test]
    fn a_sessions_folder_written_after_the_rename_is_merged_in() {
        let dir = tempfile::tempdir().unwrap();
        thread(dir.path(), "threads", "a");
        thread(dir.path(), "sessions", "b");
        sessions_folder_to_threads(dir.path()).unwrap();
        assert!(
            dir.path()
                .join("threads/2026/10/30/a/history.json")
                .is_file()
        );
        assert!(
            dir.path()
                .join("threads/2026/10/30/b/history.json")
                .is_file()
        );
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(dir.path().join("sessions"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn an_entry_in_both_folders_is_kept_and_reported_until_resolved() {
        let dir = tempfile::tempdir().unwrap();
        thread(dir.path(), "threads", "a");
        let kept = thread(dir.path(), "sessions", "a");
        thread(dir.path(), "sessions", "b");
        let error = sessions_folder_to_threads(dir.path()).unwrap_err();
        assert!(error.contains("1 entries exist in both"), "{error}");
        assert!(
            kept.join("history.json").is_file(),
            "the colliding thread stays"
        );
        assert!(
            dir.path()
                .join("threads/2026/10/30/b/history.json")
                .is_file()
        );

        fs::remove_dir_all(&kept).unwrap();
        sessions_folder_to_threads(dir.path()).unwrap();
        assert!(
            dir.path()
                .join("threads/2026/10/30/a/history.json")
                .is_file()
        );
    }
}
