//! `_rustcode/desktop/call`: run a desktop-control tool on this computer on
//! behalf of the local RustCode app (which owns the user's approval dialog).
//!
//! Deliberately absent from `remote_gateway::acp_relay`: no remote machine can
//! invoke this method directly over the network.

use serde_json::Value;

/// `_rustcode/desktop/call`: execute one `computer_*` tool on this computer.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/desktop/call", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopCallRequest {
    pub session_id: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub caller_machine: Option<String>,
    #[serde(default)]
    pub approval: Option<String>,
    #[serde(default)]
    pub approved_app: Option<String>,
}

pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    if method == "_rustcode/desktop/call" {
        let request: DesktopCallRequest = super::parse_params(params)?;
        return connection.spawn(async move {
            let call = crate::computer_use::desktop::BridgeCall {
                session_id: request.session_id,
                tool: request.tool,
                args: request.args,
                caller_machine: request.caller_machine,
                approval: request.approval,
                approved_app: request.approved_app,
            };
            match crate::computer_use::desktop::bridge(
                call,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            {
                Ok(answer) => responder.respond(answer)?,
                Err(error) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(error),
                )?,
            }
            Ok(())
        });
    }
    super::github_issues::dispatch(method, params, responder, connection, unhandled)
}
