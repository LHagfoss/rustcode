//! Edits to the user-level `[[mcp_servers]]` list made by `rustcode mcp`.

use super::{McpServerConfig, get_config_dir, load_config_from, save_config_to_result};
use std::path::Path;

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

pub(super) fn add_mcp_server_in(
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

pub(super) fn remove_mcp_server_in(dir: &Path, name: &str) -> Result<McpServerConfig, String> {
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
