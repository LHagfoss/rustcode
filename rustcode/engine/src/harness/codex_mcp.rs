//! The MCP servers Codex already defines in its own `config.toml` files.
//!
//! A `thread/start` `config` override is layered over Codex's own config table by table, so a
//! workspace server sent as `mcp_servers.<name>.url` on top of Codex's `mcp_servers.<name>` with a
//! `command` leaves one entry with both, and Codex refuses to start the thread: "url is not
//! supported for stdio in `mcp_servers.<name>`". Such a server is left to Codex's own definition.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Transport {
    Stdio,
    Http,
}

impl Transport {
    pub(super) fn of(server: &crate::config::McpServerConfig) -> Self {
        if server.url.is_some() {
            Self::Http
        } else {
            Self::Stdio
        }
    }
}

/// Server names and transports from Codex's home config (`home`, else `CODEX_HOME`, else
/// `~/.codex`) and every `.codex/config.toml` from `cwd` up.
pub(super) fn own_servers(home: Option<&Path>, cwd: &Path) -> HashMap<String, Transport> {
    let home = home.map(Path::to_path_buf).or_else(default_home);
    let files = home.map(|home| home.join("config.toml")).into_iter().chain(
        cwd.ancestors()
            .map(|dir| dir.join(".codex").join("config.toml")),
    );
    let mut servers = HashMap::new();
    for file in files {
        if let Ok(text) = std::fs::read_to_string(&file) {
            for (name, transport) in servers_in(&text) {
                servers.entry(name).or_insert(transport);
            }
        }
    }
    servers
}

fn default_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".codex")))
}

/// The `[mcp_servers.<name>]` tables in one Codex config file with a `url` or a `command`.
pub(super) fn servers_in(text: &str) -> Vec<(String, Transport)> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let Some(servers) = table.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let entry = entry.as_table()?;
            let transport = if entry.contains_key("url") {
                Transport::Http
            } else if entry.contains_key("command") {
                Transport::Stdio
            } else {
                return None;
            };
            Some((name.clone(), transport))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_servers_transport() {
        let text = r#"
[mcp_servers.context7]
command = "npx"
args = ["-y", "@upstash/context7-mcp"]

[mcp_servers.vercel]
url = "https://mcp.vercel.com"

[mcp_servers.partial]
enabled = false
"#;
        let mut servers = servers_in(text);
        servers.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            servers,
            vec![
                ("context7".to_owned(), Transport::Stdio),
                ("vercel".to_owned(), Transport::Http),
            ]
        );
        assert!(servers_in("not = [toml").is_empty());
    }

    #[test]
    fn reads_the_home_and_project_configs() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex-home");
        let project = root.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[mcp_servers.context7]\ncommand = \"npx\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".codex").join("config.toml"),
            "[mcp_servers.docs]\nurl = \"https://docs.example.com/mcp\"\n",
        )
        .unwrap();
        let servers = own_servers(Some(&home), &project);
        assert_eq!(servers.get("context7"), Some(&Transport::Stdio));
        assert_eq!(servers.get("docs"), Some(&Transport::Http));
    }
}
