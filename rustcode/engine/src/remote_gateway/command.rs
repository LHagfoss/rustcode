//! `rustcode remote …` commands: argument handling and terminal output.

use super::address::{candidate_addresses, plan_listen};
use super::control::{GatewayStatus, OfferDetails};
use super::devices::DeviceRecord;
use super::gateway::{Gateway, GatewayConfig, Limits};
use super::hub::{HubLimits, SessionHub};
use super::lifecycle::{RemoteLifecycle, Revocation};
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
) -> Result<()> {
    let plan = plan_listen(bind, advertise, &candidate_addresses())?;
    let lifecycle = RemoteLifecycle::new(config_directory);
    let hub = SessionHub::start(HubLimits::default());
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan.clone(),
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
    let _discovery =
        super::discovery::advertise(gateway.local_addr().port(), plan.advertise.is_loopback());
    println!("rustcode remote gateway");
    println!("listening on  {}", gateway.local_addr());
    println!("advertised as {}", gateway.advertised_address());
    println!("gateway id    {}", gateway.gateway_id());
    if !plan.bind.is_loopback() {
        println!(
            "warning: plain WebSocket on a non-loopback address is authenticated but not encrypted; use it only on a trusted network such as NetBird."
        );
    }
    if plan.advertise.is_loopback() {
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
    let restart = tokio::select! {
        result = &mut run => return result,
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
        };
        lifecycle.start(launcher.command()).await?;
    }
    Ok(())
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
    let lifetime = format!(
        "Pair a device within {} seconds (single use).",
        offer.expires_in_secs
    );
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

/// `/remote`: ask the running gateway for a pairing challenge and show it in
/// the command panel under `header`. Runs on its own task, so the terminal
/// never waits on the control socket; the panel is only updated while it is
/// still the one this request opened.
pub async fn show_pairing_details(
    state: Arc<tokio::sync::Mutex<crate::app::AppState>>,
    title: &'static str,
    header: String,
    config_directory: std::path::PathBuf,
) {
    let generation = state.lock().await.show_command_panel_request(
        title,
        format!("{header}\n\nAsking the gateway for pairing details…"),
    );
    let details = match RemoteLifecycle::new(config_directory).pair().await {
        Ok(offer) => format_offer(&offer, QrStyle::Panel),
        Err(error) => format!("Pairing details are not available: {error:#}"),
    };
    state.lock().await.update_command_panel_if_current(
        title,
        generation,
        format!("{details}\n\n{header}"),
    );
}

/// Table of paired devices. Carries identifiers and labels, never a token.
pub fn format_devices(devices: &[DeviceRecord]) -> String {
    if devices.is_empty() {
        return "No paired devices.".to_string();
    }
    let mut lines = vec![format!("{:<16}  {:<20}  NAME", "DEVICE", "PAIRED (UTC)")];
    for device in devices {
        lines.push(format!(
            "{:<16}  {:<20}  {}",
            device.id,
            device.paired_at.format("%Y-%m-%d %H:%M:%S"),
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
    let Some(status) = status else {
        return "Remote gateway: stopped".to_string();
    };
    let mut lines = vec![
        format!("Remote gateway: running (pid {})", status.pid),
        format!("  listening on   {}", status.listen_address),
        format!("  advertised as  {}", status.advertised_address),
        format!("  gateway id     {}", status.gateway_id),
        format!("  paired devices {}", status.paired_devices),
        format!(
            "  pairing        {}",
            if status.pairing_open {
                "challenge open"
            } else {
                "closed"
            }
        ),
        format!("  connections    {}", status.connections.len()),
    ];
    for connection in &status.connections {
        lines.push(format!(
            "    {}  {}",
            connection.device_id, connection.device_name
        ));
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
                lines.push(format!(
                    "    {}  {}  ({attached})",
                    session.session_id, session.title
                ));
            }
        }
    }
    if status.loopback_only {
        lines.push(String::new());
        lines.push(loopback_guidance(false));
    }
    lines.join("\n")
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
