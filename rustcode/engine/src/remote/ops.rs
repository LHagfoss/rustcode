//! Remote mutations applied by the session owner.
//!
//! The owner is whichever runtime holds the session's `AppState` and cancel
//! token (the terminal in v1). It calls [`apply_session_mutation`] from the
//! same loop that serves its own input, so remote and local mutations are
//! serialized through one place. Every identity a request names is checked
//! here, under the state lock that performs the mutation: a check made when
//! the gateway received the frame proves nothing by the time it is applied.

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::app::{AppState, DraftSubmitMode};
use crate::controller::{
    ApprovalChoice, ApprovalDecision, QuestionRejection, QuestionReply,
    apply_approval_decision_for_batch, apply_question_answer_for_question, cancel_turn_for_turn,
};

use super::protocol::{
    PromptDisposition, REMOTE_PROTOCOL_VERSION, RemoteAnswer, RemoteError, RemoteErrorCode,
    RemoteOperation, RemoteRequest, RemoteResult,
};

/// The live sharing registration an owner holds for its session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRegistration {
    pub session_id: String,
    pub registration_epoch: u64,
}

/// What the owner still has to do after a mutation was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerFollowUp {
    None,
    /// A prompt was queued on an idle session: start the orchestrator the
    /// way a local submission would.
    StartTurn,
    /// Cancellation was signalled: run the normal cancel cleanup.
    FinishCancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMutation {
    pub result: RemoteResult,
    pub follow_up: OwnerFollowUp,
}

impl SessionMutation {
    fn done(result: RemoteResult) -> Self {
        Self {
            result,
            follow_up: OwnerFollowUp::None,
        }
    }
}

fn error(code: RemoteErrorCode, message: &str) -> RemoteError {
    RemoteError::new(code, message)
}

/// Apply one mutating request to the owner's session, or say why not.
/// `Err` means nothing changed.
pub async fn apply_session_mutation(
    state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    registration: &SessionRegistration,
    request: &RemoteRequest,
) -> Result<SessionMutation, RemoteError> {
    if request.protocol_version != REMOTE_PROTOCOL_VERSION {
        return Err(RemoteError::incompatible_version(u64::from(
            request.protocol_version,
        )));
    }
    if !request.operation.is_mutation() {
        return Err(error(
            RemoteErrorCode::UnsupportedOperation,
            "this operation is not a session mutation",
        ));
    }
    if request.session_id.as_deref() != Some(registration.session_id.as_str())
        || request.registration_epoch != Some(registration.registration_epoch)
    {
        return Err(stale_session());
    }
    match &request.operation {
        #[cfg(unix)]
        RemoteOperation::UploadImage { .. } => {
            let state = state.lock().await;
            if state.active_session_id != registration.session_id {
                return Err(stale_session());
            }
            ensure_image_support(&state)?;
            let device = request.authenticated_device_id.clone().ok_or_else(|| {
                error(
                    RemoteErrorCode::InvalidRequest,
                    "image uploads require an authenticated device",
                )
            })?;
            let root = super::images::root(&registration.session_id).map_err(image_error)?;
            let operation = request.operation.clone();
            let result = tokio::task::spawn_blocking(move || {
                super::images::upload(&root, &device, &operation)
            })
            .await
            .map_err(|e| image_error(e.into()))?
            .map_err(image_error)?;
            Ok(SessionMutation::done(result))
        }

        RemoteOperation::SetSessionSettings {
            model,
            reasoning_effort,
        } => {
            let mut state = state.lock().await;
            if state.active_session_id != registration.session_id {
                return Err(stale_session());
            }
            set_settings(&mut state, model, reasoning_effort).map(|settings| {
                SessionMutation::done(RemoteResult::SessionSettingsUpdated { settings })
            })
        }
        RemoteOperation::CancelQuestion { question_id } => {
            ensure_session(state, registration).await?;
            crate::controller::cancel_question_for_question(state, cancel_token, question_id)
                .await
                .map_err(|_| {
                    error(
                        RemoteErrorCode::StaleQuestion,
                        "the named question is no longer pending",
                    )
                })?;
            Ok(SessionMutation::done(RemoteResult::QuestionCancelled {
                question_id: question_id.clone(),
            }))
        }
        RemoteOperation::ExecuteCommand { command } => {
            let mut state = state.lock().await;
            if state.active_session_id != registration.session_id {
                return Err(stale_session());
            }
            execute_command(&mut state, command).map(SessionMutation::done)
        }

        RemoteOperation::SubmitPrompt { prompt } => {
            accept_prompt(
                state,
                registration,
                prompt,
                request.authenticated_device_id.as_deref(),
                PromptDisposition::Started,
            )
            .await
        }
        RemoteOperation::Queue { prompt } => {
            accept_prompt(
                state,
                registration,
                prompt,
                request.authenticated_device_id.as_deref(),
                PromptDisposition::Queued,
            )
            .await
        }
        RemoteOperation::Steer { prompt } => {
            accept_prompt(
                state,
                registration,
                prompt,
                request.authenticated_device_id.as_deref(),
                PromptDisposition::Steered,
            )
            .await
        }
        RemoteOperation::CancelTurn { turn_id } => {
            ensure_session(state, registration).await?;
            if cancel_turn_for_turn(state, cancel_token, turn_id).await {
                Ok(SessionMutation {
                    result: RemoteResult::TurnCancelled {
                        turn_id: turn_id.clone(),
                    },
                    follow_up: OwnerFollowUp::FinishCancel,
                })
            } else {
                Err(error(
                    RemoteErrorCode::StaleTurn,
                    "the named turn is no longer running",
                ))
            }
        }
        RemoteOperation::AnswerQuestion {
            question_id,
            answer,
        } => {
            ensure_session(state, registration).await?;
            let reply = match answer {
                RemoteAnswer::Selected { options } => QuestionReply::Options(options.clone()),
                RemoteAnswer::Custom { text } => QuestionReply::Custom(text.clone()),
            };
            match apply_question_answer_for_question(state, question_id, reply).await {
                Ok(()) => Ok(SessionMutation::done(RemoteResult::QuestionAnswered {
                    question_id: question_id.clone(),
                })),
                Err(QuestionRejection::Stale) => Err(error(
                    RemoteErrorCode::StaleQuestion,
                    "the named question is no longer pending",
                )),
                Err(QuestionRejection::Invalid(detail)) => {
                    Err(RemoteError::new(RemoteErrorCode::InvalidAnswer, detail))
                }
            }
        }
        RemoteOperation::ResolveApproval { batch_id, choice } => {
            ensure_session(state, registration).await?;
            let decision = match choice {
                ApprovalChoice::Approve => ApprovalDecision::Approve,
                ApprovalChoice::Deny => ApprovalDecision::Deny,
            };
            if apply_approval_decision_for_batch(state, cancel_token, batch_id, decision).await {
                Ok(SessionMutation::done(RemoteResult::ApprovalResolved {
                    batch_id: batch_id.clone(),
                    choice: *choice,
                }))
            } else {
                Err(error(
                    RemoteErrorCode::StaleApproval,
                    "the named approval batch is no longer pending",
                ))
            }
        }
        _ => unreachable!("non-mutations were rejected above"),
    }
}

fn set_settings(
    state: &mut AppState,
    model: &str,
    reasoning_effort: &str,
) -> Result<super::protocol::RemoteSessionSettings, RemoteError> {
    let mut settings = super::projection::project_settings(state);
    if !settings.can_change {
        return Err(error(
            RemoteErrorCode::Busy,
            "session settings can only change while idle",
        ));
    }
    let index = state
        .config
        .models
        .iter()
        .position(|profile| profile.name == model)
        .ok_or_else(|| error(RemoteErrorCode::InvalidRequest, "unknown model profile"))?;
    if !super::projection::reasoning_efforts(&state.config.models[index])
        .iter()
        .any(|effort| effort == reasoning_effort)
    {
        return Err(error(
            RemoteErrorCode::InvalidRequest,
            "unsupported reasoning effort",
        ));
    }
    settings.selected_model = model.to_owned();
    settings.reasoning_effort = reasoning_effort.to_owned();
    let settings = super::projection::bound_settings(
        settings,
        super::projection::ProjectionLimits::default().frame_bytes / 4,
    )
    .ok_or_else(|| {
        error(
            RemoteErrorCode::FrameTooLarge,
            "the model catalog is too large for remote controls",
        )
    })?;
    // Retain the explicitly configured legacy choice when clearing it.
    let known = super::projection::reasoning_efforts(&state.config.models[index]);
    let profile = &mut state.config.models[index];
    if profile.supports_reasoning_effort.is_none() && known.len() > 1 {
        profile.supports_reasoning_effort = Some(true);
    }
    if profile.reasoning_efforts.is_none() {
        profile.reasoning_efforts = Some(
            known
                .into_iter()
                .filter(|effort| effort != "default")
                .collect(),
        );
    }
    profile.reasoning_effort = (reasoning_effort != "default").then(|| reasoning_effort.to_owned());
    let (model_name, url) = (profile.model.to_owned(), profile.url.clone());
    state.model_name = model_name;
    state.api_base_url = url;
    state.config.default.set_big(model.to_owned());
    state.request_redraw();
    crate::config::record_session_settings_for_profile(
        &state.active_session_id,
        &state.config,
        model,
    );
    Ok(settings)
}

/// One catalog shared with terminal autocomplete. Native app actions replace
/// terminal pickers; commands requiring a host UI remain visible and labelled.
pub(super) fn command_catalog() -> Vec<super::protocol::RemoteCommandInfo> {
    let mut commands: Vec<_> = crate::app::suggestion::COMMANDS
        .iter()
        .map(|command| {
            let action = match command.name {
                "/new" => "new_session",
                "/model" | "/models" => "model_picker",
                "/history" | "/resume" | "/exit" | "/quit" => "session_picker",
                "/copy" => "copy_response",
                "/help" | "/status" | "/stats" | "/usage" | "/info" | "/about" | "/perf"
                | "/context" | "/tasks" | "/ps" | "/mcp" | "/effort" | "/change_title"
                | "/cancel" | "/pwd" | "/skills" | "/tools" | "/archive" | "/memory"
                | "/prompts" | "/stop" | "/thinking" | "/yolo" | "/verbosity" | "/pi"
                | "/delegate" | "/session" => "execute",
                _ => "terminal",
            };
            let arguments = matches!(
                command.name,
                "/model"
                    | "/effort"
                    | "/change_title"
                    | "/memory"
                    | "/thinking"
                    | "/yolo"
                    | "/verbosity"
                    | "/delegate"
            );
            super::protocol::RemoteCommandInfo {
                name: command.name.into(),
                description: if action == "terminal" {
                    format!("{} · On Mac", command.desc)
                } else {
                    command.desc.into()
                },
                insertion: format!("{}{}", command.name, if arguments { " " } else { "" }),
                action: action.into(),
            }
        })
        .collect();
    commands.push(super::protocol::RemoteCommandInfo {
        name: "/title".into(),
        description: "Rename this session".into(),
        insertion: "/title ".into(),
        action: "execute".into(),
    });
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands
}

fn remote_command_help() -> String {
    command_catalog()
        .iter()
        .map(|command| format!("- `{}` — {}", command.name, command.description))
        .collect::<Vec<_>>()
        .join("\n")
}

fn require_idle(state: &AppState) -> Result<(), RemoteError> {
    if !super::projection::project_settings(state).can_change {
        Err(error(
            RemoteErrorCode::Busy,
            "this setting can only change while idle",
        ))
    } else {
        Ok(())
    }
}

fn switch_value(arguments: &str, current: bool) -> Result<bool, RemoteError> {
    match arguments {
        "on" => Ok(true),
        "off" => Ok(false),
        "toggle" => Ok(!current),
        _ => Err(error(
            RemoteErrorCode::InvalidRequest,
            "Use on, off, or toggle",
        )),
    }
}

/// Explicitly allowed owner commands. Their output is presentation-only and
/// never enters provider history or the terminal's draft/panel state.
fn execute_command(state: &mut AppState, input: &str) -> Result<RemoteResult, RemoteError> {
    let input = input.trim();
    let (name, arguments) = input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(name, args)| (name, args.trim()));
    let command = name.to_ascii_lowercase();
    let settings = super::projection::project_settings(state);
    let (title, output) = match command.as_str() {
        "/help" if arguments.is_empty() => ("Remote commands", remote_command_help()),
        "/status" | "/stats" | "/session" if arguments.is_empty() => {
            ("Status", crate::controller::native_status_report(state))
        }
        "/usage" if arguments.is_empty() => {
            ("Usage", crate::controller::native_usage_report(state))
        }
        "/info" | "/about" if arguments.is_empty() => {
            ("About RustCode", crate::app::actions::build_info_text())
        }
        "/perf" if arguments.is_empty() => (
            "Performance",
            state
                .last_turn_performance
                .as_ref()
                .map(|perf| perf.report())
                .unwrap_or_else(|| "No turn telemetry available yet.".into()),
        ),
        "/context" if arguments.is_empty() => (
            "Context",
            state
                .active_model_profile()
                .map(|profile| {
                    format!(
                        "Model: {}\nContext window: {} tokens",
                        profile.name,
                        profile.context_budget().context_window
                    )
                })
                .unwrap_or_else(|| "No active model profile.".into()),
        ),
        "/tasks" | "/ps" if arguments.is_empty() => {
            let tasks = crate::tools::background_task_snapshots(&state.active_session_id);
            (
                "Tasks",
                if tasks.is_empty() {
                    "No tasks are running.".into()
                } else {
                    tasks
                        .iter()
                        .map(|task| format!("- {}: {}", task.id, task.command))
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            )
        }
        "/mcp" if arguments.is_empty() => (
            "MCP servers",
            if state.config.mcp_servers.is_empty() {
                "No MCP servers configured.".into()
            } else {
                state
                    .config
                    .mcp_servers
                    .iter()
                    .map(|server| format!("- {}", server.name))
                    .collect::<Vec<_>>()
                    .join("\n")
            },
        ),
        "/pwd" if arguments.is_empty() => (
            "Workspace",
            state
                .effective_workspace_root()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "No workspace selected.".into()),
        ),
        "/skills" if arguments.is_empty() => (
            "Skills",
            crate::skills::format_skill_catalog(&crate::skills::discover_skills_for_catalog()),
        ),
        "/tools" if arguments.is_empty() => (
            "Tools",
            crate::tools::TOOLS
                .iter()
                .map(|t| format!("- {} — {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        "/archive" if arguments.is_empty() => {
            crate::app::session_controller::SessionController::default()
                .archive(state)
                .map_err(|e| RemoteError::new(RemoteErrorCode::InvalidRequest, e.to_string()))?;
            ("Session", "Session saved.".into())
        }
        "/memory" => {
            let root = state.effective_workspace_root();
            let args: Vec<_> = arguments.split_whitespace().collect();
            (
                "Project memory",
                crate::memory::command(root.as_deref(), &args)
                    .unwrap_or_else(|| "Use /memory show, path, add, forget, or reset.".into()),
            )
        }
        "/prompts" if arguments.is_empty() => {
            let roots = crate::prompt_commands::Roots {
                workspace: state.effective_workspace_root(),
                config_dir: crate::config::get_config_dir(),
            };
            let entries = crate::prompt_commands::list(&roots)
                .map_err(|e| RemoteError::new(RemoteErrorCode::InvalidRequest, e.to_string()))?;
            (
                "Prompt templates",
                if entries.is_empty() {
                    "No prompt templates found.".into()
                } else {
                    entries
                        .iter()
                        .map(|entry| format!("- {}", entry.path.display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                },
            )
        }
        "/stop" if arguments.is_empty() => {
            let result = crate::tools::stop_background_tasks(&state.active_session_id);
            (
                "Tasks",
                format!(
                    "Stopped: {}. Stop requested: {}. Failed: {}.",
                    result.stopped, result.requested, result.failed
                ),
            )
        }
        "/yolo" | "/pi" => {
            let current = if command == "/yolo" {
                state.auto_confirm
            } else {
                state.config.prompt_improver
            };
            let enabled = if arguments.is_empty() {
                current
            } else {
                require_idle(state)?;
                switch_value(arguments, current)?
            };
            if command == "/yolo" {
                state.auto_confirm = enabled;
            } else {
                state.config.prompt_improver = enabled;
            }
            (
                "Session setting",
                format!("{}: {}", command, if enabled { "on" } else { "off" }),
            )
        }
        "/verbosity" => {
            use crate::app::state::Verbosity;
            if !arguments.is_empty() {
                require_idle(state)?;
                state.verbosity = match arguments {
                    "low" => Verbosity::Low,
                    "high" => Verbosity::High,
                    "toggle" => match state.verbosity {
                        Verbosity::Low => Verbosity::High,
                        Verbosity::High => Verbosity::Low,
                    },
                    _ => {
                        return Err(error(
                            RemoteErrorCode::InvalidRequest,
                            "Use low, high, or toggle",
                        ));
                    }
                };
                state.config.verbosity = state.verbosity.clone();
            }
            (
                "Verbosity",
                match state.verbosity {
                    Verbosity::Low => "low",
                    Verbosity::High => "high",
                }
                .into(),
            )
        }
        "/thinking" => {
            let selected = settings.selected_model;
            if !arguments.is_empty() {
                require_idle(state)?;
            }
            let profile = state
                .config
                .models
                .iter_mut()
                .find(|p| p.name == selected)
                .ok_or_else(|| error(RemoteErrorCode::InvalidRequest, "No active model profile"))?;
            if !arguments.is_empty() {
                profile.enable_thinking = match arguments {
                    "on" => Some(true),
                    "off" => Some(false),
                    "default" => None,
                    _ => {
                        return Err(error(
                            RemoteErrorCode::InvalidRequest,
                            "Use on, off, or default",
                        ));
                    }
                };
            }
            (
                "Thinking",
                match profile.enable_thinking {
                    Some(true) => "on",
                    Some(false) => "off",
                    None => "default",
                }
                .into(),
            )
        }
        "/delegate" => {
            require_idle(state)?;
            match arguments {
                "" | "on" if !state.config.delegation_enabled => {
                    return Err(error(
                        RemoteErrorCode::InvalidRequest,
                        "Subagents are disabled by host configuration",
                    ));
                }
                "" => {
                    state.delegation_armed = true;
                    state.delegation_sticky = false;
                }
                "on" => {
                    state.delegation_sticky = true;
                    state.delegation_armed = false;
                }
                "off" => {
                    state.delegation_sticky = false;
                    state.delegation_armed = false;
                    state.delegation_active = false;
                }
                _ => {
                    return Err(error(
                        RemoteErrorCode::InvalidRequest,
                        "Use /delegate [on|off]",
                    ));
                }
            }
            (
                "Delegation",
                if arguments.is_empty() {
                    "Enabled for the next task."
                } else if arguments == "on" {
                    "Enabled for this session."
                } else {
                    "Disabled."
                }
                .into(),
            )
        }
        "/model" if arguments.is_empty() => (
            "Model",
            format!(
                "Selected: {}\n\n{}",
                settings.selected_model,
                settings
                    .models
                    .iter()
                    .map(|model| format!("- {}", model.id))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ),
        "/effort" if arguments.is_empty() => (
            "Reasoning effort",
            format!(
                "Selected: {}\nAvailable: {}",
                settings.reasoning_effort,
                state
                    .active_model_profile()
                    .map(|profile| super::projection::reasoning_efforts(&profile).join(", "))
                    .unwrap_or_else(|| "default".into())
            ),
        ),
        "/model" => {
            let profile = state
                .config
                .models
                .iter()
                .find(|profile| profile.name == arguments)
                .ok_or_else(|| {
                    error(
                        RemoteErrorCode::InvalidRequest,
                        "unknown model profile; use /model to list exact profile names",
                    )
                })?;
            let effort = profile
                .reasoning_effort
                .clone()
                .unwrap_or_else(|| "default".into());
            set_settings(state, arguments, &effort)?;
            ("Model", format!("Selected: {arguments}"))
        }
        "/effort" => {
            let effort = match arguments {
                "off" | "none" => "default",
                "med" => "medium",
                value => value,
            };
            set_settings(state, &settings.selected_model, effort)?;
            ("Reasoning effort", format!("Selected: {arguments}"))
        }
        "/title" | "/change_title" => {
            if arguments.is_empty()
                || arguments.len() > 512
                || arguments.chars().any(char::is_control)
            {
                return Err(error(
                    RemoteErrorCode::InvalidRequest,
                    "Usage: /title <title> (one line, up to 512 bytes)",
                ));
            }
            crate::config::save_session_title(&state.active_session_id, arguments);
            state.invalidate_session_title_cache();
            state.request_redraw();
            ("Session title", format!("Renamed to {arguments}"))
        }
        _ => {
            return Err(error(
                RemoteErrorCode::UnsupportedOperation,
                "this command or its arguments are not supported remotely; use /help for available commands",
            ));
        }
    };
    // Shared reports use plain line breaks; preserve their rows in Markdown.
    let output = if matches!(
        command.as_str(),
        "/status" | "/usage" | "/perf" | "/context" | "/effort"
    ) {
        output.replace('\n', "  \n")
    } else {
        output
    };
    // A result must fit the transport and receipt cache. Informational reports
    // may grow with configured catalogs, task output, or provider metadata.
    let output = if output.len() > 32 * 1024 {
        let mut end = 32 * 1024;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}\n… output truncated", &output[..end])
    } else {
        output
    };
    Ok(RemoteResult::CommandExecuted {
        command,
        title: title.into(),
        output,
    })
}

/// Answer a read that only the owner can serve (`get_history`,
/// `get_content`) from the state it holds. Pure: the owner calls it under its
/// state lock and replies after releasing it.
pub fn read_session(
    state: &AppState,
    registration: &SessionRegistration,
    request: &RemoteRequest,
    limits: &super::projection::ProjectionLimits,
) -> Result<RemoteResult, RemoteError> {
    if request.protocol_version != REMOTE_PROTOCOL_VERSION {
        return Err(RemoteError::incompatible_version(u64::from(
            request.protocol_version,
        )));
    }
    if request.session_id.as_deref() != Some(registration.session_id.as_str())
        || request.registration_epoch != Some(registration.registration_epoch)
        || state.active_session_id != registration.session_id
    {
        return Err(stale_session());
    }
    match &request.operation {
        RemoteOperation::GetHistory { cursor, limit } => {
            super::projection::project_history_page(state, limits, cursor.as_deref(), *limit)
                .map(RemoteResult::History)
        }
        RemoteOperation::GetContent {
            content_id,
            offset,
            max_bytes,
        } => {
            let content = super::projection::resolve_content(state, content_id)
                .ok_or_else(|| error(RemoteErrorCode::NotFound, "no such content"))?;
            super::projection::content_chunk(content_id, &content, *offset, *max_bytes, limits)
                .map(RemoteResult::Content)
        }
        _ => Err(error(
            RemoteErrorCode::UnsupportedOperation,
            "the session owner does not answer this operation",
        )),
    }
}

fn stale_session() -> RemoteError {
    error(
        RemoteErrorCode::StaleSession,
        "the session is no longer shared under this registration",
    )
}

/// The registration is only valid while the owner still shows its session.
/// Turn, question and batch IDs are unique per process, so the identity check
/// that follows cannot match another session's state even if it changes
/// between this check and the mutation.
async fn ensure_session(
    state: &Arc<Mutex<AppState>>,
    registration: &SessionRegistration,
) -> Result<(), RemoteError> {
    if state.lock().await.active_session_id == registration.session_id {
        Ok(())
    } else {
        Err(stale_session())
    }
}

/// Queue or steer a remote prompt. The session check, the running-turn check
/// and the enqueue share one lock, and the terminal's draft is not touched.
#[cfg(unix)]
fn image_error(error: anyhow::Error) -> RemoteError {
    RemoteError::new(
        RemoteErrorCode::InvalidRequest,
        format!("Image attachment: {error:#}"),
    )
}

#[cfg(unix)]
fn ensure_image_support(state: &AppState) -> Result<(), RemoteError> {
    if state
        .active_model_profile()
        .is_some_and(|profile| profile.image_input_supported() == Some(true))
    {
        return Ok(());
    }
    if let Some(fallback) = state.vision_model_profile() {
        if fallback.image_input_supported() == Some(false) {
            return Err(error(
                RemoteErrorCode::UnsupportedOperation,
                "The configured vision_model does not support images; choose a vision-capable profile on the Mac",
            ));
        }
        if fallback
            .credential
            .as_ref()
            .is_some_and(crate::provider_auth::CredentialRef::is_chatgpt)
        {
            return Err(error(
                RemoteErrorCode::UnsupportedOperation,
                "Use the vision-capable ChatGPT model as the main model, or configure an API-key vision_model on the Mac",
            ));
        }
        return Ok(());
    }
    Err(error(
        RemoteErrorCode::UnsupportedOperation,
        "Select a vision-capable model or configure vision_model on the Mac before attaching images",
    ))
}

async fn accept_prompt(
    state: &Arc<Mutex<AppState>>,
    registration: &SessionRegistration,
    prompt: &str,
    device: Option<&str>,
    disposition: PromptDisposition,
) -> Result<SessionMutation, RemoteError> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(error(
            RemoteErrorCode::InvalidRequest,
            "the prompt is empty",
        ));
    }
    // Remote prompt text is never dispatched as a command: `/exit`, `/new` or an
    // approval toggle must not be reachable from a phone. v1 refuses the
    // prompt outright rather than sending the model a literal slash line the
    // user meant as a command.
    if prompt.starts_with('/') {
        return Err(error(
            RemoteErrorCode::UnsupportedOperation,
            "use execute_command for supported remote slash commands",
        ));
    }
    let mut state = state.lock().await;
    if state.active_session_id != registration.session_id {
        return Err(stale_session());
    }
    #[cfg(unix)]
    if rustcode_core::paste::expand(prompt).contains("![image](file://") {
        ensure_image_support(&state)?;
        let root = super::images::root(&registration.session_id).map_err(image_error)?;
        super::images::validate_prompt(&root, device, prompt).map_err(image_error)?;
    }
    #[cfg(not(unix))]
    let _ = device;
    let running = state.has_active_turn() || !state.pending_queue.is_empty();
    let mode = match disposition {
        PromptDisposition::Started if running => {
            return Err(error(
                RemoteErrorCode::Busy,
                "the session is running a turn; steer or queue instead",
            ));
        }
        PromptDisposition::Queued | PromptDisposition::Steered if !running => {
            return Err(error(
                RemoteErrorCode::NotRunning,
                "no turn is running; submit a prompt instead",
            ));
        }
        PromptDisposition::Steered if !state.can_accept_steer() => {
            return Err(error(
                RemoteErrorCode::UnsupportedOperation,
                "the running turn does not accept steering",
            ));
        }
        PromptDisposition::Steered => DraftSubmitMode::Steer,
        PromptDisposition::Started | PromptDisposition::Queued => DraftSubmitMode::Queue,
    };
    if crate::app::submit_detached_prompt(&mut state, prompt, mode)
        == crate::app::SubmitOutcome::Empty
    {
        return Err(error(
            RemoteErrorCode::UnsupportedOperation,
            "the running turn does not accept steering",
        ));
    }
    Ok(SessionMutation {
        result: RemoteResult::PromptAccepted { disposition },
        follow_up: if disposition == PromptDisposition::Started {
            OwnerFollowUp::StartTurn
        } else {
            OwnerFollowUp::None
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppStatus, PendingQuestion, ToolConfirmation, ToolConfirmationResponse};

    const SESSION: &str = "remote-ops-session";

    #[cfg(unix)]
    #[test]
    fn image_support_requires_a_vision_model_or_valid_fallback() {
        let mut state = shared_state();
        state.config.models.clear();
        state.config.vision_model = None;
        assert_eq!(
            ensure_image_support(&state).unwrap_err().code,
            RemoteErrorCode::UnsupportedOperation
        );
        state.config.models = vec![crate::config::ModelProfile {
            name: "vision".into(),
            model: "vision".into(),
            supports_vision: Some(true),
            ..Default::default()
        }];
        state.config.vision_model = Some("vision".into());
        ensure_image_support(&state).unwrap();
        state.config.models[0].supports_vision = Some(false);
        assert!(ensure_image_support(&state).is_err());
        state.config.models[0].supports_vision = Some(true);
        state.model_name = "vision".into();
        state.config.vision_model = None;
        ensure_image_support(&state).unwrap();
    }

    #[test]
    fn catalog_includes_every_terminal_command_and_labels_native_actions() {
        let catalog = command_catalog();
        for command in crate::app::suggestion::COMMANDS {
            assert!(catalog.iter().any(|remote| remote.name == command.name));
        }
        assert_eq!(
            catalog.iter().find(|c| c.name == "/new").unwrap().action,
            "new_session"
        );
        assert_eq!(
            catalog.iter().find(|c| c.name == "/login").unwrap().action,
            "terminal"
        );
        assert!(catalog.iter().all(|c| c.insertion.starts_with(&c.name)));
        let unique: std::collections::HashSet<_> = catalog.iter().map(|c| &c.name).collect();
        assert_eq!(unique.len(), catalog.len());
    }

    #[test]
    fn session_commands_preserve_local_draft_and_guard_busy_mutations() {
        let mut state = shared_state();
        state.input_buffer = "unfinished terminal draft".into();
        let history = state.history.clone();
        state.config.delegation_enabled = true;
        execute_command(&mut state, "/delegate on").unwrap();
        assert!(state.delegation_sticky);
        execute_command(&mut state, "/delegate off").unwrap();
        assert!(!state.delegation_sticky);
        assert!(
            state.config.delegation_enabled,
            "session toggles preserve host policy"
        );
        execute_command(&mut state, "/yolo off").unwrap();
        execute_command(&mut state, "/verbosity low").unwrap();
        state.begin_turn_identity();
        for command in ["/yolo on", "/pi on", "/verbosity high", "/delegate on"] {
            assert_eq!(
                execute_command(&mut state, command).unwrap_err().code,
                RemoteErrorCode::Busy
            );
        }
        for command in ["/pwd", "/ps", "/session", "/stats", "/tools"] {
            assert!(matches!(
                execute_command(&mut state, command),
                Ok(RemoteResult::CommandExecuted { .. })
            ));
        }
        assert_eq!(state.input_buffer, "unfinished terminal draft");
        assert_eq!(state.history.len(), history.len());
        assert!(!state.auto_confirm);
    }

    fn registration() -> SessionRegistration {
        SessionRegistration {
            session_id: SESSION.to_owned(),
            registration_epoch: 7,
        }
    }

    fn shared_state() -> AppState {
        let mut state = AppState::new();
        state.active_session_id = SESSION.to_owned();
        state
    }

    fn request(operation: RemoteOperation) -> RemoteRequest {
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: "r-1".to_owned(),
            session_id: Some(SESSION.to_owned()),
            registration_epoch: Some(7),
            operation,
        }
    }

    async fn apply(
        state: &Arc<Mutex<AppState>>,
        cancel_token: &mut CancellationToken,
        operation: RemoteOperation,
    ) -> Result<SessionMutation, RemoteError> {
        apply_session_mutation(state, cancel_token, &registration(), &request(operation)).await
    }

    fn code(result: Result<SessionMutation, RemoteError>) -> RemoteErrorCode {
        result.expect_err("the mutation must be rejected").code
    }

    fn confirmation() -> ToolConfirmation {
        ToolConfirmation {
            request_id: Some("call-1".to_owned()),
            tool_name: "run_command".to_owned(),
            path: "cargo test".to_owned(),
            content_preview: String::new(),
            content_bytes: 0,
            rememberable_prefix: None,
            forbidden_prefix: None,
        }
    }

    #[tokio::test]
    async fn remote_command_validation_preserves_busy_state_and_draft() {
        let mut state = shared_state();
        state.input_buffer = "my terminal draft".into();
        state.config.models = vec![crate::config::ModelProfile {
            name: "Exact profile".into(),
            model: "model".into(),
            supports_reasoning_effort: Some(true),
            reasoning_efforts: Some(vec!["low".into(), "high".into()]),
            ..Default::default()
        }];
        let state = Arc::new(Mutex::new(state));
        let mut token = CancellationToken::new();
        let command = |text: &str| RemoteOperation::ExecuteCommand {
            command: text.into(),
        };
        apply(&state, &mut token, command("  /MODEL Exact profile  "))
            .await
            .unwrap();
        apply(&state, &mut token, command("/effort high"))
            .await
            .unwrap();
        assert_eq!(
            state.lock().await.config.models[0]
                .reasoning_effort
                .as_deref(),
            Some("high")
        );
        assert_eq!(
            code(apply(&state, &mut token, command("/effort imaginary")).await),
            RemoteErrorCode::InvalidRequest
        );
        for text in [
            "/exit",
            "/new",
            "/clear",
            "/compact",
            "/config",
            "/status change",
            "plain text",
        ] {
            assert_eq!(
                code(apply(&state, &mut token, command(text)).await),
                RemoteErrorCode::UnsupportedOperation
            );
        }
        state.lock().await.begin_turn_identity();
        assert_eq!(
            code(apply(&state, &mut token, command("/effort low")).await),
            RemoteErrorCode::Busy
        );
        apply(&state, &mut token, command("/status")).await.unwrap();
        assert_eq!(
            state.lock().await.config.models[0]
                .reasoning_effort
                .as_deref(),
            Some("high")
        );
        assert!(state.lock().await.history.is_empty());
        assert!(state.lock().await.pending_queue.is_empty());
        assert_eq!(state.lock().await.input_buffer, "my terminal draft");
    }

    #[tokio::test]
    async fn remote_cancel_releases_a_real_questionnaire_tool_waiter() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut token = CancellationToken::new();
        let tool_state = state.clone();
        let tool_token = token.clone();
        let waiter = tokio::spawn(async move {
            crate::network::tool_exec::ask_user_question(
                &tool_state,
                &tool_token,
                &serde_json::json!({
                    "questions": [
                        {"question":"First?", "options":["Yes", "No"]},
                        {"question":"Second?", "options":["Yes", "No"]}
                    ]
                }),
            )
            .await
        });
        let id = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(question) = state.lock().await.pending_question.as_ref() {
                    break question.id.clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let operation = serde_json::from_value::<RemoteOperation>(serde_json::json!({
            "type": "cancel_question", "question_id": id
        }))
        .expect("targeted cancellation is supported");
        apply(&state, &mut token, operation).await.unwrap();
        let (output, _) = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .unwrap()
            .unwrap();
        assert!(!output.success);
        assert!(state.lock().await.pending_question.is_none());
        assert!(state.lock().await.pending_question_queue.is_empty());
    }

    #[tokio::test]
    async fn remote_question_cancel_clears_chain_and_rejects_stale_identity() {
        let mut state = shared_state();
        state.begin_question_chain(vec![
            PendingQuestion::new("First?".into(), vec!["Yes".into()], false),
            PendingQuestion::new("Second?".into(), vec!["Yes".into()], false),
        ]);
        let id = state.pending_question.as_ref().unwrap().id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.question_response = Some(tx);
        let state = Arc::new(Mutex::new(state));
        let mut token = CancellationToken::new();
        let old_token = token.clone();
        let operation = |id: &str| {
            serde_json::from_value::<RemoteOperation>(serde_json::json!({
                "type": "cancel_question", "question_id": id
            }))
            .expect("targeted cancellation is supported")
        };
        assert_eq!(
            code(apply(&state, &mut token, operation("old")).await),
            RemoteErrorCode::StaleQuestion
        );
        assert!(!old_token.is_cancelled());
        apply(&state, &mut token, operation(&id)).await.unwrap();
        assert_eq!(rx.await.unwrap(), "User cancelled prompt.");
        assert!(old_token.is_cancelled());
        assert!(!token.is_cancelled());
        assert!(state.lock().await.pending_question.is_none());
        assert!(state.lock().await.pending_question_queue.is_empty());
        assert_eq!(
            code(apply(&state, &mut token, operation(&id)).await),
            RemoteErrorCode::StaleQuestion
        );
    }

    #[tokio::test]
    async fn remote_commands_produce_output_without_touching_prompt_history() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut token = CancellationToken::new();
        for command in ["/help", "/status", "/info", "/usage", "/model", "/effort"] {
            let op = serde_json::from_value::<RemoteOperation>(serde_json::json!({
                "type": "execute_command", "command": command
            }))
            .expect("remote commands are supported");
            let result = apply(&state, &mut token, op).await.unwrap();
            let json = serde_json::to_value(result.result).unwrap();
            assert_eq!(json["type"], "command_executed");
            assert!(!json["output"].as_str().unwrap().is_empty());
        }
        assert!(state.lock().await.history.is_empty());
        assert!(state.lock().await.pending_queue.is_empty());
    }

    #[tokio::test]
    async fn oversized_settings_result_is_rejected_before_mutation() {
        let mut state = shared_state();
        state.config.models = vec![crate::config::ModelProfile {
            name: "choice".into(),
            model: "m".repeat(super::super::MAX_REMOTE_FRAME_BYTES),
            ..Default::default()
        }];
        let before = state.model_name.clone();
        let state = Arc::new(Mutex::new(state));
        let mut token = CancellationToken::new();
        let result = apply(
            &state,
            &mut token,
            RemoteOperation::SetSessionSettings {
                model: "choice".into(),
                reasoning_effort: "default".into(),
            },
        )
        .await;
        assert_eq!(
            result.err().map(|error| error.code),
            Some(RemoteErrorCode::FrameTooLarge)
        );
        assert_eq!(state.lock().await.model_name, before);
    }

    #[tokio::test]
    async fn settings_mutation_selects_exact_profile_and_rejects_busy_or_invalid_values() {
        let mut state = shared_state();
        state.config.models = vec![crate::config::ModelProfile {
            name: "chosen".into(),
            model: "model".into(),
            url: "https://example.test/v1".into(),
            reasoning_effort: Some("high".into()),
            supports_reasoning_effort: Some(true),
            ..Default::default()
        }];
        let state = Arc::new(Mutex::new(state));
        let mut token = CancellationToken::new();
        let op = serde_json::from_value::<RemoteOperation>(serde_json::json!({
            "type": "set_session_settings", "model": "chosen", "reasoning_effort": "high"
        }))
        .expect("settings operation is supported");
        apply(&state, &mut token, op.clone()).await.unwrap();
        assert_eq!(state.lock().await.model_name, "model");
        let mut invalid = op.clone();
        if let RemoteOperation::SetSessionSettings {
            reasoning_effort, ..
        } = &mut invalid
        {
            *reasoning_effort = "imaginary".into();
        }
        assert_eq!(
            code(apply(&state, &mut token, invalid).await),
            RemoteErrorCode::InvalidRequest
        );
        let mut stale = request(op.clone());
        stale.registration_epoch = Some(6);
        assert_eq!(
            code(apply_session_mutation(&state, &mut token, &registration(), &stale).await),
            RemoteErrorCode::StaleSession
        );
        let clear = RemoteOperation::SetSessionSettings {
            model: "chosen".into(),
            reasoning_effort: "default".into(),
        };
        apply(&state, &mut token, clear).await.unwrap();
        assert!(
            state.lock().await.config.models[0]
                .reasoning_effort
                .is_none()
        );
        apply(&state, &mut token, op.clone()).await.unwrap();
        state.lock().await.begin_turn_identity();
        assert_eq!(
            code(apply(&state, &mut token, op).await),
            RemoteErrorCode::Busy
        );
    }

    #[tokio::test]
    async fn stale_turn_id_cannot_cancel_the_turn_that_replaced_it() {
        let mut state = shared_state();
        let observed = state.begin_turn_identity();
        // The observed turn ends and the next queued prompt starts.
        state.end_turn_identity(&observed);
        let current = state.begin_turn_identity();
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();

        let stale = apply(
            &state,
            &mut cancel_token,
            RemoteOperation::CancelTurn { turn_id: observed },
        )
        .await;
        assert_eq!(code(stale), RemoteErrorCode::StaleTurn);
        assert!(!cancel_token.is_cancelled(), "the newer turn keeps running");
        assert_eq!(
            state.lock().await.active_turn_id.as_deref(),
            Some(current.as_str())
        );

        let applied = apply(
            &state,
            &mut cancel_token,
            RemoteOperation::CancelTurn {
                turn_id: current.clone(),
            },
        )
        .await
        .expect("the running turn accepts its own cancel");
        assert_eq!(applied.follow_up, OwnerFollowUp::FinishCancel);
        assert!(cancel_token.is_cancelled());

        // A second device repeating the cancel is told the turn is gone.
        let repeated = apply(
            &state,
            &mut cancel_token,
            RemoteOperation::CancelTurn { turn_id: current },
        )
        .await;
        assert_eq!(code(repeated), RemoteErrorCode::StaleTurn);
    }

    #[tokio::test]
    async fn cancel_without_a_running_turn_is_stale() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut cancel_token = CancellationToken::new();
        let result = apply(
            &state,
            &mut cancel_token,
            RemoteOperation::CancelTurn {
                turn_id: "turn:0:0".to_owned(),
            },
        )
        .await;
        assert_eq!(code(result), RemoteErrorCode::StaleTurn);
        assert!(!cancel_token.is_cancelled());
    }

    #[tokio::test]
    async fn stale_question_id_cannot_answer_the_question_that_replaced_it() {
        let mut state = shared_state();
        let first = PendingQuestion::new("First?".to_owned(), vec!["Yes".to_owned()], false);
        let second = PendingQuestion::new("Second?".to_owned(), vec!["Yes".to_owned()], false);
        let (first_id, second_id) = (first.id.clone(), second.id.clone());
        assert_ne!(first_id, second_id);
        // Identical text and options: only the identity tells them apart.
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(second);
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.question_response = Some(tx);
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();
        let answer = |question_id: &str| RemoteOperation::AnswerQuestion {
            question_id: question_id.to_owned(),
            answer: RemoteAnswer::Selected {
                options: vec!["Yes".to_owned()],
            },
        };

        let stale = apply(&state, &mut cancel_token, answer(&first_id)).await;
        assert_eq!(code(stale), RemoteErrorCode::StaleQuestion);
        assert!(rx.try_recv().is_err(), "the pending question is untouched");
        assert!(state.lock().await.pending_question.is_some());

        apply(&state, &mut cancel_token, answer(&second_id))
            .await
            .expect("the pending question accepts its own answer");
        assert_eq!(rx.await.expect("answer delivered"), "User selected: Yes");

        // First valid answer wins; the terminal or a second device is late.
        let late = apply(&state, &mut cancel_token, answer(&second_id)).await;
        assert_eq!(code(late), RemoteErrorCode::StaleQuestion);
    }

    #[tokio::test]
    async fn answers_are_validated_against_the_named_question() {
        let mut state = shared_state();
        let question = PendingQuestion::new(
            "Pick one".to_owned(),
            vec!["A".to_owned(), "B".to_owned()],
            false,
        );
        let question_id = question.id.clone();
        state.pending_question = Some(question);
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.question_response = Some(tx);
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();
        let selected = |options: &[&str]| RemoteOperation::AnswerQuestion {
            question_id: question_id.clone(),
            answer: RemoteAnswer::Selected {
                options: options.iter().map(|option| (*option).to_owned()).collect(),
            },
        };

        for invalid in [
            selected(&[]),
            selected(&["C"]),
            selected(&["A", "B"]),
            RemoteOperation::AnswerQuestion {
                question_id: question_id.clone(),
                answer: RemoteAnswer::Custom {
                    text: "  ".to_owned(),
                },
            },
        ] {
            let result = apply(&state, &mut cancel_token, invalid).await;
            assert_eq!(code(result), RemoteErrorCode::InvalidAnswer);
        }
        assert!(rx.try_recv().is_err(), "an invalid answer resolves nothing");

        apply(&state, &mut cancel_token, selected(&["B"]))
            .await
            .expect("a listed option is accepted");
        assert_eq!(rx.await.expect("answer delivered"), "User selected: B");
    }

    #[tokio::test]
    async fn chained_questions_each_require_their_own_identity() {
        let mut state = shared_state();
        let first = PendingQuestion::new("One?".to_owned(), vec!["a".to_owned()], false);
        let second = PendingQuestion::new(
            "Two?".to_owned(),
            vec!["x".to_owned(), "y".to_owned()],
            true,
        );
        let (first_id, second_id) = (first.id.clone(), second.id.clone());
        state.begin_question_chain(vec![first, second]);
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.question_response = Some(tx);
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();
        let answer = |question_id: &str, options: &[&str]| RemoteOperation::AnswerQuestion {
            question_id: question_id.to_owned(),
            answer: RemoteAnswer::Selected {
                options: options.iter().map(|option| (*option).to_owned()).collect(),
            },
        };

        let early = apply(&state, &mut cancel_token, answer(&second_id, &["x"])).await;
        assert_eq!(code(early), RemoteErrorCode::StaleQuestion);
        apply(&state, &mut cancel_token, answer(&first_id, &["a"]))
            .await
            .expect("first question");
        let repeated = apply(&state, &mut cancel_token, answer(&first_id, &["a"])).await;
        assert_eq!(code(repeated), RemoteErrorCode::StaleQuestion);
        apply(&state, &mut cancel_token, answer(&second_id, &["x", "y"]))
            .await
            .expect("second question");

        let output = rx.await.expect("chain resolved");
        assert!(output.contains("x, y"), "unexpected chain output: {output}");
    }

    #[tokio::test]
    async fn stale_batch_id_cannot_resolve_the_batch_that_replaced_it() {
        let mut state = shared_state();
        state.pending_tool_confirmation = Some(vec![confirmation()]);
        state.pending_approval_batch_id = Some("controller:test:b".to_owned());
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        state.tool_confirmation_response = Some(tx);
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();
        let resolve = |batch_id: &str, choice| RemoteOperation::ResolveApproval {
            batch_id: batch_id.to_owned(),
            choice,
        };

        let stale = apply(
            &state,
            &mut cancel_token,
            resolve("controller:test:a", ApprovalChoice::Approve),
        )
        .await;
        assert_eq!(code(stale), RemoteErrorCode::StaleApproval);
        assert!(rx.try_recv().is_err(), "the pending batch is untouched");
        assert!(state.lock().await.pending_tool_confirmation.is_some());

        apply(
            &state,
            &mut cancel_token,
            resolve("controller:test:b", ApprovalChoice::Approve),
        )
        .await
        .expect("the pending batch accepts its own decision");
        assert_eq!(
            rx.await.expect("decision delivered"),
            ToolConfirmationResponse::Approve
        );

        let late = apply(
            &state,
            &mut cancel_token,
            resolve("controller:test:b", ApprovalChoice::Deny),
        )
        .await;
        assert_eq!(code(late), RemoteErrorCode::StaleApproval);
    }

    #[tokio::test]
    async fn stale_session_or_epoch_is_rejected_before_any_mutation() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut cancel_token = CancellationToken::new();
        let submit = RemoteOperation::SubmitPrompt {
            prompt: "hello".to_owned(),
        };

        let mut old_epoch = request(submit.clone());
        old_epoch.registration_epoch = Some(6);
        let result =
            apply_session_mutation(&state, &mut cancel_token, &registration(), &old_epoch).await;
        assert_eq!(code(result), RemoteErrorCode::StaleSession);

        let mut other_session = request(submit.clone());
        other_session.session_id = Some("another-session".to_owned());
        let result =
            apply_session_mutation(&state, &mut cancel_token, &registration(), &other_session)
                .await;
        assert_eq!(code(result), RemoteErrorCode::StaleSession);

        // The terminal moved to another session after the request was routed.
        state.lock().await.active_session_id = "replacement".to_owned();
        let result = apply(&state, &mut cancel_token, submit).await;
        assert_eq!(code(result), RemoteErrorCode::StaleSession);
        assert!(state.lock().await.pending_queue.is_empty());
    }

    #[tokio::test]
    async fn version_mismatch_is_rejected_at_the_owner_too() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut cancel_token = CancellationToken::new();
        let mut newer = request(RemoteOperation::SubmitPrompt {
            prompt: "hello".to_owned(),
        });
        newer.protocol_version = REMOTE_PROTOCOL_VERSION + 1;
        let error = apply_session_mutation(&state, &mut cancel_token, &registration(), &newer)
            .await
            .expect_err("a newer version is refused");
        assert_eq!(error.code, RemoteErrorCode::IncompatibleVersion);
        assert_eq!(error.supported_versions, [REMOTE_PROTOCOL_VERSION]);
        assert!(state.lock().await.pending_queue.is_empty());
    }

    #[tokio::test]
    async fn submit_is_idle_only_and_leaves_the_terminal_draft_alone() {
        let mut state = shared_state();
        state.input_buffer = "half-typed terminal draft".to_owned();
        state.cursor_position = 4;
        state.draft_submit_mode = DraftSubmitMode::Queue;
        let state = Arc::new(Mutex::new(state));
        let mut cancel_token = CancellationToken::new();
        let submit = |prompt: &str| RemoteOperation::SubmitPrompt {
            prompt: prompt.to_owned(),
        };

        let accepted = apply(&state, &mut cancel_token, submit("from the phone"))
            .await
            .expect("an idle session accepts a prompt");
        assert_eq!(
            accepted.result,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started
            }
        );
        assert_eq!(accepted.follow_up, OwnerFollowUp::StartTurn);
        {
            let state = state.lock().await;
            assert_eq!(state.pending_queue, ["from the phone"]);
            assert_eq!(state.input_buffer, "half-typed terminal draft");
            assert_eq!(state.cursor_position, 4);
            assert_eq!(state.draft_submit_mode, DraftSubmitMode::Queue);
        }

        // Accepted but not started yet still counts as running.
        let second = apply(&state, &mut cancel_token, submit("again")).await;
        assert_eq!(code(second), RemoteErrorCode::Busy);
        state.lock().await.status = AppStatus::Streaming;
        let third = apply(&state, &mut cancel_token, submit("again")).await;
        assert_eq!(code(third), RemoteErrorCode::Busy);
        assert_eq!(state.lock().await.pending_queue, ["from the phone"]);
    }

    #[tokio::test]
    async fn steer_and_queue_follow_the_running_turn_rules() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut cancel_token = CancellationToken::new();
        let steer = || RemoteOperation::Steer {
            prompt: "change course".to_owned(),
        };
        let queue = || RemoteOperation::Queue {
            prompt: "afterwards".to_owned(),
        };

        assert_eq!(
            code(apply(&state, &mut cancel_token, steer()).await),
            RemoteErrorCode::NotRunning
        );
        assert_eq!(
            code(apply(&state, &mut cancel_token, queue()).await),
            RemoteErrorCode::NotRunning
        );

        // Running, but this turn does not accept steering: refuse, never
        // fall back to queueing behind the user's back.
        state.lock().await.status = AppStatus::Streaming;
        assert_eq!(
            code(apply(&state, &mut cancel_token, steer()).await),
            RemoteErrorCode::UnsupportedOperation
        );
        assert!(state.lock().await.pending_queue.is_empty());

        let queued = apply(&state, &mut cancel_token, queue())
            .await
            .expect("a running turn accepts a queued prompt");
        assert_eq!(queued.follow_up, OwnerFollowUp::None);
        assert_eq!(state.lock().await.pending_queue, ["afterwards"]);

        {
            let mut state = state.lock().await;
            state.active_turn_steerable_session = Some(SESSION.to_owned());
        }
        apply(&state, &mut cancel_token, steer())
            .await
            .expect("a steerable turn accepts a steer");
        let state = state.lock().await;
        assert_eq!(state.pending_steers[0].text, "change course");
        assert_eq!(state.pending_queue, ["afterwards"]);
    }

    #[test]
    fn owner_reads_are_bound_to_the_registration() {
        let mut state = shared_state();
        state
            .history
            .push(crate::app::ChatMessage::new("user", "hello"));
        let limits = crate::remote::ProjectionLimits::default();
        let history = request(RemoteOperation::GetHistory {
            cursor: None,
            limit: 10,
        });
        let Ok(RemoteResult::History(page)) =
            read_session(&state, &registration(), &history, &limits)
        else {
            panic!("the owner serves its own transcript");
        };
        assert_eq!(page.messages.len(), 1);

        let content = request(RemoteOperation::GetContent {
            content_id: "message:missing:0".to_owned(),
            offset: 0,
            max_bytes: 16,
        });
        let missing = read_session(&state, &registration(), &content, &limits);
        assert_eq!(missing.unwrap_err().code, RemoteErrorCode::NotFound);

        let attach = request(RemoteOperation::AttachSession { resume: None });
        let unsupported = read_session(&state, &registration(), &attach, &limits);
        assert_eq!(
            unsupported.unwrap_err().code,
            RemoteErrorCode::UnsupportedOperation
        );

        let mut old_epoch = history.clone();
        old_epoch.registration_epoch = Some(6);
        let stale = read_session(&state, &registration(), &old_epoch, &limits);
        assert_eq!(stale.unwrap_err().code, RemoteErrorCode::StaleSession);
        state.active_session_id = "replacement".to_owned();
        let stale = read_session(&state, &registration(), &history, &limits);
        assert_eq!(stale.unwrap_err().code, RemoteErrorCode::StaleSession);
    }

    #[tokio::test]
    async fn remote_slash_commands_are_refused_and_reads_are_not_mutations() {
        let state = Arc::new(Mutex::new(shared_state()));
        let mut cancel_token = CancellationToken::new();
        for prompt in ["/exit", "  /yolo on", "/new"] {
            let result = apply(
                &state,
                &mut cancel_token,
                RemoteOperation::SubmitPrompt {
                    prompt: prompt.to_owned(),
                },
            )
            .await;
            assert_eq!(code(result), RemoteErrorCode::UnsupportedOperation);
        }
        assert!(state.lock().await.pending_queue.is_empty());

        let read = apply(&state, &mut cancel_token, RemoteOperation::DetachSession).await;
        assert_eq!(code(read), RemoteErrorCode::UnsupportedOperation);
        let empty = apply(
            &state,
            &mut cancel_token,
            RemoteOperation::SubmitPrompt {
                prompt: "   ".to_owned(),
            },
        )
        .await;
        assert_eq!(code(empty), RemoteErrorCode::InvalidRequest);
    }
}
