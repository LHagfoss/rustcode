//! Linux: a `systemd --user` unit in `~/.config/systemd/user`. It starts when
//! the user's manager does, which is at login, or at boot when the user has
//! lingering enabled (`loginctl enable-linger`); without it the server stops
//! at logout. Enabling it needs root, so it is printed, never run.

use super::{
    Definition, Installed, Manager, Problem, State, failure, home_dir, run_tool, systemd_word,
    write_private,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// The unit's file name: `rustcode-<name>.service`.
pub fn unit_name(name: &str) -> String {
    format!("rustcode-{name}.service")
}

/// The unit file for `definition`.
pub fn render(definition: &Definition) -> String {
    let command = std::iter::once(definition.program.to_string_lossy().into_owned())
        .chain(definition.args.iter().cloned())
        .map(|word| systemd_word(&word))
        .collect::<Vec<_>>()
        .join(" ");
    let environment = definition
        .env
        .iter()
        .map(|(key, value)| format!("Environment={}\n", systemd_word(&format!("{key}={value}"))))
        .collect::<String>();
    // Taken whole, spaces and all, but specifiers (`%h`) are still expanded.
    let log = definition.log.to_string_lossy().replace('%', "%%");
    format!(
        "[Unit]\n\
Description={description}\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={command}\n\
Restart=on-failure\n\
RestartSec=5\n\
{environment}\
StandardOutput=append:{log}\n\
StandardError=append:{log}\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        description = definition.description.replace('%', "%%"),
    )
}

/// The program an installed unit runs: the first word of `ExecStart=`.
pub fn program_of(unit: &str) -> Option<PathBuf> {
    let command = unit
        .lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))?;
    first_word(command).map(PathBuf::from)
}

fn first_word(command: &str) -> Option<String> {
    let command = command.trim_start();
    let mut chars = command.chars().peekable();
    let mut word = String::new();
    if chars.peek() == Some(&'"') {
        chars.next();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(word),
                '\\' => match chars.next()? {
                    'n' => word.push('\n'),
                    other => word.push(other),
                },
                '%' | '$' if chars.peek() == Some(&c) => {
                    chars.next();
                    word.push(c);
                }
                c => word.push(c),
            }
        }
        return None;
    }
    for c in chars {
        if c.is_whitespace() {
            break;
        }
        word.push(c);
    }
    (!word.is_empty()).then_some(word)
}

/// `Key=value` lines of `systemctl show` or `loginctl show-user`.
fn property<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name == key).then_some(value.trim())
    })
}

/// Whether the user has lingering enabled, from `loginctl show-user`.
pub fn parse_linger(output: &str) -> Option<bool> {
    match property(output, "Linger")? {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// What `systemctl --user show` says: whether the unit is started, and its
/// main process.
pub fn parse_show(output: &str) -> (bool, Option<u32>) {
    let active = matches!(
        property(output, "ActiveState"),
        Some("active" | "activating" | "reloading")
    );
    let pid = property(output, "MainPID")
        .and_then(|pid| pid.parse::<u32>().ok())
        .filter(|pid| *pid != 0);
    (active, pid)
}

/// What to tell a user whose server stops when they log out.
pub fn linger_advice() -> &'static str {
    "The server stops when you log out. To keep it running, once: sudo loginctl enable-linger \"$(id -un)\""
}

pub struct Systemd {
    unit: String,
    path: PathBuf,
}

impl Systemd {
    pub fn new(name: &str) -> Result<Self> {
        let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => home_dir()?.join(".config"),
        };
        let unit = unit_name(name);
        Ok(Self {
            path: config.join("systemd/user").join(&unit),
            unit,
        })
    }

    fn systemctl(&self, args: &[&str]) -> Result<std::process::Output> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        run_tool("systemctl", &all)
    }

    fn checked(&self, args: &[&str]) -> Result<()> {
        let output = self.systemctl(args)?;
        anyhow::ensure!(
            output.status.success(),
            "systemctl --user {} failed: {}{}",
            args.join(" "),
            failure(&output),
            hint(&output)
        );
        Ok(())
    }

    fn show(&self) -> Result<(bool, Option<u32>)> {
        let output = self.systemctl(&[
            "show",
            &self.unit,
            "--property=ActiveState",
            "--property=MainPID",
        ])?;
        Ok(parse_show(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// A user manager that cannot be reached (a bare SSH login, a container) says
/// so cryptically.
fn hint(output: &std::process::Output) -> &'static str {
    let text = String::from_utf8_lossy(&output.stderr);
    if text.contains("Failed to connect to bus") || text.contains("XDG_RUNTIME_DIR") {
        " (there is no systemd user session here; log in on the machine, or run `loginctl enable-linger` and sign in again)"
    } else {
        ""
    }
}

impl Manager for Systemd {
    fn kind(&self) -> &'static str {
        "systemd"
    }

    fn state(&self) -> Result<State> {
        let unit = std::fs::read_to_string(&self.path).ok();
        let (active, pid) = if unit.is_some() {
            self.show().unwrap_or_default()
        } else {
            (false, None)
        };
        Ok(State {
            installed: unit.is_some(),
            active,
            pid,
            path: unit.is_some().then(|| self.path.clone()),
            program: unit.as_deref().and_then(program_of),
        })
    }

    fn install(&self, definition: &Definition) -> Result<Installed> {
        if let Some(parent) = definition.log.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        let unit = render(definition);
        let existing = std::fs::read_to_string(&self.path).ok();
        let changed = existing.as_deref() != Some(unit.as_str());
        if changed {
            write_private(&self.path, unit.as_bytes())?;
            self.checked(&["daemon-reload"])?;
        }
        self.checked(&["enable", &self.unit])?;
        if self.show()?.0 {
            return Ok(if changed {
                Installed::ChangedWhileActive
            } else {
                Installed::Unchanged
            });
        }
        self.checked(&["start", &self.unit])?;
        Ok(Installed::Started)
    }

    fn restart(&self) -> Result<()> {
        self.checked(&["daemon-reload"])?;
        self.checked(&["restart", &self.unit])
    }

    fn uninstall(&self) -> Result<()> {
        // Both fail for a unit that is not there, which is the aim.
        let _ = self.systemctl(&["disable", "--now", &self.unit]);
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error)
                    .with_context(|| format!("could not remove {}", self.path.display()));
            }
            _ => {}
        }
        let _ = self.systemctl(&["daemon-reload"]);
        let _ = self.systemctl(&["reset-failed", &self.unit]);
        Ok(())
    }

    fn problems(&self) -> Vec<Problem> {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_default();
        if user.is_empty() {
            return Vec::new();
        }
        let Ok(output) = run_tool("loginctl", &["show-user", &user, "--property=Linger"]) else {
            return Vec::new();
        };
        match parse_linger(&String::from_utf8_lossy(&output.stdout)) {
            Some(false) => vec![Problem {
                code: "linger-disabled",
                message: linger_advice().to_owned(),
            }],
            _ => Vec::new(),
        }
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
            log: PathBuf::from("/home/ada/.config/rustcode/logs/server.log"),
        }
    }

    #[test]
    fn the_unit_restarts_on_failure_and_starts_with_the_user_manager() {
        let unit = render(&definition("/usr/local/bin/rustcode"));
        assert!(unit.contains("ExecStart=/usr/local/bin/rustcode server run --service\n"));
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
        assert!(
            unit.contains("StandardOutput=append:/home/ada/.config/rustcode/logs/server.log\n")
        );
        assert!(unit.contains("StandardError=append:/home/ada/.config/rustcode/logs/server.log\n"));
        assert!(!unit.contains("Environment="));
        assert_eq!(unit_name("server"), "rustcode-server.service");
    }

    #[test]
    fn a_path_with_spaces_is_quoted_in_exec_start() {
        let mut definition = definition("/home/Ada Lovelace/.local/bin/rustcode");
        definition.env = vec![(
            "RUSTCODE_CONFIG_DIR".to_owned(),
            "/home/Ada Lovelace/rc%1".to_owned(),
        )];
        definition.log = PathBuf::from("/home/Ada Lovelace/logs/server.log");
        let unit = render(&definition);
        assert!(unit.contains(
            "ExecStart=\"/home/Ada Lovelace/.local/bin/rustcode\" server run --service\n"
        ));
        assert!(
            unit.contains("Environment=\"RUSTCODE_CONFIG_DIR=/home/Ada Lovelace/rc%%1\"\n"),
            "{unit}"
        );
        assert!(unit.contains("StandardOutput=append:/home/Ada Lovelace/logs/server.log\n"));
        assert_eq!(
            program_of(&unit),
            Some(PathBuf::from("/home/Ada Lovelace/.local/bin/rustcode"))
        );
    }

    #[test]
    fn the_installed_program_is_read_back() {
        assert_eq!(
            program_of(&render(&definition("/usr/bin/rustcode"))),
            Some(PathBuf::from("/usr/bin/rustcode"))
        );
        assert_eq!(program_of("[Service]\nType=simple\n"), None);
    }

    #[test]
    fn linger_comes_from_loginctl() {
        assert_eq!(parse_linger("Linger=yes\n"), Some(true));
        assert_eq!(parse_linger("Linger=no\n"), Some(false));
        assert_eq!(parse_linger(""), None);
        assert!(linger_advice().contains("loginctl enable-linger"));
    }

    #[test]
    fn show_output_gives_state_and_pid() {
        assert_eq!(
            parse_show("ActiveState=active\nMainPID=812\n"),
            (true, Some(812))
        );
        assert_eq!(
            parse_show("ActiveState=inactive\nMainPID=0\n"),
            (false, None)
        );
        assert_eq!(parse_show(""), (false, None));
    }
}
