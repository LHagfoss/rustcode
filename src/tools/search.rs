use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;

// Re-exports needed by search tools
pub(crate) use super::parse_json_bool;
pub(crate) use super::resolve_tool_path;

use super::{Tool, ToolCapability, ToolSafety};

fn grep_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string" }, "path": { "type": "string" },
            "include": { "type": "string" }, "ignore_case": { "type": "boolean", "default": false }
        }, "required": ["pattern"]
    })
}

pub const GREP: Tool = Tool {
    name: "grep",
    description: "Recursively search file contents with regex. Respects                       .gitignore and skips hidden files. Use this to find where                       functions, classes, strings, or patterns are defined or used",
    arguments: r#"{"pattern": "regex pattern", "path": "optional directory or file (default current dir)", "include": "optional file glob filter e.g. '*.rs'", "ignore_case": optional bool (default false)}"#,
    handler: grep,
    requires_confirmation: false,
    schema: grep_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn glob_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "pattern": { "type": "string" }, "path": { "type": "string" }
        }, "required": ["pattern"]
    })
}

pub const GLOB: Tool = Tool {
    name: "glob",
    description: "Find files by glob pattern (e.g. '**/*.rs', 'src/**/*.ts').                       Respects .gitignore and skips hidden files. Returns matching                       paths, sorted. Use this to discover files by name",
    arguments: r#"{"pattern": "glob pattern", "path": "optional root directory (default current dir)"}"#,
    handler: glob,
    requires_confirmation: false,
    schema: glob_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn list_directory_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "path": { "type": "string" } }
    })
}

pub const LIST_DIRECTORY: Tool = Tool {
    name: "list_directory",
    description: "List files in a directory",
    arguments: r#"{"path": "directory path, defaults to current dir"}"#,
    handler: list_directory,
    requires_confirmation: false,
    schema: list_directory_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn find_symbol_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"]
    })
}

pub const FIND_SYMBOL: Tool = Tool {
    name: "find_symbol",
    description: "Queries the codebase symbol index for matching structures, functions, enums, impls, traits, or modules. Returns definition location and signature.",
    arguments: r#"{"query": "search query string (fuzzy matching on symbol name)"}"#,
    handler: find_symbol_tool,
    requires_confirmation: false,
    schema: find_symbol_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn get_project_map_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {}, "additionalProperties": false
    })
}

pub const GET_PROJECT_MAP: Tool = Tool {
    name: "get_project_map",
    description: "Generates a compressed map of all symbols and API signatures in the codebase to understand project structure.",
    arguments: r#"{}"#,
    handler: get_project_map_tool,
    requires_confirmation: false,
    schema: get_project_map_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

const MAX_GREP_LINES: usize = 200;
const MAX_GREP_FILES: usize = 50;
const MAX_GLOB_RESULTS: usize = 200;
const MAX_LIST_ENTRIES: usize = 10_000;
const MAX_LINE_CHARS: usize = 160;

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

/// "No matches" must say whether an `include` filter was applied — otherwise
/// the model can't tell "pattern absent" apart from "filter excluded every
/// file" and starts distrusting the search instead of its pattern.
fn no_matches_message(pattern: &str, root: &str, include: Option<&str>) -> String {
    let scope = if std::path::Path::new(root).is_file() {
        "in"
    } else {
        "under"
    };
    match include {
        Some(inc) => {
            format!("no matches for '{pattern}' {scope} '{root}' (include filter: '{inc}')")
        }
        None => format!("no matches for '{pattern}' {scope} '{root}'"),
    }
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() > MAX_LINE_CHARS {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

fn try_ripgrep(
    pattern: &str,
    root: &str,
    include: Option<&str>,
    ignore_case: bool,
) -> Option<Result<String, String>> {
    let mut cmd = std::process::Command::new("rg");
    cmd.arg("--line-number")
        .arg("--color=never")
        .arg("--heading")
        .arg("--hidden");

    if ignore_case {
        cmd.arg("-i");
    } else {
        cmd.arg("-s");
    }

    if let Some(inc) = include {
        cmd.arg("-g").arg(inc);
    }

    cmd.arg(pattern).arg(root);

    let output = match cmd.output() {
        Ok(out) => out,
        Err(_) => return None,
    };

    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Some(Ok(no_matches_message(pattern, root, include)));
    }

    let mut out = String::new();
    let mut files_hit = 0usize;
    let mut total_lines = 0usize;
    let mut current_file_has_header = false;

    for line in stdout.lines() {
        if line.is_empty() {
            continue;
        }
        if let Some((num, text)) = line.split_once(':') {
            if num.parse::<usize>().is_ok() {
                if !current_file_has_header {
                    out.push_str(&format!("{root}:\n"));
                    current_file_has_header = true;
                    files_hit += 1;
                }
                out.push_str(&format!("  {}: {}\n", num, truncate_line(text)));
                total_lines += 1;
                if total_lines >= MAX_GREP_LINES {
                    out.push_str(&format!(
                        "\n(truncated — {} matching lines across {} files, stopping at cap of {MAX_GREP_LINES} lines / {MAX_GREP_FILES} files; narrow 'pattern' or 'include')\n",
                        total_lines, files_hit
                    ));
                    break;
                }
                continue;
            }
        }

        files_hit += 1;
        if files_hit > MAX_GREP_FILES {
            break;
        }
        let file_header = if line.ends_with(':') {
            line.to_string()
        } else {
            format!("{line}:")
        };
        out.push_str(&format!("\n{file_header}\n"));
        current_file_has_header = true;
    }

    let root_path = std::path::Path::new(root);
    if root_path.is_file() {
        Some(Ok(out.trim_end().to_string()))
    } else {
        Some(Ok(format!(
            "matches for '{pattern}' under '{root}' ({} file(s)):\n{}",
            files_hit,
            out.trim_end()
        )))
    }
}

pub fn grep(args: &Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|p| p.as_str())
        .ok_or("missing 'pattern' argument")?;
    let root = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let include = args.get("include").and_then(|p| p.as_str());
    let ignore_case = args
        .get("ignore_case")
        .and_then(parse_json_bool)
        .unwrap_or(false);

    if let Some(res) = try_ripgrep(pattern, root, include, ignore_case) {
        return res;
    }

    let mut re_builder = regex::RegexBuilder::new(pattern);
    re_builder.case_insensitive(ignore_case);
    let re = re_builder
        .build()
        .map_err(|e| format!("invalid regex '{pattern}': {e}"))?;

    let include_set = build_include_matcher(include)?;

    let root_path = std::path::Path::new(root);
    if root_path.is_file() {
        return grep_one_file(root, root_path, &re, MAX_GREP_LINES);
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
        Ok(no_matches_message(pattern, root, include))
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
    path: &std::path::Path,
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

pub fn glob(args: &Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|p| p.as_str())
        .ok_or("missing 'pattern' argument")?;
    let root = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let root_path = std::path::Path::new(root);
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

pub fn list_directory(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(|p| p.as_str()).unwrap_or(".");
    let resolved_path = resolve_tool_path(path);

    if resolved_path.is_file() {
        return Err(format!(
            "'{path}' is a file, not a directory - use the read_file tool instead"
        ));
    }
    let entries =
        std::fs::read_dir(&resolved_path).map_err(|e| format!("cannot read '{path}': {e}"))?;
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

pub fn find_symbol_tool(args: &Value) -> Result<String, String> {
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
    for sym in symbols {
        out.push_str(&format!(
            "- {} ({}) in {} lines {}-{}\n",
            sym.name,
            sym.kind,
            sym.path,
            sym.start_line + 1,
            sym.end_line + 1
        ));
    }
    Ok(out)
}

pub fn get_project_map_tool(_args: &Value) -> Result<String, String> {
    let cwd =
        std::env::current_dir().map_err(|e| format!("cannot determine current directory: {e}"))?;

    let _ = crate::symbols::update_index(&cwd);

    crate::symbols::get_project_map(&cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_matches_names_the_include_filter() {
        assert_eq!(
            no_matches_message("foo", ".", Some("src/**/*.rs")),
            "no matches for 'foo' under '.' (include filter: 'src/**/*.rs')"
        );
    }

    #[test]
    fn no_matches_without_filter_stays_plain() {
        assert_eq!(
            no_matches_message("foo", ".", None),
            "no matches for 'foo' under '.'"
        );
    }
}
