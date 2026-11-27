//! What a set of threads spent, for a workflow run's budgets: the tokens
//! each harness reported over every turn of the run's workers, and the USD
//! the ones that report cost said. Nothing here prices a turn.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

/// Per-thread recorded token and cost totals from the local data store.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThreadRunUsage {
    pub tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// The spend of a run's workers.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThreadsUsage {
    /// Input plus output tokens.
    pub tokens: u64,
    /// The USD the workers that report cost reported.
    pub cost_usd: f64,
    /// How many of the threads reported any cost.
    pub threads_with_cost: usize,
    /// How many threads were asked about.
    pub threads: usize,
}

/// [`ThreadsUsage`] of `thread_ids` in the user's store. No store yet is
/// nothing spent.
pub fn threads_usage(thread_ids: &[String]) -> Result<ThreadsUsage, String> {
    match super::data_path() {
        Some(path) if path.is_file() => threads_usage_at(&path, thread_ids),
        _ => Ok(ThreadsUsage {
            threads: thread_ids.len(),
            ..ThreadsUsage::default()
        }),
    }
}

/// [`threads_usage`] against one store file.
pub fn threads_usage_at(path: &Path, thread_ids: &[String]) -> Result<ThreadsUsage, String> {
    let mut usage = ThreadsUsage {
        threads: thread_ids.len(),
        ..ThreadsUsage::default()
    };
    if thread_ids.is_empty() {
        return Ok(usage);
    }
    let ids = thread_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT thread_id, \
                SUM(COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)) AS tokens, \
                SUM(cost_usd) AS cost_usd \
         FROM turns WHERE thread_id IN ({ids}) GROUP BY thread_id"
    );
    let result = super::query_path(path, &sql, thread_ids.len().max(1))?;
    for row in &result.rows {
        let tokens = row.get("tokens").and_then(Value::as_f64).unwrap_or(0.0);
        usage.tokens = usage.tokens.saturating_add(tokens.max(0.0) as u64);
        if let Some(cost) = row.get("cost_usd").and_then(Value::as_f64) {
            usage.cost_usd += cost;
            usage.threads_with_cost += 1;
        }
    }
    Ok(usage)
}

/// Recorded usage per thread for `thread_ids` in the user's store.
pub fn thread_usage_map(thread_ids: &[String]) -> Result<HashMap<String, ThreadRunUsage>, String> {
    match super::data_path() {
        Some(path) if path.is_file() => thread_usage_map_at(&path, thread_ids),
        _ => Ok(HashMap::new()),
    }
}

/// [`thread_usage_map`] against one store file.
pub fn thread_usage_map_at(
    path: &Path,
    thread_ids: &[String],
) -> Result<HashMap<String, ThreadRunUsage>, String> {
    let mut map = HashMap::new();
    if thread_ids.is_empty() {
        return Ok(map);
    }
    let ids = thread_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT thread_id, \
                SUM(CASE WHEN input_tokens IS NOT NULL OR output_tokens IS NOT NULL \
                    THEN COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0) \
                    ELSE NULL END) AS tokens, \
                SUM(cost_usd) AS cost_usd \
         FROM turns WHERE thread_id IN ({ids}) GROUP BY thread_id"
    );
    let result = super::query_path(path, &sql, thread_ids.len().max(1))?;
    for row in &result.rows {
        let Some(thread_id) = row.get("thread_id").and_then(Value::as_str) else {
            continue;
        };
        let tokens = row
            .get("tokens")
            .and_then(Value::as_f64)
            .map(|value| value.max(0.0) as u64);
        let cost_usd = row.get("cost_usd").and_then(Value::as_f64);
        if tokens.is_some() || cost_usd.is_some() {
            map.insert(thread_id.to_owned(), ThreadRunUsage { tokens, cost_usd });
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(
        thread: &str,
        minute: u32,
        tokens: (i64, i64),
        cost: Option<f64>,
    ) -> super::super::TurnRow {
        super::super::TurnRow {
            thread_id: thread.to_owned(),
            started_at: format!("2026-11-01T10:{minute:02}:00Z"),
            input_tokens: Some(tokens.0),
            output_tokens: Some(tokens.1),
            cost_usd: cost,
            status: "completed".to_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn sums_tokens_of_the_named_threads_and_counts_who_reported_cost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(super::super::DATA_FILE);
        let connection = super::super::open(&path).unwrap();
        for row in [
            turn("w1", 1, (100, 10), Some(0.5)),
            turn("w1", 2, (50, 5), Some(0.25)),
            turn("w2", 3, (200, 20), None),
            turn("other", 4, (9_000, 900), Some(9.0)),
        ] {
            super::super::write_turn(&connection, &row).unwrap();
        }
        drop(connection);

        let usage = threads_usage_at(&path, &["w1".into(), "w2".into(), "w3".into()]).unwrap();
        assert_eq!(usage.tokens, 385);
        assert!((usage.cost_usd - 0.75).abs() < 1e-9);
        assert_eq!(usage.threads_with_cost, 1);
        assert_eq!(usage.threads, 3);
        assert_eq!(threads_usage_at(&path, &[]).unwrap().tokens, 0);

        let per_thread =
            thread_usage_map_at(&path, &["w1".into(), "w2".into(), "w3".into()]).unwrap();
        assert_eq!(
            per_thread.get("w1"),
            Some(&ThreadRunUsage {
                tokens: Some(165),
                cost_usd: Some(0.75)
            })
        );
        assert_eq!(
            per_thread.get("w2"),
            Some(&ThreadRunUsage {
                tokens: Some(220),
                cost_usd: None
            })
        );
        assert!(!per_thread.contains_key("w3"));
    }
}
