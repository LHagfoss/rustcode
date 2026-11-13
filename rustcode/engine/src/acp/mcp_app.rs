//! `_rustcode/mcp/app`: what a client's MCP App frame needs from the MCP server
//! that ran a tool call (#3607). Two actions, both for the built-in harness,
//! whose MCP connections RustCode owns (another harness owns its servers and
//! the app shows the view it embedded, display-only):
//!
//! - `read` fetches the UI resource the tool declares in `_meta.ui.resourceUri`,
//!   from the exact connection that serves the call's tool name.
//! - `call` runs a tool the app asks for on that same connection. It is a tool
//!   call like the agent's: the turn policy decides (plan mode refuses, an MCP
//!   tool always asks), the approval prompt names the app as its origin, and
//!   only tools the server marks `_meta.ui.visibility: ["app"]` can run.

use super::clients::SessionClients;
use super::permissions::AcpPolicy;
use crate::network::policy::TurnPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// An approval whose call id starts with this came from an MCP App's frame,
/// and its prompt says so.
pub(crate) const APP_CALL_PREFIX: &str = "mcp-app:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum McpAppAction {
    Read,
    Call,
}

#[derive(Debug, Clone, Serialize, Deserialize, agent_client_protocol::JsonRpcRequest)]
#[request(method = "_rustcode/mcp/app", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpAppRequest {
    pub session_id: String,
    pub action: McpAppAction,
    /// The tool call the app belongs to, by the name the transcript shows.
    pub name: String,
    /// `call`: the app's tool, a name of the same server.
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub arguments: Option<Value>,
}

/// The prompt title for a call that came from an app: its origin first.
pub(crate) fn origin_title(call_id: Option<&str>, title: &str) -> String {
    match call_id {
        Some(id) if id.starts_with(APP_CALL_PREFIX) => format!("MCP App requests {title}"),
        _ => title.to_owned(),
    }
}

fn refused(message: impl Into<String>) -> agent_client_protocol::Error {
    agent_client_protocol::Error::invalid_params().data(message.into())
}

/// Answer one request. `builtin` is false for a thread another harness runs.
pub(crate) async fn handle(
    request: McpAppRequest,
    builtin: bool,
    state: Arc<tokio::sync::Mutex<crate::app::AppState>>,
    clients: SessionClients,
    auto_approve: bool,
) -> Result<Value, agent_client_protocol::Error> {
    if !builtin {
        return Err(refused(
            "this harness runs its own MCP servers; the app is display-only here",
        ));
    }
    let Some((client, raw)) = crate::tools::resolve_mcp_tool_call(&request.name) else {
        return Err(refused(
            "the MCP server that ran this call is not connected",
        ));
    };
    match request.action {
        McpAppAction::Read => {
            let app = client.read_tool_app(&raw).await.map_err(refused)?;
            Ok(json!({ "app": app.map(|app| json!({
                "uri": app.uri,
                "mimeType": app.mime_type,
                "html": app.html,
                "totalBytes": app.total_bytes,
            })) }))
        }
        McpAppAction::Call => {
            let tool = request
                .tool
                .filter(|tool| !tool.is_empty())
                .ok_or_else(|| refused("a tool name is required"))?;
            let arguments = request.arguments.unwrap_or_else(|| json!({}));
            let policy = AcpPolicy {
                clients,
                session_id: request.session_id,
                auto_approve,
            };
            let approval_name = crate::tools::mcp_canonical_name(&client.name, &tool);
            let result = client
                .call_app_tool(&tool, arguments, |_, tool, arguments| {
                    let call = crate::tools::ToolCall {
                        name: approval_name,
                        arguments: arguments.clone(),
                        call_id: Some(format!("{APP_CALL_PREFIX}{}:{tool}", uuid::Uuid::new_v4())),
                    };
                    let state = Arc::clone(&state);
                    async move { policy.should_approve(&state, &[call]).await }
                })
                .await
                .map_err(refused)?;
            Ok(json!({ "result": result }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_use_camel_case_and_snake_actions() {
        let request: McpAppRequest = serde_json::from_value(json!({
            "sessionId": "s", "action": "call", "name": "mcp__w__show",
            "tool": "refresh", "arguments": {"a": 1}
        }))
        .unwrap();
        assert_eq!(request.action, McpAppAction::Call);
        assert_eq!(request.tool.as_deref(), Some("refresh"));
        assert!(
            serde_json::from_value::<McpAppRequest>(json!({
                "sessionId": "s", "action": "write", "name": "x"
            }))
            .is_err()
        );
    }

    #[test]
    fn an_app_origin_is_named_in_the_prompt() {
        assert_eq!(
            origin_title(Some("mcp-app:1:refresh"), "mcp__w__refresh"),
            "MCP App requests mcp__w__refresh"
        );
        assert_eq!(
            origin_title(Some("call_1"), "mcp__w__refresh"),
            "mcp__w__refresh"
        );
        assert_eq!(origin_title(None, "x"), "x");
    }

    #[tokio::test]
    async fn another_harness_is_refused_before_any_server_is_touched() {
        let state = Arc::new(tokio::sync::Mutex::new(crate::app::AppState::new()));
        let clients = super::super::clients::Clients::default().session("s");
        let request = McpAppRequest {
            session_id: "s".into(),
            action: McpAppAction::Call,
            name: "mcp__w__show".into(),
            tool: Some("refresh".into()),
            arguments: None,
        };
        let error = handle(request, false, state, clients, true)
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("display-only"));
    }

    #[tokio::test]
    async fn an_unconnected_server_is_refused() {
        let state = Arc::new(tokio::sync::Mutex::new(crate::app::AppState::new()));
        let clients = super::super::clients::Clients::default().session("s");
        let request = McpAppRequest {
            session_id: "s".into(),
            action: McpAppAction::Read,
            name: "mcp__nowhere__show".into(),
            tool: None,
            arguments: None,
        };
        let error = handle(request, true, state, clients, true)
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("not connected"));
    }
}
