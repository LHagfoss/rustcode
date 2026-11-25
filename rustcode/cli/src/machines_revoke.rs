//! `rc machines revoke-everywhere`: end a lost computer's pairings on this
//! computer and on every saved machine that answers, then forget it here
//! (`docs/remote-security.md` › Credential storage and revocation).

use anyhow::{Result, anyhow, bail};
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
use rustcode_client::{ClientKind, Connector, Machine, MachineRegistry, TokenStore};
use serde_json::{Value, json};
use std::path::Path;

const REVOKE_MACHINE: &str = "_rustcode/remote/revoke_machine";

pub async fn revoke_everywhere(config_dir: &Path, selector: &str) -> Result<()> {
    let registry = MachineRegistry::load(config_dir)?;
    let target = registry.select(selector)?.clone();
    let here = RemoteLifecycle::new(config_dir)
        .revoke_host(&target.host_id)
        .await
        .map(|(revoked, errors)| json!({"revoked": revoked.len(), "errors": errors}));
    println!("{}", describe("this computer", &here));
    let tokens = TokenStore::open(config_dir)?;
    let connector = Connector::new(config_dir, ClientKind::Desktop)?;
    for machine in registry
        .machines()
        .iter()
        .filter(|machine| machine.host_id != target.host_id)
    {
        let answer = tokio::time::timeout(
            rustcode_client::UNPAIR_TIMEOUT,
            ask(&connector, machine, &tokens, &target.host_id),
        )
        .await
        .unwrap_or_else(|_| Err(anyhow!("it did not answer in time")));
        println!("{}", describe(machine_name(machine), &answer));
    }
    crate::machines::remove(config_dir, &target.host_id, true).await
}

fn machine_name(machine: &Machine) -> &str {
    machine.display_name.as_deref().unwrap_or(&machine.name)
}

async fn ask(
    connector: &Connector,
    machine: &Machine,
    tokens: &TokenStore,
    host_id: &str,
) -> Result<Value> {
    let connected = connector.connect_raced(machine, tokens).await?;
    let mut channel = connected.session.acp().await?;
    channel
        .request(1, "initialize", json!({"protocolVersion": 1}))
        .await?;
    let answer = channel
        .request(2, REVOKE_MACHINE, json!({"hostId": host_id}))
        .await?;
    if let Some(error) = answer.get("error") {
        bail!(
            "{}",
            error["message"]
                .as_str()
                .unwrap_or("it refused; update RustCode there")
        );
    }
    Ok(answer["result"].clone())
}

/// One line per machine: revoked, not paired there, or why it could not be.
fn describe(name: &str, answer: &Result<Value>) -> String {
    match answer {
        Ok(answer) if answer["error"].is_string() => {
            format!(
                "{name}: not revoked: {}",
                answer["error"].as_str().unwrap_or_default()
            )
        }
        Ok(answer) if answer["revoked"].as_u64().unwrap_or(0) > 0 => format!("{name}: revoked"),
        Ok(_) => format!("{name}: was not paired there"),
        Err(error) => format!("{name}: not revoked ({error:#}); revoke it there"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_machine_gets_a_line_that_says_what_happened() {
        assert_eq!(
            describe("Studio", &Ok(json!({"revoked": 1, "errors": []}))),
            "Studio: revoked"
        );
        assert_eq!(
            describe("Mini", &Ok(json!({"revoked": 0}))),
            "Mini: was not paired there"
        );
        assert!(describe("Old", &Ok(json!({"error": "nope"}))).contains("not revoked: nope"));
        assert!(describe("Away", &Err(anyhow!("offline"))).contains("revoke it there"));
    }
}
