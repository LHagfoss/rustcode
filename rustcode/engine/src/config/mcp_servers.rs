//! Edits to the user-level `[[mcp_servers]]` list made by `rustcode mcp` and
//! by the `manage_mcp_servers` tool, which share one validator.

use super::{McpServerConfig, get_config_dir, load_config_from, save_config_to_result};
use std::path::Path;

/// Transport a new server is declared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Http,
}

/// What `mcp add` was given, before it is checked against the transport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpServerSpec {
    pub name: String,
    /// A URL for a remote server, or a command followed by its arguments.
    pub target: Vec<String>,
    /// Inferred from an http(s):// target when omitted.
    pub transport: Option<McpTransport>,
    /// `Name: value` pairs for a remote server.
    pub headers: Vec<String>,
    /// `KEY=value` pairs for a spawned server.
    pub env: Vec<String>,
    pub client_id: Option<String>,
    pub always_include: bool,
}

/// Turn `mcp add` input into a config entry, rejecting options that do not
/// apply to the chosen transport instead of dropping them.
pub fn mcp_server_from_spec(spec: &McpServerSpec) -> Result<McpServerConfig, String> {
    let Some(first) = spec.target.first().map(String::as_str) else {
        return Err("a server needs a URL or a command".to_owned());
    };
    let looks_remote = first.starts_with("http://") || first.starts_with("https://");
    let remote = match spec.transport {
        Some(McpTransport::Http) if !looks_remote => {
            return Err("--transport http needs an http:// or https:// URL".to_owned());
        }
        Some(McpTransport::Http) => true,
        Some(McpTransport::Stdio) => false,
        None => looks_remote,
    };
    if remote {
        if spec.target.len() > 1 {
            return Err("a remote server takes a URL and no further arguments".to_owned());
        }
        if !spec.env.is_empty() {
            return Err("--env only applies to stdio servers; use --header".to_owned());
        }
    } else if !spec.headers.is_empty() || spec.client_id.is_some() {
        return Err("--header and --client-id only apply to remote (http) servers".to_owned());
    }
    let headers = spec
        .headers
        .iter()
        .map(|raw| parse_mcp_header(raw))
        .collect::<Result<_, _>>()?;
    let env = spec
        .env
        .iter()
        .map(|raw| parse_mcp_env(raw))
        .collect::<Result<_, _>>()?;
    Ok(McpServerConfig {
        name: spec.name.clone(),
        command: if remote {
            String::new()
        } else {
            first.to_owned()
        },
        args: if remote {
            Vec::new()
        } else {
            spec.target[1..].to_vec()
        },
        env,
        url: remote.then(|| first.to_owned()),
        headers,
        client_id: spec.client_id.clone(),
        enabled: true,
        always_include: spec.always_include,
    })
}

/// One `mcp list` line. Header and environment values are secrets, so only
/// their names are shown.
pub fn mcp_server_summary(server: &McpServerConfig) -> String {
    let mut line = match server.url.as_deref().filter(|_| server.is_remote()) {
        Some(url) => format!("{}  http  {url}", server.name),
        None => {
            let mut command = vec![server.command.as_str()];
            command.extend(server.args.iter().map(String::as_str));
            format!("{}  stdio  {}", server.name, command.join(" "))
        }
    };
    for (label, values) in [("headers", &server.headers), ("env", &server.env)] {
        if !values.is_empty() {
            let mut names: Vec<&str> = values.keys().map(String::as_str).collect();
            names.sort_unstable();
            line.push_str(&format!("  ({label}: {})", names.join(", ")));
        }
    }
    if !server.enabled {
        line.push_str("  [disabled]");
    }
    line
}

/// Split a `Name: value` header argument and reject anything the remote
/// transport would otherwise drop when it builds the request.
pub fn parse_mcp_header(raw: &str) -> Result<(String, String), String> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| "header must look like `Name: value`".to_owned())?;
    let (name, value) = (name.trim(), value.trim());
    if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
        return Err(format!("`{name}` is not a valid header name"));
    }
    // The value is deliberately left out of the message: it is usually a secret.
    if value.is_empty() || reqwest::header::HeaderValue::from_str(value).is_err() {
        return Err(format!("header `{name}` has an empty or invalid value"));
    }
    Ok((name.to_owned(), value.to_owned()))
}

/// Split a `KEY=value` environment argument for a stdio server.
pub fn parse_mcp_env(raw: &str) -> Result<(String, String), String> {
    match raw.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_owned(), value.to_owned()))
        }
        _ => Err("environment variable must look like `KEY=value`".to_owned()),
    }
}

/// Add `server` to the user config. Returns true when it replaced an entry
/// with the same name, which only happens when `replace` is set.
pub fn add_mcp_server(server: McpServerConfig, replace: bool) -> Result<bool, String> {
    add_mcp_server_in(&user_config_dir()?, server, replace)
}

/// Remove the named server from the user config and return its entry.
pub fn remove_mcp_server(name: &str) -> Result<McpServerConfig, String> {
    remove_mcp_server_in(&user_config_dir()?, name)
}

fn user_config_dir() -> Result<std::path::PathBuf, String> {
    get_config_dir().ok_or_else(|| "could not determine the config directory".to_owned())
}

/// The servers active in `workspace`: the user config in `dir` with any
/// project files overlaid.
pub(crate) fn workspace_mcp_servers_in(dir: &Path, workspace: &Path) -> Vec<McpServerConfig> {
    let (_, _, config) = load_config_from(dir);
    super::overlay_project_config(config, workspace)
        .2
        .mcp_servers
}

pub(crate) fn add_mcp_server_in(
    dir: &Path,
    server: McpServerConfig,
    replace: bool,
) -> Result<bool, String> {
    if server.name.is_empty()
        || !server
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "MCP server name '{}' may only contain letters, digits, '-' and '_'",
            server.name
        ));
    }
    server.validate()?;
    edit_mcp_servers(dir, |servers| {
        match servers.iter().position(|entry| entry.name == server.name) {
            Some(_) if !replace => Err(format!(
                "MCP server '{}' already exists; pass --force to replace it",
                server.name
            )),
            Some(index) => {
                servers[index] = server;
                Ok(true)
            }
            None => {
                servers.push(server);
                Ok(false)
            }
        }
    })
}

pub(crate) fn remove_mcp_server_in(dir: &Path, name: &str) -> Result<McpServerConfig, String> {
    edit_mcp_servers(dir, |servers| {
        servers
            .iter()
            .position(|entry| entry.name == name)
            .map(|index| servers.remove(index))
            .ok_or_else(|| format!("no MCP server named '{name}' in the user config"))
    })
}

/// Apply `edit` to the user config alone, so servers a project file overlays
/// are never written back into the user file.
fn edit_mcp_servers<T>(
    dir: &Path,
    edit: impl FnOnce(&mut Vec<McpServerConfig>) -> Result<T, String>,
) -> Result<T, String> {
    let (_, _, mut config) = load_config_from(dir);
    if !config.is_valid {
        return Err(format!(
            "the config in {} could not be read; fix it before changing MCP servers",
            dir.display()
        ));
    }
    let outcome = edit(&mut config.mcp_servers)?;
    save_config_to_result(dir, &config)?;
    Ok(outcome)
}
