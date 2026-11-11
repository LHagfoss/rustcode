//! Per-user background services: a definition of what to run, and one
//! manager per operating system that installs it so it starts when the user
//! logs in (`docs/server.md`).
//!
//! - [`launchd`]: a LaunchAgent in `~/Library/LaunchAgents` (macOS).
//! - [`systemd`]: a `systemd --user` unit (Linux and other Unix).
//! - [`windows_task`]: a per-user Scheduled Task at logon, or a `Run` key
//!   entry when tasks are not allowed (Windows).
//!
//! The renderers are plain functions that build the text of a definition, so
//! every platform's output is tested on every platform; only a manager's
//! methods touch the machine, and nothing here runs from a test.
//!
//! [`server_service`] is the one service today, the RustCode server;
//! [`path_binary`] reports the `rustcode` a shell runs, which may be an older
//! copy than the engine the user is talking to (issue #3411).

pub mod launchd;
pub mod logs;
pub mod path_binary;
pub mod server_service;
pub mod systemd;
pub mod windows_task;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// What the operating system should keep running for this user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// Identifies it to the manager: `server`, or `server-<hash>` for a
    /// server of another `RUSTCODE_CONFIG_DIR`. Lowercase ASCII and dashes.
    pub name: String,
    /// One line, for the unit or task description.
    pub description: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Environment the program needs and the login environment lacks.
    pub env: Vec<(String, String)>,
    /// Where its standard output and error go.
    pub log: PathBuf,
}

/// What a manager knows about a service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// A definition is in place.
    pub installed: bool,
    /// The manager has it started: loaded in launchd, active in systemd.
    pub active: bool,
    /// The process the manager runs.
    pub process: Process,
    /// Where the definition is (not set for a registry value).
    pub path: Option<PathBuf>,
    /// The program the installed definition runs.
    pub program: Option<PathBuf>,
}

/// Whether the manager runs a process of its own for the service.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Process {
    /// The manager cannot say (a Scheduled Task or Run entry shows no pid, or
    /// its tool failed): nothing follows from a server answering.
    #[default]
    Unknown,
    /// The manager runs nothing for it now.
    Stopped,
    /// The manager runs this process.
    Running(u32),
}

impl State {
    /// The process the manager runs, when it says.
    pub fn pid(&self) -> Option<u32> {
        match self.process {
            Process::Running(pid) => Some(pid),
            Process::Unknown | Process::Stopped => None,
        }
    }
}

/// What [`Manager::install`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The definition is new and the service was started.
    Started,
    /// The definition was already as wanted.
    Unchanged,
    /// The definition changed under a service that was already started: the
    /// running process keeps the old one until it is restarted.
    ChangedWhileActive,
}

/// Something wrong that the user can fix, with a stable code for scripts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
}

/// Installs, inspects and removes a [`Definition`] on this operating system.
pub trait Manager {
    /// `launchd`, `systemd`, `scheduled-task` or `run-key`.
    fn kind(&self) -> &'static str;
    fn state(&self) -> Result<State>;
    /// Whether [`install`](Manager::install) can take `definition`, without
    /// changing anything. Asked before a running server is stopped for the
    /// service, so an install that would fail leaves it alone.
    fn check(&self, _definition: &Definition) -> Result<()> {
        Ok(())
    }
    /// Write `definition`, and start it unless it is already started.
    fn install(&self, definition: &Definition) -> Result<Installed>;
    /// Stop the service and start it again from its installed definition.
    fn restart(&self) -> Result<()>;
    /// Stop it and remove the definition. Data is never touched.
    fn uninstall(&self) -> Result<()>;
    /// What stops it running when the user is not logged in, and the like.
    fn problems(&self) -> Vec<Problem> {
        Vec::new()
    }
}

/// The manager for this operating system.
pub fn manager(name: &str) -> Result<Box<dyn Manager>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(launchd::Launchd::new(name)?))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(Box::new(systemd::Systemd::new(name)?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(windows_task::WindowsTask::new(name)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = name;
        anyhow::bail!("background services are not supported on this operating system")
    }
}

/// The `rustcode` that a service should run. A Homebrew symlink is kept as it
/// is, so the service follows the formula's upgrades.
pub fn executable_path() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    for candidate in [
        PathBuf::from("/opt/homebrew/bin/rustcode"),
        PathBuf::from("/usr/local/bin/rustcode"),
    ] {
        if candidate.canonicalize().ok().as_ref() == Some(&current) {
            return Ok(candidate);
        }
    }
    Ok(current)
}

/// Refuse to install a development build: a service would keep running a
/// binary that the next `cargo build` replaces or `cargo clean` deletes.
pub fn refuse_dev_binary(program: &Path, force_dev: bool) -> Result<()> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    if !force_dev && is_dev_binary(program, target_dir.as_deref()) {
        anyhow::bail!(
            "{} is a development build (inside a cargo target directory); a service must not run it. \
             Install RustCode and run `rustcode server install` from that, or pass --force-dev to install it anyway.",
            program.display()
        );
    }
    Ok(())
}

/// Whether `program` is cargo build output: under `target/debug`,
/// `target/release` or `target/<triple>/{debug,release}`, or under
/// `CARGO_TARGET_DIR`.
pub fn is_dev_binary(program: &Path, cargo_target_dir: Option<&Path>) -> bool {
    if let Some(dir) = cargo_target_dir
        && !dir.as_os_str().is_empty()
        && program.starts_with(dir)
    {
        return true;
    }
    let is_profile = |name: &std::ffi::OsStr| name == "debug" || name == "release";
    program.ancestors().skip(1).any(|ancestor| {
        if ancestor.file_name() != Some(std::ffi::OsStr::new("target")) {
            return false;
        }
        let Ok(rest) = program.strip_prefix(ancestor) else {
            return false;
        };
        let mut parts = rest.components().map(|part| part.as_os_str());
        match (parts.next(), parts.next()) {
            (Some(first), _) if is_profile(first) => true,
            (Some(_), Some(second)) => is_profile(second) && parts.next().is_some(),
            _ => false,
        }
    })
}

/// Text for an XML element or attribute.
pub(crate) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The text that [`xml_escape`] made.
pub(crate) fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// One word of a systemd command line or assignment: quoted when it holds
/// anything but safe characters, with the characters systemd would expand
/// (`%` specifiers, `$` variables) doubled.
pub(crate) fn systemd_word(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@+,".contains(c));
    if safe {
        return value.to_owned();
    }
    let mut quoted = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            '\n' => quoted.push_str("\\n"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// One argument of a Windows command line, quoted the way `CommandLineToArgvW`
/// reads it back.
pub(crate) fn windows_argument(value: &str) -> String {
    if !value.is_empty() && !value.contains([' ', '\t', '"']) {
        return value.to_owned();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for c in value.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            c => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// The user's home directory.
pub(crate) fn home_dir() -> Result<PathBuf> {
    crate::home::home_dir().context("the home directory is unknown")
}

/// Run a service manager's command line tool.
pub(crate) fn run_tool(program: &str, args: &[&str]) -> Result<std::process::Output> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("could not run {program}"))
}

/// The full path of a Windows system tool (`schtasks.exe`, `reg.exe`,
/// `taskkill.exe`), so a service command never resolves it through the current
/// directory or `PATH`, where another program could have put its own.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn windows_system_tool(exe: &str) -> String {
    windows_system_tool_in(std::env::var_os("SystemRoot").as_deref(), exe)
}

/// [`windows_system_tool`] with `SystemRoot` given: `C:\Windows` when it is
/// unset or empty.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_system_tool_in(system_root: Option<&std::ffi::OsStr>, exe: &str) -> String {
    let root = system_root
        .filter(|root| !root.is_empty())
        .map(|root| root.to_string_lossy().into_owned())
        .unwrap_or_else(|| "C:\\Windows".to_owned());
    format!("{}\\System32\\{exe}", root.trim_end_matches(['\\', '/']))
}

/// A tool's failure as one line: its error output, else its output.
pub(crate) fn failure(output: &std::process::Output) -> String {
    let text = if output.stderr.iter().any(|byte| !byte.is_ascii_whitespace()) {
        &output.stderr
    } else {
        &output.stdout
    };
    String::from_utf8_lossy(text).trim().to_owned()
}

/// Write a definition file for this user only, replacing an old one whole.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .context("the definition has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tools_are_full_system32_paths() {
        use std::ffi::OsStr;
        assert_eq!(
            windows_system_tool_in(Some(OsStr::new("D:\\WINNT")), "schtasks.exe"),
            "D:\\WINNT\\System32\\schtasks.exe"
        );
        assert_eq!(
            windows_system_tool_in(Some(OsStr::new("C:\\Windows\\")), "reg.exe"),
            "C:\\Windows\\System32\\reg.exe"
        );
        for unset in [None, Some(OsStr::new(""))] {
            assert_eq!(
                windows_system_tool_in(unset, "taskkill.exe"),
                "C:\\Windows\\System32\\taskkill.exe"
            );
        }
    }

    #[test]
    fn cargo_build_output_is_a_dev_binary() {
        for path in [
            "/work/rustcode/target/debug/rustcode",
            "/work/rustcode/target/release/rustcode",
            "/work/rustcode/target/aarch64-apple-darwin/release/rustcode",
            "/work/rustcode/apps/desktop/src-tauri/target/debug/rustcode",
            "C:/work/rustcode/target/debug/rustcode.exe",
        ] {
            assert!(is_dev_binary(Path::new(path), None), "{path}");
        }
    }

    #[test]
    fn installed_binaries_are_not_dev_binaries() {
        for path in [
            "/usr/local/bin/rustcode",
            "/opt/homebrew/bin/rustcode",
            "/Applications/RustCode.app/Contents/MacOS/rustcode",
            "/home/u/.cargo/bin/rustcode",
            "/home/u/target/bin/rustcode",
            "/home/u/target-dir/debug/rustcode",
            "/home/u/target/rustcode",
        ] {
            assert!(!is_dev_binary(Path::new(path), None), "{path}");
        }
    }

    #[test]
    fn cargo_target_dir_is_a_dev_location() {
        let dir = Path::new("/scratch/build");
        assert!(is_dev_binary(
            Path::new("/scratch/build/x/rustcode"),
            Some(dir)
        ));
        assert!(!is_dev_binary(
            Path::new("/usr/local/bin/rustcode"),
            Some(dir)
        ));
        assert!(!is_dev_binary(
            Path::new("/usr/local/bin/rustcode"),
            Some(Path::new(""))
        ));
    }

    #[test]
    fn a_dev_binary_is_refused_unless_forced() {
        let dev = Path::new("/work/rustcode/target/debug/rustcode");
        let error = refuse_dev_binary(dev, false).unwrap_err().to_string();
        assert!(error.contains("--force-dev"), "{error}");
        assert!(error.contains("/work/rustcode/target/debug/rustcode"));
        refuse_dev_binary(dev, true).expect("forced");
        refuse_dev_binary(Path::new("/usr/local/bin/rustcode"), false).expect("installed");
    }

    #[test]
    fn systemd_words_are_quoted_only_when_needed() {
        assert_eq!(systemd_word("/usr/bin/rustcode"), "/usr/bin/rustcode");
        assert_eq!(systemd_word("--service"), "--service");
        assert_eq!(
            systemd_word("/home/Ada Lovelace/bin/rustcode"),
            "\"/home/Ada Lovelace/bin/rustcode\""
        );
        assert_eq!(systemd_word("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(systemd_word("100%$HOME"), "\"100%%$$HOME\"");
        assert_eq!(systemd_word(""), "\"\"");
    }

    #[test]
    fn windows_arguments_round_trip_the_argv_rules() {
        assert_eq!(windows_argument("server"), "server");
        assert_eq!(
            windows_argument(r"C:\Program Files\RustCode\rustcode.exe"),
            r#""C:\Program Files\RustCode\rustcode.exe""#
        );
        // Backslashes before a closing quote are doubled; others are not.
        assert_eq!(windows_argument(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        assert_eq!(windows_argument(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(windows_argument(""), "\"\"");
    }

    #[test]
    fn xml_text_is_escaped() {
        assert_eq!(xml_escape("a&b<c>\"d\""), "a&amp;b&lt;c&gt;&quot;d&quot;");
    }

    #[test]
    fn writing_a_definition_replaces_it_whole() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("nested/unit");
        write_private(&path, b"one").expect("first");
        write_private(&path, b"two").expect("second");
        assert_eq!(std::fs::read(&path).expect("read"), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
