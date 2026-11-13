//! System setup: the small per-OS steps that make an installed RustCode behave
//! (docs/system-setup.md). Each step is *checked* first and only *applied* when
//! the check says it is missing, so running them again, at every update or from
//! Settings › System setup › Run again, changes nothing that is already right.
//!
//! - macOS: clear the download quarantine flag; put `rustcode` on the PATH.
//! - Windows: add RustCode to the user PATH; look for Git for Windows; say
//!   whether long paths are on.
//! - Linux: put `rustcode` on the PATH; write the launcher entry of an
//!   AppImage.
//! - Everywhere: the background server (a LaunchAgent, a systemd user unit, a
//!   logon task), which only runs when asked, as it keeps a process up.
//!
//! What needs the user is never done for them: a step that wants an
//! administrator (Windows long paths), a shell profile edit or a package
//! install reports *Needs you* with the exact command to run. Nothing here
//! asks for elevation silently.
//!
//! `system-setup.json` in the config folder records the last outcome of each
//! step, and the version the automatic steps last ran for, so a launch after an
//! update runs them once.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The systems the steps are written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// One step: what it is, where it applies, and whether it runs by itself.
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub os: &'static [Os],
    /// Runs after an install or an update without asking. The others wait for
    /// their button: they start something that stays running.
    pub auto: bool,
}

const UNIX: &[Os] = &[Os::Macos, Os::Linux];
const ALL: &[Os] = &[Os::Macos, Os::Windows, Os::Linux];

pub const STEPS: &[Step] = &[
    Step {
        id: "quarantine",
        title: "Clear the download quarantine flag",
        about: "macOS marks a downloaded app as quarantined, which can make its command line prompt or fail the first time. This clears the flag on RustCode.",
        os: &[Os::Macos],
        auto: true,
    },
    Step {
        id: "stable-signature",
        title: "Keep one signature across updates",
        about: "Releases are signed ad hoc, so macOS sees every update as a new app and asks again for Keychain access and Background Items. A local certificate gives updates one identity.",
        os: &[Os::Macos],
        auto: true,
    },
    Step {
        id: "cli-link",
        title: "Put `rustcode` on your PATH",
        about: "Links `rustcode` into ~/.local/bin when no copy is on your PATH, so a terminal runs the same version as the app.",
        os: UNIX,
        auto: true,
    },
    Step {
        id: "path-hint",
        title: "Check ~/.local/bin is on your shell's PATH",
        about: "Reads your login shell's PATH. RustCode never edits your shell profile; when the folder is missing it shows the line to add.",
        os: UNIX,
        auto: true,
    },
    Step {
        id: "user-path",
        title: "Add RustCode to your PATH",
        about: "Adds the folder with `rustcode.exe` to your user PATH (no administrator needed), keeping the entries you have. New terminals pick it up.",
        os: &[Os::Windows],
        auto: true,
    },
    Step {
        id: "git",
        title: "Look for Git for Windows",
        about: "Threads that work in worktrees and branches run `git`. This looks for it and shows how to install it when it is missing.",
        os: &[Os::Windows],
        auto: true,
    },
    Step {
        id: "long-paths",
        title: "Check Windows long paths",
        about: "Deep project folders can pass 260 characters. Turning long paths on needs an administrator, so this shows the command rather than running it.",
        os: &[Os::Windows],
        auto: true,
    },
    Step {
        id: "desktop-entry",
        title: "Add RustCode to your application menu",
        about: "An AppImage has no menu entry of its own. This writes ~/.local/share/applications/rustcode.desktop for it, and leaves an entry another tool made alone.",
        os: &[Os::Linux],
        auto: true,
    },
    Step {
        id: "server-service",
        title: "Run the RustCode server in the background",
        about: "Installs the server as a LaunchAgent, systemd user unit or logon task, so paired machines reach this computer with the app closed. See Machines.",
        os: ALL,
        auto: false,
    },
];

/// The steps that apply on `os`, in the order they show.
pub fn steps_for(os: Os) -> Vec<&'static Step> {
    STEPS.iter().filter(|step| step.os.contains(&os)).collect()
}

/// What looking at a step found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// Already as it should be.
    Satisfied(String),
    /// Missing, and RustCode can fix it.
    Missing(String),
    /// Wrong, and only the user can fix it: the message says how.
    Manual(String),
    /// Not for this machine (a launcher another tool made, an install that is
    /// managed elsewhere): nothing to do, and the message says why.
    Unsupported(String),
}

/// How a step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Done,
    /// Missing; nothing applied it yet (a status read, or a step that waits for
    /// its button).
    Pending,
    /// Needs the user; the message has the command.
    NeedsYou,
    Skipped,
    Failed,
}

/// The system, as the steps see it. A test supplies a fake.
pub trait Host {
    fn os(&self) -> Os;
    fn check(&self, id: &str) -> Check;
    /// Fix what `check` found missing; answers what was done.
    fn apply(&self, id: &str) -> Result<String, String>;
}

/// One step as the page shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub id: &'static str,
    pub title: &'static str,
    pub about: &'static str,
    pub auto: bool,
    pub status: Status,
    pub message: String,
    /// When this step last ran (RFC 3339), from the record.
    pub last_run: Option<String>,
}

/// What the last run of a step left.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    pub status: Status,
    pub message: String,
    pub at: String,
}

/// `system-setup.json`: the record of what ran.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    /// The app version the automatic steps last ran for.
    #[serde(default)]
    pub ran_for: Option<String>,
    #[serde(default)]
    pub steps: BTreeMap<String, Recorded>,
}

fn ledger_path() -> Option<PathBuf> {
    Some(crate::diagnostics::config_dir()?.join("system-setup.json"))
}

impl Ledger {
    pub fn load() -> Ledger {
        ledger_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = ledger_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

fn status_of(check: &Check) -> (Status, String) {
    match check {
        Check::Satisfied(message) => (Status::Done, message.clone()),
        Check::Missing(message) => (Status::Pending, message.clone()),
        Check::Manual(message) => (Status::NeedsYou, message.clone()),
        Check::Unsupported(message) => (Status::Skipped, message.clone()),
    }
}

fn report(step: &'static Step, status: Status, message: String, ledger: &Ledger) -> StepReport {
    StepReport {
        id: step.id,
        title: step.title,
        about: step.about,
        auto: step.auto,
        status,
        message,
        last_run: ledger.steps.get(step.id).map(|r| r.at.clone()),
    }
}

/// Every step for the host's system, looked at and not changed.
pub fn status(host: &dyn Host, ledger: &Ledger) -> Vec<StepReport> {
    steps_for(host.os())
        .into_iter()
        .map(|step| {
            let (status, message) = status_of(&host.check(step.id));
            report(step, status, message, ledger)
        })
        .collect()
}

/// Check, then fix what is missing: the one step named by `only`, or every
/// automatic step when it is `None`. A step that is already right is left
/// alone, so a second run applies nothing. The outcome of each step is kept in
/// `ledger` (stamped `at`); the caller saves it.
pub fn run(host: &dyn Host, only: Option<&str>, ledger: &mut Ledger, at: &str) -> Vec<StepReport> {
    let mut reports = Vec::new();
    for step in steps_for(host.os()) {
        let selected = match only {
            Some(id) => step.id == id,
            None => step.auto,
        };
        if !selected {
            continue;
        }
        let (status, message) = match host.check(step.id) {
            Check::Missing(_) => match host.apply(step.id) {
                Ok(message) => (Status::Done, message),
                Err(message) => (Status::Failed, message),
            },
            other => status_of(&other),
        };
        ledger.steps.insert(
            step.id.to_owned(),
            Recorded {
                status,
                message: message.clone(),
                at: at.to_owned(),
            },
        );
        reports.push(report(step, status, message, ledger));
    }
    reports
}

// ---------------------------------------------------------------- the real system

/// This computer.
pub struct System;

impl Host for System {
    fn os(&self) -> Os {
        Os::current()
    }

    fn check(&self, id: &str) -> Check {
        match id {
            "quarantine" => check_quarantine(),
            "stable-signature" => check_stable_signature(),
            "cli-link" => check_cli_link(),
            "path-hint" => check_path_hint(),
            "user-path" => check_user_path(),
            "git" => check_git(),
            "long-paths" => check_long_paths(),
            "desktop-entry" => check_desktop_entry(),
            "server-service" => check_server(),
            _ => Check::Unsupported("Unknown step.".into()),
        }
    }

    fn apply(&self, id: &str) -> Result<String, String> {
        match id {
            "quarantine" => apply_quarantine(),
            "cli-link" => apply_cli_link(),
            "user-path" => apply_user_path(),
            "desktop-entry" => apply_desktop_entry(),
            "server-service" => apply_server(),
            _ => Err("This step has nothing to apply.".into()),
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn tool_ok(program: &str, args: &[&str]) -> Option<std::process::Output> {
    crate::service::run_program(Path::new(program), args, Duration::from_secs(10)).ok()
}

/// What a `rustcode` search looks in: the app's PATH, then where installs put it.
fn rustcode_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .into_iter()
            .map(PathBuf::from),
    );
    if let Some(home) = home() {
        for sub in [".local/bin", ".rustcode/bin", ".cargo/bin"] {
            dirs.push(home.join(sub));
        }
    }
    dirs
}

/// The first executable file called `name` in `dirs`.
fn find_in_dirs(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| is_program(candidate))
}

fn is_program(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// macOS

/// The app bundle this process runs from, else the engine binary: what the
/// download flag sits on.
fn quarantine_target() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.ancestors()
                .find(|p| p.extension().is_some_and(|e| e == "app"))
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(crate::engine::resolve_engine)
}

fn check_quarantine() -> Check {
    let target = quarantine_target();
    let target = target.to_string_lossy();
    match tool_ok("xattr", &["-p", "com.apple.quarantine", &target]) {
        None => Check::Unsupported("`xattr` is not available here.".into()),
        Some(out) if out.status.success() => Check::Missing(format!(
            "{target} still carries the download quarantine flag."
        )),
        Some(_) => Check::Satisfied("No quarantine flag on RustCode.".into()),
    }
}

/// Whether `codesign -dv --verbose=4` output describes an ad-hoc signature.
fn is_adhoc(codesign_output: &str) -> bool {
    codesign_output
        .lines()
        .any(|line| line.trim() == "Signature=adhoc")
}

/// The command that creates the local certificate and signs the installed app
/// with it (`scripts/macos-stable-signature.sh`).
const STABLE_SIGNATURE_COMMAND: &str = "curl -fsSL https://raw.githubusercontent.com/LHagfoss/rustcode/main/scripts/macos-stable-signature.sh | bash -s -- /Applications/RustCode.app";

fn check_stable_signature() -> Check {
    let bundle = quarantine_target();
    let bundle = bundle.to_string_lossy();
    let Some(signature) = tool_ok("/usr/bin/codesign", &["-dv", "--verbose=4", &bundle]) else {
        return Check::Unsupported("`codesign` is not available here.".into());
    };
    // `codesign` prints what it found on stderr.
    if !is_adhoc(&String::from_utf8_lossy(&signature.stderr)) {
        return Check::Satisfied(
            "RustCode has a signature that stays the same across updates.".into(),
        );
    }
    let has_certificate = tool_ok(
        "/usr/bin/security",
        &["find-certificate", "-c", "RustCode Local Signing"],
    )
    .is_some_and(|out| out.status.success());
    if has_certificate {
        // The certificate exists: the next update signs with it. This copy was
        // installed before it, so it signs once more by hand.
        return Check::Manual(format!(
            "The local certificate exists, but this copy is still signed ad hoc. Run: {STABLE_SIGNATURE_COMMAND}"
        ));
    }
    Check::Manual(format!(
        "Without it, macOS asks again for Keychain access and Background Items after every update. Creates a local certificate in your login keychain (macOS may ask for your password). Run: {STABLE_SIGNATURE_COMMAND}"
    ))
}

fn apply_quarantine() -> Result<String, String> {
    let target = quarantine_target();
    let target = target.to_string_lossy().into_owned();
    let out = tool_ok("xattr", &["-dr", "com.apple.quarantine", &target])
        .ok_or("`xattr` did not answer.")?;
    if out.status.success() {
        Ok(format!("Cleared the quarantine flag on {target}."))
    } else {
        Err(format!(
            "Could not clear the flag: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

// macOS and Linux: the shell's `rustcode`

fn local_bin() -> Option<PathBuf> {
    Some(home()?.join(".local").join("bin"))
}

fn check_cli_link() -> Check {
    if let Some(found) = find_in_dirs(&rustcode_dirs(), "rustcode") {
        return Check::Satisfied(format!("`rustcode` is at {}.", found.display()));
    }
    Check::Missing("No `rustcode` is on your PATH for a terminal.".into())
}

#[cfg(unix)]
fn apply_cli_link() -> Result<String, String> {
    let dir = local_bin().ok_or("No home folder to link into.")?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let link = dir.join("rustcode");
    match link.symlink_metadata() {
        // A link to nothing: the engine moved. Anything else is the user's.
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(&link).map_err(|e| e.to_string())?;
        }
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a link, so it was left alone.",
                link.display()
            ))
        }
        Err(_) => {}
    }
    let target = crate::service::service_program();
    std::os::unix::fs::symlink(&target, &link)
        .map_err(|e| format!("Could not link {}: {e}", link.display()))?;
    Ok(format!(
        "Linked {} to {}.",
        link.display(),
        target.display()
    ))
}

#[cfg(not(unix))]
fn apply_cli_link() -> Result<String, String> {
    Err("Linking `rustcode` is for macOS and Linux.".into())
}

/// The login shell's PATH: what a terminal opened later finds. The last line of
/// output, as a profile may print a greeting first.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var_os("SHELL")?;
    let out = crate::service::run_program(
        Path::new(&shell),
        &["-l", "-c", "printf '%s' \"$PATH\""],
        Duration::from_secs(6),
    )
    .ok()?;
    out.status
        .success()
        .then(|| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .last()
                .map(str::to_owned)
        })
        .flatten()
}

fn path_contains(path: &str, dir: &Path) -> bool {
    std::env::split_paths(OsStr::new(path)).any(|entry| entry == dir)
}

/// The command that puts `~/.local/bin` on the PATH for `shell`'s login
/// sessions.
fn path_hint_command(shell: &str) -> String {
    let name = Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    match name {
        "fish" => "fish_add_path ~/.local/bin".into(),
        "bash" if cfg!(target_os = "macos") => {
            r#"echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bash_profile"#.into()
        }
        "bash" => r#"echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.profile"#.into(),
        _ => r#"echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zprofile"#.into(),
    }
}

fn check_path_hint() -> Check {
    let Some(local) = local_bin() else {
        return Check::Unsupported("No home folder.".into());
    };
    // Only the link this app makes needs the folder on the PATH.
    if local.join("rustcode").symlink_metadata().is_err() {
        return Check::Unsupported(
            "`rustcode` is not linked into ~/.local/bin, so there is nothing to add.".into(),
        );
    }
    let Some(path) = login_shell_path() else {
        return Check::Unsupported("Could not ask your login shell for its PATH.".into());
    };
    if path_contains(&path, &local) {
        return Check::Satisfied("~/.local/bin is on your shell's PATH.".into());
    }
    let shell = std::env::var("SHELL").unwrap_or_default();
    Check::Manual(format!(
        "~/.local/bin is not on your shell's PATH, so a terminal will not find `rustcode`. Run: {}",
        path_hint_command(&shell)
    ))
}

// Windows

/// Splits one value from `reg query`: `    Path    REG_EXPAND_SZ    C:\a;C:\b`
/// is `("REG_EXPAND_SZ", "C:\a;C:\b")`.
fn parse_reg_query(output: &str, name: &str) -> Option<(String, String)> {
    output.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if !words.next()?.eq_ignore_ascii_case(name) {
            return None;
        }
        let kind = words.next()?.to_owned();
        let after_kind = line.split_once(&kind)?.1.trim();
        Some((kind, after_kind.to_owned()))
    })
}

fn same_dir(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    !a.trim().is_empty() && norm(a) == norm(b)
}

/// `current` with `dir` added, or `None` when it is already an entry.
fn append_path_entry(current: &str, dir: &str) -> Option<String> {
    if current.split(';').any(|entry| same_dir(entry, dir)) {
        return None;
    }
    let current = current.trim_end_matches(';');
    Some(if current.is_empty() {
        dir.to_owned()
    } else {
        format!("{current};{dir}")
    })
}

fn engine_dir() -> Option<String> {
    crate::engine::resolve_engine()
        .parent()
        .map(|dir| dir.to_string_lossy().into_owned())
}

fn user_path() -> Option<(String, String)> {
    match tool_ok("reg", &["query", r"HKCU\Environment", "/v", "Path"]) {
        Some(out) if out.status.success() => {
            parse_reg_query(&String::from_utf8_lossy(&out.stdout), "Path")
        }
        // No Path value yet.
        Some(_) => Some(("REG_EXPAND_SZ".into(), String::new())),
        None => None,
    }
}

fn check_user_path() -> Check {
    let Some(dir) = engine_dir() else {
        return Check::Unsupported("Could not find RustCode's folder.".into());
    };
    let Some((_, current)) = user_path() else {
        return Check::Unsupported("`reg` is not available here.".into());
    };
    if current.split(';').any(|entry| same_dir(entry, &dir)) {
        Check::Satisfied(format!("{dir} is on your user PATH."))
    } else {
        Check::Missing(format!("{dir} is not on your user PATH."))
    }
}

fn apply_user_path() -> Result<String, String> {
    let dir = engine_dir().ok_or("Could not find RustCode's folder.")?;
    let (kind, current) = user_path().ok_or("`reg` is not available here.")?;
    if kind != "REG_SZ" && kind != "REG_EXPAND_SZ" {
        return Err(format!(
            "Your PATH is stored as {kind}, which RustCode does not edit."
        ));
    }
    let Some(next) = append_path_entry(&current, &dir) else {
        return Ok(format!("{dir} was already on your user PATH."));
    };
    let out = tool_ok(
        "reg",
        &[
            "add",
            r"HKCU\Environment",
            "/v",
            "Path",
            "/t",
            &kind,
            "/d",
            &next,
            "/f",
        ],
    )
    .ok_or("`reg` did not answer.")?;
    if out.status.success() {
        Ok(format!(
            "Added {dir} to your user PATH. Open a new terminal to use it."
        ))
    } else {
        Err(format!(
            "Could not change your PATH: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn check_git() -> Check {
    let version = tool_ok("git", &["--version"])
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
    if let Some(version) = version {
        return Check::Satisfied(version);
    }
    let known = Path::new(r"C:\Program Files\Git\cmd\git.exe");
    if known.is_file() {
        return Check::Satisfied(format!(
            "Git is at {}, though it is not on your PATH.",
            known.display()
        ));
    }
    Check::Manual(
        "Git for Windows was not found. Install it with: winget install --id Git.Git -e".into(),
    )
}

fn check_long_paths() -> Check {
    let out = tool_ok(
        "reg",
        &[
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\FileSystem",
            "/v",
            "LongPathsEnabled",
        ],
    );
    let enabled = out
        .filter(|out| out.status.success())
        .and_then(|out| parse_reg_query(&String::from_utf8_lossy(&out.stdout), "LongPathsEnabled"))
        .is_some_and(|(_, data)| data.eq_ignore_ascii_case("0x1"));
    if enabled {
        Check::Satisfied("Long paths are on.".into())
    } else {
        Check::Manual(
            r#"Long paths are off, so folders deeper than 260 characters fail. In PowerShell opened as administrator, run: New-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem" -Name LongPathsEnabled -Value 1 -PropertyType DWORD -Force"#.into(),
        )
    }
}

// Linux

/// `Exec=` takes a double-quoted program, with `"`, `` ` ``, `$` and `\`
/// escaped by a backslash (the desktop entry specification).
fn exec_quote(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Marks an entry this app wrote, so an update may rewrite it and nothing else.
const ENTRY_MARKER: &str = "X-RustCode-Managed=true";

fn desktop_entry(appimage: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=RustCode\nComment=Run coding agents on your projects\nExec={} %U\nTerminal=false\nCategories=Development;\n{ENTRY_MARKER}\n",
        exec_quote(&appimage.to_string_lossy())
    )
}

fn desktop_entry_path() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".local").join("share")))?;
    Some(data.join("applications").join("rustcode.desktop"))
}

fn appimage_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|a| !a.is_empty())
        .map(PathBuf::from)
}

fn check_desktop_entry() -> Check {
    let Some(appimage) = appimage_path() else {
        return Check::Unsupported(
            "Not an AppImage: a package installs its own menu entry.".into(),
        );
    };
    let Some(path) = desktop_entry_path() else {
        return Check::Unsupported("No home folder.".into());
    };
    match std::fs::read_to_string(&path) {
        Err(_) => Check::Missing("RustCode has no application menu entry.".into()),
        Ok(text) if text.contains(ENTRY_MARKER) => {
            if text == desktop_entry(&appimage) {
                Check::Satisfied(format!("{} points at this AppImage.", path.display()))
            } else {
                Check::Missing(format!("{} points at another file.", path.display()))
            }
        }
        Ok(_) => Check::Unsupported(format!(
            "{} was made by another tool, so it is left alone.",
            path.display()
        )),
    }
}

fn apply_desktop_entry() -> Result<String, String> {
    let appimage = appimage_path().ok_or("Not an AppImage.")?;
    let path = desktop_entry_path().ok_or("No home folder.")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, desktop_entry(&appimage))
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(format!("Wrote {}.", path.display()))
}

// The background server

fn check_server() -> Check {
    match crate::service::server_installed() {
        Some(true) => Check::Satisfied("The server starts when you log in.".into()),
        Some(false) => {
            Check::Missing("The server is not installed as a background service.".into())
        }
        None => Check::Unsupported("Could not read the server's status.".into()),
    }
}

fn apply_server() -> Result<String, String> {
    let out = crate::service::run_sync(&["server", "install"], crate::service::INSTALL_TIMEOUT)?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        return Err(if why.is_empty() { said } else { why });
    }
    crate::service::remember_installed_by_app(&crate::service::service_program());
    Ok("Installed the server as a background service.".into())
}

// ---------------------------------------------------------------- commands

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Every step for this system, looked at and not changed.
#[tauri::command(async)]
pub fn system_setup_status() -> Vec<StepReport> {
    status(&System, &Ledger::load())
}

/// Run one step (`step`), or every automatic one, and answer the whole list as
/// it stands afterwards. A step that is already right is not touched.
#[tauri::command(async)]
pub fn system_setup_run(step: Option<String>) -> Vec<StepReport> {
    run_and_answer(&System, step.as_deref())
}

fn run_and_answer(host: &dyn Host, step: Option<&str>) -> Vec<StepReport> {
    let mut ledger = Ledger::load();
    run(host, step, &mut ledger, &now());
    ledger.save();
    status(host, &ledger)
}

/// At app start: the automatic steps once for each version, so an install or an
/// update sets the machine up and a launch that follows it changes nothing. A
/// development build leaves the machine alone.
pub fn run_after_update(version: &str) {
    if cfg!(debug_assertions) {
        return;
    }
    run_for_version(&System, version);
}

fn run_for_version(host: &dyn Host, version: &str) {
    let mut ledger = Ledger::load();
    if run_once_per_version(host, version, &mut ledger, &now()) {
        ledger.save();
    }
}

/// Runs the automatic steps unless the record says they ran for `version`
/// already; answers whether it ran.
fn run_once_per_version(host: &dyn Host, version: &str, ledger: &mut Ledger, at: &str) -> bool {
    if ledger.ran_for.as_deref() == Some(version) {
        return false;
    }
    run(host, None, ledger, at);
    ledger.ran_for = Some(version.to_owned());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A machine whose steps start missing, and that records what was applied.
    struct Fake {
        os: Os,
        fixed: RefCell<Vec<String>>,
        applied: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(os: Os) -> Self {
            Fake {
                os,
                fixed: RefCell::new(Vec::new()),
                applied: RefCell::new(Vec::new()),
            }
        }
    }

    impl Host for Fake {
        fn os(&self) -> Os {
            self.os
        }

        fn check(&self, id: &str) -> Check {
            match id {
                // Only the user can fix these.
                "path-hint" | "long-paths" | "stable-signature" => {
                    Check::Manual(format!("do {id} yourself"))
                }
                "git" => Check::Unsupported("not here".into()),
                _ if self.fixed.borrow().iter().any(|f| f == id) => Check::Satisfied("ok".into()),
                _ => Check::Missing("missing".into()),
            }
        }

        fn apply(&self, id: &str) -> Result<String, String> {
            self.applied.borrow_mut().push(id.to_owned());
            if id == "server-service" {
                return Err("boom".into());
            }
            self.fixed.borrow_mut().push(id.to_owned());
            Ok(format!("fixed {id}"))
        }
    }

    fn ids(os: Os) -> Vec<&'static str> {
        steps_for(os).iter().map(|s| s.id).collect()
    }

    #[test]
    fn each_system_gets_its_own_steps() {
        assert_eq!(
            ids(Os::Macos),
            [
                "quarantine",
                "stable-signature",
                "cli-link",
                "path-hint",
                "server-service"
            ]
        );
        assert_eq!(
            ids(Os::Windows),
            ["user-path", "git", "long-paths", "server-service"]
        );
        assert_eq!(
            ids(Os::Linux),
            ["cli-link", "path-hint", "desktop-entry", "server-service"]
        );
    }

    #[test]
    fn step_ids_are_unique_and_every_step_has_a_system() {
        let mut seen = std::collections::BTreeSet::new();
        for step in STEPS {
            assert!(seen.insert(step.id), "{} listed twice", step.id);
            assert!(!step.os.is_empty(), "{} runs nowhere", step.id);
        }
    }

    #[test]
    fn the_background_server_waits_for_its_button() {
        let host = Fake::new(Os::Linux);
        let mut ledger = Ledger::default();
        let reports = run(&host, None, &mut ledger, "t1");
        let ran: Vec<_> = reports.iter().map(|r| r.id).collect();
        assert_eq!(ran, ["cli-link", "path-hint", "desktop-entry"]);
        assert_eq!(*host.applied.borrow(), ["cli-link", "desktop-entry"]);
    }

    #[test]
    fn running_again_applies_nothing_that_is_already_right() {
        let host = Fake::new(Os::Macos);
        let mut ledger = Ledger::default();
        run(&host, None, &mut ledger, "t1");
        assert_eq!(*host.applied.borrow(), ["quarantine", "cli-link"]);
        let again = run(&host, None, &mut ledger, "t2");
        // Nothing new was applied; the steps read as done.
        assert_eq!(*host.applied.borrow(), ["quarantine", "cli-link"]);
        let quarantine = again.iter().find(|r| r.id == "quarantine").unwrap();
        assert_eq!(quarantine.status, Status::Done);
        assert_eq!(quarantine.message, "ok");
    }

    #[test]
    fn a_step_only_the_user_can_do_is_reported_and_never_applied() {
        let host = Fake::new(Os::Windows);
        let mut ledger = Ledger::default();
        let reports = run(&host, None, &mut ledger, "t1");
        let long_paths = reports.iter().find(|r| r.id == "long-paths").unwrap();
        assert_eq!(long_paths.status, Status::NeedsYou);
        assert_eq!(long_paths.message, "do long-paths yourself");
        let git = reports.iter().find(|r| r.id == "git").unwrap();
        assert_eq!(git.status, Status::Skipped);
        assert_eq!(*host.applied.borrow(), ["user-path"]);
    }

    #[test]
    fn a_failed_step_says_why_and_the_others_still_run() {
        let host = Fake::new(Os::Linux);
        let mut ledger = Ledger::default();
        let reports = run(&host, Some("server-service"), &mut ledger, "t1");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].status, Status::Failed);
        assert_eq!(reports[0].message, "boom");
        assert_eq!(ledger.steps["server-service"].status, Status::Failed);
        // The automatic run does not touch it, and does the rest.
        let reports = run(&host, None, &mut ledger, "t2");
        assert!(reports.iter().all(|r| r.id != "server-service"));
        assert_eq!(
            *host.applied.borrow(),
            ["server-service", "cli-link", "desktop-entry"]
        );
    }

    #[test]
    fn a_step_for_another_system_is_not_run() {
        let host = Fake::new(Os::Macos);
        let mut ledger = Ledger::default();
        assert!(run(&host, Some("user-path"), &mut ledger, "t1").is_empty());
        assert!(run(&host, Some("nonsense"), &mut ledger, "t1").is_empty());
        assert!(host.applied.borrow().is_empty());
        assert!(ledger.steps.is_empty());
    }

    #[test]
    fn a_status_read_changes_nothing_and_shows_when_a_step_last_ran() {
        let host = Fake::new(Os::Linux);
        let mut ledger = Ledger::default();
        let before = status(&host, &ledger);
        let link = before.iter().find(|r| r.id == "cli-link").unwrap();
        assert_eq!(link.status, Status::Pending);
        assert_eq!(link.last_run, None);
        assert!(host.applied.borrow().is_empty());
        run(&host, Some("cli-link"), &mut ledger, "2026-11-13T10:00:00Z");
        let after = status(&host, &ledger);
        let link = after.iter().find(|r| r.id == "cli-link").unwrap();
        assert_eq!(link.status, Status::Done);
        assert_eq!(link.last_run.as_deref(), Some("2026-11-13T10:00:00Z"));
    }

    #[test]
    fn the_record_reads_back() {
        let mut ledger = Ledger {
            ran_for: Some("1.33.0".into()),
            ..Ledger::default()
        };
        ledger.steps.insert(
            "cli-link".into(),
            Recorded {
                status: Status::NeedsYou,
                message: "m".into(),
                at: "t".into(),
            },
        );
        let text = serde_json::to_string(&ledger).unwrap();
        assert!(text.contains("\"needs-you\""), "{text}");
        assert_eq!(serde_json::from_str::<Ledger>(&text).unwrap(), ledger);
        // A record from an older build, or an empty file, still reads.
        assert_eq!(
            serde_json::from_str::<Ledger>("{}").unwrap(),
            Ledger::default()
        );
    }

    #[test]
    fn adding_to_the_windows_path_keeps_what_is_there_and_never_twice() {
        assert_eq!(
            append_path_entry(r"C:\a;%USERPROFILE%\bin", r"C:\RustCode").as_deref(),
            Some(r"C:\a;%USERPROFILE%\bin;C:\RustCode")
        );
        assert_eq!(
            append_path_entry("", r"C:\RustCode").as_deref(),
            Some(r"C:\RustCode")
        );
        assert_eq!(
            append_path_entry(r"C:\a;", r"C:\RustCode").as_deref(),
            Some(r"C:\a;C:\RustCode")
        );
        // Already there: case and a trailing slash do not matter.
        assert_eq!(
            append_path_entry(r"C:\a;c:\rustcode\", r"C:\RustCode"),
            None
        );
    }

    #[test]
    fn a_registry_value_is_read_with_its_type() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    C:\\a;%USERPROFILE%\\bin\r\n\r\n";
        assert_eq!(
            parse_reg_query(out, "Path"),
            Some(("REG_EXPAND_SZ".into(), r"C:\a;%USERPROFILE%\bin".into()))
        );
        assert_eq!(parse_reg_query(out, "Other"), None);
        let dword = "    LongPathsEnabled    REG_DWORD    0x1\r\n";
        assert_eq!(
            parse_reg_query(dword, "longpathsenabled"),
            Some(("REG_DWORD".into(), "0x1".into()))
        );
    }

    #[test]
    fn an_adhoc_signature_is_told_from_a_stable_one() {
        let adhoc = "Executable=/Applications/RustCode.app/Contents/MacOS/rustcode-desktop\nSignature=adhoc\nTeamIdentifier=not set\n";
        assert!(is_adhoc(adhoc));
        let local = "Authority=RustCode Local Signing\nTeamIdentifier=not set\n";
        assert!(!is_adhoc(local));
        assert!(!is_adhoc(""));
    }

    #[test]
    fn a_launcher_entry_quotes_the_path_it_runs() {
        let entry = desktop_entry(Path::new("/home/a b/Rust$Code.AppImage"));
        assert!(
            entry.contains("Exec=\"/home/a b/Rust\\$Code.AppImage\" %U\n"),
            "{entry}"
        );
        assert!(entry.contains(ENTRY_MARKER));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert_eq!(exec_quote(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn the_path_hint_matches_the_shell() {
        assert!(path_hint_command("/bin/zsh").contains("~/.zprofile"));
        assert!(path_hint_command("/usr/bin/fish").starts_with("fish_add_path"));
        assert!(
            path_hint_command("/bin/bash").contains(".profile")
                || path_hint_command("/bin/bash").contains(".bash_profile")
        );
        assert!(path_hint_command("").contains("~/.zprofile"));
    }

    #[test]
    fn a_shell_path_is_searched_by_whole_entries() {
        let local = Path::new("/home/a/.local/bin");
        assert!(path_contains("/usr/bin:/home/a/.local/bin", local));
        assert!(!path_contains("/usr/bin:/home/a/.local/bin2", local));
        assert!(!path_contains("", local));
    }

    #[test]
    fn a_program_is_found_in_the_first_folder_that_has_it() {
        let dir = std::env::temp_dir().join(format!("rustcode-setup-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("rustcode"), "x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(b.join("rustcode"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        assert_eq!(
            find_in_dirs(&[a.clone(), b.clone()], "rustcode"),
            Some(b.join("rustcode"))
        );
        assert_eq!(find_in_dirs(&[a], "rustcode"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_automatic_steps_run_once_per_version() {
        let host = Fake::new(Os::Macos);
        let mut ledger = Ledger::default();
        assert!(run_once_per_version(&host, "1.33.0", &mut ledger, "t1"));
        assert_eq!(ledger.ran_for.as_deref(), Some("1.33.0"));
        let applied = host.applied.borrow().len();
        assert!(applied > 0);
        // The launch after it, and every one until the next update, does nothing.
        assert!(!run_once_per_version(&host, "1.33.0", &mut ledger, "t2"));
        assert_eq!(host.applied.borrow().len(), applied);
        // An update runs them again, and a step that is right is left alone.
        assert!(run_once_per_version(&host, "1.34.0", &mut ledger, "t3"));
        assert_eq!(host.applied.borrow().len(), applied);
        assert_eq!(ledger.ran_for.as_deref(), Some("1.34.0"));
    }
}
