//! `_rustcode/data/usage` and `_rustcode/data/query`: the local data store
//! (`docs/data-store.md`) for the app, which asks on an agent's behalf for
//! `thread_usage` and `query_data`. Both are read-only and need no session.
//!
//! The engine only reads. The *scope* is the app's decision: it passes the
//! caller's project while "Let agents read threads in other projects" is off,
//! and it refuses `query_data` then, since SQL cannot be narrowed to a project
//! safely.

use serde_json::Value;

/// Bumped when these answers change shape incompatibly.
pub(crate) const DATA_METHOD_VERSION: u32 = 1;

/// `_rustcode/data/usage`: tokens and reported cost by harness, model, day,
/// project or thread, from the store `rustcode usage` reads.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/usage", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataUsageRequest {
    /// `YYYY-MM-DD` or `YYYY-MM`, checked again here.
    #[serde(default)]
    pub since: Option<String>,
    /// `harness` (default), `model`, `day`, `project` or `thread`.
    #[serde(default)]
    pub group_by: Option<String>,
    /// One project's absolute path; absent reads every project.
    #[serde(default)]
    pub project: Option<String>,
}

/// `_rustcode/data/query`: one read-only statement through the same guards
/// `rustcode data query` uses.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/query", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataQueryRequest {
    pub sql: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

pub(crate) fn usage(request: DataUsageRequest) -> Result<Value, String> {
    // The first reader of a fresh install runs the one-time backfill, as the CLI does.
    crate::data_store::ensure_ready()?;
    let since = match request
        .since
        .as_deref()
        .map(str::trim)
        .filter(|since| !since.is_empty())
    {
        Some(since) => Some(crate::data_store::parse_since(since)?),
        None => None,
    };
    let group_by = match request.group_by.as_deref().map(str::trim) {
        None | Some("") => crate::data_store::GroupBy::Harness,
        Some(name) => crate::data_store::GroupBy::parse(name).ok_or_else(|| {
            format!(
                "group_by must be one of {:?}",
                crate::data_store::GroupBy::ALL
            )
        })?,
    };
    let mut answer =
        crate::data_store::usage(since.as_deref(), group_by, request.project.as_deref())?;
    if let Some(object) = answer.as_object_mut() {
        object.insert("version".to_owned(), serde_json::json!(DATA_METHOD_VERSION));
    }
    Ok(answer)
}

pub(crate) fn query(request: DataQueryRequest) -> Result<Value, String> {
    crate::data_store::ensure_ready()?;
    let limit = request
        .limit
        .map(|limit| (limit as usize).clamp(1, crate::data_store::MAX_ROWS))
        .unwrap_or(crate::data_store::MAX_ROWS);
    let result = crate::data_store::query(&request.sql, limit)?;
    serde_json::to_value(result).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_bad_group_or_day_is_refused_before_the_store_is_opened() {
        let usage = |group: Option<&str>, since: Option<&str>| usage_request(group, since);
        assert_eq!(usage(Some("harness"), Some("2026-11")), Ok(()));
        assert_eq!(usage(Some("thread"), Some("2026-11-01")), Ok(()));
        assert!(usage(Some("tool"), None).is_err());
        assert!(usage(None, Some("last week")).is_err());
    }

    /// The checks `usage` runs first, without touching a store.
    fn usage_request(group: Option<&str>, since: Option<&str>) -> Result<(), String> {
        if let Some(since) = since {
            crate::data_store::parse_since(since)?;
        }
        match group {
            None => Ok(()),
            Some(name) => crate::data_store::GroupBy::parse(name)
                .map(|_| ())
                .ok_or_else(|| "bad group".to_owned()),
        }
    }
}
