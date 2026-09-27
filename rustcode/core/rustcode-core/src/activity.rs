//! Tool classification and call summarization shared by frontends.
//!
//! Pure string/JSON helpers: which tools explore vs edit, safe parameter
//! rendering (never raw prompts or secrets), and allowlisted exploration
//! summaries. Live tracking stays in the engine; these values do not.

pub fn is_exploration_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "view_file"
            | "viewfile"
            | "read_file"
            | "readfile"
            | "list_directory"
            | "list_dir"
            | "listdir"
            | "glob"
            | "grep"
            | "grep_search"
            | "grepsearch"
            | "find_symbol"
            | "findsymbol"
            | "codebase_search"
            | "codebasesearch"
            | "codebase_symbol"
            | "codebasesymbol"
            | "get_project_map"
            | "getprojectmap"
    )
}

pub fn is_editing_tool(tool_name: &str) -> bool {
    let lower = tool_name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "replace_file_content"
            | "replacefilecontent"
            | "multi_replace_file_content"
            | "multireplacefilecontent"
            | "write_to_file"
            | "write_file_chunk"
            | "writetofile"
            | "write_file"
            | "writefile"
            | "edit_file"
            | "editfile"
            | "create_file"
            | "createfile"
            | "patch_file"
            | "patchfile"
            | "delete_file"
            | "deletefile"
            | "move_file"
            | "movefile"
            | "copy_file"
            | "copyfile"
            | "generate_sound_effect"
            | "generate_music"
            | "render_video"
    )
}

pub fn sanitize_tool_parameter(raw: &str, max_chars: usize) -> String {
    let compacted = raw
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut value = compacted.chars().take(max_chars).collect::<String>();
    if compacted.chars().count() > max_chars {
        value.push('…');
    }
    value
}

pub fn safe_parameter(key: &str, value: &str, max_chars: usize) -> String {
    let key = key.to_ascii_lowercase();
    let value_lower = value.to_ascii_lowercase();
    if key.contains("password")
        || key.contains("secret")
        || key.contains("credential")
        || key.contains("token")
        || key.contains("api_key")
        || key.contains("authorization")
        || key.contains("prompt")
        || key.contains("content")
        || value_lower.contains("bearer ")
        || value_lower.contains("sk-")
        || value_lower.contains("ghp_")
    {
        "[redacted]".to_owned()
    } else {
        sanitize_tool_parameter(value, max_chars)
    }
}

fn string_arg<'a, 'b>(
    args: &'a serde_json::Value,
    keys: &'b [&'b str],
) -> Option<(&'b str, &'a str)> {
    keys.iter().find_map(|key| {
        args.get(*key)
            .and_then(|value| value.as_str())
            .map(|value| (*key, value))
    })
}

fn path_with_home(path: &str, home_path: Option<&str>) -> String {
    let path = if let Some(home) = home_path {
        path.strip_prefix(home)
            .map(|suffix| format!("~{suffix}"))
            .unwrap_or_else(|| path.to_owned())
    } else {
        path.to_owned()
    };
    safe_parameter("path", &path, 100)
}

/// Render only the allowlisted exploration arguments shared by live and
/// committed tool summaries.
pub fn exploration_tool_parameters(
    name: &str,
    args: &serde_json::Value,
    home_path: Option<&str>,
) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let path = string_arg(
        args,
        &[
            "TargetFile",
            "target_file",
            "AbsolutePath",
            "absolute_path",
            "DirectoryPath",
            "directory_path",
            "SearchPath",
            "search_path",
            "path",
            "file",
            "filePath",
            "filepath",
        ],
    )
    .map(|(_, value)| path_with_home(value, home_path));
    match lower.as_str() {
        "view_file" | "viewfile" | "read_file" | "readfile" => {
            let path = path.unwrap_or_else(|| "?".to_owned());
            let start = ["start_line", "StartLine", "startLine"]
                .iter()
                .find_map(|key| args.get(*key).and_then(|value| value.as_u64()));
            let end = ["end_line", "EndLine", "endLine"]
                .iter()
                .find_map(|key| args.get(*key).and_then(|value| value.as_u64()));
            Some(match (start, end) {
                (Some(start), Some(end)) => format!("{path} (lines {start}-{end})"),
                (Some(start), None) => format!("{path} (line {start})"),
                _ => path,
            })
        }
        "list_directory" | "list_dir" | "listdir" | "glob" => {
            let pattern = string_arg(args, &["pattern", "glob"])
                .map(|(key, value)| safe_parameter(key, value, 80));
            Some(match (path, pattern) {
                (Some(path), Some(pattern)) if pattern != path => format!("{path} ({pattern})"),
                (Some(path), _) => path,
                (None, Some(pattern)) => pattern,
                _ => ".".to_owned(),
            })
        }
        "grep" | "grep_search" | "grepsearch" => {
            let (pattern_key, pattern) =
                string_arg(args, &["Query", "query", "pattern", "Pattern"])
                    .unwrap_or(("pattern", "?"));
            let pattern = safe_parameter(pattern_key, pattern, 80);
            let mut summary = match path {
                Some(path) if path != "." => format!("{pattern} in {path}"),
                _ => pattern,
            };
            if let Some((key, include)) = string_arg(args, &["include", "Include", "glob", "Glob"])
            {
                summary.push_str(&format!(" ({} {})", key, safe_parameter(key, include, 50)));
            }
            if args
                .get("ignore_case")
                .or_else(|| args.get("IgnoreCase"))
                .or_else(|| args.get("case_insensitive"))
                .and_then(|value| value.as_bool())
                == Some(true)
            {
                summary.push_str(" (case-insensitive)");
            }
            Some(sanitize_tool_parameter(&summary, 140))
        }
        "find_symbol" | "findsymbol" | "codebase_search" | "codebasesearch" | "codebase_symbol"
        | "codebasesymbol" => {
            let (key, query) =
                string_arg(args, &["query", "Query", "symbol"]).unwrap_or(("query", "?"));
            Some(safe_parameter(key, query, 100))
        }
        "get_project_map" | "getprojectmap" => Some("project map".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exploration_summaries_are_bounded_and_include_safe_navigation_parameters() {
        let view = exploration_tool_parameters(
            "view_file",
            &serde_json::json!({"path": "/workspace/src/lib.rs", "start_line": 10, "end_line": 20}),
            Some("/workspace"),
        )
        .unwrap();
        assert_eq!(view, "~/src/lib.rs (lines 10-20)");
    }
}
