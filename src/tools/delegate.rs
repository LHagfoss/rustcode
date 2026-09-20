use serde_json::Value;
use std::path::PathBuf;

use super::{Tool, ToolCapability, ToolSafety};

const SUPPORTED_AGENTS: &[&str] = &["claude", "gemini", "codex", "opencode", "rustcode"];
const MAX_TASK_CHARS: usize = 8_000;

fn delegate_task_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "task": { "type": "string", "description": "Self-contained task for the external agent" },
            "agent": { "type": "string", "description": "External CLI: claude, gemini, codex, opencode, rustcode (default: claude)" }
        },
        "required": ["task"]
    })
}

pub const DELEGATE_TASK: Tool = Tool {
    name: "delegate_task",
    description: "Delegate a self-contained task to an external coding CLI (claude/gemini/codex/opencode/rustcode). Writes the task to .tasks/ and invokes the CLI when installed; otherwise returns the task file for a manual handoff.",
    arguments: r#"{"task": "self-contained task text", "agent": "optional claude|gemini|codex|opencode|rustcode"}"#,
    handler: delegate_task,
    requires_confirmation: true,
    schema: delegate_task_schema,
    capabilities: &[ToolCapability::AgentDelegation],
    safety: ToolSafety::Delegation,
};

pub(crate) fn slugify(text: &str) -> String {
    let mut slug: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    let mut end = 40.min(slug.len());
    while !slug.is_char_boundary(end) {
        end -= 1;
    }
    slug[..end].trim_matches('-').to_string()
}

pub(crate) fn find_agent_binary(agent: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(agent);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn tasks_dir() -> PathBuf {
    super::active_task_working_directory()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tasks")
}

pub fn delegate_task(args: &Value) -> Result<String, String> {
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or("missing 'task'")?;
    if task.is_empty() {
        return Err("task must not be empty".to_string());
    }
    if task.len() > MAX_TASK_CHARS {
        return Err(format!("task is too long (max {MAX_TASK_CHARS} chars)"));
    }
    let agent = args
        .get("agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or("claude");
    if !SUPPORTED_AGENTS.contains(&agent) {
        return Err(format!(
            "unsupported agent '{agent}'. Use one of: {}",
            SUPPORTED_AGENTS.join(", ")
        ));
    }

    let dir = tasks_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create .tasks dir: {e}"))?;
    let slug = slugify(task);
    let filename = format!(
        "{}-{}.md",
        if slug.is_empty() { "task" } else { &slug },
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let path = dir.join(&filename);
    std::fs::write(&path, format!("# Delegated task ({agent})\n\n{task}\n"))
        .map_err(|e| format!("cannot write task file: {e}"))?;

    let Some(binary) = find_agent_binary(agent) else {
        return Ok(format!(
            "Task written to {}. External agent '{agent}' is not installed; hand this file to it manually.",
            path.display()
        ));
    };
    let output = std::process::Command::new(&binary)
        .arg("-p")
        .arg(task)
        .current_dir(path.parent().unwrap_or(std::path::Path::new(".")))
        .output()
        .map_err(|e| format!("failed to invoke {agent}: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = format!("Task file: {}\n", path.display());
    if !stdout.trim().is_empty() {
        combined.push_str(&stdout);
        if combined.len() > MAX_TASK_CHARS {
            combined.truncate(MAX_TASK_CHARS);
            combined.push_str("\n[output truncated]");
        }
    }
    if !output.status.success() && !stderr.trim().is_empty() {
        combined.push_str(&format!("\n[{agent} stderr]: {stderr}"));
    }
    Ok(combined.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_task_and_unknown_agent() {
        assert!(delegate_task(&serde_json::json!({})).is_err());
        assert!(delegate_task(&serde_json::json!({"task": "  "})).is_err());
        assert!(
            delegate_task(&serde_json::json!({"task": "do it", "agent": "clippy"}))
                .unwrap_err()
                .contains("unsupported agent")
        );
    }

    #[test]
    fn slugify_is_filesafe_and_bounded() {
        assert_eq!(
            slugify("Fix the Bug in src/main.rs!"),
            "fix-the-bug-in-src-main-rs"
        );
        assert!(slugify(&"x".repeat(200)).len() <= 40);
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn missing_binary_returns_none_without_touching_fs() {
        assert!(find_agent_binary("rustcode-definitely-missing-cli-xyz").is_none());
    }
}
