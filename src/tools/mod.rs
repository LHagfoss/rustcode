use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

mod filesystem;
mod search;
mod exec;
mod misc;

pub use filesystem::*;
pub use search::*;
pub use exec::*;
pub use misc::*;

#[allow(dead_code)]
pub struct BackgroundTaskInfo {
    pub id: String,
    pub command: String,
    pub start_time: Instant,
    pub child_pid: Option<u32>,
    pub cancel_sender: Option<tokio::sync::oneshot::Sender<()>>,
}

pub fn get_background_tasks() -> &'static StdMutex<HashMap<String, BackgroundTaskInfo>> {
    static TASKS: OnceLock<StdMutex<HashMap<String, BackgroundTaskInfo>>> = OnceLock::new();
    TASKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

static WAKEUP_CALLBACK: OnceLock<Box<dyn Fn(String, String, String) + Send + Sync + 'static>> =
    OnceLock::new();

pub fn register_wakeup_callback<F>(cb: F)
where
    F: Fn(String, String, String) + Send + Sync + 'static,
{
    let _ = WAKEUP_CALLBACK.set(Box::new(cb));
}

thread_local! {
    static ACTIVE_SESSION_ID: RefCell<Option<String>> = RefCell::new(None);
}

pub fn set_active_session_id(id: Option<String>) {
    ACTIVE_SESSION_ID.with(|f| {
        *f.borrow_mut() = id;
    });
}

pub fn get_active_session_id() -> Option<String> {
    ACTIVE_SESSION_ID.with(|f| f.borrow().clone())
}

pub(crate) fn resolve_tool_path(raw_path: &str) -> PathBuf {
    let p = Path::new(raw_path);

    // Check if the path contains a component named "sandbox"
    let mut parts_sandbox = Vec::new();
    let mut found_sandbox = false;
    for component in p.components() {
        let name = component.as_os_str();
        if found_sandbox {
            parts_sandbox.push(name);
        } else if name == "sandbox" {
            found_sandbox = true;
        }
    }

    if found_sandbox {
        if let Some(session_id) = get_active_session_id() {
            if let Some(sandbox_dir) = crate::config::get_active_session_sandbox_dir(&session_id) {
                let mut resolved = sandbox_dir;
                for part in parts_sandbox {
                    resolved.push(part);
                }
                return resolved;
            }
        }
    }

    // Check if the path contains a component named "artifacts"
    let mut parts_artifacts = Vec::new();
    let mut found_artifacts = false;
    for component in p.components() {
        let name = component.as_os_str();
        if found_artifacts {
            parts_artifacts.push(name);
        } else if name == "artifacts" {
            found_artifacts = true;
        }
    }

    if found_artifacts {
        if let Some(session_id) = get_active_session_id() {
            if let Some(artifacts_dir) =
                crate::config::get_active_session_artifacts_dir(&session_id)
            {
                let mut resolved = artifacts_dir;
                for part in parts_artifacts {
                    resolved.push(part);
                }
                return resolved;
            }
        }
    }

    PathBuf::from(raw_path)
}

fn parse_json_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(s) = v.as_str() {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

fn parse_json_bool(v: &Value) -> Option<bool> {
    if let Some(b) = v.as_bool() {
        Some(b)
    } else if let Some(s) = v.as_str() {
        match s.to_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,

    pub arguments: &'static str,
    pub handler: fn(&Value) -> Result<String, String>,
    /// If true, the agent loop will pause and show a Y/N confirmation modal
    /// to the user before executing. Use for destructive tools (write, create, run).
    pub requires_confirmation: bool,
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "check_match",
        description: "Check football match data from api-sports.io. \
                     Useful for checking live scores during matches or finding specific games.\
                     Example: check_match with team='Norway' and date='2026-07-11'",
        arguments: "{\\"date\\": \"YYYY-MM-DD format required\\", \\"team\\": \"optional team name filter\\", \\"status\\": \"optional status (LIVE, FT, NS)\"}",
        handler: misc::check_match,
        requires_confirmation: false,
    },
    Tool {
        name: "get_time",
        description: "Get the current local date and time",
        arguments: "{} (no arguments)",
        handler: misc::get_time,
        requires_confirmation: false,
    },
    Tool {
        name: "grep",
        description: "Recursively search file contents with regex. Respects \
                      .gitignore and skips hidden files. Use this to find where \
                      functions, classes, strings, or patterns are defined or used",
        arguments: "{\\"pattern\\": \"regex pattern\\", \
                     \\"path\\": \"optional directory or file (default current dir)\\", \
                     \\"include\\": \"optional file glob filter e.g. '*.rs'\\", \
                     \\"ignore_case\\": optional bool (default false)}",
        handler: search::grep,
        requires_confirmation: false,
    },
    Tool {
        name: "glob",
        description: "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ts'). \
                      Respects .gitignore and skips hidden files. Returns matching \
                      paths, sorted. Use this to discover files by name",
        arguments: "{\\"pattern\\": \"glob pattern\\", \
                     \\"path\\": \"optional root directory (default current dir)\"}",
        handler: search::glob,
        requires_confirmation: false,
    },
    Tool {
        name: "list_directory",
        description: "List files in a directory",
        arguments: "{\\"path\\": \"directory path, defaults to current dir\"}",
        handler: search::list_directory,
        requires_confirmation: false,
    },

    Tool {
        name: "delete_file",
        description: "Delete a file from the filesystem",
        arguments: "{\\"path\\": \"file to delete\"}",
        handler: filesystem::delete_file,
        requires_confirmation: true,
    },
    Tool {
        name: "move_file",
        description: "Move or rename a file or directory to a new path",
        arguments: "{\\"src\\": \"source path\\", \\"dest\\": \"destination path\"}",
        handler: filesystem::move_file,
        requires_confirmation: true,
    },
    Tool {
        name: "copy_file",
        description: "Copy a file to a new path",
        arguments: "{\\"src\\": \"source path to copy\\", \\"dest\\": \"destination path\"}",
        handler: filesystem::copy_file,
        requires_confirmation: true,
    },
    Tool {
        name: "run_command",
        description: "Run a shell command and return its stdout/stderr and exit code. \
                      Supports an optional working directory, environment overrides, and \
                      a timeout (default 120s, killed on expiry). Output is capped. Use \
                      for builds, tests, git, etc.",
        arguments: "{\\"command\\": \"full shell command string\\", \
                     \\"cwd\\": \"optional working directory (default current dir)\\", \
                     \\"timeout_ms\\": \"optional timeout in ms (default 120000)\\", \
                     \\"env\\": \"optional object of extra env vars\"}",
        handler: exec::run_command,
        requires_confirmation: true,
    },
    Tool {
        name: "search_web",
        description: "Performs a web search to look up documentation, API details, or code patterns.",
        arguments: "{\\"query\\": \"search query terms\\", \\"domain\\": \"optional domain filter e.g. 'docs.rs'\"}",
        handler: search::search_web,
        requires_confirmation: false,
    },
    Tool {
        name: "find_symbol",
        description: "Queries the codebase symbol index for matching structures, functions, enums, impls, traits, or modules. Returns definition location and signature.",
        arguments: "{\\"query\\": \"search query string (fuzzy matching on symbol name)\"}",
        handler: search::find_symbol_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "get_project_map",
        description: "Generates a compressed map of all symbols and API signatures in the codebase to understand project structure.",
        arguments: "{}",
        handler: search::get_project_map_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "view_file",
        description: "View the contents of a file. Supports line ranges (1-indexed) and optional byte offset if content is truncated.",
        arguments: "{\\"path\\": \"absolute or relative path to file\\", \
                     \\"start_line\\": \"optional start line number, 1-indexed (default 1)\\", \
                     \\"end_line\\": \"optional end line number, 1-indexed (default start_line + 500)\\", \
                     \\"content_offset\\": \"optional byte offset into content\"}",
        handler: filesystem::view_file_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "replace_file_content",
        description: "Surgically edit a contiguous block of text in an existing file. \
                      Requires specifying the line boundaries, the exact target content, \
                      and the replacement content.",
        arguments: "{\\"path\\": \"absolute or relative path to file\\", \
                     \\"start_line\\": \"1-indexed start line containing target content\\", \
                     \\"end_line\\": \"1-indexed end line containing target content\\", \
                     \\"target_content\\": \"precise block of code to edit (must match file exactly)\\", \
                     \\"replacement_content\\": \"complete replacement text for that block\"}",
        handler: filesystem::replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "multi_replace_file_content",
        description: "Apply multiple non-contiguous edits across a single file in a single tool call. \
                      Specify each edit as a separate replacement chunk.",
        arguments: "{\\"path\\": \"absolute or relative path to file\\", \
                     \\"replacements\\": \"array of objects, each containing: {start_line, end_line, target_content, replacement_content}\"}",
        handler: filesystem::multi_replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "write_to_file",
        description: "Create a new file or overwrite an existing file with complete content. \
                      Creates parent directories automatically.",
        arguments: "{\\"path\\": \"absolute or relative path to file\\", \
                     \\"content\\": \"entire contents to write\\", \
                     \\"overwrite\\": \"set true to allow overwriting an existing file (default false)\"}",
        handler: filesystem::write_to_file_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "complete_task",
        description: "Mark the continuous goal/task as successfully complete.",
        arguments: "{\\"result\\": \"summary of what was achieved and final results\"}",
        handler: misc::complete_task_tool,
        requires_confirmation: false,
    },
];

pub const MAX_TOOL_ROUNDS: usize = 60;
