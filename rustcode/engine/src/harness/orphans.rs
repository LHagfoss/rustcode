//! Harness process groups an engine left running when it died without
//! cleaning up (killed, crashed, the machine slept through a quit). Each
//! running group has a record under `run/harness-groups/`, named by its
//! process group id and naming the engine that started it; a starting engine
//! kills the groups whose engine is gone. A process group id is not reused
//! while any process of the group lives; a group whose leader runs but
//! started at another time than recorded is someone else's and is left alone.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const DIR: &str = "harness-groups";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub engine_pid: u32,
    pub harness: String,
    #[serde(default)]
    pub thread: Option<String>,
    /// When the group's leader started, as the system reports it.
    #[serde(default)]
    pub started: Option<String>,
}

/// Where the records live, or `None` without a config directory.
pub(crate) fn directory() -> Option<PathBuf> {
    crate::server::run_dir().ok().map(|dir| dir.join(DIR))
}

/// Note that this engine runs harness `group`.
pub(crate) fn remember(dir: &Path, group: i32, record: &Record) {
    let Ok(text) = serde_json::to_vec(record) else {
        return;
    };
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| crate::atomic_file::write_atomic(&dir.join(group.to_string()), &text));
    if let Err(error) = written {
        crate::dbg_log!("harness group {group}: not recorded for crash cleanup: {error}");
    }
}

/// The group ended or was killed by this engine.
pub(crate) fn forget(dir: &Path, group: i32) {
    let _ = std::fs::remove_file(dir.join(group.to_string()));
}

/// What [`reap_in`] found for one record.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Its engine still runs: the group is that engine's to stop.
    EngineAlive,
    /// The engine is gone and the group with it; only the record was left.
    Gone,
    /// The group id belongs to another process now.
    Reused,
    /// The engine is gone and the group was still running: it was killed.
    Killed(Record),
}

/// Kill every recorded group whose engine is no longer running and drop its
/// record. `alive` says whether a process id runs, `group_exists` whether a
/// process group does, `started` when a running process started, and `kill`
/// ends a group; the real ones are in [`reap`].
pub(crate) fn reap_in(
    dir: &Path,
    alive: impl Fn(u32) -> bool,
    group_exists: impl Fn(i32) -> bool,
    started: impl Fn(u32) -> Option<String>,
    mut kill: impl FnMut(i32),
) -> Vec<Outcome> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut outcomes = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(group) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<i32>().ok())
            .filter(|group| *group > 1)
        else {
            continue;
        };
        let record = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok());
        let Some(record) = record else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if record.engine_pid == std::process::id() || alive(record.engine_pid) {
            outcomes.push(Outcome::EngineAlive);
            continue;
        }
        let _ = std::fs::remove_file(&path);
        let leader = group as u32;
        if alive(leader) && record.started.is_some() && started(leader) != record.started {
            outcomes.push(Outcome::Reused);
        } else if group_exists(group) {
            kill(group);
            outcomes.push(Outcome::Killed(record));
        } else {
            outcomes.push(Outcome::Gone);
        }
    }
    outcomes
}

/// Kill the harness groups engines that died left behind, and report each in
/// its thread's log and as a `harness.orphan_reaped` operational event.
pub(crate) fn reap() {
    let Some(dir) = directory() else {
        return;
    };
    #[cfg(unix)]
    let outcomes = reap_in(
        &dir,
        unix::alive,
        unix::group_exists,
        started,
        super::kill_group,
    );
    #[cfg(not(unix))]
    let outcomes = reap_in(&dir, |_| false, |_| false, |_| None, |_| {});
    for outcome in outcomes {
        if let Outcome::Killed(record) = outcome {
            crate::logger::operational_event(
                "harness.orphan_reaped",
                serde_json::json!({
                    "session_id": record.thread.unwrap_or_default(),
                    "harness": record.harness,
                }),
            );
        }
    }
}

/// When process `pid` started, as `ps` prints it, or `None` when it does not run.
#[cfg(unix)]
pub(crate) fn started(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(unix)]
mod unix {
    /// Whether process `pid` exists, including one this user may not signal.
    pub(super) fn alive(pid: u32) -> bool {
        i32::try_from(pid).is_ok_and(|pid| signal_reaches(pid))
    }

    /// Whether any process of group `group` exists.
    pub(super) fn group_exists(group: i32) -> bool {
        signal_reaches(-group)
    }

    fn signal_reaches(target: i32) -> bool {
        // SAFETY: kill(2) with signal 0 checks for existence and sends nothing.
        let result = unsafe { libc::kill(target, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(engine_pid: u32) -> Record {
        Record {
            engine_pid,
            harness: "claude-code".to_owned(),
            thread: Some("thread-1".to_owned()),
            started: Some("Mon Nov 24 10:00:00 2026".to_owned()),
        }
    }

    #[test]
    fn records_round_trip_and_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        remember(dir.path(), 4242, &record(7));

        let saved: Record =
            serde_json::from_slice(&std::fs::read(dir.path().join("4242")).unwrap()).unwrap();
        assert_eq!(saved, record(7));

        forget(dir.path(), 4242);
        assert!(!dir.path().join("4242").exists());
    }

    #[test]
    fn kills_only_groups_whose_engine_is_gone_and_still_running() {
        let dir = tempfile::tempdir().unwrap();
        remember(dir.path(), 100, &record(1_000));
        remember(dir.path(), 200, &record(2_000));
        remember(dir.path(), 300, &record(2_000));
        remember(dir.path(), 400, &record(std::process::id()));
        remember(dir.path(), 600, &record(2_000));
        std::fs::write(dir.path().join("500"), b"not json").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"{}").unwrap();

        let mut killed = Vec::new();
        let mut outcomes = reap_in(
            dir.path(),
            |pid| pid == 1_000 || pid == 600,
            |group| group == 200 || group == 600,
            |pid| (pid == 600).then(|| "Tue Nov 25 09:00:00 2026".to_owned()),
            |group| killed.push(group),
        );
        outcomes.sort_by_key(|outcome| format!("{outcome:?}"));

        assert_eq!(killed, [200]);
        assert_eq!(
            outcomes,
            [
                Outcome::EngineAlive,
                Outcome::EngineAlive,
                Outcome::Gone,
                Outcome::Killed(record(2_000)),
                Outcome::Reused,
            ]
        );
        assert!(dir.path().join("100").exists());
        assert!(!dir.path().join("200").exists());
        assert!(!dir.path().join("300").exists());
        assert!(dir.path().join("400").exists());
        assert!(!dir.path().join("500").exists());
        assert!(!dir.path().join("600").exists());
        assert!(dir.path().join("notes.txt").exists());
    }

    #[test]
    fn a_missing_directory_reaps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let outcomes = reap_in(
            &dir.path().join("absent"),
            |_| false,
            |_| true,
            |_| None,
            |_| panic!("nothing to kill"),
        );
        assert!(outcomes.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_running_group_of_a_dead_engine_is_killed() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id() as i32;
        let dir = tempfile::tempdir().unwrap();
        let mut dead_engine = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = dead_engine.id();
        dead_engine.wait().unwrap();
        let saved = Record {
            started: started(child.id()),
            ..record(dead_pid)
        };
        assert!(saved.started.is_some());
        remember(dir.path(), group, &saved);

        let outcomes = reap_in(
            dir.path(),
            unix::alive,
            unix::group_exists,
            started,
            super::super::kill_group,
        );

        assert_eq!(outcomes, [Outcome::Killed(saved)]);
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
