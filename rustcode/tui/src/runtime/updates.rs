use crate::ui::TerminalRuntime;
use rustcode::app::{AppState, UpdateDecision};

pub(super) fn apply_update_decision(state: &mut AppState, decision: UpdateDecision) -> bool {
    let latest = match state.update_check {
        rustcode_core::update::UpdateState::Available(latest) => Some(latest),
        _ => None,
    };
    state.show_update_prompt = false;
    state.update_prompt_index = 0;
    if matches!(decision, UpdateDecision::SkipUntilNextVersion) {
        state.dismissed_update_version = latest;
    }
    state.request_redraw();
    matches!(decision, UpdateDecision::UpdateNow) && latest.is_some()
}

pub(super) async fn run_update_command(
    terminal_runtime: &mut TerminalRuntime,
    client: &reqwest::Client,
    expected_version: rustcode_core::update::Version,
) -> Result<(), String> {
    terminal_runtime
        .restore()
        .map_err(|error| format!("failed to restore the terminal before updating: {error}"))?;
    terminal_runtime
        .terminal()
        .clear()
        .map_err(|error| format!("failed to clear the TUI before updating: {error}"))?;
    rustcode::update::run_update(client, expected_version).await
}
