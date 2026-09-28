//! CLI entry points for the terminal frontend: argument dispatch,
//! headless/ACP/daemon modes, and the interactive terminal session.

use crate::runtime::AppRuntime;
use crate::ui::TerminalRuntime;
use clap::Parser;
use ratatui::{
    backend::Backend,
    widgets::{Paragraph, Widget, Wrap},
};
use rustcode::app::AppState;
use std::sync::Arc;
use tokio::sync::Mutex;
pub(crate) fn insert_scrollback_lines<B: Backend>(
    terminal: &mut crate::inline_terminal::InlineTerminal<B>,
    lines: Vec<ratatui::text::Line<'static>>,
    width: u16,
) -> Result<(), B::Error> {
    if lines.is_empty() {
        return Ok(());
    }
    let height = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false })
        .line_count(width)
        .max(1) as u16;
    terminal.insert_before(height, |buffer| {
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(buffer.area, buffer);
    })
}

pub(crate) fn should_clear_mutable_viewport_before_history(
    _response_just_finished: bool,
    _transcript_at_start: bool,
    has_pending_history: bool,
) -> bool {
    has_pending_history
}
fn hydrate_shell_provider_keys() {
    // Cheap path first: read the workspace config to learn which env names
    // the user's profiles actually reference.
    let configured: Vec<String> = std::env::current_dir()
        .ok()
        .map(|workspace| rustcode::config::load_config_for_workspace(&workspace).2)
        .map(|config| {
            config
                .models
                .iter()
                .filter_map(|profile| profile.api_key_env_name())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    rustcode::shell_env::hydrate_provider_keys(&configured);
}
#[cfg(unix)]
async fn run_daemon_or_cron_command(
    cli_args: &crate::cli::Cli,
) -> Result<bool, Box<dyn std::error::Error>> {
    use rustcode::daemon::{
        client::DaemonClient,
        create_job, format_response,
        lifecycle::DaemonLifecycle,
        protocol::{DaemonRequest, DaemonResponse},
    };

    let Some(command) = cli_args.command.as_ref() else {
        return Ok(false);
    };
    if !matches!(
        command,
        crate::cli::Commands::Daemon { .. } | crate::cli::Commands::Cron { .. }
    ) {
        return Ok(false);
    }
    let config_dir = rustcode::config::get_config_dir().ok_or("config directory unavailable")?;
    let lifecycle = DaemonLifecycle::new(&config_dir);

    match command {
        crate::cli::Commands::Daemon { command } => match command {
            crate::cli::DaemonCommands::Run { json: _ } => lifecycle.run().await?,
            crate::cli::DaemonCommands::Start { json } => {
                let mut child = tokio::process::Command::new(std::env::current_exe()?);
                child
                    .args(["daemon", "run"])
                    .env("RUSTCODE_CONFIG_DIR", &config_dir);
                let status = lifecycle.start(child).await?;
                println!(
                    "{}",
                    format_response("status", &DaemonResponse::Status { status }, *json)?
                );
            }
            crate::cli::DaemonCommands::Stop { json } => match lifecycle.stop().await? {
                Some(status) if *json => println!(
                    "{}",
                    serde_json::json!({"status":"stopped","pid":status.pid,"instance_id":status.instance_id})
                ),
                Some(status) => println!("Daemon stopped (pid {}).", status.pid),
                None if *json => println!("{}", serde_json::json!({"status":"stopped"})),
                None => println!("Daemon is not running."),
            },
            crate::cli::DaemonCommands::Status { json } => match lifecycle.status().await? {
                Some(status) => println!(
                    "{}",
                    format_response("status", &DaemonResponse::Status { status }, *json)?
                ),
                None if *json => println!("{}", serde_json::json!({"status":"stopped"})),
                None => println!("Daemon: stopped"),
            },
            crate::cli::DaemonCommands::Logs { lines, json } => {
                let output = lifecycle.read_log_tail(*lines)?;
                if *json {
                    println!("{}", serde_json::json!({"lines":lines,"log":output}));
                } else {
                    print!("{output}");
                }
            }
        },
        crate::cli::Commands::Cron { command } => {
            let (operation, request, json) = match command {
                crate::cli::CronCommands::Add {
                    id,
                    name,
                    workspace,
                    schedule,
                    action,
                    target_session,
                    retry_policy,
                    json,
                } => (
                    "add",
                    DaemonRequest::Create {
                        job: create_job(
                            id,
                            name,
                            workspace,
                            schedule,
                            action,
                            target_session.as_deref(),
                            retry_policy.as_deref(),
                        )?,
                    },
                    *json,
                ),
                crate::cli::CronCommands::List { json } => ("list", DaemonRequest::List, *json),
                crate::cli::CronCommands::Pause { job_id, json } => (
                    "pause",
                    DaemonRequest::SetPaused {
                        job_id: job_id.clone(),
                        paused: true,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Resume { job_id, json } => (
                    "resume",
                    DaemonRequest::SetPaused {
                        job_id: job_id.clone(),
                        paused: false,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Run { job_id, json } => (
                    "run",
                    DaemonRequest::RunNow {
                        job_id: job_id.clone(),
                    },
                    *json,
                ),
                crate::cli::CronCommands::History {
                    job_id,
                    limit,
                    json,
                } => (
                    "history",
                    DaemonRequest::History {
                        job_id: job_id.clone(),
                        limit: *limit,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Delete { job_id, json } => (
                    "delete",
                    DaemonRequest::Delete {
                        job_id: job_id.clone(),
                    },
                    *json,
                ),
            };
            let response = DaemonClient::new(lifecycle.socket_path())
                .request(request)
                .await
                .map_err(|error| format!("daemon unavailable: {error}"))?;
            println!("{}", format_response(operation, &response, json)?);
        }
        _ => unreachable!(),
    }
    Ok(true)
}

#[cfg(not(unix))]
async fn run_daemon_or_cron_command(
    cli_args: &crate::cli::Cli,
) -> Result<bool, Box<dyn std::error::Error>> {
    if matches!(
        cli_args.command,
        Some(crate::cli::Commands::Daemon { .. } | crate::cli::Commands::Cron { .. })
    ) {
        return Err("daemon commands are supported on macOS and Linux".into());
    }
    Ok(false)
}
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = crate::cli::Cli::parse();
    if run_daemon_or_cron_command(&cli_args).await? {
        return Ok(());
    }
    // Rotate a debug log left by a previous process before startup emits new
    // diagnostics. The logger also checks the cap before each append so long-
    // lived processes remain bounded.
    rustcode::logger::rotate_if_oversized();
    // Issue #1226: silent mid-stream hangs left zero evidence. A panic hook
    // preserves the message + location in debug.log even when the process
    // dies without a crash report.
    rustcode::logger::install_panic_hook();
    // Provider keys are often exported in `~/.zshrc` (interactive-only) and
    // invisible to GUI/systemd/IDE launches. Hydrate missing keys from the
    // login shell once at startup so profiles, MCP servers, and tool shells
    // all see the same values the user's terminal sees.
    hydrate_shell_provider_keys();

    let model_override = cli_args.model.clone();

    if cli_args.init || matches!(cli_args.command.as_ref(), Some(crate::cli::Commands::Init)) {
        let workspace = std::env::current_dir()?;
        match rustcode::config::init_project_config(&workspace) {
            Ok(path) => println!(
                "Created project config at {}\nAdded {} to .gitignore.",
                path.display(),
                ".rustcode/config.toml"
            ),
            Err(error) => {
                eprintln!("Project config initialization failed: {error}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Sessions { command }) = cli_args.command.as_ref() {
        if let Some(crate::cli::SessionCommands::Migrate { dry_run }) = command {
            let Some(report) = rustcode::config::migrate_legacy_sessions(*dry_run) else {
                eprintln!("Session migration failed: configuration directory is unavailable.");
                std::process::exit(1);
            };
            println!(
                "{} {} legacy session(s): {} migrated, {} skipped, {} error(s).",
                if *dry_run {
                    "Would migrate"
                } else {
                    "Processed"
                },
                report.found,
                report.migrated,
                report.skipped,
                report.errors.len()
            );
            for error in &report.errors {
                eprintln!("session migration: {error}");
            }
            if !report.errors.is_empty() {
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Doctor { fix }) = cli_args.command.as_ref() {
        let code = rustcode::doctor::run_doctor(*fix);
        if code != 0 {
            std::process::exit(code);
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Bench {
        rounds,
        calls,
        recoveries,
        completed,
    }) = cli_args.command.as_ref()
    {
        let stats = rustcode::benchmark::TurnStats {
            rounds: *rounds,
            tool_calls: *calls,
            recoveries: *recoveries,
            completed: *completed,
        };
        println!("{}", rustcode::benchmark::format_report(stats));
        return Ok(());
    }

    if let Some(crate::cli::Commands::Discord {
        setup,
        status: _status,
        enable,
        disable,
    }) = cli_args.command.as_ref()
    {
        let workspace = std::env::current_dir()?;
        let (_, _, mut config) = rustcode::config::load_config_for_workspace(&workspace);
        if *setup || *enable || *disable {
            config.discord_rpc_enabled = !*disable;
            rustcode::config::save_entire_config(&config);
            println!(
                "Discord Rich Presence {}.",
                if config.discord_rpc_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if *setup {
                println!(
                    "Keep the Discord desktop app running and ensure the RustCode application has a `rustcode_logo` large asset."
                );
                println!(
                    "Rich Presence uses Discord's local desktop IPC; RustCode has no Discord login flow and never uses passwords, tokens, or browser profiles."
                );
            }
        } else {
            let socket = rustcode::discord_rpc::ipc_socket_detected();
            println!(
                "Discord Rich Presence: {}",
                if config.discord_rpc_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!(
                "Discord desktop IPC: {}",
                if socket {
                    "socket detected"
                } else {
                    "not detected (Discord may be closed)"
                }
            );
            if let Some(path) = rustcode::config::get_config_dir() {
                println!("Config directory: {}", path.display());
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Sync {
        command,
        pull,
        push,
    }) = cli_args.command
    {
        let action = match crate::cli::resolve_sync_action(command, pull, push) {
            Ok(action) => action,
            Err(error) => {
                eprintln!("Sync argument error: {error}");
                std::process::exit(2);
            }
        };

        match action {
            crate::cli::SyncAction::Pull => {
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = rustcode::config::sync_config_pull() {
                    eprintln!("Sync pull failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            crate::cli::SyncAction::Push => {
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = rustcode::config::sync_config_push() {
                    eprintln!("Sync push failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            crate::cli::SyncAction::Init(remote_url) => {
                if let Err(e) = rustcode::config::init_sync_repo(&remote_url) {
                    eprintln!("Error initializing sync repo: {e}");
                    std::process::exit(1);
                }
                println!(
                    "Sync repository setup complete! You can now run `rustcode sync` anytime."
                );
            }
            crate::cli::SyncAction::PullThenPush => {
                // Default behavior for `rustcode sync` (pull then push)
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = rustcode::config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = rustcode::config::sync_config_push() {
                    eprintln!("Sync failed during push: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
        }
        return Ok(());
    }

    if cli_args.update {
        println!("Checking if new release...");
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;
        let check = match rustcode::update::check_for_update(&client).await {
            Ok(check) => check,
            Err(error) => {
                eprintln!("Update check failed: {error}");
                std::process::exit(1);
            }
        };

        match check {
            rustcode_core::update::UpdateCheck::UpToDate { current, latest } => {
                println!(
                    "No new release. rustcode v{} is up to date (latest: v{}).",
                    rustcode_core::update::format_version(current),
                    rustcode_core::update::format_version(latest)
                );
            }
            rustcode_core::update::UpdateCheck::Available { current, latest } => {
                println!(
                    "Found new release: v{} → v{}, updating now...",
                    rustcode_core::update::format_version(current),
                    rustcode_core::update::format_version(latest)
                );
                match rustcode::update::run_update(&client, latest).await {
                    Ok(()) => {
                        println!("🎉 Update ran successfully! Please restart rustcode.");
                    }
                    Err(error) => {
                        eprintln!("Update failed: {error}");
                        std::process::exit(1);
                    }
                }
            }
        }
        return Ok(());
    }

    let acp_subcommand = matches!(cli_args.command.as_ref(), Some(crate::cli::Commands::Acp));
    if cli_args.acp || acp_subcommand {
        rustcode::acp::run_acp(cli_args.yolo).await?;
        rustcode::config::flush_history();
        return Ok(());
    }

    if let Some(prompt) = cli_args.prompt {
        if let Some(max_iters) = cli_args.loop_count {
            let report =
                rustcode::raw_cli::run_raw_cli_loop(&prompt, model_override.as_deref(), max_iters)
                    .await?;
            println!(
                "Loop finished after {} turn(s){}.",
                report.iters,
                if report.completed_via_done {
                    " (done marker)"
                } else {
                    " (iteration cap)"
                }
            );
        } else {
            rustcode::raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
        }
        rustcode::config::flush_history();
        return Ok(());
    }

    return run_interactive(cli_args, model_override).await;
}
async fn run_interactive(
    cli_args: crate::cli::Cli,
    model_override: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let terminal_runtime = TerminalRuntime::start()?;

    rustcode::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    // Themes are a terminal-UI concern: shared state no longer applies them.
    // The interactive runtime seeds the palette once before the first frame;
    // every render re-applies it from `state.config().theme`.
    crate::ui::theme::ensure_themes_dir();
    crate::ui::theme::set_active_theme(&app_state_struct.config.theme);
    if cli_args.yolo {
        app_state_struct.auto_confirm = true;
    }
    if cli_args.resume || cli_args.continue_session {
        if let Err(error) = rustcode::app::session_controller::SessionController::default()
            .resume(&mut app_state_struct, rustcode::app::SessionAction::Latest)
        {
            let message = if matches!(
                &error,
                rustcode::app::session_controller::SessionError::NoSessionToResume
            ) {
                "No previous session to resume.".to_owned()
            } else {
                error.to_string()
            };
            app_state_struct
                .history
                .push(rustcode::app::ChatMessage::new("system", message));
        } else if cli_args.continue_session {
            let queued = rustcode::app::actions::queue_restored_segment(&mut app_state_struct);
            if !queued {
                app_state_struct
                    .history
                    .push(rustcode::app::ChatMessage::new(
                        "system",
                        "No pending session work is available to continue.",
                    ));
            }
        }
    }
    if let Some(ref m_name) = model_override
        && let Some(profile) = app_state_struct
            .config
            .models
            .iter()
            .find(|m| m.name == *m_name)
    {
        app_state_struct.api_base_url = profile.url.clone();
        app_state_struct.model_name = profile.model.clone();
    }
    let app_state = Arc::new(Mutex::new(app_state_struct));

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        // Keep the transport-level body read bounded as a second safety net
        // for providers that close or stall an SSE connection without waking
        // the stream future. The stream loop also tracks meaningful-event
        // progress, so this does not impose a total response deadline.
        .read_timeout(std::time::Duration::from_secs(120))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .build()?;
    {
        let mut state = app_state.lock().await;
        state.update_check = rustcode_core::update::UpdateState::Checking;
    }
    let update_state = Arc::clone(&app_state);
    let update_client = client.clone();
    tokio::spawn(async move {
        let result = rustcode::update::check_for_update(&update_client).await;
        let mut state = update_state.lock().await;
        state.update_check = match result {
            Ok(rustcode_core::update::UpdateCheck::UpToDate { latest, .. }) => {
                rustcode_core::update::UpdateState::UpToDate(latest)
            }
            Ok(rustcode_core::update::UpdateCheck::Available { latest, .. }) => {
                if state.dismissed_update_version != Some(latest) {
                    state.show_update_prompt = true;
                    state.update_prompt_index = 0;
                }
                rustcode_core::update::UpdateState::Available(latest)
            }
            Err(_) => rustcode_core::update::UpdateState::Failed,
        };
        state.request_redraw();
    });
    // Spawn startup initialization of enabled MCP servers.
    let mcp_servers = app_state.lock().await.config.mcp_servers.clone();
    let mcp_state = Arc::clone(&app_state);
    tokio::spawn(async move {
        let warnings = rustcode::mcp::start_enabled_servers(&mcp_servers, |name| async move {
            rustcode::mcp::start_server_by_name(&name).await
        })
        .await;
        if !warnings.is_empty() {
            let mut state = mcp_state.lock().await;
            state.exit_warnings.extend(warnings);
        }
    });

    rustcode::app::spawn_context_window_detection(Arc::clone(&app_state), client.clone());

    {
        let state_quota = Arc::clone(&app_state);
        let client_quota = client.clone();
        tokio::spawn(async move {
            rustcode::network::fetch_model_quota(&client_quota, &state_quota).await;
        });
    }

    let app_runtime = AppRuntime::new(terminal_runtime, app_state, client)?;
    let exit_summary = app_runtime.run().await?;
    if exit_summary.print_handoff {
        print_exit_summary(&exit_summary);
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExitSummary {
    pub(crate) prompt_tokens: u64,
    pub(crate) cached_tokens: u64,
    pub(crate) completion_tokens: u64,
    pub(crate) reasoning_tokens: u64,
    pub(crate) session_id: String,
    pub(crate) composer_y: Option<u16>,
    pub(crate) print_handoff: bool,
    pub(crate) warnings: Vec<String>,
}

impl ExitSummary {
    pub(crate) fn from_state(state: &AppState) -> Self {
        let mut summary = Self {
            prompt_tokens: 0,
            cached_tokens: 0,
            completion_tokens: 0,
            reasoning_tokens: 0,
            session_id: state.active_session_id.clone(),
            composer_y: state.input_text_area.map(|area| area.y),
            print_handoff: true,
            warnings: state.exit_warnings.clone(),
        };

        for message in &state.history {
            if let Some(usage) = &message.token_usage {
                summary.prompt_tokens = summary
                    .prompt_tokens
                    .saturating_add(u64::from(usage.prompt_tokens));
                summary.cached_tokens = summary
                    .cached_tokens
                    .saturating_add(u64::from(usage.cached_tokens.unwrap_or(0)));
                summary.completion_tokens = summary
                    .completion_tokens
                    .saturating_add(u64::from(usage.completion_tokens));
            }
            summary.reasoning_tokens = summary
                .reasoning_tokens
                .saturating_add(u64::from(message.thought_tokens.unwrap_or(0)));
        }

        summary
    }

    pub(crate) fn usage_line(&self) -> Option<String> {
        let total = self.prompt_tokens.saturating_add(self.completion_tokens);
        if total == 0 {
            return None;
        }
        let cached = (self.cached_tokens > 0)
            .then(|| format!(" (+ {} cached)", format_number(self.cached_tokens)))
            .unwrap_or_default();
        let reasoning = (self.reasoning_tokens > 0)
            .then(|| format!(" (reasoning {})", format_number(self.reasoning_tokens)))
            .unwrap_or_default();
        Some(format!(
            "Token usage: total={} input={}{} output={}{}",
            format_number(total),
            format_number(self.prompt_tokens),
            cached,
            format_number(self.completion_tokens),
            reasoning,
        ))
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// Printed after restoring the terminal and erasing the transient composer,
/// matching Codex's compact usage and resume handoff.
fn print_exit_summary(summary: &ExitSummary) {
    use std::io::Write;

    let mut out = std::io::stdout();
    if let Some(usage) = summary.usage_line() {
        let _ = writeln!(out, "{usage}");
    }
    if !summary.session_id.is_empty() {
        let _ = writeln!(out, "To continue this session, run rustcode --resume");
    }
    if !summary.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Warnings:");
        for warning in &summary.warnings {
            let _ = writeln!(out, "  - {warning}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ExitSummary, format_number, should_clear_mutable_viewport_before_history};

    #[test]
    fn exit_summary_formats_codex_style_usage() {
        let summary = ExitSummary {
            prompt_tokens: 2_249_608,
            cached_tokens: 60_154_240,
            completion_tokens: 132_560,
            reasoning_tokens: 48_884,
            session_id: "session-id".to_string(),
            composer_y: Some(12),
            print_handoff: true,
            warnings: Vec::new(),
        };
        assert_eq!(
            summary.usage_line().as_deref(),
            Some(
                "Token usage: total=2,382,168 input=2,249,608 (+ 60,154,240 cached) output=132,560 (reasoning 48,884)"
            )
        );
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
    }

    #[test]
    fn exit_summary_inherits_state_warnings() {
        let mut state = rustcode::app::AppState::new();
        state
            .exit_warnings
            .push("[mcp] timed out starting server test after 10.0s; continuing".to_owned());
        let summary = ExitSummary::from_state(&state);
        assert_eq!(
            summary.warnings.as_slice(),
            ["[mcp] timed out starting server test after 10.0s; continuing"]
        );
    }

    #[test]
    fn pending_history_clears_mutable_cell_before_history_insertion() {
        assert!(should_clear_mutable_viewport_before_history(
            true, false, true,
        ));
        assert!(should_clear_mutable_viewport_before_history(
            false, true, true,
        ));
        assert!(should_clear_mutable_viewport_before_history(
            false, false, true,
        ));
        assert!(!should_clear_mutable_viewport_before_history(
            false, false, false,
        ));
    }
}
