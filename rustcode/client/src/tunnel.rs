//! SSH routes: reach a machine's gateway through the user's own `ssh`
//! (`docs/remote.md` › SSH routes).
//!
//! A [`Bridge`] listens on this computer's loopback, on a port the system
//! picks, and runs `ssh -W 127.0.0.1:<gateway port> <destination>` for each
//! socket it accepts, copying bytes both ways. The WebSocket code dials the
//! bridge's address as it dials any LAN address, so nothing above the socket
//! knows the route is SSH. Each connection owns its `ssh` process: it ends
//! when either side closes, and no tunnel outlives the connection it carried.
//! The listener stops when the bridge is dropped, once the race or the probe
//! that opened it is done; the connections it accepted keep running.
//!
//! The gateway there may listen on its own loopback only (`remote.bind =
//! "127.0.0.1"`): `-W` connects from the machine itself.

use crate::registry::SshRoute;
use crate::ssh::{SSH_OPTIONS, SshTarget, classify_ssh_error};
use anyhow::Result;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// How long [`Bridge::failure`] waits for `ssh` to exit and say why.
const FAILURE_WAIT: Duration = Duration::from_millis(1500);

/// A loopback address that reaches the machine through `ssh`, for as long as
/// this value lives.
pub struct Bridge {
    address: String,
    host: String,
    gateway_port: u16,
    failure: Arc<Mutex<Option<String>>>,
    /// Dropped with the bridge, which ends the accept loop.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Bridge {
    /// Start listening for `route`. Nothing is run until a socket connects.
    pub async fn open(route: &SshRoute) -> Result<Self> {
        let target = SshTarget::parse(&route.destination, route.port, route.identity.clone())?;
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?.to_string();
        let failure = Arc::new(Mutex::new(None));
        let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
        let args = args(&target, route.gateway_port);
        let host = target.host().to_owned();
        let accepted_failure = Arc::clone(&failure);
        let accepted_host = host.clone();
        let gateway_port = route.gateway_port;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let args = args.clone();
                        let failure = Arc::clone(&accepted_failure);
                        let host = accepted_host.clone();
                        tokio::spawn(carry(socket, args, host, gateway_port, failure));
                    }
                }
            }
        });
        Ok(Self {
            address,
            host,
            gateway_port,
            failure,
            _stop: stop,
        })
    }

    /// The `127.0.0.1:<port>` to dial.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Why `ssh` ended, when it did: waits briefly for it to exit, so a dial
    /// that failed because SSH did can name the reason instead of a closed
    /// socket.
    pub async fn failure(&self) -> Option<String> {
        let deadline = tokio::time::Instant::now() + FAILURE_WAIT;
        loop {
            if let Some(failure) = self.failure.lock().unwrap().clone() {
                return Some(failure);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The failure of a dial over this bridge, with SSH's own reason when it
    /// gave one.
    pub async fn explain(&self, error: impl std::fmt::Display) -> String {
        match self.failure().await {
            Some(failure) => failure,
            None => format!(
                "{error} (through SSH to {}, RustCode's port {})",
                self.host, self.gateway_port
            ),
        }
    }
}

/// `ssh -W 127.0.0.1:<port> -- <destination>` with the options every SSH
/// command here gets.
fn args(target: &SshTarget, gateway_port: u16) -> Vec<String> {
    let mut args = Vec::new();
    for option in SSH_OPTIONS {
        args.push("-o".to_owned());
        args.push((*option).to_owned());
    }
    args.push("-o".to_owned());
    args.push("ExitOnForwardFailure=yes".to_owned());
    if let Some(port) = target.port {
        args.push("-p".to_owned());
        args.push(port.to_string());
    }
    if let Some(identity) = &target.identity {
        args.push("-i".to_owned());
        args.push(identity.to_string_lossy().into_owned());
    }
    args.push("-W".to_owned());
    args.push(format!("127.0.0.1:{gateway_port}"));
    args.push("--".to_owned());
    args.push(target.destination.clone());
    args
}

/// One accepted socket: run `ssh` and copy both ways until either side ends.
async fn carry(
    socket: TcpStream,
    args: Vec<String>,
    host: String,
    gateway_port: u16,
    failure: Arc<Mutex<Option<String>>>,
) {
    let mut command = tokio::process::Command::new("ssh");
    command
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            *failure.lock().unwrap() = Some(format!(
                "could not run ssh on this computer ({error}); install OpenSSH"
            ));
            return;
        }
    };
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return;
    };
    let said = tokio::spawn(async move {
        let mut text = String::new();
        let _ = (&mut stderr)
            .take(64 * 1024)
            .read_to_string(&mut text)
            .await;
        text
    });
    let (mut from_socket, mut to_socket) = socket.into_split();
    let up = async {
        let _ = tokio::io::copy(&mut from_socket, &mut stdin).await;
        let _ = stdin.shutdown().await;
    };
    let down = async {
        let _ = tokio::io::copy(&mut stdout, &mut to_socket).await;
        let _ = to_socket.shutdown().await;
    };
    tokio::select! {
        () = up => {}
        () = down => {}
    }
    let _ = child.start_kill();
    let status = child.wait().await.ok();
    let stderr = said.await.unwrap_or_default();
    if status.is_some_and(|status| !status.success()) && !stderr.trim().is_empty() {
        *failure.lock().unwrap() = Some(describe_failure(&host, gateway_port, &stderr));
    }
}

/// What an `ssh -W` that ended said, for a person.
fn describe_failure(host: &str, gateway_port: u16, stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    // `-W` signed in, but nothing listens on the gateway's port there.
    if lower.contains("open failed") || lower.contains("stdio forwarding failed") {
        return format!(
            "SSH to {host} works, but nothing answers on RustCode's port {gateway_port} there. Start its server (`rustcode up`) and try again."
        );
    }
    classify_ssh_error(host, stderr).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_forwards_to_the_gateway_on_the_machines_own_loopback() {
        let target = SshTarget::parse("lars@box", Some(2222), None).unwrap();
        let args = args(&target, 17879);
        let joined = args.join(" ");
        assert!(joined.contains("-o BatchMode=yes"));
        assert!(joined.contains("-p 2222"));
        assert!(joined.ends_with("-W 127.0.0.1:17879 -- lars@box"));
    }

    #[test]
    fn a_forward_nothing_answers_names_the_server() {
        let said =
            "channel 0: open failed: connect failed: Connection refused\nstdio forwarding failed";
        let message = describe_failure("box", 17879, said);
        assert!(
            message.contains("nothing answers on RustCode's port 17879"),
            "{message}"
        );
    }

    #[test]
    fn a_refused_sign_in_says_so() {
        let message = describe_failure("box", 17879, "lars@box: Permission denied (publickey).");
        assert!(
            message.contains("without asking for a password"),
            "{message}"
        );
    }
}
