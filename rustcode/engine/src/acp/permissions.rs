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

const ALLOW_ONCE: &str = "allow_once";
const ALLOW_ALWAYS: &str = "allow_always";
const REJECT_ONCE: &str = "reject_once";

/// The choices offered for one call. A plain `run_command` that the TUI can
/// remember as a reusable prefix also gets "Always allow"; clients that do
/// not know the kind just show another option.
fn permission_options(call: &crate::tools::ToolCall) -> Vec<PermissionOption> {
    let mut options = vec![PermissionOption::new(
        ALLOW_ONCE,
        "Allow once",
        PermissionOptionKind::AllowOnce,
    )];
    if let Some(prefix) = rememberable_prefix(call) {
        options.push(PermissionOption::new(
            ALLOW_ALWAYS,
            format!("Always allow `{prefix}`"),
            PermissionOptionKind::AllowAlways,
        ));
    }
    options.push(PermissionOption::new(
        REJECT_ONCE,
        "Reject",
        PermissionOptionKind::RejectOnce,
    ));
    options
}

fn rememberable_prefix(call: &crate::tools::ToolCall) -> Option<String> {
    (call.name == "run_command")
        .then(|| crate::tools::rememberable_command_prefix_for_call(&call.arguments))
        .flatten()
}

#[derive(Debug, PartialEq, Eq)]
enum PermissionChoice {
    AllowOnce,
    AllowAlways,
    Reject,
}

fn permission_choice(outcome: &RequestPermissionOutcome) -> PermissionChoice {
    match outcome {
        RequestPermissionOutcome::Selected(selected) => match selected.option_id.0.as_ref() {
            ALLOW_ONCE => PermissionChoice::AllowOnce,
            ALLOW_ALWAYS => PermissionChoice::AllowAlways,
            _ => PermissionChoice::Reject,
        },
        _ => PermissionChoice::Reject,
    }
}

/// Whether a prefix the user already chose to always allow covers this call,
/// so it is not asked again (same rule as the interactive policy).
fn covered_by_saved_prefix(
    config: &crate::config::AppConfig,
    call: &crate::tools::ToolCall,
) -> bool {
    crate::tools::approved_command_prefix_covers_call(
        &call.name,
        &call.arguments,
        &config.approved_command_prefixes,
    ) && !crate::tools::denied_command_prefix_covers_call(
        &call.name,
        &call.arguments,
        &config.denied_command_prefixes,
    )
}

/// Remember an "Always allow" answer the way the TUI's remember choice does.
fn remember_approved_prefix(state: &mut crate::app::AppState, call: &crate::tools::ToolCall) {
    let Some(prefix) = rememberable_prefix(call) else {
        return;
    };
    let stored_prefix = crate::tools::persisted_approved_command_prefix(&prefix);
    if !state
        .config
        .approved_command_prefixes
        .contains(&stored_prefix)
    {
        state.config.approved_command_prefixes.push(stored_prefix);
        crate::config::save_entire_config(&state.config);
    }
}

/// Time spent waiting on the client's answer is user wait, not agent work:
/// the tool batch moves it into the turn's `user_wait_us` (#1891).
fn record_approval_wait(state: &mut crate::app::AppState, wait: std::time::Duration) {
    state.approval_wait += wait;
    state.exclude_user_wait(wait);
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
                if covered_by_saved_prefix(&state.lock().await.config, call) {
                    crate::dbg_log!("ACP approval skipped: saved prefix covers {}", call.name);
                    continue;
                }

                let call_id = call
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("acp-{index}-{}", call.name));
                // The client shows this to the user, so an MCP registration
                // names what it would launch and leaves secret values out.
                let mcp_approval =
                    crate::tools::mcp_servers_approval(&call.name, &call.arguments, None);
                let tool_call =
                    ToolCallUpdate::new(
                        call_id,
                        ToolCallUpdateFields::new()
                            .title(mcp_approval.as_ref().map_or_else(
                                || call.name.clone(),
                                |approval| format!("{} {}", call.name, approval.label),
                            ))
                            .raw_input(mcp_approval.map_or_else(
                                || call.arguments.clone(),
                                |approval| approval.arguments,
                            )),
                    );
                let request = RequestPermissionRequest::new(
                    session_id.clone(),
                    tool_call,
                    permission_options(call),
                );
                state
                    .lock()
                    .await
                    .begin_user_wait(std::time::Instant::now());
                let wait_started = std::time::Instant::now();
                let response = connection.send_request(request).block_task().await;
                let wait = wait_started.elapsed();
                record_approval_wait(&mut *state.lock().await, wait);
                let choice = response
                    .as_ref()
                    .map_or(PermissionChoice::Reject, |response| {
                        permission_choice(&response.outcome)
                    });
                crate::dbg_log!(
                    "ACP approval for {} answered after {} ms: {:?}{}",
                    call.name,
                    wait.as_millis(),
                    choice,
                    if response.is_err() {
                        " (request failed)"
                    } else {
                        ""
                    }
                );
                match choice {
                    PermissionChoice::AllowOnce => {}
                    PermissionChoice::AllowAlways => {
                        remember_approved_prefix(&mut *state.lock().await, call);
                    }
                    PermissionChoice::Reject => return false,
                }
            }
            true
        }
    }

    fn should_verify_completion(&self) -> bool {
        true
    }

    /// An ACP turn is the regular interactive turn, so `_rustcode/steer` can
    /// hand it a message mid-turn. Without this the marker stayed unset and
    /// every steer the client sent was refused and queued instead.
    fn supports_live_turn_steering(&self) -> bool {
        acp_turn_supports_live_steering()
    }
}

/// An ACP turn is the regular interactive turn (#2255). Named so the decision
/// is covered by a test without constructing an ACP connection.
pub(crate) fn acp_turn_supports_live_steering() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::SelectedPermissionOutcome;

    fn run_command(command: &str) -> crate::tools::ToolCall {
        crate::tools::ToolCall {
            name: "run_command".to_owned(),
            arguments: serde_json::json!({ "command": command }),
            call_id: Some("call-1".to_owned()),
        }
    }

    fn option_ids(call: &crate::tools::ToolCall) -> Vec<String> {
        permission_options(call)
            .into_iter()
            .map(|option| option.option_id.0.to_string())
            .collect()
    }

    #[test]
    fn plain_commands_offer_always_allow_between_once_and_reject() {
        assert_eq!(
            option_ids(&run_command("cargo test --lib")),
            [ALLOW_ONCE, ALLOW_ALWAYS, REJECT_ONCE]
        );
        let write = crate::tools::ToolCall {
            name: "write_to_file".to_owned(),
            arguments: serde_json::json!({ "path": "a.txt", "content": "x" }),
            call_id: None,
        };
        assert_eq!(option_ids(&write), [ALLOW_ONCE, REJECT_ONCE]);
        let networked = crate::tools::ToolCall {
            arguments: serde_json::json!({ "command": "cargo test", "network_access": true }),
            ..run_command("")
        };
        assert_eq!(option_ids(&networked), [ALLOW_ONCE, REJECT_ONCE]);
    }

    #[test]
    fn selected_options_map_to_choices() {
        let select = |id: &str| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id.to_owned()))
        };
        assert_eq!(
            permission_choice(&select(ALLOW_ONCE)),
            PermissionChoice::AllowOnce
        );
        assert_eq!(
            permission_choice(&select(ALLOW_ALWAYS)),
            PermissionChoice::AllowAlways
        );
        assert_eq!(
            permission_choice(&select(REJECT_ONCE)),
            PermissionChoice::Reject
        );
        assert_eq!(
            permission_choice(&RequestPermissionOutcome::Cancelled),
            PermissionChoice::Reject
        );
    }

    #[test]
    fn always_allow_remembers_the_prefix_so_it_is_not_asked_again() {
        let mut state = crate::app::AppState::new();
        let call = run_command("cargo test --lib");
        assert!(!covered_by_saved_prefix(&state.config, &call));
        remember_approved_prefix(&mut state, &call);
        remember_approved_prefix(&mut state, &call);
        assert_eq!(
            state.config.approved_command_prefixes,
            ["prefix-v1:cargo test --lib"]
        );
        assert!(covered_by_saved_prefix(&state.config, &call));
        assert!(!covered_by_saved_prefix(
            &state.config,
            &run_command("rm -rf target")
        ));
    }

    /// #2255: the macOS app steers with ⌘⏎. An ACP turn is the regular
    /// interactive turn, so live steering must be admitted; otherwise the
    /// steerable marker stays unset and every steer is refused and queued.
    #[test]
    fn acp_turns_admit_live_steering() {
        assert!(acp_turn_supports_live_steering());
    }

    #[test]
    fn approval_wait_counts_as_user_wait() {
        let mut state = crate::app::AppState::new();
        state.begin_user_wait(std::time::Instant::now());
        record_approval_wait(&mut state, std::time::Duration::from_secs(180));
        record_approval_wait(&mut state, std::time::Duration::from_secs(5));
        assert_eq!(state.approval_wait, std::time::Duration::from_secs(185));
    }
}
