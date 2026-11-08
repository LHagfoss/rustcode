//! Windows: a Scheduled Task that runs "At log on" of this user, limited
//! (no elevation), restarting on failure. Where the policy forbids tasks, an
//! entry in the user's `Run` key starts the server at logon instead (no
//! restart on failure).
//!
//! A task's output cannot be redirected, so the server's own log file
//! (`logs/server.log`) is the only log here.

use super::{
    Definition, Installed, Manager, State, failure, run_tool, windows_argument,
    windows_system_tool, xml_escape, xml_unescape,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// The task's path in Task Scheduler: `RustCode\<Name>`.
pub fn task_name(name: &str) -> String {
    let mut chars = name.chars();
    let name = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    format!(r"RustCode\{name}")
}

/// The `Run` value's name.
pub fn run_value_name(name: &str) -> String {
    format!("RustCode-{name}")
}

/// The command line a `Run` value holds: the program quoted when it needs it,
/// then its arguments.
pub fn command_line(definition: &Definition) -> String {
    std::iter::once(definition.program.to_string_lossy().into_owned())
        .chain(definition.args.iter().cloned())
        .map(|word| windows_argument(&word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The task definition for `definition`, run as `user` (`DOMAIN\name`).
/// `Command` is the program alone and `Arguments` the rest, each as the
/// command line reads them.
pub fn render(definition: &Definition, user: &str) -> String {
    let arguments = definition
        .args
        .iter()
        .map(|argument| windows_argument(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let user = xml_escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        description = xml_escape(&definition.description),
        command = xml_escape(&definition.program.to_string_lossy()),
        arguments = xml_escape(&arguments),
    )
}

/// The bytes `schtasks /XML` reads: UTF-16 little-endian with a byte order
/// mark, as the document's declaration says.
pub fn encode(document: &str) -> Vec<u8> {
    std::iter::once(0xFEFF_u16)
        .chain(document.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The program of an exported task definition.
pub fn program_of_task(document: &str) -> Option<PathBuf> {
    let (_, rest) = document.split_once("<Command>")?;
    let (command, _) = rest.split_once("</Command>")?;
    Some(PathBuf::from(xml_unescape(command.trim())))
}

/// The program of a `Run` value's command line.
pub fn program_of_command_line(line: &str) -> Option<PathBuf> {
    let line = line.trim();
    let program = match line.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?.0,
        None => line.split_whitespace().next()?,
    };
    Some(PathBuf::from(program))
}

/// The data of a value in `reg query` output (`    Name    REG_SZ    data`).
pub fn parse_reg_value(output: &str, value: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix(value)?.trim_start();
        let data = rest.strip_prefix("REG_SZ")?;
        Some(data.trim().to_owned())
    })
}

pub struct WindowsTask {
    task: String,
    value: String,
}

impl WindowsTask {
    pub fn new(name: &str) -> Self {
        Self {
            task: task_name(name),
            value: run_value_name(name),
        }
    }

    fn user() -> Result<String> {
        let name = std::env::var("USERNAME").context("USERNAME is not set")?;
        Ok(match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => format!(r"{domain}\{name}"),
            _ => name,
        })
    }

    fn task_xml(&self) -> Option<String> {
        let output = run_tool(
            &windows_system_tool("schtasks.exe"),
            &["/Query", "/TN", &self.task, "/XML"],
        )
        .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn run_value(&self) -> Option<String> {
        let output = run_tool(
            &windows_system_tool("reg.exe"),
            &["query", RUN_KEY, "/v", &self.value],
        )
        .ok()?;
        output
            .status
            .success()
            .then(|| parse_reg_value(&String::from_utf8_lossy(&output.stdout), &self.value))?
    }

    fn remove_run_value(&self) {
        let _ = run_tool(
            &windows_system_tool("reg.exe"),
            &["delete", RUN_KEY, "/v", &self.value, "/f"],
        );
    }

    fn start(&self, program: &std::path::Path) -> Result<()> {
        if self.task_xml().is_some() {
            let output = run_tool(
                &windows_system_tool("schtasks.exe"),
                &["/Run", "/TN", &self.task],
            )?;
            anyhow::ensure!(
                output.status.success(),
                "could not start the task {}: {}",
                self.task,
                failure(&output)
            );
            Ok(())
        } else {
            crate::server::start_in_background(program).context("could not start the server")
        }
    }
}

impl Manager for WindowsTask {
    fn kind(&self) -> &'static str {
        if self.task_xml().is_some() {
            "scheduled-task"
        } else {
            "run-key"
        }
    }

    fn state(&self) -> Result<State> {
        if let Some(document) = self.task_xml() {
            return Ok(State {
                installed: true,
                program: program_of_task(&document),
                ..State::default()
            });
        }
        let value = self.run_value();
        Ok(State {
            installed: value.is_some(),
            program: value.as_deref().and_then(program_of_command_line),
            ..State::default()
        })
    }

    fn install(&self, definition: &Definition) -> Result<Installed> {
        anyhow::ensure!(
            definition.env.is_empty(),
            "a background server for a custom RUSTCODE_CONFIG_DIR is not supported on Windows: \
             a scheduled task cannot set it"
        );
        let document = render(definition, &Self::user()?);
        let file = tempfile::Builder::new()
            .prefix("rustcode-task")
            .suffix(".xml")
            .tempfile()?;
        std::fs::write(file.path(), encode(&document))?;
        let created = run_tool(
            &windows_system_tool("schtasks.exe"),
            &[
                "/Create",
                "/TN",
                &self.task,
                "/XML",
                &file.path().to_string_lossy(),
                "/F",
            ],
        )?;
        if created.status.success() {
            self.remove_run_value();
        } else {
            let registered = run_tool(
                &windows_system_tool("reg.exe"),
                &[
                    "add",
                    RUN_KEY,
                    "/v",
                    &self.value,
                    "/t",
                    "REG_SZ",
                    "/d",
                    &command_line(definition),
                    "/f",
                ],
            )?;
            anyhow::ensure!(
                registered.status.success(),
                "could not create the scheduled task ({}) or the Run entry ({})",
                failure(&created),
                failure(&registered)
            );
        }
        // A server already running makes this start a no-op.
        self.start(&definition.program)?;
        Ok(Installed::Started)
    }

    fn restart(&self) -> Result<()> {
        let state = self.state()?;
        let program = state
            .program
            .context("the server is not installed; run `rustcode server install`")?;
        let _ = run_tool(
            &windows_system_tool("schtasks.exe"),
            &["/End", "/TN", &self.task],
        );
        self.start(&program)
    }

    fn uninstall(&self) -> Result<()> {
        // Each fails for what is not there, which is the aim.
        let _ = run_tool(
            &windows_system_tool("schtasks.exe"),
            &["/End", "/TN", &self.task],
        );
        let _ = run_tool(
            &windows_system_tool("schtasks.exe"),
            &["/Delete", "/TN", &self.task, "/F"],
        );
        self.remove_run_value();
        Ok(())
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
            log: PathBuf::from(r"C:\Users\Ada\AppData\Roaming\rustcode\logs\server.log"),
        }
    }

    #[test]
    fn the_task_runs_at_logon_limited_hidden_and_restarts_on_failure() {
        let task = render(&definition(r"C:\Tools\rustcode.exe"), r"HOME\ada");
        assert!(task.contains("<LogonTrigger>"));
        assert!(task.contains("<UserId>HOME\\ada</UserId>"));
        assert!(task.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(task.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(task.contains("<Hidden>true</Hidden>"));
        assert!(task.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(task.contains("<RestartOnFailure>"));
        assert!(task.contains("<Command>C:\\Tools\\rustcode.exe</Command>"));
        assert!(task.contains("<Arguments>server run --service</Arguments>"));
        assert_eq!(task_name("server"), r"RustCode\Server");
    }

    #[test]
    fn a_program_path_with_spaces_stays_whole() {
        let definition = definition(r"C:\Program Files\Rust & Code\rustcode.exe");
        let task = render(&definition, r"HOME\ada");
        assert!(
            task.contains("<Command>C:\\Program Files\\Rust &amp; Code\\rustcode.exe</Command>")
        );
        assert_eq!(
            program_of_task(&task),
            Some(PathBuf::from(r"C:\Program Files\Rust & Code\rustcode.exe"))
        );
        // The Run value is one command line, so the program is quoted there.
        assert_eq!(
            command_line(&definition),
            r#""C:\Program Files\Rust & Code\rustcode.exe" server run --service"#
        );
        assert_eq!(
            program_of_command_line(&command_line(&definition)),
            Some(PathBuf::from(r"C:\Program Files\Rust & Code\rustcode.exe"))
        );
    }

    #[test]
    fn the_document_is_utf16_with_a_byte_order_mark() {
        let bytes = encode("<a>é</a>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "<a>é</a>");
    }

    #[test]
    fn a_run_value_is_read_from_reg_query() {
        let output = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    RustCode-server    REG_SZ    \"C:\\Program Files\\rc\\rustcode.exe\" server run --service\r\n\r\n";
        assert_eq!(
            parse_reg_value(output, "RustCode-server").as_deref(),
            Some("\"C:\\Program Files\\rc\\rustcode.exe\" server run --service")
        );
        assert_eq!(parse_reg_value(output, "Other"), None);
    }
}
