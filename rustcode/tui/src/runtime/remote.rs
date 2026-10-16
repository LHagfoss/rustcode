//! The terminal's side of `/remote` (issue #1908): shares the session this
//! terminal is running over an [`OwnerLink`] and applies the commands that
//! arrive on it.
//!
//! The event loop runs [`RemoteBridge::tick`] once per iteration, after it has
//! applied that iteration's agent events. Everything remote therefore happens
//! on the loop's own task, between two pieces of terminal input: a remote
//! command and a key press are never applied concurrently. The link is only
//! ever used with `try_send`/`try_recv`, and never while the state is locked.

use super::*;
use rustcode::remote::owner::{
    GatewayLinkStatus, OwnerCommand, OwnerConnector, OwnerLink, OwnerLinkError, OwnerMessage,
    OwnerReply, SharingCommand, default_connector, next_registration_epoch,
};
use rustcode::remote::{
    OwnerFollowUp, ProjectionLimits, ReceiptState, RemoteError, RemoteErrorCode, RemoteRequest,
    RemoteResponse, SessionCloseReason, SessionPublisher, SessionRegistration,
    apply_session_mutation, read_session,
};
use tokio::sync::mpsc::error::{TryRecvError, TrySendError};

#[cfg(unix)]
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
#[cfg(unix)]
use rustcode::remote_gateway::owner_ipc::{GatewayFrame, OWNER_IPC_VERSION, OwnerFrame};

/// A standby owner link: this terminal announced its unshared session as
/// reachable, so a device can ask it to share. Presence only: no transcript
/// ever leaves the terminal until the user accepts with y and `/remote` runs.
/// Unix only: the owner socket does not exist on other platforms.
#[cfg(unix)]
struct Standby {
    to_task: tokio::sync::mpsc::Sender<OwnerFrame>,
    from_task: tokio::sync::mpsc::UnboundedReceiver<StandbyEvent>,
    session_id: String,
}

#[cfg(unix)]
enum StandbyEvent {
    ShareRequested {
        pending_id: u64,
        device_name: String,
    },
    Gone,
}

const PANEL_TITLE: &str = "Remote";

/// Remote commands applied in one loop iteration, so a burst from a device
/// cannot hold back the terminal's own input and rendering.
const COMMANDS_PER_TICK: usize = 8;

/// How long the bridge waits before announcing again after the gateway went
/// away. The link is never started when no gateway runs: announcing must not
/// bring one up behind the user's back. Unix only.
#[cfg(unix)]
const STANDBY_RETRY: std::time::Duration = std::time::Duration::from_secs(30);

/// How often the bridge re-reads whether remote access is switched on for the
/// machine. `/remote` in one terminal reaches the others within this time.
const SHARE_ALL_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);

/// How long the bridge waits before it tries to share the session again after
/// the gateway could not be reached.
const SHARE_ALL_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

struct Share {
    publisher: SessionPublisher,
    link: OwnerLink,
    /// What the gateway last reported; `None` until it accepts the
    /// registration.
    gateway: Option<GatewayLinkStatus>,
    /// `/remote` was entered and its pairing details are not on screen yet:
    /// they are asked for once the gateway is known to be there.
    pairing_due: bool,
}

/// What the loop lends the bridge for one iteration.
pub(super) struct RemotePump<'a> {
    pub(super) app_state: &'a Arc<Mutex<AppState>>,
    pub(super) client: &'a reqwest::Client,
    pub(super) cancel_token: &'a mut CancellationToken,
    pub(super) agent_ui_event_sender: &'a AgentUiEventSender,
    /// No terminal input is waiting in the loop's queue. Remote commands wait
    /// for this, so an answer the terminal already produced is applied before
    /// a remote one that arrived after it.
    pub(super) terminal_input_idle: bool,
    pub(super) needs_redraw: &'a mut bool,
}

pub(crate) struct RemoteBridge {
    connector: Box<dyn OwnerConnector>,
    share: Option<Share>,
    /// Announced-but-unshared presence for device-initiated share requests.
    /// Dropped the moment the session is shared, and re-announced on session
    /// switch. Never started when no gateway runs. Unix only: the owner
    /// socket does not exist on other platforms.
    #[cfg(unix)]
    standby: Option<Standby>,
    #[cfg(unix)]
    standby_retry_at: Option<std::time::Instant>,
    /// A turn's start was applied and its end not yet. Tracked while nothing
    /// is shared too, so a registration made mid-turn publishes that end.
    turn_stream_open: bool,
    /// Why sharing last ended or could not start, until someone asks.
    stopped: Option<String>,
    /// Nobody is looking at this owner: it never asks for pairing details,
    /// which would replace the challenge a terminal is showing.
    headless: bool,
    /// Whether remote access is switched on for the machine, as last read,
    /// and when. While it is on, the session on screen is shared without
    /// being asked.
    share_all: Option<(bool, std::time::Instant)>,
    /// Not before this: the next attempt to share the session by itself.
    share_all_retry_at: Option<std::time::Instant>,
}

impl RemoteBridge {
    pub(super) fn new() -> Self {
        Self::with_connector(default_connector())
    }

    pub(super) fn with_connector(connector: Box<dyn OwnerConnector>) -> Self {
        Self {
            connector,
            share: None,
            #[cfg(unix)]
            standby: None,
            #[cfg(unix)]
            standby_retry_at: None,
            turn_stream_open: false,
            stopped: None,
            headless: false,
            share_all: None,
            share_all_retry_at: None,
        }
    }

    /// Whether remote access is switched on for the machine. Read from disk
    /// at most every [`SHARE_ALL_RECHECK`]: this runs on every loop pass.
    fn share_all_enabled(&mut self) -> bool {
        if self.headless {
            return false;
        }
        let now = std::time::Instant::now();
        if let Some((enabled, read_at)) = self.share_all
            && now.duration_since(read_at) < SHARE_ALL_RECHECK
        {
            return enabled;
        }
        #[cfg(unix)]
        let enabled = self
            .connector
            .gateway_directory()
            .is_some_and(|directory| rustcode::remote_gateway::service::enabled(&directory));
        #[cfg(not(unix))]
        let enabled = false;
        self.share_all = Some((enabled, now));
        enabled
    }

    /// Share the session on screen because remote access is on, without
    /// showing anything: this happens at start and after every session
    /// change. A gateway that cannot be reached is tried again later.
    async fn share_by_itself(&mut self, app_state: &Arc<Mutex<AppState>>) {
        let now = std::time::Instant::now();
        if self.share_all_retry_at.is_some_and(|at| now < at) {
            return;
        }
        self.enable(app_state).await;
        match self.share.as_mut() {
            Some(share) => {
                share.pairing_due = false;
                self.share_all_retry_at = None;
                self.stopped = None;
            }
            None => self.share_all_retry_at = Some(now + SHARE_ALL_RETRY),
        }
    }

    /// For an owner without a terminal.
    pub(super) fn headless(mut self) -> Self {
        self.headless = true;
        self
    }

    /// Whether this session is registered or registering with a gateway.
    pub(super) fn is_shared(&self) -> bool {
        self.share.is_some()
    }

    /// The address of the gateway that accepted the registration, while it
    /// is reachable.
    pub(super) fn gateway_address(&self) -> Option<&str> {
        let gateway = self.share.as_ref()?.gateway.as_ref()?;
        gateway
            .connected
            .then_some(gateway.advertised_address.as_str())
    }

    /// Why sharing is off, once per time it stopped.
    pub(super) fn take_stopped(&mut self) -> Option<String> {
        self.stopped.take()
    }

    #[cfg(test)]
    pub(super) fn registration(&self) -> Option<&SessionRegistration> {
        self.share
            .as_ref()
            .map(|share| share.publisher.registration())
    }

    /// Note an agent event the loop has applied this iteration.
    pub(super) fn observe(&mut self, event: &AgentUiEvent) {
        match event {
            AgentUiEvent::PromptStarted { .. } => self.turn_stream_open = true,
            AgentUiEvent::TurnFinished { .. } | AgentUiEvent::Cancelled { .. } => {
                self.turn_stream_open = false;
            }
            _ => {}
        }
        if let Some(share) = self.share.as_mut() {
            share.publisher.observe(event);
        }
    }

    /// One pass: run a `/remote` command if the user entered one, apply
    /// waiting remote commands, then publish what changed.
    pub(super) async fn tick(&mut self, command: Option<SharingCommand>, pump: RemotePump<'_>) {
        if let Some(command) = command {
            self.run_command(command, pump.app_state).await;
            *pump.needs_redraw = true;
        }
        if self.share.is_none() && self.share_all_enabled() {
            self.share_by_itself(pump.app_state).await;
        }
        if self.share.is_some() {
            self.pump(pump).await;
        } else {
            let RemotePump {
                app_state,
                needs_redraw,
                ..
            } = pump;
            self.maintain_standby(app_state, needs_redraw).await;
        }
    }

    async fn run_command(&mut self, command: SharingCommand, app_state: &Arc<Mutex<AppState>>) {
        let report = match command {
            SharingCommand::Enable | SharingCommand::Pair => {
                let mut report = self.enable(app_state).await;
                // A code is only in the way once a device is paired; `/remote
                // pair` asks for one explicitly.
                let paired = self.paired_devices();
                let wants_code = matches!(command, SharingCommand::Pair) || paired == 0;
                if let Some(share) = self.share.as_mut() {
                    share.pairing_due = wants_code;
                }
                self.share_all = None;
                if self.show_pairing_details(app_state) {
                    return;
                }
                if self.share.is_none() {
                    self.stopped = Some(report.clone());
                } else if !wants_code {
                    report.push_str(&format!(
                        "\n{paired} paired device{}. Run /remote pair to add another.",
                        if paired == 1 { "" } else { "s" }
                    ));
                }
                report
            }
            SharingCommand::Status => {
                if self.show_gateway_status(app_state).await {
                    return;
                }
                self.status()
            }
            SharingCommand::Off => self.switch_off().await,
        };
        app_state
            .lock()
            .await
            .show_command_panel(PANEL_TITLE, report);
    }

    fn paired_devices(&self) -> usize {
        #[cfg(unix)]
        if let Some(directory) = self.connector.gateway_directory() {
            return rustcode::remote_gateway::service::paired_devices(&directory);
        }
        0
    }

    /// `/remote off`: switch remote access off for the machine. The gateway
    /// stops, so every terminal's session becomes private and devices are
    /// disconnected; pairings are kept.
    async fn switch_off(&mut self) -> String {
        let was_shared = self.close(SessionCloseReason::SharingDisabled);
        self.share_all = Some((false, std::time::Instant::now()));
        #[cfg(unix)]
        if let Some(directory) = self.connector.gateway_directory() {
            return match rustcode::remote_gateway::service::disable(&directory).await {
                Ok(_) => "Remote access is off for every session on this machine. Running turns are not affected.\n\
                          Paired devices stay paired; /remote turns it back on."
                    .to_owned(),
                Err(error) => {
                    format!("Remote access could not be switched off completely: {error:#}")
                }
            };
        }
        if was_shared {
            "Remote sharing is off. This session is private again; a running turn is not affected."
                .to_owned()
        } else {
            "This session is not shared.".to_owned()
        }
    }

    /// Put the pairing details for the last `/remote` on screen, if they are
    /// due and the gateway has accepted the registration. The request runs on
    /// its own task: the loop does not wait on the gateway's control socket.
    fn show_pairing_details(&mut self, app_state: &Arc<Mutex<AppState>>) -> bool {
        let due = self.share.as_ref().is_some_and(|share| {
            share.pairing_due
                && share
                    .gateway
                    .as_ref()
                    .is_some_and(|gateway| gateway.connected)
        });
        if !due {
            return false;
        }
        if let Some(share) = self.share.as_mut() {
            share.pairing_due = false;
        }
        if self.headless {
            return false;
        }
        #[cfg(unix)]
        if let Some(directory) = self.connector.gateway_directory() {
            tokio::spawn(rustcode::remote_gateway::command::show_pairing_details(
                Arc::clone(app_state),
                PANEL_TITLE,
                self.status(),
                directory,
            ));
            return true;
        }
        let _ = app_state;
        false
    }

    /// `/remote status`: this terminal's sharing state at once, followed by
    /// the gateway's addresses, devices, sessions and logs as soon as its
    /// control socket answers, on its own task like the pairing details.
    async fn show_gateway_status(&self, app_state: &Arc<Mutex<AppState>>) -> bool {
        #[cfg(unix)]
        if let Some(directory) = self.connector.gateway_directory() {
            let session_id = self
                .share
                .as_ref()
                .map(|share| share.publisher.registration().session_id.clone());
            tokio::spawn(rustcode::remote_gateway::command::show_gateway_status(
                Arc::clone(app_state),
                PANEL_TITLE,
                self.session_status(),
                directory,
                session_id,
            ));
            return true;
        }
        let _ = app_state;
        false
    }

    /// Share the session on screen. Repeating it changes nothing about the
    /// registration; it shows fresh pairing details.
    async fn enable(&mut self, app_state: &Arc<Mutex<AppState>>) -> String {
        if let Some(share) = self.share.as_mut() {
            share.pairing_due = true;
            return self.status();
        }
        let registration = SessionRegistration {
            session_id: app_state.lock().await.active_session_id.clone(),
            registration_epoch: next_registration_epoch(),
        };
        // Opening the link is the connector's business; the state is not
        // locked while it runs.
        let link = match self.connector.connect(&registration) {
            Ok(link) => link,
            Err(error @ OwnerLinkError::NoGateway) => {
                return format!(
                    "Remote sharing is not available: {error}.\n\
                     This session stays private. The remote gateway runs on macOS and Linux."
                );
            }
            Err(error) => {
                return format!(
                    "Remote sharing could not start: {error}.\nThis session stays private."
                );
            }
        };
        let (publisher, register) = {
            let state = app_state.lock().await;
            if state.active_session_id != registration.session_id {
                return "Remote sharing could not start: the session changed. Run /remote again."
                    .to_owned();
            }
            SessionPublisher::register(&state, registration, self.turn_stream_open)
        };
        if link.outbound.try_send(register).is_err() {
            return "Remote sharing could not start: the gateway link closed.\nThis session stays private."
                .to_owned();
        }
        self.share = Some(Share {
            publisher,
            link,
            gateway: None,
            pairing_due: true,
        });
        // The shared registration replaces the standby announcement; its
        // connection ends when the task notices the dropped sender.
        #[cfg(unix)]
        {
            self.standby = None;
        }
        {
            let mut state = app_state.lock().await;
            state.pending_share_prompt = None;
            state.share_decision = None;
        }
        self.status()
    }

    /// Announce the unshared session as reachable when a gateway already runs,
    /// pump share requests and user decisions. Never starts a gateway: the
    /// announcement must not bring one up behind the user's back.
    /// Unix only: the owner socket does not exist on other platforms.
    #[cfg(unix)]
    async fn maintain_standby(
        &mut self,
        app_state: &Arc<Mutex<AppState>>,
        needs_redraw: &mut bool,
    ) {
        if self.headless {
            return;
        }
        let session_id = app_state.lock().await.active_session_id.clone();
        if self
            .standby
            .as_ref()
            .is_some_and(|standby| standby.session_id != session_id)
        {
            self.drop_standby(app_state).await;
        }
        if self.standby.is_none()
            && self
                .standby_retry_at
                .is_none_or(|at| std::time::Instant::now() >= at)
        {
            self.standby_retry_at = None;
            if let Some(directory) = self.connector.gateway_directory() {
                // Resolved only when an announcement starts: this runs on
                // every pass of the event loop, so anything read here is paid
                // per key press and per pointer move.
                let (title, workspace) = {
                    let state = app_state.lock().await;
                    (
                        standby_title(&state),
                        state
                            .task_working_directory
                            .as_ref()
                            .map(|path| path.to_string_lossy().into_owned()),
                    )
                };
                self.standby = Some(spawn_standby(directory, session_id, title, workspace));
            }
        }
        let mut gone = false;
        let mut requested = Vec::new();
        if let Some(standby) = self.standby.as_mut() {
            while let Ok(event) = standby.from_task.try_recv() {
                match event {
                    StandbyEvent::Gone => {
                        gone = true;
                        break;
                    }
                    StandbyEvent::ShareRequested {
                        pending_id,
                        device_name,
                    } => requested.push((pending_id, device_name)),
                }
            }
        }
        if gone {
            self.drop_standby(app_state).await;
            self.standby_retry_at = Some(std::time::Instant::now() + STANDBY_RETRY);
            return;
        }
        for (pending_id, device_name) in requested {
            let mut state = app_state.lock().await;
            if state.pending_share_prompt.is_some() {
                // One prompt at a time; decline the extra at once.
                if let Some(standby) = self.standby.as_ref() {
                    let _ = standby.to_task.try_send(OwnerFrame::ShareDecision {
                        pending_id,
                        accepted: false,
                    });
                }
            } else {
                state.pending_share_prompt = Some(rustcode::app::PendingSharePrompt {
                    pending_id,
                    device_name: device_name.clone(),
                });
                state.show_command_panel(
                    PANEL_TITLE,
                    format!(
                        "Device '{device_name}' asks to share this session.\nPress y to share it, n to decline."
                    ),
                );
                *needs_redraw = true;
            }
        }
        let decision = { app_state.lock().await.share_decision.take() };
        if let Some(accepted) = decision {
            let prompt = { app_state.lock().await.pending_share_prompt.take() };
            if let (Some(standby), Some(prompt)) = (self.standby.as_ref(), prompt) {
                let _ = standby.to_task.try_send(OwnerFrame::ShareDecision {
                    pending_id: prompt.pending_id,
                    accepted,
                });
            }
            if accepted {
                // Drop the announcement first; the shared registration
                // that follows resolves the device's request.
                self.drop_standby(app_state).await;
                self.run_command(SharingCommand::Enable, app_state).await;
            } else {
                app_state.lock().await.show_command_panel(
                    PANEL_TITLE,
                    "Share request declined. This session stays private.",
                );
            }
            *needs_redraw = true;
        }
    }

    /// End the announcement. The gateway clears it when the task's
    /// connection ends; prompts for the old session die with it.
    /// Unix only: there is no standby link on other platforms.
    #[cfg(unix)]
    async fn drop_standby(&mut self, app_state: &Arc<Mutex<AppState>>) {
        self.standby = None;
        let mut state = app_state.lock().await;
        state.pending_share_prompt = None;
        state.share_decision = None;
    }

    /// No standby announcements off unix: the owner socket does not exist.
    #[cfg(not(unix))]
    async fn maintain_standby(
        &mut self,
        _app_state: &Arc<Mutex<AppState>>,
        _needs_redraw: &mut bool,
    ) {
    }

    fn status(&self) -> String {
        let status = self.session_status();
        if self.share.is_none() {
            return status;
        }
        format!(
            "{status}\n/remote status shows the gateway's addresses, paired devices, pairing state and logs."
        )
    }

    /// Whether and how this terminal's session is shared, without the
    /// gateway's own detail.
    fn session_status(&self) -> String {
        let Some(share) = self.share.as_ref() else {
            return "This session is private. Run /remote to turn remote access on for every session."
                .to_owned();
        };
        let registration = share.publisher.registration();
        let names = |names: &[String]| {
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        };
        let gateway = match &share.gateway {
            None => "Gateway  connecting…".to_owned(),
            Some(gateway) if !gateway.connected => {
                "Gateway  unreachable; looking for it again. Devices cannot see this session meanwhile."
                    .to_owned()
            }
            Some(gateway) => format!(
                "Gateway  {}{}\n\
                 Attached devices  {}\n\
                 Connected devices  {}",
                gateway.advertised_address,
                if gateway.loopback_only {
                    " (this machine only; a phone cannot reach it)"
                } else {
                    ""
                },
                names(&gateway.attached_devices),
                names(&gateway.connected_devices),
            ),
        };
        format!(
            "This session is shared with the remote gateway.\n\
             Session  {}\n\
             Epoch  {}\n\
             {gateway}\n\
             Published  {} updates{}\n\
             Run /remote off to turn remote access off for every session.",
            registration.session_id,
            registration.registration_epoch,
            share.publisher.sequence(),
            if share.publisher.is_lagging() {
                " (the gateway is behind; a resync is pending)"
            } else {
                ""
            },
        )
    }

    /// End the registration, if there is one: reject what is still queued on
    /// it, tell the gateway why, and drop the link.
    pub(super) fn close(&mut self, reason: SessionCloseReason) -> bool {
        let Some(mut share) = self.share.take() else {
            return false;
        };
        while let Ok(command) = share.link.commands.try_recv() {
            if let OwnerCommand::Request { request, reply } = command {
                reject_stale(request, reply);
            }
        }
        // Dropping the link is what ends the registration; the reason is a
        // courtesy that a full queue may lose.
        let _ = share
            .link
            .outbound
            .try_send(OwnerMessage::Unregister { reason });
        true
    }

    async fn pump(&mut self, pump: RemotePump<'_>) {
        let RemotePump {
            app_state,
            client,
            cancel_token,
            agent_ui_event_sender,
            terminal_input_idle,
            needs_redraw,
        } = pump;
        let share_all = self.share_all_enabled();
        let Some(share) = self.share.as_mut() else {
            return;
        };

        let mut closed = None;
        for _ in 0..COMMANDS_PER_TICK {
            if !terminal_input_idle {
                break;
            }
            match share.link.commands.try_recv() {
                Ok(OwnerCommand::Request { request, reply }) => {
                    let registration = share.publisher.registration();
                    let response = if request.operation.is_mutation() {
                        apply_mutation(
                            registration,
                            &request,
                            app_state,
                            client,
                            cancel_token,
                            agent_ui_event_sender,
                        )
                        .await
                    } else {
                        let state = app_state.lock().await;
                        let limits = ProjectionLimits::default();
                        match read_session(&state, registration, &request, &limits) {
                            Ok(result) => RemoteResponse::new(request.request_id, result),
                            Err(error) => RemoteResponse::error(request.request_id, error),
                        }
                    };
                    reply.send(response);
                    *needs_redraw = true;
                }
                Ok(OwnerCommand::SnapshotRequested) => share.publisher.request_snapshot(),
                Ok(OwnerCommand::Status(status)) => share.gateway = Some(status),
                Ok(OwnerCommand::Closed { reason }) => {
                    closed = Some(format!("Remote sharing stopped: {reason}"));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = Some("Remote sharing stopped: the gateway link closed".to_owned());
                    break;
                }
            }
        }

        if closed.is_none() {
            // A lagging link resyncs once half of its queue has drained, so a
            // slow gateway is not handed a snapshot for every freed slot.
            let room_for_resync =
                share.link.outbound.capacity() * 2 >= share.link.outbound.max_capacity();
            let messages = {
                let state = app_state.lock().await;
                share
                    .publisher
                    .owns(&state)
                    .then(|| share.publisher.publish(&state, room_for_resync))
            };
            match messages {
                // The terminal moved to another session: nothing of it is
                // published under the old registration. With remote access
                // on, the next pass shares it under its own; otherwise it
                // stays private until `/remote` runs.
                None => {
                    self.close(SessionCloseReason::SessionChanged);
                    if share_all {
                        self.share_all_retry_at = None;
                        return;
                    }
                    closed = Some(
                        "Remote sharing stopped: the session changed. Run /remote to share this one."
                            .to_owned(),
                    );
                }
                Some(messages) => {
                    for message in messages {
                        match share.link.outbound.try_send(message) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                share.publisher.mark_lagging();
                                break;
                            }
                            Err(TrySendError::Closed(_)) => {
                                closed = Some(
                                    "Remote sharing stopped: the gateway link closed".to_owned(),
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }

        if let Some(notice) = closed {
            self.close(SessionCloseReason::OwnerExited);
            // The gateway may have gone because remote access was switched
            // off elsewhere: read that again before sharing by itself, or
            // this terminal would start the gateway right back up.
            self.share_all = None;
            self.stopped = Some(notice.clone());
            let mut state = app_state.lock().await;
            // The panel may still say that sharing is starting.
            state.update_command_panel(PANEL_TITLE, format!("{notice}\nThis session is private."));
            state.set_transient_notice(notice);
            *needs_redraw = true;
        } else if self.show_pairing_details(app_state) {
            *needs_redraw = true;
        }
    }
}

/// Title shown to the asking device, so the terminal can tell its sessions
/// apart. Falls back to the session id when nothing was saved yet.
/// Unix only: part of the standby announcement.
#[cfg(unix)]
/// The name a device sees for the session on screen: its saved title, else
/// one derived from the first prompt, else its id. Reads one small file at
/// most; listing the session store here parsed every recent transcript.
fn standby_title(state: &AppState) -> String {
    rustcode::config::load_session_title(&state.active_session_id)
        .or_else(|| {
            rustcode::config::session_has_content(&state.history)
                .then(|| rustcode::config::session_title(&state.history))
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| state.active_session_id.clone())
}

#[cfg(unix)]
fn spawn_standby(
    config_directory: std::path::PathBuf,
    session_id: String,
    title: String,
    workspace: Option<String>,
) -> Standby {
    let (to_task, from_ui) = tokio::sync::mpsc::channel(8);
    let (to_ui, from_task) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(standby_task(
        config_directory,
        session_id.clone(),
        title,
        workspace,
        to_ui,
        from_ui,
    ));
    Standby {
        to_task,
        from_task,
        session_id,
    }
}

/// Announce one session and relay share requests until the bridge drops the
/// link, the gateway closes it, or a write fails. Connects only: a gateway
/// the user stopped is never brought back behind their back.
/// Unix only: the owner socket does not exist on other platforms.
#[cfg(unix)]
async fn standby_task(
    config_directory: std::path::PathBuf,
    session_id: String,
    title: String,
    workspace: Option<String>,
    to_ui: tokio::sync::mpsc::UnboundedSender<StandbyEvent>,
    mut from_ui: tokio::sync::mpsc::Receiver<OwnerFrame>,
) {
    let gone = |to_ui: &tokio::sync::mpsc::UnboundedSender<StandbyEvent>| {
        let _ = to_ui.send(StandbyEvent::Gone);
    };
    let socket = RemoteLifecycle::new(&config_directory).owner_socket_path();
    let stream = match tokio::net::UnixStream::connect(&socket).await {
        Ok(stream) => stream,
        Err(_) => return gone(&to_ui),
    };
    let (reader, writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader);
    let mut writer = writer;
    let mut buffer = Vec::new();
    let announce = OwnerFrame::Announce {
        ipc_version: OWNER_IPC_VERSION,
        session_id,
        title,
        workspace,
    };
    if rustcode::daemon::protocol::write_async_frame(&mut writer, &announce)
        .await
        .is_err()
    {
        return gone(&to_ui);
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        rustcode::daemon::protocol::read_async_frame_with_buffer::<_, GatewayFrame>(
            &mut reader,
            &mut buffer,
        ),
    )
    .await
    {
        Ok(Ok(GatewayFrame::Announced { .. })) => {}
        _ => return gone(&to_ui),
    }
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            outbound = from_ui.recv() => {
                let Some(frame) = outbound else { return };
                if rustcode::daemon::protocol::write_async_frame(&mut writer, &frame).await.is_err() {
                    return gone(&to_ui);
                }
            }
            inbound = rustcode::daemon::protocol::read_async_frame_with_buffer::<_, GatewayFrame>(&mut reader, &mut buffer) => {
                match inbound {
                    Ok(GatewayFrame::ShareRequested { pending_id, device_name }) => {
                        let _ = to_ui.send(StandbyEvent::ShareRequested { pending_id, device_name });
                    }
                    Ok(GatewayFrame::Pong) | Ok(GatewayFrame::Announced { .. }) => {}
                    _ => return gone(&to_ui),
                }
            }
            _ = heartbeat.tick() => {
                if rustcode::daemon::protocol::write_async_frame(&mut writer, &OwnerFrame::Ping).await.is_err() {
                    return gone(&to_ui);
                }
            }
        }
    }
}

/// Apply one remote mutation on the loop's task, with the loop's own cancel
/// token, and finish it the way the terminal finishes its own.
async fn apply_mutation(
    registration: &SessionRegistration,
    request: &RemoteRequest,
    app_state: &Arc<Mutex<AppState>>,
    client: &reqwest::Client,
    cancel_token: &mut CancellationToken,
    agent_ui_event_sender: &AgentUiEventSender,
) -> RemoteResponse {
    match apply_session_mutation(app_state, cancel_token, registration, request).await {
        Ok(mutation) => {
            match mutation.follow_up {
                OwnerFollowUp::None => {}
                // The queue lease makes this the same single worker a prompt
                // typed in the terminal would start.
                OwnerFollowUp::StartTurn => {
                    spawn_observed_orchestrator(
                        client.clone(),
                        Arc::clone(app_state),
                        cancel_token.clone(),
                        agent_ui_event_sender.clone(),
                    )
                    .await;
                }
                OwnerFollowUp::FinishCancel => {
                    rustcode::app::stop_turn_keeping_draft(app_state, cancel_token).await;
                }
            }
            RemoteResponse::new(request.request_id.clone(), mutation.result)
                .with_receipt(ReceiptState::Applied)
        }
        Err(error) => RemoteResponse::error(request.request_id.clone(), error)
            .with_receipt(ReceiptState::Rejected),
    }
}

fn reject_stale(request: RemoteRequest, reply: OwnerReply) {
    let response = RemoteResponse::error(
        request.request_id,
        RemoteError::new(
            RemoteErrorCode::StaleSession,
            "the session is no longer shared under this registration",
        ),
    );
    reply.send(if request.operation.is_mutation() {
        response.with_receipt(ReceiptState::Rejected)
    } else {
        response
    });
}

/// Apply an approval decision made in the terminal, unless the batch it was
/// made for is gone. A remote device may have resolved it while the key press
/// was queued; applying the late decision anyway would let a stale deny
/// cancel the turn the other answer allowed to continue.
pub(super) async fn apply_terminal_approval(
    app_state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    decision: rustcode::app::ApprovalDecision,
) -> bool {
    let pending = app_state.lock().await.tool_confirmation_response.is_some();
    if pending {
        apply_approval_decision(app_state, cancel_token, decision).await;
    } else {
        app_state
            .lock()
            .await
            .set_transient_notice("That approval was already resolved");
    }
    pending
}

/// As [`apply_terminal_approval`], for an answer to a question.
pub(super) async fn apply_terminal_answer(
    app_state: &Arc<Mutex<AppState>>,
    cancel_token: &mut CancellationToken,
    answer: rustcode::app::QuestionAnswer,
) -> bool {
    let pending = app_state.lock().await.question_response.is_some();
    if pending {
        apply_question_answer(app_state, cancel_token, answer).await;
    } else {
        app_state
            .lock()
            .await
            .set_transient_notice("That question was already answered");
    }
    pending
}

impl AppRuntime {
    /// What one iteration of [`AppRuntime::run`] does for remote sharing and
    /// the prompt queue, without a terminal.
    pub(crate) async fn run_remote_iteration(&mut self) {
        let command = self.app_state.lock().await.remote_command.take();
        while let Ok(event) = self.agent_ui_event_receiver.try_recv() {
            self.remote.observe(&event);
        }
        self.remote
            .tick(
                command,
                RemotePump {
                    app_state: &self.app_state,
                    client: &self.client,
                    cancel_token: &mut self.current_cancel_token,
                    agent_ui_event_sender: &self.agent_ui_event_sender,
                    terminal_input_idle: self.app_event_receiver.is_empty(),
                    needs_redraw: &mut self.needs_redraw,
                },
            )
            .await;
        spawn_observed_orchestrator(
            self.client.clone(),
            Arc::clone(&self.app_state),
            self.current_cancel_token.clone(),
            self.agent_ui_event_sender.clone(),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode::app::{DraftSubmitMode, PendingQuestion, QuestionAnswer};
    use rustcode::controller::ApprovalChoice;
    use rustcode::remote::owner::{GatewayLink, InMemoryConnector, InMemoryGateway};
    use rustcode::remote::{
        PromptDisposition, REMOTE_PROTOCOL_VERSION, RemoteAnswer, RemoteEvent, RemoteOperation,
        RemoteResult, RemoteSnapshot, ResyncReason,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PROVIDER_TEXT: &str = "hello from provider";
    const WAIT: Duration = Duration::from_secs(15);

    /// A local streaming provider that counts the requests it receives. Every
    /// response sends its text, then stays open until `release` is set.
    struct FakeProvider {
        endpoint: String,
        requests: Arc<AtomicUsize>,
        release: tokio::sync::watch::Sender<bool>,
    }

    impl FakeProvider {
        async fn start(held: bool) -> Self {
            Self::start_mode(held, false).await
        }

        async fn start_mode(held: bool, failed: bool) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local provider");
            let endpoint = format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().expect("provider address")
            );
            let requests = Arc::new(AtomicUsize::new(0));
            let (release, released) = tokio::sync::watch::channel(!held);
            let counter = Arc::clone(&requests);
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(serve_stream(socket, released.clone(), failed));
                }
            });
            Self {
                endpoint,
                requests,
                release,
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }

        fn release(&self) {
            let _ = self.release.send(true);
        }

        fn state(&self) -> AppState {
            use rustcode::config::{ApiProtocol, ModelProfile};
            let mut state = AppState::new();
            state.api_base_url = self.endpoint.clone();
            state.model_name = "remote-bridge-test".to_owned();
            state.config.models = vec![ModelProfile {
                name: state.model_name.clone(),
                url: self.endpoint.clone(),
                model: state.model_name.clone(),
                api_protocol: Some(ApiProtocol::ChatCompletions),
                context_window: Some(8_192),
                ..ModelProfile::default()
            }];
            state.record_function_calling_support(&self.endpoint, false);
            state
        }
    }

    async fn serve_stream(
        mut socket: tokio::net::TcpStream,
        mut released: tokio::sync::watch::Receiver<bool>,
        failed: bool,
    ) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let Ok(read) = socket.read(&mut buffer).await else {
                return;
            };
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        if failed {
            let body = r#"{"error":{"message":"test provider failure"}}"#;
            let _ = socket.write_all(format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            return;
        }
        let first = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{PROVIDER_TEXT}\"}}}}]}}\n\n"
        );
        let last = concat!(
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        if socket
            .write_all(format!("{head}{:X}\r\n{first}\r\n", first.len()).as_bytes())
            .await
            .is_err()
        {
            return;
        }
        if released.wait_for(|released| *released).await.is_err() {
            return;
        }
        let _ = socket
            .write_all(format!("{:X}\r\n{last}\r\n0\r\n\r\n", last.len()).as_bytes())
            .await;
    }

    /// A runtime whose `/remote` connects to an in-memory gateway.
    fn shared_runtime(state: AppState, outbound_capacity: usize) -> (AppRuntime, InMemoryGateway) {
        let (connector, gateway) = InMemoryConnector::new(outbound_capacity, 4);
        let mut runtime = AppRuntime::for_test(state);
        runtime.remote = RemoteBridge::with_connector(Box::new(connector));
        (runtime, gateway)
    }

    /// Enter `line` in the composer the way the terminal does.
    async fn enter(runtime: &mut AppRuntime, line: &str) {
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = line.to_owned();
            state.cursor_position = line.len();
        }
        rustcode::app::handle_enter_with_ui_events(
            &runtime.app_state,
            &runtime.client,
            &mut runtime.current_cancel_token,
            runtime.agent_ui_event_sender.clone(),
            &|| Vec::new(),
        )
        .await;
    }

    /// Run `/remote` and return the link the gateway was handed, with the
    /// snapshot that opened it.
    async fn share(
        runtime: &mut AppRuntime,
        gateway: &InMemoryGateway,
    ) -> (SessionRegistration, GatewayLink, Box<RemoteSnapshot>) {
        enter(runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        let (registration, mut link) = gateway.accept().expect("/remote opened a link");
        let Ok(OwnerMessage::Register {
            registration: registered,
            snapshot,
        }) = link.messages.try_recv()
        else {
            panic!("a link opens with its registration");
        };
        assert_eq!(registered, registration);
        (registration, link, snapshot)
    }

    fn request(registration: &SessionRegistration, operation: RemoteOperation) -> RemoteRequest {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        RemoteRequest {
            authenticated_device_id: None,
            protocol_version: REMOTE_PROTOCOL_VERSION,
            request_id: format!("req-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            session_id: Some(registration.session_id.clone()),
            registration_epoch: Some(registration.registration_epoch),
            operation,
        }
    }

    /// Send `operation` and run the loop once: the owner's decision.
    async fn send(
        runtime: &mut AppRuntime,
        link: &GatewayLink,
        registration: &SessionRegistration,
        operation: RemoteOperation,
    ) -> RemoteResponse {
        let response = link
            .try_request(request(registration, operation))
            .expect("the command queue has room");
        runtime.run_remote_iteration().await;
        response.await.expect("the owner replied")
    }

    fn rejection(response: &RemoteResponse) -> RemoteErrorCode {
        assert_eq!(response.receipt, Some(ReceiptState::Rejected));
        match &response.result {
            RemoteResult::Error(error) => error.code,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    fn submit(prompt: &str) -> RemoteOperation {
        RemoteOperation::SubmitPrompt {
            prompt: prompt.to_owned(),
        }
    }

    /// Run the loop until `done` holds for the messages received so far.
    async fn publish_until(
        runtime: &mut AppRuntime,
        link: &mut GatewayLink,
        received: &mut Vec<OwnerMessage>,
        done: impl Fn(&[OwnerMessage]) -> bool,
    ) {
        tokio::time::timeout(WAIT, async {
            loop {
                runtime.run_remote_iteration().await;
                while let Ok(message) = link.messages.try_recv() {
                    received.push(message);
                }
                if done(received) {
                    break;
                }
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the bridge published what the test waits for");
    }

    fn events(messages: &[OwnerMessage]) -> Vec<(u64, RemoteEvent)> {
        messages
            .iter()
            .filter_map(|message| match message {
                OwnerMessage::Event(frame) => Some((frame.sequence, frame.event.clone())),
                _ => None,
            })
            .collect()
    }

    fn turn_ended(messages: &[OwnerMessage]) -> bool {
        events(messages)
            .iter()
            .any(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
    }

    async fn panel(runtime: &AppRuntime) -> String {
        runtime
            .app_state()
            .await
            .command_panel
            .as_ref()
            .map(|panel| panel.content.clone())
            .unwrap_or_default()
    }

    #[test]
    fn remote_panel_paints_the_whole_qr_black_on_white_in_light_and_dark_themes() {
        use crate::inline_terminal::InlineTerminal as Terminal;
        use crate::ui::render;
        use crate::ui::tests::THEME_TEST_LOCK;
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;
        use rustcode::controller::RenderState;
        use rustcode::remote_gateway::address::AdvertisedAddress;
        use rustcode::remote_gateway::command::{QrStyle, format_offer};
        use rustcode::remote_gateway::control::OfferDetails;
        use rustcode::remote_gateway::handshake::Secret;
        use rustcode::remote_gateway::pairing::PairingOffer;
        use rustcode::remote_gateway::qr;

        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        for (theme, host) in [("default", "192.168.1.20"), ("light", "127.0.0.1")] {
            let offer = OfferDetails::new(
                &AdvertisedAddress::new(host, 17879).unwrap(),
                "0123456789abcdef0123456789abcdef",
                Some("a-host-with-a-longer-name"),
                PairingOffer {
                    credential: Secret::new("q0lYc1o3bXJ3T2d5d0l2Rk5kU2tqeTZqZ0Z2ZUo0V2s"),
                    code: Secret::new("1234-5678"),
                    lifetime: std::time::Duration::from_secs(120),
                },
            );
            let rows = qr::rows(offer.qr_payload.expose()).unwrap();
            let mut state = RenderState::new();
            state.config.theme = theme.into();
            state.command_panel = Some(rustcode::controller::CommandPanel {
                title: "Remote",
                content: format!(
                    "{}\n\nThis session is shared with the remote gateway.\nSession  test-session\nAttached devices  none",
                    format_offer(&offer, QrStyle::Panel)
                ),
            });
            let mut terminal = Terminal::new(TestBackend::new(80, 40)).unwrap();
            terminal
                .draw(|frame| {
                    render(frame, &state);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let qr_y = (0..40)
                .find(|&y| buffer[(2, y)].bg == Color::Indexed(231))
                .expect("QR is visible");
            assert!(
                qr_y + rows.len() as u16 <= 38,
                "the entire QR fits above the footer"
            );
            let text = (0..7)
                .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains(&offer.advertised_address) && text.contains("1234-5678"),
                "{text}"
            );
            for (y, row) in rows.iter().enumerate() {
                for (x, symbol) in row.chars().enumerate() {
                    let cell = &buffer[(2 + x as u16, qr_y + y as u16)];
                    assert_eq!(
                        cell.symbol(),
                        symbol.to_string(),
                        "QR cell ({x}, {y}) in {theme}"
                    );
                    assert_eq!(
                        cell.fg,
                        Color::Indexed(16),
                        "QR foreground ({x}, {y}) in {theme}"
                    );
                    assert_eq!(
                        cell.bg,
                        Color::Indexed(231),
                        "QR background ({x}, {y}) in {theme}"
                    );
                }
            }
        }
        crate::ui::theme::set_active_theme("default");
    }

    #[tokio::test]
    async fn remote_without_a_gateway_reports_it_and_shares_nothing() {
        let mut runtime = AppRuntime::for_test(AppState::new());

        enter(&mut runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        let report = panel(&runtime).await;
        assert!(
            report.contains("not available") && report.contains("no remote gateway is available"),
            "unexpected report: {report}"
        );
        assert!(report.contains("stays private"));
        assert!(runtime.remote.registration().is_none());

        enter(&mut runtime, "/remote status").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("This session is private"));

        enter(&mut runtime, "/remote off").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("not shared"));

        enter(&mut runtime, "/remote everywhere").await;
        assert!(panel(&runtime).await.starts_with("Usage:"));
        assert!(runtime.app_state().await.remote_command.is_none());
    }

    /// Run the loop until the `/remote` panel satisfies `done`.
    async fn panel_until(runtime: &mut AppRuntime, done: impl Fn(&str) -> bool) -> String {
        tokio::time::timeout(WAIT, async {
            loop {
                runtime.run_remote_iteration().await;
                let report = panel(runtime).await;
                if done(&report) {
                    return report;
                }
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the panel shows what the test waits for")
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_registers_with_a_running_gateway_and_shows_how_to_pair() {
        use rustcode::remote_gateway::address::plan_listen;
        use rustcode::remote_gateway::gateway::{Gateway, GatewayConfig, Limits};
        use rustcode::remote_gateway::hub::{HubLimits, SessionHub};
        use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
        use rustcode::remote_gateway::owner_client::GatewayConnector;

        // A gateway on a loopback port in its own configuration directory.
        let dir = std::path::PathBuf::from(format!(
            "/tmp/rustcode-tui-remote-{}-{}",
            std::process::id(),
            next_registration_epoch()
        ));
        std::fs::create_dir_all(&dir).expect("temporary configuration directory");
        let lifecycle = RemoteLifecycle::new(&dir);
        let hub = SessionHub::start(HubLimits::default());
        let gateway = Gateway::bind(
            &lifecycle,
            GatewayConfig {
                replan: None,
                plan: plan_listen("127.0.0.1", None, &[]).expect("loopback plan"),
                port: 0,
                router: hub.clone(),
                sessions: Some(hub),
                limits: Limits::default(),
            },
        )
        .await
        .expect("the gateway binds");
        let address = gateway.local_addr().to_string();
        let shutdown = gateway.shutdown_token();
        let serving = tokio::spawn(gateway.run());

        let mut runtime = AppRuntime::for_test(AppState::new());
        runtime.remote =
            RemoteBridge::with_connector(Box::new(GatewayConnector::discover(lifecycle.clone())));
        let session_id = runtime.app_state().await.active_session_id.clone();

        // `/remote` answers at once; the gateway is reached from its own task.
        enter(&mut runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("This session is shared"));
        let report = panel_until(&mut runtime, |report| report.contains("code:")).await;
        assert!(report.contains(&format!("Gateway  {address}")), "{report}");
        assert!(report.contains(&format!("address:  {address}")), "{report}");
        // The gateway is on loopback: the panel says what that means and
        // what to run instead.
        assert!(report.contains("a phone cannot reach it"), "{report}");
        assert!(report.contains("rustcode remote serve"));
        assert!(report.contains("[remote]"));
        // A scannable code, as marked rows the panel paints black on white.
        assert!(
            report
                .lines()
                .filter(|line| line.starts_with('\u{2060}'))
                .count()
                > 10
        );
        let offer_code = report
            .lines()
            .find_map(|line| line.trim().strip_prefix("code:"))
            .expect("the manual code is shown")
            .trim()
            .to_owned();
        assert_eq!(offer_code.len(), 9, "{offer_code}");

        let status = lifecycle
            .status()
            .await
            .unwrap()
            .expect("gateway is running");
        let sessions = status.sessions.expect("the gateway routes sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);

        // Repeating `/remote` keeps the registration and replaces the offer.
        let registration = runtime.remote.registration().cloned();
        enter(&mut runtime, "/remote").await;
        let again = panel_until(&mut runtime, |report| {
            report.contains("code:") && !report.contains(&offer_code)
        })
        .await;
        assert!(again.contains("This session is shared"));
        assert_eq!(runtime.remote.registration().cloned(), registration);
        assert_eq!(
            lifecycle
                .status()
                .await
                .unwrap()
                .unwrap()
                .sessions
                .unwrap()
                .len(),
            1
        );

        enter(&mut runtime, "/remote status").await;
        runtime.run_remote_iteration().await;
        let status = panel(&runtime).await;
        assert!(status.contains("Attached devices  none"), "{status}");
        assert!(!status.contains("code:"), "status shows no pairing secret");
        // The gateway's own detail follows from its control socket: where it
        // listens, the sessions with this terminal's marked, and the logs.
        let status = panel_until(&mut runtime, |report| report.contains("gateway log")).await;
        assert!(status.contains("Attached devices  none"), "{status}");
        assert!(
            status.contains(&format!("listening on   {address}  (fixed bind)")),
            "{status}"
        );
        assert!(
            status.contains(&format!("{session_id}  ")) && status.contains("[this terminal]"),
            "{status}"
        );
        assert!(
            status.contains(&lifecycle.log_path().display().to_string()),
            "{status}"
        );
        assert!(!status.contains("code:"), "status shows no pairing secret");

        // Remote access is on for the machine now: a session the terminal
        // switches to shares itself, with nothing typed and no code shown.
        runtime
            .handle_event(AppEvent::NewSession)
            .await
            .expect("new session");
        let new_session_id = runtime.app_state().await.active_session_id.clone();
        assert_ne!(new_session_id, session_id);
        tokio::time::timeout(WAIT, async {
            loop {
                runtime.run_remote_iteration().await;
                let sessions = lifecycle.status().await.unwrap().unwrap().sessions.unwrap();
                if sessions.len() == 1 && sessions[0].session_id == new_session_id {
                    break;
                }
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the new session is shared by itself");
        assert!(!panel(&runtime).await.contains("code:"));

        // `/remote off` switches remote access off for the machine: the
        // gateway stops, and nothing shares itself afterwards.
        enter(&mut runtime, "/remote off").await;
        runtime.run_remote_iteration().await;
        assert!(
            panel(&runtime)
                .await
                .contains("Remote access is off for every session")
        );
        assert!(!rustcode::remote_gateway::service::enabled(&dir));
        tokio::time::timeout(WAIT, async {
            while lifecycle.status().await.unwrap().is_some() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the gateway stops");
        for _ in 0..3 {
            runtime.run_remote_iteration().await;
        }
        assert!(runtime.remote.registration().is_none());

        shutdown.cancel();
        serving.await.unwrap().unwrap();

        // Without a gateway, and with nothing allowed to start one, sharing
        // stops and says what to run.
        enter(&mut runtime, "/remote").await;
        let stopped = panel_until(&mut runtime, |report| report.contains("stopped")).await;
        assert!(stopped.contains("rustcode remote serve"), "{stopped}");
        assert!(stopped.contains("This session is private"), "{stopped}");
        assert!(runtime.remote.registration().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn repeating_remote_keeps_the_same_registration() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, _link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(snapshot.sequence, 0);
        assert_eq!(snapshot.session.session_id, registration.session_id);

        enter(&mut runtime, "/remote").await;
        runtime.run_remote_iteration().await;
        assert!(gateway.accept().is_none(), "no second link is opened");
        assert_eq!(runtime.remote.registration(), Some(&registration));
        assert!(panel(&runtime).await.contains("This session is shared"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attaching_during_generation_starts_no_second_turn() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        enter(&mut runtime, "say hello").await;
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.current_response.is_empty() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the terminal's turn is streaming");
        let (session_id, turn_id) = {
            let state = runtime.app_state().await;
            (
                state.active_session_id.clone(),
                state.active_turn_id.clone().expect("a running turn"),
            )
        };

        let (registration, mut link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(registration.session_id, session_id);
        let turn = snapshot.turn.as_ref().expect("the snapshot shows the turn");
        assert_eq!(turn.turn_id, turn_id);
        assert_eq!(turn.live_response.text, PROVIDER_TEXT);
        {
            let state = runtime.app_state().await;
            assert_eq!(state.active_session_id, session_id);
            assert_eq!(state.active_turn_id.as_deref(), Some(turn_id.as_str()));
            assert!(state.orchestrator_running);
        }
        assert_eq!(provider.requests(), 1);

        provider.release();
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, turn_ended).await;
        let published = events(&received);
        // The text the snapshot already held is not sent again, and the turn
        // that ends is the one the snapshot named.
        assert!(
            !published
                .iter()
                .any(|(_, event)| matches!(event, RemoteEvent::TextDelta { text, .. } if !text.is_empty())),
            "unexpected text after the snapshot: {published:?}"
        );
        let Some((
            _,
            RemoteEvent::TurnFinished {
                turn_id: Some(ended_id),
                timing: Some(timing),
            },
        )) = published.last()
        else {
            panic!("missing terminal timing: {published:?}");
        };
        assert_eq!(ended_id, &turn_id);
        assert_eq!(timing.turn_id, turn_id);
        assert_eq!(
            timing.outcome,
            Some(rustcode::remote::protocol::RemoteTurnOutcome::Completed)
        );
        assert!(
            timing.elapsed_work_ms.is_some()
                && timing.started_at.is_some()
                && timing.ended_at.is_some()
        );
        let registration = runtime.remote.registration().cloned().unwrap();
        let history = send(
            &mut runtime,
            &link,
            &registration,
            RemoteOperation::GetHistory {
                cursor: None,
                limit: 40,
            },
        )
        .await;
        let RemoteResult::History(page) = history.result else {
            panic!("history response")
        };
        let assistant = page
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .unwrap();
        assert_eq!(assistant.turn.as_ref(), Some(timing));
        assert!(assistant.response_time_ms.is_some());
        for (index, (sequence, _)) in published.iter().enumerate() {
            assert_eq!(*sequence, index as u64 + 1);
        }
        assert_eq!(provider.requests(), 1, "one provider request in total");
        assert_eq!(runtime.app_state().await.active_session_id, session_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_remote_prompt_runs_once_and_leaves_the_terminal_draft_alone() {
        let provider = FakeProvider::start(false).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        // An attachment is part of the draft text: a pasted image path.
        let draft = "look at /tmp/screenshot.png and";
        let draft_of = |state: &AppState| {
            (
                state.input_buffer.clone(),
                state.cursor_position,
                state.draft_submit_mode,
                state.selected_subagent_id,
                state.input_history.clone(),
            )
        };
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = draft.to_owned();
            state.cursor_position = 7;
            state.draft_submit_mode = DraftSubmitMode::Queue;
        }
        let before = draft_of(&*runtime.app_state().await);
        // `/remote` is entered through the composer, so it replaces the draft
        // like any command; restore it to what the user was typing.
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = draft.to_owned();
            state.cursor_position = 7;
            state.draft_submit_mode = DraftSubmitMode::Queue;
            state.input_history.clear();
        }

        // Remote text is never a command, and refusing it changes nothing.
        let refused = send(&mut runtime, &link, &registration, submit("/exit")).await;
        assert_eq!(rejection(&refused), RemoteErrorCode::UnsupportedOperation);
        assert_eq!(provider.requests(), 0);

        let accepted = send(&mut runtime, &link, &registration, submit("from the phone")).await;
        assert_eq!(accepted.receipt, Some(ReceiptState::Applied));
        assert_eq!(
            accepted.result,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Started
            }
        );
        assert_eq!(draft_of(&*runtime.app_state().await), before);

        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, turn_ended).await;
        let published = events(&received);
        // The turn is announced once: by a `turn_started` event, or, when the
        // orchestrator got there before the first publication, by the
        // snapshot that already shows it running.
        let started = published
            .iter()
            .filter(|(_, event)| matches!(event, RemoteEvent::TurnStarted { .. }))
            .count();
        let announced_by_snapshot = received.iter().any(|message| {
            matches!(message, OwnerMessage::Snapshot { snapshot, .. } if snapshot.turn.is_some())
        });
        if started == 1 {
            assert!(matches!(
                published.first(),
                Some((_, RemoteEvent::TurnStarted { turn_id: Some(_), prompt, timing: Some(_), })) if prompt.text == "from the phone"
            ));
        } else {
            assert!(started == 0 && announced_by_snapshot, "{published:?}");
        }
        // Snapshot text plus the deltas after it is the response, once.
        let mut text = String::new();
        for message in &received {
            match message {
                OwnerMessage::Snapshot { snapshot, .. } => {
                    if let Some(turn) = &snapshot.turn {
                        text = turn.live_response.text.clone();
                    }
                }
                OwnerMessage::Event(frame) => {
                    if let RemoteEvent::TextDelta { text: delta, .. } = &frame.event {
                        text.push_str(delta);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(text, PROVIDER_TEXT);

        let state = runtime.app_state().await;
        assert_eq!(provider.requests(), 1, "the prompt ran exactly once");
        assert_eq!(
            state
                .history
                .iter()
                .filter(|message| message.role == "user" && message.content == "from the phone")
                .count(),
            1
        );
        assert_eq!(draft_of(&state), before);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_provider_failure_retains_authoritative_terminal_timing() {
        let provider = FakeProvider::start_mode(false, true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 64);
        let (_, mut link, _) = share(&mut runtime, &gateway).await;
        enter(&mut runtime, "fail this turn").await;
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, turn_ended).await;
        let failed = events(&received)
            .into_iter()
            .find_map(|(_, event)| match event {
                RemoteEvent::TurnFinished {
                    timing: Some(timing),
                    ..
                } => Some(timing),
                _ => None,
            })
            .expect("failed turn exports its timing");
        assert_eq!(
            failed.outcome,
            Some(rustcode::remote::protocol::RemoteTurnOutcome::Failed)
        );
        assert!(failed.ended_at.is_some() && failed.elapsed_work_ms.is_some());
        let state = runtime.app_state().await;
        assert!(state.active_turn_id.is_none());
        assert!(state.history.iter().any(|message| {
            message.turn.as_ref().is_some_and(|turn| {
                turn.turn_id == failed.turn_id
                    && turn.outcome == Some(rustcode::app::TurnOutcome::Failed)
            })
        }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_queue_and_steer_follow_the_running_turn_rules() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        let queue = || RemoteOperation::Queue {
            prompt: "afterwards".to_owned(),
        };
        let steer = || RemoteOperation::Steer {
            prompt: "change course".to_owned(),
        };

        // Idle: there is nothing to queue behind or to steer.
        let idle_queue = send(&mut runtime, &link, &registration, queue()).await;
        assert_eq!(rejection(&idle_queue), RemoteErrorCode::NotRunning);
        let idle_steer = send(&mut runtime, &link, &registration, steer()).await;
        assert_eq!(rejection(&idle_steer), RemoteErrorCode::NotRunning);

        enter(&mut runtime, "terminal prompt").await;
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.current_response.is_empty() {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the terminal's turn is streaming");
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = "terminal draft".to_owned();
            state.cursor_position = 3;
        }

        // Running: a new prompt is refused, a steer is accepted only by a
        // turn that takes steering, and a queued prompt waits its turn.
        let busy = send(&mut runtime, &link, &registration, submit("now")).await;
        assert_eq!(rejection(&busy), RemoteErrorCode::Busy);
        let steerable = runtime.app_state().await.can_accept_steer();
        let steered = send(&mut runtime, &link, &registration, steer()).await;
        if steerable {
            assert_eq!(steered.receipt, Some(ReceiptState::Applied));
        } else {
            assert_eq!(rejection(&steered), RemoteErrorCode::UnsupportedOperation);
            assert!(runtime.app_state().await.pending_steers.is_empty());
        }
        let queued = send(&mut runtime, &link, &registration, queue()).await;
        assert_eq!(
            queued.result,
            RemoteResult::PromptAccepted {
                disposition: PromptDisposition::Queued
            }
        );
        {
            let state = runtime.app_state().await;
            assert_eq!(state.pending_queue, ["afterwards"]);
            assert_eq!(state.input_buffer, "terminal draft");
            assert_eq!(state.cursor_position, 3);
        }
        assert_eq!(provider.requests(), 1, "queueing starts nothing");

        provider.release();
        // An accepted steer runs as a turn of its own between the terminal's
        // prompt and the queued one. Once released, the provider answers at
        // once, so two of the turns may end within one iteration of the loop:
        // wait for all of them, not for a count the loop might step over.
        let turns = if steerable { 3 } else { 2 };
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, |messages| {
            events(messages)
                .iter()
                .filter(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
                .count()
                >= turns
        })
        .await;
        let finished = events(&received)
            .iter()
            .filter(|(_, event)| matches!(event, RemoteEvent::TurnFinished { .. }))
            .count();
        assert_eq!(finished, turns, "every accepted prompt ran exactly once");
        assert_eq!(provider.requests(), turns);
        let state = runtime.app_state().await;
        let prompts: Vec<&str> = state
            .history
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.as_str())
            .collect();
        let expected: &[&str] = if steerable {
            &["terminal prompt", "change course", "afterwards"]
        } else {
            &["terminal prompt", "afterwards"]
        };
        assert_eq!(prompts, expected);
        assert_eq!(state.input_buffer, "terminal draft");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_remote_cancel_stops_only_the_turn_it_names() {
        let provider = FakeProvider::start(true).await;
        let (mut runtime, gateway) = shared_runtime(provider.state(), 256);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;
        let cancel = |turn_id: &str| RemoteOperation::CancelTurn {
            turn_id: turn_id.to_owned(),
        };
        let streaming_turn = |runtime: &AppRuntime| {
            let state = Arc::clone(&runtime.app_state);
            async move {
                tokio::time::timeout(WAIT, async {
                    loop {
                        let state = state.lock().await;
                        if !state.current_response.is_empty()
                            && let Some(turn_id) = state.active_turn_id.clone()
                        {
                            break turn_id;
                        }
                        drop(state);
                        tokio::time::sleep(EVENT_POLL_INTERVAL).await;
                    }
                })
                .await
                .expect("a turn is streaming")
            }
        };

        enter(&mut runtime, "first").await;
        let first = streaming_turn(&runtime).await;
        {
            let mut state = runtime.app_state.lock().await;
            state.input_buffer = "terminal draft".to_owned();
            state.cursor_position = 5;
        }
        let stale = send(&mut runtime, &link, &registration, cancel("turn:0:0")).await;
        assert_eq!(rejection(&stale), RemoteErrorCode::StaleTurn);
        assert!(runtime.app_state().await.orchestrator_running);

        let cancelled = send(&mut runtime, &link, &registration, cancel(&first)).await;
        assert_eq!(cancelled.receipt, Some(ReceiptState::Applied));
        let mut received = Vec::new();
        publish_until(&mut runtime, &mut link, &mut received, |messages| {
            events(messages).iter().any(|(_, event)| {
                matches!(event, RemoteEvent::TurnCancelled { turn_id , ..} if turn_id.as_deref() == Some(first.as_str()))
            }) || messages.iter().any(|message| matches!(message,
                OwnerMessage::Snapshot { snapshot, .. } if snapshot.last_turn.as_ref().is_some_and(|turn|
                    turn.turn_id == first && turn.outcome == Some(rustcode::remote::protocol::RemoteTurnOutcome::Cancelled))))
        })
        .await;
        {
            let state = runtime.app_state().await;
            assert_eq!(state.input_buffer, "terminal draft");
            assert_eq!(state.cursor_position, 5);
            assert_eq!(state.active_turn_id, None);
        }

        // The terminal stops the next turn itself. A remote cancel that still
        // names it is stale and leaves the replacement token alone.
        tokio::time::timeout(WAIT, async {
            while runtime.app_state().await.orchestrator_running {
                tokio::time::sleep(EVENT_POLL_INTERVAL).await;
            }
        })
        .await
        .expect("the cancelled turn unwinds");
        enter(&mut runtime, "second").await;
        let second = streaming_turn(&runtime).await;
        runtime
            .handle_event(AppEvent::CancelActiveTurn)
            .await
            .expect("Esc is handled");
        let token = runtime.current_cancel_token.clone();
        let late = send(&mut runtime, &link, &registration, cancel(&second)).await;
        assert_eq!(rejection(&late), RemoteErrorCode::StaleTurn);
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn a_full_outbound_queue_resyncs_without_stalling_the_terminal() {
        // Room for the registration and two more messages; nothing reads.
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 3);
        let (_, mut link, _) = share(&mut runtime, &gateway).await;
        let ask = |runtime: &AppRuntime, text: &str| {
            let question = PendingQuestion::new(text.to_owned(), vec!["Yes".to_owned()], false);
            let id = question.id.clone();
            let state = Arc::clone(&runtime.app_state);
            async move {
                state.lock().await.pending_question = Some(question);
                id
            }
        };

        let mut last_question = String::new();
        for round in 0..40 {
            last_question = ask(&runtime, &format!("Question {round}?")).await;
            tokio::time::timeout(Duration::from_secs(1), runtime.run_remote_iteration())
                .await
                .expect("a full link never blocks the loop");
        }
        // The terminal still takes its own input meanwhile.
        runtime
            .handle_event(AppEvent::RequestDraw)
            .await
            .expect("terminal input is handled");

        let mut delivered = Vec::new();
        while let Ok(message) = link.messages.try_recv() {
            delivered.push(message);
        }
        assert_eq!(delivered.len(), 3, "the queue held its bound");
        let last_delivered = events(&delivered)
            .last()
            .map_or(0, |(sequence, _)| *sequence);

        // With room again the owner resyncs: no event in between, then a
        // snapshot that says so and whose watermark shows the gap.
        runtime.run_remote_iteration().await;
        let Ok(OwnerMessage::Snapshot { snapshot, resync }) = link.messages.try_recv() else {
            panic!("the owner resyncs with a snapshot");
        };
        assert_eq!(resync, Some(ResyncReason::Lagged));
        assert!(snapshot.sequence > last_delivered + 1);
        assert_eq!(
            snapshot
                .pending_question
                .as_ref()
                .map(|question| question.question_id.as_str()),
            Some(last_question.as_str())
        );
        assert!(link.messages.try_recv().is_err());

        // Events resume right after the snapshot's watermark.
        let next_question = ask(&runtime, "One more?").await;
        runtime.run_remote_iteration().await;
        let mut resumed = Vec::new();
        while let Ok(message) = link.messages.try_recv() {
            resumed.push(message);
        }
        let resumed = events(&resumed);
        assert_eq!(resumed[0].0, snapshot.sequence + 1);
        assert!(matches!(
            &resumed[1],
            (_, RemoteEvent::QuestionRequested { question }) if question.question_id == next_question
        ));
    }

    #[tokio::test]
    async fn remote_off_drops_the_registration_and_rejects_what_was_queued() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;

        // The command reached the owner's queue before `/remote off` ran.
        let queued = link
            .try_request(request(&registration, submit("too late")))
            .expect("queued");
        enter(&mut runtime, "/remote off").await;
        runtime.run_remote_iteration().await;

        let response = queued.await.expect("the owner replied");
        assert_eq!(rejection(&response), RemoteErrorCode::StaleSession);
        assert!(runtime.remote.registration().is_none());
        assert!(runtime.app_state().await.pending_queue.is_empty());
        assert!(panel(&runtime).await.contains("Remote sharing is off"));
        assert_eq!(
            link.messages.recv().await,
            Some(OwnerMessage::Unregister {
                reason: SessionCloseReason::SharingDisabled
            })
        );
        assert_eq!(link.messages.recv().await, None, "the link is closed");
        assert!(
            link.try_request(request(&registration, submit("x")))
                .is_err()
        );

        // Sharing again is a new registration; the old epoch stays dead.
        let (again, new_link, _) = share(&mut runtime, &gateway).await;
        assert_eq!(again.session_id, registration.session_id);
        assert!(again.registration_epoch > registration.registration_epoch);
        let old = new_link
            .try_request(request(&registration, submit("old epoch")))
            .expect("queued");
        runtime.run_remote_iteration().await;
        assert_eq!(
            rejection(&old.await.expect("the owner replied")),
            RemoteErrorCode::StaleSession
        );
        assert!(runtime.app_state().await.pending_queue.is_empty());
    }

    #[tokio::test]
    async fn a_session_switch_drops_the_registration_and_stays_private() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 16);
        let (registration, mut link, _) = share(&mut runtime, &gateway).await;

        let queued = link
            .try_request(request(&registration, submit("for the old session")))
            .expect("queued");
        runtime
            .handle_event(AppEvent::NewSession)
            .await
            .expect("the terminal starts a new session");
        let new_session = runtime.app_state().await.active_session_id.clone();
        assert_ne!(new_session, registration.session_id);
        runtime.run_remote_iteration().await;

        assert_eq!(
            rejection(&queued.await.expect("the owner replied")),
            RemoteErrorCode::StaleSession
        );
        assert!(runtime.app_state().await.pending_queue.is_empty());
        assert!(runtime.remote.registration().is_none());
        // Nothing about the new session went out under the old registration.
        assert_eq!(
            link.messages.recv().await,
            Some(OwnerMessage::Unregister {
                reason: SessionCloseReason::SessionChanged
            })
        );
        assert_eq!(link.messages.recv().await, None);

        // Activity in the new session publishes nowhere until it is shared.
        runtime
            .app_state
            .lock()
            .await
            .history
            .push(ChatMessage::new("user", "private"));
        runtime.run_remote_iteration().await;
        assert!(gateway.accept().is_none());
        enter(&mut runtime, "/remote status").await;
        runtime.run_remote_iteration().await;
        assert!(panel(&runtime).await.contains("This session is private"));

        let (again, _link, snapshot) = share(&mut runtime, &gateway).await;
        assert_eq!(again.session_id, new_session);
        assert_eq!(snapshot.session.session_id, new_session);
        assert!(again.registration_epoch > registration.registration_epoch);
    }

    fn pending_question(state: &mut AppState) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let question = PendingQuestion::new(
            "Proceed?".to_owned(),
            vec!["Yes".to_owned(), "No".to_owned()],
            false,
        );
        let id = question.id.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.status = AppStatus::AwaitingQuestion;
        state.pending_question = Some(question);
        state.question_response = Some(tx);
        (id, rx)
    }

    fn answer(question_id: &str, option: &str) -> RemoteOperation {
        RemoteOperation::AnswerQuestion {
            question_id: question_id.to_owned(),
            answer: RemoteAnswer::Selected {
                options: vec![option.to_owned()],
            },
        }
    }

    #[tokio::test]
    async fn remote_commands_and_question_cancel_pass_through_the_owner_loop() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 64);
        let (registration, link, snapshot) = share(&mut runtime, &gateway).await;
        assert!(
            snapshot
                .capabilities
                .iter()
                .any(|value| value == "execute_command")
        );
        let (question_id, answered) = pending_question(&mut *runtime.app_state.lock().await);
        let old_token = runtime.current_cancel_token.clone();
        let response = send(
            &mut runtime,
            &link,
            &registration,
            RemoteOperation::ExecuteCommand {
                command: "/status".into(),
            },
        )
        .await;
        assert!(matches!(
            response.result,
            RemoteResult::CommandExecuted { output, .. } if output.contains("Turn: active")
        ));
        assert!(runtime.app_state().await.pending_question.is_some());
        let response = send(
            &mut runtime,
            &link,
            &registration,
            RemoteOperation::CancelQuestion {
                question_id: question_id.clone(),
            },
        )
        .await;
        assert_eq!(response.receipt, Some(ReceiptState::Applied));
        assert!(matches!(
            response.result,
            RemoteResult::QuestionCancelled { .. }
        ));
        assert_eq!(answered.await.unwrap(), "User cancelled prompt.");
        assert!(old_token.is_cancelled());
        assert!(!runtime.current_cancel_token.is_cancelled());
        assert!(runtime.app_state().await.pending_question.is_none());
        let stale = send(
            &mut runtime,
            &link,
            &registration,
            RemoteOperation::CancelQuestion { question_id },
        )
        .await;
        assert_eq!(rejection(&stale), RemoteErrorCode::StaleQuestion);
    }

    #[tokio::test]
    async fn the_first_answer_to_a_question_wins_whichever_side_gives_it() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 64);
        let (registration, link, _) = share(&mut runtime, &gateway).await;

        // The remote device answers first; the terminal's answer is late.
        let (question_id, mut answered) = pending_question(&mut *runtime.app_state.lock().await);
        let remote = send(
            &mut runtime,
            &link,
            &registration,
            answer(&question_id, "Yes"),
        )
        .await;
        assert_eq!(remote.receipt, Some(ReceiptState::Applied));
        let token = runtime.current_cancel_token.clone();
        for late in [
            QuestionAnswer::Selected("No".to_owned()),
            QuestionAnswer::Cancelled,
        ] {
            runtime
                .handle_event(AppEvent::AnswerQuestion(late))
                .await
                .expect("the terminal event is handled");
        }
        assert_eq!(answered.try_recv().as_deref(), Ok("User selected: Yes"));
        assert!(!token.is_cancelled(), "a late cancel stops nothing");
        assert_eq!(
            runtime.app_state().await.active_transient_notice(),
            Some("That question was already answered")
        );

        // The terminal answered first: its key press is already queued when
        // the remote answer arrives, so the remote one is the stale one.
        let (question_id, mut answered) = pending_question(&mut *runtime.app_state.lock().await);
        let _ = runtime
            .app_event_sender
            .send(AppEvent::AnswerQuestion(QuestionAnswer::Selected(
                "No".to_owned(),
            )));
        let remote = link
            .try_request(request(&registration, answer(&question_id, "Yes")))
            .expect("queued");
        runtime.run_remote_iteration().await;
        assert!(answered.try_recv().is_err(), "the remote answer waits");
        let terminal = runtime
            .app_event_receiver
            .try_recv()
            .expect("the terminal's answer is queued");
        runtime
            .handle_event(terminal)
            .await
            .expect("the terminal event is handled");
        runtime.run_remote_iteration().await;
        assert_eq!(answered.try_recv().as_deref(), Ok("User selected: No"));
        assert_eq!(
            rejection(&remote.await.expect("the owner replied")),
            RemoteErrorCode::StaleQuestion
        );
    }

    #[tokio::test]
    async fn a_late_terminal_denial_cannot_undo_a_remote_approval() {
        let (mut runtime, gateway) = shared_runtime(AppState::new(), 64);
        let (registration, link, _) = share(&mut runtime, &gateway).await;
        let (tx, mut decided) = tokio::sync::oneshot::channel();
        {
            let mut state = runtime.app_state.lock().await;
            state.status = AppStatus::AwaitingToolConfirmation;
            state.pending_tool_confirmation = Some(Vec::new());
            state.pending_approval_batch_id = Some("batch-1".to_owned());
            state.tool_confirmation_response = Some(tx);
        }
        let resolve = |choice| RemoteOperation::ResolveApproval {
            batch_id: "batch-1".to_owned(),
            choice,
        };

        let approved = send(
            &mut runtime,
            &link,
            &registration,
            resolve(ApprovalChoice::Approve),
        )
        .await;
        assert_eq!(approved.receipt, Some(ReceiptState::Applied));
        let token = runtime.current_cancel_token.clone();
        runtime
            .handle_event(AppEvent::ApprovalDecision(
                rustcode::app::ApprovalDecision::Deny,
            ))
            .await
            .expect("the terminal event is handled");

        assert_eq!(
            decided.try_recv(),
            Ok(rustcode::app::ToolConfirmationResponse::Approve)
        );
        assert!(!token.is_cancelled(), "the approved turn keeps running");
        let second = send(
            &mut runtime,
            &link,
            &registration,
            resolve(ApprovalChoice::Deny),
        )
        .await;
        assert_eq!(rejection(&second), RemoteErrorCode::StaleApproval);
    }
}

#[cfg(test)]
mod standby_title_tests {
    use super::standby_title;
    use rustcode::app::{AppState, ChatMessage};

    #[test]
    fn title_comes_from_the_open_session_without_listing_the_store() {
        let mut state = AppState::new();
        // Nothing said yet: the id is the only name there is.
        assert_eq!(standby_title(&state), state.active_session_id);

        state
            .history
            .push(ChatMessage::new("user", "Fix the flaky scheduler test"));
        state.history.push(ChatMessage::new("assistant", "On it."));
        assert_eq!(
            standby_title(&state),
            rustcode::config::session_title(&state.history)
        );
    }
}
