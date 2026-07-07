use serde_json::Value;

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
];

/// Maximum tool-call rounds per user prompt, so a confused model
/// can't loop forever.
pub const MAX_TOOL_ROUNDS: usize = 4;

fn get_time(_args: &Value) -> Result<String, String> {
    Ok(chrono::Local::now()
        .format("%A %Y-%m-%d %H:%M:%S")
        .to_string())
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
        Ok(format!("'{path}' is empty"))
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

pub fn tool_system_prompt() -> String {
    let mut p = String::from(
        "You have access to tools. To use one, reply with ONLY a tool call in \
         this exact format, nothing else:\n\
         <tool_call>{\"name\": \"tool_name\", \"arguments\": {}}</tool_call>\n\n\
         Available tools:\n",
    );
    for t in TOOLS {
        p.push_str(&format!(
            "- {}: {}. Arguments: {}\n",
            t.name, t.description, t.arguments
        ));
    }
    p.push_str(
        "\nThe tool result will be sent back to you in a <tool_result> block. \
         After receiving it, answer the user normally in plain text. \
         Only call a tool when it is actually needed to answer. \
         If a tool returns an error, retry with corrected arguments or a \
         more suitable tool instead of giving up.",
    );
    p
}

pub fn parse_tool_call(text: &str) -> Option<(String, Value)> {
    let start = text.find("<tool_call>")? + "<tool_call>".len();
    let end = text[start..].find("</tool_call>")? + start;
    let json: Value = serde_json::from_str(text[start..end].trim()).ok()?;
    let name = json.get("name")?.as_str()?.to_string();
    let args = json
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    Some((name, args))
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
    fn test_parse_rejects_plain_text() {
        assert!(parse_tool_call("just a normal reply").is_none());
        assert!(parse_tool_call("<tool_call>not json</tool_call>").is_none());
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
        let p = tool_system_prompt();
        for t in TOOLS {
            assert!(p.contains(t.name));
        }
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
    fn test_needs_confirmation() {
        assert!(!needs_confirmation("get_time"));
        assert!(!needs_confirmation("list_directory"));
        assert!(!needs_confirmation("read_file"));
        assert!(needs_confirmation("create_file"));
        assert!(needs_confirmation("write_file"));
        assert!(!needs_confirmation("nonexistent"));
    }
}
