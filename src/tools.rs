use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
        name: "get_time",
        description: "Get the current local date and time",
        arguments: "{} (no arguments)",
        handler: get_time,
        requires_confirmation: false,
    },
    Tool {
        name: "grep",
        description: "Recursively search file contents with regex. Respects \
                      .gitignore and skips hidden files. Use this to find where \
                      functions, classes, strings, or patterns are defined or used",
        arguments: "{\"pattern\": \"regex pattern\", \
                     \"path\": \"optional directory or file (default current dir)\", \
                     \"include\": \"optional file glob filter e.g. '*.rs'\", \
                     \"ignore_case\": optional bool (default false)}",
        handler: grep,
        requires_confirmation: false,
    },
    Tool {
        name: "glob",
        description: "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ts'). \
                      Respects .gitignore and skips hidden files. Returns matching \
                      paths, sorted. Use this to discover files by name",
        arguments: "{\"pattern\": \"glob pattern\", \
                     \"path\": \"optional root directory (default current dir)\"}",
        handler: glob,
        requires_confirmation: false,
    },
    Tool {
        name: "list_directory",
        description: "List files in a directory",
        arguments: "{\"path\": \"directory path, defaults to current dir\"}",
        handler: list_directory,
        requires_confirmation: false,
    },
    Tool {
        name: "read_file",
        description: "Read a small chunk of a file, or search inside it. \
                      Never returns whole files: page with start_line, or \
                      use search to find the relevant lines first",
        arguments: "{\"path\": \"file to read\", \
                     \"start_line\": optional number to page from (default 1), \
                     \"search\": \"optional text - returns only matching lines\"}",
        handler: read_file,
        requires_confirmation: false,
    },
    Tool {
        name: "create_file",
        description: "Create a new file with content. Fails if the file already \
                      exists — use write_file to overwrite. Creates parent \
                      directories automatically",
        arguments: "{\"path\": \"file to create\", \"content\": \"file content\"}",
        handler: create_file,
        requires_confirmation: true,
    },
    Tool {
        name: "write_file",
        description: "Overwrite an existing file with new content. Fails if the \
                      file does not exist — use create_file for new files",
        arguments: "{\"path\": \"file to overwrite\", \"content\": \"new content\"}",
        handler: write_file,
        requires_confirmation: true,
    },
    Tool {
        name: "edit",
        description: "Surgically replace text in an existing file. Finds `old_string` \
                      and replaces it with `new_string`. Fails if old_string is not \
                      found, or appears more than once (use `replace_all: true` to \
                      replace every occurrence). Read the file first to get exact text. \
                      Prefer this over write_file for small changes to large files",
        arguments: "{\"path\": \"file to edit\", \
                     \"old_string\": \"exact text to find (include surrounding context for uniqueness)\", \
                     \"new_string\": \"text to replace it with\", \
                     \"replace_all\": \"optional bool (default false)\"}",
        handler: edit,
        requires_confirmation: true,
    },
    Tool {
        name: "delete_file",
        description: "Delete a file from the filesystem",
        arguments: "{\"path\": \"file to delete\"}",
        handler: delete_file,
        requires_confirmation: true,
    },
    Tool {
        name: "move_file",
        description: "Move or rename a file or directory to a new path",
        arguments: "{\"src\": \"source path\", \"dest\": \"destination path\"}",
        handler: move_file,
        requires_confirmation: true,
    },
    Tool {
        name: "copy_file",
        description: "Copy a file to a new path",
        arguments: "{\"src\": \"source path to copy\", \"dest\": \"destination path\"}",
        handler: copy_file,
        requires_confirmation: true,
    },
    Tool {
        name: "run_command",
        description: "Run a shell command and return its stdout/stderr and exit code. \
                      Supports an optional working directory, environment overrides, and \
                      a timeout (default 120s, killed on expiry). Output is capped. Use \
                      for builds, tests, git, etc.",
        arguments: "{\"command\": \"full shell command string\", \
                     \"cwd\": \"optional working directory (default current dir)\", \
                     \"timeout_ms\": \"optional timeout in ms (default 120000)\", \
                     \"env\": \"optional object of extra env vars\"}",
        handler: run_command,
        requires_confirmation: true,
    },
];

/// Maximum tool-call rounds per user prompt, so a confused model
/// can't loop forever.
pub const MAX_TOOL_ROUNDS: usize = 25;

fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
}

const MAX_GREP_LINES: usize = 200;
const MAX_GREP_FILES: usize = 50;
const MAX_GLOB_RESULTS: usize = 200;

fn build_include_matcher(include: Option<&str>) -> Result<Option<globset::GlobSet>, String> {
    let Some(glob_str) = include else {
        return Ok(None);
    };
    let glob = Glob::new(glob_str).map_err(|e| format!("invalid 'include' glob '{glob_str}': {e}"))?;
    let mut b = GlobSetBuilder::new();
    b.add(glob);
    Ok(Some(b.build().map_err(|e| format!("globset build failed: {e}"))?))
}

fn grep(args: &Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|p| p.as_str())
        .ok_or("missing 'pattern' argument")?;
    let root = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let include = args.get("include").and_then(|p| p.as_str());
    let ignore_case = args
        .get("ignore_case")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    let mut re_builder = regex::RegexBuilder::new(pattern);
    re_builder.case_insensitive(ignore_case);
    let re = re_builder
        .build()
        .map_err(|e| format!("invalid regex '{pattern}': {e}"))?;

    let include_set = build_include_matcher(include)?;

    let root_path = Path::new(root);
    if root_path.is_file() {
        let rel = root_path;
        return grep_one_file(root, rel, &re, MAX_GREP_LINES);
    }
    if !root_path.is_dir() {
        return Err(format!("'{root}' is not a file or directory"));
    }

    let walker = WalkBuilder::new(root_path)
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .build();

    let mut out = String::new();
    let mut total_lines = 0usize;
    let mut files_hit = 0usize;

    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root_path).unwrap_or(path);
        if let Some(ref set) = include_set {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if !set.is_match(rel_str.as_str()) && !set.is_match(path.to_string_lossy().as_ref()) {
                continue;
            }
        }

        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut file_lines = 0usize;
        let mut wrote_header = false;
        for (i, line) in content.lines().enumerate() {
            if re.is_match(line) {
                if !wrote_header {
                    files_hit += 1;
                    if files_hit > MAX_GREP_FILES {
                        break;
                    }
                    out.push_str(&format!("\n{}:\n", path.display()));
                    wrote_header = true;
                }
                out.push_str(&format!(
                    "  {}: {}\n",
                    i + 1,
                    truncate_line(line)
                ));
                file_lines += 1;
                total_lines += 1;
                if total_lines >= MAX_GREP_LINES {
                    out.push_str(&format!(
                        "\n(truncated — {} matching lines across {} files, stopping at cap of {MAX_GREP_LINES} lines / {MAX_GREP_FILES} files; narrow 'pattern' or 'include')\n",
                        total_lines, files_hit
                    ));
                    return Ok(out.trim_end().to_string());
                }
            }
        }
        let _ = file_lines;
    }

    if out.is_empty() {
        Ok(format!("no matches for '{pattern}' under '{root}'"))
    } else {
        Ok(format!(
            "matches for '{pattern}' under '{root}' ({} file(s)):\n{}",
            files_hit,
            out.trim_end()
        ))
    }
}

fn grep_one_file(path_str: &str, path: &Path, re: &Regex, max_lines: usize) -> Result<String, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read '{path_str}': {e}"))?;
    let mut out = String::new();
    let mut hits = 0usize;
    for (i, line) in content.lines().enumerate() {
        if re.is_match(line) {
            hits += 1;
            if hits == 1 {
                out.push_str(&format!("{path_str}:\n"));
            }
            out.push_str(&format!("  {}: {}\n", i + 1, truncate_line(line)));
            if hits >= max_lines {
                out.push_str(&format!(
                    "(truncated at {max_lines} matching lines)\n"
                ));
                break;
            }
        }
    }
    if hits == 0 {
        Ok(format!("no matches for '{}' in '{path_str}'", re.as_str()))
    } else {
        Ok(out.trim_end().to_string())
    }
}

fn glob(args: &Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|p| p.as_str())
        .ok_or("missing 'pattern' argument")?;
    let root = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return Err(format!("'{root}' is not a directory"));
    }

    let glob = Glob::new(pattern)
        .map_err(|e| format!("invalid glob '{pattern}': {e}"))?;
    let mut b = GlobSetBuilder::new();
    b.add(glob);
    let set = b
        .build()
        .map_err(|e| format!("globset build failed: {e}"))?;

    let walker = WalkBuilder::new(root_path)
        .hidden(true)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .build();

    let mut matched: Vec<String> = Vec::new();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root_path)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
        if set.is_match(rel.as_str()) || set.is_match(path.to_string_lossy().as_ref()) {
            matched.push(path.to_string_lossy().to_string());
            if matched.len() >= MAX_GLOB_RESULTS {
                break;
            }
        }
    }

    if matched.is_empty() {
        Ok(format!("no files matched '{pattern}' under '{root}'"))
    } else {
        matched.sort();
        let mut out = format!(
            "{} file(s) matched '{pattern}' under '{root}':\n",
            matched.len()
        );
        out.push_str(&matched.join("\n"));
        if matched.len() >= MAX_GLOB_RESULTS {
            out.push_str(&format!("\n(truncated at {MAX_GLOB_RESULTS} results)"));
        }
        Ok(out)
    }
}

fn list_directory(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");

    if std::path::Path::new(path).is_file() {
        return Err(format!(
            "'{path}' is a file, not a directory - use the read_file tool instead"
        ));
    }
    let entries = std::fs::read_dir(path).map_err(|e| format!("cannot read '{path}': {e}"))?;
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| {
            let mut name = e.file_name().to_string_lossy().to_string();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                name.push('/');
            }
            name
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return Ok(format!("'{path}' is empty"));
    }
    let total = names.len();
    if total > MAX_LIST_ENTRIES {
        let mut out = names[..MAX_LIST_ENTRIES].join("\n");
        out.push_str(&format!(
            "\n... ({} more entries, total {total} — use grep/glob to narrow)",
            total - MAX_LIST_ENTRIES
        ));
        Ok(out)
    } else {
        Ok(names.join("\n"))
    }
}

const MAX_READ_LINES: usize = 40;
const MAX_SEARCH_HITS: usize = 8;
const SEARCH_CONTEXT_LINES: usize = 2;
const MAX_LINE_CHARS: usize = 160;

fn truncate_line(line: &str) -> String {
    if line.chars().count() > MAX_LINE_CHARS {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

fn read_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read '{path}': {e}"))?;
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    if let Some(query) = args.get("search").and_then(|s| s.as_str()) {
        let needle = query.to_lowercase();
        let hit_indices: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        if hit_indices.is_empty() {
            return Ok(format!("{path} ({total} lines): no matches for '{query}'"));
        }
        let shown = hit_indices.len().min(MAX_SEARCH_HITS);
        let mut out = format!(
            "{path} ({total} lines): {} match(es) for '{query}'",
            hit_indices.len()
        );
        if hit_indices.len() > shown {
            out.push_str(&format!(", showing first {shown}"));
        }
        out.push('\n');

        let mut printed_up_to = 0usize;
        for &idx in &hit_indices[..shown] {
            let end = (idx + SEARCH_CONTEXT_LINES).min(total - 1);
            for i in idx..=end {
                if i + 1 > printed_up_to {
                    out.push_str(&format!("{}: {}\n", i + 1, truncate_line(lines[i])));
                    printed_up_to = i + 1;
                }
            }
        }
        return Ok(out.trim_end().to_string());
    }

    let start = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
        .max(1) as usize;
    if start > total {
        return Ok(format!("{path} has only {total} lines (asked for {start})"));
    }
    let end = (start + MAX_READ_LINES - 1).min(total);
    let mut out = format!("{path}: lines {start}-{end} of {total}\n");
    for (i, line) in lines[start - 1..end].iter().enumerate() {
        out.push_str(&format!("{}: {}\n", start + i, truncate_line(line)));
    }
    if end < total {
        out.push_str(&format!(
            "(truncated - call read_file again with \"start_line\": {} for more)",
            end + 1
        ));
    }
    Ok(out)
}

fn create_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(|c| c.as_str())
        .ok_or("missing 'content' argument")?;

    let p = std::path::Path::new(path);
    if p.exists() {
        return Err(format!(
            "'{path}' already exists — use write_file to overwrite it"
        ));
    }

    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{path}': {e}"))?;
    }
    std::fs::write(path, content).map_err(|e| format!("cannot create '{path}': {e}"))?;
    let lines = content.lines().count().max(1);
    Ok(format!(
        "created '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}

fn write_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(|c| c.as_str())
        .ok_or("missing 'content' argument")?;

    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!(
            "'{path}' does not exist — use create_file for new files"
        ));
    }
    if p.is_dir() {
        return Err(format!("'{path}' is a directory, not a file"));
    }
    std::fs::write(path, content).map_err(|e| format!("cannot write '{path}': {e}"))?;
    let lines = content.lines().count().max(1);
    Ok(format!(
        "wrote '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}

fn edit(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let old_string = args
        .get("old_string")
        .and_then(|s| s.as_str())
        .ok_or("missing 'old_string' argument")?;
    let new_string = args
        .get("new_string")
        .and_then(|s| s.as_str())
        .ok_or("missing 'new_string' argument")?;
    let replace_all = args
        .get("replace_all")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);

    if old_string.is_empty() {
        return Err("'old_string' must not be empty".to_string());
    }
    if old_string == new_string {
        return Err("'old_string' and 'new_string' are identical — nothing to do".to_string());
    }

    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    if p.is_dir() {
        return Err(format!("'{path}' is a directory, not a file"));
    }
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read '{path}': {e}"))?;

    let count = content.matches(old_string).count();
    if count == 0 {
        return Err(format!(
            "'old_string' not found in '{path}'. Make sure it matches the file exactly (whitespace, indentation, quotes). Use read_file to get the exact text."
        ));
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "'old_string' appears {count} times in '{path}'. Add more surrounding context to make it unique, or set \"replace_all\": true."
        ));
    }

    let new_content = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };

    std::fs::write(path, &new_content).map_err(|e| format!("cannot write '{path}': {e}"))?;

    let changed = if replace_all { count } else { 1 };
    Ok(format!(
        "edited '{path}' ({} replacement{}, {} -> {} bytes)",
        changed,
        if changed == 1 { "" } else { "s" },
        content.len(),
        new_content.len()
    ))
}

fn delete_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    if p.is_dir() {
        return Err(format!(
            "'{path}' is a directory — use delete_dir if needed (not supported yet)"
        ));
    }
    std::fs::remove_file(p).map_err(|e| format!("cannot delete '{path}': {e}"))?;
    Ok(format!("deleted '{path}'"))
}

fn move_file(args: &Value) -> Result<String, String> {
    let src = args
        .get("src")
        .and_then(|s| s.as_str())
        .ok_or("missing 'src' argument")?;
    let dest = args
        .get("dest")
        .and_then(|d| d.as_str())
        .ok_or("missing 'dest' argument")?;
    let src_path = std::path::Path::new(src);
    if !src_path.exists() {
        return Err(format!("source '{src}' does not exist"));
    }
    if let Some(parent) = std::path::Path::new(dest).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{dest}': {e}"))?;
    }
    std::fs::rename(src, dest).map_err(|e| format!("cannot move '{src}' to '{dest}': {e}"))?;
    Ok(format!("moved '{src}' to '{dest}'"))
}

fn copy_file(args: &Value) -> Result<String, String> {
    let src = args
        .get("src")
        .and_then(|s| s.as_str())
        .ok_or("missing 'src' argument")?;
    let dest = args
        .get("dest")
        .and_then(|d| d.as_str())
        .ok_or("missing 'dest' argument")?;
    let src_path = std::path::Path::new(src);
    if !src_path.exists() {
        return Err(format!("source '{src}' does not exist"));
    }
    if src_path.is_dir() {
        return Err(format!(
            "source '{src}' is a directory — copy_file only supports copying files"
        ));
    }
    if let Some(parent) = std::path::Path::new(dest).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{dest}': {e}"))?;
    }
    std::fs::copy(src, dest).map_err(|e| format!("cannot copy '{src}' to '{dest}': {e}"))?;
    Ok(format!("copied '{src}' to '{dest}'"))
}

/// Agent-orchestration tools handled by the orchestrator itself (async,
/// need app state), not by the sync handlers in TOOLS. Only offered to the
/// main agent — subagents cannot spawn or message other agents.
pub fn is_agent_tool(name: &str) -> bool {
    matches!(name, "spawn_agent" | "send_agent")
}

pub fn tool_system_prompt(include_agent_tools: bool) -> String {
    let mut p = String::new();

    p.push_str(
        "You are rustcode, an interactive coding agent running in a terminal. \
You help with software engineering tasks: reading code, finding symbols, \
editing files, running commands, and explaining code.\n\n\
# How to work\n\
- Be concise and direct. No filler, no preamble.\n\
- Explore before editing. Use `grep` to search code, `glob` to find files by \
name, and `read_file` to read relevant sections. Understand the surrounding \
code and project conventions before changing anything.\n\
- Prefer `read_file` with `start_line` paging over dumping whole files. \
Read just what you need.\n\
- Mirror existing style. Use the same libraries, naming, and patterns as the \
surrounding code.\n\
- After meaningful edits, tell the user how to verify (run tests, lint, \
typecheck, build). Do NOT run those yourself unless asked.\n\
- Do NOT commit or push unless the user explicitly asks.\n\
- Destructive or state-changing tools (create, write, edit, delete, move, \
copy, run_command) will prompt the user for confirmation before running. \
Read-only tools (grep, glob, list_directory, read_file) run immediately.\n\n"
    );

    p.push_str(
        "# Tools\n\
You have access to tools. To use one, output a tool call block OUTSIDE of any \
<think> tags, after the thinking tags close. The block must be a fenced code \
block with the `tool` language:\n\n\
```tool\n\
{\"name\": \"tool_name\", \"arguments\": {}}\n\
```\n\n\
After the call, the tool result is sent back to you inside a \
<tool_result> block, and you continue. Emit ONE tool call per turn, then wait \
for the result. Do not narrate the JSON — just emit the block.\n\n\
Only call a tool when the task actually requires information or changes from \
the filesystem, codebase, or shell. Greetings, chit-chat, and questions you \
can answer from the conversation get a plain text reply with NO tool call.\n\n\
Available tools:\n",
    );
    for t in TOOLS {
        p.push_str(&format!(
            "- {}: {}. Arguments: {}\n",
            t.name, t.description, t.arguments
        ));
    }
    if include_agent_tools {
        p.push_str(
            "- spawn_agent: Delegate a self-contained task to a fresh subagent and get \
its final answer back. The subagent has the same tools as you (except agent tools) \
but starts with NO context — put everything it needs in the task description. Use it \
for research, multi-file searches, or isolated subtasks to keep your own context \
small. Arguments: {\"task\": \"full task description with all needed context\"}\n\
- send_agent: Send a follow-up message to a subagent you spawned earlier; it keeps \
its own conversation memory and replies. \
Arguments: {\"id\": subagent id number, \"message\": \"follow-up message\"}\n",
        );
    }
    p.push_str(
        "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant: I'll search the codebase for the agent loop.\n\
```tool\n\
{\"name\": \"grep\", \"arguments\": {\"pattern\": \"agent loop\", \"include\": \"*.rs\"}}\n\
```\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
    );
    p
}

fn extract_tool_call(json: &Value) -> Option<(String, Value)> {
    let name = json.get("name")?.as_str()?.to_string();
    let args = json
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    Some((name, args))
}

pub fn parse_tool_call(text: &str) -> Option<(String, Value)> {
    let search_start = text.find("</think>").map(|idx| idx + "</think>".len()).unwrap_or(0);
    let search_text = &text[search_start..];

    for marker in &["```tool", "```json"] {
        if let Some(start_code_idx) = search_text.find(marker) {
            let start = start_code_idx + marker.len();
            if let Some(end_code_offset) = search_text[start..].find("```") {
                let end = start + end_code_offset;
                if let Ok(json) = serde_json::from_str::<Value>(search_text[start..end].trim()) {
                    if let Some(res) = extract_tool_call(&json) {
                        return Some(res);
                    }
                }
            }
        }
    }

    if let Some(start_tag_idx) = text.find("<tool_call>") {
        let start = start_tag_idx + "<tool_call>".len();
        if let Some(end_tag_offset) = text[start..].find("</tool_call>") {
            let end = start + end_tag_offset;
            if let Ok(json) = serde_json::from_str::<Value>(text[start..end].trim()) {
                if let Some(res) = extract_tool_call(&json) {
                    return Some(res);
                }
            }
        }
    }

    if let Some(first_brace) = search_text.find('{') {
        if let Some(last_brace) = search_text.rfind('}') {
            if last_brace > first_brace {
                if let Ok(json) = serde_json::from_str::<Value>(search_text[first_brace..=last_brace].trim()) {
                    if let Some(res) = extract_tool_call(&json) {
                        return Some(res);
                    }
                }
            }
        }
    }

    None
}

pub fn execute(name: &str, args: &Value) -> String {
    match TOOLS.iter().find(|t| t.name == name) {
        Some(tool) => match (tool.handler)(args) {
            Ok(out) => out,
            Err(e) => format!("error: {e}"),
        },
        None => format!(
            "error: unknown tool '{name}'. Available: {}",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        ),
    }
}

pub fn needs_confirmation(name: &str) -> bool {
    TOOLS
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.requires_confirmation)
        .unwrap_or(false)
}

const MAX_COMMAND_OUTPUT_BYTES: usize = 20_000;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 120_000;
const MAX_LIST_ENTRIES: usize = 200;

fn run_command(args: &Value) -> Result<String, String> {
    let command_str = args
        .get("command")
        .and_then(|c| c.as_str())
        .ok_or("missing 'command' argument")?;
    let cwd = args.get("cwd").and_then(|c| c.as_str());
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS);
    let env = args.get("env").and_then(|e| e.as_object());

    if let Some(cwd) = cwd {
        let p = Path::new(cwd);
        if !p.is_dir() {
            return Err(format!("cwd '{cwd}' is not a directory"));
        }
    }

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command_str]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command_str]);
        c
    };

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    if let Some(env_map) = env {
        for (k, v) in env_map {
            if let Some(val) = v.as_str() {
                cmd.env(k, val);
            }
        }
    }

    let output = run_with_timeout(
        cmd,
        Duration::from_millis(timeout_ms.max(1)),
    )?;

    let mut result = String::new();
    result.push_str(&format!("exit code: {}\n", output.status.code().unwrap_or(-1)));

    let stdout = truncate_bytes(&output.stdout, MAX_COMMAND_OUTPUT_BYTES);
    let stderr = truncate_bytes(&output.stderr, MAX_COMMAND_OUTPUT_BYTES);

    if !stdout.is_empty() {
        result.push_str("stdout:\n");
        result.push_str(&stdout);
        if !stdout.ends_with('\n') {
            result.push('\n');
        }
    }
    if !stderr.is_empty() {
        result.push_str("stderr:\n");
        result.push_str(&stderr);
        if !stderr.ends_with('\n') {
            result.push('\n');
        }
    }
    if stdout.is_empty() && stderr.is_empty() {
        result.push_str("(no output)\n");
    }
    Ok(result.trim_end().to_string())
}

fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> Result<Output, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn process: {e}"))?;
    let mut child_stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut child_stderr = child.stderr.take().ok_or("no stderr pipe")?;

    let out_handle = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = child_stdout.read_to_end(&mut b);
        b
    });
    let err_handle = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = child_stderr.read_to_end(&mut b);
        b
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "command timed out after {} ms and was killed",
                        timeout.as_millis()
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("failed to wait on process: {e}")),
        }
    };

    let stdout_bytes = out_handle.join().unwrap_or_default();
    let stderr_bytes = err_handle.join().unwrap_or_default();

    Ok(Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    })
}

fn truncate_bytes(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).to_string();
    }
    let head_end = (max * 3 / 4).min(bytes.len());
    let head = &bytes[..head_end];
    let mut out = String::from_utf8_lossy(head).to_string();
    out.push_str(&format!(
        "\n... (truncated, {} bytes total — showing first {} bytes)\n",
        bytes.len(),
        head_end
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_call() {
        let text = "<tool_call>{\"name\": \"get_time\", \"arguments\": {}}</tool_call>";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "get_time");
        assert!(args.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_parse_tool_call_with_surrounding_text() {
        let text = "Sure!\n<tool_call>{\"name\": \"list_directory\", \"arguments\": {\"path\": \"/tmp\"}}</tool_call>";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "list_directory");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "/tmp");
    }

    #[test]
    fn test_parse_tool_call_fallbacks() {
        let text = "<think>some thought</think>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"README.md\"}}";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "read_file");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "README.md");

        let text = "Use this tool:\n```json\n{\"name\": \"delete_file\", \"arguments\": {\"path\": \"/tmp/test\"}}\n```";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "delete_file");
        assert_eq!(args.get("path").unwrap().as_str().unwrap(), "/tmp/test");
    }

    #[test]
    fn test_parse_rejects_plain_text() {
        assert!(parse_tool_call("just a normal reply").is_none());
        assert!(parse_tool_call("<tool_call>not json</tool_call>").is_none());
        assert!(parse_tool_call("<think>{\"name\": \"get_time\"}</think>").is_none());
    }

    #[test]
    fn test_execute_get_time() {
        let out = execute("get_time", &serde_json::json!({}));
        assert!(!out.starts_with("error:"));
    }

    #[test]
    fn test_execute_unknown_tool() {
        let out = execute("nope", &serde_json::json!({}));
        assert!(out.contains("unknown tool"));
    }

    fn temp_file(name: &str, lines: usize) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("rustcode-tool-{name}-{}", std::process::id()));
        let body: String = (1..=lines).map(|i| format!("line number {i}\n")).collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn test_read_file_chunks_and_paginates() {
        let path = temp_file("page", 100);
        let out = execute("read_file", &serde_json::json!({"path": path}));
        assert!(out.contains("lines 1-40 of 100"));
        assert!(out.contains("start_line"));
        assert!(!out.contains("line number 41"));

        let out = execute(
            "read_file",
            &serde_json::json!({"path": path, "start_line": 90}),
        );
        assert!(out.contains("lines 90-100 of 100"));
        assert!(!out.contains("truncated"));
    }

    #[test]
    fn test_read_file_search() {
        let path = temp_file("search", 100);
        let out = execute(
            "read_file",
            &serde_json::json!({"path": path, "search": "number 42"}),
        );
        assert!(out.contains("1 match(es)"));
        assert!(out.contains("42: line number 42"));

        let out = execute(
            "read_file",
            &serde_json::json!({"path": path, "search": "zzz"}),
        );
        assert!(out.contains("no matches"));
    }

    #[test]
    fn test_read_file_missing_args() {
        let out = execute("read_file", &serde_json::json!({}));
        assert!(out.contains("missing 'path'"));
        let out = execute(
            "read_file",
            &serde_json::json!({"path": "/nope/nothing.txt"}),
        );
        assert!(out.contains("cannot read"));
    }

    #[test]
    fn test_system_prompt_lists_tools() {
        let p = tool_system_prompt(true);
        for t in TOOLS {
            assert!(p.contains(t.name));
        }
        assert!(p.contains("spawn_agent"));
        assert!(p.contains("send_agent"));
    }

    #[test]
    fn test_system_prompt_without_agent_tools() {
        let p = tool_system_prompt(false);
        assert!(!p.contains("spawn_agent"));
        assert!(!p.contains("send_agent"));
    }

    #[test]
    fn test_is_agent_tool() {
        assert!(is_agent_tool("spawn_agent"));
        assert!(is_agent_tool("send_agent"));
        assert!(!is_agent_tool("grep"));
        assert!(!is_agent_tool("run_command"));
    }

    #[test]
    fn test_create_file_basic() {
        let dir = std::env::temp_dir().join(format!("rustcode-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("hello.txt");
        let out = execute(
            "create_file",
            &serde_json::json!({"path": path.to_str().unwrap(), "content": "hello world\n"}),
        );
        assert!(out.contains("created"), "got: {out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world\n");

        let out = execute(
            "create_file",
            &serde_json::json!({"path": path.to_str().unwrap(), "content": "nope"}),
        );
        assert!(out.contains("already exists"), "got: {out}");
        assert!(out.contains("write_file"), "cross-tool hint missing: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_file_basic() {
        let path = std::env::temp_dir().join(format!("rustcode-write-{}", std::process::id()));
        std::fs::write(&path, "old content").unwrap();
        let out = execute(
            "write_file",
            &serde_json::json!({"path": path.to_str().unwrap(), "content": "new content\nline 2\n"}),
        );
        assert!(out.contains("wrote"), "got: {out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "new content\nline 2\n"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_write_file_nonexistent() {
        let out = execute(
            "write_file",
            &serde_json::json!({"path": "/tmp/rustcode-nope-12345.txt", "content": "x"}),
        );
        assert!(out.contains("does not exist"), "got: {out}");
        assert!(
            out.contains("create_file"),
            "cross-tool hint missing: {out}"
        );
    }

    #[test]
    fn test_create_file_missing_args() {
        let out = execute("create_file", &serde_json::json!({}));
        assert!(out.contains("missing 'path'"));
        let out = execute("create_file", &serde_json::json!({"path": "/tmp/x"}));
        assert!(out.contains("missing 'content'"));
    }

    #[test]
    fn test_write_file_missing_args() {
        let out = execute("write_file", &serde_json::json!({}));
        assert!(out.contains("missing 'path'"));
        let out = execute("write_file", &serde_json::json!({"path": "/tmp/x"}));
        assert!(out.contains("missing 'content'"));
    }

    #[test]
    fn test_delete_file() {
        let path = std::env::temp_dir().join(format!("rustcode-delete-{}", std::process::id()));
        std::fs::write(&path, "temp content").unwrap();
        assert!(path.exists());
        let out = execute(
            "delete_file",
            &serde_json::json!({"path": path.to_str().unwrap()}),
        );
        assert!(out.contains("deleted"));
        assert!(!path.exists());
        let out = execute(
            "delete_file",
            &serde_json::json!({"path": path.to_str().unwrap()}),
        );
        assert!(out.contains("does not exist"));
    }

    #[test]
    fn test_move_file() {
        let src = std::env::temp_dir().join(format!("rustcode-move-src-{}", std::process::id()));
        let dest = std::env::temp_dir().join(format!("rustcode-move-dest-{}", std::process::id()));
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dest);
        std::fs::write(&src, "move content").unwrap();
        let out = execute(
            "move_file",
            &serde_json::json!({"src": src.to_str().unwrap(), "dest": dest.to_str().unwrap()}),
        );
        assert!(out.contains("moved"));
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "move content");
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn test_copy_file() {
        let src = std::env::temp_dir().join(format!("rustcode-copy-src-{}", std::process::id()));
        let dest = std::env::temp_dir().join(format!("rustcode-copy-dest-{}", std::process::id()));
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dest);
        std::fs::write(&src, "copy content").unwrap();
        let out = execute(
            "copy_file",
            &serde_json::json!({"src": src.to_str().unwrap(), "dest": dest.to_str().unwrap()}),
        );
        assert!(out.contains("copied"));
        assert!(src.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "copy content");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn test_run_command() {
        let out = execute(
            "run_command",
            &serde_json::json!({"command": "echo 'hello world'"}),
        );
        assert!(out.contains("hello world"), "got: {out}");
        assert!(out.contains("exit code:"), "got: {out}");
    }

    #[test]
    fn test_run_command_cwd() {
        let dir = std::env::temp_dir();
        let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let out = execute(
            "run_command",
            &serde_json::json!({"command": "pwd", "cwd": dir.to_str().unwrap()}),
        );
        assert!(
            out.contains(canonical.to_str().unwrap()),
            "got: {out}"
        );
    }

    #[test]
    fn test_run_command_cwd_invalid() {
        let out = execute(
            "run_command",
            &serde_json::json!({"command": "echo hi", "cwd": "/definitely/not/here"}),
        );
        assert!(out.starts_with("error:"), "got: {out}");
    }

    #[test]
    fn test_run_command_env() {
        let out = execute(
            "run_command",
            &serde_json::json!({
                "command": "echo $RC_ENV_VAR",
                "env": {"RC_ENV_VAR": "envvalue123"}
            }),
        );
        assert!(out.contains("envvalue123"), "got: {out}");
    }

    #[test]
    fn test_run_command_timeout() {
        let out = execute(
            "run_command",
            &serde_json::json!({
                "command": "sleep 5",
                "timeout_ms": 200
            }),
        );
        assert!(out.contains("error:") && out.contains("timed out"), "got: {out}");
    }

    #[test]
    fn test_run_command_stderr_separated() {
        let out = execute(
            "run_command",
            &serde_json::json!({"command": "echo outmsg; echo errmsg 1>&2"}),
        );
        assert!(out.contains("stdout:"), "got: {out}");
        assert!(out.contains("stderr:"), "got: {out}");
        assert!(out.contains("outmsg"), "got: {out}");
        assert!(out.contains("errmsg"), "got: {out}");
    }

    fn edit_fixture(content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustcode-edit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.txt");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_edit_unique_replace() {
        let path = edit_fixture("line one\nline two\nline three\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "line two",
                "new_string": "LINE TWO"
            }),
        );
        assert!(out.contains("edited"), "got: {out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line one\nLINE TWO\nline three\n"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_not_found() {
        let path = edit_fixture("hello\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "does not exist",
                "new_string": "x"
            }),
        );
        assert!(out.starts_with("error:") && out.contains("not found"), "got: {out}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_multiple_requires_replace_all() {
        let path = edit_fixture("dup\ndup\ndup\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "dup",
                "new_string": "x"
            }),
        );
        assert!(
            out.starts_with("error:") && out.contains("3 times"),
            "got: {out}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_replace_all() {
        let path = edit_fixture("dup\ndup\ndup\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "dup",
                "new_string": "x",
                "replace_all": true
            }),
        );
        assert!(out.contains("3 replacements"), "got: {out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x\nx\nx\n");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_identical_strings() {
        let path = edit_fixture("hello\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "hello"
            }),
        );
        assert!(out.starts_with("error:") && out.contains("identical"), "got: {out}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_missing_file() {
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": "/tmp/rustcode-nope-edit-12345.txt",
                "old_string": "a",
                "new_string": "b"
            }),
        );
        assert!(out.starts_with("error:") && out.contains("does not exist"), "got: {out}");
    }

    #[test]
    fn test_list_directory_cap() {
        let dir = std::env::temp_dir().join(format!(
            "rustcode-listcap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(MAX_LIST_ENTRIES + 5) {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        let out = execute(
            "list_directory",
            &serde_json::json!({"path": dir.to_str().unwrap()}),
        );
        assert!(out.contains("more entries"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_needs_confirmation() {
        assert!(!needs_confirmation("get_time"));
        assert!(!needs_confirmation("list_directory"));
        assert!(!needs_confirmation("read_file"));
        assert!(needs_confirmation("create_file"));
        assert!(needs_confirmation("write_file"));
        assert!(needs_confirmation("edit"));
        assert!(needs_confirmation("delete_file"));
        assert!(needs_confirmation("move_file"));
        assert!(needs_confirmation("copy_file"));
        assert!(needs_confirmation("run_command"));
        assert!(!needs_confirmation("nonexistent"));
    }

    fn grep_fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustcode-grep-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        std::fs::write(
            dir.join("src/b.txt"),
            "alpha is a letter\nnothing here\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn test_grep_recursive_regex() {
        let dir = grep_fixture("rec");
        let out = execute(
            "grep",
            &serde_json::json!({"pattern": "alpha", "path": dir.to_str().unwrap()}),
        );
        assert!(out.contains("src/a.rs"), "got: {out}");
        assert!(out.contains("fn alpha()"), "got: {out}");
        assert!(out.contains("src/b.txt"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_include_filter() {
        let dir = grep_fixture("incl");
        let out = execute(
            "grep",
            &serde_json::json!({
                "pattern": "alpha",
                "path": dir.to_str().unwrap(),
                "include": "*.rs"
            }),
        );
        assert!(out.contains("src/a.rs"), "got: {out}");
        assert!(!out.contains("src/b.txt"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_no_match() {
        let dir = grep_fixture("nomatch");
        let out = execute(
            "grep",
            &serde_json::json!({
                "pattern": "zzzznotfound",
                "path": dir.to_str().unwrap()
            }),
        );
        assert!(out.contains("no matches"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_bad_regex_errors() {
        let out = execute(
            "grep",
            &serde_json::json!({"pattern": "*(", "path": "."}),
        );
        assert!(out.starts_with("error:") || out.contains("invalid regex"), "got: {out}");
    }

    #[test]
    fn test_grep_single_file_path() {
        let dir = grep_fixture("single");
        let file = dir.join("src/a.rs");
        let out = execute(
            "grep",
            &serde_json::json!({"pattern": "beta", "path": file.to_str().unwrap()}),
        );
        assert!(out.contains("fn beta()"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_glob_pattern() {
        let dir = grep_fixture("glob");
        let out = execute(
            "glob",
            &serde_json::json!({"pattern": "**/*.rs", "path": dir.to_str().unwrap()}),
        );
        assert!(out.contains("src/a.rs"), "got: {out}");
        assert!(!out.contains("src/b.txt"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_glob_no_match() {
        let dir = grep_fixture("globno");
        let out = execute(
            "glob",
            &serde_json::json!({"pattern": "**/*.zig", "path": dir.to_str().unwrap()}),
        );
        assert!(out.contains("no files matched"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
