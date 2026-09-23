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
        .map(|assessment| assessment.local_authorization.clone())
        .unwrap_or_else(|| {
            tools::authorize_tool_with_args(&call.name, &call.arguments, mode, auto_confirm, false)
        })
}

fn saved_prefix_covers_call(call: &ToolCall, prefixes: &[String]) -> bool {
    tools::approved_command_prefix_covers_call(&call.name, &call.arguments, prefixes)
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
        let approved_command_prefixes = state.lock().await.config.approved_command_prefixes.clone();
        let denied_command_prefixes = state.lock().await.config.denied_command_prefixes.clone();

        if !auto_confirm {
            for call in tool_calls {
                let mode = { state.lock().await.agent_mode };
                let decision = authorization_for_interactive_call(call, mode, false, assessments);
                let covered_by_prefix = saved_prefix_covers_call(call, &approved_command_prefixes);
                let blocked_by_prefix = tools::denied_command_prefix_covers_call(
                    &call.name,
                    &call.arguments,
                    &denied_command_prefixes,
                );
                if matches!(decision, tools::AuthorizationDecision::RequireConfirmation)
                    && !tools::is_agent_tool(&call.name)
                    && !covered_by_prefix
                    && !blocked_by_prefix
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
                        rememberable_prefix: (call.name == "run_command")
                            .then(|| tools::rememberable_command_prefix_for_call(&call.arguments))
                            .flatten(),
                        forbidden_prefix: (call.name == "run_command")
                            .then(|| {
                                tools::rememberable_command_forbid_prefix_for_call(&call.arguments)
                            })
                            .flatten(),
                    });
                }
            }
        }

        let mut approved = true;
        if !confirmations.is_empty() {
            let (tx, rx) = tokio::sync::oneshot::channel::<crate::app::ToolConfirmationResponse>();
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
                Ok(crate::app::ToolConfirmationResponse::Approve) => {
                    crate::dbg_log!("User approved batch tool calls");
                    true
                }
                Ok(crate::app::ToolConfirmationResponse::ApproveAndRemember(prefix)) => {
                    let mut state = state.lock().await;
                    if tool_calls.len() == 1
                        && tool_calls[0].name == "run_command"
                        && tools::rememberable_command_prefix_for_call(&tool_calls[0].arguments)
                            .as_deref()
                            == Some(prefix.as_str())
                        && !state.config.approved_command_prefixes.contains(&prefix)
                    {
                        state.config.approved_command_prefixes.push(prefix);
                        crate::config::save_entire_config(&state.config);
                    }
                    true
                }
                Ok(crate::app::ToolConfirmationResponse::ForbidAndRemember(prefix)) => {
                    let mut state = state.lock().await;
                    if tool_calls.len() == 1
                        && tool_calls[0].name == "run_command"
                        && tools::rememberable_command_forbid_prefix_for_call(
                            &tool_calls[0].arguments,
                        )
                        .as_deref()
                            == Some(prefix.as_str())
                        && !state.config.denied_command_prefixes.contains(&prefix)
                    {
                        state.config.denied_command_prefixes.push(prefix);
                        crate::config::save_entire_config(&state.config);
                    }
                    true
                }
                Ok(crate::app::ToolConfirmationResponse::Deny) => {
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
    use super::{InteractivePolicy, TurnPolicy, saved_prefix_covers_call};
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

    #[test]
    fn saved_prefix_applies_only_to_plain_run_command_calls() {
        let prefixes = vec!["cargo test".to_string()];
        let call = |command: &str, extra: serde_json::Value| ToolCall {
            name: "run_command".to_string(),
            arguments: serde_json::json!({"command": command})
                .as_object()
                .map(|args| {
                    let mut args = args.clone();
                    args.extend(extra.as_object().cloned().unwrap_or_default());
                    serde_json::Value::Object(args)
                })
                .unwrap(),
            call_id: None,
        };
        assert!(saved_prefix_covers_call(
            &call("cargo test --lib", serde_json::json!({})),
            &prefixes
        ));
        assert!(!saved_prefix_covers_call(
            &call("cargo testing", serde_json::json!({})),
            &prefixes
        ));
        assert!(!saved_prefix_covers_call(
            &call("cargo test", serde_json::json!({"background": true})),
            &prefixes
        ));
        assert!(!saved_prefix_covers_call(
            &call(
                "cargo test",
                serde_json::json!({"env": {"RUSTFLAGS": "-C opt-level=3"}})
            ),
            &prefixes
        ));
    }

    #[tokio::test]
    async fn explicit_remember_choice_persists_the_approved_prefix() {
        let state = Arc::new(Mutex::new(crate::app::AppState::new()));
        let policy_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let calls = [ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({"command": "cargo test --lib"}),
                call_id: None,
            }];
            InteractivePolicy
                .should_approve(&policy_state, &calls)
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if state.lock().await.status == crate::app::AppStatus::AwaitingToolConfirmation {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the command should await interactive confirmation");

        let response = state
            .lock()
            .await
            .tool_confirmation_response
            .take()
            .expect("confirmation response channel");
        response
            .send(crate::app::ToolConfirmationResponse::ApproveAndRemember(
                "cargo test --lib".to_owned(),
            ))
            .expect("policy task should be waiting");

        assert!(task.await.expect("policy task should finish"));
        assert_eq!(
            state.lock().await.config.approved_command_prefixes,
            ["cargo test --lib"]
        );
    }

    #[tokio::test]
    async fn explicit_forbid_choice_persists_a_denied_prefix() {
        let state = Arc::new(Mutex::new(crate::app::AppState::new()));
        let policy_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let calls = [ToolCall {
                name: "run_command".to_string(),
                arguments: serde_json::json!({"command": "make test"}),
                call_id: None,
            }];
            InteractivePolicy
                .should_approve(&policy_state, &calls)
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if state.lock().await.status == crate::app::AppStatus::AwaitingToolConfirmation {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the command should await interactive confirmation");

        let response = state
            .lock()
            .await
            .tool_confirmation_response
            .take()
            .expect("confirmation response channel");
        response
            .send(crate::app::ToolConfirmationResponse::ForbidAndRemember(
                "make test".to_owned(),
            ))
            .expect("policy task should be waiting");

        assert!(task.await.expect("policy task should finish"));
        assert_eq!(
            state.lock().await.config.denied_command_prefixes,
            ["make test"]
        );
    }
}
