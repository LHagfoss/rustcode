#[macro_use]
mod logger;
mod acp;
mod app;
mod cli;
mod clipboard;
mod config;
mod context;
mod inline_terminal;
mod mcp;
mod memory;
mod network;
mod notifications;
mod raw_cli;
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

fn background_task_history_message(
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
) -> ChatMessage {
    let prefix = format!("background_task: Task {task_id} completed. Output:\n");
    crate::network::bounded_tool_result_history_message(
        crate::network::ToolResult {
            tool_name: "background_task".to_string(),
            content: output.content,
            diff: None,
            file_preview: None,
            metadata: crate::network::ToolResultMetadata {
                success: output.success,
                exit_code: output.exit_code,
                truncated: output.truncated,
                replayed: output.replayed,
                error_kind: output.error_kind,
                retryable: output.retryable,
                ..Default::default()
            },
        },
        &prefix,
        None,
    )
}

fn queue_background_wakeup(state: &mut AppState, task_id: &str) {
    state
        .pending_queue
        .push(format!("__task_wakeup__:{task_id}"));
    state.request_redraw();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cheap, once-per-process check: rotate debug.log out of the way if a
    // prior session let it grow past the size cap, instead of letting every
    // subsequent write add to an already-huge file.
    crate::logger::rotate_if_oversized();

    let cli_args = cli::Cli::parse();
    let model_override = cli_args.model.clone();

    if let Some(cli::Commands::Sync { command }) = cli_args.command {
        match command {
            Some(cli::SyncCommands::Pull) => {
                println!("📥 [sync] Pulling latest config and skills from remote origin...");
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync pull failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            Some(cli::SyncCommands::Push) => {
                println!("💾 [sync] Staging and pushing config files...");
                if let Err(e) = config::sync_config_push() {
                    eprintln!("Sync push failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            Some(cli::SyncCommands::Init { remote_url }) => {
                if let Err(e) = config::init_sync_repo(&remote_url) {
                    eprintln!("Error initializing sync repo: {e}");
                    std::process::exit(1);
                }
                println!(
                    "Sync repository setup complete! You can now run `rustcode sync` anytime."
                );
            }
            None => {
                // Default behavior for `rustcode sync` (pull then push)
                println!("📥 [sync] Pulling latest config and skills from remote origin...");
                if let Err(e) = config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging and pushing config files...");
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

                let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let running_clone = std::sync::Arc::clone(&running);

                let handle = std::thread::spawn(move || {
                    let mut idx = 0;
                    while running_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        print!("\r  {} Running Homebrew update & upgrade...", spinner[idx % spinner.len()]);
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        std::thread::sleep(std::time::Duration::from_millis(80));
                        idx += 1;
                    }
                    print!("\r\x1b[2K");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                });

                let upgrade_res = tokio::task::spawn_blocking(crate::update::run_brew_upgrade).await;
                running.store(false, std::sync::atomic::Ordering::Relaxed);
                let _ = handle.join();

                match upgrade_res {
                    Ok(Ok(())) => {
                        println!(
                            "🎉 Successfully updated rustcode to v{}!",
                            crate::update::format_version(latest)
                        );
                    }
                    Ok(Err(error)) => {
                        eprintln!("Update failed: {error}");
                        std::process::exit(1);
                    }
                    Err(error) => {
                        eprintln!("Update task error: {error}");
                        std::process::exit(1);
                    }
                }
            }
        }
        return Ok(());
    }

    if cli_args.acp {
        crate::acp::run_acp(cli_args.yolo).await?;
        crate::config::flush_history();
        return Ok(());
    }

    if let Some(prompt) = cli_args.prompt {
        raw_cli::run_raw_cli(&prompt, model_override.as_deref()).await?;
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
                crate::update::UpdateState::Available(latest)
            }
            Err(_) => crate::update::UpdateState::Failed,
        };
        state.request_redraw();
    });
    // Register the background task wakeup callback
    let state_cb = Arc::clone(&app_state);
    let handle = tokio::runtime::Handle::current();
    crate::tools::register_wakeup_callback(move |session_id, task_id, output| {
        let state_clone = Arc::clone(&state_cb);
        let handle_clone = handle.clone();
        handle_clone.spawn(async move {
            let mut s = state_clone.lock().await;
            if s.active_session_id == session_id {
                // Background output can be huge (long-running servers dump MBs of
                // logs). Head+tail truncate it like any other tool result so it
                // doesn't bloat context and the scroll buffer.
                s.history
                    .push(background_task_history_message(&task_id, output));
                crate::config::save_session_history(&session_id, &s.history);
                // Drive a fresh model turn when a background task completes in the active session
                // so the agent automatically receives the result and continues working.
                queue_background_wakeup(&mut s, &task_id);
                // Do not start an orchestrator here. This callback outlives individual
                // turns, so capturing their cancellation token leaves future wakeups
                // permanently cancelled after the first interrupt. The main event loop
                // observes this queued item and starts it with the current token instead.
            } else {
                let mut history = crate::config::load_session_history_direct(&session_id);
                history.push(background_task_history_message(&task_id, output));
                crate::config::save_session_history(&session_id, &history);
            }
        });
    });

    // Spawn startup initialization of enabled MCP servers.
    let mcp_servers = app_state.lock().await.config.mcp_servers.clone();
    tokio::spawn(async move {
        crate::mcp::start_enabled_servers(&mcp_servers, |name| async move {
            crate::mcp::start_server_by_name(&name).await
        })
        .await;
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
    print_exit_summary(&exit_summary);
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

        assert_eq!(state.pending_queue, ["__task_wakeup__:task_42"]);
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
                exit_code: Some(9),
                truncated: false,
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
            },
        );

        assert!(message.content.len() <= 50 * 1024);
        assert!(message.content.lines().count() <= 1000);
        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
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
                exit_code: Some(11),
                truncated: false,
                replayed: false,
                error_kind: Some(crate::tools::ToolErrorKind::CommandFailed),
                retryable: false,
            },
        );

        let metadata = message.tool_result.expect("background metadata");
        assert!(!metadata.success);
        assert_eq!(metadata.exit_code, Some(11));
        assert!(!metadata.truncated);
        assert_eq!(metadata.full_output_artifact, None);
    }
}
