use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::thread;

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
    description: "Bounded ripgrep-style regex search over repository files. Respects .gitignore, skips hidden files, and returns structured matches. Use this first to locate exact definitions and references; use `rg` via run_command only when advanced ripgrep flags, counts, or file-list modes are needed.",
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
const MAX_GREP_BYTES: usize = 32_768;
const MAX_SCAN_LINE_BYTES: usize = 32_768;
const MAX_GLOB_RESULTS: usize = 200;
const MAX_LIST_ENTRIES: usize = 10_000;
const MAX_LINE_CHARS: usize = 1000;

struct CappedRead {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_capped<R: Read>(mut reader: R, cap: usize) -> CappedRead {
    let mut output = Vec::with_capacity(cap);
    let mut truncated = false;
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let remaining = cap.saturating_sub(output.len());
                let take = remaining.min(read);
                output.extend_from_slice(&chunk[..take]);
                truncated |= take < read;
            }
        }
    }
    CappedRead {
        bytes: output,
        truncated,
    }
}

#[derive(Default)]
struct ScanReport {
    truncated_line: bool,
}

fn scan_file_lines<R: BufRead, F: FnMut(usize, &str) -> bool>(
    mut reader: R,
    mut visit: F,
) -> std::io::Result<ScanReport> {
    let mut line = Vec::new();
    let mut line_number = 0;
    let mut report = ScanReport::default();
    loop {
        line.clear();
        let mut read_any = false;
        let mut line_truncated = false;
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            read_any = true;
            let newline = buffer.iter().position(|byte| *byte == b'\n');
            let chunk_len = newline.map(|index| index + 1).unwrap_or(buffer.len());
            let remaining = MAX_SCAN_LINE_BYTES.saturating_sub(line.len());
            let take = remaining.min(chunk_len);
            line.extend_from_slice(&buffer[..take]);
            line_truncated |= take < chunk_len;
            reader.consume(chunk_len);
            if newline.is_some() {
                break;
            }
        }
        if !read_any {
            break;
        }
        if line_truncated {
            report.truncated_line = true;
        }
        line_number += 1;
        let line = if line.last() == Some(&b'\n') {
            &line[..line.len() - 1]
        } else {
            &line
        };
        let line = if line.last() == Some(&b'\r') {
            &line[..line.len() - 1]
        } else {
            line
        };
        let line = match std::str::from_utf8(line) {
            Ok(line) => line,
            Err(error) if error.error_len().is_none() => {
                report.truncated_line = true;
                std::str::from_utf8(&line[..error.valid_up_to()])
                    .expect("UTF-8 prefix must be valid")
            }
            Err(error) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
            }
        };
        if !visit(line_number, line) {
            break;
        }
    }
    Ok(report)
}

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

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return None,
    };
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let stdout_handle = thread::spawn(move || read_capped(stdout, MAX_GREP_BYTES));
    let stderr_handle = thread::spawn(move || read_capped(stderr, 4096));
    let status = match child.wait() {
        Ok(status) => status,
        Err(_) => return None,
    };
    let stdout_capture = stdout_handle.join().unwrap_or(CappedRead {
        bytes: Vec::new(),
        truncated: false,
    });
    let _ = stderr_handle.join();

    if !status.success() && status.code() != Some(1) {
        return None;
    }

    let stdout_truncated = stdout_capture.truncated;
    let stdout = String::from_utf8_lossy(&stdout_capture.bytes);
    if stdout.trim().is_empty() {
        if stdout_truncated {
            return Some(Ok(format!(
                "ripgrep output exceeded the {} KB capture limit; matches may be incomplete. Narrow 'pattern' or 'include'.",
                MAX_GREP_BYTES / 1024
            )));
        }
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
                let line_formatted = format!("  {}: {}\n", num, truncate_line(text));
                if total_lines >= MAX_GREP_LINES
                    || out.len() + line_formatted.len() >= MAX_GREP_BYTES
                {
                    let cap_desc = if total_lines >= MAX_GREP_LINES {
                        format!("{MAX_GREP_LINES} lines")
                    } else {
                        format!("{} KB", MAX_GREP_BYTES / 1024)
                    };
                    out.push_str(&format!(
                        "\n(truncated — {} matching lines across {} files, stopping at cap of {cap_desc} / {MAX_GREP_FILES} files; narrow 'pattern' or 'include')\n",
                        total_lines, files_hit
                    ));
                    break;
                }
                out.push_str(&line_formatted);
                total_lines += 1;
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

    if stdout_truncated && !out.contains("matches may be incomplete") {
        out.push_str(&format!(
            "\n(ripgrep output exceeded the {} KB capture limit; matches may be incomplete — narrow 'pattern' or 'include')\n",
            MAX_GREP_BYTES / 1024
        ));
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
    let mut incomplete = false;

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

        let Ok(file) = File::open(path) else {
            incomplete = true;
            continue;
        };
        let mut file_lines = 0usize;
        let mut wrote_header = false;
        let mut stop_search = false;
        match scan_file_lines(BufReader::new(file), |line_number, line| {
            if re.is_match(line) {
                if !wrote_header {
                    files_hit += 1;
                    if files_hit > MAX_GREP_FILES {
                        stop_search = true;
                        return false;
                    }
                    out.push_str(&format!("\n{}:\n", path.display()));
                    wrote_header = true;
                }
                let line_formatted = format!("  {}: {}\n", line_number, truncate_line(line));
                if total_lines >= MAX_GREP_LINES
                    || out.len() + line_formatted.len() >= MAX_GREP_BYTES
                {
                    let cap_desc = if total_lines >= MAX_GREP_LINES {
                        format!("{MAX_GREP_LINES} lines")
                    } else {
                        format!("{} KB", MAX_GREP_BYTES / 1024)
                    };
                    out.push_str(&format!(
                        "\n(truncated — {} matching lines across {} files, stopping at cap of {cap_desc} / {MAX_GREP_FILES} files; narrow 'pattern' or 'include')\n",
                        total_lines, files_hit
                    ));
                    stop_search = true;
                    return false;
                }
                out.push_str(&line_formatted);
                file_lines += 1;
                total_lines += 1;
            }
            true
        }) {
            Ok(report) => incomplete |= report.truncated_line,
            Err(_) => incomplete = true,
        }
        if stop_search {
            break;
        }
        let _ = file_lines;
    }

    if out.is_empty() {
        if incomplete {
            return Ok(format!(
                "fallback search may be incomplete; some input exceeded the per-line limit or could not be read while searching for '{pattern}' under '{root}'"
            ));
        }
        Ok(no_matches_message(pattern, root, include))
    } else {
        if incomplete {
            out.push_str(
                "\n(fallback search may be incomplete; some input exceeded the per-line limit or could not be read)\n",
            );
        }
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
    let file = File::open(path).map_err(|e| format!("cannot read '{path_str}': {e}"))?;
    let mut out = String::new();
    let mut hits = 0usize;
    let report = scan_file_lines(BufReader::new(file), |line_number, line| {
        if re.is_match(line) {
            hits += 1;
            if hits == 1 {
                out.push_str(&format!("{path_str}:\n"));
            }
            let line_formatted = format!("  {}: {}\n", line_number, truncate_line(line));
            if hits > max_lines {
                out.push_str(&format!("(truncated at {max_lines} matching lines)\n"));
                return false;
            }
            if out.len() + line_formatted.len() >= MAX_GREP_BYTES {
                out.push_str(&format!("(truncated at {} KB)\n", MAX_GREP_BYTES / 1024));
                return false;
            }
            out.push_str(&line_formatted);
        }
        true
    })
    .map_err(|e| format!("cannot read '{path_str}': {e}"))?;
    if report.truncated_line {
        out.push_str(
            "\n(fallback search may be incomplete; an input line exceeded the per-line limit)\n",
        );
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

    #[test]
    fn ripgrep_reader_is_bounded_before_processing_matches() {
        let input = std::io::Cursor::new(vec![b'x'; MAX_GREP_BYTES * 2]);
        let captured = read_capped(input, MAX_GREP_BYTES);

        assert_eq!(captured.bytes.len(), MAX_GREP_BYTES);
        assert!(captured.truncated);
    }

    #[test]
    fn fallback_search_scans_input_line_by_line() {
        let mut lines = Vec::new();
        scan_file_lines(std::io::Cursor::new("first\nsecond\n"), |number, line| {
            lines.push((number, line.to_string()));
            true
        })
        .expect("scan");

        assert_eq!(lines, [(1, "first".to_string()), (2, "second".to_string())]);
    }

    #[test]
    fn fallback_search_bounds_newline_free_lines() {
        let input = format!("{}\n", "x".repeat(MAX_SCAN_LINE_BYTES * 2));
        let mut observed_len = 0;
        let report = scan_file_lines(std::io::Cursor::new(input), |_, line| {
            observed_len = line.len();
            true
        })
        .expect("scan");

        assert_eq!(observed_len, MAX_SCAN_LINE_BYTES);
        assert!(report.truncated_line);
    }

    #[test]
    fn grep_stops_at_byte_cap_for_large_matching_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("long_matches.txt");
        // Create 100 lines each of 800 characters = ~80KB, which exceeds MAX_GREP_BYTES (32KB) before MAX_GREP_LINES (200)
        let long_line = format!("MATCH {}", "a".repeat(800));
        let mut content = String::new();
        for _ in 0..100 {
            content.push_str(&long_line);
            content.push('\n');
        }
        std::fs::write(&file, content).expect("write");

        let res = grep(&serde_json::json!({
            "path": file.to_string_lossy().to_string(),
            "pattern": "MATCH",
        }))
        .expect("grep should succeed");

        assert!(
            res.contains("truncated") && res.contains("32 KB"),
            "got: {res}"
        );
        assert!(res.contains("matches may be incomplete"), "got: {res}");
    }

    #[test]
    fn grep_one_file_returns_the_line_at_the_line_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("matches.txt");
        std::fs::write(&file, "alpha\nbeta MATCH one\ngamma MATCH two\n").expect("write");
        let re = Regex::new("MATCH").unwrap();
        let path_str = file.to_string_lossy().to_string();

        // With max_lines == 1 the single permitted match must still be
        // returned; only the hit beyond the cap triggers truncation.
        let res = grep_one_file(&path_str, &file, &re, 1).expect("grep should succeed");
        assert!(res.contains("MATCH one"), "got: {res}");
        assert!(res.contains("truncated at 1 matching lines"), "got: {res}");
        assert!(!res.contains("MATCH two"), "got: {res}");

        // Exactly at the cap there is no truncation notice at all.
        let res = grep_one_file(&path_str, &file, &re, 2).expect("grep should succeed");
        assert!(
            res.contains("MATCH one") && res.contains("MATCH two"),
            "got: {res}"
        );
        assert!(!res.contains("truncated"), "got: {res}");
    }
}
