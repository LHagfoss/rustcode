const MAX_TOOL_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_TOOL_OUTPUT_LINES: usize = 1000;

/// Truncate tool output at execution time if it exceeds size limits.
/// Full output is saved to a temp file so the agent can still access it.
pub(crate) fn truncate_tool_output(name: &str, result: String) -> String {
    let bytes = result.len();
    let lines: Vec<&str> = result.lines().collect();
    let line_count = lines.len();

    if bytes <= MAX_TOOL_OUTPUT_BYTES && line_count <= MAX_TOOL_OUTPUT_LINES {
        return result;
    }

    let saved_path = save_full_tool_output(name, &result);
    let max_lines = MAX_TOOL_OUTPUT_LINES.min(line_count);
    let head_count = (max_lines * 3) / 10;
    let tail_count = (max_lines * 3) / 10;

    let head: String = lines[..head_count.min(line_count)].join("\n");
    let tail: String = if tail_count > 0 && line_count > head_count + tail_count {
        lines[line_count - tail_count..].join("\n")
    } else {
        String::new()
    };

    let omitted_lines = line_count.saturating_sub(head_count + tail_count);
    let omitted_bytes = bytes.saturating_sub(head.len() + tail.len());
    let path_note = match saved_path {
        Some(path) => format!(
            " Full output saved to: {path}\nUse grep to search the full content or view_file with line offsets to read specific sections."
        ),
        None => String::new(),
    };

    format!(
        "{head}\n\n... [{omitted_lines} lines / {omitted_bytes} bytes truncated] ...\n\n{tail}\n\n[Output truncated: {bytes} bytes total, {line_count} lines.{path_note}]"
    )
}

fn save_full_tool_output(name: &str, content: &str) -> Option<String> {
    let dir = crate::config::get_config_dir()?.join("tool_output");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::from_secs(0))
        .as_millis();
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let path = dir.join(format!("{ts}_{safe_name}.txt"));
    match std::fs::write(&path, content) {
        Ok(_) => Some(path.to_string_lossy().to_string()),
        Err(_) => None,
    }
}
