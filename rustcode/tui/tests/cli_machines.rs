//! Binary-level coverage for `rustcode machines`.
//!
//! The commands read and write only the configuration directory they are
//! given, so every case gets a private `RUSTCODE_CONFIG_DIR` and `HOME` under
//! the system temp directory, removed on drop. Nothing here reaches the
//! developer's real `~/.config/rustcode`, the OS keychain, or the network:
//! `list`, `remove` and `discover` with no announcement are local, and `add`
//! is not run against a live machine (the crate's integration test pairs with
//! a real gateway instead).

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const BINARY: &str = env!("CARGO_BIN_EXE_rustcode");
/// Upper bound for one run: none of these wait on a model or a terminal.
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustcode-machines-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("create sandbox config dir");
        std::fs::create_dir_all(root.join("home")).expect("create sandbox home");
        Self { root }
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_stdin(args, None)
    }

    fn run_with_stdin(&self, args: &[&str], stdin: Option<&str>) -> Output {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            .current_dir(&self.root)
            .env("RUSTCODE_CONFIG_DIR", self.config_dir())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("home"))
            // A private token store: never the developer's keychain.
            .env("RUSTCODE_MACHINE_TOKEN_STORE", "file")
            .env("RUSTCODE_SHELL_PROBE", "1")
            .env("NO_COLOR", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match stdin {
            Some(text) => {
                use std::io::Write;
                command.stdin(Stdio::piped());
                let mut child = command.spawn().expect("spawn rustcode");
                child
                    .stdin
                    .as_mut()
                    .expect("piped stdin")
                    .write_all(text.as_bytes())
                    .expect("write the code");
                let started = Instant::now();
                let output = child.wait_with_output().expect("run rustcode");
                assert!(
                    started.elapsed() < RUN_TIMEOUT,
                    "rustcode {args:?} took longer than {RUN_TIMEOUT:?}"
                );
                output
            }
            None => {
                command.stdin(Stdio::null());
                let started = Instant::now();
                let output = command.output().expect("run rustcode");
                assert!(
                    started.elapsed() < RUN_TIMEOUT,
                    "rustcode {args:?} took longer than {RUN_TIMEOUT:?}"
                );
                output
            }
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A saved machine written by hand, in the shape `machines add` writes.
fn write_machine(config: &std::path::Path, name: &str, host_id: &str, address: &str) {
    let path = config.join("machines.json");
    let machine = serde_json::json!({
        "version": 1,
        "machines": [{
            "host_id": host_id,
            "name": name,
            "platform": "macos",
            "routes": [{"kind": "lan", "address": address}],
            "device_id": "0011223344556677",
            "added_at": "2026-01-01T00:00:00Z",
            "main": true,
        }],
    });
    std::fs::write(path, serde_json::to_vec_pretty(&machine).unwrap()).unwrap();
}

#[test]
fn machines_list_says_how_to_add_the_first_one() {
    let sandbox = Sandbox::new("empty");
    let output = sandbox.run(&["machines", "list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("No machines saved"), "{text}");
    assert!(text.contains("rustcode machines add"), "{text}");
    // JSON is a list either way, so a script can read it.
    let output = sandbox.run(&["machines", "list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "[]");
}

#[test]
fn machines_list_shows_columns_and_reports_an_unreachable_machine() {
    let sandbox = Sandbox::new("offline");
    // Nothing listens on port 1 of the loopback address: the check must say
    // offline instead of hanging or failing the command.
    write_machine(
        &sandbox.config_dir(),
        "Studio",
        "gw-8e1f4b7a00112233",
        "127.0.0.1:1",
    );
    let output = sandbox.run(&["machines", "list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("NAME") && text.contains("STATE"), "{text}");
    // The file says `main: true`, as one written before every machine was a peer does.
    assert!(text.contains("Studio"), "{text}");
    assert!(!text.contains("(main)"), "{text}");
    assert!(text.contains("127.0.0.1:1"), "{text}");
    assert!(text.contains("not paired"), "{text}");
    // A machine with no token is not dialled at all.
    let output = sandbox.run(&["machines", "list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let machines: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert_eq!(machines[0]["name"], "Studio");
    assert_eq!(machines[0]["host_id"], "gw-8e1f4b7a00112233");
    assert_eq!(machines[0]["platform"], "macos");
    assert_eq!(machines[0]["state"], "not paired");
    assert!(machines[0].get("main").is_none());
    assert_eq!(machines[0]["routes"][0]["kind"], "lan");
    assert!(machines[0]["threads"].is_null());
}

#[test]
fn machines_remove_forgets_a_machine_by_name_or_prefix() {
    let sandbox = Sandbox::new("remove");
    write_machine(
        &sandbox.config_dir(),
        "Studio",
        "gw-8e1f4b7a00112233",
        "127.0.0.1:1",
    );
    let output = sandbox.run(&["machines", "remove", "gw-8e1f"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Removed Studio"),
        "{}",
        stdout(&output)
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sandbox.config_dir().join("machines.json")).unwrap())
            .unwrap();
    assert_eq!(stored["machines"], serde_json::json!([]));
    // Removing it again fails, and says what is saved.
    let output = sandbox.run(&["machines", "remove", "Studio"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("rustcode machines") && stderr(&output).contains("Studio"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn machines_add_needs_an_address_or_discovery() {
    let sandbox = Sandbox::new("add");
    let output = sandbox.run(&["machines", "add"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("address") && stderr(&output).contains("--discover"),
        "{}",
        stderr(&output)
    );
    // A refused scheme fails before anything is dialled or saved.
    let output = sandbox.run(&["machines", "add", "wss://studio.local:17879"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no TLS listener"),
        "{}",
        stderr(&output)
    );
    assert!(!sandbox.config_dir().join("machines.json").exists());
}

/// A public address is refused before anything is dialled: the pairing code
/// and the token would cross it in the clear (#2651).
#[test]
fn machines_add_refuses_a_public_address() {
    let sandbox = Sandbox::new("public");
    let output = sandbox.run(&["machines", "add", "203.0.113.9:17879"]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("#2651"), "{text}");
    assert!(text.contains("203.0.113.9"), "{text}");
    assert!(text.contains("public address"), "{text}");
    assert!(!sandbox.config_dir().join("machines.json").exists());
    // A saved machine whose only route is public says the same instead of
    // handing over its token.
    write_machine(
        &sandbox.config_dir(),
        "Studio",
        "gw-8e1f4b7a00112233",
        "203.0.113.9:17879",
    );
    let output = sandbox.run(&["machines", "list"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("not paired"),
        "{}",
        stdout(&output)
    );
}

/// A `--code` that is not the eight digits the machine shows is refused
/// before the request is even sent, so it costs no pairing attempt.
#[test]
fn machines_add_refuses_a_code_that_is_not_eight_digits() {
    let sandbox = Sandbox::new("badcode");
    for code in ["1234", "482190341", "4821-90x4"] {
        let output = sandbox.run(&["machines", "add", "127.0.0.1:1", "--code", code]);
        assert!(!output.status.success(), "{code} was accepted");
        let text = stderr(&output);
        assert!(text.contains("exactly 8 digits"), "{code}: {text}");
        assert!(!text.contains("could not ask"), "{code}: {text}");
    }
    // The separators a person might type are ignored.
    let output = sandbox.run(&["machines", "add", "127.0.0.1:1", "--code", "4821-9034"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("could not ask"),
        "a well-shaped code reaches the machine: {}",
        stderr(&output)
    );
}

/// A code typed on standard input, as a person would, is read when `--code`
/// is absent. Nothing is listening, so the command stops at the request.
#[test]
fn machines_add_reads_a_code_from_standard_input() {
    let sandbox = Sandbox::new("stdin");
    let output = sandbox.run_with_stdin(&["machines", "add", "127.0.0.1:1"], Some("4821-9034\n"));
    assert!(!output.status.success());
    // Port 1 refuses the connection: the failure names the address, and no
    // machine is saved from a request that never reached anyone.
    assert!(
        stderr(&output).contains("127.0.0.1:1"),
        "{}",
        stderr(&output)
    );
    assert!(!sandbox.config_dir().join("machines.json").exists());
}

#[test]
fn machines_discover_with_nothing_on_the_network_says_so() {
    let sandbox = Sandbox::new("discover");
    let output = sandbox.run(&["machines", "discover", "--secs", "1"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("No machines found") || text.contains("NAME"),
        "{text}"
    );
    let output = sandbox.run(&["machines", "discover", "--secs", "1", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let found: serde_json::Value = serde_json::from_str(stdout(&output).trim()).unwrap();
    assert!(found.is_array(), "{}", stdout(&output));
}
