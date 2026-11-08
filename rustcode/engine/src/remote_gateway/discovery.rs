//! Bonjour advertises a service name, the protocol version, the gateway
//! identifier and the optional handshakes the gateway takes, never pairing
//! material. The identifier lets a paired device recognise its host at a new
//! address; it is a name, not a credential.
use std::process::Stdio;

/// The TXT record's `caps` value: comma-separated optional handshakes.
/// `connect_requests` means a device may send `connect_request` and have the
/// host's owner accept it ([`super::connect`]).
pub const TXT_CAPS: &str = "caps=connect_requests";

pub fn advertise(port: u16, loopback: bool, gateway_id: &str) -> Option<tokio::process::Child> {
    if loopback {
        return None;
    }
    let gateway = format!("gateway={gateway_id}");
    let name = super::gateway::host_name().unwrap_or_else(|| "RustCode".into());
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = crate::platform::tokio_command("/usr/bin/dns-sd");
        command.args([
            "-R",
            &name,
            "_rustcode._tcp",
            "local.",
            &port.to_string(),
            "protocol=1",
            &gateway,
            TXT_CAPS,
        ]);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = crate::platform::tokio_command("avahi-publish-service");
        command.args([
            &name,
            "_rustcode._tcp",
            &port.to_string(),
            "protocol=1",
            &gateway,
            TXT_CAPS,
        ]);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match command.spawn() {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!("Bonjour announcement unavailable: {error}");
            None
        }
    }
}
