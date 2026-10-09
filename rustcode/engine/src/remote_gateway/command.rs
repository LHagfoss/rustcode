//! `rustcode remote …` commands: argument handling and terminal output.

use super::address::{candidate_addresses, plan_listen};
use super::control::{GatewayStatus, OfferDetails};
use super::devices::DeviceRecord;
use super::gateway::{Gateway, GatewayConfig, Limits};
use super::lifecycle::{RemoteLifecycle, Revocation};
use super::router::NoSessionsRouter;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

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
    let gateway = Gateway::bind(
        &lifecycle,
        GatewayConfig {
            plan: plan.clone(),
            port,
            router: Arc::new(NoSessionsRouter),
            limits: Limits::default(),
        },
    )
    .await?;
    println!("rustcode remote gateway (foundation: no sessions are shared yet)");
    println!("listening on  {}", gateway.local_addr());
    println!("advertised as {}", gateway.advertised_address());
    println!("gateway id    {}", gateway.gateway_id());
    if !plan.bind.is_loopback() {
        println!(
            "warning: plain WebSocket on a non-loopback address is authenticated but not encrypted; use it only on a trusted network such as NetBird."
        );
    }
    println!("Pair a device with `rustcode remote pair` from another terminal.");

    let shutdown = gateway.shutdown_token();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let run = gateway.run();
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => return result,
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    // Let the gateway close its connections and remove its registration.
    shutdown.cancel();
    run.await
}

/// Text for a freshly issued pairing challenge. This is the one place the
/// pairing secrets are shown.
pub fn format_offer(offer: &OfferDetails) -> String {
    // QR rendering hook: draw `offer.qr_payload` as a QR code here once the
    // workspace has an encoder. Until then the payload is printed as text.
    format!(
        "Pair a device within {} seconds (single use).\n  address:    {}\n  code:       {}\n  QR payload: {}\nA new `rustcode remote pair` replaces this challenge.",
        offer.expires_in_secs,
        offer.advertised_address,
        offer.code.expose(),
        offer.qr_payload.expose(),
    )
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
    lines.push("  sessions       none shared (gateway foundation)".to_string());
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
