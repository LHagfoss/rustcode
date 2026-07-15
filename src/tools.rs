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

fn resolve_tool_path(raw_path: &str) -> PathBuf {
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
    Tool {
        name: "view_file",
        description: "View the contents of a file. Supports line ranges (1-indexed) and optional byte offset if content is truncated.",
        arguments: "{\"path\": \"absolute or relative path to file\", \
                     \"start_line\": \"optional start line number, 1-indexed (default 1)\", \
                     \"end_line\": \"optional end line number, 1-indexed (default start_line + 500)\", \
                     \"content_offset\": \"optional byte offset into content\"}",
        handler: view_file_tool,
        requires_confirmation: false,
    },
    Tool {
        name: "replace_file_content",
        description: "Surgically edit a contiguous block of text in an existing file. \
                      Requires specifying the line boundaries, the exact target content, \
                      and the replacement content.",
        arguments: "{\"path\": \"absolute or relative path to file\", \
                     \"start_line\": \"1-indexed start line containing target content\", \
                     \"end_line\": \"1-indexed end line containing target content\", \
                     \"target_content\": \"precise block of code to edit (must match file exactly)\", \
                     \"replacement_content\": \"complete replacement text for that block\"}",
        handler: replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "multi_replace_file_content",
        description: "Apply multiple non-contiguous edits across a single file in a single tool call. \
                      Specify each edit as a separate replacement chunk.",
        arguments: "{\"path\": \"absolute or relative path to file\", \
                     \"replacements\": \"array of objects, each containing: {start_line, end_line, target_content, replacement_content}\"}",
        handler: multi_replace_file_content_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "write_to_file",
        description: "Create a new file or overwrite an existing file with complete content. \
                      Creates parent directories automatically.",
        arguments: "{\"path\": \"absolute or relative path to file\", \
                     \"content\": \"entire contents to write\", \
                     \"overwrite\": \"set true to allow overwriting an existing file (default false)\"}",
        handler: write_to_file_tool,
        requires_confirmation: true,
    },
    Tool {
        name: "complete_task",
        description: "Mark the continuous goal/task as successfully complete.",
        arguments: "{\"result\": \"summary of what was achieved and final results\"}",
        handler: complete_task_tool,
        requires_confirmation: false,
    },
];

pub const MAX_TOOL_ROUNDS: usize = 60;

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
        .and_then(parse_json_bool)
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

const MAX_LINE_CHARS: usize = 160;

fn truncate_line(line: &str) -> String {
    if line.chars().count() > MAX_LINE_CHARS {
        let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}









/// Normalizes text by trimming trailing whitespace from each line.


/// Finds the byte range of all matches of a block of text in content using indentation-insensitive matching.




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
    let resolved_path = resolve_tool_path(path);
    if !resolved_path.exists() {
        return Err(format!("'{path}' does not exist"));
    }
    if resolved_path.is_dir() {
        return Err(format!(
            "'{path}' is a directory — use delete_dir if needed (not supported yet)"
        ));
    }
    std::fs::remove_file(&resolved_path).map_err(|e| format!("cannot delete '{path}': {e}"))?;
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
    let resolved_src = resolve_tool_path(src);
    let resolved_dest = resolve_tool_path(dest);
    if !resolved_src.exists() {
        return Err(format!("source '{src}' does not exist"));
    }
    if let Some(parent) = resolved_dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{dest}': {e}"))?;
    }
    std::fs::rename(&resolved_src, &resolved_dest)
        .map_err(|e| format!("cannot move '{src}' to '{dest}': {e}"))?;
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
    let resolved_src = resolve_tool_path(src);
    let resolved_dest = resolve_tool_path(dest);
    if !resolved_src.exists() {
        return Err(format!("source '{src}' does not exist"));
    }
    if resolved_src.is_dir() {
        return Err(format!(
            "source '{src}' is a directory — copy_file only supports copying files"
        ));
    }
    if let Some(parent) = resolved_dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{dest}': {e}"))?;
    }
    std::fs::copy(&resolved_src, &resolved_dest)
        .map_err(|e| format!("cannot copy '{src}' to '{dest}': {e}"))?;
    Ok(format!("copied '{src}' to '{dest}'"))
}

/// Agent-orchestration tools handled by the orchestrator itself (async,
/// need app state), not by the sync handlers in TOOLS. Only offered to the
/// main agent — subagents cannot spawn or message other agents.
pub fn is_agent_tool(name: &str) -> bool {
    matches!(name, "spawn_agent" | "send_agent" | "set_goal")
}

pub fn tool_system_prompt(
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
) -> String {
    let mut p = String::new();

    p.push_str(
        "You are rustcode, a terminal-based coding assistant.\n\
- Use `sandbox/` for temporary scripts/builds, and `artifacts/` for persistent designs/reports.\n\
- For long commands (>2s, e.g. build, test, install), set `\"background\": true` in `run_command`.\n\n\
# Rules\n\
- Be concise and direct. No filler or preamble.\n\
- Explore first: use `grep`, `glob`, `view_file` to understand context before editing.\n\
- Prefer targeted `replace_file_content` or `multi_replace_file_content` over `write_to_file`. Use paging with `view_file` (start_line/end_line).\n\
- Match project code style.\n\
- Only run tests/builds or commit/push code when explicitly requested by the user.\n\
- Read-only tools run immediately; modifying/destructive tools require confirmation.\n\n"
    );

    p.push_str("# Tool Format\n");
    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "To call a tool, output exactly one fenced `tool` block containing a single JSON object. Do not output any conversational text or narration before or after the block.\n\n\
                ```tool\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}\n\
                ```\n\n\
                Rules:\n\
                - Keys must be \"name\" and \"arguments\".\n\
                - Pass correct type for arguments (no quotes for numbers/booleans).\n\n"
            );
        }
    }

    p.push_str("Available tools:\n");
    for t in TOOLS {
        p.push_str(&format!(
            "- {} | Args: {} | {}\n",
            t.name, t.arguments, t.description
        ));
    }
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        for client in reg.values() {
            if let Ok(tools) = client.get_tools() {
                for tool in tools {
                    let name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let desc = tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    let schema = tool.get("inputSchema").unwrap_or(&serde_json::Value::Null);
                    p.push_str(&format!(
                        "- {} | Args: {} | {}\n",
                        name,
                        serde_json::to_string(schema).unwrap_or_default(),
                        desc
                    ));
                }
            }
        }
    }
    if include_agent_tools {
        p.push_str(
            "- spawn_agent | Args: {\"task\": \"task description\"} | Delegate task to a fresh subagent.\n\
            - send_agent | Args: {\"id\": subagent_id, \"message\": \"message\"} | Send follow-up to subagent.\n\
            - set_goal | Args: {\"goal\": \"goal description\"} | Set a new long-running task and switch the agent to continuous autoloop mode.\n",
        );
    }

    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
```tool\n\
{\"name\": \"grep\", \"arguments\": {\"pattern\": \"agent loop\", \"include\": \"*.rs\"}}\n\
```\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
    }

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

fn parse_tool_calls_impl(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Vec<(String, Value)> {
    match protocol {
        crate::config::ToolProtocol::Json => {
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
                    if let Ok(json) = serde_json::from_str::<Value>(search_text[start..end].trim())
                    {
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
                            if let Ok(json) =
                                serde_json::from_str::<Value>(search_text[start..end].trim())
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
            // "run this", instead of the tool protocol. Treat them as run_command
            // ONLY if the block constitutes the entire response (excluding think block).
            if out.is_empty() {
                let trimmed_search = search_text.trim();
                for marker in &["```shell", "```bash"] {
                    if trimmed_search.starts_with(marker) && trimmed_search.ends_with("```") {
                        if let Some(first_end) = trimmed_search[marker.len()..].find("```") {
                            let absolute_end = marker.len() + first_end;
                            if absolute_end + "```".len() == trimmed_search.len() {
                                let cmd = trimmed_search[marker.len()..absolute_end].trim();
                                if !cmd.is_empty() {
                                    out.push((
                                        "run_command".to_string(),
                                        serde_json::json!({ "command": cmd }),
                                    ));
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // 3. Fallback: scan for matching braced JSON objects
            if out.is_empty() {
                let mut current_pos = 0;
                while let Some(start_idx) = search_text[current_pos..].find('{') {
                    let start = current_pos + start_idx;
                    let mut brace_count = 0;
                    let mut end = None;
                    let bytes = search_text.as_bytes();
                    let mut in_string = false;
                    let mut escaped = false;

                    for i in start..bytes.len() {
                        let c = bytes[i] as char;
                        if in_string {
                            if escaped {
                                escaped = false;
                            } else if c == '\\' {
                                escaped = true;
                            } else if c == '"' {
                                in_string = false;
                            }
                        } else if c == '"' {
                            in_string = true;
                        } else if c == '{' {
                            brace_count += 1;
                        } else if c == '}' {
                            brace_count -= 1;
                            if brace_count == 0 {
                                end = Some(i);
                                break;
                            }
                        }
                    }

                    if let Some(end_idx) = end {
                        if let Ok(json) =
                            serde_json::from_str::<Value>(&search_text[start..=end_idx])
                        {
                            if let Some(res) = extract_tool_call(&json) {
                                out.push(res);
                            }
                        }
                        current_pos = end_idx + 1;
                    } else {
                        current_pos = start + 1;
                    }
                }
            }

            // 4. Fallback: robust key-value extraction for pseudo-XML / JSON hybrids (e.g. <arg_key>...)
            if out.is_empty() {
                let mut tool_name = None;
                let mut args_obj = serde_json::Map::new();

                let mut current_pos = 0;
                while let Some(start_idx) = search_text[current_pos..].find("<arg_key>") {
                    let key_start = current_pos + start_idx + "<arg_key>".len();
                    let key_end = search_text[key_start..]
                        .find('<')
                        .map(|offset| key_start + offset)
                        .unwrap_or(search_text.len());

                    let raw_key = &search_text[key_start..key_end];

                    // Now check for a value tag
                    let mut val_str = String::new();
                    if let Some(val_idx) = search_text[key_end..].find("<arg_value>") {
                        let val_start = key_end + val_idx + "<arg_value>".len();
                        let val_end = search_text[val_start..]
                            .find('<')
                            .map(|offset| val_start + offset)
                            .unwrap_or(search_text.len());
                        val_str = search_text[val_start..val_end].to_string();
                        current_pos = val_end;
                    } else {
                        current_pos = key_end;
                    }

                    // Clean up key and value
                    let mut key = raw_key.trim().to_string();
                    let mut val = val_str.trim().to_string();

                    // If the key has a JSON-like pair inside (e.g. `start_line": "1619"`), split it!
                    if key.contains(':') {
                        let (split_key, split_val) = {
                            let parts: Vec<&str> = key.splitn(2, ':').collect();
                            let sk = parts[0]
                                .trim_matches(|c: char| {
                                    c == '"' || c == '\'' || c == ' ' || c == '{' || c == '}'
                                })
                                .to_string();
                            let sv = parts[1]
                                .trim_matches(|c: char| {
                                    c == '"' || c == '\'' || c == ' ' || c == '{' || c == '}'
                                })
                                .to_string();
                            (sk, sv)
                        };
                        key = split_key;
                        val = split_val;
                    } else {
                        key = key
                            .trim_matches(|c: char| {
                                c == '"' || c == '\'' || c == ' ' || c == '{' || c == '}'
                            })
                            .to_string();
                        val = val
                            .trim_matches(|c: char| {
                                c == '"' || c == '\'' || c == ' ' || c == '{' || c == '}'
                            })
                            .to_string();
                    }

                    if key == "name" {
                        tool_name = Some(val);
                    } else if !key.is_empty() {
                        if let Ok(num) = val.parse::<i64>() {
                            args_obj.insert(key, Value::Number(num.into()));
                        } else if let Ok(b) = val.parse::<bool>() {
                            args_obj.insert(key, Value::Bool(b));
                        } else {
                            args_obj.insert(key, Value::String(val));
                        }
                    }
                }

                if let Some(name) = tool_name {
                    out.push((name, Value::Object(args_obj)));
                }
            }

            out
        }
    }
}

pub fn parse_tool_calls(text: &str, protocol: crate::config::ToolProtocol) -> Vec<(String, Value)> {
    let raw_calls = parse_tool_calls_impl(text, protocol);
    let mut unique_calls = Vec::new();
    for call in raw_calls {
        if !unique_calls
            .iter()
            .any(|(n, a)| n == &call.0 && a == &call.1)
        {
            unique_calls.push(call);
        }
    }
    unique_calls
}

pub fn parse_tool_call(
    text: &str,
    protocol: crate::config::ToolProtocol,
) -> Option<(String, Value)> {
    parse_tool_calls(text, protocol).into_iter().next()
}

pub fn execute(name: &str, args: &Value) -> String {
    if let Ok(reg) = crate::mcp::get_mcp_registry().lock() {
        for client in reg.values() {
            if let Ok(tools) = client.get_tools() {
                if tools
                    .iter()
                    .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
                {
                    let handle = tokio::runtime::Handle::current();
                    let client_clone = Arc::clone(client);
                    let name_owned = name.to_string();
                    let args_clone = args.clone();

                    let res = handle.block_on(async move {
                        client_clone
                            .call(
                                "tools/call",
                                serde_json::json!({
                                    "name": name_owned,
                                    "arguments": args_clone
                                }),
                            )
                            .await
                    });

                    return match res {
                        Ok(val) => {
                            if let Some(content_arr) = val
                                .get("result")
                                .and_then(|r| r.get("content"))
                                .and_then(|c| c.as_array())
                            {
                                let mut text_parts = Vec::new();
                                for item in content_arr {
                                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                        text_parts.push(text.to_string());
                                    }
                                }
                                text_parts.join("\n")
                            } else {
                                serde_json::to_string_pretty(&val).unwrap_or_default()
                            }
                        }
                        Err(e) => format!("error: MCP tool call failed: {e}"),
                    };
                }
            }
        }
    }

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
        .and_then(parse_json_number)
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS);
    let env = args.get("env").and_then(|e| e.as_object());

    let resolved_cwd = match cwd {
        Some("sandbox") | Some("./sandbox") => {
            if let Some(session_id) = get_active_session_id() {
                crate::config::get_active_session_sandbox_dir(&session_id)
            } else {
                None
            }
        }
        Some(other) => Some(PathBuf::from(other)),
        None => None,
    };

    if let Some(ref cwd_path) = resolved_cwd {
        if !cwd_path.is_dir() {
            return Err(format!("cwd '{}' is not a directory", cwd_path.display()));
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
    if let Some(ref cwd_path) = resolved_cwd {
        cmd.current_dir(cwd_path);
    }
    if let Some(env_map) = env {
        for (k, v) in env_map {
            if let Some(val) = v.as_str() {
                cmd.env(k, val);
            }
        }
    }

    let run_in_bg = args
        .get("background")
        .and_then(parse_json_bool)
        .unwrap_or(false);
    if run_in_bg {
        let session_id = get_active_session_id().unwrap_or_default();
        let cmd_str = command_str.to_string();
        let task_id = format!(
            "task_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::from_secs(0))
                .as_millis()
        );

        let resolved_cwd_clone = resolved_cwd.clone();
        let env_clone = env.cloned();
        let task_id_clone = task_id.clone();

        if let Some(mut tasks) = get_background_tasks().lock().ok() {
            tasks.insert(
                task_id.clone(),
                BackgroundTaskInfo {
                    id: task_id.clone(),
                    command: cmd_str.clone(),
                    start_time: std::time::Instant::now(),
                    child_pid: None,
                    cancel_sender: None,
                },
            );
        }

        std::thread::spawn(move || {
            let mut cmd = if cfg!(target_os = "windows") {
                let mut c = std::process::Command::new("cmd");
                c.args(["/C", &cmd_str]);
                c
            } else {
                let mut c = std::process::Command::new("sh");
                c.args(["-c", &cmd_str]);
                c
            };

            cmd.stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());

            if let Some(ref cwd_path) = resolved_cwd_clone {
                cmd.current_dir(cwd_path);
            }
            if let Some(env_map) = env_clone {
                for (k, v) in env_map {
                    if let Some(val) = v.as_str() {
                        cmd.env(k, val);
                    }
                }
            }

            let result = match cmd.spawn() {
                Ok(child) => {
                    if let Some(pid) = Some(child.id()) {
                        if let Some(mut tasks) = get_background_tasks().lock().ok() {
                            if let Some(info) = tasks.get_mut(&task_id_clone) {
                                info.child_pid = Some(pid);
                            }
                        }
                    }

                    match child.wait_with_output() {
                        Ok(output) => {
                            let out_str = String::from_utf8_lossy(&output.stdout).to_string();
                            let err_str = String::from_utf8_lossy(&output.stderr).to_string();
                            let mut full = out_str;
                            if !err_str.is_empty() {
                                if !full.is_empty() {
                                    full.push('\n');
                                }
                                full.push_str("stderr:\n");
                                full.push_str(&err_str);
                            }
                            if output.status.success() {
                                Ok(full)
                            } else {
                                Err(format!("exit code {:?}\n{}", output.status.code(), full))
                            }
                        }
                        Err(e) => Err(format!("failed to wait: {e}")),
                    }
                }
                Err(e) => Err(format!("failed to spawn: {e}")),
            };

            if let Some(mut tasks) = get_background_tasks().lock().ok() {
                tasks.remove(&task_id_clone);
            }

            let output_str = match result {
                Ok(out) => out,
                Err(err) => err,
            };

            if let Some(cb) = WAKEUP_CALLBACK.get() {
                cb(session_id, task_id_clone, output_str);
            }
        });

        return Ok(format!(
            "Task started in background. Task ID: {task_id}. Status: Running."
        ));
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

    fn parse_tool_call(text: &str) -> Option<(String, Value)> {
        super::parse_tool_call(text, crate::config::ToolProtocol::Json)
    }

    fn parse_tool_calls(text: &str) -> Vec<(String, Value)> {
        super::parse_tool_calls(text, crate::config::ToolProtocol::Json)
    }

    #[test]
    fn test_resolve_tool_path() {
        set_active_session_id(Some("test_session_123".to_string()));
        let resolved = resolve_tool_path("sandbox/script.py");
        assert!(resolved.to_string_lossy().contains("test_session_123"));
        assert!(resolved.to_string_lossy().contains("sandbox"));
        assert!(resolved.to_string_lossy().ends_with("script.py"));

        let resolved_artifacts = resolve_tool_path("./artifacts/report.md");
        assert!(
            resolved_artifacts
                .to_string_lossy()
                .contains("test_session_123")
        );
        assert!(resolved_artifacts.to_string_lossy().contains("artifacts"));
        assert!(resolved_artifacts.to_string_lossy().ends_with("report.md"));

        let resolved_normal = resolve_tool_path("src/main.rs");
        assert_eq!(resolved_normal, Path::new("src/main.rs"));
        set_active_session_id(None);
    }

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
        assert_eq!(
            args.get("command").and_then(|c| c.as_str()),
            Some("echo hi")
        );

        // explicit tool protocol still wins over the shell fallback
        let tool = "```tool\n{\"name\":\"get_time\",\"arguments\":{}}\n```\n```bash\nls\n```";
        let (name, _) = parse_tool_call(tool).unwrap();
        assert_eq!(name, "get_time");
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        let text = "<tool_call>{\"name\": \"get_time\", \"arguments\": {}}</tool_call>\n\
                    <tool_call>{\"name\": \"view_file\", \"arguments\": {\"path\": \"main.rs\"}}</tool_call>";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "get_time");
        assert_eq!(calls[1].0, "view_file");
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
        let text = "<think>some thought</think>\n{\"name\": \"view_file\", \"arguments\": {\"path\": \"README.md\"}}";
        let (name, args) = parse_tool_call(text).unwrap();
        assert_eq!(name, "view_file");
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
    fn test_parse_tool_call_robust_fallback() {
        let text = "Here it is: {\"name\": \"get_time\", \"arguments\": {}} Hope this helps!";
        let res = parse_tool_calls(text);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, "get_time");

        let text = "First: {\"name\": \"get_time\", \"arguments\": {}} and second: {\"name\": \"view_file\", \"arguments\": {\"path\": \"x\"}}";
        let res = parse_tool_calls(text);
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].0, "get_time");
        assert_eq!(res[1].0, "view_file");
        assert_eq!(res[1].1.get("path").unwrap().as_str().unwrap(), "x");
    }

    #[test]
    fn test_parse_tool_call_pseudo_xml() {
        let text = "```tool<arg_key>name</arg_key><arg_value>view_file</arg_value><arg_key>path</arg_key><arg_value>src/main.rs</arg_value>```";
        let res = parse_tool_calls(text);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, "view_file");
        assert_eq!(
            res[0].1.get("path").unwrap().as_str().unwrap(),
            "src/main.rs"
        );

        let text_corrupted = "<arg_key>name</arg_key><arg_value>grep</arg_value>\
                              <arg_key>start_line\": \"1619\"</arg_value>\
                              <arg_key>end_line\": \"1640\"}";
        let res_corrupted = parse_tool_calls(text_corrupted);
        assert_eq!(res_corrupted.len(), 1);
        assert_eq!(res_corrupted[0].0, "grep");
        assert_eq!(
            res_corrupted[0]
                .1
                .get("start_line")
                .unwrap()
                .as_i64()
                .unwrap(),
            1619
        );
        assert_eq!(
            res_corrupted[0]
                .1
                .get("end_line")
                .unwrap()
                .as_i64()
                .unwrap(),
            1640
        );
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



    #[test]
    fn test_system_prompt_lists_tools() {
        let p = tool_system_prompt(true, crate::config::ToolProtocol::Json);
        for t in TOOLS {
            assert!(p.contains(t.name));
        }
        assert!(p.contains("spawn_agent"));
        assert!(p.contains("send_agent"));
    }

    #[test]
    fn test_system_prompt_without_agent_tools() {
        let p = tool_system_prompt(false, crate::config::ToolProtocol::Json);
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
    fn test_view_file_slicing() {
        let path = std::env::temp_dir().join(format!("rustcode-view-{}", std::process::id()));
        std::fs::write(&path, "line 1\nline 2\nline 3\nline 4\nline 5\n").unwrap();

        let out = execute(
            "view_file",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "start_line": 2,
                "end_line": 4
            }),
        );
        assert!(out.contains("Lines 2 to 4 of 5"), "got: {out}");
        assert!(out.contains("2: line 2"), "got: {out}");
        assert!(out.contains("3: line 3"), "got: {out}");
        assert!(out.contains("4: line 4"), "got: {out}");
        assert!(!out.contains("1: line 1"), "got: {out}");
        assert!(!out.contains("5: line 5"), "got: {out}");

        let out_offset = execute(
            "view_file",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "start_line": 1,
                "end_line": 2,
                "content_offset": 7
            }),
        );
        assert!(out_offset.contains("1: line 2"), "got: {out_offset}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_replace_file_content_matching() {
        let path = std::env::temp_dir().join(format!("rustcode-replace-{}", std::process::id()));
        std::fs::write(&path, "line 1\nline 2\nline 3\nline 4\n").unwrap();

        let out = execute(
            "replace_file_content",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "start_line": 2,
                "end_line": 3,
                "target_content": "line 2\nline 3",
                "replacement_content": "line 2_new\nline 3_new"
            }),
        );
        assert!(out.contains("successfully replaced"), "got: {out}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line 1\nline 2_new\nline 3_new\nline 4\n"
        );

        let out_fail = execute(
            "replace_file_content",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "start_line": 2,
                "end_line": 3,
                "target_content": "wrong content",
                "replacement_content": "whatever"
            }),
        );
        assert!(out_fail.contains("error: Discrepancy"), "got: {out_fail}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_multi_replace_file_descending_order() {
        let path = std::env::temp_dir().join(format!("rustcode-multi-{}", std::process::id()));
        std::fs::write(&path, "line 1\nline 2\nline 3\nline 4\n").unwrap();

        let out = execute(
            "multi_replace_file_content",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "replacements": [
                    {
                        "start_line": 2,
                        "end_line": 2,
                        "target_content": "line 2",
                        "replacement_content": "line 2_new"
                    },
                    {
                        "start_line": 4,
                        "end_line": 4,
                        "target_content": "line 4",
                        "replacement_content": "line 4_new"
                    }
                ]
            }),
        );
        assert!(
            out.contains("successfully applied 2 replacements"),
            "got: {out}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "line 1\nline 2_new\nline 3\nline 4_new\n"
        );

        let out_overlap = execute(
            "multi_replace_file_content",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "replacements": [
                    {
                        "start_line": 2,
                        "end_line": 3,
                        "target_content": "line 2_new\nline 3",
                        "replacement_content": "x"
                    },
                    {
                        "start_line": 3,
                        "end_line": 4,
                        "target_content": "line 3\nline 4_new",
                        "replacement_content": "y"
                    }
                ]
            }),
        );
        assert!(
            out_overlap.contains("overlapping replacement ranges"),
            "got: {out_overlap}"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_write_to_file_overwrite_prevention() {
        let path = std::env::temp_dir().join(format!("rustcode-write-to-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let out = execute(
            "write_to_file",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "content": "hello"
            }),
        );
        assert!(out.contains("wrote"), "got: {out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        let out_fail = execute(
            "write_to_file",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "content": "new",
                "overwrite": false
            }),
        );
        assert!(out_fail.contains("already exists"), "got: {out_fail}");

        let out_overwrite = execute(
            "write_to_file",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "content": "new",
                "overwrite": true
            }),
        );
        assert!(out_overwrite.contains("wrote"), "got: {out_overwrite}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_complete_task_tool() {
        let out = execute(
            "complete_task",
            &serde_json::json!({
                "result": "Fixed 3 bugs and compiled successfully."
            }),
        );
        assert!(out.contains("marked as complete"), "got: {out}");
        assert!(out.contains("Fixed 3 bugs and compiled successfully."), "got: {out}");

        // Missing argument
        let out_fail = execute("complete_task", &serde_json::json!({}));
        assert!(out_fail.contains("missing 'result'"), "got: {out_fail}");
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
        assert!(!needs_confirmation("view_file"));
        assert!(needs_confirmation("write_to_file"));
        assert!(needs_confirmation("replace_file_content"));
        assert!(needs_confirmation("multi_replace_file_content"));
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

struct ReplacementChunk {
    start_line: usize,
    end_line: usize,
    target_content: String,
    replacement_content: String,
}

fn view_file_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let resolved_path = resolve_tool_path(path);
    let start_line = args
        .get("start_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(1);
    let end_line = args
        .get("end_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(start_line + 500);

    let content_bytes =
        std::fs::read(&resolved_path).map_err(|e| format!("cannot read '{path}': {e}"))?;

    let byte_offset = args
        .get("content_offset")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(0);

    if byte_offset >= content_bytes.len() && !content_bytes.is_empty() {
        return Err(format!(
            "content_offset {} exceeds file size {}",
            byte_offset,
            content_bytes.len()
        ));
    }

    let sliced_content = String::from_utf8_lossy(&content_bytes[byte_offset..]);
    let lines: Vec<&str> = sliced_content.lines().collect();
    let total = lines.len();

    if total == 0 {
        return Ok(format!(
            "[File: {}, Empty file, Bytes offset: {}]",
            path, byte_offset
        ));
    }

    if start_line < 1 || start_line > total {
        return Err(format!(
            "start_line {} is out of bounds (1 to {})",
            start_line, total
        ));
    }

    let actual_end = end_line.min(total);
    let mut out = format!(
        "[File: {}, Lines {} to {} of {}, Bytes offset: {}]\n",
        path, start_line, actual_end, total, byte_offset
    );

    for (idx, line) in lines[start_line - 1..actual_end].iter().enumerate() {
        out.push_str(&format!("{}: {}\n", start_line + idx, line));
    }

    if actual_end < total {
        out.push_str("... content truncated (use end_line or content_offset to read more) ...\n");
    }

    Ok(out)
}

fn replace_file_content_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let start_line = args
        .get("start_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .ok_or("missing 'start_line' argument")?;
    let end_line = args
        .get("end_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .ok_or("missing 'end_line' argument")?;
    let target_content = args
        .get("target_content")
        .and_then(|t| t.as_str())
        .ok_or("missing 'target_content' argument")?;
    let replacement_content = args
        .get("replacement_content")
        .and_then(|r| r.as_str())
        .ok_or("missing 'replacement_content' argument")?;

    let resolved_path = resolve_tool_path(path);
    let content = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?;

    let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let total = file_lines.len();

    if start_line < 1 || start_line > total || end_line < start_line || end_line > total {
        return Err(format!(
            "line range {start_line}-{end_line} is out of bounds (1-{total})"
        ));
    }

    let segment = file_lines[start_line - 1..end_line].join("\n");
    if segment.trim_end() != target_content.trim_end() {
        let mut mismatch = format!(
            "Discrepancy: target_content does not match file contents at {start_line}-{end_line}.\n"
        );
        mismatch.push_str("=== Expected (target_content) ===\n");
        mismatch.push_str(target_content);
        mismatch.push_str("\n=== Found in File ===\n");
        mismatch.push_str(&segment);
        mismatch.push_str("\n======================\n");
        return Err(mismatch);
    }

    let has_trailing_newline = content.ends_with('\n');
    let mut new_lines = Vec::new();
    new_lines.extend_from_slice(&file_lines[..start_line - 1]);
    new_lines.push(replacement_content.to_string());
    new_lines.extend_from_slice(&file_lines[end_line..]);

    let mut new_content = new_lines.join("\n");
    if has_trailing_newline && !new_content.is_empty() {
        new_content.push('\n');
    }
    std::fs::write(&resolved_path, &new_content)
        .map_err(|e| format!("cannot write '{path}': {e}"))?;

    Ok(format!(
        "successfully replaced lines {start_line}-{end_line} in '{path}'"
    ))
}

fn multi_replace_file_content_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let replacements_val = args
        .get("replacements")
        .and_then(|r| r.as_array())
        .ok_or("missing 'replacements' array")?;

    let resolved_path = resolve_tool_path(path);
    let content = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?;

    let mut file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    let mut chunks = Vec::new();
    for (i, r_val) in replacements_val.iter().enumerate() {
        let obj = r_val
            .as_object()
            .ok_or(format!("replacement at index {i} is not an object"))?;
        let start_line = obj
            .get("start_line")
            .and_then(parse_json_number)
            .map(|v| v as usize)
            .ok_or(format!("replacement index {i} missing 'start_line'"))?;
        let end_line = obj
            .get("end_line")
            .and_then(parse_json_number)
            .map(|v| v as usize)
            .ok_or(format!("replacement index {i} missing 'end_line'"))?;
        let target_content = obj
            .get("target_content")
            .and_then(|t| t.as_str())
            .ok_or(format!("replacement index {i} missing 'target_content'"))?;
        let replacement_content = obj
            .get("replacement_content")
            .and_then(|r| r.as_str())
            .ok_or(format!(
                "replacement index {i} missing 'replacement_content'"
            ))?;
        chunks.push(ReplacementChunk {
            start_line,
            end_line,
            target_content: target_content.to_string(),
            replacement_content: replacement_content.to_string(),
        });
    }

    chunks.sort_by(|a, b| b.start_line.cmp(&a.start_line));

    // Verify all ranges are disjoint
    for idx in 0..chunks.len().saturating_sub(1) {
        if chunks[idx].start_line <= chunks[idx + 1].end_line {
            return Err(format!(
                "overlapping replacement ranges: range {}-{} overlaps with range {}-{}",
                chunks[idx + 1].start_line,
                chunks[idx + 1].end_line,
                chunks[idx].start_line,
                chunks[idx].end_line
            ));
        }
    }

    // Validate matching target contents
    for (i, chunk) in chunks.iter().enumerate() {
        let total = file_lines.len();
        if chunk.start_line < 1
            || chunk.start_line > total
            || chunk.end_line < chunk.start_line
            || chunk.end_line > total
        {
            return Err(format!(
                "replacement index {i} range {}-{} is out of bounds (1-{})",
                chunk.start_line, chunk.end_line, total
            ));
        }
        let segment = file_lines[chunk.start_line - 1..chunk.end_line].join("\n");
        if segment.trim_end() != chunk.target_content.trim_end() {
            let mut mismatch = format!(
                "Discrepancy at replacement index {i} (lines {}-{}), target_content does not match file.\n",
                chunk.start_line, chunk.end_line
            );
            mismatch.push_str("=== Expected (target_content) ===\n");
            mismatch.push_str(&chunk.target_content);
            mismatch.push_str("\n=== Found in File ===\n");
            mismatch.push_str(&segment);
            mismatch.push_str("\n======================\n");
            return Err(mismatch);
        }
    }

    // Apply descending edits
    for chunk in chunks {
        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&file_lines[..chunk.start_line - 1]);
        new_lines.push(chunk.replacement_content);
        new_lines.extend_from_slice(&file_lines[chunk.end_line..]);
        file_lines = new_lines;
    }

    let has_trailing_newline = content.ends_with('\n');
    let mut new_content = file_lines.join("\n");
    if has_trailing_newline && !new_content.is_empty() {
        new_content.push('\n');
    }
    std::fs::write(&resolved_path, &new_content)
        .map_err(|e| format!("cannot write '{path}': {e}"))?;

    Ok(format!(
        "successfully applied {} replacements to '{path}'",
        replacements_val.len()
    ))
}

fn write_to_file_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let content = args
        .get("content")
        .and_then(|c| c.as_str())
        .ok_or("missing 'content' argument")?;
    let overwrite = args
        .get("overwrite")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);

    let resolved_path = resolve_tool_path(path);
    if resolved_path.exists() && !overwrite {
        return Err(format!(
            "'{path}' already exists — set 'overwrite' to true to allow overwriting"
        ));
    }

    if let Some(parent) = resolved_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directories for '{path}': {e}"))?;
    }

    std::fs::write(&resolved_path, content).map_err(|e| format!("cannot write '{path}': {e}"))?;

    let lines = content.lines().count().max(1);
    Ok(format!(
        "wrote '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}

fn complete_task_tool(args: &Value) -> Result<String, String> {
    let result = args
        .get("result")
        .and_then(|r| r.as_str())
        .ok_or("missing 'result' argument")?;
    Ok(format!("Task successfully marked as complete! Result: {result}"))
}
