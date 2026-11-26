//! `_rustcode/issues`: read a project's GitHub issues through `gh`.

use std::path::PathBuf;

use serde_json::Value;

/// List GitHub issues for `cwd`, filtered by `state`, `search`, and `labels`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/issues", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssuesRequest {
    pub cwd: PathBuf,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Dispatch GitHub issue methods, forwarding anything else to `memory::dispatch`.
pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    match method.as_str() {
        "_rustcode/issues" => {
            let request: IssuesRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let listed = crate::github_issues::list(
                    &request.cwd,
                    request.state.as_deref().unwrap_or("open"),
                    request.search.as_deref(),
                    &request.labels,
                    request.limit.unwrap_or(50),
                )
                .await;
                responder.respond(match listed {
                    Ok(list) => serde_json::json!({ "list": list }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        _ => super::memory::dispatch(method, params, responder, connection, unhandled),
    }
}
