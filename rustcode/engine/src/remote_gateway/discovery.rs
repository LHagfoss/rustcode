//! Bonjour advertises only a service name and version, never pairing material.
use std::process::Stdio;

pub fn advertise(port: u16, loopback: bool) -> Option<tokio::process::Child> {
    if loopback {
        return None;
    }
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
        ]);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = tokio::process::Command::new("avahi-publish-service");
        command.args([&name, "_rustcode._tcp", &port.to_string(), "protocol=1"]);
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
