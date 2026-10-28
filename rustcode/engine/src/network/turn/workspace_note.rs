//! A one-time note when a turn works mostly outside the session workspace
//! through per-call `cwd` overrides (#2565). Subagents and tools without a
//! `cwd` still start in the session workspace, so the agent should suggest
//! moving the session instead of silently working elsewhere.

use std::path::{Path, PathBuf};

pub(crate) const OUTSIDE_WORKSPACE_NOTE_PREFIX: &str = "[Workspace note:";
/// Fewer outside calls than this are a quick look elsewhere, not a pattern.
const MIN_OUTSIDE_CALLS: usize = 3;

#[derive(Debug, Default)]
pub(crate) struct OutsideWorkspaceTally {
    calls: usize,
    outside: usize,
    last_outside: Option<PathBuf>,
    noted: bool,
}

impl OutsideWorkspaceTally {
    /// Count one executed call; `arguments` is its tool input.
    pub(crate) fn record(
        &mut self,
        arguments: &serde_json::Value,
        workspace: &Path,
        session_dirs: &[PathBuf],
    ) {
        self.calls += 1;
        if let Some(directory) = outside_cwd(arguments, workspace, session_dirs) {
            self.outside += 1;
            self.last_outside = Some(directory);
        }
    }

    /// The note to add, once per turn, when most calls so far ran outside
    /// the workspace and `history` does not already carry one.
    pub(crate) fn take_note(
        &mut self,
        workspace: &Path,
        history: &[crate::app::ChatMessage],
    ) -> Option<String> {
        if self.noted || self.outside < MIN_OUTSIDE_CALLS || self.outside * 2 <= self.calls {
            return None;
        }
        self.noted = true;
        if history.iter().any(|message| {
            message.role == "system" && message.content.starts_with(OUTSIDE_WORKSPACE_NOTE_PREFIX)
        }) {
            return None;
        }
        let directory = self.last_outside.as_deref()?;
        Some(format!(
            "{OUTSIDE_WORKSPACE_NOTE_PREFIX} most tool calls in this turn run in {} through `cwd`, outside this session's workspace {}. \
Subagents and tools without a `cwd` still start in the session workspace. Tell the user once that the session can be moved to the directory they are working in (RustCode can change a session's workspace); until then pass `cwd` to spawn_agent and git_status.]",
            directory.display(),
            workspace.display()
        ))
    }
}

/// The directory a call's `cwd` names, when it lies outside `workspace` and
/// outside the session's own `sandbox`/`artifacts` directories.
fn outside_cwd(
    arguments: &serde_json::Value,
    workspace: &Path,
    session_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let raw = arguments.get("cwd")?.as_str()?.trim();
    if raw.is_empty() || matches!(raw, "sandbox" | "./sandbox") {
        return None;
    }
    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let resolved_canonical = canonical(&resolved);
    // Compare both spellings: a path that does not exist yet cannot be
    // canonicalized, and the workspace may be reached through a symlink.
    let inside = std::iter::once(workspace)
        .chain(session_dirs.iter().map(PathBuf::as_path))
        .any(|root| resolved.starts_with(root) || resolved_canonical.starts_with(canonical(root)));
    (!inside).then_some(resolved_canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ChatMessage;

    #[test]
    fn notes_once_when_most_calls_run_outside_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let sandbox = tempfile::tempdir().unwrap();
        let session_dirs = vec![sandbox.path().to_path_buf()];
        let mut tally = OutsideWorkspaceTally::default();
        let outside = serde_json::json!({"command": "ls", "cwd": elsewhere.path()});
        let inside = serde_json::json!({"command": "ls", "cwd": "src"});
        let scratch = serde_json::json!({"command": "ls", "cwd": sandbox.path()});

        for arguments in [&outside, &outside, &inside, &scratch] {
            tally.record(arguments, workspace.path(), &session_dirs);
        }
        // Two of four calls ran outside: not most, and below the minimum.
        assert_eq!(tally.take_note(workspace.path(), &[]), None);

        tally.record(&outside, workspace.path(), &session_dirs);
        tally.record(&outside, workspace.path(), &session_dirs);
        let note = tally.take_note(workspace.path(), &[]).expect("note");
        assert!(note.starts_with(OUTSIDE_WORKSPACE_NOTE_PREFIX));
        let elsewhere = elsewhere.path().canonicalize().unwrap();
        assert!(note.contains(&elsewhere.display().to_string()), "{note}");
        // Once per turn.
        assert_eq!(tally.take_note(workspace.path(), &[]), None);

        // Once per session: an earlier note in history suppresses another.
        let mut next_turn = OutsideWorkspaceTally::default();
        for _ in 0..3 {
            next_turn.record(&outside, workspace.path(), &session_dirs);
        }
        let history = [ChatMessage::new("system", note)];
        assert_eq!(next_turn.take_note(workspace.path(), &history), None);
    }

    #[test]
    fn calls_without_cwd_or_inside_the_workspace_are_not_outside() {
        let workspace = tempfile::tempdir().unwrap();
        for arguments in [
            serde_json::json!({"command": "ls"}),
            serde_json::json!({"command": "ls", "cwd": ""}),
            serde_json::json!({"command": "ls", "cwd": "sandbox"}),
            serde_json::json!({"command": "ls", "cwd": workspace.path().join("src")}),
        ] {
            assert_eq!(outside_cwd(&arguments, workspace.path(), &[]), None);
        }
    }
}
