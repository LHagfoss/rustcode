//! `_rustcode/config/files/{list,get,put,prune}`: the config directory's
//! skills, themes and prompt commands, copied between paired machines one
//! entry at a time (`crate::config_files`, #4485).

use crate::config_files::{self, ConfigKind, Entry};
use serde_json::Value;

/// Every skill, theme and command entry of `kinds` (all three when empty),
/// with a content hash and no content.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/files/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListRequest {
    #[serde(default)]
    pub kinds: Vec<ConfigKind>,
}

/// One entry with its files, base64 encoded.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/files/get", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GetRequest {
    pub kind: ConfigKind,
    pub name: String,
}

/// Write one entry as the copy `source` (that machine's id) sent.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/files/put", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutRequest {
    pub source: String,
    pub entry: Entry,
}

/// Remove the `kind` copies taken from `source` that `keep` no longer names.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/config/files/prune", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PruneRequest {
    pub source: String,
    pub kind: ConfigKind,
    #[serde(default)]
    pub keep: Vec<String>,
}

fn config_dir() -> Result<std::path::PathBuf, String> {
    crate::config::get_config_dir().ok_or_else(|| "there is no configuration directory".to_owned())
}

fn answer(method: &str, params: Value) -> Result<Value, agent_client_protocol::Error> {
    let invalid = |error: String| agent_client_protocol::Error::invalid_params().data(error);
    let dir = config_dir().map_err(invalid)?;
    match method {
        "_rustcode/config/files/list" => {
            let request: ListRequest = super::parse_params(params)?;
            let kinds = if request.kinds.is_empty() {
                ConfigKind::ALL.to_vec()
            } else {
                request.kinds
            };
            Ok(serde_json::json!({ "entries": config_files::list(&dir, &kinds) }))
        }
        "_rustcode/config/files/get" => {
            let request: GetRequest = super::parse_params(params)?;
            let entry = config_files::read(&dir, request.kind, &request.name).map_err(invalid)?;
            serde_json::to_value(entry).map_err(|error| {
                agent_client_protocol::Error::internal_error().data(error.to_string())
            })
        }
        "_rustcode/config/files/put" => {
            let request: PutRequest = super::parse_params(params)?;
            let outcome =
                config_files::put(&dir, &request.source, &request.entry).map_err(invalid)?;
            Ok(serde_json::json!({ "outcome": outcome }))
        }
        _ => {
            let request: PruneRequest = super::parse_params(params)?;
            let removed = config_files::prune(&dir, &request.source, request.kind, &request.keep)
                .map_err(invalid)?;
            Ok(serde_json::json!({ "removed": removed }))
        }
    }
}

pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    if !matches!(
        method.as_str(),
        "_rustcode/config/files/list"
            | "_rustcode/config/files/get"
            | "_rustcode/config/files/put"
            | "_rustcode/config/files/prune"
    ) {
        return super::browser_panel::dispatch(method, params, responder, connection, unhandled);
    }
    connection.spawn(async move {
        match tokio::task::spawn_blocking(move || answer(&method, params)).await {
            Ok(Ok(value)) => responder.respond(value)?,
            Ok(Err(error)) => responder.respond_with_error(error)?,
            Err(error) => responder.respond_with_error(
                agent_client_protocol::Error::internal_error().data(error.to_string()),
            )?,
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_read_what_the_app_sends() {
        let list: ListRequest = serde_json::from_str(r#"{"kinds":["skills","themes"]}"#).unwrap();
        assert_eq!(list.kinds, vec![ConfigKind::Skills, ConfigKind::Themes]);
        let put: PutRequest = serde_json::from_str(
            r#"{"source":"gw-1","entry":{"kind":"commands","name":"review.md","files":[{"path":".","content":"eA=="}]}}"#,
        )
        .unwrap();
        assert_eq!(put.entry.kind, ConfigKind::Commands);
        let prune: PruneRequest =
            serde_json::from_str(r#"{"source":"gw-1","kind":"skills"}"#).unwrap();
        assert!(prune.keep.is_empty());
    }
}
