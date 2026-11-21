//! `_rustcode/machine/exec`: run a shell command on this computer for another
//! machine in the fleet (`run_machine_command`).
//!
//! Runs through the engine's own `run_command_output_with_workspace_for_call`,
//! so the target machine's `.rustcode/command-policy.toml` command guard,
//! toolchain-aware PATH, bounded output and cancellation all apply unchanged.

use std::path::PathBuf;

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Default command timeout when the caller omits `timeout_secs`.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Maximum command timeout accepted over ACP or orchestration.
pub(crate) const MAX_TIMEOUT_SECS: u64 = 300;
/// Maximum command text length, matching orchestration message bounds.
pub(crate) const MAX_COMMAND_BYTES: usize = 32 * 1024;

/// `_rustcode/machine/exec`: execute one shell command on this machine.
#[derive(
    Debug, Clone, serde::Serialize, serde::Deserialize, agent_client_protocol::JsonRpcRequest,
)]
#[request(method = "_rustcode/machine/exec", response = serde_json::Value)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineExecRequest {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

fn resolve_cwd(asked: Option<&str>) -> Result<PathBuf, String> {
    let trimmed = asked.map(str::trim).filter(|value| !value.is_empty());
    let dir = match trimmed {
        Some(path) => {
            let candidate = PathBuf::from(path);
            if !candidate.is_absolute() {
                return Err("cwd must be an absolute directory path".to_owned());
            }
            candidate
        }
        None => crate::home::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| "could not determine a working directory on this machine".to_owned())?,
    };
    if !dir.exists() {
        return Err(format!("cwd '{}' does not exist", dir.display()));
    }
    if !dir.is_dir() {
        return Err(format!("cwd '{}' is not a directory", dir.display()));
    }
    Ok(dir)
}

fn machine_name() -> String {
    crate::fleet::own_name_override()
        .or_else(crate::remote_gateway::gateway::os_host_name)
        .unwrap_or_else(|| "local".to_owned())
}

fn own_host_id() -> Option<String> {
    let dir = crate::config::get_config_dir()?;
    let id = crate::fleet::own_id(&dir);
    (!id.is_empty()).then_some(id)
}

/// Execute `request` on this machine inside a blocking worker thread.
pub(crate) async fn exec(
    request: MachineExecRequest,
    cancel: CancellationToken,
) -> Result<Value, String> {
    let command = request.command.trim().to_owned();
    if command.is_empty() {
        return Err("command is required".to_owned());
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(format!("command is over {MAX_COMMAND_BYTES} bytes"));
    }
    let cwd = resolve_cwd(request.cwd.as_deref())?;
    let timeout_secs = request
        .timeout_secs
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let cwd_str = cwd.to_string_lossy().into_owned();
    let args = json!({
        "command": command,
        "cwd": cwd_str,
        "timeout_ms": timeout_secs.saturating_mul(1000),
    });
    let workspace = cwd.clone();
    let output = tokio::task::spawn_blocking(move || {
        crate::tools::run_command_output_with_workspace_for_call(
            &args,
            Some(cancel),
            None,
            Some(workspace),
        )
    })
    .await
    .map_err(|error| format!("command worker failed: {error}"))?;

    let mut answer = json!({
        "success": output.success,
        "output": output.content,
        "cwd": cwd.to_string_lossy(),
        "machine": machine_name(),
    });
    if let Some(host_id) = own_host_id() {
        answer["host_id"] = json!(host_id);
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_a_command_in_the_requested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let answer = exec(
            MachineExecRequest {
                command: "echo fleet-ok".to_owned(),
                cwd: Some(dir.path().to_string_lossy().into_owned()),
                timeout_secs: Some(15),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(answer["success"], json!(true), "{answer}");
        assert!(
            answer["output"]
                .as_str()
                .unwrap_or_default()
                .contains("fleet-ok"),
            "{answer}"
        );
        assert_eq!(answer["cwd"], json!(dir.path().to_string_lossy()));
    }

    #[tokio::test]
    async fn refuses_empty_command_and_relative_or_missing_cwd() {
        let empty = exec(
            MachineExecRequest {
                command: "   ".to_owned(),
                cwd: None,
                timeout_secs: None,
            },
            CancellationToken::new(),
        )
        .await;
        assert!(empty.is_err());

        let relative = exec(
            MachineExecRequest {
                command: "echo hi".to_owned(),
                cwd: Some("relative/dir".to_owned()),
                timeout_secs: None,
            },
            CancellationToken::new(),
        )
        .await;
        assert!(relative.is_err());

        let missing = exec(
            MachineExecRequest {
                command: "echo hi".to_owned(),
                cwd: Some("/nonexistent/rustcode-fleet-dir-xyz".to_owned()),
                timeout_secs: None,
            },
            CancellationToken::new(),
        )
        .await;
        assert!(missing.is_err());
    }
}
