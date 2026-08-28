use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Client, ConnectionTo};
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ApprovalRequirement {
    Allow,
    Request,
    Deny(String),
}

pub(crate) fn approval_requirement(
    tool_calls: &[crate::tools::ToolCall],
    mode: crate::config::AgentMode,
    auto_approve: bool,
) -> ApprovalRequirement {
    let mut requires_permission = false;
    for call in tool_calls {
        match crate::tools::authorize_tool_with_args(
            &call.name,
            &call.arguments,
            mode,
            auto_approve,
            false,
        ) {
            crate::tools::AuthorizationDecision::Allow => {}
            crate::tools::AuthorizationDecision::RequireConfirmation => {
                requires_permission = true;
            }
            crate::tools::AuthorizationDecision::Deny(reason) => {
                return ApprovalRequirement::Deny(reason);
            }
        }
    }
    if requires_permission {
        ApprovalRequirement::Request
    } else {
        ApprovalRequirement::Allow
    }
}

pub(crate) struct AcpPolicy {
    pub(crate) connection: ConnectionTo<Client>,
    pub(crate) session_id: String,
    pub(crate) auto_approve: bool,
}

impl crate::network::policy::TurnPolicy for AcpPolicy {
    fn should_approve(
        &self,
        state: &Arc<tokio::sync::Mutex<crate::app::AppState>>,
        tool_calls: &[crate::tools::ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let connection = self.connection.clone();
        let session_id = self.session_id.clone();
        let auto_approve = self.auto_approve;
        let state = Arc::clone(state);
        let tool_calls = tool_calls.to_vec();
        async move {
            let mode = state.lock().await.agent_mode;
            if let ApprovalRequirement::Deny(_) =
                approval_requirement(&tool_calls, mode, auto_approve)
            {
                return false;
            }
            if auto_approve {
                return true;
            }

            for (index, call) in tool_calls.iter().enumerate() {
                match crate::tools::authorize_tool_with_args(
                    &call.name,
                    &call.arguments,
                    mode,
                    false,
                    false,
                ) {
                    crate::tools::AuthorizationDecision::Allow => continue,
                    crate::tools::AuthorizationDecision::Deny(_) => return false,
                    crate::tools::AuthorizationDecision::RequireConfirmation => {}
                }

                let call_id = call
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("acp-{index}-{}", call.name));
                let tool_call = ToolCallUpdate::new(
                    call_id,
                    ToolCallUpdateFields::new()
                        .title(call.name.clone())
                        .raw_input(call.arguments.clone()),
                );
                let request = RequestPermissionRequest::new(
                    session_id.clone(),
                    tool_call,
                    vec![
                        PermissionOption::new(
                            "allow_once",
                            "Allow once",
                            PermissionOptionKind::AllowOnce,
                        ),
                        PermissionOption::new(
                            "reject_once",
                            "Reject",
                            PermissionOptionKind::RejectOnce,
                        ),
                    ],
                );
                let Ok(response) = connection.send_request(request).block_task().await else {
                    return false;
                };
                let approved = matches!(
                    response.outcome,
                    RequestPermissionOutcome::Selected(selected)
                        if selected.option_id.0.as_ref() == "allow_once"
                );
                if !approved {
                    return false;
                }
            }
            true
        }
    }

    fn should_verify_completion(&self) -> bool {
        true
    }
}
