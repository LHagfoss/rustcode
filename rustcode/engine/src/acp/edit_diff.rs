//! ACP `diff` tool-call content for completed file edits.
//!
//! Edit tools already report a real unified diff of the file's before/after
//! content (fenced in their receipt, or carried in `ToolResult::diff`). Each
//! hunk becomes one ACP `Diff { path, oldText, newText }` so a client can draw
//! it, with `_meta.rustcode.{oldStart,newStart}` giving the hunk's first line
//! numbers. Large or truncated diffs produce no content rather than a wrong one.

use agent_client_protocol::schema::v1::{Diff, ToolCallContent};
use std::path::{Path, PathBuf};

/// Diffs above this many bytes are not sent; the receipt still is.
const MAX_DIFF_BYTES: usize = 256 * 1024;

/// Tools whose successful result describes a change to one file.
fn is_file_edit_tool(name: &str) -> bool {
    matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "write_file_chunk"
            | "delete_file"
            | "copy_file"
    )
}

/// The `diff` content for a finished tool call, or nothing when the call is
/// not a successful file edit or its diff is missing, oversized or truncated.
pub(crate) fn edit_diff_content(
    tool_name: &str,
    success: bool,
    content: &str,
    diff: Option<&str>,
    changed_paths: &[String],
    cwd: Option<&Path>,
) -> Vec<ToolCallContent> {
    if !success || !is_file_edit_tool(tool_name) {
        return Vec::new();
    }
    let Some(unified) = diff
        .filter(|diff| !diff.trim().is_empty())
        .or_else(|| embedded_diff(content))
    else {
        return Vec::new();
    };
    if unified.len() > MAX_DIFF_BYTES || unified.contains('\0') {
        return Vec::new();
    }
    let Some(path) = edited_path(content, changed_paths, cwd) else {
        return Vec::new();
    };
    let Some(hunks) = parse_hunks(unified) else {
        return Vec::new();
    };
    hunks
        .into_iter()
        .map(|hunk| {
            let mut meta = serde_json::Map::new();
            meta.insert(
                "rustcode".to_owned(),
                serde_json::json!({"oldStart": hunk.old_start, "newStart": hunk.new_start}),
            );
            let new_file = hunk.old_start == 0 && hunk.old_count == 0;
            ToolCallContent::Diff(
                Diff::new(path.clone(), hunk.new_text)
                    .old_text((!new_file).then_some(hunk.old_text))
                    .meta(meta),
            )
        })
        .collect()
}

/// The unified diff fenced as ```` ```diff ```` in a tool receipt. The fence
/// must close: a receipt clipped mid-diff yields nothing.
fn embedded_diff(content: &str) -> Option<&str> {
    let start = content.find("```diff\n")? + "```diff\n".len();
    let body = &content[start..];
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        // Diff lines always carry a prefix, so a bare fence ends the block.
        if line.trim_end() == "```" {
            return Some(&body[..offset]);
        }
        offset += line.len();
    }
    None
}

/// The absolute path the receipt names, else the recorded changed path
/// resolved against the session directory.
fn edited_path(content: &str, changed_paths: &[String], cwd: Option<&Path>) -> Option<PathBuf> {
    let head = content.split("```diff").next().unwrap_or(content);
    let resolved = if let Some((_, rest)) = head.split_once("resolved paths: '") {
        // `copied 'a' to 'b'; resolved paths: '/a' -> '/b'`: the target.
        rest.split_once("' -> '")
            .and_then(|(_, dest)| dest.split_once('\''))
            .map(|(dest, _)| dest)
    } else {
        head.split_once("resolved path: '")
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(path, _)| path)
    };
    if let Some(path) = resolved.filter(|path| Path::new(path).is_absolute()) {
        return Some(PathBuf::from(path));
    }
    let path = Path::new(changed_paths.first()?);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.map(|cwd| cwd.join(path))
    }
}

#[derive(Debug, PartialEq)]
struct Hunk {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    old_text: String,
    new_text: String,
    old_seen: u32,
    new_seen: u32,
}

impl Hunk {
    fn complete(&self) -> bool {
        self.old_seen == self.old_count && self.new_seen == self.new_count
    }
}

/// `-a[,b] +c[,d]` from a `@@ … @@` header; a missing count means one line.
fn parse_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let ranges = line.strip_prefix("@@ ")?.split(" @@").next()?;
    let (old, new) = ranges.split_once(' ')?;
    let range = |text: &str| -> Option<(u32, u32)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old.strip_prefix('-')?)?;
    let (new_start, new_count) = range(new.strip_prefix('+')?)?;
    Some((old_start, old_count, new_start, new_count))
}

/// Every hunk of a unified diff, or `None` when a hunk's lines do not add up
/// to its header (the diff was clipped or is not a diff).
fn parse_hunks(diff: &str) -> Option<Vec<Hunk>> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("@@ ") {
            if hunks.last().is_some_and(|hunk| !hunk.complete()) {
                return None;
            }
            let (old_start, old_count, new_start, new_count) = parse_header(line)?;
            hunks.push(Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                old_text: String::new(),
                new_text: String::new(),
                old_seen: 0,
                new_seen: 0,
            });
            continue;
        }
        // File headers and anything before the first hunk.
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        if line.starts_with('\\') {
            // `\ No newline at end of file`
            continue;
        }
        if hunk.complete() {
            // Trailing blank lines after the last hunk.
            if line.trim().is_empty() {
                continue;
            }
            return None;
        }
        let (prefix, text) = match line.chars().next() {
            Some(prefix @ (' ' | '+' | '-')) => (prefix, &line[1..]),
            // An empty context line whose leading space was stripped.
            None => (' ', ""),
            Some(_) => return None,
        };
        if matches!(prefix, ' ' | '-') {
            hunk.old_text.push_str(text);
            hunk.old_text.push('\n');
            hunk.old_seen += 1;
        }
        if matches!(prefix, ' ' | '+') {
            hunk.new_text.push_str(text);
            hunk.new_text.push('\n');
            hunk.new_seen += 1;
        }
        if hunk.old_seen > hunk.old_count || hunk.new_seen > hunk.new_count {
            return None;
        }
    }
    (!hunks.is_empty() && hunks.iter().all(Hunk::complete)).then_some(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diffs(content: Vec<ToolCallContent>) -> Vec<serde_json::Value> {
        content
            .into_iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect()
    }

    #[test]
    fn replace_receipt_becomes_one_diff_per_hunk_with_line_starts() {
        let content = "successfully replaced target_content in 'src/a.rs' (resolved path: '/repo/src/a.rs')\n\n```diff\n@@ -1,3 +1,3 @@\n fn a() {\n-    old();\n+    new();\n }\n@@ -10,2 +10,3 @@\n x\n+y\n z\n\n```";
        let items = diffs(edit_diff_content(
            "replace_file_content",
            true,
            content,
            None,
            &[],
            None,
        ));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["type"], "diff");
        assert_eq!(items[0]["path"], "/repo/src/a.rs");
        assert_eq!(items[0]["oldText"], "fn a() {\n    old();\n}\n");
        assert_eq!(items[0]["newText"], "fn a() {\n    new();\n}\n");
        assert_eq!(items[0]["_meta"]["rustcode"]["oldStart"], 1);
        assert_eq!(items[1]["oldText"], "x\nz\n");
        assert_eq!(items[1]["newText"], "x\ny\nz\n");
        assert_eq!(items[1]["_meta"]["rustcode"]["newStart"], 10);
    }

    #[test]
    fn new_file_has_no_old_text_and_relative_paths_resolve_against_cwd() {
        let items = diffs(edit_diff_content(
            "write_file_chunk",
            true,
            "wrote chunk",
            Some("@@ -0,0 +1,2 @@\n+one\n+two\n"),
            &["notes/new.md".to_owned()],
            Some(Path::new("/repo")),
        ));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["path"], "/repo/notes/new.md");
        assert!(items[0].get("oldText").is_none());
        assert_eq!(items[0]["newText"], "one\ntwo\n");
    }

    #[test]
    fn copy_receipt_names_the_destination() {
        let items = diffs(edit_diff_content(
            "copy_file",
            true,
            "copied 'a' to 'b'; resolved paths: '/repo/a' -> '/repo/b'",
            Some("@@ -0,0 +1 @@\n+hi\n"),
            &[],
            None,
        ));
        assert_eq!(items[0]["path"], "/repo/b");
    }

    #[test]
    fn failed_unrelated_clipped_and_oversized_results_carry_no_diff() {
        let receipt = "wrote 'a' (resolved path: '/repo/a'; 1 lines, 2 bytes)\n\n```diff\n@@ -1 +1 @@\n-a\n+b\n\n```";
        assert!(edit_diff_content("write_to_file", false, receipt, None, &[], None).is_empty());
        assert!(edit_diff_content("read_file", true, receipt, None, &[], None).is_empty());
        assert_eq!(
            edit_diff_content("write_to_file", true, receipt, None, &[], None).len(),
            1
        );
        // The fence never closes: the receipt was clipped.
        let clipped = "wrote 'a' (resolved path: '/repo/a')\n\n```diff\n@@ -1,3 +1,3 @@\n a\n-b\n";
        assert!(edit_diff_content("write_to_file", true, clipped, None, &[], None).is_empty());
        // The hunk is short of its header's line counts.
        let short = "wrote 'a' (resolved path: '/repo/a')\n\n```diff\n@@ -1,3 +1,3 @@\n a\n-b\n```";
        assert!(edit_diff_content("write_to_file", true, short, None, &[], None).is_empty());
        let huge = format!("@@ -1 +1 @@\n-a\n+{}\n", "x".repeat(MAX_DIFF_BYTES));
        assert!(
            edit_diff_content(
                "write_to_file",
                true,
                "(resolved path: '/repo/a')",
                Some(&huge),
                &[],
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn diff_fences_inside_edited_markdown_do_not_end_the_block() {
        let content = "successfully replaced target_content in 'README.md' (resolved path: '/repo/README.md')\n\n```diff\n@@ -1,2 +1,2 @@\n ```\n-old\n+new\n```";
        let items = diffs(edit_diff_content(
            "replace_file_content",
            true,
            content,
            None,
            &[],
            None,
        ));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["oldText"], "```\nold\n");
    }
}
