//! `rc config sync from <machine>`: copy a paired machine's skills, themes
//! and prompt commands here once, over the paired connection, without a Git
//! remote and without following it (`rustcode::config_files`, #4485).

use anyhow::{Context, Result, bail};
use rustcode::config_files::{self, ConfigKind, Entry, EntrySummary};
use rustcode_client::{ClientKind, Connector, MachineRegistry, TokenStore};

/// Which machine to copy from and which folders.
#[derive(clap::Args, Debug, Clone, PartialEq, Eq)]
pub struct SyncFromArgs {
    /// The paired machine: its name or host id, as `rc machines list` shows it
    pub machine: String,
    /// Only these folders, comma-separated: skills, themes, commands (default: all three)
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
}

fn kinds(only: &[String]) -> Result<Vec<ConfigKind>> {
    if only.is_empty() {
        return Ok(ConfigKind::ALL.to_vec());
    }
    only.iter()
        .map(|name| {
            ConfigKind::parse(name).with_context(|| {
                format!("'{name}' is not a folder that travels; use skills, themes or commands")
            })
        })
        .collect()
}

pub async fn run(args: &SyncFromArgs) -> Result<()> {
    let kinds = kinds(&args.only)?;
    let client_dir = rustcode_client::config::config_dir()?;
    let Some(config_dir) = rustcode::config::get_config_dir() else {
        bail!("there is no configuration directory");
    };
    let registry = MachineRegistry::load(&client_dir)?;
    let machine = registry.select(&args.machine)?.clone();
    let tokens = TokenStore::open(&client_dir)?;
    let connector = Connector::new(&client_dir, ClientKind::Desktop)?;
    let connected = connector
        .connect_raced(&machine, &tokens)
        .await
        .with_context(|| format!("could not reach {}", machine.name))?;
    let mut channel = connected.session.acp().await?;
    channel
        .request(1, "initialize", serde_json::json!({"protocolVersion": 1}))
        .await?;
    let listing = channel
        .request(
            2,
            "_rustcode/config/files/list",
            serde_json::json!({ "kinds": kinds }),
        )
        .await
        .context("that machine's RustCode is too old to share its skills; update it first")?;
    let remote: Vec<EntrySummary> = serde_json::from_value(listing["result"]["entries"].clone())?;
    let mut fetched: Vec<Entry> = Vec::new();
    for (id, (kind, name)) in (3u64..).zip(config_files::plan(&config_dir, &kinds, &remote)) {
        let answer = channel
            .request(
                id,
                "_rustcode/config/files/get",
                serde_json::json!({ "kind": kind, "name": name }),
            )
            .await?;
        fetched.push(serde_json::from_value(answer["result"].clone())?);
    }
    let report = config_files::apply(&config_dir, &machine.host_id, &kinds, &remote, &fetched)
        .map_err(anyhow::Error::msg)?;
    let name = machine.display_name.as_deref().unwrap_or(&machine.name);
    println!("From {name}: {}.", report.summary());
    for skipped in &report.skipped {
        println!("Skipped {skipped}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_names_the_folders_that_travel() {
        assert_eq!(kinds(&[]).unwrap(), ConfigKind::ALL.to_vec());
        assert_eq!(
            kinds(&["themes".to_owned()]).unwrap(),
            vec![ConfigKind::Themes]
        );
        assert!(kinds(&["credentials".to_owned()]).is_err());
    }
}
