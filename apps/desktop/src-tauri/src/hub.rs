//! Settings › Machines › *Hub*: this machine's role in the fleet's hub
//! (`docs/remote.md` › The hub).
//!
//! The work is the CLI's own: `rc machines hub status --json`, `on`,
//! `off`, `join <machine> --access <level> --ask <bool>` and `leave`, in the
//! same binary the app starts its engine from (as `service.rs` does for the
//! server). The CLI writes `<config>/hub.json` and checks that the hub is a
//! paired machine; the remote gateway picks the change up within a minute.

use crate::service::{run, STATUS_TIMEOUT};
use serde_json::Value;

/// `machines hub status --json`: `{ hub, member, members }`, with `members`
/// null unless this machine is the hub and its gateway answers.
#[tauri::command]
pub async fn hub_status() -> Result<Value, String> {
    let output = run(&["machines", "hub", "status", "--json"], STATUS_TIMEOUT).await?;
    if !output.status.success() {
        return Err(failure(&output, "The hub role could not be read."));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("The hub status did not parse: {error}"))
}

/// Change the role (`on`, `off`, `join`, `leave`) and answer the status after.
#[tauri::command]
pub async fn hub_set(
    action: String,
    machine: Option<String>,
    access: Option<String>,
    ask: Option<bool>,
) -> Result<Value, String> {
    let ask = ask.unwrap_or(true).to_string();
    let args = arguments(&action, machine.as_deref(), access.as_deref(), &ask)?;
    let output = run(&args, STATUS_TIMEOUT).await?;
    if !output.status.success() {
        return Err(failure(&output, "The hub role could not be changed."));
    }
    hub_status().await
}

fn arguments<'a>(
    action: &'a str,
    machine: Option<&'a str>,
    access: Option<&'a str>,
    ask: &'a str,
) -> Result<Vec<&'a str>, String> {
    let mut args = vec!["machines", "hub"];
    match action {
        "on" | "off" | "leave" => args.push(action),
        "join" => {
            let machine = machine
                .filter(|machine| !machine.trim().is_empty())
                .ok_or("Choose the machine that is the hub.")?;
            args.extend([
                "join",
                machine,
                "--access",
                access.unwrap_or("read-only"),
                "--ask",
                ask,
            ]);
        }
        other => return Err(format!("{other} is not a hub action.")),
    }
    Ok(args)
}

fn failure(output: &std::process::Output, fallback: &str) -> String {
    let why = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let said = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let message = if why.is_empty() { said } else { why };
    if message.is_empty() {
        fallback.to_owned()
    } else {
        // anyhow prints "Error: …" from main.
        message.trim_start_matches("Error: ").to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::arguments;

    #[test]
    fn each_action_is_the_cli_command() {
        assert_eq!(
            arguments("on", None, None, "true").unwrap(),
            ["machines", "hub", "on"]
        );
        assert_eq!(
            arguments("leave", Some("x"), None, "true").unwrap(),
            ["machines", "hub", "leave"]
        );
        assert_eq!(
            arguments("join", Some("host-1"), Some("delegate"), "false").unwrap(),
            ["machines", "hub", "join", "host-1", "--access", "delegate", "--ask", "false"]
        );
        assert_eq!(
            arguments("join", None, None, "true").unwrap_err(),
            "Choose the machine that is the hub."
        );
        assert!(arguments("status; rm", None, None, "true").is_err());
    }
}
