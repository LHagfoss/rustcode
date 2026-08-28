use crate::app::{AppState, UpdateDecision};
use crate::ui::TerminalRuntime;

pub(super) fn apply_update_decision(state: &mut AppState, decision: UpdateDecision) -> bool {
    let latest = match state.update_check {
        crate::update::UpdateState::Available(latest) => Some(latest),
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
    expected_version: crate::update::Version,
) -> Result<(), String> {
    terminal_runtime
        .restore()
        .map_err(|error| format!("failed to restore the terminal before updating: {error}"))?;
    crate::update::run_update(client, expected_version).await
}
