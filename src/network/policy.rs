use crate::app::{AppState, AppStatus, ToolConfirmation};
use crate::tools::{self, ToolCall};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) trait TurnPolicy: Send + Sync {
    fn should_approve(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send;
    fn should_approve_with_assessments(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[ToolCall],
        assessments: &crate::tools::ShellAssessmentCache,
    ) -> impl std::future::Future<Output = bool> + Send {
        let _ = assessments;
        self.should_approve(state, tool_calls)
    }
    fn should_verify_completion(&self) -> bool;

    /// Whether this policy represents a regular interactive turn that may
    /// accept live steering. Non-interactive callers must opt in explicitly.
    fn supports_live_turn_steering(&self) -> bool {
        false
    }

    fn is_headless(&self) -> bool {
        false
    }
}

pub(crate) struct InteractivePolicy;

fn authorization_for_interactive_call(
    call: &ToolCall,
    mode: crate::config::AgentMode,
    auto_confirm: bool,
    assessments: &crate::tools::ShellAssessmentCache,
) -> tools::AuthorizationDecision {
    crate::tools::shell_assessment_for_call(assessments, call)
        .map(|assessment| assessment.effective_authorization.clone())
        .unwrap_or_else(|| {
            tools::authorize_tool_with_args(&call.name, &call.arguments, mode, auto_confirm, false)
        })
}

impl InteractivePolicy {
    async fn approve(
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[ToolCall],
        assessments: &crate::tools::ShellAssessmentCache,
    ) -> bool {
        let mut confirmations = Vec::new();
        let (auto_confirm, task_working_directory) = {
            let state = state.lock().await;
            (
                state.auto_confirm,
                state
                    .task_working_directory
                    .clone()
                    .or_else(|| state.workspace_root.clone()),
            )
        };

        if !auto_confirm {
            for call in tool_calls {
                let mode = { state.lock().await.agent_mode };
                let decision = authorization_for_interactive_call(call, mode, false, assessments);
                if matches!(decision, tools::AuthorizationDecision::RequireConfirmation)
                    && !tools::is_agent_tool(&call.name)
                {
                    let path = if let Some(p) = call.arguments.get("path").and_then(|p| p.as_str())
                    {
                        p.to_string()
                    } else if let Some(cmd) = call.arguments.get("command").and_then(|c| c.as_str())
                    {
                        cmd.to_string()
                    } else if let (Some(src), Some(dest)) = (
                        call.arguments.get("src").and_then(|s| s.as_str()),
                        call.arguments.get("dest").and_then(|d| d.as_str()),
                    ) {
                        format!("{src} -> {dest}")
                    } else {
                        "?".to_string()
                    };

                    let diff_opt = crate::network::get_diff_preview(&call.name, &call.arguments);
                    let render_preview = (call.name == "render_video")
                        .then(|| {
                            tools::render_confirmation_preview(
                                &call.arguments,
                                task_working_directory.as_deref(),
                            )
                        })
                        .flatten();
                    let (preview, content_bytes) = if let Some(preview) = render_preview {
                        let content_bytes = preview.len();
                        (preview, content_bytes)
                    } else if call.name == "run_command" {
                        let command = call
                            .arguments
                            .get("command")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let preview = tools::command_confirmation_preview(command);
                        (preview, command.len())
                    } else if let Some(ref d) = diff_opt {
                        (d.clone(), d.len())
                    } else {
                        let content = call
                            .arguments
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        let preview = content.lines().take(6).collect::<Vec<_>>().join("\n");
                        (preview, content.len())
                    };

                    confirmations.push(ToolConfirmation {
                        tool_name: call.name.clone(),
                        path,
                        content_preview: preview,
                        content_bytes,
                    });
                }
            }
        }

        let mut approved = true;
        if !confirmations.is_empty() {
            let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
            {
                let mut s = state.lock().await;
                s.modal_scroll_row = 0;
                s.tool_confirmation_selected = 0;
                s.pending_tool_confirmation = Some(confirmations);
                s.tool_confirmation_response = Some(tx);
                s.status = AppStatus::AwaitingToolConfirmation;
                s.request_redraw();
            }

            let first_tool_name = &tool_calls[0].name;
            let _ = crate::notifications::notify_pending_confirmation(first_tool_name);

            crate::dbg_log!(
                "Awaiting user batch confirmation for {} tools",
                tool_calls.len()
            );
            approved = match rx.await {
                Ok(true) => {
                    crate::dbg_log!("User approved batch tool calls");
                    true
                }
                Ok(false) => {
                    crate::dbg_log!("User denied batch tool calls");
                    let _ = crate::notifications::notify_finished(
                        crate::notifications::FinishedStatus::Denied,
                    );
                    false
                }
                Err(_) => {
                    crate::dbg_log!("Confirmation channel closed during batch confirmation");
                    false
                }
            };
        }

        approved
    }

    fn verify_completion(&self) -> bool {
        true
    }
}

impl TurnPolicy for InteractivePolicy {
    fn should_approve(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[ToolCall],
    ) -> impl std::future::Future<Output = bool> + Send {
        let assessments = crate::tools::ShellAssessmentCache::default();
        async move { Self::approve(state, tool_calls, &assessments).await }
    }

    fn should_approve_with_assessments(
        &self,
        state: &Arc<Mutex<AppState>>,
        tool_calls: &[ToolCall],
        assessments: &crate::tools::ShellAssessmentCache,
    ) -> impl std::future::Future<Output = bool> + Send {
        Self::approve(state, tool_calls, assessments)
    }

    fn should_verify_completion(&self) -> bool {
        self.verify_completion()
    }

    fn supports_live_turn_steering(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{InteractivePolicy, TurnPolicy};
    use crate::tools::ToolCall;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct DefaultPolicy;

    impl TurnPolicy for DefaultPolicy {
        fn should_approve(
            &self,
            _state: &Arc<Mutex<crate::app::AppState>>,
            _tool_calls: &[ToolCall],
        ) -> impl std::future::Future<Output = bool> + Send {
            async { true }
        }

        fn should_verify_completion(&self) -> bool {
            false
        }
    }

    #[test]
    fn only_interactive_policy_opts_into_live_turn_steering() {
        assert!(InteractivePolicy.supports_live_turn_steering());
        assert!(!DefaultPolicy.supports_live_turn_steering());
    }
}
