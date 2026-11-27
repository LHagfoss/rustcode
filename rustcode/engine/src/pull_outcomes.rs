use std::collections::HashMap;
use std::path::Path;

use rusqlite::params;

use crate::data_store::PullRequestOutcome;
use crate::pull_watch::Snapshot;

/// How often one settled thread's open pull request is read again.
pub const RECHECK_SECS: i64 = 10 * 60;

/// Pull requests first seen longer ago than this are no longer followed.
pub const FOLLOW_DAYS: i64 = 30;

/// Pull requests read in one round of the watch loop.
pub const PER_ROUND: usize = 3;

/// The rows one query considers before skipping recently tried ones.
const CANDIDATES: i64 = 50;

/// A `thread_outcomes` row of a settled thread whose pull request was last
/// seen open. Settling stops the thread's pull request watch, so a pull
/// request merged after it would otherwise stay `open` in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owed {
    pub thread_id: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
}

/// The rows owed a final outcome at `now`, least recently updated first.
pub fn owed_at(path: &Path, now: chrono::DateTime<chrono::Utc>) -> Result<Vec<Owed>, String> {
    let since = (now - chrono::Duration::days(FOLLOW_DAYS))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let connection = crate::data_store::open(path)?;
    let mut statement = connection
        .prepare(
            "SELECT o.thread_id, o.repo, o.pr_number, COALESCE(o.url, ''), COALESCE(o.title, '')
             FROM thread_outcomes o JOIN threads t ON t.id = o.thread_id
             WHERE o.state = 'open' AND t.settled_at IS NOT NULL AND o.first_seen_at >= ?1
             ORDER BY o.updated_at LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![since, CANDIDATES], |row| {
            Ok(Owed {
                thread_id: row.get(0)?,
                repo: row.get(1)?,
                number: row.get::<_, i64>(2)?.max(0) as u64,
                url: row.get(3)?,
                title: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<_, _>>()
        .map_err(|error| error.to_string())
}

/// The pull requests to read this round: one read per pull request, none
/// tried within [`RECHECK_SECS`] (`tried` holds Unix seconds per pull
/// request), at most [`PER_ROUND`].
pub fn due(owed: &[Owed], tried: &HashMap<(String, u64), i64>, now: i64) -> Vec<(String, u64)> {
    let mut due: Vec<(String, u64)> = Vec::new();
    for row in owed {
        let key = (row.repo.clone(), row.number);
        let recent = tried.get(&key).is_some_and(|at| now - at < RECHECK_SECS);
        if !recent && !due.contains(&key) && crate::pull_requests::is_repository(&row.repo) {
            due.push(key);
        }
        if due.len() == PER_ROUND {
            break;
        }
    }
    due
}

/// What the store keeps of one `gh pr view` answer for an owed row.
pub fn outcome_of(row: &Owed, value: &serde_json::Value) -> Option<PullRequestOutcome> {
    let snapshot = Snapshot::from_view(value)?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let (reverts_number, reverts_title) = if snapshot.state == "merged" {
        crate::pull_watch::reverts(value, &row.repo)
    } else {
        (None, None)
    };
    Some(PullRequestOutcome {
        url: row.url.clone(),
        number: row.number,
        title: text("title").unwrap_or_else(|| row.title.clone()),
        state: snapshot.state.clone(),
        head: snapshot.head.clone(),
        checks_failed: !snapshot.failing.is_empty(),
        merged_at: text("mergedAt"),
        closed_at: text("closedAt"),
        reverts_number,
        reverts_title,
    })
}

/// Read the pull requests settled threads still owe an outcome and record what
/// they became. The thread hears nothing; only the data store changes.
pub async fn follow_settled(tried: &mut HashMap<(String, u64), i64>) {
    let Some(path) = crate::data_store::data_path() else {
        return;
    };
    let now = chrono::Utc::now();
    let owed = match owed_at(&path, now) {
        Ok(owed) => owed,
        Err(error) => {
            crate::dbg_log!("pull outcomes: owed rows not read: {error}");
            return;
        }
    };
    let Ok(folder) = std::env::current_dir() else {
        return;
    };
    for key in due(&owed, tried, now.timestamp()) {
        tried.insert(key.clone(), now.timestamp());
        let value = match crate::pull_requests::watch_view_on(&folder, &key.0, key.1).await {
            Ok(value) => value,
            Err(error) => {
                crate::dbg_log!(
                    "pull outcomes: {}#{} not read: {}",
                    key.0,
                    key.1,
                    error.message
                );
                continue;
            }
        };
        for row in owed
            .iter()
            .filter(|row| row.repo == key.0 && row.number == key.1)
        {
            if let Some(outcome) = outcome_of(row, &value) {
                crate::data_store::record_pull_request(&row.thread_id, &outcome);
            }
        }
    }
}

#[cfg(test)]
#[path = "pull_outcomes_tests.rs"]
mod tests;
