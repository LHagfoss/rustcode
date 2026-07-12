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
        name: "check_match",
        description: "Check football match data from api-sports.io. \
                     Useful for checking live scores during matches or finding specific games.\\\n                     Example: check_match with team='Norway' and date='2026-07-11'",
        arguments: "{\\\"date\\\": \"YYYY-MM-DD format required\\\", \\\"team\\\": \"optional team name filter\\\", \\\"status\\\": \"optional status (LIVE, FT, NS)\"}",
        handler: check_match,
        requires_confirmation: false,
    },
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
                      Never returns whole files: use start_line for paging, search for matches, \
                      or ranges for multiple specific sections",
        arguments: "{\"path\": \"file to read\", \
                     \"start_line\": optional number to page from (default 1), \
                     \"search\": \"optional text - returns only matching lines\", \
                     \"ranges\": \"optional list of {start: line, end: line} objects to read multiple sections\"}",
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
        description: "Surgically replace text in an existing file using robust block matching. \
                      Finds a `search_block` and replaces it with a `replace_block`. \
                      Resilient to indentation changes. Supports optional line-number fallback.",
        arguments: "{\"path\": \"file to edit\", \
                      \"search_block\": \"text to find (include enough context for uniqueness)\", \
                      \"replace_block\": \"text to replace it with\", \
                      \"start_line\": \"optional line number for start of block\", \
                      \"end_line\": \"optional line number for end of block\"}",
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
    Tool {
        name: "search_web",
        description: "Performs a web search to look up documentation, API details, or code patterns.",
        arguments: "{\"query\": \"search query terms\", \"domain\": \"optional domain filter e.g. 'docs.rs'\"}",
        handler: search_web,
        requires_confirmation: false,
    },
    Tool {
        name: "find_symbol",
        description: "Queries the codebase symbol index for matching structures, functions, enums, impls, traits, or modules. Returns definition location and signature.",
        arguments: "{\"query\": \"search query string (fuzzy matching on symbol name)\"}",
        handler: find_symbol_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "get_project_map",
        description: "Generates a compressed map of all symbols and API signatures in the codebase to understand project structure.",
        arguments: "{}",
        handler: get_project_map_tool,
        requires_confirmation: false,
    },
];

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
    let glob =
        Glob::new(glob_str).map_err(|e| format!("invalid 'include' glob '{glob_str}': {e}"))?;
    let mut b = GlobSetBuilder::new();
    b.add(glob);
    Ok(Some(
        b.build()
            .map_err(|e| format!("globset build failed: {e}"))?,
    ))
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
                out.push_str(&format!("  {}: {}\n", i + 1, truncate_line(line)));
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

fn grep_one_file(
    path_str: &str,
    path: &Path,
    re: &Regex,
    max_lines: usize,
) -> Result<String, String> {
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
                out.push_str(&format!("(truncated at {max_lines} matching lines)\n"));
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

    let glob = Glob::new(pattern).map_err(|e| format!("invalid glob '{pattern}': {e}"))?;
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

    if let Some(ranges_val) = args.get("ranges").and_then(|r| r.as_array()) {
        let mut out = format!("{path} ({total} lines):\n");
        for (i, range_val) in ranges_val.iter().enumerate() {
            if let Some(range_obj) = range_val.as_object() {
                let start = range_obj
                    .get("start")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let end = range_obj
                    .get("end")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                if let (Some(s), Some(e)) = (start, end) {
                    if s >= 1 && e >= s && s <= total {
                        let actual_end = e.min(total);
                        out.push_str(&format!(
                            "\n--- Range {}: lines {s}-{actual_end} ---\n",
                            i + 1
                        ));
                        for (idx, line) in lines[s - 1..actual_end].iter().enumerate() {
                            out.push_str(&format!("{}: {}\n", s + idx, truncate_line(line)));
                        }
                    } else {
                        out.push_str(&format!(
                            "\n--- Range {}: invalid range {s}-{e} ---\n",
                            i + 1
                        ));
                    }
                } else {
                    out.push_str(&format!(
                        "\n--- Range {}: missing start or end ---\n",
                        i + 1
                    ));
                }
            }
        }
        return Ok(out.trim_end().to_string());
    }

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

fn find_partial_match_error(content: &str, old_string: &str) -> String {
    let old_lines: Vec<&str> = old_string.lines().collect();
    let first_line = old_lines.iter().find(|l| !l.trim().is_empty());
    if let Some(first) = first_line {
        let escaped = regex::escape(first);
        let parts: Vec<&str> = escaped.split_whitespace().collect();
        if !parts.is_empty() {
            let pattern = parts.join(r"\s+");
            if let Ok(re) = Regex::new(&pattern) {
                if let Some(m) = re.find(content) {
                    let char_idx = m.start();
                    let line_number =
                        content[..char_idx].chars().filter(|&c| c == '\n').count() + 1;

                    let file_lines: Vec<&str> = content.lines().collect();
                    let start_idx = line_number.saturating_sub(1);
                    let end_idx = (start_idx + old_lines.len()).min(file_lines.len());
                    let actual_context = file_lines[start_idx..end_idx].join("\n");

                    return format!(
                        "error: 'old_string' not found in file. However, a partial match for the first line was found at line {line_number}.\n\n\
                        Your 'old_string' was:\n\
                        {old_string}\n\n\
                        The file content starting at line {line_number} is:\n\
                        {actual_context}\n\n\
                        Please check for whitespace, indentation, or character differences."
                    );
                }
            }
        }
    }

    "error: 'old_string' not found in file. Make sure it matches the file exactly (whitespace, indentation, quotes). Use read_file to check the exact text.".to_string()
}

/// Normalizes text by trimming trailing whitespace from each line.
fn normalize_text(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds the byte range of all matches of a block of text in content using indentation-insensitive matching.
fn find_block_ranges(content: &str, search_block: &str) -> Vec<std::ops::Range<usize>> {
    let search_lines: Vec<&str> = search_block.lines().collect();
    if search_lines.is_empty() {
        return Vec::new();
    }

    let mut line_starts = Vec::new();
    let mut last_pos = 0;
    for (i, c) in content.char_indices() {
        if c == '\n' {
            line_starts.push(last_pos);
            last_pos = i + 1;
        }
    }
    line_starts.push(last_pos);

    let file_lines: Vec<&str> = content.lines().collect();
    let mut matches = Vec::new();

    for i in 0..=file_lines.len().saturating_sub(search_lines.len()) {
        if file_lines[i].trim_end() != search_lines[0].trim_end() {
            continue;
        }

        let f0_indent = file_lines[i].len() - file_lines[i].trim_start().len();
        let s0_indent = search_lines[0].len() - search_lines[0].trim_start().len();
        let delta = (f0_indent as isize) - (s0_indent as isize);

        let mut match_found = true;
        for j in 1..search_lines.len() {
            if i + j >= file_lines.len()
                || file_lines[i + j].trim_end() != search_lines[j].trim_end()
            {
                match_found = false;
                break;
            }

            let fj_indent = file_lines[i + j].len() - file_lines[i + j].trim_start().len();
            let sj_indent = search_lines[j].len() - search_lines[j].trim_start().len();
            if (fj_indent as isize) - (sj_indent as isize) != delta {
                match_found = false;
                break;
            }
        }

        if match_found {
            let start = line_starts[i];
            let last_line_idx = i + search_lines.len() - 1;
            let end = line_starts[last_line_idx] + file_lines[last_line_idx].len();
            matches.push(start..end);
        }
    }
    matches
}

fn edit(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let search_block = args
        .get("search_block")
        .or_else(|| args.get("old_string"))
        .and_then(|s| s.as_str())
        .ok_or("missing 'search_block' or 'old_string' argument")?;
    let replace_block = args
        .get("replace_block")
        .or_else(|| args.get("new_string"))
        .and_then(|s| s.as_str())
        .ok_or("missing 'replace_block' or 'new_string' argument")?;
    let replace_all = args
        .get("replace_all")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let start_line = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let end_line = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    if search_block.is_empty() {
        return Err("search_block must not be empty".to_string());
    }
    if search_block == replace_block {
        return Err("search_block and replace_block are identical — nothing to do".to_string());
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

    let mut matches = find_block_ranges(&content, search_block);

    if matches.is_empty() {
        if let (Some(s), Some(e)) = (start_line, end_line) {
            let file_lines: Vec<&str> = content.lines().collect();
            if s >= 1 && e >= s && s <= file_lines.len() {
                // Sanity check: does the trimmed content match search_block?
                let actual_content = file_lines[s - 1..e.min(file_lines.len())].join("\n");
                if normalize_text(&actual_content) == normalize_text(search_block) {
                    // Calculate byte range for these lines
                    let mut line_starts = Vec::new();
                    let mut last_pos = 0;
                    for c in content.chars() {
                        if c == '\n' {
                            line_starts.push(last_pos);
                            last_pos += 1;
                        } else {
                            last_pos += c.len_utf8();
                        }
                    }
                    line_starts.push(last_pos);

                    let start_byte = line_starts[s - 1];
                    let end_byte = if e < line_starts.len() {
                        line_starts[e]
                    } else {
                        content.len()
                    };
                    matches.push(start_byte..end_byte);
                }
            }
        }
    }

    // 3. Fallback to substring matching and regex whitespace-insensitive matching (for backwards compatibility & replace_all)
    if matches.is_empty() {
        // Try exact matches first
        matches = content
            .match_indices(search_block)
            .map(|(idx, s)| idx..idx + s.len())
            .collect();

        // If no exact matches, try whitespace-insensitive regex matching
        if matches.is_empty() {
            let escaped = regex::escape(search_block);
            let parts: Vec<&str> = escaped.split_whitespace().collect();
            if !parts.is_empty() {
                let pattern = parts.join(r"\s+");
                if let Ok(re) = Regex::new(&pattern) {
                    matches = re.find_iter(&content).map(|m| m.range()).collect();
                }
            }
        }
    }

    if matches.is_empty() {
        return Err(find_partial_match_error(&content, search_block));
    }

    if matches.len() > 1 && !replace_all {
        return Err(format!(
            "'old_string' appears {} times in '{path}'. Add more surrounding context to make it unique, or set \"replace_all\": true.",
            matches.len()
        ));
    }

    let mut new_content = content.clone();
    for m in matches.iter().rev() {
        new_content.replace_range(m.clone(), replace_block);
    }

    std::fs::write(path, &new_content).map_err(|e| format!("cannot write '{path}': {e}"))?;

    // Async auto-format: run 'cargo fmt' in background without blocking the response
    let path_clone = path.to_string();
    std::thread::spawn(move || {
        let _ = std::process::Command::new("cargo")
            .args(["fmt", "--", &path_clone])
            .output();
    });

    let changed = matches.len();
    Ok(format!(
        "edited '{path}' ({} replacement{}, {} -> {} bytes)",
        changed,
        if changed == 1 { "" } else { "s" },
        content.len(),
        new_content.len()
    ))
}

fn search_web(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or("missing 'query' argument")?;
    let domain = args.get("domain").and_then(|d| d.as_str());

    let mut search_query = query.to_string();
    if let Some(dom) = domain {
        search_query.push_str(&format!(" site:{}", dom));
    }

    if let Ok(api_key) = std::env::var("TAVILY_API_KEY") {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

        let body = serde_json::json!({
            "api_key": api_key,
            "query": search_query,
            "max_results": 5
        });

        let response = client
            .post("https://api.tavily.com/search")
            .json(&body)
            .send()
            .map_err(|e| format!("Tavily request failed: {e}"))?;

        if response.status().is_success() {
            let res_json: serde_json::Value = response
                .json()
                .map_err(|e| format!("failed to parse Tavily JSON: {e}"))?;

            if let Some(results) = res_json.get("results").and_then(|r| r.as_array()) {
                let mut out = String::new();
                out.push_str(&format!(
                    "Web Search Results for '{}' (via Tavily):\n\n",
                    search_query
                ));
                for (i, r) in results.iter().enumerate() {
                    let title = r
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("No Title");
                    let url = r.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    let content = r.get("content").and_then(|c| c.as_str()).unwrap_or("");

                    out.push_str(&format!(
                        "{}. {}\n   Snippet: {}\n   Source: {}\n\n",
                        i + 1,
                        title,
                        content,
                        url
                    ));
                }
                if !results.is_empty() {
                    return Ok(out);
                }
            }
        }
    }

    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        urlencoding::encode(&search_query)
    );

    let client = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("failed to request search results: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "web search failed with status: {}",
            response.status()
        ));
    }

    let html_content = response
        .text()
        .map_err(|e| format!("failed to read search response body: {e}"))?;

    if html_content.contains("anomaly-modal") || html_content.contains("bots use DuckDuckGo too") {
        return Err("Web search failed because DuckDuckGo triggered bot/CAPTCHA protection.\n\
                   To bypass this and get reliable web search, please sign up for a free Tavily account (1,000 free searches/mo) at https://tavily.com and set the TAVILY_API_KEY environment variable.".to_string());
    }

    let document = scraper::Html::parse_document(&html_content);

    let result_selector = scraper::Selector::parse(".result").unwrap();
    let snippet_selector = scraper::Selector::parse(".result__snippet").unwrap();
    let url_selector = scraper::Selector::parse(".result__url").unwrap();

    let mut out = String::new();
    out.push_str(&format!(
        "Web Search Results for '{}' (via DuckDuckGo):\n\n",
        search_query
    ));

    let mut count = 0;
    for element in document.select(&result_selector) {
        if count >= 6 {
            break;
        }

        let snippet_node = element.select(&snippet_selector).next();
        let url_node = element.select(&url_selector).next();

        if let (Some(s_node), Some(u_node)) = (snippet_node, url_node) {
            let snippet = s_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();
            let link = u_node
                .text()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string();

            count += 1;
            out.push_str(&format!(
                "{}. Snippet: {}\n   Source: https://{}\n\n",
                count, snippet, link
            ));
        }
    }

    if count == 0 {
        return Ok("No results found. Try refining your query.".to_string());
    }

    Ok(out)
}

fn find_symbol_tool(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|q| q.as_str())
        .ok_or("missing 'query' argument")?;

    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot determine current directory: {e}"))?;

    let _ = crate::symbols::update_index(&cwd);

    let symbols = crate::symbols::find_symbol(&cwd, query)?;
    if symbols.is_empty() {
        return Ok(format!("No symbols found matching query '{}'.", query));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Found {} symbols matching '{}':\n\n",
        symbols.len(),
        query
    ));
    for (i, sym) in symbols.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} [{}] defined in {} (lines {}-{})\n   Signature: {}\n\n",
            i + 1,
            sym.name,
            sym.kind,
            sym.path,
            sym.start_line,
            sym.end_line,
            sym.signature
        ));
    }

    Ok(out)
}

fn get_project_map_tool(_args: &Value) -> Result<String, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot determine current directory: {e}"))?;

    let _ = crate::symbols::update_index(&cwd);

    crate::symbols::get_project_map(&cwd)
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
- Prefer the `edit` tool over `write_file` for targeted code modifications. \
When using `edit`, provide a unique `search_block` with enough context lines (usually 3-5 lines) \
to avoid ambiguity, and ensure indentation and formatting match the target file exactly.\n\
- Mirror existing style. Use the same libraries, naming, and patterns as the \
surrounding code.\n\
- After meaningful edits, tell the user how to verify (run tests, lint, \
typecheck, build). Do NOT run those yourself unless asked.\n\
- Do NOT commit or push unless the user explicitly asks.\n\
- Destructive or state-changing tools (create, write, edit, delete, move, \
copy, run_command) will prompt the user for confirmation before running. \
Read-only tools (grep, glob, list_directory, read_file) run immediately.\n\n",
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
<tool_result> block, and you continue. You can emit one or more tool calls in parallel if needed; otherwise, wait \
for the results before proceeding. Do not narrate the JSON — just emit the block.\n\n\
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

pub fn parse_tool_calls(text: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let search_start = text
        .find("</think>")
        .map(|idx| idx + "</think>".len())
        .unwrap_or(0);
    let search_text = &text[search_start..];

    // 1. Check for <tool_call>...</tool_call> tags
    let mut current_pos = 0;
    while let Some(start_idx) = search_text[current_pos..].find("<tool_call>") {
        let start = current_pos + start_idx + "<tool_call>".len();
        if let Some(end_offset) = search_text[start..].find("</tool_call>") {
            let end = start + end_offset;
            if let Ok(json) = serde_json::from_str::<Value>(search_text[start..end].trim()) {
                if let Some(res) = extract_tool_call(&json) {
                    out.push(res);
                }
            }
            current_pos = end + "</tool_call>".len();
        } else {
            break;
        }
    }

    // 2. Fallback: check for code blocks or JSON blocks
    if out.is_empty() {
        for marker in &["```tool", "```json"] {
            let mut current_pos = 0;
            while let Some(start_idx) = search_text[current_pos..].find(marker) {
                let start = current_pos + start_idx + marker.len();
                if let Some(end_offset) = search_text[start..].find("```") {
                    let end = start + end_offset;
                    if let Ok(json) = serde_json::from_str::<Value>(search_text[start..end].trim())
                    {
                        if let Some(res) = extract_tool_call(&json) {
                            out.push(res);
                        }
                    }
                    current_pos = end + "```".len();
                } else {
                    break;
                }
            }
        }
    }

    // 2b. Fallback: some open models emit ```shell / ```bash fences to mean
    // "run this", instead of the tool protocol. Treat them as run_command.
    if out.is_empty() {
        for marker in &["```shell", "```bash"] {
            let mut current_pos = 0;
            while let Some(start_idx) = search_text[current_pos..].find(marker) {
                let start = current_pos + start_idx + marker.len();
                if let Some(end_offset) = search_text[start..].find("```") {
                    let end = start + end_offset;
                    let cmd = search_text[start..end].trim();
                    if !cmd.is_empty() {
                        out.push((
                            "run_command".to_string(),
                            serde_json::json!({ "command": cmd }),
                        ));
                    }
                    current_pos = end + "```".len();
                } else {
                    break;
                }
            }
        }
    }

    // 3. Fallback: single braced JSON
    if out.is_empty() {
        if let Some(first_brace) = search_text.find('{') {
            if let Some(last_brace) = search_text.rfind('}') {
                if last_brace > first_brace {
                    if let Ok(json) =
                        serde_json::from_str::<Value>(search_text[first_brace..=last_brace].trim())
                    {
                        if let Some(res) = extract_tool_call(&json) {
                            out.push(res);
                        }
                    }
                }
            }
        }
    }

    out
}

pub fn parse_tool_call(text: &str) -> Option<(String, Value)> {
    parse_tool_calls(text).into_iter().next()
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

const MAX_COMMAND_OUTPUT_BYTES: usize = 100_000;
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

    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
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

    let output = run_with_timeout(cmd, Duration::from_millis(timeout_ms.max(1)))?;

    let mut result = String::new();
    result.push_str(&format!(
        "exit code: {}\n",
        output.status.code().unwrap_or(-1)
    ));

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

fn run_with_timeout(mut cmd: std::process::Command, timeout: Duration) -> Result<Output, String> {
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
    fn test_parse_shell_fence_as_run_command() {
        let text = "<think>plan</think>\n```shell\ngit diff src/network.rs | head\n```";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "run_command");
        assert_eq!(
            args.get("command").and_then(|c| c.as_str()),
            Some("git diff src/network.rs | head")
        );

        let bash = "```bash\necho hi\n```";
        let (name, args) = parse_tool_call(bash).unwrap();
        assert_eq!(name, "run_command");
        assert_eq!(args.get("command").and_then(|c| c.as_str()), Some("echo hi"));

        // explicit tool protocol still wins over the shell fallback
        let tool = "```tool\n{\"name\":\"get_time\",\"arguments\":{}}\n```\n```bash\nls\n```";
        let (name, _) = parse_tool_call(tool).unwrap();
        assert_eq!(name, "get_time");
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        let text = "<tool_call>{\"name\": \"get_time\", \"arguments\": {}}</tool_call>\n\
                    <tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"main.rs\"}}</tool_call>";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "get_time");
        assert_eq!(calls[1].0, "read_file");
        assert_eq!(calls[1].1.get("path").unwrap().as_str().unwrap(), "main.rs");
    }

    #[test]
    fn test_search_web() {
        let out = execute(
            "search_web",
            &serde_json::json!({
                "query": "rust programming language",
                "domain": "rust-lang.org"
            }),
        );
        assert!(
            out.contains("Web Search Results")
                || out.contains("failed")
                || out.contains("No results found"),
            "got: {out}"
        );
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
        assert!(out.contains(canonical.to_str().unwrap()), "got: {out}");
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
        assert!(
            out.contains("error:") && out.contains("timed out"),
            "got: {out}"
        );
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
        assert!(
            out.starts_with("error:") && out.contains("not found"),
            "got: {out}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_whitespace_insensitive() {
        let path = edit_fixture("fn  hello()   {\n\tprintln!(\"hi\");\n}\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "fn hello() {\n    println!(\"hi\");\n}",
                "new_string": "fn hello() { println!(\"hello\"); }"
            }),
        );
        assert!(out.contains("edited"), "got: {out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "fn hello() { println!(\"hello\"); }\n"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_edit_partial_match_diagnostic() {
        let path = edit_fixture("fn hello() {\n    println!(\"hi\");\n}\n");
        let out = execute(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_string": "fn hello() {\n    println!(\"wrong\");\n}",
                "new_string": "x"
            }),
        );
        assert!(out.starts_with("error:"), "got: {out}");
        assert!(out.contains("partial match"), "got: {out}");
        assert!(out.contains("line 1"), "got: {out}");
        assert!(out.contains("println!(\"hi\");"), "got: {out}");
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
        assert!(
            out.starts_with("error:") && out.contains("identical"),
            "got: {out}"
        );
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
        assert!(
            out.starts_with("error:") && out.contains("does not exist"),
            "got: {out}"
        );
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
        std::fs::write(dir.join("src/b.txt"), "alpha is a letter\nnothing here\n").unwrap();
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
        let out = execute("grep", &serde_json::json!({"pattern": "*(", "path": "."}));
        assert!(
            out.starts_with("error:") || out.contains("invalid regex"),
            "got: {out}"
        );
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

/// Check football match data from api-sports.io
fn check_match(args: &serde_json::Value) -> Result<String, String> {
    use reqwest::blocking::Client;
    use std::fmt::Write as _;

    let team = args.get("team").and_then(|v| v.as_str()).unwrap_or("");
    let date = args.get("date").and_then(|v| v.as_str()).unwrap_or("");

    if date.is_empty() {
        return Err("date parameter required (YYYY-MM-DD format)".to_string());
    }

    let client = Client::new();
    let api_key = "fb492b51acab4d134f2d33ef9777865a";
    let url = if !team.is_empty() {
        format!(
            "https://v3.football.api-sports.io/fixtures?date={}&team={}",
            date, team
        )
    } else {
        format!("https://v3.football.api-sports.io/fixtures?date={}", date)
    };

    let response = client
        .get(&url)
        .header("x-apisports-key", api_key)
        .send()
        .map_err(|e| format!("API request failed: {}", e))?
        .json::<serde_json::Value>()
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let matches = match response.get("response") {
        Some(serde_json::Value::Array(matches)) => matches,
        _ => return Ok("No matches found".to_string()),
    };

    if matches.is_empty() {
        return Ok(format!("Found 0 match(es) for {}", date));
    }

    let mut output = String::new();
    write!(output, "Found {} match(es),\n", matches.len()).unwrap();
    writeln!(
        output,
        "══════════════════════════════════════════════════════"
    )
    .unwrap();

    for (i, match_data) in matches.iter().enumerate() {
        let teams = &match_data["teams"];
        let home = &teams["home"]["name"];
        let away = &teams["away"]["name"];

        let goals = &match_data["goals"];
        let score_home = goals
            .get("home")
            .and_then(|v| v.as_i64())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        let score_away = goals
            .get("away")
            .and_then(|v| v.as_i64())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());

        let status_info = &match_data["status"];
        let long_status = status_info
            .get("long")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown");
        let elapsed = status_info.get("elapsed").and_then(|v| v.as_i64());

        let ts = match_data["fixture"]["timestamp"].as_i64().unwrap_or(0);
        let dt = chrono::DateTime::from_timestamp(ts, 0)
            .map(|d| d.format("%H:%M UTC").to_string())
            .unwrap_or_else(|| "Invalid time".to_string());

        let league_name = match_data["league"]["name"].as_str().unwrap_or("Unknown");
        let country = match_data["league"]["country"].as_str().unwrap_or("");

        if i > 0 {
            writeln!(
                output,
                "\n──────────────────────────────────────────────────────"
            )
            .unwrap();
        }

        write!(output, "Match {}:\n", i + 1).unwrap();
        write!(output, "League: {}\n", league_name).unwrap();
        write!(output, "Country: {}\n", country).unwrap();
        write!(output, "Time: {}\n", dt).unwrap();

        if let Some(minutes) = elapsed {
            writeln!(output, "Status: LIVE - Minute {}", minutes).unwrap();
        } else {
            writeln!(output, "Status: {}", long_status).unwrap();
        }

        writeln!(output, "Teams: {} vs {}", home, away).unwrap();
        writeln!(output, "Score: {} - {}", score_home, score_away).unwrap();
    }

    Ok(output)
}
