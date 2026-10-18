//! `rustcode remote …` commands: argument handling and terminal output.

use super::address::{candidate_addresses, plan_listen_with_preference};
use super::control::{GatewayStatus, OfferDetails};
use super::devices::DeviceRecord;
use super::gateway::{Gateway, GatewayConfig, Limits};
use super::hub::{HubLimits, SessionHub};
use super::lifecycle::{RemoteLifecycle, Revocation};
use super::pairing_watch::{self, PairingWatch, format_progress};
use super::qr;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// What to do when the gateway only listens on loopback. Shown by `/remote`
/// and by the `rustcode remote` commands, because a phone cannot reach
/// loopback and nothing else tells the user why pairing does not work.
/// `markdown` is for the terminal UI's panel, which renders Markdown and
/// would reflow the commands and rewrite their punctuation: there each one
/// is a list item in a code span, which is shown verbatim.
pub fn loopback_guidance(markdown: bool) -> String {
    let block = |lines: &[&str]| {
        lines
            .iter()
            .map(|line| {
                if markdown {
                    format!("- `{line}`")
                } else {
                    format!("  {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut blocks = vec![
        "The gateway listens on this machine only (loopback), so a phone cannot reach it."
            .to_owned(),
        "Join the same Wi-Fi as your phone, then restart with automatic LAN selection:".to_owned(),
        block(&["rustcode remote stop", "rustcode remote serve"]),
        "If config.toml explicitly sets a loopback bind, remove it or use:".to_owned(),
        block(&["[remote]", "bind = \"auto\""]),
    ];
    let candidates = candidate_addresses();
    if candidates.is_empty() {
        blocks.push("No LAN or NetBird address was found on this machine.".to_owned());
    } else {
        blocks.push("Addresses of this machine:".to_owned());
        let lines: Vec<String> = candidates
            .iter()
            .map(|candidate| format!("{}  ({})", candidate.address, candidate.interface))
            .collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        blocks.push(block(&lines));
    }
    blocks.push(
        "A NetBird address is the recommended choice. Plain LAN traffic is authenticated but not encrypted."
            .to_owned(),
    );
    blocks.join(if markdown { "\n\n" } else { "\n" })
}

/// Run the gateway in the foreground until Ctrl-C, SIGTERM or
/// `rustcode remote stop`.
pub async fn serve(
    config_directory: &Path,
    bind: &str,
    port: u16,
    advertise: Option<&str>,
    prefer_interface: Option<&str>,
) -> Result<()> {
    let plan =
        plan_listen_with_preference(bind, advertise, &candidate_addresses(), prefer_interface)?;
    let lifecycle = RemoteLifecycle::new(config_directory);
    let hub = SessionHub::start(HubLimits::default());
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan.clone(),
            // The address is a result of the arguments, never a setting: a
            // lease that changes moves the listener instead of stranding it.
            replan: Some({
                let (bind, advertise, prefer_interface) = (
                    bind.to_owned(),
                    advertise.map(str::to_owned),
                    prefer_interface.map(str::to_owned),
                );
                Arc::new(move || {
                    plan_listen_with_preference(
                        &bind,
                        advertise.as_deref(),
                        &candidate_addresses(),
                        prefer_interface.as_deref(),
                    )
                    .ok()
                })
            }),
            port,
            router: std::sync::Arc::new(super::workspace::WorkspaceRouter::new(
                hub.clone(),
                config_directory.to_path_buf(),
                std::env::current_exe()?,
            )),
            sessions: Some(hub),
            limits: Limits::default(),
        },
    )
    .await?;
    let port = gateway.local_addr().port();
    let gateway_id = gateway.gateway_id().to_owned();
    let mut discovery =
        super::discovery::advertise(port, plan.advertise.is_loopback(), &gateway_id);
    // Bonjour is announced only while a phone could reach the address.
    let mut advertised = gateway.advertised_changes();
    let announce = async move {
        let mut loopback = plan.advertise.is_loopback();
        while advertised.changed().await.is_ok() {
            let now = advertised.borrow_and_update().host().is_loopback();
            if now != loopback {
                loopback = now;
                discovery = super::discovery::advertise(port, loopback, &gateway_id);
            }
        }
        // Keeps the announcement up when the address can no longer change.
        let _discovery = discovery;
        std::future::pending::<()>().await
    };
    let plan = ListenPlanSummary {
        loopback_bind: gateway.local_addr().ip().is_loopback(),
        loopback_advertise: gateway.advertised_address().host().is_loopback(),
    };
    println!("rustcode remote gateway");
    println!("listening on  {}", gateway.local_addr());
    println!("advertised as {}", gateway.advertised_address());
    println!("gateway id    {}", gateway.gateway_id());
    if !plan.loopback_bind {
        println!(
            "warning: plain WebSocket on a non-loopback address is authenticated but not encrypted; use it only on a trusted network such as NetBird."
        );
    }
    if plan.loopback_advertise {
        println!("{}", loopback_guidance(false));
    }
    println!(
        "Share a session with `/remote` in a terminal session; pair a device with `rustcode remote pair`."
    );

    let program = super::service::executable_path()?;
    let upgraded = super::service::wait_for_upgrade(program.clone());
    tokio::pin!(upgraded);
    let shutdown = gateway.shutdown_token();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let run = gateway.run();
    tokio::pin!(run);
    tokio::pin!(announce);
    let restart = tokio::select! {
        result = &mut run => return result,
        _ = &mut announce => false,
        _ = tokio::signal::ctrl_c() => false,
        _ = terminate.recv() => false,
        _ = &mut upgraded => true,
    };
    // Let the gateway close its connections and remove its registration.
    shutdown.cancel();
    run.await?;
    if restart && std::env::var_os("RUSTCODE_REMOTE_MANAGED").is_none() {
        let launcher = super::owner_client::Launcher {
            program,
            config_directory: config_directory.to_path_buf(),
            bind: Some(bind.into()),
            port: Some(port),
            advertise: advertise.map(str::to_owned),
            prefer_interface: prefer_interface.map(str::to_owned),
        };
        lifecycle.start(launcher.command()).await?;
    }
    Ok(())
}

struct ListenPlanSummary {
    loopback_bind: bool,
    loopback_advertise: bool,
}

/// How a pairing QR code is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrStyle {
    /// ANSI colours, for a terminal.
    Ansi,
    /// Marked rows for the terminal UI's command panel.
    Panel,
    /// No drawing: the payload is printed as text (output is not a terminal).
    Text,
}

/// Text for a freshly issued pairing challenge. This and the `/remote` panel
/// built from it are the only places the pairing secrets are shown.
pub fn format_offer(offer: &OfferDetails, style: QrStyle) -> String {
    let panel = style == QrStyle::Panel;
    let mut blocks = Vec::new();
    if offer.loopback_only && !panel {
        blocks.push(loopback_guidance(panel));
    }
    let code = match style {
        QrStyle::Ansi => qr::ansi(offer.qr_payload.expose()),
        QrStyle::Panel => qr::panel(offer.qr_payload.expose()),
        QrStyle::Text => None,
    };
    let expires_at = chrono::Local::now()
        + chrono::Duration::seconds(offer.expires_in_secs.try_into().unwrap_or(i64::MAX));
    let lifetime = format!(
        "Pair a device within {} seconds, by {} (single use, {} wrong codes allowed).",
        offer.expires_in_secs,
        expires_at.format("%H:%M:%S"),
        super::pairing::ATTEMPTS_PER_CHALLENGE
    );
    if let Some(locked) = offer.locked_for_secs {
        blocks.push(format!(
            "Pairing is locked for {locked} s after too many wrong codes on this host: \
             even this code is refused until then."
        ));
    }
    let by_hand = format!(
        "  address:  {}\n  code:     {}",
        offer.advertised_address,
        offer.code.expose()
    );
    match code {
        Some(code) if panel => {
            blocks.push(format!("{lifetime}\n{by_hand}"));
            blocks.push(code);
            blocks.push("Scan the QR in the app, or enter the address and code above.".to_owned());
        }
        Some(code) => {
            blocks.push(format!("{lifetime} Scan this code in the app:"));
            blocks.push(code);
            blocks.push(format!("Or enter by hand:\n{by_hand}"));
        }
        None => blocks.push(format!(
            "{lifetime}\n  QR payload: {}\n{by_hand}",
            offer.qr_payload.expose()
        )),
    }
    if offer.loopback_only && panel {
        blocks.push(loopback_guidance(true));
    }
    blocks.push("Asking for pairing details again replaces this challenge.".to_owned());
    blocks.join("\n\n")
}

/// `/remote`: open the command panel under `header` at once, then return the
/// work that asks the running gateway for a pairing challenge, shows it and
/// counts it down. The caller runs that on its own task, so the terminal
/// never waits on the control socket; the panel is only updated while it is
/// still the one this request opened.
pub async fn show_pairing_details(
    state: Arc<tokio::sync::Mutex<crate::app::AppState>>,
    title: &'static str,
    header: String,
    config_directory: std::path::PathBuf,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let generation = state.lock().await.show_command_panel_request(
        title,
        format!("{header}\n\nAsking the gateway for pairing details…"),
    );
    async move {
        follow_pairing_details(state, title, generation, header, config_directory).await;
    }
}

async fn follow_pairing_details(
    state: Arc<tokio::sync::Mutex<crate::app::AppState>>,
    title: &'static str,
    generation: u64,
    header: String,
    config_directory: std::path::PathBuf,
) {
    let lifecycle = RemoteLifecycle::new(config_directory);
    let before = lifecycle.status().await.ok().flatten();
    let offer = match lifecycle.pair().await {
        Ok(offer) => offer,
        Err(error) => {
            state.lock().await.update_command_panel_if_current(
                title,
                generation,
                format!("Pairing details are not available: {error:#}\n\n{header}"),
            );
            return;
        }
    };
    let details = format_offer(&offer, QrStyle::Panel);
    if !state.lock().await.update_command_panel_if_current(
        title,
        generation,
        format!("{details}\n\n{header}"),
    ) {
        return;
    }
    // Count the challenge down in the panel, and say how it ended, for as
    // long as the panel stays open.
    let watch = PairingWatch::new(&offer, before.as_ref());
    loop {
        tokio::time::sleep(pairing_watch::REFRESH).await;
        let progress = match lifecycle.status().await {
            Ok(status) => watch.observe(status.as_ref()),
            // A slow control socket is not an answer; ask again.
            Err(_) => continue,
        };
        let Some(line) = format_progress(&progress, "/remote pair") else {
            return;
        };
        let content = if progress.is_final() {
            format!("{line}\n\n{header}")
        } else {
            format!("{line}\n\n{details}\n\n{header}")
        };
        if !state
            .lock()
            .await
            .update_command_panel_if_current(title, generation, content)
            || progress.is_final()
        {
            return;
        }
    }
}

/// `rustcode remote pair` in a terminal: after the details, one line that
/// counts the challenge down and says how it ended. Returns once it ended or
/// on Ctrl-C.
pub async fn watch_pairing_in_terminal(
    lifecycle: &RemoteLifecycle,
    offer: &OfferDetails,
    before: Option<&GatewayStatus>,
) {
    use std::io::Write as _;
    let watch = PairingWatch::new(offer, before);
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    loop {
        tokio::select! {
            _ = &mut interrupted => {
                println!();
                return;
            }
            _ = tokio::time::sleep(pairing_watch::REFRESH) => {}
        }
        let Ok(status) = lifecycle.status().await else {
            continue;
        };
        let progress = watch.observe(status.as_ref());
        let Some(line) = format_progress(&progress, "`rustcode remote pair`") else {
            return;
        };
        // Rewrite the same line: carriage return, then clear it.
        print!("\r\x1b[2K{line}");
        if progress.is_final() {
            println!();
            return;
        }
        let _ = std::io::stdout().flush();
    }
}

/// Table of paired devices. Carries identifiers and labels, never a token.
pub fn format_devices(devices: &[DeviceRecord]) -> String {
    if devices.is_empty() {
        return "No paired devices.".to_string();
    }
    let now = chrono::Utc::now();
    let mut lines = vec![format!(
        "{:<16}  {:<20}  {:<20}  NAME",
        "DEVICE", "PAIRED (UTC)", "EXPIRES (UTC)"
    )];
    for device in devices {
        let expires = if device.is_expired(now) {
            "expired".to_owned()
        } else {
            device.expires_at().format("%Y-%m-%d %H:%M:%S").to_string()
        };
        lines.push(format!(
            "{:<16}  {:<20}  {:<20}  {}",
            device.id,
            device.paired_at.format("%Y-%m-%d %H:%M:%S"),
            expires,
            device.name
        ));
    }
    lines.join("\n")
}

/// JSON view of the paired devices, without the stored token digests.
pub fn devices_json(devices: &[DeviceRecord]) -> serde_json::Value {
    devices
        .iter()
        .map(|device| {
            serde_json::json!({
                "id": device.id,
                "name": device.name,
                "paired_at": device.paired_at,
                "expires_at": device.expires_at(),
            })
        })
        .collect()
}

pub fn format_revocation(revocation: &Revocation) -> String {
    let connections = if revocation.gateway_running {
        format!(
            "closed {} live connection(s)",
            revocation.closed_connections
        )
    } else {
        "the gateway is not running".to_string()
    };
    format!(
        "Revoked {} ({}); {connections}.",
        revocation.device_name, revocation.device_id
    )
}

pub fn format_status(status: Option<&GatewayStatus>) -> String {
    format_status_for(status, None)
}

/// [`format_status`], with `this_session` (the terminal asking) marked in
/// the session list.
pub fn format_status_for(status: Option<&GatewayStatus>, this_session: Option<&str>) -> String {
    let Some(status) = status else {
        return "Remote gateway: stopped".to_string();
    };
    let pairing = match &status.pairing {
        Some(pairing) => match (
            pairing.locked_for_secs,
            pairing.expires_in_secs,
            pairing.attempts_left,
        ) {
            (Some(locked), _, _) => {
                format!("locked for {locked} s after too many wrong codes")
            }
            (None, Some(expires), Some(attempts)) => {
                format!("challenge open ({expires} s left, {attempts} wrong codes allowed)")
            }
            _ => "closed (run `rustcode remote pair` for a new code)".to_string(),
        },
        None if status.pairing_open => "challenge open".to_string(),
        None => "closed".to_string(),
    };
    let bind_mode = match status.automatic_bind {
        Some(true) => "  (auto: follows address changes)",
        Some(false) => "  (fixed bind)",
        None => "",
    };
    let advertise_mode = match status.fixed_advertise {
        Some(true) => "  (fixed --advertise)",
        _ => "",
    };
    let mut lines = vec![
        format!("Remote gateway: running (pid {})", status.pid),
        format!("  listening on   {}{bind_mode}", status.listen_address),
        format!(
            "  advertised as  {}{advertise_mode}",
            status.advertised_address
        ),
    ];
    if !status.candidates.is_empty() {
        lines.push("  addresses now".to_string());
        lines.extend(status.candidates.iter().map(|line| format!("    {line}")));
    }
    for (warning, recovery) in stale_address_warnings(status) {
        lines.push(format!("  warning        {warning}"));
        let mut recovery = recovery.into_iter();
        if let Some(first) = recovery.next() {
            lines.push(format!("  recover        {first}"));
        }
        lines.extend(recovery.map(|line| format!("                 {line}")));
    }
    lines.extend([
        format!("  gateway id     {}", status.gateway_id),
        format!("  paired devices {}", status.paired_devices),
    ]);
    for device in &status.devices {
        let connected = status
            .connections
            .iter()
            .any(|connection| connection.device_id == device.id);
        lines.push(format!(
            "    {}  {}{}",
            device.id,
            device.name,
            if connected { "  (connected)" } else { "" }
        ));
    }
    lines.push(format!("  pairing        {pairing}"));
    lines.push(format!("  connections    {}", status.connections.len()));
    if status.devices.is_empty() {
        for connection in &status.connections {
            lines.push(format!(
                "    {}  {}",
                connection.device_id, connection.device_name
            ));
        }
    }
    match &status.sessions {
        None => lines.push("  sessions       not supported by this gateway".to_string()),
        Some(sessions) if sessions.is_empty() => {
            lines.push("  sessions       none shared (run /remote in a session)".to_string());
        }
        Some(sessions) => {
            lines.push(format!("  sessions       {}", sessions.len()));
            for session in sessions {
                let attached = if session.attached_devices.is_empty() {
                    "no device attached".to_string()
                } else {
                    format!("attached: {}", session.attached_devices.join(", "))
                };
                let mark = if this_session == Some(session.session_id.as_str()) {
                    "  [this terminal]"
                } else {
                    ""
                };
                lines.push(format!(
                    "    {}  {}  ({attached}){mark}",
                    session.session_id, session.title
                ));
            }
        }
    }
    if !status.unregistered_owners.is_empty() {
        lines.push(format!(
            "  idle owners    {} (running but not shared here; they register again by themselves, or end them with `kill`)",
            status.unregistered_owners.len()
        ));
        for session in &status.unregistered_owners {
            lines.push(format!("    {session}"));
        }
    }
    if let Some(log) = &status.log_path {
        lines.push(format!("  gateway log    {}", log.display()));
    }
    if let Some(logs) = &status.owner_log_directory {
        lines.push(format!("  session logs   {}", logs.display()));
    }
    if status.loopback_only {
        lines.push(String::new());
        lines.push(loopback_guidance(false));
    }
    lines.join("\n")
}

/// Addresses in `status` that this machine no longer has, each with what to
/// run about it. The listener on such an address is unreachable until it
/// moves; an advertised address there sends devices nowhere. A wildcard or
/// loopback address cannot go stale.
fn stale_address_warnings(status: &GatewayStatus) -> Vec<(String, Vec<String>)> {
    if status.candidates.is_empty() {
        return Vec::new();
    }
    let current: Vec<std::net::IpAddr> = status
        .candidates
        .iter()
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect();
    let stale = |address: &str| {
        let ip = address.parse::<std::net::SocketAddr>().ok()?.ip();
        (!ip.is_loopback() && !ip.is_unspecified() && !current.contains(&ip)).then_some(ip)
    };
    // The address to suggest by hand: the first IPv4 one, as `auto` would.
    let suggestion = current
        .iter()
        .find(|ip| ip.is_ipv4())
        .or(current.first())
        .map(ToString::to_string)
        .unwrap_or_else(|| "<address>".to_owned());
    let mut warnings = Vec::new();
    let listen = stale(&status.listen_address);
    if let Some(listen) = listen {
        if status.automatic_bind == Some(true) {
            warnings.push((
                format!(
                    "{listen} is no longer an address of this machine; the bind is auto, so the gateway moves to a current address within seconds"
                ),
                vec![
                    "if it stays here: `rustcode remote stop`, then `/remote` or `rustcode remote serve`"
                        .to_owned(),
                ],
            ));
        } else {
            // `None`: a gateway that does not say how it binds.
            let why = if status.automatic_bind == Some(false) {
                " and the bind is fixed: no device can reach the gateway"
            } else {
                "; an auto bind moves within seconds, a fixed one stays unreachable"
            };
            warnings.push((
                format!("{listen} is no longer an address of this machine{why}"),
                vec![
                    "`rustcode remote stop && rustcode remote serve --bind auto` (follows address changes from now on)".to_owned(),
                    format!("or `--bind {suggestion}` for the address it has now; a `[remote] bind` in config.toml needs the same change"),
                ],
            ));
        }
    }
    if status.fixed_advertise != Some(false)
        && let Some(advertised) = stale(&status.advertised_address)
        && Some(advertised) != listen
    {
        warnings.push((
            format!(
                "devices are told to dial {advertised}, which is no longer an address of this machine"
            ),
            vec![
                format!(
                    "`rustcode remote stop && rustcode remote serve --advertise <stable name or {suggestion}>`"
                ),
                "a `[remote] advertise` in config.toml needs the same change".to_owned(),
            ],
        ));
    }
    warnings
}

/// `/remote status`: the terminal's own sharing state (`header`) in the
/// command panel at once, then the returned work fills in the running
/// gateway's full status. The caller runs that on its own task so the
/// terminal never waits on the control socket. The panel is only updated
/// while it is still the one this request opened.
pub async fn show_gateway_status(
    state: Arc<tokio::sync::Mutex<crate::app::AppState>>,
    title: &'static str,
    header: String,
    config_directory: std::path::PathBuf,
    this_session: Option<String>,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    // Opened before returning: a caller that looks right after `/remote
    // status` sees this terminal's state, not the panel it replaced.
    let generation = state.lock().await.show_command_panel_request(
        title,
        format!("{header}\n\nAsking the gateway for its status…"),
    );
    async move {
        let detail = match RemoteLifecycle::new(config_directory).status().await {
            Ok(status) => format_status_for(status.as_ref(), this_session.as_deref()),
            Err(error) => format!("Remote gateway status is not available: {error:#}"),
        };
        state.lock().await.update_command_panel_if_current(
            title,
            generation,
            format!("{header}\n\n{detail}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_gateway::devices::Devices;

    #[test]
    fn device_listings_never_contain_token_material() {
        let mut devices = Devices::default();
        let (record, token) = devices.enroll("Lars's iPhone").unwrap();
        let stored = serde_json::to_value(&devices).unwrap();
        let digest = stored["devices"][0]["token_sha256"].as_str().unwrap();

        let table = format_devices(devices.list());
        let json = devices_json(devices.list()).to_string();
        for output in [&table, &json] {
            assert!(output.contains(&record.id), "{output}");
            assert!(output.contains("Lars's iPhone"), "{output}");
            assert!(!output.contains(token.expose()), "{output}");
            assert!(!output.contains(digest), "{output}");
        }
        assert_eq!(format_devices(&[]), "No paired devices.");
    }

    #[test]
    fn an_offer_draws_the_code_and_explains_a_loopback_gateway() {
        use crate::remote_gateway::address::AdvertisedAddress;
        use crate::remote_gateway::handshake::Secret;
        use crate::remote_gateway::pairing::PairingOffer;
        let offer = |host: &str| {
            OfferDetails::new(
                &AdvertisedAddress::new(host, 17879).unwrap(),
                "gw",
                Some("studio"),
                PairingOffer {
                    credential: Secret::new("credential-value"),
                    code: Secret::new("1234-5678"),
                    lifetime: std::time::Duration::from_secs(120),
                },
            )
        };
        let lan = format_offer(&offer("192.168.1.20"), QrStyle::Panel);
        assert!(lan.contains("192.168.1.20:17879") && lan.contains("1234-5678"));
        assert!(
            lan.lines()
                .filter(|l| l.starts_with(qr::PANEL_ROW_MARK))
                .count()
                > 10
        );
        assert!(!lan.contains("loopback"));
        // The drawing replaces the raw payload, which carries the credential.
        assert!(!lan.contains("credential-value"));

        let local_panel = format_offer(&offer("127.0.0.1"), QrStyle::Panel);
        let qr_start = local_panel.find(qr::PANEL_ROW_MARK).unwrap();
        assert!(local_panel.find("address:").unwrap() < qr_start);
        assert!(local_panel.find("code:").unwrap() < qr_start);
        assert!(local_panel[..qr_start].lines().count() <= 5);
        assert!(local_panel.find("The gateway listens").unwrap() > qr_start);

        let local = format_offer(&offer("127.0.0.1"), QrStyle::Text);
        assert!(local.starts_with("The gateway listens on this machine only"));
        assert!(local.contains("rustcode remote serve"));
        assert!(local.contains("[remote]"));
        assert!(local.contains("QR payload: "));
    }

    #[test]
    fn status_shows_addresses_devices_pairing_and_logs() {
        use crate::remote_gateway::control::{ConnectionSummary, DeviceSummary, PairingStatus};
        let mut status = GatewayStatus {
            version: "1.0.0".into(),
            pid: 7,
            process_start_time: 1,
            instance_id: "instance".into(),
            gateway_id: "gateway".into(),
            uptime_seconds: 3,
            listen_address: "192.168.31.237:17879".into(),
            advertised_address: "192.168.31.237:17879".into(),
            paired_devices: 2,
            connections: vec![ConnectionSummary {
                device_id: "aa".into(),
                device_name: "phone".into(),
            }],
            pairing_open: true,
            loopback_only: false,
            sessions: Some(Vec::new()),
            candidates: vec!["192.168.31.185  (en0)".into()],
            devices: vec![
                DeviceSummary {
                    id: "aa".into(),
                    name: "phone".into(),
                },
                DeviceSummary {
                    id: "bb".into(),
                    name: "tablet".into(),
                },
            ],
            pairing: Some(PairingStatus {
                expires_in_secs: Some(87),
                attempts_left: Some(4),
                ..PairingStatus::default()
            }),
            unregistered_owners: vec!["session-1".into()],
            log_path: Some("/config/remote/gateway.log".into()),
            owner_log_directory: Some("/config/remote/owners".into()),
            automatic_bind: Some(false),
            fixed_advertise: Some(false),
        };
        let text = format_status(Some(&status));
        for expected in [
            "listening on   192.168.31.237:17879",
            "192.168.31.185  (en0)",
            "192.168.31.237 is no longer an address of this machine and the bind is fixed",
            "recover        `rustcode remote stop && rustcode remote serve --bind auto`",
            "or `--bind 192.168.31.185` for the address it has now",
            "listening on   192.168.31.237:17879  (fixed bind)",
            "aa  phone  (connected)",
            "bb  tablet",
            "challenge open (87 s left, 4 wrong codes allowed)",
            "idle owners    1",
            "gateway log    /config/remote/gateway.log",
            "session logs   /config/remote/owners",
        ] {
            assert!(text.contains(expected), "{expected}\n{text}");
        }
        assert!(!text.contains("bb  tablet  (connected)"), "{text}");

        status.listen_address = "192.168.31.185:17879".into();
        status.pairing.as_mut().unwrap().locked_for_secs = Some(240);
        let text = format_status(Some(&status));
        assert!(!text.contains("warning"), "{text}");
        assert!(text.contains("locked for 240 s"), "{text}");

        // A gateway from before these fields existed still reads cleanly.
        let old = serde_json::json!({
            "pid": 7, "process_start_time": 1, "instance_id": "i", "gateway_id": "g",
            "uptime_seconds": 3, "listen_address": "127.0.0.1:1",
            "advertised_address": "127.0.0.1:1", "paired_devices": 0,
            "connections": [], "pairing_open": false,
        });
        let old: GatewayStatus = serde_json::from_value(old).unwrap();
        assert!(format_status(Some(&old)).contains("pairing        closed"));
    }

    fn running(listen: &str, advertised: &str, candidates: &[&str]) -> GatewayStatus {
        let mut status: GatewayStatus = serde_json::from_value(serde_json::json!({
            "pid": 7, "process_start_time": 1, "instance_id": "i", "gateway_id": "g",
            "uptime_seconds": 3, "listen_address": listen,
            "advertised_address": advertised, "paired_devices": 0,
            "connections": [], "pairing_open": false,
        }))
        .unwrap();
        status.candidates = candidates.iter().map(|line| line.to_string()).collect();
        status
    }

    #[test]
    fn a_stale_address_names_the_command_that_recovers_it() {
        // An auto bind moves by itself; the status says so instead of asking
        // for a restart.
        let mut status = running(
            "192.168.1.20:17879",
            "192.168.1.20:17879",
            &["192.168.1.31  (en0)", "fe80::1  (en0)"],
        );
        status.automatic_bind = Some(true);
        status.fixed_advertise = Some(false);
        let text = format_status(Some(&status));
        assert!(text.contains("(auto: follows address changes)"), "{text}");
        assert!(
            text.contains("the bind is auto, so the gateway moves"),
            "{text}"
        );
        assert!(!text.contains("--bind"), "{text}");
        assert_eq!(text.matches("warning").count(), 1, "{text}");

        // A gateway that does not report its bind mode gets both readings.
        status.automatic_bind = None;
        let text = format_status(Some(&status));
        assert!(text.contains("an auto bind moves within seconds"), "{text}");
        assert!(text.contains("--bind 192.168.1.31"), "{text}");

        // A wildcard bind cannot go stale, but a fixed advertised IP can.
        let mut status = running("0.0.0.0:17879", "10.0.0.5:17879", &["192.168.1.31  (en0)"]);
        status.automatic_bind = Some(false);
        status.fixed_advertise = Some(true);
        let text = format_status(Some(&status));
        assert!(text.contains("(fixed --advertise)"), "{text}");
        assert!(
            text.contains("devices are told to dial 10.0.0.5, which is no longer"),
            "{text}"
        );
        assert!(
            text.contains("--advertise <stable name or 192.168.1.31>"),
            "{text}"
        );
        // A derived advertise follows the plan; a name is never checked.
        status.fixed_advertise = Some(false);
        assert!(!format_status(Some(&status)).contains("warning"));
        status.fixed_advertise = Some(true);
        status.advertised_address = "mac.netbird.cloud:17879".into();
        assert!(!format_status(Some(&status)).contains("warning"));
        // Loopback and an empty address list are never stale.
        let local = running(
            "127.0.0.1:17879",
            "127.0.0.1:17879",
            &["192.168.1.31  (en0)"],
        );
        assert!(!format_status(Some(&local)).contains("warning"));
        let offline = running("192.168.1.20:17879", "192.168.1.20:17879", &[]);
        assert!(!format_status(Some(&offline)).contains("warning"));
    }

    #[test]
    fn the_asking_terminal_finds_its_own_session_marked() {
        use crate::remote_gateway::hub::SessionSummary;
        let mut status = running("127.0.0.1:1", "127.0.0.1:1", &[]);
        let session = |id: &str, attached: &[&str]| SessionSummary {
            session_id: id.to_owned(),
            title: format!("title {id}"),
            attached_devices: attached.iter().map(|name| name.to_string()).collect(),
        };
        status.sessions = Some(vec![session("s1", &["phone"]), session("s2", &[])]);
        let text = format_status_for(Some(&status), Some("s2"));
        assert!(text.contains("s1  title s1  (attached: phone)\n"), "{text}");
        assert!(
            text.contains("s2  title s2  (no device attached)  [this terminal]"),
            "{text}"
        );
        assert!(!format_status(Some(&status)).contains("[this terminal]"));
    }

    #[test]
    fn status_and_revocation_text_make_no_session_claims() {
        assert_eq!(format_status(None), "Remote gateway: stopped");
        let revocation = Revocation {
            device_id: "0011223344556677".into(),
            device_name: "phone".into(),
            closed_connections: 2,
            gateway_running: true,
        };
        assert_eq!(
            format_revocation(&revocation),
            "Revoked phone (0011223344556677); closed 2 live connection(s)."
        );
    }
}
