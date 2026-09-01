use serde_json::Value;

use super::{Tool, ToolCapability, ToolSafety};

fn context() -> rustcode_tools::ToolContext {
    super::current_tool_context()
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
    description: "Return exact, numbered file text for an inclusive 1-indexed range (or list a directory). Output is never silently summarized: when the 800-line hard cap omits content, the result reports the omitted lines and exact next start line. Use targeted follow-up ranges instead of retrying through cat/sed/awk. Supports an optional UTF-8 byte offset.",
    arguments: r#"{"path": "absolute or relative path to file or directory", "start_line": "optional start line number, 1-indexed (default 1)", "end_line": "optional end line number, 1-indexed (each call is capped at 800 lines; request targeted follow-up ranges for more content)", "content_offset": "optional byte offset into content"}"#,
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
    description: "Surgically edit code in an existing file. Supports single replacement (target_content/replacement_content or old_string/new_string) or array of batch edits (edits: [{old_string, new_string}]). Line numbers are optional. This tool only replaces: to INSERT text, target an existing neighbouring line and repeat it in the replacement — to prepend, target the current first line and replace it with the new text followed by that line. An empty target is rejected, since it matches everywhere.",
    arguments: r#"{"path": "absolute or relative path to file", "target_content": "precise block of code to edit (or old_string) — never empty; to insert, anchor on an adjacent line and repeat it in the replacement", "replacement_content": "complete replacement text (or new_string)", "edits": "optional array of [{old_string, new_string}] for multiple edits in 1 call"}"#,
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
    description: "Apply multiple non-contiguous edits across a single file in a single tool call.                       Specify each edit as a separate replacement chunk.",
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

pub fn delete_file(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::delete_file_with_context(args, &context())
}

pub fn move_file(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::move_file_with_context(args, &context())
}

pub fn copy_file(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::copy_file_with_context(args, &context())
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
    })
}

pub(crate) fn edit_target_and_replacement(args: &Value) -> (Option<String>, Option<String>) {
    rustcode_tools::filesystem::edit_target_and_replacement(args)
}

pub fn replace_file_content_tool(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::replace_file_content_with_context(args, &context())
}

pub fn multi_replace_file_content_tool(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::multi_replace_file_content_with_context(args, &context())
}

pub fn write_to_file_tool(args: &Value) -> Result<String, String> {
    rustcode_tools::filesystem::write_to_file_with_context(args, &context())
}

pub fn generate_unified_diff(before: &str, after: &str) -> String {
    rustcode_tools::filesystem::generate_unified_diff(before, after)
}

#[allow(dead_code)]
pub(crate) fn normalise_unicode_punctuation(s: &str) -> String {
    rustcode_tools::filesystem::normalise_unicode_punctuation(s)
}
