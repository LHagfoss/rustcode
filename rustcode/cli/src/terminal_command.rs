//! `rc terminal`: named background commands for a thread's agent.
//!
//! A harness's shell call ends when its command does, so a dev server started
//! in it dies with the call or leaves no handle to read. `rc terminal start`
//! runs the command in a process group of its own, writes everything it
//! prints to a log, and keeps its input open on a named pipe, so later calls
//! can read the output from an offset, wait for a line, type into it and stop
//! it. Each thread's terminals live under
//! `<config>/run/terminals/<thread>/<name>/` (`RUSTCODE_THREAD_ID`, or
//! `shell` outside a thread): `meta.json`, `output.log`, `input` and, once
//! the command ends, `exit` with its status.

#![cfg_attr(not(unix), allow(dead_code))]

use std::path::{Path, PathBuf};

#[derive(clap::Subcommand, Debug)]
pub enum TerminalCommand {
    /// Start a command in the background under a name; one argument is run
    /// as a shell line, several are quoted as words
    Start {
        name: String,
        /// The folder to run it in (default: the current one)
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List this thread's terminals
    List {
        /// Every thread's terminals
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print a terminal's output: its last `--tail` bytes, or everything from
    /// byte `--since` (the `next` offset an earlier call printed)
    Output {
        name: String,
        #[arg(long)]
        since: Option<u64>,
        #[arg(long, default_value_t = 16 * 1024)]
        tail: u64,
        /// Print `{"output", "next", "status"}`
        #[arg(long)]
        json: bool,
    },
    /// Wait until its output contains a text, searching from its first line.
    /// Exits 0 when it appears, 1 when the command ends first and 2 at the
    /// timeout
    Wait {
        name: String,
        #[arg(long)]
        contains: String,
        /// Seconds to wait
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Only match output printed after this call (after a `send`, say)
        #[arg(long)]
        new: bool,
    },
    /// Type a text into a terminal's input
    Send {
        name: String,
        #[arg(long)]
        text: String,
        /// Follow the text with a newline
        #[arg(long)]
        enter: bool,
    },
    /// Stop a terminal's command and everything it started, and forget it
    Stop { name: String },
}

/// Run one `rc terminal` command; the process exit code.
pub async fn run(command: &TerminalCommand) -> i32 {
    #[cfg(unix)]
    {
        match unix::run(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("error: {error}");
                1
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        eprintln!("error: rc terminal needs macOS or Linux");
        1
    }
}

/// A terminal name: letters, digits, `.`, `_` and `-`, up to 64 bytes, not
/// starting with a dot, so it is always one plain folder name.
fn valid_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "terminal name {name:?} must be 1-64 letters, digits, '.', '_' or '-'"
        ))
    }
}

/// The folder a thread id or `shell` maps to.
fn scope_name(thread: Option<&str>) -> String {
    let thread = thread.filter(|id| !id.is_empty()).unwrap_or("shell");
    thread
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn root() -> Result<PathBuf, String> {
    rustcode::config::get_config_dir()
        .map(|dir| dir.join("run").join("terminals"))
        .ok_or_else(|| "the RustCode config directory is unavailable".to_owned())
}

fn scope_dir() -> Result<PathBuf, String> {
    let thread = std::env::var(rustcode::cli_bridge::THREAD_ENV).ok();
    Ok(root()?.join(scope_name(thread.as_deref())))
}

/// One shell line from the words after `--`.
fn shell_line(words: &[String]) -> String {
    if let [line] = words {
        return line.clone();
    }
    words
        .iter()
        .map(|word| {
            if !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@%+,".contains(&b))
            {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The most a single read takes from a log, so a huge one never fills memory.
const MAX_READ: u64 = 1024 * 1024;

/// Where `needle` first appears in `haystack`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The whole line around byte `at` in `text`, for `wait`'s answer.
fn line_around(text: &[u8], at: usize) -> String {
    let start = text[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end = text[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(text.len(), |i| at + i);
    String::from_utf8_lossy(&text[start..end]).into_owned()
}

/// Up to `max` bytes of `log` from byte `from`, and the offset after them.
fn read_from(log: &Path, from: u64, max: u64) -> std::io::Result<(Vec<u8>, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(log)?;
    let len = file.metadata()?.len();
    let from = from.min(len);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take((len - from).min(max)).read_to_end(&mut bytes)?;
    let next = from + bytes.len() as u64;
    Ok((bytes, next))
}

fn log_len(log: &Path) -> u64 {
    std::fs::metadata(log).map_or(0, |meta| meta.len())
}

/// The `sh` line that runs a terminal's command and records how it ended.
/// The command runs in a shell of its own (`$1`), so `exit`, `exec` or a
/// syntax error in it still leaves this one to write the status; the status
/// goes to a temporary file first, so a reader never sees half of it.
const WRAPPER: &str =
    r#"/bin/sh -c "$1"; code=$?; printf '%s' "$code" > "$2.tmp" && mv -f "$2.tmp" "$2""#;

#[cfg(unix)]
mod unix {
    use super::*;
    use serde_json::{Value, json};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};

    const POLL: Duration = Duration::from_millis(250);

    struct Terminal {
        dir: PathBuf,
    }

    impl Terminal {
        fn named(name: &str) -> Result<Self, String> {
            valid_name(name)?;
            Ok(Self {
                dir: scope_dir()?.join(name),
            })
        }
        fn log(&self) -> PathBuf {
            self.dir.join("output.log")
        }
        fn input(&self) -> PathBuf {
            self.dir.join("input")
        }
        fn exit_file(&self) -> PathBuf {
            self.dir.join("exit")
        }
        fn meta(&self) -> Option<Value> {
            let text = std::fs::read_to_string(self.dir.join("meta.json")).ok()?;
            serde_json::from_str(&text).ok()
        }
        /// The process group `start` made: its leader's pid, never 0 or 1.
        fn group(&self) -> Option<i32> {
            self.meta()?["pid"]
                .as_i64()
                .and_then(|pid| i32::try_from(pid).ok())
                .filter(|&pid| pid > 1)
        }
        fn exists(&self) -> bool {
            self.dir.join("meta.json").is_file()
        }
        fn exit_code(&self) -> Option<String> {
            std::fs::read_to_string(self.exit_file())
                .ok()
                .map(|code| code.trim().to_owned())
                .filter(|code| !code.is_empty())
        }
        /// Something in its process group is still alive.
        fn group_alive(&self) -> bool {
            self.group().is_some_and(group_alive)
        }
        fn running(&self) -> bool {
            self.exit_code().is_none() && self.group_alive()
        }
        /// `running`, `exited <code>` (with `, children still running` when
        /// something it started outlived it) or `stopped`.
        fn status(&self) -> String {
            match self.exit_code() {
                Some(code) if self.group_alive() => {
                    format!("exited {code}, children still running")
                }
                Some(code) => format!("exited {code}"),
                None if self.group_alive() => "running".to_owned(),
                None => "stopped".to_owned(),
            }
        }
        fn missing(name: &str) -> String {
            format!("no terminal named {name}; `rc terminal list` lists this thread's")
        }
    }

    fn group_alive(group: i32) -> bool {
        if group <= 1 {
            return false;
        }
        // SAFETY: signal 0 to a process group only checks that it has a member.
        let result = unsafe { libc::kill(-group, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn signal_group(group: i32, signal: i32) {
        if group <= 1 {
            return;
        }
        // SAFETY: a negative pid signals the process group `start` created.
        unsafe {
            libc::kill(-group, signal);
        }
    }

    pub(super) async fn run(command: &TerminalCommand) -> Result<i32, String> {
        match command {
            TerminalCommand::Start { name, cwd, command } => start(name, cwd.as_deref(), command),
            TerminalCommand::List { all, json } => list(*all, *json),
            TerminalCommand::Output {
                name,
                since,
                tail,
                json,
            } => output(name, *since, *tail, *json),
            TerminalCommand::Wait {
                name,
                contains,
                timeout,
                new,
            } => wait(name, contains, *timeout, *new).await,
            TerminalCommand::Send { name, text, enter } => send(name, text, *enter),
            TerminalCommand::Stop { name } => stop(name).await,
        }
    }

    fn start(name: &str, cwd: Option<&Path>, words: &[String]) -> Result<i32, String> {
        let terminal = Terminal::named(name)?;
        if terminal.group_alive() {
            return Err(format!(
                "{name} is still running; `rc terminal stop {name}` first"
            ));
        }
        if terminal.dir.exists() {
            std::fs::remove_dir_all(&terminal.dir)
                .map_err(|error| format!("cannot clear the old {name}: {error}"))?;
        }
        std::fs::create_dir_all(&terminal.dir).map_err(|error| {
            format!(
                "cannot create {}: {error}{}",
                terminal.dir.display(),
                sandbox_hint(&error)
            )
        })?;
        let _ = std::fs::set_permissions(&terminal.dir, std::fs::Permissions::from_mode(0o700));
        let fifo = std::ffi::CString::new(terminal.input().as_os_str().as_encoded_bytes())
            .map_err(|_| "the terminal path holds a NUL byte".to_owned())?;
        // SAFETY: `fifo` is a valid NUL-terminated path.
        if unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) } != 0 {
            return Err(format!(
                "cannot create the input pipe: {}",
                std::io::Error::last_os_error()
            ));
        }
        let stdin = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(terminal.input())
            .map_err(|error| format!("cannot open the input pipe: {error}"))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(terminal.log())
            .map_err(|error| format!("cannot open the output log: {error}"))?;
        let stderr = log
            .try_clone()
            .map_err(|error| format!("cannot open the output log: {error}"))?;
        let line = shell_line(words);
        let cwd = match cwd {
            Some(cwd) => cwd.to_path_buf(),
            None => std::env::current_dir().map_err(|error| error.to_string())?,
        };
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(WRAPPER)
            .arg("rc-terminal")
            .arg(format!(" {line}"))
            .arg(terminal.exit_file())
            .env_remove(rustcode::cli_bridge::CREDENTIAL_ENV)
            .env_remove(rustcode::cli_bridge::SOCKET_ENV)
            .env_remove(rustcode::ORCHESTRATOR_TOKEN_ENV)
            .current_dir(&cwd)
            .stdin(stdin)
            .stdout(log)
            .stderr(stderr)
            .process_group(0)
            .spawn()
            .map_err(|error| format!("cannot start {line}: {error}"))?;
        let meta = json!({
            "name": name,
            "pid": child.id(),
            "command": line,
            "cwd": cwd,
            "started": chrono::Utc::now().to_rfc3339(),
        });
        std::fs::write(terminal.dir.join("meta.json"), meta.to_string())
            .map_err(|error| format!("cannot record {name}: {error}"))?;
        let settle = Instant::now() + Duration::from_millis(300);
        let mut ended = false;
        while Instant::now() < settle {
            if matches!(child.try_wait(), Ok(Some(_))) {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if ended {
            let (bytes, _) = read_from(&terminal.log(), 0, MAX_READ).unwrap_or_default();
            eprintln!(
                "{name} {} right away. Its output:\n{}",
                terminal.status(),
                String::from_utf8_lossy(&bytes)
            );
            return Ok(1);
        }
        println!(
            "Started {name} (pid {}): {line}\nRead it with `rc terminal output {name}`, wait for it with `rc terminal wait {name} --contains <text>`, stop it with `rc terminal stop {name}`.",
            child.id()
        );
        Ok(0)
    }

    /// What to add when a sandboxed harness refused a write.
    fn sandbox_hint(error: &std::io::Error) -> &'static str {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            " (the harness's sandbox may block RustCode's run folder; run with Full access, or ask the user)"
        } else {
            ""
        }
    }

    fn entries(all: bool) -> Result<Vec<(String, Terminal)>, String> {
        let scopes = if all {
            match std::fs::read_dir(root()?) {
                Ok(dirs) => dirs.flatten().map(|entry| entry.path()).collect(),
                Err(_) => Vec::new(),
            }
        } else {
            vec![scope_dir()?]
        };
        let mut found = Vec::new();
        for scope in scopes {
            let Ok(dirs) = std::fs::read_dir(&scope) else {
                continue;
            };
            let thread = scope
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            for entry in dirs.flatten() {
                let terminal = Terminal { dir: entry.path() };
                if terminal.exists() {
                    found.push((thread.clone(), terminal));
                }
            }
        }
        found.sort_by(|a, b| a.1.dir.cmp(&b.1.dir));
        Ok(found)
    }

    fn list(all: bool, as_json: bool) -> Result<i32, String> {
        let rows: Vec<Value> = entries(all)?
            .into_iter()
            .map(|(thread, terminal)| {
                let meta = terminal.meta().unwrap_or_default();
                json!({
                    "name": meta["name"],
                    "thread": thread,
                    "status": terminal.status(),
                    "pid": meta["pid"],
                    "command": meta["command"],
                    "started": meta["started"],
                })
            })
            .collect();
        if as_json {
            println!("{}", Value::Array(rows));
            return Ok(0);
        }
        if rows.is_empty() {
            println!("No terminals. Start one with `rc terminal start <name> -- <command>`.");
        }
        for row in rows {
            let thread = if all {
                format!("{}  ", row["thread"].as_str().unwrap_or_default())
            } else {
                String::new()
            };
            println!(
                "{thread}{:<16} {:<12} {}",
                row["name"].as_str().unwrap_or_default(),
                row["status"].as_str().unwrap_or_default(),
                row["command"].as_str().unwrap_or_default()
            );
        }
        Ok(0)
    }

    fn output(name: &str, since: Option<u64>, tail: u64, as_json: bool) -> Result<i32, String> {
        let terminal = Terminal::named(name)?;
        if !terminal.exists() {
            return Err(Terminal::missing(name));
        }
        let log = terminal.log();
        let from = since.unwrap_or_else(|| log_len(&log).saturating_sub(tail.min(MAX_READ)));
        let (bytes, next) = read_from(&log, from, MAX_READ).map_err(|error| error.to_string())?;
        let more = next < log_len(&log);
        let text = String::from_utf8_lossy(&bytes);
        let status = terminal.status();
        if as_json {
            println!(
                "{}",
                json!({ "output": text, "next": next, "more": more, "status": status })
            );
        } else {
            print!("{text}");
            if !text.is_empty() && !text.ends_with('\n') {
                println!();
            }
            let more = if more { "; more follows" } else { "" };
            eprintln!("[{name}: {status}{more}; next: rc terminal output {name} --since {next}]");
        }
        Ok(0)
    }

    async fn wait(name: &str, contains: &str, timeout: u64, new: bool) -> Result<i32, String> {
        let terminal = Terminal::named(name)?;
        if !terminal.exists() {
            return Err(Terminal::missing(name));
        }
        if contains.is_empty() {
            return Err("--contains needs a text to wait for".to_owned());
        }
        let needle = contains.as_bytes();
        let log = terminal.log();
        let mut offset = if new { log_len(&log) } else { 0 };
        let mut seen: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(timeout);
        loop {
            let running = terminal.running();
            let mut full = false;
            if let Ok((bytes, next)) = read_from(&log, offset, MAX_READ) {
                full = bytes.len() as u64 == MAX_READ;
                offset = next;
                seen.extend_from_slice(&bytes);
            }
            if let Some(at) = find_bytes(&seen, needle) {
                println!("{}", line_around(&seen, at));
                return Ok(0);
            }
            let keep = 64 * 1024 + needle.len();
            if seen.len() > keep {
                seen.drain(..seen.len() - keep);
            }
            if full {
                continue;
            }
            if !running {
                let tail = seen.len().saturating_sub(2048);
                eprintln!(
                    "{name} {} before printing {contains:?}. Its last output:\n{}",
                    terminal.status(),
                    String::from_utf8_lossy(&seen[tail..])
                );
                return Ok(1);
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "{name} did not print {contains:?} within {timeout} s; it is still running. Read it with `rc terminal output {name}`."
                );
                return Ok(2);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    fn send(name: &str, text: &str, enter: bool) -> Result<i32, String> {
        use std::io::Write;
        let terminal = Terminal::named(name)?;
        if !terminal.running() {
            return Err(format!("{name} is not running"));
        }
        let mut pipe = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(terminal.input())
            .map_err(|error| format!("cannot open {name}'s input: {error}"))?;
        let mut bytes = text.as_bytes().to_vec();
        if enter {
            bytes.push(b'\n');
        }
        pipe.write_all(&bytes).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                format!("{name}'s input is full: it is not reading what was sent")
            } else {
                format!("cannot write to {name}: {error}")
            }
        })?;
        println!("Sent {} bytes to {name}.", bytes.len());
        Ok(0)
    }

    async fn stop(name: &str) -> Result<i32, String> {
        let terminal = Terminal::named(name)?;
        if !terminal.exists() {
            return Err(Terminal::missing(name));
        }
        if let Some(group) = terminal.group().filter(|&group| group_alive(group)) {
            signal_group(group, libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(5);
            while group_alive(group) && Instant::now() < deadline {
                tokio::time::sleep(POLL).await;
            }
            if group_alive(group) {
                signal_group(group, libc::SIGKILL);
            }
        }
        std::fs::remove_dir_all(&terminal.dir)
            .map_err(|error| format!("cannot forget {name}: {error}"))?;
        println!("Stopped {name}.");
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_one_plain_folder() {
        assert!(valid_name("dev").is_ok());
        assert!(valid_name("web-1.server_a").is_ok());
        for bad in ["", ".hidden", "a/b", "..", "with space", &"x".repeat(65)] {
            assert!(valid_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_thread_id_becomes_one_folder_name() {
        assert_eq!(scope_name(None), "shell");
        assert_eq!(scope_name(Some("")), "shell");
        assert_eq!(scope_name(Some("abc/12:34")), "abc_12_34");
    }

    #[test]
    fn words_are_quoted_and_one_line_runs_as_is() {
        let words = |list: &[&str]| list.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            shell_line(&words(&["bun run dev && echo ok"])),
            "bun run dev && echo ok"
        );
        assert_eq!(shell_line(&words(&["bun", "run", "dev"])), "bun run dev");
        assert_eq!(
            shell_line(&words(&["echo", "it's", ""])),
            r"echo 'it'\''s' ''"
        );
    }

    #[test]
    fn the_matching_line_is_reported_whole() {
        let text = "starting\n  ➜  Local: http://localhost:5173\nready".as_bytes();
        let at = find_bytes(text, "Local:".as_bytes()).unwrap();
        assert_eq!(line_around(text, at), "  ➜  Local: http://localhost:5173");
        assert_eq!(find_bytes(text, b"missing"), None);
        assert_eq!(find_bytes(b"ab", b"abc"), None);
    }

    #[test]
    fn output_is_read_from_an_offset() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("output.log");
        std::fs::write(&log, "hello world").unwrap();
        let (bytes, next) = read_from(&log, 6, MAX_READ).unwrap();
        assert_eq!(bytes, b"world");
        assert_eq!(next, 11);
        let (bytes, next) = read_from(&log, 0, 5).unwrap();
        assert_eq!((bytes.as_slice(), next), (&b"hello"[..], 5));
        let (bytes, _) = read_from(&log, 99, MAX_READ).unwrap();
        assert!(bytes.is_empty());
    }
}
