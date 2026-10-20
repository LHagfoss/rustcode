//! `_rustcode/capabilities` requests (#2165): the macOS Settings page lists
//! skills and MCP servers and toggles them through the engine process that
//! actually runs the servers, so "connected" is never a guess.

use serde::{Deserialize, Serialize};

/// List every discovered skill and configured MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/capabilities", response = serde_json::Value)]
pub(crate) struct CapabilitiesRequest {}

/// What a toggle applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CapabilityKind {
    Skill,
    Mcp,
}

/// Turn one skill or MCP server on or off; answers with the fresh snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/capabilities/set", response = serde_json::Value)]
pub(crate) struct SetCapabilityRequest {
    pub kind: CapabilityKind,
    pub name: String,
    pub enabled: bool,
}

pub(crate) fn snapshot_value() -> Result<serde_json::Value, agent_client_protocol::Error> {
    serde_json::to_value(crate::capabilities::snapshot())
        .map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))
}

/// Apply `request` and return the snapshot after it. A failure to save is an
/// error; a server that fails to start is reported in the snapshot instead.
pub(crate) async fn set_capability(
    request: SetCapabilityRequest,
) -> Result<serde_json::Value, agent_client_protocol::Error> {
    let outcome = match request.kind {
        CapabilityKind::Skill => {
            crate::capabilities::set_skill_enabled(&request.name, request.enabled).map(|_| ())
        }
        CapabilityKind::Mcp => {
            crate::capabilities::set_mcp_server_enabled(&request.name, request.enabled)
                .await
                .map(|_| ())
        }
    };
    outcome.map_err(|message| agent_client_protocol::Error::invalid_params().data(message))?;
    snapshot_value()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::JsonRpcMessage;

    #[test]
    fn requests_use_the_rustcode_extension_methods() {
        assert!(CapabilitiesRequest::matches_method(
            "_rustcode/capabilities"
        ));
        assert!(SetCapabilityRequest::matches_method(
            "_rustcode/capabilities/set"
        ));
        let request: SetCapabilityRequest = serde_json::from_value(
            serde_json::json!({"kind": "mcp", "name": "github", "enabled": false}),
        )
        .unwrap();
        assert_eq!(request.kind, CapabilityKind::Mcp);
        assert!(!request.enabled);
        let empty: CapabilitiesRequest = serde_json::from_value(serde_json::json!({})).unwrap();
        let _ = empty;
    }
}
