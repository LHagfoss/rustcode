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
    fn should_verify_completion(&self) -> bool;
}

pub(crate) struct InteractivePolicy;

impl TurnPolicy for InteractivePolicy {
    async fn should_approve(&self, state: &Arc<Mutex<AppState>>, tool_calls: &[ToolCall]) -> bool {
        let mut confirmations = Vec::new();
        let auto_confirm = { state.lock().await.auto_confirm };

        if !auto_confirm {
            for call in tool_calls {
                let mode = { state.lock().await.agent_mode };
                let decision = tools::authorize_tool_with_args(
                    &call.name,
                    &call.arguments,
                    mode,
                    false,
                    false,
                );
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
                    let (preview, content_bytes) = if call.name == "run_command" {
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

    fn should_verify_completion(&self) -> bool {
        true
    }
}
