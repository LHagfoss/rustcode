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

use std::collections::HashMap;
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
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut text);
        }
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = std::io::Read::read_to_end(pipe, &mut text);
        }
        text
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    let mut output = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    output.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
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
    std::thread::spawn(move || {
        let outcome = run(&hook, &cwd, WAIT_LIMIT);
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
    let outcome = tokio::time::timeout(WAIT_LIMIT, receiver.wait_for(Option::is_some))
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
    std::thread::spawn(move || {
        let outcome = run(&hook, &worktree.path, WAIT_LIMIT);
        crate::dbg_log!(
            "[hooks] settle {} in {}: {}",
            hook.name,
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
                wait: true
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
        let slow = run(
            &Hook {
                name: "slow".into(),
                command: "sleep 5".into(),
                wait: true,
            },
            dir.path(),
            Duration::from_millis(200),
        );
        assert!(!slow.ok);
        assert!(slow.output.contains("stopped after"));
    }
}
