//! A session owner with the same turn, question, approval, and remote pump as the TUI.
use super::*;
use std::path::Path;

impl AppRuntime {
    pub(crate) async fn run_detached_session(
        config: &Path,
        session_id: &str,
    ) -> Result<(), Box<dyn Error>> {
        if session_id.len() != 32 || !session_id.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid mobile session identifier".into());
        }
        let owners = config.join("remote").join("owners");
        let (_lease, mut state) =
            rustcode::remote_gateway::workspace::owner_state(config, session_id)?;
        state.remote_command = Some(rustcode::remote::owner::SharingCommand::Enable);
        let mut runtime = Self::detached(state);
        let subscription = rustcode::tools::background_task_manager().subscribe_session(session_id);
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        loop {
            tokio::select! { _=interval.tick()=>{}, _=tokio::signal::ctrl_c()=>break, _=terminate.recv()=>break }
            // Removing a test/config owner directory is an explicit lease withdrawal.
            if !owners.exists() {
                break;
            }
            while let Ok(event) = subscription.try_recv() {
                apply_background_task_event(&runtime.app_state, event).await;
            }
            runtime.run_remote_iteration().await;
        }
        runtime.current_cancel_token.cancel();
        let state = runtime.app_state.lock().await;
        rustcode::config::save_session_history(&state.active_session_id, &state.history);
        Ok(())
    }
}
