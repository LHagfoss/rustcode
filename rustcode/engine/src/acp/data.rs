//! `_rustcode/data/usage`, `_rustcode/data/query` and `_rustcode/data/activity`:
//! the local data store (`docs/data-store.md`) for the app, which asks on an
//! agent's behalf for `thread_usage`, `query_data` and `read_activity`. All
//! three are read-only and need no session.
//!
//! `_rustcode/data/links/load` and `_rustcode/data/links/save` keep the app's
//! link book (who started whom, and what may be done without asking again) in
//! the same store, so a reinstall or a second window does not lose it. They
//! need no session either.
//!
//! The engine only reads turns; the *scope* is the app's decision: it passes
//! the caller's project while "Let agents read threads in other projects" is off,
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

/// `_rustcode/data/activity`: one page of a thread's recorded activity, merged
/// from its operational events, provider attempts and harness messages.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/activity", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataActivityRequest {
    pub session_id: String,
    /// The `next` token an earlier page returned; absent starts at the oldest.
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// How large a link book may be: far more than the app's own bound (a few
/// hundred links), so a body over this is a mistake, not a big book.
pub(crate) const MAX_LINK_BOOK_BYTES: usize = 4 * 1024 * 1024;

/// `_rustcode/data/links/load`: the link book, and whether one was saved.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/links/load", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataLinksLoadRequest {}

/// `_rustcode/data/links/save`: replaces the link book with `book`.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/links/save", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataLinksSaveRequest {
    /// The app's `LinkBook`: `links`, `grants`, `counters` and `fanOuts`.
    pub book: Value,
}

pub(crate) fn links_load() -> Result<Value, String> {
    crate::data_store::load_link_book()
}

pub(crate) fn links_save(request: DataLinksSaveRequest) -> Result<Value, String> {
    if !request.book.is_object() {
        return Err("book must be a JSON object".to_owned());
    }
    let size = serde_json::to_vec(&request.book)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_LINK_BOOK_BYTES {
        return Err(format!(
            "the link book is {size} bytes; at most {MAX_LINK_BOOK_BYTES} are kept"
        ));
    }
    crate::data_store::save_link_book(&request.book)?;
    Ok(serde_json::json!({ "saved": true, "bytes": size }))
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

pub(crate) fn activity(request: DataActivityRequest) -> Result<Value, String> {
    if request.session_id.trim().is_empty() {
        return Err("session_id is required".to_owned());
    }
    // The first reader of a fresh install migrates the store once, as
    // `thread_usage` does; the page itself is still read read-only.
    crate::data_store::ensure_ready()?;
    // A page is bounded here as well as in the store: a caller cannot ask for
    // the whole table in one answer.
    let limit = request
        .limit
        .map(|limit| limit as usize)
        .unwrap_or(crate::data_store::DEFAULT_ACTIVITY_ITEMS)
        .clamp(1, crate::data_store::MAX_ACTIVITY_ITEMS);
    let page = crate::data_store::activity(&request.session_id, request.after.as_deref(), limit)?;
    Ok(serde_json::json!({
        "session_id": request.session_id,
        "items": page.items,
        "next": page.next,
        "has_more": page.has_more,
        "version": DATA_METHOD_VERSION,
    }))
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

    #[test]
    fn an_activity_page_is_bounded_before_the_store_is_opened() {
        // An empty session is refused without a store; the limit is clamped to
        // the store's page cap rather than erroring.
        let refused = super::activity(super::DataActivityRequest {
            session_id: "  ".to_owned(),
            after: None,
            limit: None,
        });
        assert_eq!(refused.unwrap_err(), "session_id is required");
    }

    #[test]
    fn a_link_book_over_the_bound_is_refused_before_it_reaches_the_store() {
        // A body over the bound is a mistake, not a big book: refused before
        // the store is opened, so this needs no config directory.
        let refused = super::links_save(super::DataLinksSaveRequest {
            book: serde_json::json!({"links": [{"message": "x".repeat(super::MAX_LINK_BOOK_BYTES)}]}),
        });
        assert!(refused.unwrap_err().contains("at most"));
        let not_an_object = super::links_save(super::DataLinksSaveRequest {
            book: serde_json::json!([1, 2, 3]),
        });
        assert_eq!(not_an_object.unwrap_err(), "book must be a JSON object");
    }
}
