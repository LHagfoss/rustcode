//! `_rustcode/remote/access`: Settings › Machines › Access sets how much a
//! paired computer may drive here (`remote_gateway::access`). The gateway
//! applies a lowered level to that computer's open connections at once.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::remote_gateway::access::Access;

/// Set a paired device's ACP ceiling: `none`, `read`, `threads` or `full`.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[serde(rename_all = "camelCase")]
#[request(method = "_rustcode/remote/access", response = serde_json::Value)]
pub(crate) struct RemoteAccessRequest {
    pub device_id: String,
    pub access: String,
}

/// The level now set, or why it could not be.
pub(super) async fn set(
    request: RemoteAccessRequest,
) -> Result<Value, agent_client_protocol::Error> {
    let refuse = |message: String| agent_client_protocol::Error::invalid_params().data(message);
    let level = Access::parse(&request.access)
        .ok_or_else(|| refuse(format!("unknown access level '{}'", request.access)))?;
    let directory = crate::config::get_config_dir()
        .ok_or_else(|| refuse("no configuration directory".to_owned()))?;
    crate::remote_gateway::lifecycle::RemoteLifecycle::new(&directory)
        .set_access(&request.device_id, level)
        .await
        .map_err(|error| refuse(format!("{error:#}")))?;
    Ok(json!({ "deviceId": request.device_id, "access": level.name() }))
}
