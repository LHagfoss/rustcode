//! macOS: a LaunchAgent in `~/Library/LaunchAgents`, bootstrapped into the
//! user's `gui/<uid>` domain. It starts at login (`RunAtLoad`) and comes back
//! after a crash (`KeepAlive`); a server asked to stop, which exits cleanly,
//! stays stopped until the next login or `server restart`.

use super::{
    Definition, Installed, Manager, State, failure, home_dir, run_tool, write_private, xml_escape,
    xml_unescape,
};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;

/// The agent's label, which names its file: `no.rustcode.<name>`.
pub fn label(name: &str) -> String {
    format!("no.rustcode.{name}")
}

/// The LaunchAgent document for `definition`. Each argument is its own
/// `<string>`, so a path with spaces needs no quoting.
pub fn render(definition: &Definition) -> String {
    let arguments = std::iter::once(definition.program.to_string_lossy().into_owned())
        .chain(definition.args.iter().cloned())
        .map(|argument| format!("\t\t<string>{}</string>\n", xml_escape(&argument)))
        .collect::<String>();
    let environment = if definition.env.is_empty() {
        String::new()
    } else {
        let pairs = definition
            .env
            .iter()
            .map(|(key, value)| {
                format!(
                    "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                    xml_escape(key),
                    xml_escape(value)
                )
            })
            .collect::<String>();
        format!("\t<key>EnvironmentVariables</key>\n\t<dict>\n{pairs}\t</dict>\n")
    };
    let log = xml_escape(&definition.log.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\
\t<string>{label}</string>\n\
\t<key>ProgramArguments</key>\n\
\t<array>\n{arguments}\t</array>\n\
{environment}\
\t<key>RunAtLoad</key>\n\
\t<true/>\n\
\t<key>KeepAlive</key>\n\
\t<dict>\n\
\t\t<key>SuccessfulExit</key>\n\
\t\t<false/>\n\
\t</dict>\n\
\t<key>ThrottleInterval</key>\n\
\t<integer>5</integer>\n\
\t<key>StandardOutPath</key>\n\
\t<string>{log}</string>\n\
\t<key>StandardErrorPath</key>\n\
\t<string>{log}</string>\n\
</dict>\n\
</plist>\n",
        label = xml_escape(&label(&definition.name)),
    )
}

/// The program an installed agent runs: its first `ProgramArguments` string.
pub fn program_of(document: &str) -> Option<PathBuf> {
    let (_, rest) = document.split_once("<key>ProgramArguments</key>")?;
    let (_, rest) = rest.split_once("<string>")?;
    let (program, _) = rest.split_once("</string>")?;
    Some(PathBuf::from(xml_unescape(program)))
}

/// The pid in `launchctl print` output (`pid = 123`), when it has one.
pub fn parse_print_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(" = ")?;
        (key == "pid").then(|| value.trim().parse().ok())?
    })
}

pub struct Launchd {
    label: String,
    path: PathBuf,
    domain: String,
}

impl Launchd {
    pub fn new(name: &str) -> Result<Self> {
        let label = label(name);
        let path = home_dir()?
            .join("Library/LaunchAgents")
            .join(format!("{label}.plist"));
        Ok(Self {
            label,
            path,
            domain: format!("gui/{}", uid()),
        })
    }

    fn service(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    fn print(&self) -> Result<Option<String>> {
        let output = run_tool("/bin/launchctl", &["print", &self.service()])?;
        Ok(output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
    }

    fn bootstrap(&self) -> Result<()> {
        let path = self.path.to_string_lossy().into_owned();
        let mut last = String::new();
        // A job removed a moment ago can still be registered.
        for _ in 0..10 {
            let output = run_tool("/bin/launchctl", &["bootstrap", &self.domain, &path])?;
            if output.status.success() || self.print()?.is_some() {
                return Ok(());
            }
            last = failure(&output);
            std::thread::sleep(Duration::from_millis(500));
        }
        anyhow::bail!("launchctl could not load {}: {last}", self.path.display())
    }

    fn bootout(&self) -> Result<()> {
        if self.print()?.is_none() {
            return Ok(());
        }
        let _ = run_tool("/bin/launchctl", &["bootout", &self.service()]);
        // Removal finishes after the command returns.
        for _ in 0..100 {
            if self.print()?.is_none() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        anyhow::bail!("launchd did not stop {}", self.label)
    }
}

impl Manager for Launchd {
    fn kind(&self) -> &'static str {
        "launchd"
    }

    fn state(&self) -> Result<State> {
        let document = std::fs::read_to_string(&self.path).ok();
        let printed = self.print()?;
        Ok(State {
            installed: document.is_some(),
            active: printed.is_some(),
            pid: printed.as_deref().and_then(parse_print_pid),
            path: document.is_some().then(|| self.path.clone()),
            program: document.as_deref().and_then(program_of),
        })
    }

    fn install(&self, definition: &Definition) -> Result<Installed> {
        if let Some(parent) = definition.log.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let document = render(definition);
        let existing = std::fs::read_to_string(&self.path).ok();
        let changed = existing.as_deref() != Some(document.as_str());
        if changed {
            write_private(&self.path, document.as_bytes())?;
        }
        let Some(printed) = self.print()? else {
            self.bootstrap()?;
            return Ok(Installed::Started);
        };
        if parse_print_pid(&printed).is_none() {
            // Loaded but not running: it stopped cleanly, which `KeepAlive`
            // leaves alone. A changed definition needs loading again.
            if changed {
                self.restart()?;
            } else {
                let output = run_tool("/bin/launchctl", &["kickstart", &self.service()])?;
                anyhow::ensure!(
                    output.status.success(),
                    "launchctl could not start {}: {}",
                    self.label,
                    failure(&output)
                );
            }
            return Ok(Installed::Started);
        }
        Ok(if changed {
            Installed::ChangedWhileActive
        } else {
            Installed::Unchanged
        })
    }

    fn restart(&self) -> Result<()> {
        // Not `kickstart -k`: that keeps the definition loaded earlier.
        self.bootout()?;
        self.bootstrap()
    }

    fn uninstall(&self) -> Result<()> {
        self.bootout()?;
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("could not remove {}", self.path.display()))
            }
            _ => Ok(()),
        }
    }
}

/// The user whose `gui/<uid>` domain the agent lives in.
fn uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }
    // launchd exists only on macOS; this keeps the module compiling elsewhere.
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(program: &str) -> Definition {
        Definition {
            name: "server".to_owned(),
            description: "RustCode server".to_owned(),
            program: PathBuf::from(program),
            args: vec![
                "server".to_owned(),
                "run".to_owned(),
                "--service".to_owned(),
            ],
            env: Vec::new(),
            log: PathBuf::from("/Users/Ada Lovelace/.config/rustcode/logs/server.log"),
        }
    }

    #[test]
    fn the_agent_starts_at_login_and_returns_after_a_crash() {
        let plist = render(&definition("/opt/homebrew/bin/rustcode"));
        assert!(plist.contains("<key>Label</key>\n\t<string>no.rustcode.server</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(plist.contains(
            "<string>/opt/homebrew/bin/rustcode</string>\n\t\t<string>server</string>\n\t\t<string>run</string>\n\t\t<string>--service</string>"
        ));
        assert!(!plist.contains("EnvironmentVariables"));
    }

    #[test]
    fn paths_with_spaces_and_markup_stay_one_argument() {
        let mut definition = definition("/Applications/Rust Code & Co.app/Contents/MacOS/rustcode");
        definition.env = vec![(
            "RUSTCODE_CONFIG_DIR".to_owned(),
            "/Users/Ada Lovelace/rc <test>".to_owned(),
        )];
        let plist = render(&definition);
        assert!(plist.contains(
            "<string>/Applications/Rust Code &amp; Co.app/Contents/MacOS/rustcode</string>"
        ));
        assert!(plist.contains("<key>RUSTCODE_CONFIG_DIR</key>\n\t\t<string>/Users/Ada Lovelace/rc &lt;test&gt;</string>"));
        assert!(plist.contains(
            "<key>StandardOutPath</key>\n\t<string>/Users/Ada Lovelace/.config/rustcode/logs/server.log</string>"
        ));
        assert!(plist.contains("<key>StandardErrorPath</key>"));
        assert_eq!(
            program_of(&plist),
            Some(PathBuf::from(
                "/Applications/Rust Code & Co.app/Contents/MacOS/rustcode"
            ))
        );
    }

    #[test]
    fn rendering_is_stable() {
        let one = render(&definition("/usr/local/bin/rustcode"));
        assert_eq!(one, render(&definition("/usr/local/bin/rustcode")));
    }

    #[test]
    fn another_config_directory_gets_its_own_label() {
        assert_eq!(label("server-1a2b3c4d"), "no.rustcode.server-1a2b3c4d");
    }

    #[test]
    fn the_pid_comes_from_launchctl_print() {
        let output = "gui/501/no.rustcode.server = {\n\tstate = running\n\tpid = 4242\n\tlast exit code = (never exited)\n}";
        assert_eq!(parse_print_pid(output), Some(4242));
        assert_eq!(
            parse_print_pid("\tstate = waiting\n\tlast exit code = 1"),
            None
        );
    }
}
