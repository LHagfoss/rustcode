//! `_rustcode/director/next`: the stopped thread a Director run checks
//! (`crate::director`). The scheduler daemon asks it of the running app,
//! which knows which threads are working right now.

use serde_json::{Value, json};

/// The next stopped thread of the project at `workspace`, as the ping a
/// Director run appends to its prompt; `ping` is null when none stopped.
#[derive(
    Debug,
    Clone,
    Default,
    serde::Serialize,
    serde::Deserialize,
    agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/director/next", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NextRequest {
    pub workspace: String,
}

/// Dispatch the Director's method, forwarding anything else to `desktop_call::dispatch`.
pub(super) fn dispatch(
    method: String,
    params: Value,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    match method.as_str() {
        "_rustcode/director/next" => {
            let request: NextRequest = super::parse_params(params)?;
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || crate::director::next(&request.workspace))
                    .await
                {
                    Ok(Ok(ping)) => responder.respond(json!({ "ping": ping }))?,
                    Ok(Err(error)) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    )?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        _ => super::desktop_call::dispatch(method, params, responder, connection, unhandled),
    }
}
