//! Who drives a workflow run, and which runs are waiting for a driver (#4387).
//!
//! A run is driven by one engine process at a time. That process stamps the
//! run's `driver.json` every [`BEAT_EVERY`] with its owner id
//! (`data_store::engine_owner`), the same id delegate links carry, so the
//! app's engine and the background server, which share one config folder,
//! can tell a run another process drives from one whose process died. A
//! stamp older than [`STALE_MS`] is a dead driver's.
//!
//! The beat is kept beside `run.json`, not in it: the run's driver replaces
//! `run.json` whole on every change, and a beat written into it by a second
//! writer would be undone by the next change.
//!
//! `<config>/workflows/running/<run id>` holds the id of the thread that
//! started each run still `running`, so an engine that starts can find the
//! runs a restart left behind without reading every thread's folder. A marker
//! whose run settled or disappeared is removed by the next sweep.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::store::{Store, now_ms};

/// How often a driving process stamps its runs.
pub(super) const BEAT_EVERY: Duration = Duration::from_secs(10);
/// How old a stamp may be before the run counts as undriven.
pub(super) const STALE_MS: i64 = 45_000;
/// How often an engine looks for undriven runs it may take up.
pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(60);

const DRIVER_FILE: &str = "driver.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Driver {
    pub owner: String,
    pub beat_ms: i64,
}

/// Whether `driver` is a live process other than `me`.
pub(super) fn held_elsewhere(driver: Option<&Driver>, me: &str, at: i64) -> bool {
    driver.is_some_and(|driver| driver.owner != me && at - driver.beat_ms <= STALE_MS)
}

pub(super) fn load(store: &Store, run_id: &str) -> Option<Driver> {
    let path = store.run_dir(run_id)?.join(DRIVER_FILE);
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(super) fn stamp(store: &Store, run_id: &str, owner: &str, at: i64) {
    let Some(dir) = store.run_dir(run_id) else {
        return;
    };
    let driver = Driver {
        owner: owner.to_owned(),
        beat_ms: at,
    };
    if let Ok(text) = serde_json::to_vec(&driver) {
        let _ = crate::atomic_file::write_atomic(&dir.join(DRIVER_FILE), &text);
    }
}

/// The folder of running-run markers under `config`.
pub(super) fn index_at(config: &Path) -> PathBuf {
    config.join("workflows").join("running")
}

/// The index in the user's config folder. Unit tests pass a folder of their
/// own to [`index_at`] and never write the user's.
pub(super) fn index() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::config::get_config_dir().map(|config| index_at(&config))
}

/// Note that `thread`'s run `run_id` is running.
pub(super) fn mark_running(index: &Path, thread: &str, run_id: &str) {
    if !super::store::valid_run_id(run_id) || std::fs::create_dir_all(index).is_err() {
        return;
    }
    let _ = crate::atomic_file::write_atomic(&index.join(run_id), thread.as_bytes());
}

pub(super) fn unmark(index: &Path, run_id: &str) {
    if super::store::valid_run_id(run_id) {
        let _ = std::fs::remove_file(index.join(run_id));
    }
}

/// Every `(thread, run id)` the index names.
pub(super) fn marked(index: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(index) else {
        return Vec::new();
    };
    let mut runs: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let run_id = entry.file_name().to_str()?.to_owned();
            if !super::store::valid_run_id(&run_id) {
                return None;
            }
            let thread = std::fs::read_to_string(entry.path()).ok()?;
            let thread = thread.trim().to_owned();
            rustcode_session::valid_session_id(&thread).then_some((thread, run_id))
        })
        .collect();
    runs.sort();
    runs
}

/// What a sweep should do with one marked run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// The run settled or is gone: drop its marker.
    Forget,
    /// Driven here or by a live process, or its thread cannot start workers
    /// without the app: leave it.
    Leave,
    /// Undriven and its thread may start workers: take it up.
    Resume,
}

pub(super) fn verdict(
    store: &Store,
    run_id: &str,
    me: &str,
    at: i64,
    driven_here: bool,
    may_start_workers: bool,
) -> Verdict {
    let Some(record) = store.load(run_id) else {
        return Verdict::Forget;
    };
    if record.status.settled() {
        return Verdict::Forget;
    }
    if driven_here || held_elsewhere(load(store, run_id).as_ref(), me, at) || !may_start_workers {
        return Verdict::Leave;
    }
    Verdict::Resume
}

pub(super) fn now() -> i64 {
    now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(owner: &str, beat_ms: i64) -> Driver {
        Driver {
            owner: owner.to_owned(),
            beat_ms,
        }
    }

    #[test]
    fn only_another_process_with_a_fresh_stamp_holds_a_run() {
        let at = 1_000_000;
        assert!(!held_elsewhere(None, "me", at));
        assert!(!held_elsewhere(Some(&driver("me", at)), "me", at));
        assert!(held_elsewhere(
            Some(&driver("other", at - 10_000)),
            "me",
            at
        ));
        assert!(held_elsewhere(
            Some(&driver("other", at - STALE_MS)),
            "me",
            at
        ));
        assert!(!held_elsewhere(
            Some(&driver("other", at - STALE_MS - 1)),
            "me",
            at
        ));
    }

    #[test]
    fn a_stamp_is_read_back_from_the_run_folder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf());
        std::fs::create_dir_all(dir.path().join("wf-1-2")).unwrap();
        assert_eq!(load(&store, "wf-1-2"), None);
        stamp(&store, "wf-1-2", "owner-a", 42);
        assert_eq!(load(&store, "wf-1-2"), Some(driver("owner-a", 42)));
    }

    #[test]
    fn the_index_names_each_marked_run_with_its_thread_until_unmarked() {
        let dir = tempfile::tempdir().unwrap();
        let index = index_at(dir.path());
        assert!(marked(&index).is_empty());
        mark_running(&index, "1784626803679", "wf-2-b");
        mark_running(&index, "1784626803679", "wf-1-a");
        mark_running(&index, "1784626803679", "../escape");
        assert_eq!(
            marked(&index),
            vec![
                ("1784626803679".to_owned(), "wf-1-a".to_owned()),
                ("1784626803679".to_owned(), "wf-2-b".to_owned()),
            ]
        );
        unmark(&index, "wf-1-a");
        assert_eq!(marked(&index).len(), 1);
    }
}
