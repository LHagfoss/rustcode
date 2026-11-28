//! `rc machines relay`: register this machine with a relay
//! ([ADR 0013](../../../docs/adr/0013-machine-relay-and-nat-traversal.md),
//! `docs/remote.md` › Relay).

use std::path::Path;

use anyhow::Result;
use rustcode::remote_relay::member::RelayLink;
use rustcode::remote_relay::protocol::JoinLink;

use crate::cli::RelayCommands;

pub fn run(config_dir: &Path, command: &RelayCommands) -> Result<()> {
    match command {
        RelayCommands::Join { link } => {
            let link = JoinLink::parse(link)?;
            RelayLink::join(config_dir, &link)?;
            println!(
                "This machine registers with the relay at {} within a minute, while remote \
                 access is on (`rc remote start`, or the app). Devices paired with it then \
                 reach it through the relay from any network; the relay sees which device \
                 reaches this machine and how many bytes, never what they say.",
                link.address()
            );
        }
        RelayCommands::Leave => {
            if RelayLink::leave(config_dir)? {
                println!("This machine no longer registers with a relay.");
            } else {
                println!("This machine was not registered with a relay.");
            }
        }
        RelayCommands::Status { json } => {
            let link = RelayLink::load(config_dir)?
                .map(|saved| saved.join_link())
                .transpose()?;
            if *json {
                let status = serde_json::json!({
                    "relay": link.as_ref().map(|link| serde_json::json!({
                        "address": link.address(),
                        "fingerprint": link.fingerprint,
                    })),
                });
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                match link {
                    Some(link) => println!(
                        "Registers with the relay at {} (certificate {}).",
                        link.address(),
                        link.fingerprint
                    ),
                    None => println!(
                        "Not registered with a relay. Run `rc remote relay` on a VPS and \
                         `rc machines relay join <link>` here."
                    ),
                }
            }
        }
    }
    Ok(())
}
