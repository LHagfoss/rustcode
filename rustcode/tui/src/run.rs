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
use std::io::Write;
use std::sync::Arc;
use tokio::sync::Mutex;
/// Hard cap on the rows one committed block may push into terminal scrollback.
///
/// Independent of the engine's `MAX_TOOL_OUTPUT_LINES` and of the per-tool
/// transcript cap in `ui::tool_result`: this is the last bound before a block
/// becomes terminal output, so a regression in either render layer still
/// cannot make scrollback grow without limit. (#1593)
pub(crate) const MAX_SCROLLBACK_BLOCK_LINES: usize = 400;

pub(crate) fn insert_scrollback_lines<B: Backend>(
    terminal: &mut crate::inline_terminal::InlineTerminal<B>,
    lines: Vec<ratatui::text::Line<'static>>,
    width: u16,
) -> Result<(), B::Error> {
    if lines.is_empty() {
        return Ok(());
    }
    let lines = cap_scrollback_block(lines);
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

/// Truncate a committed block to [`MAX_SCROLLBACK_BLOCK_LINES`], naming the
/// omitted count so scrollback never silently misrepresents the transcript.
pub(crate) fn cap_scrollback_block(
    mut lines: Vec<ratatui::text::Line<'static>>,
) -> Vec<ratatui::text::Line<'static>> {
    if lines.len() <= MAX_SCROLLBACK_BLOCK_LINES {
        return lines;
    }
    let omitted = lines.len() - MAX_SCROLLBACK_BLOCK_LINES;
    lines.truncate(MAX_SCROLLBACK_BLOCK_LINES);
    lines.push(ratatui::text::Line::from(format!(
        "… +{omitted} more transcript lines"
    )));
    lines
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
/// Run `rustcode mcp …`. Edits go to the user config; the workspace's merged
/// view is consulted only to say when a project file hides the change.
fn run_mcp_command(
    command: &crate::cli::McpCommands,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let active = |name: &str| {
        let (_, _, config) = rustcode::config::load_config_for_workspace(workspace);
        config.mcp_servers.iter().any(|server| server.name == name)
    };
    match command {
        crate::cli::McpCommands::Add(args) => {
            let server = crate::cli::mcp_server_from_args(args)?;
            let summary = crate::cli::mcp_server_summary(&server);
            let replaced = rustcode::config::add_mcp_server(server, args.force)?;
            println!(
                "{} MCP server: {summary}",
                if replaced { "Replaced" } else { "Added" }
            );
            if !active(&args.name) {
                println!(
                    "Note: a project .rustcode/config.toml sets its own mcp_servers, so '{}' is not active in this workspace.",
                    args.name
                );
            }
        }
        crate::cli::McpCommands::List => {
            let (_, _, config) = rustcode::config::load_config_for_workspace(workspace);
            if config.mcp_servers.is_empty() {
                println!("No MCP servers configured. Add one with `rustcode mcp add`.");
            }
            for server in &config.mcp_servers {
                println!("{}", crate::cli::mcp_server_summary(server));
            }
        }
        crate::cli::McpCommands::Remove { name } => {
            rustcode::config::remove_mcp_server(name)?;
            println!("Removed MCP server: {name}");
            if active(name) {
                println!(
                    "Note: a project .rustcode/config.toml still defines '{name}' for this workspace."
                );
            }
        }
    }
    Ok(())
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = crate::cli::Cli::parse();
    if run_daemon_or_cron_command(&cli_args).await? {
        return Ok(());
    }
    if let Some(crate::cli::Commands::Serve {
        port,
        bind,
        token,
        allow_remote,
    }) = cli_args.command.as_ref()
    {
        let workspace = std::env::current_dir()?;
        rustcode::serve::run(bind, *port, token.clone(), *allow_remote, workspace).await?;
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
        match command {
            Some(crate::cli::SessionCommands::Migrate { dry_run }) => {
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
            // `rustcode sessions` with no subcommand lists sessions, scoped
            // to the current workspace by default (`list --all` shows every
            // workspace with its path per entry).
            Some(crate::cli::SessionCommands::List { all }) => {
                run_sessions_list(*all || cli_args.all);
            }
            None => {
                run_sessions_list(cli_args.all);
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
        report,
        baseline,
        rounds,
        calls,
        recoveries,
        completed,
    }) = cli_args.command.as_ref()
    {
        if let Some(path) = report {
            let measured: rustcode::benchmark::TurnPerformance =
                serde_json::from_slice(&std::fs::read(path)?)?;
            println!("{}", measured.report());
            if let Some(path) = baseline {
                let original = serde_json::from_slice(&std::fs::read(path)?)?;
                println!(
                    "{}",
                    rustcode::benchmark::compare_reports(&original, &measured)
                );
            }
            return Ok(());
        }
        let stats = rustcode::benchmark::TurnStats {
            rounds: *rounds,
            tool_calls: *calls,
            recoveries: *recoveries,
            completed: *completed,
        };
        println!("{}", rustcode::benchmark::format_report(stats));
        return Ok(());
    }

    if let Some(crate::cli::Commands::Mcp { command }) = cli_args.command.as_ref() {
        if let Err(error) = run_mcp_command(command, &std::env::current_dir()?) {
            eprintln!("rustcode mcp: {error}");
            std::process::exit(1);
        }
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
            let report = rustcode::raw_cli::run_raw_cli_loop(
                &prompt,
                model_override.as_deref(),
                max_iters,
                cli_args.yolo,
            )
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
            rustcode::raw_cli::run_raw_cli(&prompt, model_override.as_deref(), cli_args.yolo)
                .await?;
        }
        rustcode::config::flush_history();
        return Ok(());
    }

    return run_interactive(cli_args, model_override).await;
}
/// Outcome of resolving `--resume`/`--continue` at startup. The explicit-id
/// failure is returned as data rather than exiting here so the caller can own
/// the stderr message and the exit code, and so tests can assert both without
/// spawning the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupResume {
    /// Startup may continue into the TUI: either no restore flag was given,
    /// or the request was satisfied — or, for a bare flag with nothing to
    /// restore, reported as a transcript message instead of a hard failure.
    Continue,
    /// An explicit `--resume <id>`/`--continue <id>` named a session that
    /// cannot be restored. The caller reports `error` on stderr and exits
    /// non-zero rather than opening a TUI with no session behind it.
    Failed { id: String, error: String },
}

/// Bare `--resume`/`--continue` restore the most recent session in the
/// current workspace; `<id>` restores that exact session from any workspace
/// and adopts its cwd. `--all` lifts scoping for the bare flags only.
fn resolve_startup_resume(
    state: &mut AppState,
    resume_raw: Option<&str>,
    continue_raw: Option<&str>,
    all: bool,
) -> StartupResume {
    let Some(raw) = continue_raw.or(resume_raw) else {
        return StartupResume::Continue;
    };
    let wants_continue = continue_raw.is_some();
    let id = raw.trim();
    let explicit = !id.is_empty();
    let action = if explicit {
        rustcode::app::SessionAction::Id(id.to_owned())
    } else {
        rustcode::app::SessionAction::Latest
    };
    let result = if all && !explicit {
        resume_latest_unscoped(state)
    } else {
        rustcode::app::session_controller::SessionController::default().resume(state, action)
    };
    match result {
        Ok(_) => {
            if wants_continue {
                let queued = rustcode::app::actions::queue_restored_segment(state);
                if !queued {
                    state.history.push(rustcode::app::ChatMessage::new(
                        "system",
                        "No pending session work is available to continue.",
                    ));
                }
            }
            StartupResume::Continue
        }
        Err(error) => {
            // Only an explicit id is fatal. A bare flag with no match keeps
            // the pre-existing behavior of reporting the reason in the
            // transcript and starting fresh.
            if explicit {
                return StartupResume::Failed {
                    id: id.to_owned(),
                    error: error.to_string(),
                };
            }
            let message = if matches!(
                &error,
                rustcode::app::session_controller::SessionError::NoSessionToResume
            ) {
                "No previous session to resume.".to_owned()
            } else {
                error.to_string()
            };
            state
                .history
                .push(rustcode::app::ChatMessage::new("system", message));
            StartupResume::Continue
        }
    }
}

/// Bare `--resume` with `--all`: restore the most recent session across
/// every workspace, bypassing the default current-workspace scoping.
fn resume_latest_unscoped(
    state: &mut AppState,
) -> Result<
    rustcode::app::session_controller::SessionTransition,
    rustcode::app::session_controller::SessionError,
> {
    if !rustcode::config::session_has_content(&state.history)
        && let Some(live) = rustcode::config::live_session_meta()
    {
        if rustcode::app::actions::load_session_into(state, &live) {
            return Ok(
                rustcode::app::session_controller::SessionTransition::Resumed {
                    session_id: state.active_session_id.clone(),
                },
            );
        }
    }
    let meta = rustcode::config::latest_resumable_session_meta_in_scope(
        &rustcode::config::SessionScope::All,
    )
    .ok_or(rustcode::app::session_controller::SessionError::NoSessionToResume)?;
    let session_id =
        rustcode::app::session_controller::session_id_from_meta(&meta).ok_or_else(|| {
            rustcode::app::session_controller::SessionError::SessionNotFound(meta.title.clone())
        })?;
    if !rustcode::app::actions::load_session_into(state, &meta) {
        return Err(rustcode::app::session_controller::SessionError::SessionNotFound(session_id));
    }
    Ok(
        rustcode::app::session_controller::SessionTransition::Resumed {
            session_id: state.active_session_id.clone(),
        },
    )
}

/// `rustcode sessions list`: print saved sessions scoped to the current
/// workspace by default; `--all` prints every workspace with its path.
fn run_sessions_list(show_all: bool) {
    const LIMIT: usize = 50;
    // `--all` lists every workspace; the default scope is the store's shared
    // parent/child rule against the current directory.
    let scope = if show_all {
        rustcode::config::SessionScope::All
    } else {
        rustcode::config::SessionScope::for_current_dir()
    };
    let (sessions, truncated) = rustcode::config::list_sessions_in_scope(LIMIT, &scope);
    if sessions.is_empty() {
        println!("No saved sessions found.");
        return;
    }
    for meta in &sessions {
        let id = rustcode::config::session_id_from_path(&meta.path).unwrap_or_default();
        let workspace = meta
            .workspace_cwd
            .as_deref()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_else(|| "no workspace recorded".to_owned());
        if show_all {
            println!(
                "{id}  {}  {} msgs  {workspace}  {}",
                meta.title, meta.message_count, meta.when
            );
        } else {
            println!(
                "{id}  {}  {} msgs  {}",
                meta.title, meta.message_count, meta.when
            );
        }
    }
    if truncated {
        eprintln!("Showing the {LIMIT} most recent sessions; use --all to see every workspace.");
    }
}
async fn run_interactive(
    cli_args: crate::cli::Cli,
    model_override: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    rustcode::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    let fullscreen_requested = cli_args.fullscreen || app_state_struct.config.fullscreen;
    let local_terminal = crate::terminal_probe::probe().supports_alternate_screen();
    let fullscreen = fullscreen_requested && local_terminal;
    if cli_args.yolo {
        app_state_struct.auto_confirm = true;
    }
    // Resolve `--resume`/`--continue` before the terminal is taken over. A
    // named session that cannot be restored then fails on stderr with a
    // non-zero exit, instead of leaving the shell in raw mode and a
    // half-initialized TUI behind — and the failure needs no TTY at all.
    if let StartupResume::Failed { id, error } = resolve_startup_resume(
        &mut app_state_struct,
        cli_args.resume.as_deref(),
        cli_args.continue_session.as_deref(),
        cli_args.all,
    ) {
        eprintln!("rustcode: cannot resume session '{id}': {error}");
        std::process::exit(1);
    }
    rustcode::clipboard::warm_image_paste();
    let terminal_runtime = TerminalRuntime::start(fullscreen, local_terminal)?;
    // Themes are a terminal-UI concern: shared state no longer applies them.
    // The interactive runtime seeds the palette once before the first frame;
    // every render re-applies it from `state.config().theme`.
    crate::ui::theme::ensure_themes_dir();
    crate::ui::theme::set_active_theme(&app_state_struct.config.theme);
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

/// SGR foreground sequence for a theme color, written outside ratatui once the
/// terminal is restored. Colors without an exact code keep the terminal default.
fn ansi_foreground(color: ratatui::style::Color) -> String {
    match color {
        ratatui::style::Color::Rgb(red, green, blue) => format!("\x1b[38;2;{red};{green};{blue}m"),
        ratatui::style::Color::Indexed(index) => format!("\x1b[38;5;{index}m"),
        _ => "\x1b[39m".to_owned(),
    }
}

/// Exit handoff written to an injectable sink. `color` and `wide` are
/// resolved by the caller — production asks the real stdout and terminal —
/// so a test can capture the transcript without a TTY.
fn write_exit_summary(out: &mut dyn Write, summary: &ExitSummary, color: bool, wide: bool) {
    let wordmark = if wide {
        crate::ui::RUSTCODE_WORDMARK.lines().collect::<Vec<_>>()
    } else {
        vec!["RustCode"]
    };
    for line in wordmark {
        if color {
            let purple = line
                .chars()
                .take(crate::ui::RUSTCODE_WORDMARK_SPLIT)
                .collect::<String>();
            let white = line
                .chars()
                .skip(crate::ui::RUSTCODE_WORDMARK_SPLIT)
                .collect::<String>();
            // Follow the active theme rather than a fixed purple and white.
            let _ = writeln!(
                out,
                "{}{purple}{}{white}\x1b[0m",
                ansi_foreground(crate::ui::COLOR_PRIMARY()),
                ansi_foreground(crate::ui::COLOR_TEXT()),
            );
        } else {
            let _ = writeln!(out, "{line}");
        }
    }
    let _ = writeln!(out);
    if let Some(usage) = summary.usage_line() {
        let _ = writeln!(out, "{usage}");
    }
    if !summary.session_id.is_empty() {
        let _ = writeln!(out, "Session   {}", summary.session_id);
        let _ = writeln!(out, "Continue  rustcode --resume {}", summary.session_id);
    }
    if !summary.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Warnings:");
        for warning in &summary.warnings {
            let _ = writeln!(out, "  - {warning}");
        }
    }
}

/// Printed after restoring the terminal and erasing the session projection
/// (#1544), matching Codex's compact usage and resume handoff. The session id
/// is the recovery path for the erased conversation.
fn print_exit_summary(summary: &ExitSummary) {
    use std::io::IsTerminal;

    let mut out = std::io::stdout();
    let color = out.is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let wide = crossterm::terminal::size().is_ok_and(|(width, _)| width >= 50);
    write_exit_summary(&mut out, summary, color, wide);
}

#[cfg(test)]
mod tests {
    use super::{
        ExitSummary, MAX_SCROLLBACK_BLOCK_LINES, StartupResume, cap_scrollback_block,
        format_number, resolve_startup_resume, should_clear_mutable_viewport_before_history,
        write_exit_summary,
    };
    use rustcode::app::AppState;

    /// Never a real session id, so the lookup fails the same way on every run.
    const UNKNOWN_ID: &str = "ffffffff-ffff-7fff-8fff-ffffffffffff";

    fn system_messages(state: &AppState) -> Vec<&str> {
        state
            .history
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.as_str())
            .collect()
    }

    #[test]
    fn unknown_resume_id_is_a_hard_failure_not_a_transcript_message() {
        let mut state = AppState::new();
        let outcome = resolve_startup_resume(&mut state, Some(UNKNOWN_ID), None, false);
        assert_eq!(
            outcome,
            StartupResume::Failed {
                id: UNKNOWN_ID.to_owned(),
                error: format!("session not found: {UNKNOWN_ID}"),
            }
        );
        // A regression that downgraded this to an in-TUI system message is
        // exactly what the binary-level `tests/cli_resume.rs` guards, so the
        // transcript must stay untouched here too.
        assert_eq!(system_messages(&state), Vec::<&str>::new());
    }

    #[test]
    fn malformed_resume_id_reports_the_invalid_id() {
        let mut state = AppState::new();
        let outcome = resolve_startup_resume(&mut state, Some("not a session id"), None, false);
        assert_eq!(
            outcome,
            StartupResume::Failed {
                id: "not a session id".to_owned(),
                error: "invalid session id: not a session id".to_owned(),
            }
        );
        assert_eq!(system_messages(&state), Vec::<&str>::new());
    }

    #[test]
    fn unknown_continue_id_fails_the_same_way_as_resume() {
        let mut state = AppState::new();
        assert_eq!(
            resolve_startup_resume(&mut state, None, Some(UNKNOWN_ID), false),
            StartupResume::Failed {
                id: UNKNOWN_ID.to_owned(),
                error: format!("session not found: {UNKNOWN_ID}"),
            }
        );
        assert_eq!(system_messages(&state), Vec::<&str>::new());
    }

    #[test]
    fn resume_is_skipped_when_no_flag_is_given() {
        let mut state = AppState::new();
        assert_eq!(
            resolve_startup_resume(&mut state, None, None, false),
            StartupResume::Continue
        );
        assert_eq!(system_messages(&state), Vec::<&str>::new());
    }

    #[test]
    fn exit_handoff_names_the_session_that_ran() {
        // The id that just ran, taken from live state rather than a literal:
        // the handoff is only useful if it repeats the real session id.
        let mut state = AppState::new();
        state.active_session_id = "01a0f0c9-8ebe-7000-9e19-31da83800034".to_owned();
        let summary = ExitSummary::from_state(&state);

        let mut sink: Vec<u8> = Vec::new();
        write_exit_summary(&mut sink, &summary, false, false);
        let rendered = String::from_utf8(sink).expect("handoff is utf-8");

        assert!(
            rendered.contains("Session   01a0f0c9-8ebe-7000-9e19-31da83800034"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Continue  rustcode --resume 01a0f0c9-8ebe-7000-9e19-31da83800034"),
            "{rendered}"
        );
        assert!(
            !rendered.contains('\x1b'),
            "an uncolored sink must stay free of escape codes: {rendered:?}"
        );
    }

    #[test]
    fn exit_handoff_omits_the_resume_command_without_a_session_id() {
        let summary = ExitSummary {
            prompt_tokens: 0,
            cached_tokens: 0,
            completion_tokens: 0,
            reasoning_tokens: 0,
            session_id: String::new(),
            composer_y: None,
            print_handoff: true,
            warnings: Vec::new(),
        };
        let mut sink: Vec<u8> = Vec::new();
        write_exit_summary(&mut sink, &summary, false, false);
        let rendered = String::from_utf8(sink).expect("handoff is utf-8");
        assert!(!rendered.contains("rustcode --resume"), "{rendered}");
    }

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
    fn exit_wordmark_uses_the_active_theme_colors() {
        let summary = ExitSummary {
            prompt_tokens: 0,
            cached_tokens: 0,
            completion_tokens: 0,
            reasoning_tokens: 0,
            session_id: String::new(),
            composer_y: None,
            print_handoff: true,
            warnings: Vec::new(),
        };
        let mut sink = Vec::new();
        write_exit_summary(&mut sink, &summary, true, false);
        let printed = String::from_utf8(sink).unwrap();
        assert!(printed.contains(&super::ansi_foreground(crate::ui::COLOR_PRIMARY())));
        assert!(printed.contains(&super::ansi_foreground(crate::ui::COLOR_TEXT())));
        assert_eq!(
            super::ansi_foreground(ratatui::style::Color::Rgb(1, 2, 3)),
            "\x1b[38;2;1;2;3m"
        );
    }

    #[test]
    fn exit_wordmark_is_bundled_for_the_terminal_handoff() {
        let lines: Vec<_> = crate::ui::RUSTCODE_WORDMARK.lines().collect();
        assert_eq!(
            lines,
            [
                "                  ▄                   █",
                "▄▀▀▀ █   █ ▄▀▀▀▀ ▀█▀▀ ▄▀▀▀▀ ▄▀▀▀▄ ▄▀▀▀█ ▄▀▀▀▄",
                "█    █   █  ▀▀▀▄  █   █     █   █ █   █ █▀▀▀▀",
                "▀     ▀▀▀  ▀▀▀▀    ▀▀  ▀▀▀▀  ▀▀▀   ▀▀▀▀  ▀▀▀▀",
            ]
        );
        let banner = crate::ui::build_claude_startup_banner(
            &rustcode::controller::RenderState::new(),
            100,
            30,
        );
        // The wordmark belongs to the exit handoff only; the welcome banner
        // no longer repeats it (#1772).
        let rendered = banner.iter().map(ToString::to_string).collect::<Vec<_>>();
        for line in lines {
            assert!(!rendered.iter().any(|row| row.contains(line.trim_end())));
        }
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
    fn committed_scrollback_blocks_are_capped_independently_of_the_engine() {
        // #1593: the engine bounds a payload at 1000 lines; the commit path
        // needs its own bound so a render regression cannot make scrollback
        // grow without limit.
        let block = (0..MAX_SCROLLBACK_BLOCK_LINES + 50)
            .map(|index| ratatui::text::Line::from(format!("row {index}")))
            .collect();
        let capped = cap_scrollback_block(block);

        assert_eq!(capped.len(), MAX_SCROLLBACK_BLOCK_LINES + 1);
        assert!(
            capped
                .last()
                .is_some_and(|line| line.to_string().contains("+50 more transcript lines")),
            "{:?}",
            capped.last()
        );

        let short = vec![ratatui::text::Line::from("only row")];
        assert_eq!(cap_scrollback_block(short).len(), 1);
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
