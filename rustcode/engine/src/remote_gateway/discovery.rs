//! Bonjour advertises a service name, the protocol version and the gateway
//! identifier, never pairing material. The identifier lets a paired device
//! recognise its host at a new address; it is a name, not a credential.
use std::process::Stdio;

pub fn advertise(port: u16, loopback: bool, gateway_id: &str) -> Option<tokio::process::Child> {
    if loopback {
        return None;
    }
    let gateway = format!("gateway={gateway_id}");
    let name = super::gateway::host_name().unwrap_or_else(|| "RustCode".into());
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = tokio::process::Command::new("/usr/bin/dns-sd");
        command.args([
            "-R",
            &name,
            "_rustcode._tcp",
            "local.",
            &port.to_string(),
            "protocol=1",
            &gateway,
        ]);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = tokio::process::Command::new("avahi-publish-service");
        command.args([
            &name,
            "_rustcode._tcp",
            &port.to_string(),
            "protocol=1",
            &gateway,
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
