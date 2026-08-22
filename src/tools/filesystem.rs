use serde_json::Value;
use std::path::PathBuf;

// Re-exports needed by filesystem tools
pub(crate) use super::coerce_array;
pub(crate) use super::parse_json_number;
pub(crate) use super::resolve_tool_path;

use super::{Tool, ToolCapability, ToolSafety};

fn delete_file_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"]
    })
}

pub const DELETE_FILE: Tool = Tool {
    name: "delete_file",
    description: "Delete a file from the filesystem",
    arguments: r#"{"path": "file to delete"}"#,
    handler: delete_file,
    requires_confirmation: true,
    schema: delete_file_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn move_file_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "src": { "type": "string" }, "dest": { "type": "string" }
        }, "required": ["src", "dest"]
    })
}

pub const MOVE_FILE: Tool = Tool {
    name: "move_file",
    description: "Move or rename a file or directory to a new path",
    arguments: r#"{"src": "source path", "dest": "destination path"}"#,
    handler: move_file,
    requires_confirmation: true,
    schema: move_file_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn copy_file_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "src": { "type": "string" }, "dest": { "type": "string" }
        }, "required": ["src", "dest"]
    })
}

pub const COPY_FILE: Tool = Tool {
    name: "copy_file",
    description: "Copy a file to a new path",
    arguments: r#"{"src": "source path to copy", "dest": "destination path"}"#,
    handler: copy_file,
    requires_confirmation: true,
    schema: copy_file_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn view_file_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "path": { "type": "string" }, "start_line": { "type": "integer", "minimum": 1 },
            "end_line": { "type": "integer", "minimum": 1, "description": "Inclusive end line; each call is capped at 800 lines. Request targeted follow-up ranges for more content." },
            "content_offset": { "type": "integer", "minimum": 0 }
        }, "required": ["path"]
    })
}

pub const VIEW_FILE: Tool = Tool {
    name: "view_file",
    description: "View the contents of a file or directory. Each call has a 800-line hard cap; request targeted follow-up ranges with start_line/end_line for more content. Supports 1-indexed line ranges and an optional byte offset.",
    arguments: r#"{"path": "absolute or relative path to file or directory", "start_line": "optional start line number, 1-indexed (default 1)", "end_line": "optional end line number, 1-indexed (each call is capped at 800 lines; request targeted follow-up ranges for more content)", "content_offset": "optional byte offset into content"}"#,
    handler: view_file_tool,
    requires_confirmation: false,
    schema: view_file_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn replace_file_content_schema() -> Value {
    serde_json::json!({
        "type": "object", "additionalProperties": false, "properties": {
            "path": { "type": "string", "description": "Absolute or relative path to file" },
            "old_string": { "type": "string", "description": "Precise block of code to edit (or target_content)" },
            "new_string": { "type": "string", "description": "Complete replacement text (or replacement_content)" },
            "target_content": { "type": "string", "description": "Alias for old_string" },
            "replacement_content": { "type": "string", "description": "Alias for new_string" },
            "target": { "type": "string", "description": "Alias for old_string" },
            "replacement": { "type": "string", "description": "Alias for new_string" },
            "old_text": { "type": "string", "description": "Alias for old_string" },
            "new_text": { "type": "string", "description": "Alias for new_string" },
            "oldString": { "type": "string", "description": "Alias for old_string" },
            "newString": { "type": "string", "description": "Alias for new_string" },
            "oldText": { "type": "string", "description": "Alias for old_string" },
            "newText": { "type": "string", "description": "Alias for new_string" },
            "start_line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed start line to anchor the edit" },
            "end_line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed end line to anchor the edit" },
            "edits": { "type": "array", "items": { "type": "object", "properties": {
                "old_string": { "type": "string" }, "new_string": { "type": "string" },
                "target_content": { "type": "string" }, "replacement_content": { "type": "string" },
                "target": { "type": "string" }, "replacement": { "type": "string" },
                "old_text": { "type": "string" }, "new_text": { "type": "string" },
                "oldString": { "type": "string" }, "newString": { "type": "string" },
                "oldText": { "type": "string" }, "newText": { "type": "string" },
                "start_line": { "type": "integer" }, "end_line": { "type": "integer" }
            } } }
        }, "required": ["path"]
    })
}

pub const REPLACE_FILE_CONTENT: Tool = Tool {
    name: "replace_file_content",
    description: "Surgically edit code in an existing file. Supports single replacement (target_content/replacement_content or old_string/new_string) or array of batch edits (edits: [{old_string, new_string}]). Line numbers are optional. This tool only replaces: to INSERT text, target an existing neighbouring line and repeat it in the replacement — to prepend, target the current first line and replace it with the new text followed by that line. An empty target is rejected, since it matches everywhere.",
    arguments: r#"{"path": "absolute or relative path to file", "target_content": "precise block of code to edit (or old_string) — never empty; to insert, anchor on an adjacent line and repeat it in the replacement", "replacement_content": "complete replacement text (or new_string)", "edits": "optional array of [{old_string, new_string}] for multiple edits in 1 call"}"#,
    handler: replace_file_content_tool,
    requires_confirmation: true,
    schema: replace_file_content_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn multi_replace_file_content_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "path": { "type": "string" }, "replacements": { "type": "array", "items": { "type": "object", "properties": {
                "start_line": { "type": "integer" }, "end_line": { "type": "integer" },
                "target_content": { "type": "string" }, "replacement_content": { "type": "string" }
            }, "required": ["start_line", "end_line", "target_content", "replacement_content"] } }
        }, "required": ["path", "replacements"]
    })
}

pub const MULTI_REPLACE_FILE_CONTENT: Tool = Tool {
    name: "multi_replace_file_content",
    description: "Apply multiple non-contiguous edits across a single file in a single tool call.                       Specify each edit as a separate replacement chunk.",
    arguments: r#"{"path": "absolute or relative path to file", "replacements": "array of objects, each containing: {start_line, end_line, target_content, replacement_content}"}"#,
    handler: multi_replace_file_content_tool,
    requires_confirmation: true,
    schema: multi_replace_file_content_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn write_to_file_schema() -> Value {
    serde_json::json!({
        "type": "object", "properties": {
            "path": { "type": "string" }, "content": { "type": "string" },
            "overwrite": { "type": "boolean", "default": true }
        }, "required": ["path", "content"]
    })
}

pub const WRITE_TO_FILE: Tool = Tool {
    name: "write_to_file",
    description: "Create a new file or overwrite an existing file with complete content.                       Creates parent directories automatically.",
    arguments: r#"{"path": "absolute or relative path to file", "content": "entire contents to write", "overwrite": "optional boolean, defaults to true to allow overwriting an existing file"}"#,
    handler: write_to_file_tool,
    requires_confirmation: true,
    schema: write_to_file_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

/// Hard cap on the number of lines a single `view_file` call can return,
/// applied both as the default window (when the caller omits `end_line`) and
/// as a ceiling on any explicit `end_line` the caller requests. Without this
/// cap a model asking for a huge explicit range (or relying on a large
/// default) could pull thousands of lines / tens of KB into one tool result,
/// which blows up the transcript across many rounds long before any
/// downstream byte truncation kicks in. Reads that hit this window are
/// genuinely truncated (see `view_file_tool`'s truncation message) — as
/// opposed to a read that stopped exactly where the caller's own `end_line`
/// asked it to.
const DEFAULT_READ_WINDOW_LINES: usize = 800;

struct ReplacementChunk {
    start_line: usize,
    end_line: usize,
    target_content: String,
    replacement_content: String,
}

pub(super) struct ViewFileOutput {
    pub(super) content: String,
    pub(super) truncated: bool,
}

fn resolve(path: &str) -> PathBuf {
    resolve_tool_path(path)
}

pub fn delete_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let resolved_path = resolve(path);
    if !resolved_path.exists() {
        // Idempotent: a missing file is already in the desired state. Returning an
        // error here used to derail the agent into confusion mid-task.
        return Ok(format!("'{path}' does not exist (already gone)"));
    }
    if resolved_path.is_dir() {
        return Err(format!(
            "'{path}' is a directory — use delete_dir if needed (not supported yet)"
        ));
    }
    std::fs::remove_file(&resolved_path).map_err(|e| format!("cannot delete '{path}': {e}"))?;
    Ok(format!("deleted '{path}'"))
}

pub fn move_file(args: &Value) -> Result<String, String> {
    let src = args
        .get("src")
        .and_then(|s| s.as_str())
        .ok_or("missing 'src' argument")?;
    let dest = args
        .get("dest")
        .and_then(|d| d.as_str())
        .ok_or("missing 'dest' argument")?;
    let resolved_src = resolve(src);
    let resolved_dest = resolve(dest);
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

pub fn copy_file(args: &Value) -> Result<String, String> {
    let src = args
        .get("src")
        .and_then(|s| s.as_str())
        .ok_or("missing 'src' argument")?;
    let dest = args
        .get("dest")
        .and_then(|d| d.as_str())
        .ok_or("missing 'dest' argument")?;
    let resolved_src = resolve(src);
    let resolved_dest = resolve(dest);
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

pub fn view_file_tool(args: &Value) -> Result<String, String> {
    view_file_output(args).map(|output| output.content)
}

pub(super) fn view_file_output(args: &Value) -> Result<ViewFileOutput, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let resolved_path = resolve(path);
    if resolved_path.is_dir() {
        return super::search::list_directory(args).map(|content| ViewFileOutput {
            content,
            truncated: false,
        });
    }
    let parse_index = |name: &str| -> Result<Option<usize>, String> {
        let Some(value) = args.get(name) else {
            return Ok(None);
        };
        let number = parse_json_number(value)
            .ok_or_else(|| format!("{name} must be a non-negative integer"))?;
        usize::try_from(number)
            .map(Some)
            .map_err(|_| format!("{name} is too large for this platform"))
    };
    let requested_start = parse_index("start_line")?;
    let requested_end = parse_index("end_line")?;
    if requested_start == Some(0) {
        return Err("start_line must be at least 1".to_string());
    }
    if requested_end == Some(0) {
        return Err("end_line must be at least 1".to_string());
    }
    if let (Some(start_line), Some(end_line)) = (requested_start, requested_end)
        && end_line < start_line
    {
        return Err(format!(
            "end_line {end_line} must be greater than or equal to start_line {start_line}"
        ));
    }

    let content_bytes =
        std::fs::read(&resolved_path).map_err(|e| format!("cannot read '{path}': {e}"))?;

    let byte_offset = parse_index("content_offset")?.unwrap_or(0);

    if byte_offset > content_bytes.len()
        || (byte_offset == content_bytes.len() && !content_bytes.is_empty())
    {
        return Err(format!(
            "content_offset {} exceeds file size {}",
            byte_offset,
            content_bytes.len()
        ));
    }
    if content_bytes
        .get(byte_offset)
        .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
    {
        return Err(format!(
            "content_offset {byte_offset} is not at a valid UTF-8 boundary"
        ));
    }

    let sliced_content = String::from_utf8_lossy(&content_bytes[byte_offset..]);
    let lines: Vec<&str> = sliced_content.lines().collect();
    let line_number_offset = content_bytes[..byte_offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count();
    let total = line_number_offset + lines.len();

    if total == 0 {
        if requested_start.is_some() || requested_end.is_some() {
            return Err(
                "requested line range is out of bounds because the sliced content is empty"
                    .to_string(),
            );
        }
        return Ok(ViewFileOutput {
            content: format!(
                "[File: {}, Empty file, Bytes offset: {}]",
                path, byte_offset
            ),
            truncated: false,
        });
    }

    let start_line = requested_start.unwrap_or(line_number_offset + 1);
    if let Some(end_line) = requested_end
        && end_line < start_line
    {
        return Err(format!(
            "end_line {end_line} must be greater than or equal to start_line {start_line}"
        ));
    }
    let max_end = start_line.saturating_add(DEFAULT_READ_WINDOW_LINES - 1);
    // A caller-supplied end_line beyond the hard cap is bounded, not honored —
    // otherwise a single explicit huge range (e.g. end_line: 999999999) would
    // bypass the safety window entirely.
    let cap_applied = requested_end.is_some_and(|e| e > max_end);
    let end_line = requested_end.map(|e| e.min(max_end)).unwrap_or(max_end);

    let first_available_line = line_number_offset + 1;
    if start_line < first_available_line || start_line > total {
        return Err(format!(
            "start_line {} is out of bounds ({} to {})",
            start_line, first_available_line, total
        ));
    }

    let actual_end = end_line.min(total);
    let mut out = format!(
        "[File: {}, Lines {} to {} of {}, Bytes offset: {}]\n",
        path, start_line, actual_end, total, byte_offset
    );

    let slice_start = start_line - line_number_offset - 1;
    let slice_end = actual_end - line_number_offset;
    for (idx, line) in lines[slice_start..slice_end].iter().enumerate() {
        out.push_str(&format!("{}: {}\n", start_line + idx, line));
    }

    if actual_end < total {
        // A read that stopped where the caller asked it to is not truncated.
        // Calling it truncated tells the model it is missing something, and a
        // model that thinks it is missing something reads the file again — which
        // is exactly the loop this produced when a one-line read reported itself
        // as cut short.
        if cap_applied {
            // The caller asked for an explicit end_line beyond the hard cap.
            // Bounding it silently — and still calling the result "end of
            // requested range" — would hand back a partial read while
            // implying it was complete, exactly the mismatch that produces
            // exact-match edits against text the model never saw.
            let next_start = actual_end + 1;
            let omitted = total - actual_end;
            out.push_str(&format!(
                "[Truncated: lines {next_start}-{total} of {total} ({omitted} lines) were NOT shown \
(a single view_file call is capped at {DEFAULT_READ_WINDOW_LINES} lines for safety, and the requested \
end_line exceeded that cap). This is not the complete file — do not treat it as such. To read the rest, \
call view_file again with start_line={next_start} and end_line={total} (or a smaller end_line to read it \
in chunks).]\n"
            ));
        } else if requested_end.is_some() {
            out.push_str(&format!(
                "... end of requested range; the file continues to line {total} ...\n"
            ));
        } else {
            // Genuine truncation: the caller asked to read from `start_line` with
            // no explicit `end_line`, so this tool applied its own default
            // window and stopped short of the file's actual end. Say exactly
            // which lines were left out and exactly how to get them, so the
            // model can't mistake this for a complete read (requirement: an
            // exact-match edit against text the model never actually saw is
            // the failure mode this message exists to prevent).
            let next_start = actual_end + 1;
            let omitted = total - actual_end;
            out.push_str(&format!(
                "[Truncated: lines {next_start}-{total} of {total} ({omitted} lines) were NOT shown \
(default read window is {DEFAULT_READ_WINDOW_LINES} lines). This is not the complete file — do not \
treat it as such. To read the rest, call view_file again with start_line={next_start} and \
end_line={total} (or a smaller end_line to read it in chunks).]\n"
            ));
        }
    }

    Ok(ViewFileOutput {
        content: out,
        truncated: actual_end < total && (cap_applied || requested_end.is_none()),
    })
}

/// Produces a real unified diff between the actual file content before and
/// after an edit (or set of edits) — never between the tool call's raw
/// arguments. Argument text can diverge from what actually changed on disk
/// (fuzzy/block-anchor matching, CRLF normalization, no-op chunks skipped by
/// the idempotency guard), so the diff must be computed from the true
/// before/after full-file content to be truthful, including its hunk line
/// numbers.
pub fn generate_unified_diff(before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .to_string()
}

struct SingleEdit {
    target: String,
    replacement: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

/// Outcome of attempting to apply one edit to file content.
#[derive(Debug)]
enum EditOutcome {
    /// The edit was applied; content differs from the input.
    Changed(String),
    /// The file already reflects the intended result; content is untouched.
    Unchanged,
}

/// True when `content` already reflects the result of replacing `target` with
/// `replacement` — i.e. re-applying this exact edit would be a no-op or,
/// worse, would duplicate content that a prior identical call already
/// inserted (the target text is frequently a suffix of the replacement, so
/// it keeps matching after the edit has already landed).
fn already_applied(content: &str, target: &str, replacement: &str) -> bool {
    if replacement.is_empty() || !content.contains(replacement) {
        return false;
    }
    if replacement.contains(target) {
        if !content.contains(target) {
            return true;
        }
        let repl_ranges: Vec<(usize, usize)> = content
            .match_indices(replacement)
            .map(|(i, _)| (i, i + replacement.len()))
            .collect();
        return content.match_indices(target).all(|(i, _)| {
            let end = i + target.len();
            repl_ranges.iter().any(|&(rs, re)| rs <= i && end <= re)
        });
    }

    // When replacement does NOT contain target:
    // If target is still in content, the transformation has not occurred.
    if content.contains(target) {
        return false;
    }

    // If target is absent: require strong proof that this exact edit was already performed.
    // Specifically, for single-line statements (e.g., `let status = Idle;` -> `let status = Active;`),
    // check that target and replacement share identical indentation, statement prefix, AND a
    // long common prefix — at least half of the shorter line. A shared first token alone is not
    // enough: `let foo = 1;` -> `let bar = 2;` also shares the `let` prefix, yet the target may
    // never have existed and the edit must error instead of reporting a false no-op.
    // If target has surrounding context lines or multiple lines that never existed in content,
    // we must not guess based on substring/word overlap.
    let target_lines: Vec<&str> = target.lines().collect();
    let replacement_lines: Vec<&str> = replacement.lines().collect();
    if target_lines.len() == 1 && replacement_lines.len() == 1 {
        let t = target_lines[0];
        let r = replacement_lines[0];
        let t_indent = t.len() - t.trim_start().len();
        let r_indent = r.len() - r.trim_start().len();
        let t_trimmed = t.trim();
        let r_trimmed = r.trim();
        let common_prefix = t_trimmed
            .bytes()
            .zip(r_trimmed.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        if t_indent == r_indent
            && !t_trimmed.is_empty()
            && !r_trimmed.is_empty()
            && t_trimmed.split_whitespace().next() == r_trimmed.split_whitespace().next()
            && common_prefix * 2 >= t_trimmed.len().min(r_trimmed.len())
        {
            return true;
        }
    }

    false
}

/// Every key an edit tool call may use for its old/new text, in priority
/// order. A model (or a legacy caller) may send any of these shapes for the
/// same intent; every consumer of an edit call's arguments must recognize
/// them identically.
const EDIT_TARGET_ALIASES: &[&str] = &[
    "target_content",
    "target",
    "old_string",
    "old_text",
    "oldString",
    "oldText",
];
const EDIT_REPLACEMENT_ALIASES: &[&str] = &[
    "replacement_content",
    "replacement",
    "new_string",
    "new_text",
    "newString",
    "newText",
];

fn read_edit_alias(v: &Value, keys: &[&str]) -> Option<String> {
    for &k in keys {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Read an edit call's old/new text under any supported alias
/// (`target_content`/`replacement_content`, `target`/`replacement`,
/// `old_string`/`new_string`, `old_text`/`new_text`, `oldString`/
/// `newString`, `oldText`/`newText`). Used by the actual edit tools
/// (`extract_edit_chunks`) and by the confirmation-modal preview
/// (`get_diff_preview` in `network.rs`) so both recognize a call shaped with
/// any of these keys the same way.
pub(crate) fn edit_target_and_replacement(args: &Value) -> (Option<String>, Option<String>) {
    (
        read_edit_alias(args, EDIT_TARGET_ALIASES),
        read_edit_alias(args, EDIT_REPLACEMENT_ALIASES),
    )
}

fn extract_edit_chunks(args: &Value) -> Result<Vec<SingleEdit>, String> {
    if let Some(edits_arr) = args.get("edits").and_then(coerce_array) {
        if edits_arr.is_empty() {
            return Err("edits array cannot be empty".to_string());
        }
        let mut chunks = Vec::new();
        for (i, item) in edits_arr.iter().enumerate() {
            let (target, replacement) = edit_target_and_replacement(item);
            let target =
                target.ok_or_else(|| format!("edits[{i}] is missing target_content/old_string"))?;
            let replacement = replacement
                .ok_or_else(|| format!("edits[{i}] is missing replacement_content/new_string"))?;
            let start_line = item
                .get("start_line")
                .and_then(parse_json_number)
                .map(|v| v as usize);
            let end_line = item
                .get("end_line")
                .and_then(parse_json_number)
                .map(|v| v as usize);
            chunks.push(SingleEdit {
                target,
                replacement,
                start_line,
                end_line,
            });
        }
        Ok(chunks)
    } else {
        let (target, replacement) = edit_target_and_replacement(args);
        let target = target.ok_or("missing 'target_content' (or 'old_string') argument")?;
        let replacement =
            replacement.ok_or("missing 'replacement_content' (or 'new_string') argument")?;
        let start_line = args
            .get("start_line")
            .and_then(parse_json_number)
            .map(|v| v as usize);
        let end_line = args
            .get("end_line")
            .and_then(parse_json_number)
            .map(|v| v as usize);
        Ok(vec![SingleEdit {
            target,
            replacement,
            start_line,
            end_line,
        }])
    }
}

fn apply_single_edit_to_content(
    content: &str,
    path: &str,
    edit: &SingleEdit,
) -> Result<EditOutcome, String> {
    let had_crlf = content.contains("\r\n");
    let content_norm = content.replace("\r\n", "\n");
    let result = apply_single_edit_to_content_inner(&content_norm, path, edit)?;
    match result {
        EditOutcome::Unchanged => Ok(EditOutcome::Unchanged),
        EditOutcome::Changed(new_content) => Ok(EditOutcome::Changed(if had_crlf {
            new_content.replace("\n", "\r\n")
        } else {
            new_content
        })),
    }
}

fn apply_single_edit_to_content_inner(
    content: &str,
    path: &str,
    edit: &SingleEdit,
) -> Result<EditOutcome, String> {
    if edit.target == edit.replacement {
        return Err("old_string and new_string are identical".to_string());
    }

    // An empty target matches at every byte offset, so it is never the anchor the
    // model meant — it is what a model reaches for when it wants to insert rather
    // than replace. Saying so beats reporting thousands of matches, which reads
    // like the file is the problem.
    if edit.target.is_empty() {
        return Err(format!(
            "error: target_content (old_string) is empty, which matches everywhere in '{path}' and cannot anchor an edit. To insert text, set target_content to the line the new text goes next to and include that line in replacement_content — e.g. to prepend, target the current first line and replace it with the new text followed by that same line."
        ));
    }

    let target_content = &edit.target;
    let replacement_content = &edit.replacement;

    // Idempotency guard: if the file already reflects this edit's intended
    // result, applying it again must not modify (and, for insert-shaped
    // edits where replacement embeds target, must not re-duplicate) content.
    if already_applied(content, target_content, replacement_content) {
        return Ok(EditOutcome::Unchanged);
    }

    // 1. Line range matching (with +-15 tolerance window). A lone `start_line`
    // anchors the edit to that single line — previously both start_line AND
    // end_line were required, so start_line-only edits silently fell through
    // to global matching and failed on non-unique targets.
    if let Some((start, end)) = edit.start_line.map(|s| (s, edit.end_line.unwrap_or(s))) {
        let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let total = file_lines.len();

        let window_start = start.saturating_sub(15).max(1);
        let window_end = (end + 15).min(total);

        if window_start <= window_end {
            if start >= 1 && start <= total && end >= start && end <= total {
                let segment = file_lines[start - 1..end].join("\n");
                if segment.trim_end() == target_content.trim_end()
                    || segment.trim() == target_content.trim()
                    || normalise_unicode_punctuation(&segment).trim()
                        == normalise_unicode_punctuation(target_content).trim()
                {
                    let has_trailing_newline = content.ends_with('\n');
                    let mut new_lines = Vec::new();
                    new_lines.extend_from_slice(&file_lines[..start - 1]);
                    new_lines.push(replacement_content.to_string());
                    new_lines.extend_from_slice(&file_lines[end..]);

                    let mut new_content = new_lines.join("\n");
                    if has_trailing_newline && !new_content.is_empty() {
                        new_content.push('\n');
                    }
                    return Ok(EditOutcome::Changed(new_content));
                }
            }

            let window_text = file_lines[window_start - 1..window_end].join("\n");
            if let Some(pos) = window_text.find(target_content) {
                let bytes_before = file_lines[..window_start - 1]
                    .iter()
                    .map(|l| l.len() + 1)
                    .sum::<usize>();
                let match_start_byte = bytes_before + pos;
                let mut new_content = content.to_string();
                new_content.replace_range(
                    match_start_byte..match_start_byte + target_content.len(),
                    replacement_content,
                );
                return Ok(EditOutcome::Changed(new_content));
            }
        }
    }

    // 2. Exact semantic matching anywhere in content
    let occurrences: Vec<_> = content.match_indices(target_content).collect();
    if occurrences.len() == 1 {
        let (index, _) = occurrences[0];
        let mut new_content = content.to_string();
        new_content.replace_range(index..index + target_content.len(), replacement_content);
        return Ok(EditOutcome::Changed(new_content));
    } else if occurrences.len() > 1 {
        return Err(format!(
            "Error: found {} matches for target_content in '{path}'. Either include more surrounding context lines to make it unique, or pass `start_line` (optionally with `end_line`) to anchor the edit to the specific occurrence you mean.",
            occurrences.len()
        ));
    }

    // 3. Line-ending normalized matching
    let clean_content = content.replace("\r\n", "\n");
    let clean_target = target_content.replace("\r\n", "\n");
    let clean_occurrences: Vec<_> = clean_content.match_indices(&clean_target).collect();

    if clean_occurrences.len() == 1 {
        let (index, _) = clean_occurrences[0];
        let mut new_content = clean_content.clone();
        new_content.replace_range(index..index + clean_target.len(), replacement_content);
        return Ok(EditOutcome::Changed(new_content));
    } else if clean_occurrences.len() > 1 {
        return Err(format!(
            "Error: found {} matches for target_content (with normalized newlines) in '{path}'. Include more surrounding context, or pass `start_line`/`end_line` to target a specific occurrence.",
            clean_occurrences.len()
        ));
    }

    // 4. Fuzzy matching (line-trimmed & block anchor fallback)
    if let Some((start_byte, end_byte)) = find_fuzzy_span(&clean_content, &clean_target) {
        let mut new_content = clean_content.clone();
        let end_byte = end_byte.min(new_content.len());
        if start_byte <= end_byte {
            new_content.replace_range(start_byte..end_byte, replacement_content);
            return Ok(EditOutcome::Changed(new_content));
        }
    }

    // 5. Failure feedback
    let mut err_msg = format!(
        "Error: target_content not found in '{path}'.\n\
         Please check that your target content matches the file."
    );
    if let Some((start, end)) = edit.start_line.map(|s| (s, edit.end_line.unwrap_or(s))) {
        let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let total = file_lines.len();
        if start >= 1 && start <= total && end >= start && end <= total {
            let segment = file_lines[start - 1..end].join("\n");
            err_msg = format!(
                "Error: target_content was not found between lines {start}..{end} in '{path}'.\n\
                 The actual content currently at lines {start}..{end} is:\n\
                 ```\n{segment}\n```\n\
                 Action required: Adjust your start_line/end_line range, or omit line numbers to use exact string matching."
            );
        }
    } else if let Some((lo, hi, ratio)) = closest_region(content, target_content) {
        // No line range was given. Show the closest region verbatim (with line
        // numbers) so the caller can see exactly how their target drifted from
        // the file — the usual culprits are escaped characters (e.g. a Rust
        // `\\` byte, which must appear as `\\` in target_content, not `\`) and
        // whitespace. This is what stops the caller from re-submitting the same
        // failing edit in a loop.
        let file_lines: Vec<&str> = content.lines().collect();
        let snippet = file_lines[lo..hi]
            .iter()
            .enumerate()
            .map(|(k, l)| format!("{:>5}: {l}", lo + 1 + k))
            .collect::<Vec<_>>()
            .join("\n");
        err_msg = format!(
            "Error: target_content not found in '{path}'. Closest region is ~{:.0}% similar.\n\
             Actual file content at lines {}..{}:\n```\n{snippet}\n```\n\
             Action required: copy the block above verbatim (watch for escaped backslashes and trailing whitespace), or shrink target_content to a single unique line as an anchor. Do not re-send the same target unchanged.",
            ratio * 100.0,
            lo + 1,
            hi
        );
    }
    Err(err_msg)
}

/// Find a bounded, line-similar region for actionable edit error feedback.
///
/// This deliberately avoids character-level diffing across every window. A
/// failed edit must return an error promptly; it must never monopolize a
/// worker thread while trying to produce a nicer diagnostic.
fn closest_region(content: &str, target: &str) -> Option<(usize, usize, f32)> {
    const MAX_COMPARISON_CELLS: usize = 250_000;
    let content_lines: Vec<&str> = content.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();
    if content_lines.is_empty() || target_lines.is_empty() {
        return None;
    }
    let win = target_lines.len().min(content_lines.len());
    if content_lines.len().saturating_mul(win) > MAX_COMPARISON_CELLS {
        return None;
    }
    let mut best: Option<(usize, usize, f32)> = None;
    for i in 0..=(content_lines.len() - win) {
        let score: f32 = content_lines[i..i + win]
            .iter()
            .zip(target_lines.iter())
            .map(|(actual, expected)| {
                if actual.trim() == expected.trim() {
                    return 1.0;
                }
                let expected_words: Vec<&str> = expected.split_whitespace().collect();
                let actual_words: Vec<&str> = actual.split_whitespace().collect();
                if expected_words.is_empty() {
                    return 0.0;
                }
                expected_words
                    .iter()
                    .filter(|word| actual_words.contains(word))
                    .count() as f32
                    / expected_words.len() as f32
            })
            .sum();
        let ratio = score / win as f32;
        if best.map(|(_, _, r)| ratio > r).unwrap_or(true) {
            best = Some((i, i + win, ratio));
        }
    }
    best
}

pub fn replace_file_content_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;

    let resolved_path = resolve(path);
    if resolved_path.is_dir() {
        return Err(format!("'{path}' is a directory"));
    }

    let mut chunks = extract_edit_chunks(args)?;
    // Issue 171 fix: sort chunks descending
    chunks.sort_by(|a, b| {
        let a_line = a.start_line.unwrap_or(0);
        let b_line = b.start_line.unwrap_or(0);
        b_line.cmp(&a_line)
    });
    let original_content = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?;
    let mut current_content = original_content.clone();

    let mut any_changed = false;
    let mut unchanged_count = 0usize;
    for (idx, edit) in chunks.iter().enumerate() {
        let outcome = apply_single_edit_to_content(&current_content, path, edit).map_err(|e| {
            if chunks.len() > 1 {
                format!("Edit #{}: {}", idx + 1, e)
            } else {
                e
            }
        })?;

        match outcome {
            EditOutcome::Unchanged => {
                unchanged_count += 1;
            }
            EditOutcome::Changed(new_content) => {
                any_changed = true;
                current_content = new_content;
            }
        }
    }

    if !any_changed {
        return Ok(if chunks.len() == 1 {
            format!(
                "already applied; no changes made to '{path}' (target_content already reflects replacement_content)"
            )
        } else {
            format!(
                "already applied; no changes made to '{path}' ({unchanged_count} of {} edits already reflected)",
                chunks.len()
            )
        });
    }

    std::fs::write(&resolved_path, &current_content)
        .map_err(|e| format!("cannot write '{path}': {e}"))?;

    // One real diff from the true before/after full-file content — never
    // fabricated from the call's target/replacement arguments.
    let combined_diffs = generate_unified_diff(&original_content, &current_content);

    let msg = if chunks.len() == 1 {
        format!(
            "successfully replaced target_content in '{path}'\n\n```diff\n{combined_diffs}\n```"
        )
    } else {
        let note = if unchanged_count > 0 {
            format!(" ({unchanged_count} already applied, skipped)")
        } else {
            String::new()
        };
        format!(
            "successfully applied {} edits in '{path}'{note}\n\n```diff\n{combined_diffs}\n```",
            chunks.len() - unchanged_count
        )
    };

    Ok(msg)
}

pub(crate) fn normalise_unicode_punctuation(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            // Various dash / hyphen code-points -> ASCII '-'
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // Fancy single quotes -> '\''
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Fancy double quotes -> '"'
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Non-breaking space and other odd spaces -> normal space
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn find_fuzzy_span(content: &str, target: &str) -> Option<(usize, usize)> {
    let content_lines: Vec<&str> = content.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();

    if target_lines.is_empty() || content_lines.is_empty() {
        return None;
    }

    let n_content = content_lines.len();
    let n_target = target_lines.len();

    if n_content < n_target {
        return None;
    }

    // Tier 1: Line-rstrip match (ignores trailing whitespace per line)
    let mut rstrip_matches = Vec::new();
    for i in 0..=(n_content - n_target) {
        let window = &content_lines[i..i + n_target];
        let matches_rstrip = window
            .iter()
            .zip(target_lines.iter())
            .all(|(c, t)| c.trim_end() == t.trim_end());
        if matches_rstrip {
            rstrip_matches.push((i, i + n_target));
        }
    }
    if rstrip_matches.len() == 1 {
        let (start_idx, end_idx) = rstrip_matches[0];
        let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
        let byte_end = get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
        return Some((byte_start, byte_end));
    }

    // Tier 2: Line-trimmed match (ignores per-line leading/trailing whitespace & indentation)
    let mut trim_matches = Vec::new();
    for i in 0..=(n_content - n_target) {
        let window = &content_lines[i..i + n_target];
        let matches_trimmed = window
            .iter()
            .zip(target_lines.iter())
            .all(|(c, t)| c.trim() == t.trim());
        if matches_trimmed {
            trim_matches.push((i, i + n_target));
        }
    }
    if trim_matches.len() == 1 {
        let (start_idx, end_idx) = trim_matches[0];
        let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
        let byte_end = get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
        return Some((byte_start, byte_end));
    }

    // Tier 3: Unicode punctuation normalized match
    let mut unicode_matches = Vec::new();
    for i in 0..=(n_content - n_target) {
        let window = &content_lines[i..i + n_target];
        let matches_unicode = window
            .iter()
            .zip(target_lines.iter())
            .all(|(c, t)| {
                normalise_unicode_punctuation(c).trim() == normalise_unicode_punctuation(t).trim()
            });
        if matches_unicode {
            unicode_matches.push((i, i + n_target));
        }
    }
    if unicode_matches.len() == 1 {
        let (start_idx, end_idx) = unicode_matches[0];
        let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
        let byte_end = get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
        return Some((byte_start, byte_end));
    }

    // Tier 4: Block-anchor match for multi-line blocks (>= 3 lines)
    if target_lines.len() >= 3 {
        let first_anchor = target_lines[0].trim();
        let last_anchor = target_lines[target_lines.len() - 1].trim();
        let target_len = target_lines.len();

        let mut anchor_matches: Vec<(usize, usize, f32)> = Vec::new();
        for i in 0..content_lines.len() {
            if content_lines[i].trim() != first_anchor
                && normalise_unicode_punctuation(content_lines[i]).trim()
                    != normalise_unicode_punctuation(first_anchor).trim()
            {
                continue;
            }
            // Consider EVERY line matching the closing anchor, not just the
            // first. The first `last_anchor` is frequently a nested delimiter
            // (e.g. an inner `}` closing a loop before the block's own `}`), so
            // breaking on it discards the real end and the whole match fails.
            for j in (i + 2)..content_lines.len() {
                if content_lines[j].trim() != last_anchor
                    && normalise_unicode_punctuation(content_lines[j]).trim()
                        != normalise_unicode_punctuation(last_anchor).trim()
                {
                    continue;
                }
                let block_len = j - i + 1;
                if (block_len as isize - target_len as isize).abs() > 2 {
                    continue;
                }
                let inner_content = &content_lines[i + 1..j];
                let inner_target = &target_lines[1..target_lines.len() - 1];
                let min_len = inner_content.len().min(inner_target.len());
                let ratio = if min_len == 0 {
                    1.0
                } else {
                    let matched_count = inner_content
                        .iter()
                        .take(min_len)
                        .zip(inner_target.iter().take(min_len))
                        .filter(|(c, t)| {
                            c.trim() == t.trim()
                                || normalise_unicode_punctuation(c).trim()
                                    == normalise_unicode_punctuation(t).trim()
                        })
                        .count();
                    matched_count as f32 / min_len as f32
                };
                if ratio >= 0.6 {
                    anchor_matches.push((i, j + 1, ratio));
                }
            }
        }

        // Apply only when there is a single, unambiguous best block; if two
        // candidates tie on the top score, fall through rather than guess.
        if !anchor_matches.is_empty() {
            anchor_matches
                .sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
            let best = anchor_matches[0].2;
            let top_count = anchor_matches
                .iter()
                .filter(|m| (m.2 - best).abs() < f32::EPSILON)
                .count();
            if top_count == 1 {
                let (start_idx, end_idx, _) = anchor_matches[0];
                let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
                let byte_end =
                    get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
                return Some((byte_start, byte_end));
            }
        }
    }

    None
}

fn get_byte_offset_of_line(lines: &[&str], line_idx: usize) -> usize {
    lines[..line_idx].iter().map(|l| l.len() + 1).sum()
}

fn get_byte_offset_of_line_end(lines: &[&str], line_idx: usize, total_len: usize) -> usize {
    let offset: usize = lines[..=line_idx].iter().map(|l| l.len() + 1).sum();
    offset.min(total_len)
}

pub fn multi_replace_file_content_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let replacements_val = args
        .get("replacements")
        .and_then(coerce_array)
        .ok_or("missing 'replacements' array")?;

    let resolved_path = resolve(path);
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

    chunks.sort_by_key(|c| std::cmp::Reverse(c.start_line));

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

    // Validate matching target contents. A chunk whose range already reads
    // as replacement_content is treated as already applied rather than a
    // mismatch — re-sending the same multi_replace call must not error or
    // re-stack content that a prior identical call already landed.
    let mut needs_apply = vec![true; chunks.len()];
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
        // Same insert-shaped mistake as the single-edit path: an empty target
        // reported as "does not match" against a blank expectation reads like a
        // file problem rather than a malformed edit.
        if chunk.target_content.is_empty() {
            return Err(format!(
                "replacement index {i} has an empty target_content, which cannot anchor an edit. Set target_content to the lines currently at {}-{} and include them in replacement_content to insert around them.",
                chunk.start_line, chunk.end_line
            ));
        }
        let segment = file_lines[chunk.start_line - 1..chunk.end_line].join("\n");
        if segment.trim_end() == chunk.target_content.trim_end() {
            continue;
        }
        if segment.trim_end() == chunk.replacement_content.trim_end() {
            needs_apply[i] = false;
            continue;
        }
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

    if needs_apply.iter().all(|&needed| !needed) {
        return Ok(format!(
            "already applied; no changes made to '{path}' (all {} replacements already reflected)",
            chunks.len()
        ));
    }

    // Apply descending edits
    let applied_count = needs_apply.iter().filter(|&&needed| needed).count();
    for (chunk, needed) in chunks.into_iter().zip(needs_apply) {
        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&file_lines[..chunk.start_line - 1]);
        if needed {
            new_lines.push(chunk.replacement_content);
        } else {
            new_lines.extend_from_slice(&file_lines[chunk.start_line - 1..chunk.end_line]);
        }
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

    let note = if applied_count < replacements_val.len() {
        format!(
            " ({} already applied, skipped)",
            replacements_val.len() - applied_count
        )
    } else {
        String::new()
    };
    // One real diff from the true before/after full-file content — never
    // fabricated from each replacement's target/replacement arguments.
    let diff = generate_unified_diff(&content, &new_content);
    Ok(format!(
        "successfully applied {applied_count} replacements to '{path}'{note}\n\n```diff\n{diff}\n```"
    ))
}

pub fn write_to_file_tool(args: &Value) -> Result<String, String> {
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
        .unwrap_or(true);

    let resolved_path = resolve(path);
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

    let lines = content.lines().count();
    Ok(format!(
        "wrote '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}

#[cfg(test)]
#[path = "filesystem/tests.rs"]
mod tests;
