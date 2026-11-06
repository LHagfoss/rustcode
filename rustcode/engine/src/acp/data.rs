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

/// `_rustcode/data/receipts/get`: what the caller's earlier call under the same
/// retry key did — `none`, its recorded `result`, or `conflict` when the key
/// was used for another action or other arguments. The app asks before it acts;
/// the check is what makes a retried `start_session`, `message_session` or
/// `fan_out` safe.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/receipts/get", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataReceiptsGetRequest {
    /// The thread making the call.
    pub caller: String,
    /// Its retry key: the call's `client_request_id`.
    pub client_request_id: String,
    /// The orchestration tool the call names.
    pub action: String,
    /// The engine's sha256 of the action and the call's arguments.
    pub args_hash: String,
}

/// `_rustcode/data/receipts/put`: records what a call did, right after it
/// succeeded and before the answer goes back, so a retry whose answer was
/// dropped is answered from here instead of acting again.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/data/receipts/put", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataReceiptsPutRequest {
    pub caller: String,
    pub client_request_id: String,
    pub action: String,
    pub args_hash: String,
    /// The answer the call gave.
    pub result: Value,
}

pub(crate) fn receipts_get(request: DataReceiptsGetRequest) -> Result<Value, String> {
    let caller = non_empty(&request.caller, "caller")?;
    let key = client_request_id(&request.client_request_id)?;
    let action = non_empty(&request.action, "action")?;
    let hash = non_empty(&request.args_hash, "args_hash")?;
    match crate::data_store::check_receipt(caller, key, action, hash)? {
        crate::data_store::Receipt::None => Ok(serde_json::json!({ "state": "none" })),
        crate::data_store::Receipt::Conflict => Ok(serde_json::json!({ "state": "conflict" })),
        crate::data_store::Receipt::Replay { result, created_at } => Ok(serde_json::json!({
            "state": "replayed",
            "result": result,
            "created_at": created_at,
        })),
    }
}

pub(crate) fn receipts_put(request: DataReceiptsPutRequest) -> Result<Value, String> {
    let caller = non_empty(&request.caller, "caller")?;
    let key = client_request_id(&request.client_request_id)?;
    let action = non_empty(&request.action, "action")?;
    let hash = non_empty(&request.args_hash, "args_hash")?;
    if !request.result.is_object() {
        return Err("result must be a JSON object".to_owned());
    }
    let result_json = serde_json::to_string(&request.result).map_err(|error| error.to_string())?;
    let receipt = crate::data_store::ReceiptRow {
        caller: caller.to_owned(),
        client_request_id: key.to_owned(),
        action: action.to_owned(),
        args_hash: hash.to_owned(),
        result_json,
    };
    crate::data_store::record_receipt(&receipt)?;
    Ok(serde_json::json!({ "saved": true }))
}

fn non_empty<'a>(value: &'a str, field: &str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} is required"));
    }
    Ok(value)
}

fn client_request_id(value: &str) -> Result<&str, String> {
    let value = non_empty(value, "client_request_id")?;
    crate::orchestration::check_request_id(value)?;
    Ok(value)
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
    fn a_receipt_needs_a_caller_and_a_valid_key_before_the_store_is_opened() {
        // Refused before the store is opened, so this needs no config directory.
        let get = |caller: &str, key: &str| {
            super::receipts_get(super::DataReceiptsGetRequest {
                caller: caller.to_owned(),
                client_request_id: key.to_owned(),
                action: "start_session".to_owned(),
                args_hash: "h1".to_owned(),
            })
        };
        assert_eq!(get("", "k1").unwrap_err(), "caller is required");
        assert_eq!(get("a", "  ").unwrap_err(), "client_request_id is required");
        assert!(
            get("a", "has space")
                .unwrap_err()
                .contains("must be 1 to 128")
        );
        let put = super::receipts_put(super::DataReceiptsPutRequest {
            caller: "a".to_owned(),
            client_request_id: "k1".to_owned(),
            action: "start_session".to_owned(),
            args_hash: "h1".to_owned(),
            result: serde_json::json!([1, 2]),
        });
        assert_eq!(put.unwrap_err(), "result must be a JSON object");
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
