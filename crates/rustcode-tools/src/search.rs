use serde_json::Value;
use rustcode_core::ToolResultCompleteness;

const MAX_LIST_ENTRIES: usize = 10_000;

pub struct DirectoryListingOutput {
    pub content: String,
    pub completeness: ToolResultCompleteness,
}

/// List a directory for `view_file`'s directory fallback.
pub fn list_directory(args: &Value) -> Result<String, String> {
    list_directory_output(args).map(|output| output.content)
}

pub(crate) fn list_directory_output(args: &Value) -> Result<DirectoryListingOutput, String> {
    list_directory_output_with_context(args, &super::active_context())
}

/// List a directory using an explicit workspace/session context.
pub fn list_directory_with_context(
    args: &Value,
    context: &super::ToolContext,
) -> Result<String, String> {
    list_directory_output_with_context(args, context).map(|output| output.content)
}

pub fn list_directory_output_with_context(
    args: &Value,
    context: &super::ToolContext,
) -> Result<DirectoryListingOutput, String> {
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
        return Ok(DirectoryListingOutput {
            content: format!("'{path}' is empty"),
            completeness: ToolResultCompleteness::Complete,
        });
    }
    let total = names.len();
    if total > MAX_LIST_ENTRIES {
        let mut out = names[..MAX_LIST_ENTRIES].join("\n");
        out.push_str(&format!(
            "\n... ({} more entries, total {total} — use grep/glob to narrow)",
            total - MAX_LIST_ENTRIES
        ));
        Ok(DirectoryListingOutput {
            content: out,
            completeness: ToolResultCompleteness::ByteTruncated,
        })
    } else {
        Ok(DirectoryListingOutput {
            content: names.join("\n"),
            completeness: ToolResultCompleteness::Complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_listing_distinguishes_exhaustive_results_from_capped_results() {
        let capped_dir = tempfile::tempdir().expect("tempdir");
        for index in 0..=MAX_LIST_ENTRIES {
            std::fs::write(capped_dir.path().join(format!("file-{index}")), "content")
                .expect("write");
        }
        let capped = list_directory_output(&serde_json::json!({
            "path": capped_dir.path().to_string_lossy().to_string(),
        }))
        .expect("list directory");
        assert_eq!(
            capped.completeness,
            ToolResultCompleteness::ByteTruncated
        );
        assert!(capped.content.contains("more entries"));

        let exhaustive_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(exhaustive_dir.path().join("file"), "content").expect("write");
        let exhaustive = list_directory_output(&serde_json::json!({
            "path": exhaustive_dir.path().to_string_lossy().to_string(),
        }))
        .expect("list directory");
        assert_eq!(exhaustive.completeness, ToolResultCompleteness::Complete);
        assert!(!exhaustive.content.contains("more entries"));
    }
}
