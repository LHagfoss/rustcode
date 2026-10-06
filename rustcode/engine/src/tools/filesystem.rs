use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};

fn context() -> rustcode_tools::ToolContext {
    super::current_tool_context()
}

/// Notify derived workspace state after native tools finish a successful write.
fn workspace_mutation(
    args: &Value,
    paths: &[&str],
    operation: fn(&Value, &rustcode_tools::ToolContext) -> Result<String, String>,
) -> Result<String, String> {
    let context = context();
    let result = operation(args, &context);
    if result.is_ok() {
        let task = context
            .task_working_directory
            .as_ref()
            .or(context.workspace_root.as_ref())
            .cloned()
            .or_else(|| std::env::current_dir().ok());
        if let Some(task) = task {
            let workspace = context.workspace_root.as_ref().unwrap_or(&task);
            for key in paths {
                if let Some(path) = args.get(key).and_then(Value::as_str) {
                    let path = rustcode_tools::resolve_tool_path_with_context(path, &context);
                    crate::workspace_intelligence::invalidate_path(workspace, &path);
                    if workspace != &task {
                        crate::workspace_intelligence::invalidate_path(&task, &path);
                    }
                }
            }
        }
    }
    result
}

fn delete_file_schema() -> Value {
    rustcode_tools::filesystem::delete_file_schema()
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
    rustcode_tools::filesystem::move_file_schema()
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
    rustcode_tools::filesystem::copy_file_schema()
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
    rustcode_tools::filesystem::view_file_schema()
}

pub const VIEW_FILE: Tool = Tool {
    name: "view_file",
    description: "Return exact numbered file text for a requested inclusive 1-indexed range (or list a directory); ordinary output is never silently summarized. Ordinary reads have an 800-line hard cap; use targeted follow-up ranges to retrieve more content. For Markdown, explicitly request outline=true to get bounded headings with exact section ranges; expand a section with start_line and end_line. An outline is partial and never counts as a complete file read. Supports a UTF-8 byte offset for ordinary reads.",
    arguments: r#"{"path": "absolute or relative path to file or directory", "start_line": "optional start line number, 1-indexed (default 1)", "end_line": "optional end line number, 1-indexed (each call is capped at 800 lines; request targeted follow-up ranges for more content)", "content_offset": "optional byte offset into content", "outline": "optional true to return a bounded Markdown heading outline only", "outline_offset": "optional zero-based offset for the next heading page", "outline_limit": "optional number of headings per page, 1 to 100 (default 50)"}"#,
    handler: view_file_tool,
    requires_confirmation: false,
    schema: view_file_schema,
    capabilities: &[ToolCapability::ReadWorkspace],
    safety: ToolSafety::ReadOnly,
};

fn replace_file_content_schema() -> Value {
    rustcode_tools::filesystem::replace_file_content_schema()
}

pub const REPLACE_FILE_CONTENT: Tool = Tool {
    name: "replace_file_content",
    description: "Edit an existing file by replacing one precise target_content block with replacement_content. replacement_content is required (use an empty string only to delete the target). The legacy old_string/new_string names and the edits array remain accepted for compatibility. To insert, anchor on an adjacent line and repeat that line in the replacement; an empty target is rejected.",
    arguments: r#"{"path": "file path", "target_content": "canonical exact block to replace (legacy old_string is accepted)", "replacement_content": "REQUIRED complete replacement text (legacy new_string is accepted; use an empty string only to delete)", "edits": "optional array of edit objects for multiple replacements"}"#,
    handler: replace_file_content_tool,
    requires_confirmation: true,
    schema: replace_file_content_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn multi_replace_file_content_schema() -> Value {
    rustcode_tools::filesystem::multi_replace_file_content_schema()
}

pub const MULTI_REPLACE_FILE_CONTENT: Tool = Tool {
    name: "multi_replace_file_content",
    description: "Apply multiple non-contiguous edits to one file. Each replacement must include its line range, target_content, and replacement_content.",
    arguments: r#"{"path": "absolute or relative path to file", "replacements": "array of objects, each containing: {start_line, end_line, target_content, replacement_content}"}"#,
    handler: multi_replace_file_content_tool,
    requires_confirmation: true,
    schema: multi_replace_file_content_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

fn write_to_file_schema() -> Value {
    rustcode_tools::filesystem::write_to_file_schema()
}

fn write_file_chunk_schema() -> Value {
    rustcode_tools::filesystem::write_file_chunk_schema()
}

pub const WRITE_TO_FILE: Tool = Tool {
    name: "write_to_file",
    description: "Create or overwrite a small file with complete content. Start chunked with write_file_chunk past ~4 KiB: a large single write risks a provider output cutoff that loses the whole call, and content larger than 16 KiB must use write_file_chunk repeatedly so an interrupted response cannot lose the whole write. Parent directories are created automatically.",
    arguments: r#"{"path": "new or existing file path", "content": "complete contents (keep at or below 16384 bytes; past ~4 KiB prefer write_file_chunk from the start; use write_file_chunk for larger files)", "overwrite": "optional boolean, defaults to true to allow overwriting an existing file"}"#,
    handler: write_to_file_tool,
    requires_confirmation: true,
    schema: write_to_file_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub const WRITE_FILE_CHUNK: Tool = Tool {
    name: "write_file_chunk",
    description: "Write one bounded file chunk at a byte offset. Calls are capped at 16 KiB and return offset, next_offset, bytes, size, and SHA-256 metadata so interrupted writes can resume without duplicating content.",
    arguments: r#"{"path": "file path", "content": "chunk (maximum 16384 bytes)", "offset": "optional byte offset, defaults to 0", "truncate": "optional boolean for the first chunk at offset 0", "expected_size": "optional current file size guard", "expected_sha256": "optional current file SHA-256 guard"}"#,
    handler: write_file_chunk_tool,
    requires_confirmation: true,
    schema: write_file_chunk_schema,
    capabilities: &[ToolCapability::WriteWorkspace],
    safety: ToolSafety::WorkspaceMutation,
};

pub fn delete_file(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["path"],
        rustcode_tools::filesystem::delete_file_with_context,
    )
}

pub fn move_file(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["src", "dest"],
        rustcode_tools::filesystem::move_file_with_context,
    )
}

pub fn copy_file(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["dest"],
        rustcode_tools::filesystem::copy_file_with_context,
    )
}

pub fn view_file_tool(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::view_file_with_context(args, &context())
        .map(|output| output.content)
}

pub(crate) fn view_file_output(args: &Value) -> Result<super::ToolExecutionOutput, String> {
    let output = rustcode_tools::filesystem::view_file_with_context(args, &context())?;
    Ok(super::ToolExecutionOutput {
        content: output.content,
        success: true,
        pending: false,
        command: None,
        exit_code: None,
        truncated: output.truncated,
        completeness: output.completeness,
        replayed: false,
        error_kind: None,
        retryable: false,
        command_status: None,
    })
}

pub(crate) fn edit_target_and_replacement(args: &Value) -> (Option<String>, Option<String>) {
    rustcode_tools::filesystem::edit_target_and_replacement(args)
}

pub fn replace_file_content_tool(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["path"],
        rustcode_tools::filesystem::replace_file_content_with_context,
    )
}

pub fn multi_replace_file_content_tool(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["path"],
        rustcode_tools::filesystem::multi_replace_file_content_with_context,
    )
}

pub fn write_to_file_tool(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["path"],
        rustcode_tools::filesystem::write_to_file_with_context,
    )
}

pub fn write_file_chunk_tool(args: &Value) -> Result<String, String> {
    workspace_mutation(
        args,
        &["path"],
        rustcode_tools::filesystem::write_file_chunk_with_context,
    )
}

#[allow(dead_code)]
pub(crate) fn normalise_unicode_punctuation(s: &str) -> String {
    rustcode_tools::filesystem::normalise_unicode_punctuation(s)
}
