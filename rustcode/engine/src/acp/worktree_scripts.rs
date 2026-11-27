//! `_rustcode/worktrees/scripts/{state,trust,action}`: read project worktree
//! lifecycle script state, record explicit project trust, and run `setup`,
//! `teardown`, or the on-demand `run` process.

use std::path::PathBuf;

use serde_json::Value;

/// Read a worktree or project folder's lifecycle script config, trust, setup
/// status, and managed `run` process state.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/worktrees/scripts/state", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StateRequest {
    pub path: PathBuf,
}

/// Record or revoke explicit user trust for a project's `.rustcode/worktrees.toml`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/worktrees/scripts/trust", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrustRequest {
    pub path: PathBuf,
    #[serde(default)]
    pub sha256: Option<String>,
    pub trusted: bool,
}

/// Trigger one lifecycle script action: `setup`, `teardown`, `run_start`, or `run_stop`.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/worktrees/scripts/action", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionRequest {
    pub path: PathBuf,
    pub action: String,
}

/// Dispatch worktree script methods, forwarding anything else to `director::dispatch`.
pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    match method.as_str() {
        "_rustcode/worktrees/scripts/state" => {
            let request: StateRequest = super::parse_params(params)?;
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || {
                    crate::worktree_scripts::state(&request.path)
                })
                .await
                {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/scripts/trust" => {
            let request: TrustRequest = super::parse_params(params)?;
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || {
                    crate::worktree_scripts::set_trust(
                        &request.path,
                        request.sha256.as_deref(),
                        request.trusted,
                    )
                })
                .await
                {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/scripts/action" => {
            let request: ActionRequest = super::parse_params(params)?;
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || {
                    crate::worktree_scripts::run_action(&request.path, &request.action)
                })
                .await
                {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        _ => super::director::dispatch(method, params, responder, connection, unhandled),
    }
}
