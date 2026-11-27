//! The Director (`docs/daemon.md` › Director): a Loop that checks one
//! stopped thread of its project per run and unblocks it when the next move
//! is obvious. Its prompt is the app's Director template
//! (`apps/desktop/src/state/director.ts`).
//!
//! Plain code picks the thread before any model turn: the daemon asks the
//! running app for [`next`] (`_rustcode/director/next`), and a run with no
//! stopped thread ends silent, so a quiet project costs no tokens. What the
//! Director last saw of each thread is kept in `director.json` in the config
//! folder, so a thread is checked once per stop.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A thread must be stopped this long first: the user may still be typing to it.
pub(crate) const QUIET_SECS: i64 = 3 * 60;

/// Older stops are stale, so a first run never digs through months of threads.
pub(crate) const RECENT_SECS: i64 = 24 * 60 * 60;

/// What the Director remembers of a thread is forgotten this long after its check.
const FORGET_SECS: i64 = 7 * 24 * 60 * 60;

/// A thread as the picker reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub id: String,
    pub title: String,
    /// Unix seconds of its last change.
    pub stopped_at: i64,
    /// Its first reply never came (`stalled`), which goes before a plain stop.
    pub stalled: bool,
}

/// What the Director last saw of one thread.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Seen {
    /// The thread's `stopped_at` when the Director was handed it.
    pub stopped_at: i64,
    /// How many runs were handed this thread.
    pub checks: u32,
    /// Unix seconds of the last hand-off.
    pub at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    threads: BTreeMap<String, Seen>,
}

/// The thread one run checks: stopped for [`QUIET_SECS`], within
/// [`RECENT_SECS`], and changed since the Director last saw it. A stalled
/// thread first, then the most recent stop.
pub(crate) fn pick<'a>(
    rows: &'a [Row],
    seen: &BTreeMap<String, Seen>,
    now: i64,
) -> Option<&'a Row> {
    rows.iter()
        .filter(|row| {
            let age = now - row.stopped_at;
            (QUIET_SECS..=RECENT_SECS).contains(&age)
                && seen
                    .get(&row.id)
                    .is_none_or(|seen| row.stopped_at > seen.stopped_at)
        })
        .max_by_key(|row| (row.stalled, row.stopped_at))
}

/// What a run adds to the Director's prompt: the thread, how long ago it stopped and how
/// often the Director was handed it before.
pub(crate) fn ping(row: &Row, checks: u32, now: i64) -> String {
    let minutes = ((now - row.stopped_at) / 60).max(1);
    let how = if row.stalled {
        "never got a first reply"
    } else {
        "stopped"
    };
    let before = match checks {
        0 => "You have not checked it before.".to_owned(),
        1 => "You checked it once before.".to_owned(),
        n => format!("You checked it {n} times before."),
    };
    format!(
        "Thread `{}` \"{}\" {how} {minutes} min ago. {before}",
        row.id,
        row.title.trim()
    )
}

/// `seen` with `row` handed off at `now`, and entries past [`FORGET_SECS`] dropped.
pub(crate) fn record(seen: &mut BTreeMap<String, Seen>, row: &Row, now: i64) -> u32 {
    seen.retain(|_, seen| now - seen.at < FORGET_SECS);
    let entry = seen.entry(row.id.clone()).or_default();
    let before = entry.checks;
    *entry = Seen {
        stopped_at: row.stopped_at,
        checks: before + 1,
        at: now,
    };
    before
}

fn store_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join("director.json"))
}

fn load(path: &Path) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(path: &Path, store: &Store) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(store).map_err(std::io::Error::other)?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}

/// The threads of the project at `workspace` that are stopped now: not
/// working, waiting on the user, in review, settled, snoozed, a job's run, a
/// delegate or a side chat.
fn stopped_rows(workspace: &str, now_ms: u64) -> Vec<Row> {
    let project = crate::orchestration::local::clean_path(workspace);
    let mut rows = Vec::new();
    for meta in crate::sessions::list_sessions() {
        let Some(id) = crate::sessions::session_id_from_path(&meta.path) else {
            continue;
        };
        if meta.message_count == 0 || crate::orchestration::local::reach_of(&id, &meta) != project {
            continue;
        }
        let metadata = crate::sessions::load_session_metadata(&id);
        let aside = metadata.as_ref().is_some_and(|metadata| {
            metadata.scheduled_job.is_some()
                || metadata.snoozed_until.is_some_and(|until| until > now_ms)
                || metadata.parent.as_ref().is_some_and(|parent| {
                    parent.relationship() != rustcode_session::SessionRelationship::Fork
                })
        });
        if aside {
            continue;
        }
        let stalled = match crate::thread_status::snapshot(&id) {
            (crate::thread_status::Status::Idle, false) => false,
            (crate::thread_status::Status::Stalled, false) => true,
            _ => continue,
        };
        let Some(stopped_at) = chrono::DateTime::parse_from_rfc3339(&meta.when)
            .ok()
            .map(|when| when.timestamp())
            .or_else(|| {
                std::fs::metadata(&meta.path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .map(|modified| chrono::DateTime::<chrono::Utc>::from(modified).timestamp())
            })
        else {
            continue;
        };
        rows.push(Row {
            id,
            title: meta.title,
            stopped_at,
            stalled,
        });
    }
    rows
}

/// The ping for the next stopped thread in the project at `workspace`, or
/// `None` when none stopped since the Director last looked. Handing a thread
/// off records it, so the next run picks another.
pub(crate) fn next(workspace: &str) -> Result<Option<String>, String> {
    let now = chrono::Utc::now();
    let rows = stopped_rows(workspace, now.timestamp_millis().max(0) as u64);
    let path = store_path().ok_or("no config folder")?;
    let mut store = load(&path);
    let Some(row) = pick(&rows, &store.threads, now.timestamp()) else {
        return Ok(None);
    };
    let checks = record(&mut store.threads, row, now.timestamp());
    save(&path, &store).map_err(|error| format!("director.json: {error}"))?;
    Ok(Some(ping(row, checks, now.timestamp())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn row(id: &str, minutes_ago: i64, stalled: bool) -> Row {
        Row {
            id: id.to_owned(),
            title: format!("Task {id}"),
            stopped_at: NOW - minutes_ago * 60,
            stalled,
        }
    }

    #[test]
    fn a_run_takes_a_stalled_thread_first_then_the_latest_stop() {
        let rows = [row("old", 90, false), row("new", 10, false)];
        assert_eq!(pick(&rows, &BTreeMap::new(), NOW).unwrap().id, "new");
        let rows = [row("new", 10, false), row("stuck", 60, true)];
        assert_eq!(pick(&rows, &BTreeMap::new(), NOW).unwrap().id, "stuck");
    }

    #[test]
    fn a_thread_just_stopped_or_long_stopped_is_left_out() {
        let rows = [row("typing", 1, false), row("stale", 25 * 60, false)];
        assert_eq!(pick(&rows, &BTreeMap::new(), NOW), None);
    }

    #[test]
    fn a_thread_is_checked_once_per_stop() {
        let rows = [row("a", 10, false)];
        let mut seen = BTreeMap::new();
        assert_eq!(record(&mut seen, &rows[0], NOW), 0);
        assert_eq!(pick(&rows, &seen, NOW), None);
        let again = [row("a", 5, false)];
        assert_eq!(pick(&again, &seen, NOW).unwrap().id, "a");
        assert_eq!(record(&mut seen, &again[0], NOW), 1);
        assert_eq!(seen["a"].checks, 2);
    }

    #[test]
    fn what_the_director_saw_is_forgotten_after_a_week() {
        let mut seen = BTreeMap::from([(
            "gone".to_owned(),
            Seen {
                stopped_at: 0,
                checks: 3,
                at: NOW - FORGET_SECS,
            },
        )]);
        record(&mut seen, &row("a", 10, false), NOW);
        assert!(!seen.contains_key("gone"));
    }

    #[test]
    fn the_ping_names_the_thread_and_earlier_checks() {
        assert_eq!(
            ping(&row("t1", 12, false), 0, NOW),
            "Thread `t1` \"Task t1\" stopped 12 min ago. You have not checked it before."
        );
        assert!(ping(&row("t1", 12, true), 2, NOW).contains("never got a first reply"));
        assert!(ping(&row("t1", 12, false), 2, NOW).ends_with("You checked it 2 times before."));
    }
}
