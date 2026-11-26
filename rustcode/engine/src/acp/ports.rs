//! `_rustcode/ports/{list,kill}`: inspect listening TCP ports scoped to a
//! thread's working directory and terminate a verified listener process.

use std::path::PathBuf;

use serde_json::Value;

/// List TCP listening ports scoped to `cwd`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/ports/list", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListRequest {
    pub cwd: PathBuf,
}

/// Stop a listening process owned by `cwd` on `port`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/ports/kill", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KillRequest {
    pub cwd: PathBuf,
    pub pid: u32,
    pub port: u16,
}

/// Dispatch port inspection methods, forwarding anything else to `memory::dispatch`.
pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    match method.as_str() {
        "_rustcode/ports/list" => {
            let request: ListRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = tokio::task::spawn_blocking(move || crate::ports::list(&request.cwd))
                    .await
                    .map_err(|error| crate::ports::PortError::new("failed", error.to_string()))
                    .and_then(|res| res);
                responder.respond(match outcome {
                    Ok(list) => serde_json::json!({ "list": list }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/ports/kill" => {
            let request: KillRequest = super::parse_params(params)?;
            connection.spawn(async move {
                let outcome = tokio::task::spawn_blocking(move || {
                    crate::ports::kill(&request.cwd, request.pid, request.port)
                })
                .await
                .map_err(|error| crate::ports::PortError::new("failed", error.to_string()))
                .and_then(|res| res);
                responder.respond(match outcome {
                    Ok(list) => serde_json::json!({ "list": list }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        _ => super::memory::dispatch(method, params, responder, connection, unhandled),
    }
}
