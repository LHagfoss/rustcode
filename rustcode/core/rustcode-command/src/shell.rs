//! Which shell runs a command string, and how that shell is invoked.
//!
//! Every decision here is a pure function of a [`ShellKind`], so the Windows
//! behavior can be unit-tested on any host. The `cfg(windows)` call sites only
//! probe `PATH` and hand the result to these functions.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The shell family a command string is written for and executed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// `/bin/bash` (or `sh`) on macOS and Linux.
    Posix,
    /// PowerShell 7+ (`pwsh.exe`).
    Pwsh,
    /// Windows PowerShell 5.1 (`powershell.exe`), shipped with Windows.
    WindowsPowerShell,
    /// `cmd.exe`, the last resort when no PowerShell is on `PATH`.
    Cmd,
}

impl ShellKind {
    /// Short name shown in the UI for a shell tool call.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Posix => "Bash",
            Self::Pwsh | Self::WindowsPowerShell => "PowerShell",
            Self::Cmd => "Cmd",
        }
    }

    pub const fn is_powershell(self) -> bool {
        matches!(self, Self::Pwsh | Self::WindowsPowerShell)
    }

    /// One-line statement of the shell for the model's runtime context.
    /// `None` on POSIX hosts, where the caller keeps reporting `$SHELL`.
    pub const fn prompt_summary(self) -> Option<&'static str> {
        match self {
            Self::Posix => None,
            Self::Pwsh => Some("PowerShell 7 (pwsh); `&&` and `||` are available"),
            Self::WindowsPowerShell => Some(
                "Windows PowerShell 5.1 (powershell.exe); no `&&`, `||` or ternary, chain with `;`",
            ),
            Self::Cmd => Some(
                "cmd.exe (no PowerShell found on PATH); write cmd syntax, not PowerShell or POSIX sh",
            ),
        }
    }

    const fn program(self) -> &'static str {
        match self {
            Self::Posix => "sh",
            Self::Pwsh => "pwsh.exe",
            Self::WindowsPowerShell => "powershell.exe",
            Self::Cmd => "cmd.exe",
        }
    }
}

/// Pick the Windows shell from what `on_path` reports: PowerShell 7, then
/// Windows PowerShell, then `cmd.exe`. Probing stops at the first hit.
pub fn select_windows_shell(mut on_path: impl FnMut(&str) -> bool) -> ShellKind {
    [ShellKind::Pwsh, ShellKind::WindowsPowerShell]
        .into_iter()
        .find(|kind| on_path(kind.program()))
        .unwrap_or(ShellKind::Cmd)
}

/// The shell that runs commands on this host. Resolved once per process.
pub fn host_shell() -> ShellKind {
    #[cfg(target_os = "windows")]
    {
        windows_shell().0
    }
    #[cfg(not(target_os = "windows"))]
    {
        ShellKind::Posix
    }
}

/// UI label for a shell tool call on this host.
pub fn shell_label() -> &'static str {
    host_shell().label()
}

/// The resolved Windows shell and the executable to spawn for it. The absolute
/// path is kept because callers replace `PATH` in the child environment.
#[cfg(target_os = "windows")]
pub(crate) fn windows_shell() -> &'static (ShellKind, PathBuf) {
    static SHELL: std::sync::OnceLock<(ShellKind, PathBuf)> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        let path = std::env::var_os("PATH");
        let mut program = None;
        let kind = select_windows_shell(|name| {
            program = find_on_path(path.as_deref(), name);
            program.is_some()
        });
        (
            kind,
            program.unwrap_or_else(|| PathBuf::from(kind.program())),
        )
    })
}

#[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
fn find_on_path(path: Option<&OsStr>, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// How the arguments of a shell invocation reach the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellArguments {
    /// Ordinary arguments, quoted by the standard library.
    Escaped(Vec<String>),
    /// A command-line tail appended verbatim. `cmd.exe` does not follow the C
    /// runtime's quoting rules, so escaping its command would corrupt quotes.
    Raw(String),
}

/// Sets up a non-interactive session whose output is UTF-8 without a BOM.
/// Progress records are silenced because a redirected host serializes them
/// onto stderr.
const POWERSHELL_PRELUDE: &str = "$ProgressPreference = 'SilentlyContinue'\n\
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false; $OutputEncoding = [Console]::OutputEncoding } catch {}\n";

/// Turns the outcome of the last statement into the process exit code.
/// `-Command` and `-EncodedCommand` otherwise collapse every failure to 1 and
/// lose a native program's code. A failed status with a zero native exit code
/// whose newest error is native stderr text is Windows PowerShell's
/// `2>&1` artifact (`cargo build 2>&1`), not a failure.
const POWERSHELL_EXIT_TRAILER: &str = "if ($?) { exit 0 }\n\
if ($LASTEXITCODE) { exit $LASTEXITCODE }\n\
if ($null -ne $LASTEXITCODE -and $Error.Count -gt 0 -and $Error[0].Exception -is [System.Management.Automation.RemoteException]) { exit 0 }\n\
exit 1\n";

/// The complete script PowerShell runs for `command`.
pub fn powershell_script(command: &str) -> String {
    format!("{POWERSHELL_PRELUDE}{command}\n{POWERSHELL_EXIT_TRAILER}")
}

/// Arguments that run `command` in `kind`.
///
/// PowerShell receives the script as `-EncodedCommand` (base64 of UTF-16LE):
/// `-Command` re-parses its argument after Windows command-line quoting, which
/// mangles embedded quotes, and reading the script from stdin would take away
/// the null stdin that keeps child programs from waiting for input.
pub fn shell_arguments(kind: ShellKind, command: &str) -> ShellArguments {
    match kind {
        ShellKind::Posix => ShellArguments::Escaped(vec!["-c".to_owned(), command.to_owned()]),
        ShellKind::Pwsh | ShellKind::WindowsPowerShell => ShellArguments::Escaped(
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                // Process scope only. The default Restricted policy blocks the
                // `.ps1` shims npm, npx, pnpm and yarn resolve to; a policy
                // set by Group Policy still wins over this switch.
                "-ExecutionPolicy",
                "Bypass",
                "-OutputFormat",
                "Text",
                "-EncodedCommand",
                &encode_powershell_command(&powershell_script(command)),
            ]
            .map(str::to_owned)
            .to_vec(),
        ),
        // `/S` strips exactly the outer quotes and runs the rest verbatim;
        // `/D` skips AutoRun commands from the registry.
        ShellKind::Cmd => ShellArguments::Raw(format!("/D /S /C \"{command}\"")),
    }
}

/// Windows caps a process command line at 32,767 UTF-16 units, and `cmd.exe`
/// caps its own at 8,191. Leave room for the executable path and switches.
const MAX_ENCODED_POWERSHELL_COMMAND: usize = 32_000;
const MAX_CMD_COMMAND: usize = 8_000;

/// Explain a command that cannot fit on the shell's command line, instead of
/// letting process creation fail with an opaque OS error.
pub fn command_length_error(kind: ShellKind, command: &str) -> Option<String> {
    match kind {
        ShellKind::Posix => None,
        ShellKind::Pwsh | ShellKind::WindowsPowerShell => {
            let units = powershell_script(command).encode_utf16().count();
            (base64_len(units * 2) > MAX_ENCODED_POWERSHELL_COMMAND).then(|| {
                format!(
                    "command is too long to pass to PowerShell ({} characters); write it to a .ps1 file and run that file instead",
                    command.chars().count()
                )
            })
        }
        ShellKind::Cmd => {
            let units = command.encode_utf16().count();
            (units > MAX_CMD_COMMAND).then(|| {
                format!(
                    "command is too long for cmd.exe ({units} characters, limit {MAX_CMD_COMMAND}); write it to a .cmd file and run that file instead"
                )
            })
        }
    }
}

/// Wrap a detached command so the shell stays alive as the process-tree root
/// while nothing it starts inherits RustCode's output pipes.
pub fn detached_command(kind: ShellKind, command: &str, has_background_operator: bool) -> String {
    match kind {
        ShellKind::Posix if has_background_operator => {
            format!("{{ {command}; wait; }} </dev/null >/dev/null 2>&1")
        }
        ShellKind::Posix => format!("{{ {command}; }} </dev/null >/dev/null 2>&1"),
        // The status of a script block call does not reflect what ran inside
        // it, so the exit code is decided before the block returns.
        ShellKind::Pwsh | ShellKind::WindowsPowerShell => {
            format!("& {{\n{command}\n{POWERSHELL_EXIT_TRAILER}}} *> $null")
        }
        ShellKind::Cmd => format!("({command}) <nul >nul 2>&1"),
    }
}

fn encode_powershell_command(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    base64(&bytes)
}

const fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(base64_len(bytes.len()));
    for chunk in bytes.chunks(3) {
        let group = u32::from(chunk[0]) << 16
            | u32::from(chunk.get(1).copied().unwrap_or(0)) << 8
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        for position in 0..4 {
            if position <= chunk.len() {
                let index = (group >> (18 - 6 * position)) & 0x3f;
                out.push(ALPHABET[index as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_powershell_command(encoded: &str) -> String {
        let mut bits = 0u32;
        let mut bit_count = 0;
        let mut bytes = Vec::new();
        for ch in encoded.bytes().filter(|byte| *byte != b'=') {
            let value = match ch {
                b'A'..=b'Z' => ch - b'A',
                b'a'..=b'z' => ch - b'a' + 26,
                b'0'..=b'9' => ch - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                other => panic!("not base64: {other}"),
            };
            bits = bits << 6 | u32::from(value);
            bit_count += 6;
            if bit_count >= 8 {
                bit_count -= 8;
                bytes.push((bits >> bit_count) as u8);
            }
        }
        let units = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units).expect("valid UTF-16")
    }

    #[test]
    fn base64_matches_reference_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
            assert_eq!(base64_len(input.len()), expected.len());
        }
        // `dir` as UTF-16LE, the encoding `-EncodedCommand` expects.
        assert_eq!(encode_powershell_command("dir"), "ZABpAHIA");
    }

    #[test]
    fn windows_shell_preference_is_pwsh_then_powershell_then_cmd() {
        assert_eq!(select_windows_shell(|_| true), ShellKind::Pwsh);
        assert_eq!(
            select_windows_shell(|name| name == "powershell.exe"),
            ShellKind::WindowsPowerShell
        );
        assert_eq!(select_windows_shell(|_| false), ShellKind::Cmd);

        let mut probed = Vec::new();
        select_windows_shell(|name| {
            probed.push(name.to_owned());
            true
        });
        assert_eq!(probed, ["pwsh.exe"], "probing stops at the first hit");
    }

    #[test]
    fn find_on_path_returns_the_first_directory_holding_the_program() {
        let root = std::env::temp_dir().join(format!("rustcode-shell-{}", std::process::id()));
        let (empty, holding) = (root.join("empty"), root.join("holding"));
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&holding).unwrap();
        std::fs::write(holding.join("pwsh.exe"), b"").unwrap();
        let path = std::env::join_paths([&empty, &holding]).unwrap();

        assert_eq!(
            find_on_path(Some(&path), "pwsh.exe"),
            Some(holding.join("pwsh.exe"))
        );
        assert_eq!(find_on_path(Some(&path), "powershell.exe"), None);
        assert_eq!(find_on_path(None, "pwsh.exe"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn labels_name_the_shell_that_runs_the_command() {
        assert_eq!(ShellKind::Posix.label(), "Bash");
        assert_eq!(ShellKind::Pwsh.label(), "PowerShell");
        assert_eq!(ShellKind::WindowsPowerShell.label(), "PowerShell");
        assert_eq!(ShellKind::Cmd.label(), "Cmd");
        #[cfg(not(target_os = "windows"))]
        assert_eq!(shell_label(), "Bash");
    }

    #[test]
    fn prompt_summary_is_absent_on_posix_and_names_each_windows_shell() {
        assert_eq!(ShellKind::Posix.prompt_summary(), None);
        assert!(
            ShellKind::Pwsh
                .prompt_summary()
                .unwrap()
                .contains("PowerShell 7")
        );
        assert!(
            ShellKind::WindowsPowerShell
                .prompt_summary()
                .unwrap()
                .contains("5.1")
        );
        assert!(ShellKind::Cmd.prompt_summary().unwrap().contains("cmd.exe"));
    }

    #[test]
    fn powershell_arguments_carry_any_command_text_intact() {
        let command = "Write-Output \"it's \\\"quoted\\\"\" | Select-String 'a|b' # 100% ünïcödé ✓\n$env:X = 'y'";
        for kind in [ShellKind::Pwsh, ShellKind::WindowsPowerShell] {
            let ShellArguments::Escaped(arguments) = shell_arguments(kind, command) else {
                panic!("PowerShell arguments are escaped normally");
            };
            assert_eq!(
                arguments[..8],
                [
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-OutputFormat",
                    "Text",
                    "-EncodedCommand"
                ]
            );
            assert_eq!(arguments.len(), 9);
            // Nothing in the payload needs, or can be damaged by, quoting.
            assert!(
                arguments[8]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
            );
            let script = decode_powershell_command(&arguments[8]);
            assert_eq!(script, powershell_script(command));
            assert!(script.contains(command));
        }
    }

    #[test]
    fn powershell_script_reports_the_last_statement_status() {
        let script = powershell_script("cargo test # trailing comment");
        let command_at = script.find("cargo test").unwrap();
        let trailer_at = script.find("if ($?) { exit 0 }").unwrap();
        assert!(script.starts_with("$ProgressPreference = 'SilentlyContinue'\n"));
        assert!(script.contains("UTF8Encoding $false"));
        // The trailer starts on its own line so a trailing comment in the
        // command cannot swallow it, and nothing runs between the two.
        assert_eq!(
            &script[command_at..trailer_at],
            "cargo test # trailing comment\n"
        );
        assert!(script.contains("if ($LASTEXITCODE) { exit $LASTEXITCODE }\n"));
        assert!(script.ends_with("exit 1\n"));
    }

    #[test]
    fn cmd_arguments_pass_the_command_verbatim() {
        assert_eq!(
            shell_arguments(ShellKind::Cmd, "echo \"a b\" & dir"),
            ShellArguments::Raw("/D /S /C \"echo \"a b\" & dir\"".to_owned())
        );
    }

    #[test]
    fn posix_arguments_are_a_plain_dash_c() {
        assert_eq!(
            shell_arguments(ShellKind::Posix, "ls -la"),
            ShellArguments::Escaped(vec!["-c".to_owned(), "ls -la".to_owned()])
        );
    }

    #[test]
    fn detached_wrappers_use_each_shell_syntax() {
        assert_eq!(
            detached_command(ShellKind::Posix, "server", false),
            "{ server; } </dev/null >/dev/null 2>&1"
        );
        assert_eq!(
            detached_command(ShellKind::Posix, "server &", true),
            "{ server &; wait; } </dev/null >/dev/null 2>&1"
        );
        assert_eq!(
            detached_command(ShellKind::Cmd, "server", false),
            "(server) <nul >nul 2>&1"
        );
        for kind in [ShellKind::Pwsh, ShellKind::WindowsPowerShell] {
            let wrapped = detached_command(kind, "npm run dev", false);
            assert!(wrapped.starts_with("& {\nnpm run dev\nif ($?) { exit 0 }\n"));
            assert!(wrapped.ends_with("exit 1\n} *> $null"));
            assert!(!wrapped.contains("nul "), "no cmd redirection: {wrapped}");
        }
    }

    #[test]
    fn overlong_commands_are_rejected_with_an_actionable_message() {
        let long = "x".repeat(20_000);
        assert_eq!(command_length_error(ShellKind::Posix, &long), None);
        for kind in [ShellKind::Pwsh, ShellKind::WindowsPowerShell] {
            assert_eq!(command_length_error(kind, "Get-ChildItem"), None);
            assert_eq!(command_length_error(kind, &"x".repeat(10_000)), None);
            let error = command_length_error(kind, &long).unwrap();
            assert!(
                error.contains("too long") && error.contains(".ps1"),
                "{error}"
            );
        }
        assert_eq!(command_length_error(ShellKind::Cmd, "dir"), None);
        assert!(command_length_error(ShellKind::Cmd, &long).is_some());
    }
}
