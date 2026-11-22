//! Private user replies bypass the harness transcript and orchestration audit.
use super::{
    ask_user::Asker,
    tool_socket::{CallAnswer, CallRequest},
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
    json!({"type":"object","properties":{"name":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"},"reason":{"type":"string","minLength":1},"destination":{"type":"string","minLength":1},"env":{"type":"boolean"}},"required":["name","reason"],"additionalProperties":false})
}

/// Secrets a thread received as environment variables (`env: true`). They live
/// in this process's memory only, keyed by thread, until the thread closes;
/// nothing here is ever written to disk.
type ThreadEnv = std::collections::HashMap<String, std::collections::BTreeMap<String, String>>;
static THREAD_ENV: std::sync::LazyLock<std::sync::Mutex<ThreadEnv>> =
    std::sync::LazyLock::new(Default::default);
/// Shorter values are not masked: replacing every `1` or `ab` in command
/// output would garble it while hiding nothing.
const MIN_MASKED_LEN: usize = 4;
pub const MASK: &str = "***";

fn thread_env_store() -> std::sync::MutexGuard<'static, ThreadEnv> {
    THREAD_ENV.lock().unwrap_or_else(|e| e.into_inner())
}
/// Make `name` available to the thread's later commands.
pub fn set_thread_env(session_id: &str, name: &str, value: &str) {
    thread_env_store()
        .entry(session_id.to_owned())
        .or_default()
        .insert(name.to_owned(), value.to_owned());
}
/// The variables `run_command` adds to a command the thread runs.
pub fn thread_env(session_id: &str) -> Vec<(String, String)> {
    thread_env_store()
        .get(session_id)
        .map(|vars| vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}
/// Drop a closed thread's secrets.
pub fn forget_thread_env(session_id: &str) {
    thread_env_store().remove(session_id);
}
/// `text` with every held secret value replaced by [`MASK`], or `None` when it
/// holds none. Every thread's values are masked: output is only ever shown, so
/// hiding another thread's value too costs nothing and needs no thread id.
pub fn redact(text: &str) -> Option<String> {
    let mut values: Vec<String> = {
        let store = thread_env_store();
        if store.is_empty() {
            return None;
        }
        store
            .values()
            .flat_map(|vars| vars.values())
            .filter(|value| value.len() >= MIN_MASKED_LEN && text.contains(value.as_str()))
            .cloned()
            .collect()
    };
    if values.is_empty() {
        return None;
    }
    // Longest first, so a value containing another is masked whole.
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    let mut text = text.to_owned();
    for value in values {
        text = text.replace(&value, MASK);
    }
    Some(text)
}
/// Mask held secrets in a tool's result before it reaches the model, the
/// transcript or history.
pub fn redact_output(output: &mut rustcode_tools::ToolExecutionOutput) {
    if let Some(content) = redact(&output.content) {
        output.content = content;
    }
    if let Some(command) = output.command.as_deref().and_then(redact) {
        output.command = Some(command);
    }
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
/// Answer a `request_secret` call. `env_thread` names the thread whose later
/// commands receive the value when the call asks for `env`; it is `None` for an
/// external harness, whose shell the engine does not run.
pub async fn answer(
    request: &CallRequest,
    root: Option<PathBuf>,
    asker: Option<Asker>,
    env_thread: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> CallAnswer {
    let run = async {
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
        let file = request.args["destination"].as_str();
        let env = request.args["env"].as_bool().unwrap_or(false);
        if file.is_none() && !env {
            return Err("Pass destination, env: true, or both".to_owned());
        }
        if env && env_thread.is_none() {
            return Err(
                "env needs RustCode's built-in harness, which runs the commands; pass destination"
                    .to_owned(),
            );
        }
        let root = match file {
            Some(file) => {
                let root = root.ok_or("Secret requests need an attached workspace")?;
                destination(&root, file)?;
                Some((root, file))
            }
            None => None,
        };
        let call = ToolCallUpdate::new(
            format!("secret:{}", rand::random::<u64>()),
            ToolCallUpdateFields::new()
                .title(format!("Secret {name}"))
                .raw_input(json!({"name":name,"reason":reason,"destination":file,"env":env})),
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
        if value.contains(['\n', '\r', '\0']) || value.len() > 65536 {
            return Err("Secret must be a single line of at most 64 KiB".into());
        }
        let mut told = Vec::new();
        if let Some((root, file)) = &root {
            store(root, file, name, value)?;
            told.push(format!("Secret {name} saved to {file}."));
        }
        if let Some(thread) = env_thread.filter(|_| env) {
            set_thread_env(thread, name, value);
            told.push(format!(
                "${name} is set for this thread's later run_command calls; its value shows as {MASK} in output."
            ));
        }
        Ok(told.join(" "))
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
            None,
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

    fn saving_asker(value: &'static str) -> Asker {
        use agent_client_protocol::schema::v1::{
            RequestPermissionResponse, SelectedPermissionOutcome,
        };
        std::sync::Arc::new(move |_| {
            Box::pin(async move {
                let mut meta = serde_json::Map::new();
                meta.insert("rustcode".into(), json!({"secret": value}));
                Some(
                    RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                        SelectedPermissionOutcome::new("secret_save"),
                    ))
                    .meta(meta),
                )
            })
        })
    }

    fn env_request(thread: &str, name: &str) -> CallRequest {
        CallRequest {
            session_id: thread.into(),
            token: String::new(),
            tool: TOOL_NAME.into(),
            args: json!({"name":name,"reason":"Run the deploy","env":true}),
            outside: false,
        }
    }

    #[tokio::test]
    async fn env_secret_stays_in_memory_and_out_of_the_result() {
        let thread = "secret-env-memory";
        let root = workspace();
        let result = answer(
            &env_request(thread, "STRIPE_KEY"),
            Some(root.path().into()),
            Some(saving_asker("env-canary-24680")),
            Some(thread),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(result.success, "{}", result.content);
        assert!(result.content.contains("$STRIPE_KEY is set"));
        assert!(!result.content.contains("env-canary-24680"));
        assert_eq!(
            thread_env(thread),
            [("STRIPE_KEY".to_owned(), "env-canary-24680".to_owned())]
        );
        // No destination: nothing was written into the workspace.
        let written = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != ".git")
            .count();
        assert_eq!(written, 0);
        forget_thread_env(thread);
        assert!(thread_env(thread).is_empty());
    }

    #[tokio::test]
    async fn env_is_refused_for_an_external_harness() {
        let result = answer(
            &env_request("secret-env-external", "KEY"),
            None,
            Some(saving_asker("never-stored-1357")),
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
        assert!(!result.success);
        assert!(result.content.contains("destination"));
        assert!(thread_env("secret-env-external").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn later_command_receives_the_secret_and_its_output_is_masked() {
        let thread = "secret-env-exec";
        let value = "exec-canary-11235";
        set_thread_env(thread, "RC_TEST_SECRET", value);
        crate::tools::set_active_session_id(Some(thread.to_owned()));
        let root = tempfile::tempdir().unwrap();
        // The command names only the variable, never the value.
        let mut output = crate::tools::exec::run_command_output_with_workspace(
            &json!({"command": "printf 'len=%s\\n' ${#RC_TEST_SECRET}; echo \"key=$RC_TEST_SECRET\""}),
            Some(root.path().to_path_buf()),
        )
        .unwrap();
        crate::tools::set_active_session_id(None);
        assert!(output.success, "{}", output.content);
        assert!(output.content.contains(&format!("len={}", value.len())));
        // The raw process output holds the value; what reaches the model does not.
        assert!(output.content.contains(value));
        redact_output(&mut output);
        assert!(output.content.contains("key=***"), "{}", output.content);
        assert!(!output.content.contains(value));
        // Another thread's commands do not receive it.
        assert!(thread_env("secret-env-exec-other").is_empty());
        forget_thread_env(thread);
    }

    #[test]
    fn echoed_secret_never_reaches_history() {
        let thread = "secret-env-history";
        let value = "history-canary-31415";
        set_thread_env(thread, "DEPLOY_TOKEN", value);
        let echoed = || {
            let mut output =
                rustcode_tools::ToolExecutionOutput::success(format!("token {value}\n"));
            output.command = Some(format!("echo {value}"));
            output
        };
        // A background completion is built straight into a history message.
        let message = crate::background_task_history_message("task_1", Some("call_1"), echoed());
        let saved = serde_json::to_string(&message).unwrap();
        assert!(!saved.contains(value), "{saved}");
        assert!(saved.contains(MASK));
        // A foreground result is masked before it becomes a tool message.
        let mut output = echoed();
        redact_output(&mut output);
        assert!(!output.content.contains(value));
        assert!(!output.command.unwrap().contains(value));
        // Live output chunks are masked as well.
        assert_eq!(redact(&format!("a{value}b")).unwrap(), "a***b");
        forget_thread_env(thread);
        assert!(redact(&format!("a{value}b")).is_none());
    }

    #[test]
    fn short_values_are_not_masked() {
        let thread = "secret-env-short";
        set_thread_env(thread, "PIN", "ab1");
        assert!(redact("cab1d").is_none());
        forget_thread_env(thread);
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
) -> rustcode_tools::ToolExecutionOutput {
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
            session_id: id.clone(),
            token: String::new(),
            tool: TOOL_NAME.into(),
            args: args.clone(),
            outside: false,
        },
        root,
        asker,
        Some(&id),
        cancel,
    )
    .await;
    if result.success {
        rustcode_tools::ToolExecutionOutput::success(result.content)
    } else {
        rustcode_tools::ToolExecutionOutput::failure(result.content)
    }
}
