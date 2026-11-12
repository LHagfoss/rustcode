//! Connect over SSH: make a computer this one reaches with `ssh` a paired
//! machine in one step (`docs/remote.md` › Connect over SSH).
//!
//! 1. **Sign in**: `ssh <target> exit 0`, non-interactive.
//! 2. **Check**: the operating system, the architecture and every `rustcode`
//!    there with its version, from a POSIX shell or, when there is none,
//!    Windows PowerShell.
//! 3. **Install**: when RustCode is missing or older than this computer's, the
//!    release installer of this version (`scripts/install.sh --version`, or
//!    `install.ps1 -Version`), sent over SSH's standard input, so both
//!    computers end on the same release.
//! 4. **Start**: `rustcode up --yes --json` there: the server as a service,
//!    remote access on, and a connect code in the JSON it prints.
//! 5. **Pair**: redeem that code here over the routes it lists.
//!
//! The connect code only ever travels inside SSH. Every command uses the
//! user's own `ssh` binary, configuration and agent, with `BatchMode=yes`,
//! so a key that needs a passphrase or a password fails with a sentence
//! instead of hanging, and every step is bounded. Host key checking is left
//! as the user configured it. Nothing is interpolated into a remote command
//! from user text: the target is validated and passed after `--`, scripts go
//! on standard input or as an encoded PowerShell command, and the only
//! dynamic words are paths the remote reported, quoted for its shell.

use crate::connect_code::ConnectCode;
use crate::machine::{Connector, Paired, Unreachable};
use crate::registry::Route;
use anyhow::{Result, bail, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// This computer's RustCode release: what a machine set up over SSH gets.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The installers of this release, sent to the other computer.
const INSTALL_SH: &str = include_str!("../../../scripts/install.sh");
const INSTALL_PS1: &str = include_str!("../../../scripts/install.ps1");

/// Options every SSH command gets. `BatchMode` turns any prompt into a
/// failure, `ConnectTimeout` bounds an address that does not answer, the
/// keepalives end a connection that dies mid-step, and `ControlMaster=no`
/// keeps a multiplexed master from outliving the step.
pub const SSH_OPTIONS: &[&str] = &[
    "BatchMode=yes",
    "ConnectTimeout=10",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    "ControlMaster=no",
];

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(30);
const CHECK_TIMEOUT: Duration = Duration::from_secs(45);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// A computer to reach with `ssh`: `user@host` or a `Host` alias from
/// `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub destination: String,
    pub port: Option<u16>,
    pub identity: Option<PathBuf>,
}

impl SshTarget {
    /// Validate a destination: no leading `-` (it would be an option), no
    /// whitespace and no shell or `ssh` metacharacters.
    pub fn parse(destination: &str, port: Option<u16>, identity: Option<PathBuf>) -> Result<Self> {
        let destination = destination.trim();
        let destination = destination.strip_prefix("ssh://").unwrap_or(destination);
        ensure!(!destination.is_empty(), "type the host, like user@host");
        ensure!(
            !destination.starts_with('-'),
            "a host cannot start with \"-\""
        );
        ensure!(destination.len() <= 255, "that host is too long");
        let allowed =
            |c: char| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-' | ':' | '%');
        if let Some(bad) = destination.chars().find(|c| !allowed(*c)) {
            bail!(
                "a host cannot contain {bad:?}; type it like user@host or an alias from ~/.ssh/config"
            );
        }
        ensure!(
            destination.matches('@').count() <= 1,
            "a host has at most one \"@\""
        );
        ensure!(port != Some(0), "port 0 is not a port");
        Ok(Self {
            destination: destination.to_owned(),
            port,
            identity,
        })
    }

    /// The host part, for messages: `box` of `lars@box`.
    pub fn host(&self) -> &str {
        self.destination
            .rsplit_once('@')
            .map_or(self.destination.as_str(), |(_, host)| host)
    }

    /// The arguments to `ssh` that run `remote` there.
    pub fn args(&self, remote: &str) -> Vec<String> {
        let mut args = Vec::new();
        for option in SSH_OPTIONS {
            args.push("-o".to_owned());
            args.push((*option).to_owned());
        }
        if let Some(port) = self.port {
            args.push("-p".to_owned());
            args.push(port.to_string());
        }
        if let Some(identity) = &self.identity {
            args.push("-i".to_owned());
            args.push(identity.to_string_lossy().into_owned());
        }
        // No terminal: nothing here is interactive, and a pty would mix
        // standard error into the JSON.
        args.push("-T".to_owned());
        args.push("--".to_owned());
        args.push(self.destination.clone());
        args.push(remote.to_owned());
        args
    }
}

/// The steps a connection over SSH goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    SignIn,
    Check,
    Install,
    Start,
    Pair,
}

impl Step {
    /// The step as a person reads it, while it runs.
    pub fn label(self, host: &str) -> String {
        match self {
            Self::SignIn => format!("Signing in to {host}"),
            Self::Check => format!("Checking RustCode on {host}"),
            Self::Install => format!("Installing RustCode {VERSION} on {host}"),
            Self::Start => format!("Starting the server on {host}"),
            Self::Pair => format!("Pairing with {host}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Running,
    Done,
    Skipped,
    Failed,
}

/// One change of a step, for a progress display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub step: Step,
    pub state: StepState,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Why a step failed, so a caller can say what to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// SSH wanted a password or a passphrase, or the key was refused.
    Auth,
    /// The host key is unknown or changed.
    HostKey,
    /// The host did not answer, or its name did not resolve.
    Unreachable,
    /// Not macOS, Linux or Windows on a CPU RustCode is built for.
    Unsupported,
    /// RustCode is missing or older and installing was not allowed.
    InstallRefused,
    InstallFailed,
    StartFailed,
    /// The gateway there answered on no route this computer can reach.
    NoRoute,
    Other,
}

/// A step that failed, with the sentence to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshFailure {
    pub step: Step,
    pub kind: FailureKind,
    pub message: String,
}

impl std::fmt::Display for SshFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SshFailure {}

/// What the check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub os: RemoteOs,
    /// `uname -m`, or `PROCESSOR_ARCHITECTURE` on Windows.
    pub arch: String,
    /// The newest `rustcode` there, if any.
    pub installed: Option<Installed>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOs {
    Macos,
    Linux,
    Windows,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub path: String,
    pub version: String,
}

/// The POSIX check, run by `sh -l -s` so the `PATH` is a login shell's.
/// `~/.rustcode/bin` is where this bootstrap installs and what `rustcode
/// server update-cli` keeps current, so it is looked at even when it is not
/// on the `PATH`.
const PROBE_SH: &str = r#"printf 'os\t%s\n' "$(uname -s 2>/dev/null)"
printf 'arch\t%s\n' "$(uname -m 2>/dev/null)"
for c in "$HOME/.rustcode/bin/rustcode" "$(command -v rustcode 2>/dev/null)"; do
  if [ -n "$c" ] && [ -x "$c" ]; then
    printf 'bin\t%s\t%s\n' "$c" "$("$c" --version 2>/dev/null | head -n 1)"
  fi
done
"#;

/// The Windows check, for a host whose shell is `cmd.exe` or PowerShell.
const PROBE_PS1: &str = r#"$ProgressPreference = 'SilentlyContinue'
Write-Output "os`tWindows"
Write-Output ("arch`t" + $env:PROCESSOR_ARCHITECTURE)
$candidates = @(Join-Path $env:USERPROFILE '.rustcode\bin\rustcode.exe')
$found = Get-Command rustcode -ErrorAction SilentlyContinue
if ($found) { $candidates += $found.Source }
foreach ($c in $candidates) {
  if (Test-Path -PathType Leaf $c) {
    $v = (& $c --version 2>$null | Select-Object -First 1)
    Write-Output ("bin`t" + $c + "`t" + $v)
  }
}
"#;

/// A PowerShell command line for `script`, safe through `cmd.exe` and
/// PowerShell alike: `-EncodedCommand` is base64 of UTF-16LE, with no quoting
/// to get wrong.
pub fn powershell_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(utf16)
    )
}

/// `word` quoted for a POSIX shell.
pub fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `word` quoted for PowerShell.
fn ps_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

/// Read what a check printed. Lines it does not know (a login banner) are
/// skipped; of several `rustcode`s the newest wins.
pub fn parse_probe(output: &str) -> Option<Probe> {
    let mut os = None;
    let mut arch = String::new();
    let mut installed: Option<Installed> = None;
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        let mut fields = line.split('\t');
        match fields.next() {
            Some("os") => {
                os = Some(match fields.next().unwrap_or("").trim() {
                    "Darwin" => RemoteOs::Macos,
                    "Linux" => RemoteOs::Linux,
                    "Windows" => RemoteOs::Windows,
                    _ => RemoteOs::Other,
                });
            }
            Some("arch") => arch = fields.next().unwrap_or("").trim().to_owned(),
            Some("bin") => {
                let (Some(path), Some(version)) = (fields.next(), fields.next()) else {
                    continue;
                };
                // `rustcode 1.31.4`: the last word.
                let Some(version) = version.split_whitespace().last() else {
                    continue;
                };
                if parse_version(version).is_none() {
                    continue;
                }
                let newer = installed
                    .as_ref()
                    .is_none_or(|current| version_older(&current.version, version));
                if newer {
                    installed = Some(Installed {
                        path: path.to_owned(),
                        version: version.to_owned(),
                    });
                }
            }
            _ => {}
        }
    }
    Some(Probe {
        os: os?,
        arch,
        installed,
    })
}

/// `1.31.4` or `1.32.0-beta.2`: the three numbers and the prerelease, if any.
fn parse_version(version: &str) -> Option<((u64, u64, u64), Option<&str>)> {
    let version = version.trim().trim_start_matches('v');
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let numbers = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    Some((numbers, pre))
}

/// Whether release `a` is older than `b`. A prerelease is older than its
/// release; two prereleases of one release compare by their number.
pub fn version_older(a: &str, b: &str) -> bool {
    let (Some((a_core, a_pre)), Some((b_core, b_pre))) = (parse_version(a), parse_version(b))
    else {
        return false;
    };
    if a_core != b_core {
        return a_core < b_core;
    }
    match (a_pre, b_pre) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => {
            let number = |pre: &str| pre.rsplit('.').next().and_then(|n| n.parse::<u64>().ok());
            match (number(a), number(b)) {
                (Some(a), Some(b)) => a < b,
                _ => a < b,
            }
        }
    }
}

/// Whether the machine needs this release installed.
pub fn needs_install(probe: &Probe) -> bool {
    probe
        .installed
        .as_ref()
        .is_none_or(|installed| version_older(&installed.version, VERSION))
}

/// Refuse a platform RustCode publishes no build for, before installing.
pub fn check_supported(probe: &Probe) -> std::result::Result<(), String> {
    let arch = probe.arch.to_ascii_lowercase();
    match probe.os {
        RemoteOs::Macos | RemoteOs::Linux
            if matches!(arch.as_str(), "x86_64" | "amd64" | "arm64" | "aarch64") =>
        {
            Ok(())
        }
        // ARM64 Windows runs the x86_64 build through emulation.
        RemoteOs::Windows if matches!(arch.as_str(), "amd64" | "arm64" | "x86_64") => Ok(()),
        RemoteOs::Other => {
            Err("RustCode runs on macOS, Linux and Windows; this host is none of them".to_owned())
        }
        _ => Err(format!(
            "RustCode has no build for {} on {}",
            match probe.os {
                RemoteOs::Macos => "macOS",
                RemoteOs::Linux => "Linux",
                _ => "Windows",
            },
            if probe.arch.is_empty() {
                "this CPU"
            } else {
                probe.arch.as_str()
            }
        )),
    }
}

/// What `rustcode up --json` printed there (`rustcode/tui/src/up.rs`).
#[derive(Debug, Clone, Deserialize)]
pub struct UpReport {
    pub version: String,
    #[serde(default)]
    pub host_name: Option<String>,
    #[serde(default)]
    pub os: String,
    pub server: UpServer,
    pub connect_code: UpCode,
    #[serde(default)]
    pub problems: Vec<UpProblem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpServer {
    pub mode: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub running: bool,
}

#[derive(Clone, Deserialize)]
pub struct UpCode {
    pub link: String,
    #[serde(default)]
    pub loopback_only: bool,
}

impl std::fmt::Debug for UpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpCode")
            .field("link", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpProblem {
    pub code: String,
    pub message: String,
}

impl UpReport {
    /// The report in what the command printed, past anything a login shell
    /// printed first.
    pub fn from_output(output: &str) -> Result<Self> {
        let mut offset = 0;
        for line in output.split_inclusive('\n') {
            if line.trim_start().starts_with('{') {
                let mut stream =
                    serde_json::Deserializer::from_str(&output[offset..]).into_iter::<Self>();
                if let Some(Ok(report)) = stream.next() {
                    return Ok(report);
                }
            }
            offset += line.len();
        }
        bail!("`rustcode up` printed no report")
    }
}

/// What a connection over SSH ends with: the pairing, to be saved by the
/// caller as any other, and what `rustcode up` said there.
pub struct Connected {
    pub paired: Paired,
    pub route: Route,
    pub report: UpReport,
    pub probe: Probe,
}

/// The output of one SSH command.
struct Output {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run `ssh <target> <remote>`, with `input` on its standard input, for at
/// most `timeout`.
async fn run_ssh(
    target: &SshTarget,
    remote: &str,
    input: Option<&str>,
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut command = tokio::process::Command::new("ssh");
    command
        .args(target.args(remote))
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        let input = input.to_owned();
        tokio::spawn(async move {
            let _ = stdin.write_all(input.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer within {}s", timeout.as_secs()),
            )
        })??;
    Ok(Output {
        status: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// The last line of what a command said on standard error, for a message.
fn last_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
}

/// Name an `ssh` failure (exit 255) from what it printed.
pub fn classify_ssh_error(host: &str, stderr: &str) -> (FailureKind, String) {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
        || lower.contains("no matching host key")
    {
        return (
            FailureKind::HostKey,
            format!(
                "{host}'s host key is not known here, or it changed. Connect once in a terminal (`ssh {host}`) to check and accept it, then try again."
            ),
        );
    }
    if lower.contains("permission denied")
        || lower.contains("passphrase")
        || lower.contains("too many authentication failures")
        || lower.contains("no more authentication methods")
    {
        return (
            FailureKind::Auth,
            format!(
                "SSH could not sign in to {host} without asking for a password or passphrase. Add your key to the agent (`ssh-add`), or connect once in a terminal, then try again."
            ),
        );
    }
    if lower.contains("could not resolve hostname")
        || lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
        || lower.contains("connection closed")
    {
        return (
            FailureKind::Unreachable,
            format!(
                "{host} did not answer over SSH: {}",
                last_line(stderr).trim_start_matches("ssh: ")
            ),
        );
    }
    (
        FailureKind::Other,
        format!("SSH to {host} failed: {}", last_line(stderr)),
    )
}

fn failure(step: Step, kind: FailureKind, message: impl Into<String>) -> anyhow::Error {
    SshFailure {
        step,
        kind,
        message: message.into(),
    }
    .into()
}

/// Run one step's SSH command; an `ssh` failure (exit 255) or a timeout is
/// this step's failure, anything else is the command's and is returned.
async fn ssh_step(
    step: Step,
    target: &SshTarget,
    remote: &str,
    input: Option<&str>,
    timeout: Duration,
) -> Result<Output> {
    let host = target.host();
    let output = run_ssh(target, remote, input, timeout)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => failure(
                step,
                FailureKind::Other,
                "No `ssh` command was found on this computer. Install OpenSSH and try again.",
            ),
            std::io::ErrorKind::TimedOut => failure(
                step,
                FailureKind::Unreachable,
                format!("{host} stopped answering ({error})."),
            ),
            _ => failure(
                step,
                FailureKind::Other,
                format!("Could not run ssh: {error}"),
            ),
        })?;
    if output.status == Some(255) {
        let (kind, message) = classify_ssh_error(host, &output.stderr);
        return Err(failure(step, kind, message));
    }
    Ok(output)
}

/// Everything a connection over SSH takes besides the target.
pub struct Options<'a> {
    /// Asked once RustCode is found missing or older: answer whether to
    /// install this release there.
    pub approve_install: &'a mut (dyn FnMut(&Probe) -> bool + Send),
    /// Told each time a step starts, ends or fails.
    pub progress: &'a mut (dyn FnMut(Progress) + Send),
}

/// Connect to `target` over SSH: sign in, check, install if needed, start
/// and pair. The pairing is returned for the caller to save; a failure is an
/// [`SshFailure`] naming its step.
pub async fn connect(
    connector: &Connector,
    target: &SshTarget,
    options: Options<'_>,
) -> Result<Connected> {
    let Options {
        approve_install,
        progress,
    } = options;
    let host = target.host().to_owned();
    let mut report = |step: Step, state: StepState, detail: Option<String>| {
        progress(Progress {
            step,
            state,
            label: step.label(&host),
            detail,
        });
    };
    let result = async {
        report(Step::SignIn, StepState::Running, None);
        let output = ssh_step(Step::SignIn, target, "exit 0", None, SIGN_IN_TIMEOUT).await?;
        if output.status != Some(0) {
            return Err(failure(
                Step::SignIn,
                FailureKind::Other,
                format!(
                    "{host} accepted the sign-in but its shell failed: {}",
                    last_line(&output.stderr)
                ),
            ));
        }
        report(Step::SignIn, StepState::Done, None);

        report(Step::Check, StepState::Running, None);
        let probe = check(target).await?;
        check_supported(&probe)
            .map_err(|message| failure(Step::Check, FailureKind::Unsupported, message))?;
        let found = probe.installed.as_ref().map_or_else(
            || "RustCode is not installed".to_owned(),
            |installed| format!("RustCode {} at {}", installed.version, installed.path),
        );
        report(Step::Check, StepState::Done, Some(found));

        let program = if needs_install(&probe) {
            if !approve_install(&probe) {
                return Err(failure(
                    Step::Install,
                    FailureKind::InstallRefused,
                    match &probe.installed {
                        Some(installed) => format!(
                            "{host} has RustCode {}, older than this computer's {VERSION}. Update it there, or allow installing.",
                            installed.version
                        ),
                        None => format!(
                            "RustCode is not installed on {host}. Install it there, or allow installing."
                        ),
                    },
                ));
            }
            report(Step::Install, StepState::Running, None);
            let path = install(target, &probe).await?;
            report(Step::Install, StepState::Done, Some(path.clone()));
            path
        } else {
            report(Step::Install, StepState::Skipped, None);
            probe
                .installed
                .as_ref()
                .map(|installed| installed.path.clone())
                .unwrap_or_default()
        };

        report(Step::Start, StepState::Running, None);
        let up = start(target, probe.os, &program).await?;
        let detail = up
            .problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        report(
            Step::Start,
            StepState::Done,
            (!detail.is_empty()).then_some(detail),
        );

        report(Step::Pair, StepState::Running, None);
        let (paired, route) = pair(connector, target, probe.os, &program, &up).await?;
        report(Step::Pair, StepState::Done, Some(route.address.clone()));
        Ok(Connected {
            paired,
            route,
            report: up,
            probe,
        })
    }
    .await;
    if let Err(error) = &result
        && let Some(failed) = error.downcast_ref::<SshFailure>()
    {
        report(failed.step, StepState::Failed, Some(failed.message.clone()));
    }
    result
}

/// Step 2: what runs there. A host without a POSIX shell is asked again in
/// PowerShell.
async fn check(target: &SshTarget) -> Result<Probe> {
    let host = target.host();
    let posix = ssh_step(
        Step::Check,
        target,
        "sh -l -s",
        Some(PROBE_SH),
        CHECK_TIMEOUT,
    )
    .await?;
    let posix_probe = (posix.status == Some(0))
        .then(|| parse_probe(&posix.stdout))
        .flatten();
    // A Windows host with Git's or Cygwin's `sh` on its `PATH` answers the
    // POSIX check as an unknown system; PowerShell knows better.
    if let Some(probe) = &posix_probe
        && probe.os != RemoteOs::Other
    {
        return Ok(probe.clone());
    }
    let windows = ssh_step(
        Step::Check,
        target,
        &powershell_command(PROBE_PS1),
        None,
        CHECK_TIMEOUT,
    )
    .await?;
    if windows.status == Some(0)
        && let Some(probe) = parse_probe(&windows.stdout)
    {
        return Ok(probe);
    }
    if let Some(probe) = posix_probe {
        return Ok(probe);
    }
    Err(failure(
        Step::Check,
        FailureKind::Unsupported,
        format!(
            "{host} has neither a POSIX shell nor Windows PowerShell that RustCode can use: {}",
            last_line(if posix.stderr.trim().is_empty() {
                &windows.stderr
            } else {
                &posix.stderr
            })
        ),
    ))
}

/// Step 3: install this release, and answer the path of the `rustcode` it
/// installed.
async fn install(target: &SshTarget, probe: &Probe) -> Result<String> {
    let host = target.host();
    let (remote, script) = match probe.os {
        RemoteOs::Windows => (
            powershell_command(&format!(
                "$s = [Console]::In.ReadToEnd(); & ([scriptblock]::Create($s)) -NoApp -Version {}",
                ps_quote(VERSION)
            )),
            INSTALL_PS1,
        ),
        _ => (
            format!(
                "bash -s -- --no-app --version {} --bin-dir \"$HOME/.rustcode/bin\"",
                sh_quote(VERSION)
            ),
            INSTALL_SH,
        ),
    };
    let output = ssh_step(
        Step::Install,
        target,
        &remote,
        Some(script),
        INSTALL_TIMEOUT,
    )
    .await?;
    if output.status != Some(0) {
        let said = [last_line(&output.stderr), last_line(&output.stdout)]
            .into_iter()
            .find(|line| !line.is_empty())
            .unwrap_or("no message");
        return Err(failure(
            Step::Install,
            FailureKind::InstallFailed,
            format!("Installing RustCode {VERSION} on {host} failed: {said}"),
        ));
    }
    // Look again rather than guess where the installer put it.
    let probe = check(target).await?;
    match probe.installed {
        Some(installed) if !version_older(&installed.version, VERSION) => Ok(installed.path),
        _ => Err(failure(
            Step::Install,
            FailureKind::InstallFailed,
            format!("The installer finished, but no RustCode {VERSION} was found on {host}."),
        )),
    }
}

/// The command line that runs `program` with `args` there.
fn remote_rustcode(os: RemoteOs, program: &str, args: &str) -> (String, Option<String>) {
    match os {
        RemoteOs::Windows => (
            powershell_command(&format!(
                "$ProgressPreference = 'SilentlyContinue'; & {} {args}; exit $LASTEXITCODE",
                ps_quote(program)
            )),
            None,
        ),
        _ => (
            "sh -l -s".to_owned(),
            Some(format!("exec {} {args}\n", sh_quote(program))),
        ),
    }
}

/// Step 4: `rustcode up --yes --json` there.
async fn start(target: &SshTarget, os: RemoteOs, program: &str) -> Result<UpReport> {
    let host = target.host();
    let (remote, input) = remote_rustcode(os, program, "up --yes --json");
    let output = ssh_step(
        Step::Start,
        target,
        &remote,
        input.as_deref(),
        START_TIMEOUT,
    )
    .await?;
    if output.status != Some(0) {
        let said = last_line(&output.stderr);
        let message = if said.contains("unrecognized subcommand") {
            format!(
                "RustCode on {host} is too old for `rustcode up`. Update it there, or allow installing."
            )
        } else {
            format!(
                "RustCode on {host} could not start its server: {}",
                said.trim_start_matches("rustcode up: ")
            )
        };
        return Err(failure(Step::Start, FailureKind::StartFailed, message));
    }
    let report = UpReport::from_output(&output.stdout).map_err(|error| {
        failure(
            Step::Start,
            FailureKind::StartFailed,
            format!("RustCode on {host} started, but {error:#}."),
        )
    })?;
    if report.connect_code.loopback_only {
        return Err(failure(
            Step::Start,
            FailureKind::NoRoute,
            format!(
                "Remote access on {host} listens on that computer only (`remote.bind`). Set it to `auto` there and try again."
            ),
        ));
    }
    Ok(report)
}

/// Step 5: redeem the code here. When no route answers, the code is
/// withdrawn there so it does not stay open.
async fn pair(
    connector: &Connector,
    target: &SshTarget,
    os: RemoteOs,
    program: &str,
    up: &UpReport,
) -> Result<(Paired, Route)> {
    let host = target.host();
    let code: ConnectCode = crate::connect_code::parse(&up.connect_code.link).map_err(|error| {
        failure(
            Step::Pair,
            FailureKind::Other,
            format!("{host} printed a connect code this computer cannot read ({error:#})."),
        )
    })?;
    match connector.pair_with_code(&code, None).await {
        Ok(paired) => Ok(paired),
        Err(error) => {
            let (remote, input) = remote_rustcode(os, program, "machines code --cancel");
            let _ = run_ssh(target, &remote, input.as_deref(), SIGN_IN_TIMEOUT).await;
            if let Some(unreachable) = error.downcast_ref::<Unreachable>() {
                let addresses = code
                    .routes()
                    .iter()
                    .map(|route| route.address.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(failure(
                    Step::Pair,
                    FailureKind::NoRoute,
                    format!(
                        "{host} is reachable over SSH, but not on RustCode's port ({}): {}. Put both computers on the same network or a VPN such as NetBird or Tailscale, then try again. A host reachable over SSH alone needs the SSH route, which is not in RustCode yet.",
                        if addresses.is_empty() {
                            "it reported no address".to_owned()
                        } else {
                            addresses
                        },
                        unreachable.attempts.join("; ")
                    ),
                ));
            }
            Err(failure(
                Step::Pair,
                FailureKind::Other,
                format!("Pairing with {host} failed: {error:#}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_a_host_or_an_alias_and_never_an_option_or_a_command() {
        let target = SshTarget::parse("lars@box.lan", Some(2222), None).unwrap();
        assert_eq!(target.host(), "box.lan");
        assert_eq!(SshTarget::parse("box", None, None).unwrap().host(), "box");
        assert_eq!(
            SshTarget::parse("ssh://lars@box", None, None)
                .unwrap()
                .destination,
            "lars@box"
        );
        for bad in [
            "",
            "-oProxyCommand=evil",
            "box; rm -rf ~",
            "box && x",
            "lars@box extra",
            "$(id)@box",
            "box`id`",
            "a@b@c",
            "box\nnext",
        ] {
            assert!(SshTarget::parse(bad, None, None).is_err(), "{bad:?}");
        }
        assert!(SshTarget::parse("box", Some(0), None).is_err());
    }

    #[test]
    fn every_command_is_batch_mode_bounded_and_the_target_follows_double_dash() {
        let target =
            SshTarget::parse("lars@box", Some(2222), Some(PathBuf::from("/k/id_ed25519"))).unwrap();
        let args = target.args("exit 0");
        let joined = args.join(" ");
        assert!(joined.contains("-o BatchMode=yes"));
        assert!(joined.contains("-o ConnectTimeout=10"));
        assert!(joined.contains("-o ControlMaster=no"));
        assert!(joined.contains("-p 2222"));
        assert!(joined.contains("-i /k/id_ed25519"));
        let dash = args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(args[dash + 1], "lars@box");
        assert_eq!(args[dash + 2], "exit 0");
        assert_eq!(args.len(), dash + 3);
    }

    #[test]
    fn a_check_reads_the_system_and_the_newest_rustcode_past_a_banner() {
        let output = "Welcome to Ubuntu\nos\tLinux\narch\tx86_64\nbin\t/home/a/.rustcode/bin/rustcode\trustcode 1.30.0\nbin\t/usr/local/bin/rustcode\trustcode 1.31.4\n";
        let probe = parse_probe(output).unwrap();
        assert_eq!(probe.os, RemoteOs::Linux);
        assert_eq!(probe.arch, "x86_64");
        assert_eq!(
            probe.installed,
            Some(Installed {
                path: "/usr/local/bin/rustcode".into(),
                version: "1.31.4".into()
            })
        );
        let windows = parse_probe(
            "os\tWindows\r\narch\tAMD64\r\nbin\tC:\\Users\\a\\.rustcode\\bin\\rustcode.exe\trustcode 1.31.4\r\n",
        )
        .unwrap();
        assert_eq!(windows.os, RemoteOs::Windows);
        assert_eq!(
            windows.installed.unwrap().path,
            "C:\\Users\\a\\.rustcode\\bin\\rustcode.exe"
        );
        let bare = parse_probe("os\tDarwin\narch\tarm64\nbin\t/x/rustcode\t\n").unwrap();
        assert_eq!(bare.os, RemoteOs::Macos);
        assert_eq!(bare.installed, None);
        assert_eq!(parse_probe("'sh' is not recognized"), None);
    }

    #[test]
    fn versions_compare_as_releases() {
        assert!(version_older("1.30.9", "1.31.0"));
        assert!(version_older("1.31.4", "1.31.10"));
        assert!(!version_older("1.31.4", "1.31.4"));
        assert!(!version_older("2.0.0", "1.99.99"));
        assert!(version_older("1.32.0-beta.2", "1.32.0"));
        assert!(version_older("1.32.0-beta.2", "1.32.0-beta.10"));
        assert!(!version_older("1.32.0", "1.32.0-beta.1"));
        assert!(!version_older("garbage", "1.0.0"));
    }

    #[test]
    fn install_only_when_missing_or_older_and_only_where_a_build_exists() {
        let probe = |os, arch: &str, version: Option<&str>| Probe {
            os,
            arch: arch.into(),
            installed: version.map(|version| Installed {
                path: "/x".into(),
                version: version.into(),
            }),
        };
        assert!(needs_install(&probe(RemoteOs::Linux, "x86_64", None)));
        assert!(needs_install(&probe(
            RemoteOs::Linux,
            "x86_64",
            Some("0.1.0")
        )));
        assert!(!needs_install(&probe(
            RemoteOs::Linux,
            "x86_64",
            Some(VERSION)
        )));
        assert!(!needs_install(&probe(
            RemoteOs::Linux,
            "x86_64",
            Some("999.0.0")
        )));
        assert!(check_supported(&probe(RemoteOs::Linux, "aarch64", None)).is_ok());
        assert!(check_supported(&probe(RemoteOs::Macos, "arm64", None)).is_ok());
        assert!(check_supported(&probe(RemoteOs::Windows, "AMD64", None)).is_ok());
        let riscv = check_supported(&probe(RemoteOs::Linux, "riscv64", None)).unwrap_err();
        assert!(riscv.contains("riscv64"), "{riscv}");
        assert!(check_supported(&probe(RemoteOs::Other, "x86_64", None)).is_err());
    }

    #[test]
    fn ssh_failures_say_what_to_do() {
        let (kind, message) =
            classify_ssh_error("box", "lars@box: Permission denied (publickey,password).\n");
        assert_eq!(kind, FailureKind::Auth);
        assert!(message.contains("ssh-add"), "{message}");
        let (kind, message) = classify_ssh_error("box", "Host key verification failed.\n");
        assert_eq!(kind, FailureKind::HostKey);
        assert!(message.contains("ssh box"), "{message}");
        let (kind, _) = classify_ssh_error(
            "box",
            "ssh: Could not resolve hostname box: nodename nor servname provided\n",
        );
        assert_eq!(kind, FailureKind::Unreachable);
        let (kind, _) = classify_ssh_error(
            "box",
            "ssh: connect to host box port 22: Operation timed out\n",
        );
        assert_eq!(kind, FailureKind::Unreachable);
        let (kind, message) = classify_ssh_error("box", "kex_exchange_identification: boom\n");
        assert_eq!(kind, FailureKind::Other);
        assert!(message.ends_with("boom"));
    }

    #[test]
    fn the_up_report_is_read_past_what_a_login_shell_printed() {
        let output = "Last login: today\n{ not json\n{\n  \"version\": \"1.31.4\",\n  \"host_name\": \"box\",\n  \"os\": \"linux\",\n  \"server\": {\"mode\": \"service\", \"kind\": \"systemd\", \"running\": true},\n  \"connect_code\": {\"code\": \"K7MQ\", \"link\": \"rustcode://connect/x\", \"expires_in_secs\": 600},\n  \"problems\": []\n}\n";
        let report = UpReport::from_output(output).unwrap();
        assert_eq!(report.host_name.as_deref(), Some("box"));
        assert_eq!(report.connect_code.link, "rustcode://connect/x");
        assert!(!format!("{:?}", report.connect_code).contains("connect/x"));
        assert!(UpReport::from_output("nothing here").is_err());
    }

    #[test]
    fn remote_words_are_quoted_for_their_shell() {
        assert_eq!(sh_quote("/home/o'neil/bin"), r"'/home/o'\''neil/bin'");
        assert_eq!(ps_quote(r"C:\Users\o'neil"), r"'C:\Users\o''neil'");
        let command = powershell_command("Write-Output 'hi'");
        assert!(command.starts_with("powershell -NoLogo -NoProfile -NonInteractive"));
        let encoded = command.rsplit(' ').next().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let utf16: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16(&utf16).unwrap(), "Write-Output 'hi'");
        let (remote, input) = remote_rustcode(RemoteOs::Linux, "/x/rustcode", "up --yes --json");
        assert_eq!(remote, "sh -l -s");
        assert_eq!(input.unwrap(), "exec '/x/rustcode' up --yes --json\n");
    }

    #[test]
    fn the_installers_this_release_sends_can_install_a_named_version() {
        assert!(INSTALL_SH.contains("--version)"));
        assert!(INSTALL_PS1.contains("[string]$Version"));
    }
}
