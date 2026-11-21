//! The `ssh -L` behind a forwarded loopback port, so RustCode can take its
//! tunnel over (`docs/remote.md` › SSH routes).
//!
//! A machine reached through `127.0.0.1:<port>` that a user-run `ssh -L`
//! forwards goes dark after this computer sleeps: ssh keeps the port open on
//! a dead link, and only notices minutes later. An SSH route to the same host
//! has no such tunnel to go stale: RustCode runs `ssh -W` for each connection
//! ([`crate::tunnel`]), so the attempt after a wake opens a fresh one. This
//! module reads the running `ssh` that owns the port, so the app can offer
//! that SSH route with the host already filled in.

use crate::registry::SshRoute;
use crate::ssh::SshTarget;
use std::path::PathBuf;
use std::time::Duration;

/// How long listing this computer's processes may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(2);

/// `ssh` options that take a value, from `ssh(1)`: the value is the rest of
/// the word, or the next word.
const TAKES_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// The SSH route that reaches the same gateway as the forward of `local_port`
/// in `argv`, an `ssh` command line; `None` when `argv` is not `ssh`, forwards
/// no such port, or forwards it somewhere other than the host's own loopback
/// (an SSH route reaches the gateway at `127.0.0.1` there).
pub fn ssh_route_for_forward<S: AsRef<str>>(argv: &[S], local_port: u16) -> Option<SshRoute> {
    let (program, args) = argv.split_first()?;
    let program = program.as_ref();
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    if !(name == "ssh" || name == "ssh.exe") {
        return None;
    }
    let mut gateway_port = None;
    let mut port = None;
    let mut identity = None;
    let mut user = None;
    let mut destination = None;
    let mut words = args.iter().map(AsRef::as_ref);
    while let Some(word) = words.next() {
        if word == "--" {
            destination = destination.or(words.next());
            break;
        }
        let Some(flags) = word.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            // The first word that is not an option is the destination. Like
            // `ssh`, options may follow it; the next plain word starts the
            // remote command.
            if destination.is_some() {
                break;
            }
            destination = Some(word);
            continue;
        };
        for (at, flag) in flags.char_indices() {
            if !TAKES_VALUE.contains(flag) {
                continue;
            }
            let rest = &flags[at + flag.len_utf8()..];
            let value = if rest.is_empty() {
                words.next().unwrap_or_default()
            } else {
                rest
            };
            match flag {
                'L' => {
                    if gateway_port.is_none() {
                        gateway_port = forwarded_gateway_port(value, local_port);
                    }
                }
                'p' => port = value.parse::<u16>().ok(),
                'i' => identity = Some(PathBuf::from(value)),
                'l' => user = Some(value),
                _ => {}
            }
            break;
        }
    }
    let gateway_port = gateway_port?;
    let destination = destination?;
    let destination = destination
        .strip_prefix("ssh://")
        .unwrap_or(destination)
        .to_owned();
    let destination = match user {
        Some(user) if !destination.contains('@') => format!("{user}@{destination}"),
        _ => destination,
    };
    let target = SshTarget::parse(&destination, port, identity).ok()?;
    Some(SshRoute {
        destination: target.destination,
        port: target.port,
        identity: target.identity,
        gateway_port,
    })
}

/// The far port of a `-L [bind:]port:host:hostport` that listens on
/// `local_port` and goes to the remote host's own loopback.
fn forwarded_gateway_port(spec: &str, local_port: u16) -> Option<u16> {
    let parts = split_spec(spec);
    let (port, host, host_port) = match parts.as_slice() {
        [port, host, host_port] | [_, port, host, host_port] => (*port, *host, *host_port),
        _ => return None,
    };
    if port.parse::<u16>().ok()? != local_port {
        return None;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback {
        return None;
    }
    host_port.parse::<u16>().ok().filter(|port| *port != 0)
}

/// Split a forward spec at `:` outside brackets, or at `/` (the spelling
/// `ssh` takes for IPv6 addresses).
fn split_spec(spec: &str) -> Vec<&str> {
    if spec.contains('/') {
        return spec.split('/').collect();
    }
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, c) in spec.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                parts.push(&spec[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&spec[start..]);
    parts
}

/// The SSH route behind the `ssh -L` running on this computer that forwards
/// `local_port`, read from the process list; `None` when no such process
/// runs (it may already have ended) or the list cannot be read.
pub async fn find_forward_owner(local_port: u16) -> Option<SshRoute> {
    let lines = process_command_lines().await?;
    lines.lines().find_map(|line| {
        let argv: Vec<&str> = line.split_whitespace().collect();
        ssh_route_for_forward(&argv, local_port)
    })
}

/// Every process's command line, one per line. `-A -ww -o args=` means the
/// same to the `ps` of macOS and of Linux.
#[cfg(unix)]
async fn process_command_lines() -> Option<String> {
    let output = tokio::time::timeout(
        LIST_TIMEOUT,
        tokio::process::Command::new("ps")
            .args(["-A", "-ww", "-o", "args="])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(unix))]
async fn process_command_lines() -> Option<String> {
    let _ = LIST_TIMEOUT;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(line: &str, port: u16) -> Option<SshRoute> {
        let argv: Vec<&str> = line.split_whitespace().collect();
        ssh_route_for_forward(&argv, port)
    }

    #[test]
    fn a_plain_forward_names_its_host_and_the_gateway_port() {
        let found = route("ssh -N -L 17880:127.0.0.1:17879 LagosVPS", 17880).unwrap();
        assert_eq!(found.destination, "LagosVPS");
        assert_eq!(found.port, None);
        assert_eq!(found.gateway_port, 17879);
    }

    #[test]
    fn clustered_flags_options_and_a_bind_address_are_read() {
        let found = route(
            "/usr/bin/ssh -fNL127.0.0.1:17880:localhost:18000 -o ServerAliveInterval=15 \
             -p 2222 -i /Users/lars/.ssh/vps -l lars vps.example.com",
            17880,
        )
        .unwrap();
        assert_eq!(found.destination, "lars@vps.example.com");
        assert_eq!(found.port, Some(2222));
        assert_eq!(found.identity, Some(PathBuf::from("/Users/lars/.ssh/vps")));
        assert_eq!(found.gateway_port, 18000);
        let found = route("ssh -L [::1]:17880:[::1]:17879 me@box", 17880).unwrap();
        assert_eq!(
            (found.destination.as_str(), found.gateway_port),
            ("me@box", 17879)
        );
        // ssh reads options after the destination too.
        let found = route("ssh box -N -L 17880:127.0.0.1:17879", 17880).unwrap();
        assert_eq!(found.destination, "box");
        let found = route("ssh -L 17880/::1/17879 box", 17880).unwrap();
        assert_eq!(found.gateway_port, 17879);
    }

    #[test]
    fn anything_else_is_not_the_owner() {
        // Another port, another program, a forward to a third host, no host.
        assert!(route("ssh -N -L 17881:127.0.0.1:17879 LagosVPS", 17880).is_none());
        assert!(route("autossh -M 0 -L 17880:127.0.0.1:17879 LagosVPS", 17880).is_none());
        assert!(route("ssh -L 17880:10.0.0.5:17879 jump", 17880).is_none());
        assert!(route("ssh -N -L 17880:127.0.0.1:17879", 17880).is_none());
        // A -L after the remote command is the command's, not ssh's.
        assert!(route("ssh LagosVPS nc -L 17880:127.0.0.1:17879", 17880).is_none());
        // A destination ssh would take as an option is refused.
        assert!(route("ssh -L 17880:127.0.0.1:17879 -- -oProxyCommand=x", 17880).is_none());
    }
}
