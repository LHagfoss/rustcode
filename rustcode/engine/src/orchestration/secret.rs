//! Private user replies bypass the harness transcript and orchestration audit.
use super::{
    ask_user::Asker,
    hub::{CallAnswer, CallRequest},
};
use agent_client_protocol::schema::v1::{
    PermissionOption, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    ToolCallUpdate, ToolCallUpdateFields,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub const TOOL_NAME: &str = "request_secret";
pub fn schema() -> Value {
    json!({"type":"object","properties":{"name":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"},"reason":{"type":"string","minLength":1},"destination":{"type":"string","minLength":1}},"required":["name","reason","destination"],"additionalProperties":false})
}
fn destination(root: &Path, file: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|_| "Workspace unavailable")?;
    let path = root.join(file);
    let parent = path
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .ok_or("Destination parent must exist")?;
    if !parent.starts_with(&root) {
        return Err("Destination must be inside the workspace".into());
    }
    let path = parent.join(path.file_name().ok_or("Invalid destination")?);
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err("Destination must be a regular file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                return Err("Hard-linked destinations are refused".into());
            }
        }
    }
    let relative = path.strip_prefix(&root).map_err(|_| "Outside workspace")?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(relative)
        .output()
        .map_err(|_| "Cannot check git tracking")?;
    if output.status.success() {
        return Err("Tracked destinations are refused".into());
    }
    if output.status.code() != Some(1) {
        return Err("Cannot verify destination is untracked".into());
    }
    Ok(path)
}
fn store(root: &Path, file: &str, name: &str, value: &str) -> Result<(), String> {
    if value.contains(['\n', '\r', '\0']) || value.len() > 65536 {
        return Err("Secret must be a single line of at most 64 KiB".into());
    }
    let path = destination(root, file)?;
    let old = if path.exists() {
        std::fs::read_to_string(&path).map_err(|_| "Cannot read destination")?
    } else {
        String::new()
    };
    let mut content = old
        .lines()
        .filter(|line| {
            let assignment = line
                .trim_start()
                .strip_prefix("export ")
                .unwrap_or(line.trim_start());
            assignment
                .split_once('=')
                .is_none_or(|(key, _)| key.trim() != name)
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`");
    content.push(format!("{name}=\"{escaped}\""));
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid destination")?)
        .map_err(|_| "Cannot create private destination")?;
    tmp.write_all(format!("{}\n", content.join("\n")).as_bytes())
        .map_err(|_| "Cannot write destination")?;
    tmp.persist(&path).map_err(|_| "Cannot save destination")?;
    Ok(())
}
pub async fn answer(
    request: &CallRequest,
    root: Option<PathBuf>,
    asker: Option<Asker>,
    cancel: &tokio_util::sync::CancellationToken,
) -> CallAnswer {
    let run = async {
        let root = root.ok_or("Secret requests need an attached workspace")?;
        let asker = asker.ok_or("No private user response channel")?;
        let name = request.args["name"].as_str().ok_or("Missing name")?;
        if name.is_empty()
            || !name
                .bytes()
                .enumerate()
                .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        {
            return Err("Invalid environment variable name".to_owned());
        }
        let reason = request.args["reason"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or("Missing reason")?;
        let file = request.args["destination"]
            .as_str()
            .ok_or("Missing destination")?;
        destination(&root, file)?;
        let call = ToolCallUpdate::new(
            format!("secret:{}", rand::random::<u64>()),
            ToolCallUpdateFields::new()
                .title(format!("Secret {name}"))
                .raw_input(json!({"name":name,"reason":reason,"destination":file})),
        );
        let mut meta = serde_json::Map::new();
        meta.insert("rustcode".into(), json!({"kind":"secret","version":1}));
        let permission = RequestPermissionRequest::new(
            request.session_id.clone(),
            call,
            vec![
                PermissionOption::new("secret_save", "Save", PermissionOptionKind::AllowOnce),
                PermissionOption::new(
                    "secret_decline",
                    "Decline",
                    PermissionOptionKind::RejectOnce,
                ),
            ],
        )
        .meta(meta);
        let response = tokio::select! { _ = cancel.cancelled() => None, response = asker(permission) => response };
        let Some(response) = response else {
            return Ok("Declined".into());
        };
        if !matches!(&response.outcome, RequestPermissionOutcome::Selected(s) if s.option_id.0.as_ref() == "secret_save")
        {
            return Ok("Declined".into());
        }
        let value = response
            .meta
            .as_ref()
            .and_then(|m| m.get("rustcode"))
            .and_then(|m| m.get("secret"))
            .and_then(Value::as_str)
            .ok_or("Missing private secret response")?;
        store(&root, file, name, value)?;
        Ok(format!("Secret {name} saved to {file}"))
    };
    match run.await {
        Ok(content) => CallAnswer {
            success: true,
            content,
        },
        Err(error) => CallAnswer::failure(error),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn workspace() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("init")
                .arg(root.path())
                .output()
                .unwrap()
                .status
                .success()
        );
        root
    }
    #[test]
    fn refuses_outside_tracked_and_links() {
        let root = workspace();
        assert!(destination(root.path(), "../outside.env").is_err());
        std::fs::write(root.path().join("tracked"), "").unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(root.path())
            .args(["add", "tracked"])
            .output()
            .unwrap();
        assert!(destination(root.path(), "tracked").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("tracked", root.path().join("link")).unwrap();
            assert!(destination(root.path(), "link").is_err());
        }
    }
    #[tokio::test]
    async fn value_never_enters_request_or_tool_result() {
        use agent_client_protocol::schema::v1::{
            RequestPermissionResponse, SelectedPermissionOutcome,
        };
        let root = workspace();
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured = recorded.clone();
        let asker: Asker = std::sync::Arc::new(move |request| {
            *captured.lock().unwrap() = serde_json::to_string(&request).unwrap();
            Box::pin(async {
                let mut meta = serde_json::Map::new();
                meta.insert("rustcode".into(), json!({"secret":"private-canary-97531"}));
                Some(
                    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                        SelectedPermissionOutcome::new("secret_save"),
                    ))
                    .meta(meta),
                )
            })
        });
        let request = CallRequest {
            session_id: "thread".into(),
            token: String::new(),
            tool: TOOL_NAME.into(),
            args: json!({"name":"KEY","reason":"Deploy","destination":".env"}),
            outside: false,
        };
        let result = answer(
            &request,
            Some(root.path().into()),
            Some(asker),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(result.success, "{}", result.content);
        assert!(!result.content.contains("private-canary-97531"));
        // Permission metadata is the only payload offered to transcript/mirror clients.
        assert!(!recorded.lock().unwrap().contains("private-canary-97531"));
        assert!(
            std::fs::read_to_string(root.path().join(".env"))
                .unwrap()
                .contains("private-canary-97531")
        );
    }

    #[test]
    fn merges_private_dotenv() {
        let root = workspace();
        std::fs::write(root.path().join(".env"), "OTHER=keep\nKEY=old\n").unwrap();
        store(root.path(), ".env", "KEY", "private-value").unwrap();
        let text = std::fs::read_to_string(root.path().join(".env")).unwrap();
        assert_eq!(text, "OTHER=keep\nKEY=\"private-value\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(root.path().join(".env"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}

pub async fn execute(
    state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    cancel: &tokio_util::sync::CancellationToken,
    args: &Value,
) -> crate::tools::ToolExecutionOutput {
    let (root, asker, id) = {
        let state = state.lock().await;
        (
            state.effective_workspace_root(),
            state.secret_asker.clone(),
            state.active_session_id.clone(),
        )
    };
    let result = answer(
        &CallRequest {
            session_id: id,
            token: String::new(),
            tool: TOOL_NAME.into(),
            args: args.clone(),
            outside: false,
        },
        root,
        asker,
        cancel,
    )
    .await;
    if result.success {
        crate::tools::ToolExecutionOutput::success(result.content)
    } else {
        crate::tools::ToolExecutionOutput::failure(result.content)
    }
}
