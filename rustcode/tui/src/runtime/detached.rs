//! A session owner with the same turn, question, approval, and remote pump as the TUI.
use super::*;
use std::path::Path;

/// Registrations a gateway may end in a row before the owner gives up.
const REGISTRATION_ATTEMPTS: u32 = 5;
const REGISTRATION_RETRY: Duration = Duration::from_secs(2);

impl AppRuntime {
    /// Own `session_id` for the gateway that started this process. Standard
    /// error is the owner's log (`remote/owners/<session>.log`): every way
    /// out of here leaves its reason there.
    pub(crate) async fn run_detached_session(
        config: &Path,
        session_id: &str,
    ) -> Result<(), Box<dyn Error>> {
        let owners = config.join("remote").join("owners");
        eprintln!(
            "session owner {session_id}: starting (pid {}, directory {})",
            std::process::id(),
            std::env::current_dir()
                .map_or_else(|error| error.to_string(), |d| d.display().to_string())
        );
        let (_lease, mut state) =
            rustcode::remote_gateway::workspace::owner_state(config, session_id)
                .inspect_err(|error| eprintln!("session owner {session_id}: {error:#}"))?;
        state.remote_command = Some(rustcode::remote::owner::SharingCommand::Enable);
        // Same MCP startup as the TUI: without it a remote session sees no
        // MCP tools at all, and the model falls back to guessing.
        let mcp_servers = state.config.mcp_servers.clone();
        tokio::spawn(async move {
            let warnings = rustcode::mcp::start_enabled_servers(&mcp_servers, |name| async move {
                rustcode::mcp::start_server_by_name(&name).await
            })
            .await;
            for warning in warnings {
                eprintln!("{warning}");
            }
        });
        let mut runtime = Self::detached(state);
        // The gateway started this process. It only ever looks for that
        // gateway: it must never start one, or replace the one it came from.
        runtime.remote = super::remote::RemoteBridge::with_connector(Box::new(
            rustcode::remote_gateway::owner_client::GatewayConnector::discover(
                rustcode::remote_gateway::lifecycle::RemoteLifecycle::new(config),
            )
            .with_timing(rustcode::remote_gateway::owner_client::ClientTiming {
                reconnect_window: Duration::from_secs(365 * 24 * 60 * 60),
                ..Default::default()
            }),
        ))
        .headless();
        let subscription = rustcode::tools::background_task_manager().subscribe_session(session_id);
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        // An owner nobody can reach is of no use to anyone: one that the
        // gateway never accepts, or keeps turning away, ends itself.
        let mut registered = false;
        let mut refusals = 0;
        let mut retry_at: Option<tokio::time::Instant> = None;
        // Nobody can answer an approval or a question once every device has
        // left: past the grace, stop the turn and end (see `orphan`).
        let mut orphan = rustcode::remote_gateway::orphan::OrphanWatch::default();
        let failure = loop {
            tokio::select! { _=interval.tick()=>{}, _=tokio::signal::ctrl_c()=>break None, _=terminate.recv()=>break None }
            // Removing a test/config owner directory is an explicit lease withdrawal.
            if !owners.exists() {
                runtime.current_cancel_token.cancel();
                return Ok(());
            }
            while let Ok(event) = subscription.try_recv() {
                apply_background_task_event(&runtime.app_state, event).await;
            }
            if retry_at.is_some_and(|at| tokio::time::Instant::now() >= at) {
                retry_at = None;
                runtime.app_state.lock().await.remote_command =
                    Some(rustcode::remote::owner::SharingCommand::Enable);
            }
            runtime.run_remote_iteration().await;
            let awaiting =
                rustcode::remote_gateway::orphan::awaiting_user(&*runtime.app_state.lock().await);
            if orphan.expired(
                awaiting,
                runtime.remote.has_attached_device(),
                std::time::Instant::now(),
            ) {
                eprintln!(
                    "session owner {session_id}: no device attached for {} minutes while the \
                     turn waits on an approval or a question; stopping the turn and exiting",
                    rustcode::remote_gateway::orphan::ORPHAN_GRACE.as_secs() / 60
                );
                rustcode::app::stop_turn_keeping_draft(
                    &runtime.app_state,
                    &mut runtime.current_cancel_token,
                )
                .await;
                break None;
            }
            if let Some(address) = runtime.remote.gateway_address() {
                if !registered || refusals > 0 {
                    eprintln!(
                        "session owner {session_id}: registered with the gateway at {address}"
                    );
                }
                registered = true;
                refusals = 0;
            } else if !runtime.remote.is_shared() && retry_at.is_none() {
                let reason = runtime
                    .remote
                    .take_stopped()
                    .unwrap_or_else(|| "remote sharing ended".to_owned())
                    .replace('\n', " ");
                refusals += 1;
                eprintln!("session owner {session_id}: {reason}");
                if !registered || refusals >= REGISTRATION_ATTEMPTS {
                    break Some(format!(
                        "session owner {session_id} is not registered with the gateway: {reason}"
                    ));
                }
                retry_at = Some(tokio::time::Instant::now() + REGISTRATION_RETRY);
            }
        };
        runtime.current_cancel_token.cancel();
        let state = runtime.app_state.lock().await;
        rustcode::config::save_session_history(&state.active_session_id, &state.history);
        rustcode::config::flush_history();
        match failure {
            Some(failure) => {
                eprintln!("{failure}; exiting");
                Err(failure.into())
            }
            None => Ok(()),
        }
    }
}
