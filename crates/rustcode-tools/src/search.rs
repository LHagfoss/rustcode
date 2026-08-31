use serde_json::Value;

const MAX_LIST_ENTRIES: usize = 10_000;

/// List a directory for `view_file`'s directory fallback.
pub fn list_directory(args: &Value) -> Result<String, String> {
    list_directory_with_context(args, &super::active_context())
}

/// List a directory using an explicit workspace/session context.
pub fn list_directory_with_context(
    args: &Value,
    context: &super::ToolContext,
) -> Result<String, String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved_path = super::resolve_tool_path_with_context(path, context);

    if resolved_path.is_file() {
        return Err(format!(
            "'{path}' is a file, not a directory - use the read_file tool instead"
        ));
    }
    let entries = std::fs::read_dir(&resolved_path)
        .map_err(|error| format!("cannot read '{path}': {error}"))?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let mut name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
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
