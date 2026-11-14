//! Project actions that run on their own (#3725): one when a thread's worktree
//! is created (`bun install`, copying `.env`), one when the thread settles.
//!
//! The hooks come from two places and merge, the repository's file winning a
//! slot both fill: `.rustcode/actions.json` in the repository, which a team
//! shares, and the app's own project actions, which `session/new` carries in
//! `_meta.rustcode.worktree.hooks`. The engine runs them, so a thread an agent
//! starts with `start_session` gets the same setup without the desktop app.
//!
//! A create hook starts as soon as the worktree exists. With `wait`, the
//! thread's first turn is held until it exits (at most [`WAIT_LIMIT`]); a
//! failure is told to the agent at the top of that turn, with its output tail.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long a held turn waits for the create hook, and how long a hook runs.
pub(crate) const WAIT_LIMIT: Duration = Duration::from_secs(600);
/// The file a repository names its shared actions in.
pub(crate) const ACTIONS_FILE: &str = ".rustcode/actions.json";
const OUTPUT_TAIL: usize = 3_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hook {
    pub name: String,
    pub command: String,
    /// Hold the thread's first turn until the hook exits (create hooks only).
    #[serde(default)]
    pub wait: bool,
    #[serde(default = "legacy_repository_action")]
    pub repository: bool,
}

fn legacy_repository_action() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<Hook>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settle: Option<Hook>,
}

impl Hooks {
    pub(crate) fn is_empty(&self) -> bool {
        self.create.is_none() && self.settle.is_none()
    }
}

/// How a hook ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Outcome {
    pub name: String,
    pub command: String,
    pub ok: bool,
    pub output: String,
}

fn hook_of(value: &Value) -> Option<Hook> {
    let name = value.get("name").and_then(Value::as_str)?.trim();
    let command = value.get("command").and_then(Value::as_str)?.trim();
    if command.is_empty() {
        return None;
    }
    Some(Hook {
        name: if name.is_empty() { command } else { name }.to_owned(),
        command: command.to_owned(),
        wait: value.get("wait").and_then(Value::as_bool).unwrap_or(false),
        repository: false,
    })
}

/// The hooks a `.rustcode/actions.json` names: the first action marked
/// `runOnWorktreeCreate` and the first marked `runOnSettle`.
pub(crate) fn parse_file(text: &str) -> Hooks {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Hooks::default();
    };
    let actions = value
        .get("actions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let first = |flag: &str| {
        actions
            .iter()
            .filter(|action| action.get(flag).and_then(Value::as_bool) == Some(true))
            .find_map(hook_of)
            .map(|hook| Hook {
                repository: true,
                ..hook
            })
    };
    Hooks {
        create: first("runOnWorktreeCreate"),
        settle: first("runOnSettle").map(|hook| Hook {
            wait: false,
            ..hook
        }),
    }
}

/// The hooks `_meta.rustcode.worktree.hooks` carries: `{ create, settle }`.
pub(crate) fn from_meta(meta: Option<&serde_json::Map<String, Value>>) -> Hooks {
    let Some(hooks) = meta
        .and_then(|meta| meta.get("rustcode"))
        .and_then(|rustcode| rustcode.get("worktree"))
        .and_then(|worktree| worktree.get("hooks"))
    else {
        return Hooks::default();
    };
    Hooks {
        create: hooks.get("create").and_then(hook_of),
        settle: hooks.get("settle").and_then(hook_of).map(|hook| Hook {
            wait: false,
            ..hook
        }),
    }
}

/// The repository's file wins each slot it fills; the app's list fills the rest.
pub(crate) fn merge(file: Hooks, app: Hooks) -> Hooks {
    Hooks {
        create: file.create.or(app.create),
        settle: file.settle.or(app.settle),
    }
}

/// The hooks of a worktree: its own copy of the repository's file, merged with
/// what the app sent.
pub(crate) fn resolve(worktree: &Path, meta: Option<&serde_json::Map<String, Value>>) -> Hooks {
    let file = std::fs::read_to_string(worktree.join(ACTIONS_FILE))
        .map(|text| parse_file(&text))
        .unwrap_or_default();
    merge(file, from_meta(meta))
}

type Channel = crate::acp::clients::SessionClients;
static CHANNELS: LazyLock<Mutex<HashMap<PathBuf, Channel>>> = LazyLock::new(Default::default);
type Pending = (Hook, Arc<Slot>);
static PENDING: LazyLock<Mutex<HashMap<PathBuf, Pending>>> = LazyLock::new(Default::default);
static SETTLING: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

struct SettleGuard(PathBuf);
impl SettleGuard {
    fn acquire(project: &Path) -> Option<Self> {
        let key = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf());
        let inserted = SETTLING.lock().ok()?.insert(key.clone());
        // Only a successful acquisition owns a guard. Constructing and
        // dropping one on rejection would relock the mutex and remove the
        // existing runner's entry.
        inserted.then(|| Self(key))
    }
}
impl Drop for SettleGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = SETTLING.lock() {
            active.remove(&self.0);
        }
    }
}

fn consent_key(repo: &Path, scope: &str, command: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(repo, scope, command)).unwrap())
    )
}

fn trust_path(cwd: &Path, command: &str) -> Option<PathBuf> {
    let repo = crate::worktrees::repo_of(cwd)?;
    // Unknown provenance fails closed, as does a fork branch marked by prepare_branch.
    let branch = crate::pull_requests::current_branch(cwd)?;
    let status = crate::platform::command("git")
        .current_dir(cwd)
        .args([
            "config",
            "--get",
            &format!("branch.{branch}.rustcodeUntrustedActions"),
        ])
        .output()
        .ok()?;
    let scope = if status.status.code() == Some(1) {
        String::new()
    } else {
        cwd.to_string_lossy().into_owned()
    };
    Some(
        crate::config::get_config_dir()?
            .join("project-action-consent")
            .join(consent_key(&repo, &scope, command)),
    )
}

fn needs_approval(repository: bool, consent: Option<&Path>) -> bool {
    repository && !consent.is_some_and(Path::is_file)
}

async fn approve(clients: &Channel, cwd: &Path, hook: &Hook) -> bool {
    use agent_client_protocol::schema::v1::*;
    if !hook.repository {
        return true;
    }
    let path = trust_path(cwd, &hook.command);
    if !needs_approval(hook.repository, path.as_deref()) {
        return true;
    }
    let request = RequestPermissionRequest::new(
        clients.session_id().to_owned(),
        ToolCallUpdate::new(
            format!("project-action-{:016x}", rand::random::<u64>()),
            ToolCallUpdateFields::new()
                .title(format!(
                    "Repository action: {} — {}",
                    hook.name, hook.command
                ))
                .raw_input(serde_json::json!({"command": hook.command, "cwd": cwd})),
        ),
        vec![
            PermissionOption::new("once", "Run once", PermissionOptionKind::AllowOnce),
            PermissionOption::new(
                "always",
                "Always for this project",
                PermissionOptionKind::AllowAlways,
            ),
            PermissionOption::new("deny", "Don't run", PermissionOptionKind::RejectOnce),
        ],
    );
    let response = tokio::time::timeout(WAIT_LIMIT, clients.request(None, request))
        .await
        .ok()
        .and_then(Result::ok);
    let choice = response.and_then(|response| match response.outcome {
        RequestPermissionOutcome::Selected(selected) => Some(selected.option_id.to_string()),
        _ => None,
    });
    let allowed = matches!(choice.as_deref(), Some("once" | "always"));
    if choice.as_deref() == Some("always") {
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
                let _ = std::fs::write(path, b"approved");
            }
        }
    }
    if !allowed {
        let _ = clients.update(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(format!(
                "Skipped repository action '{}' ({}): not approved.\n",
                hook.name, hook.command
            ))),
        )));
    }
    allowed
}

fn kill_tree(pid: u32) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: the child was started in a separate process group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = crate::platform::command("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status();
    }
}

fn tail(text: &str) -> String {
    let text = text.trim();
    if text.len() <= OUTPUT_TAIL {
        return text.to_owned();
    }
    let mut start = text.len() - OUTPUT_TAIL;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Run `hook` in `cwd` through the user's shell, at most `limit`.
pub(crate) fn run(hook: &Hook, cwd: &Path, limit: Duration) -> Outcome {
    let mut command = if cfg!(windows) {
        let mut command = crate::platform::command("cmd");
        command.arg("/C").arg(&hook.command);
        command
    } else {
        let mut command = crate::platform::command("sh");
        command.arg("-c").arg(&hook.command);
        command
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let capture = || -> std::io::Result<(std::fs::File, std::fs::File)> {
        Ok((tempfile::tempfile()?, tempfile::tempfile()?))
    };
    let (mut stdout, mut stderr) = match capture() {
        Ok(files) => files,
        Err(error) => {
            return Outcome {
                name: hook.name.clone(),
                command: hook.command.clone(),
                ok: false,
                output: format!("could not capture output: {error}"),
            };
        }
    };
    let handles = stdout
        .try_clone()
        .and_then(|out| stderr.try_clone().map(|err| (out, err)));
    let (out, err) = match handles {
        Ok(handles) => handles,
        Err(error) => {
            return Outcome {
                name: hook.name.clone(),
                command: hook.command.clone(),
                ok: false,
                output: format!("could not capture output: {error}"),
            };
        }
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    let outcome = |ok: bool, output: String| Outcome {
        name: hook.name.clone(),
        command: hook.command.clone(),
        ok,
        output: tail(&output),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return outcome(false, format!("could not start: {error}")),
    };
    let pid = child.id();
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                kill_tree(pid);
                break Some(status);
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_tree(pid);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                kill_tree(pid);
                let _ = child.wait();
                break None;
            }
        }
    };
    // Regular files avoid pipe-reader threads that can hang forever when a
    // descendant escapes the group or keeps an inherited handle open.
    let read_tail = |file: &mut std::fs::File| {
        use std::io::{Read, Seek, SeekFrom};
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(size.saturating_sub(OUTPUT_TAIL as u64)));
        let mut bytes = Vec::new();
        let _ = file.take(OUTPUT_TAIL as u64).read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let mut output = read_tail(&mut stdout);
    output.push_str(&read_tail(&mut stderr));
    match status {
        Some(status) if status.success() => outcome(true, output),
        Some(status) => outcome(false, format!("{output}\n(exit status {status})")),
        None => outcome(
            false,
            format!("{output}\n(stopped after {} s)", limit.as_secs()),
        ),
    }
}

// ------------------------------------------------------- the create hook ----

type Slot = tokio::sync::watch::Sender<Option<Outcome>>;

/// The create hooks started in this engine, by worktree folder.
static SETUPS: LazyLock<Mutex<HashMap<PathBuf, (bool, Arc<Slot>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Start `worktree`'s create hook in the background.
pub(crate) fn start_create(worktree: &Path, hook: Hook) {
    let (sender, _) = tokio::sync::watch::channel(None);
    let slot = Arc::new(sender);
    if let Ok(mut setups) = SETUPS.lock() {
        setups.insert(worktree.to_path_buf(), (hook.wait, Arc::clone(&slot)));
    }
    let cwd = worktree.to_path_buf();
    PENDING.lock().unwrap().insert(cwd, (hook, slot));
}

pub(crate) fn attach(clients: crate::acp::clients::SessionClients, worktree: &Path) {
    CHANNELS
        .lock()
        .unwrap()
        .insert(worktree.to_path_buf(), clients.clone());
    let pending = PENDING.lock().unwrap().remove(worktree);
    let Some((hook, slot)) = pending else { return };
    let cwd = worktree.to_path_buf();
    tokio::spawn(async move {
        let outcome = if approve(&clients, &cwd, &hook).await {
            let run_hook = hook.clone();
            let run_cwd = cwd.clone();
            tokio::task::spawn_blocking(move || run(&run_hook, &run_cwd, WAIT_LIMIT))
                .await
                .ok()
        } else {
            None
        };
        let outcome = outcome.unwrap_or_else(|| Outcome {
            name: hook.name.clone(),
            command: hook.command.clone(),
            ok: true,
            output: "Skipped: repository action was not approved.".into(),
        });
        crate::dbg_log!(
            "[hooks] {} in {}: {}",
            hook.name,
            cwd.display(),
            if outcome.ok { "ok" } else { "failed" }
        );
        slot.send_replace(Some(outcome));
    });
}

/// Hold a turn of `session_id` until its worktree's waiting create hook has
/// exited. Answers what the agent should be told first when it failed, once:
/// the next turns are not held or told again.
pub(crate) async fn wait_setup(session_id: &str) -> Option<String> {
    let path = crate::worktrees::of_session(session_id)?.path;
    let (wait, slot) = SETUPS.lock().ok()?.get(&path).cloned()?;
    if !wait {
        return None;
    }
    let mut receiver = slot.subscribe();
    let outcome = tokio::time::timeout(
        WAIT_LIMIT * 2 + Duration::from_secs(1),
        receiver.wait_for(Option::is_some),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .and_then(|value| value.clone());
    if let Ok(mut setups) = SETUPS.lock() {
        setups.remove(&path);
    }
    match outcome {
        Some(outcome) if outcome.ok => None,
        Some(outcome) => Some(failure_note(&outcome)),
        None => Some(
            "Note: the worktree's setup action did not finish in time; it may still be running."
                .to_owned(),
        ),
    }
}

/// What the agent reads at the top of its first turn after a failed setup.
pub(crate) fn failure_note(outcome: &Outcome) -> String {
    format!(
        "Note: this worktree's setup action \"{}\" (`{}`) failed. Its output:\n```\n{}\n```\n\n",
        outcome.name, outcome.command, outcome.output
    )
}

// ------------------------------------------------------- the settle hook ----

/// Run the settle hook of `session_id`'s worktree in the background.
pub(crate) fn on_settle(session_id: &str) {
    let Some(worktree) = crate::worktrees::of_session(session_id) else {
        return;
    };
    let Some(hook) = crate::worktrees::hooks_of(&worktree.path).and_then(|hooks| hooks.settle)
    else {
        return;
    };
    let Some(_guard) = SettleGuard::acquire(&worktree.repo) else {
        return;
    };
    let clients = CHANNELS.lock().unwrap().get(&worktree.path).cloned();
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    runtime.spawn(async move {
        let _guard = _guard;
        if hook.repository {
            let Some(clients) = clients else { return };
            if !approve(&clients, &worktree.path, &hook).await {
                return;
            }
        }
        let name = hook.name.clone();
        let path = worktree.path.clone();
        let outcome = tokio::task::spawn_blocking(move || run(&hook, &path, WAIT_LIMIT)).await;
        let Ok(outcome) = outcome else { return };
        crate::dbg_log!(
            "[hooks] settle {} in {}: {}",
            name,
            worktree.path.display(),
            if outcome.ok { "ok" } else { "failed" }
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repository_actions_require_explicit_exact_consent() {
        let dir = tempfile::tempdir().unwrap();
        let approved = dir
            .path()
            .join(consent_key(Path::new("/project"), "", "bun install"));
        assert!(needs_approval(true, Some(&approved)));
        assert!(needs_approval(true, None));
        std::fs::write(&approved, "approved").unwrap();
        assert!(!needs_approval(true, Some(&approved)));
        let changed = dir
            .path()
            .join(consent_key(Path::new("/project"), "", "npm install"));
        assert!(needs_approval(true, Some(&changed)));
        assert!(!needs_approval(false, None));
    }

    #[test]
    fn consent_is_for_exact_command_project_and_fork_scope() {
        let root = Path::new("/project");
        let approved = consent_key(root, "", "bun install");
        assert_eq!(approved, consent_key(root, "", "bun install"));
        assert_ne!(approved, consent_key(root, "", "bun install; echo changed"));
        assert_ne!(
            approved,
            consent_key(Path::new("/other"), "", "bun install")
        );
        assert_ne!(approved, consent_key(root, "/fork-worktree", "bun install"));
    }

    #[test]
    fn settle_guard_serializes_projects_and_releases_on_drop() {
        let (finished, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let dir = tempfile::tempdir().unwrap();
            let guard = SettleGuard::acquire(dir.path()).unwrap();
            assert!(SettleGuard::acquire(dir.path()).is_none());
            // Rejection must leave the first runner's ownership intact.
            assert!(SettleGuard::acquire(dir.path()).is_none());
            let other = tempfile::tempdir().unwrap();
            assert!(SettleGuard::acquire(other.path()).is_some());
            drop(guard);
            assert!(SettleGuard::acquire(dir.path()).is_some());
            finished.send(()).unwrap();
        });
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("settle guard acquisition or release deadlocked");
        worker.join().unwrap();
    }

    #[test]
    fn the_file_names_the_first_hook_of_each_kind() {
        let hooks = parse_file(
            r#"{ "actions": [
                { "name": "Dev", "command": "bun run dev" },
                { "name": "Install", "command": "bun install", "runOnWorktreeCreate": true, "wait": true },
                { "name": "Again", "command": "npm i", "runOnWorktreeCreate": true },
                { "name": "Clean", "command": "rm -rf dist", "runOnSettle": true, "wait": true }
            ] }"#,
        );
        assert_eq!(
            hooks.create,
            Some(Hook {
                name: "Install".into(),
                command: "bun install".into(),
                wait: true,
                repository: true
            })
        );
        // A settle hook never holds anything.
        assert_eq!(
            hooks.settle.map(|hook| (hook.command, hook.wait)),
            Some(("rm -rf dist".into(), false))
        );
        assert!(parse_file("not json").is_empty());
    }

    #[test]
    fn the_file_wins_a_slot_both_fill() {
        let meta = json!({ "rustcode": { "worktree": { "repo": "/r", "hooks": {
            "create": { "name": "App install", "command": "pnpm i", "wait": false },
            "settle": { "name": "App clean", "command": "make clean" }
        } } } });
        let app = from_meta(meta.as_object());
        let file = parse_file(
            r#"{ "actions": [ { "name": "Install", "command": "bun install", "runOnWorktreeCreate": true } ] }"#,
        );
        let merged = merge(file, app);
        assert_eq!(
            merged.create.map(|hook| hook.command),
            Some("bun install".into())
        );
        assert_eq!(
            merged.settle.map(|hook| hook.command),
            Some("make clean".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_reports_its_exit_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let ok = run(
            &Hook {
                name: "ok".into(),
                command: "echo ready > made.txt && echo done".into(),
                wait: true,
                repository: false,
            },
            dir.path(),
            Duration::from_secs(10),
        );
        assert!(ok.ok);
        assert_eq!(ok.output, "done");
        assert!(dir.path().join("made.txt").exists());
        let failed = run(
            &Hook {
                name: "bad".into(),
                command: "echo nope >&2; exit 3".into(),
                wait: true,
                repository: false,
            },
            dir.path(),
            Duration::from_secs(10),
        );
        assert!(!failed.ok);
        assert!(failed.output.contains("nope"));
        assert!(failure_note(&failed).contains("\"bad\""));
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_past_its_limit_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let slow = run(
            &Hook {
                name: "slow".into(),
                command: "sh -c 'sleep 30 & sleep 30'".into(),
                wait: true,
                repository: false,
            },
            dir.path(),
            Duration::from_millis(200),
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!slow.ok);
        assert!(slow.output.contains("stopped after"));
    }
}
