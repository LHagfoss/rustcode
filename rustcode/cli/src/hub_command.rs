//! `rustcode machines hub`: the fleet's hub role (#3957, `docs/remote.md` ›
//! The hub).

use std::path::Path;

use anyhow::{Context, Result, bail};
use rustcode::remote_gateway::hub_link::{HubConfig, HubMember, Membership, Scope};
use rustcode::remote_gateway::lifecycle::RemoteLifecycle;
use rustcode_client::{MachineRegistry, TokenStore};

use crate::cli::HubCommands;

pub async fn run(config_dir: &Path, command: &HubCommands) -> Result<()> {
    let mut config = HubConfig::load(config_dir);
    match command {
        HubCommands::On => {
            config.hub = true;
            config.save(config_dir)?;
            println!(
                "This machine is the fleet's hub. Machines join it with \
                 `rustcode machines hub join <this machine>` and keep a link to its remote \
                 gateway, so keep remote access on here (`rustcode remote start`, or \
                 `rustcode server` with remote access) and give them a route they reach from \
                 anywhere, such as its public wss:// address."
            );
        }
        HubCommands::Off => {
            let was = config.hub;
            config.hub = false;
            config.save(config_dir)?;
            println!(
                "{}",
                if was {
                    "This machine is no longer the hub. Linked machines are let go within a minute."
                } else {
                    "This machine was not the hub."
                }
            );
        }
        HubCommands::Join {
            machine,
            access,
            ask,
        } => {
            let access = Scope::parse(access).with_context(|| {
                format!("{access} is not an access level; use read-only, delegate, manage or admin")
            })?;
            let registry = MachineRegistry::load(config_dir)?;
            let hub = registry.select(machine)?;
            let tokens = TokenStore::open(config_dir)?;
            if tokens.get(&hub.host_id)?.is_none() {
                bail!(
                    "{} is saved but not paired; pair with it first (`rustcode machines add`)",
                    hub.name
                );
            }
            config.member = Some(Membership {
                machine: hub.host_id.clone(),
                name: hub.name.clone(),
                access,
                ask: *ask,
            });
            config.save(config_dir)?;
            println!(
                "Joined the hub {} at {} access{}. This machine's remote gateway keeps the \
                 link, so keep remote access on here (`rustcode remote start`). Outside agents \
                 on {} can now pass `machine` to act here, at most at that access.",
                hub.name,
                access.label(),
                if *ask {
                    ", asking in the app here before each change"
                } else {
                    ", without asking"
                },
                hub.name
            );
        }
        HubCommands::Leave => match config.member.take() {
            Some(membership) => {
                config.save(config_dir)?;
                println!(
                    "Left the hub {}. The link closes within a minute.",
                    membership.name
                );
            }
            None => println!("This machine is in no hub."),
        },
        HubCommands::Status { json } => {
            let members = if config.hub {
                RemoteLifecycle::new(config_dir)
                    .hub_members()
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "hub": config.hub,
                        "member": config.member,
                        "members": members,
                    }))?
                );
                return Ok(());
            }
            print!("{}", format_status(&config, members.as_deref()));
        }
    }
    Ok(())
}

fn format_status(config: &HubConfig, members: Option<&[HubMember]>) -> String {
    let mut out = String::new();
    match &config.member {
        Some(member) => out.push_str(&format!(
            "Member of the hub {} at {} access, {}.\n",
            member.name,
            member.access.label(),
            if member.ask {
                "asking before each change"
            } else {
                "without asking"
            }
        )),
        None => out.push_str("Member of no hub.\n"),
    }
    if !config.hub {
        out.push_str("Not the hub.\n");
        return out;
    }
    out.push_str("This machine is the fleet's hub.\n");
    match members {
        None => out.push_str(
            "Its remote gateway is not running, so no machine can link to it. Start it with `rustcode remote start`.\n",
        ),
        Some([]) => out.push_str(
            "No machine has joined yet. On each one: `rustcode machines hub join <this machine>`.\n",
        ),
        Some(members) => {
            for member in members {
                let state = if member.online {
                    "linked".to_owned()
                } else {
                    format!(
                        "offline, last seen {}",
                        member.last_seen_at.format("%Y-%m-%d %H:%M UTC")
                    )
                };
                let queued = if member.queued > 0 {
                    format!(", {} waiting", member.queued)
                } else {
                    String::new()
                };
                out.push_str(&format!(
                    "  {}  {state}  {}{}{queued}\n",
                    member.name,
                    member.access.label(),
                    if member.ask { ", asks" } else { "" }
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_says_the_role_and_each_members_state() {
        let config = HubConfig {
            hub: true,
            member: None,
        };
        assert!(format_status(&config, None).contains("gateway is not running"));
        let seen = chrono::DateTime::parse_from_rfc3339("2026-11-18T21:04:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let members = [
            HubMember {
                host_id: "a".into(),
                name: "laptop".into(),
                access: Scope::Delegate,
                ask: true,
                online: true,
                last_seen_at: seen,
                queued: 0,
            },
            HubMember {
                host_id: "b".into(),
                name: "desk".into(),
                access: Scope::ReadOnly,
                ask: false,
                online: false,
                last_seen_at: seen,
                queued: 2,
            },
        ];
        let text = format_status(&config, Some(&members));
        assert!(text.contains("laptop  linked  Delegate, asks"), "{text}");
        assert!(
            text.contains("desk  offline, last seen 2026-11-18 21:04 UTC  Read-only, 2 waiting"),
            "{text}"
        );
        let member = HubConfig {
            hub: false,
            member: Some(Membership {
                machine: "vps-id".into(),
                name: "vps".into(),
                access: Scope::Delegate,
                ask: false,
            }),
        };
        let text = format_status(&member, None);
        assert!(text.contains("Member of the hub vps at Delegate access, without asking"));
        assert!(text.contains("Not the hub."));
    }
}
