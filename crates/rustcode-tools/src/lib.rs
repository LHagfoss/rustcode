//! Dependency-light built-in workspace tools.
//!
//! This crate owns filesystem-oriented tool implementations without depending
//! on RustCode's configuration, UI, network, or agent orchestration layers.
//! Callers provide the workspace/session paths through [`ToolContext`].
//! The higher-level grep/glob implementations remain in the application for
//! now because their ignore/walk policy is coupled to the search orchestration;
//! directory listing is intentionally small enough to share this boundary.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Paths needed to resolve project-relative tool arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolContext {
    pub workspace_root: Option<PathBuf>,
    pub sandbox_dir: Option<PathBuf>,
    pub artifacts_dir: Option<PathBuf>,
}

thread_local! {
    static ACTIVE_CONTEXT: RefCell<ToolContext> = const { RefCell::new(ToolContext {
        workspace_root: None,
        sandbox_dir: None,
        artifacts_dir: None,
    }) };
}

/// Run a legacy handler with an explicit context.
pub fn with_context<T>(context: &ToolContext, f: impl FnOnce() -> T) -> T {
    ACTIVE_CONTEXT.with(|active| {
        let previous = active.replace(context.clone());
        let _restore = ContextGuard { active, previous };
        f()
    })
}

struct ContextGuard<'a> {
    active: &'a RefCell<ToolContext>,
    previous: ToolContext,
}

impl Drop for ContextGuard<'_> {
    fn drop(&mut self) {
        self.active.replace(std::mem::take(&mut self.previous));
    }
}

pub(crate) fn active_context() -> ToolContext {
    ACTIVE_CONTEXT.with(|active| active.borrow().clone())
}

pub(crate) fn resolve_tool_path(raw_path: &str) -> PathBuf {
    resolve_tool_path_with_context(raw_path, &active_context())
}

/// Resolve a path using the same project, sandbox, artifact, and home rules as
/// the original in-process handlers.
pub fn resolve_tool_path_with_context(raw_path: &str, context: &ToolContext) -> PathBuf {
    let path = Path::new(raw_path);
    if !path.is_absolute()
        && let Some(root) = context.workspace_root.as_ref()
    {
        return root.join(path);
    }

    let mut sandbox_parts = Vec::new();
    let mut found_sandbox = false;
    for component in path.components() {
        let name = component.as_os_str();
        if found_sandbox {
            sandbox_parts.push(name);
        } else if name == "sandbox" {
            found_sandbox = true;
        }
    }
    if found_sandbox && let Some(sandbox_dir) = context.sandbox_dir.as_ref() {
        let mut resolved = sandbox_dir.clone();
        for part in sandbox_parts {
            resolved.push(part);
        }
        return resolved;
    }

    let mut artifacts_parts = Vec::new();
    let mut found_artifacts = false;
    for component in path.components() {
        let name = component.as_os_str();
        if found_artifacts {
            artifacts_parts.push(name);
        } else if name == "artifacts" {
            found_artifacts = true;
        }
    }
    if found_artifacts && let Some(artifacts_dir) = context.artifacts_dir.as_ref() {
        let mut resolved = artifacts_dir.clone();
        for part in artifacts_parts {
            resolved.push(part);
        }
        return resolved;
    }

    if (raw_path.starts_with("~/") || raw_path == "~")
        && let Ok(home) = std::env::var("HOME")
    {
        let tail = raw_path.strip_prefix('~').unwrap_or("");
        let tail = tail.strip_prefix('/').unwrap_or(tail);
        return PathBuf::from(home).join(tail);
    }

    PathBuf::from(raw_path)
}

pub(crate) fn parse_json_number(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}

pub(crate) fn coerce_array(value: &serde_json::Value) -> Option<Vec<serde_json::Value>> {
    if let Some(array) = value.as_array() {
        return Some(array.clone());
    }
    value
        .as_str()
        .and_then(|string| serde_json::from_str::<serde_json::Value>(string).ok())
        .and_then(|value| value.as_array().cloned())
}

pub mod filesystem;
pub mod search;

#[cfg(test)]
mod tests {
    use super::{ToolContext, resolve_tool_path_with_context, search, with_context};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn path_resolution_preserves_workspace_and_session_roots() {
        let context = ToolContext {
            workspace_root: Some(PathBuf::from("/workspace")),
            sandbox_dir: Some(PathBuf::from("/session/sandbox")),
            artifacts_dir: Some(PathBuf::from("/session/artifacts")),
        };

        assert_eq!(
            resolve_tool_path_with_context("src/main.rs", &context),
            PathBuf::from("/workspace/src/main.rs")
        );
        assert_eq!(
            resolve_tool_path_with_context("/tmp/sandbox/output.txt", &context),
            PathBuf::from("/session/sandbox/output.txt")
        );
        assert_eq!(
            resolve_tool_path_with_context("/tmp/artifacts/result.txt", &context),
            PathBuf::from("/session/artifacts/result.txt")
        );
    }

    #[test]
    fn directory_listing_uses_the_explicit_workspace_context() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(root.path().join("nested")).expect("nested directory");
        std::fs::write(root.path().join("file.txt"), "content").expect("file");
        let context = ToolContext {
            workspace_root: Some(root.path().to_path_buf()),
            ..ToolContext::default()
        };

        let listing = with_context(&context, || search::list_directory(&json!({"path": "."})))
            .expect("list directory");
        assert_eq!(listing, "file.txt\nnested/");
    }

    #[test]
    fn context_is_restored_when_handler_unwinds() {
        let context = ToolContext {
            workspace_root: Some(PathBuf::from("/temporary")),
            ..ToolContext::default()
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_context(&context, || panic!("intentional test unwind"));
        }));
        assert!(result.is_err());
        assert_eq!(super::active_context(), ToolContext::default());
    }
}
