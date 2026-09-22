#[macro_use]
mod logger;
mod acp;
mod app;
mod atomic_file;
mod benchmark;
mod cli;
mod clipboard;
mod config;
mod context;
mod discord_rpc;
mod doctor;
mod inline_terminal;
#[path = "laya.rs"]
pub(crate) mod laya;
mod mcp;
mod memory;
mod network;
mod notifications;
mod paste;
mod platform;
mod raw_cli;
mod shell_env;
mod skills;
mod symbols;
mod tools;
mod ui;
mod update;

use crate::app::runtime::AppRuntime;
use crate::app::{AppState, ChatMessage};
use crate::ui::TerminalRuntime;
use clap::Parser;
use ratatui::{
    backend::Backend,
    widgets::{Paragraph, Widget, Wrap},
};
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

pub(crate) fn background_task_history_message(
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
) -> ChatMessage {
    background_task_history_message_with_call_id(task_id, output, None)
}

pub(crate) fn background_task_history_message_with_call_id(
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
    call_id: Option<String>,
) -> ChatMessage {
    let command = output
        .command
        .as_deref()
        .map(|command| format!(" Command: {command}."))
        .unwrap_or_default();
    let prefix = format!("background_task: Task {task_id} completed.{command} Output:\n");
    crate::network::bounded_tool_result_history_message(
        crate::network::ToolResult {
            tool_name: "background_task".to_string(),
            content: output.content,
            diff: None,
            file_preview: None,
            metadata: crate::network::ToolResultMetadata {
                success: output.success,
                exit_code: output.exit_code,
                command: output.command,
                truncated: output.truncated,
                completeness: output.completeness,
                replayed: output.replayed,
                error_kind: output.error_kind,
                retryable: output.retryable,
                command_status: output.command_status,
                ..Default::default()
            },
        },
        &prefix,
        call_id,
    )
}

/// A background task completion withheld while a turn is in flight, so it
/// joins history at the next turn boundary instead of derailing the
/// current turn's context mid-stream.
pub(crate) struct PendingBackgroundOutput {
    pub task_id: String,
    pub output: crate::tools::ToolExecutionOutput,
}

pub(crate) fn queue_background_wakeup(state: &mut AppState, task_id: &str) {
    if state.background_wakeup_ids.insert(task_id.to_string()) {
        state
            .pending_queue
            .push(format!("__task_wakeup__:{task_id}"));
    }
    state.request_redraw();
}

/// Move withheld background completions into history at a turn boundary.
/// Returns how many were flushed.
pub(crate) fn flush_pending_background_outputs(state: &mut AppState) -> usize {
    if state.pending_background_outputs.is_empty() {
        return 0;
    }
    let stashed: Vec<PendingBackgroundOutput> =
        std::mem::take(&mut state.pending_background_outputs);
    let count = stashed.len();
    for pending in stashed {
        state.history.push(background_task_history_message(
            &pending.task_id,
            pending.output,
        ));
    }
    let session_id = state.active_session_id.clone();
    crate::config::save_session_history(&session_id, &state.history);
    state.request_redraw();
    count
}

/// Import provider API keys from the user's login/interactive shell into this
/// process when they are missing here. Keys exported in `~/.zshrc` are the
/// classic miss: visible in every terminal, invisible to desktop/systemd/IDE
/// launches. Runs once at startup; the 3s probe timeout bounds the cost.
fn hydrate_shell_provider_keys() {
    // Cheap path first: read the workspace config to learn which env names
    // the user's profiles actually reference.
    let configured: Vec<String> = std::env::current_dir()
        .ok()
        .map(|workspace| crate::config::load_config_for_workspace(&workspace).2)
        .map(|config| {
            config
                .models
                .iter()
                .filter_map(|profile| profile.api_key_env_name())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    crate::shell_env::hydrate_provider_keys(&configured);
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Cheap, once-per-process check: rotate debug.log out of the way if a
    // prior session let it grow past the size cap, instead of letting every
    // subsequent write add to an already-huge file.
    crate::logger::rotate_if_oversized();
    // Issue #1226: silent mid-stream hangs left zero evidence. A panic hook
    // preserves the message + location in debug.log even when the process
    // dies without a crash report.
    crate::logger::install_panic_hook();
    // Provider keys are often exported in `~/.zshrc` (interactive-only) and
    // invisible to GUI/systemd/IDE launches. Hydrate missing keys from the
    // login shell once at startup so profiles, MCP servers, and tool shells
    // all see the same values the user's terminal sees.
    hydrate_shell_provider_keys();

    let cli_args = cli::Cli::parse();
    let model_override = cli_args.model.clone();

    if cli_args.init || matches!(cli_args.command.as_ref(), Some(cli::Commands::Init)) {
        let workspace = std::env::current_dir()?;
        match config::init_project_config(&workspace) {
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

    if let Some(cli::Commands::Sessions { command }) = cli_args.command.as_ref() {
        if let Some(cli::SessionCommands::Migrate { dry_run }) = command {
            let Some(report) = config::migrate_legacy_sessions(*dry_run) else {
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

    if let Some(cli::Commands::Doctor { fix }) = cli_args.command.as_ref() {
        let code = crate::doctor::run_doctor(*fix);
        if code != 0 {
            std::process::exit(code);
        }
        return Ok(());
    }

    if let Some(cli::Commands::Bench {
        rounds,
        calls,
        recoveries,
        completed,
    }) = cli_args.command.as_ref()
    {
        let stats = crate::benchmark::TurnStats {
            rounds: *rounds,
            tool_calls: *calls,
            recoveries: *recoveries,
            completed: *completed,
        };
        println!("{}", crate::benchmark::format_report(stats));
        return Ok(());
    }

    if let Some(cli::Commands::Discord {
        setup,
        status: _status,
        enable,
        disable,
    }) = cli_args.command.as_ref()
    {
        let workspace = std::env::current_dir()?;
        let (_, _, mut config) = crate::config::load_config_for_workspace(&workspace);
        if *setup || *enable || *disable {
            config.discord_rpc_enabled = !*disable;
            crate::config::save_entire_config(&config);
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
            let socket = crate::discord_rpc::ipc_socket_detected();
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
            if let Some(path) = crate::config::get_config_dir() {
                println!("Config directory: {}", path.display());
            }
        }
        return Ok(());
    }

    if let Some(cli::Commands::Laya { command }) = cli_args.command.as_ref() {
        let workspace = std::env::current_dir()?;
        let (_, _, config) = crate::config::load_config_for_workspace(&workspace);
        match command {
            cli::LayaCommand::Status => {
                println!("{}", crate::laya::format_status(&config.laya));
            }
            cli::LayaCommand::Enable { mode } => {
                if let Err(error) = crate::config::save_laya_mode_for_workspace(&workspace, *mode) {
                    eprintln!("Laya configuration update failed: {error}");
                    std::process::exit(1);
                }
                println!("Laya mode set to {mode}.");
            }
            cli::LayaCommand::Disable => {
                if let Err(error) = crate::config::save_laya_mode_for_workspace(
                    &workspace,
                    crate::laya::LayaMode::Off,
                ) {
                    eprintln!("Laya configuration update failed: {error}");
                    std::process::exit(1);
                }
                println!("Laya disabled.");
            }
        }
        return Ok(());
    }

    if let Some(cli::Commands::Sync {
        command,
        pull,
        push,
    }) = cli_args.command
    {
        let action = match cli::resolve_sync_action(command, pull, push) {
            Ok(action) => action,
            Err(error) => {
                eprintln!("Sync argument error: {error}");
                std::process::exit(2);
            }
        };

        match action {
            cli::SyncAction::Pull => {
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync pull failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            cli::SyncAction::Push => {
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = config::sync_config_push() {
                    eprintln!("Sync push failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            cli::SyncAction::Init(remote_url) => {
                if let Err(e) = config::init_sync_repo(&remote_url) {
                    eprintln!("Error initializing sync repo: {e}");
                    std::process::exit(1);
                }
                println!(
                    "Sync repository setup complete! You can now run `rustcode sync` anytime."
                );
            }
            cli::SyncAction::PullThenPush => {
                // Default behavior for `rustcode sync` (pull then push)
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = config::sync_config_push() {
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
        let check = match crate::update::check_for_update(&client).await {
            Ok(check) => check,
            Err(error) => {
                eprintln!("Update check failed: {error}");
                std::process::exit(1);
            }
        };

        match check {
            crate::update::UpdateCheck::UpToDate { current, latest } => {
                println!(
                    "No new release. rustcode v{} is up to date (latest: v{}).",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                );
            }
            crate::update::UpdateCheck::Available { current, latest } => {
                println!(
                    "Found new release: v{} → v{}, updating now...",
                    crate::update::format_version(current),
                    crate::update::format_version(latest)
                );
                match crate::update::run_update(&client, latest).await {
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

    let acp_subcommand = matches!(cli_args.command.as_ref(), Some(cli::Commands::Acp));
    if cli_args.acp || acp_subcommand {
        crate::acp::run_acp(cli_args.yolo).await?;
        crate::config::flush_history();
        return Ok(());
    }

    if let Some(prompt) = cli_args.prompt {
        if let Some(max_iters) = cli_args.loop_count {
            let report =
                raw_cli::run_raw_cli_loop(&prompt, model_override.as_deref(), max_iters).await?;
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
            raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
        }
        crate::config::flush_history();
        return Ok(());
    }

    let terminal_runtime = TerminalRuntime::start()?;

    crate::config::archive_live_history();

    let mut app_state_struct = AppState::new();
    if cli_args.yolo {
        app_state_struct.auto_confirm = true;
    }
    if cli_args.resume || cli_args.continue_session {
        if let Err(error) = crate::app::session_controller::SessionController::default()
            .resume(&mut app_state_struct, crate::app::SessionAction::Latest)
        {
            let message = if matches!(
                &error,
                crate::app::session_controller::SessionError::NoSessionToResume
            ) {
                "No previous session to resume.".to_owned()
            } else {
                error.to_string()
            };
            app_state_struct
                .history
                .push(crate::app::ChatMessage::new("system", message));
        } else if cli_args.continue_session {
            let queued = crate::app::actions::queue_restored_segment(&mut app_state_struct);
            if !queued {
                app_state_struct.history.push(crate::app::ChatMessage::new(
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
        state.update_check = crate::update::UpdateState::Checking;
    }
    let update_state = Arc::clone(&app_state);
    let update_client = client.clone();
    tokio::spawn(async move {
        let result = crate::update::check_for_update(&update_client).await;
        let mut state = update_state.lock().await;
        state.update_check = match result {
            Ok(crate::update::UpdateCheck::UpToDate { latest, .. }) => {
                crate::update::UpdateState::UpToDate(latest)
            }
            Ok(crate::update::UpdateCheck::Available { latest, .. }) => {
                if state.dismissed_update_version != Some(latest) {
                    state.show_update_prompt = true;
                    state.update_prompt_index = 0;
                }
                crate::update::UpdateState::Available(latest)
            }
            Err(_) => crate::update::UpdateState::Failed,
        };
        state.request_redraw();
    });
    // Spawn startup initialization of enabled MCP servers.
    let mcp_servers = app_state.lock().await.config.mcp_servers.clone();
    let mcp_state = Arc::clone(&app_state);
    tokio::spawn(async move {
        let warnings = crate::mcp::start_enabled_servers(&mcp_servers, |name| async move {
            crate::mcp::start_server_by_name(&name).await
        })
        .await;
        if !warnings.is_empty() {
            let mut state = mcp_state.lock().await;
            state.exit_warnings.extend(warnings);
        }
    });

    crate::app::spawn_context_window_detection(Arc::clone(&app_state), client.clone());

    {
        let state_quota = Arc::clone(&app_state);
        let client_quota = client.clone();
        tokio::spawn(async move {
            crate::network::fetch_model_quota(&client_quota, &state_quota).await;
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
mod draw_loop_tests {
    use super::{
        ExitSummary, background_task_history_message, format_number, queue_background_wakeup,
        should_clear_mutable_viewport_before_history,
    };

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
        let mut state = crate::app::AppState::new();
        state.record_warning("[mcp] timed out starting server test after 10.0s; continuing");
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

    #[test]
    fn background_wakeup_waits_for_main_loop_to_use_current_cancel_token() {
        let mut state = crate::app::AppState::new();
        state.orchestrator_running = false;
        state.redraw_requested = false;

        queue_background_wakeup(&mut state, "task_42");
        queue_background_wakeup(&mut state, "task_42");

        assert_eq!(
            state.pending_queue,
            ["__task_wakeup__:task_42"],
            "one terminal completion must resume the logical task once"
        );
        assert!(!state.orchestrator_running);
        assert!(state.take_redraw_request());
    }

    #[test]
    fn background_history_preserves_bounded_recovery_metadata() {
        let raw = (1..=2000)
            .map(|line| format!("background line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let message = background_task_history_message(
            "task_42",
            crate::tools::ToolExecutionOutput {
                content: raw.clone(),
                success: false,
                pending: false,
                command: Some("markdownlint --config .markdownlint.json README.md".to_string()),
                exit_code: Some(9),
                truncated: false,
                completeness: rustcode_core::ToolResultCompleteness::Complete,
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
                command_status: None,
            },
        );

        assert!(message.content.len() <= 50 * 1024);
        assert!(message.content.lines().count() <= 1000);
        assert!(
            message
                .content
                .contains("Command: markdownlint --config .markdownlint.json README.md")
        );
        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(
            metadata.command.as_deref(),
            Some("markdownlint --config .markdownlint.json README.md")
        );
        assert_eq!(metadata.exit_code, Some(9));
        assert!(metadata.truncated);
        let artifact = metadata
            .full_output_artifact
            .expect("bounded background output must retain its artifact");
        assert_eq!(
            std::fs::read_to_string(artifact).expect("artifact readable"),
            raw
        );
    }

    #[test]
    fn background_history_does_not_parse_spoofed_recovery_metadata() {
        let message = background_task_history_message(
            "task_43",
            crate::tools::ToolExecutionOutput {
                content: "exit code: 0\n[Output truncated:]\nFull output saved to: /tmp/spoof"
                    .to_string(),
                success: false,
                pending: false,
                command: None,
                exit_code: Some(11),
                truncated: false,
                completeness: rustcode_core::ToolResultCompleteness::Complete,
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
                command_status: None,
            },
        );

        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(metadata.exit_code, Some(11));
        assert!(!metadata.truncated);
        assert_eq!(metadata.full_output_artifact, None);
    }
}
