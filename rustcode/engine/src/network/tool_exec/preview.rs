use rustcode_tool_protocol::text::cap_diff_lines;

pub(crate) fn get_diff_preview(name: &str, args: &serde_json::Value) -> Option<String> {
    if name == "replace_file_content" {
        let (target, replacement) = crate::tools::edit_target_and_replacement(args);
        let search_block = target.as_deref().unwrap_or("");
        let replace_block = replacement.as_deref().unwrap_or("");

        let diff = similar::TextDiff::from_lines(search_block, replace_block);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for op in diff.ops() {
            let old_slice = &old_slices[op.old_range()];
            let new_slice = &new_slices[op.new_range()];
            match op.tag() {
                similar::DiffTag::Equal => {
                    for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                        prev.push_str(&format!(
                            " {}\x00 {}\n",
                            o.trim_end_matches('\n').trim_end_matches('\r'),
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Delete => {
                    for o in old_slice {
                        prev.push_str(&format!(
                            "-{}\x00~\n",
                            o.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Insert => {
                    for n in new_slice {
                        prev.push_str(&format!(
                            "~\x00+{}\n",
                            n.trim_end_matches('\n').trim_end_matches('\r')
                        ));
                    }
                }
                similar::DiffTag::Replace => {
                    let max_len = old_slice.len().max(new_slice.len());
                    for i in 0..max_len {
                        let o_val = old_slice.get(i);
                        let n_val = new_slice.get(i);
                        match (o_val, n_val) {
                            (Some(o), Some(n)) => {
                                prev.push_str(&format!(
                                    "-{}\x00+{}\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r'),
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (Some(o), None) => {
                                prev.push_str(&format!(
                                    "-{}\x00~\n",
                                    o.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, Some(n)) => {
                                prev.push_str(&format!(
                                    "~\x00+{}\n",
                                    n.trim_end_matches('\n').trim_end_matches('\r')
                                ));
                            }
                            (None, None) => {}
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else if name == "write_to_file" && args.get("__rustcode_legacy_write_diff").is_some() {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
        let old_content = std::fs::read_to_string(path).unwrap_or_default();
        let new_content = args.get("content").and_then(|c| c.as_str()).unwrap_or("");

        let diff = similar::TextDiff::from_lines(&old_content, new_content);
        let old_slices: Vec<&str> = diff.iter_old_slices().collect();
        let new_slices: Vec<&str> = diff.iter_new_slices().collect();

        let mut prev = String::new();
        for group in diff.grouped_ops(3) {
            for op in group {
                let old_slice = &old_slices[op.old_range()];
                let new_slice = &new_slices[op.new_range()];
                match op.tag() {
                    similar::DiffTag::Equal => {
                        for (o, n) in old_slice.iter().zip(new_slice.iter()) {
                            prev.push_str(&format!(
                                " {}\x00 {}\n",
                                o.trim_end_matches('\n').trim_end_matches('\r'),
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Delete => {
                        for o in old_slice {
                            prev.push_str(&format!(
                                "-{}\x00~\n",
                                o.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Insert => {
                        for n in new_slice {
                            prev.push_str(&format!(
                                "~\x00+{}\n",
                                n.trim_end_matches('\n').trim_end_matches('\r')
                            ));
                        }
                    }
                    similar::DiffTag::Replace => {
                        let max_len = old_slice.len().max(new_slice.len());
                        for i in 0..max_len {
                            let o_val = old_slice.get(i);
                            let n_val = new_slice.get(i);
                            match (o_val, n_val) {
                                (Some(o), Some(n)) => {
                                    prev.push_str(&format!(
                                        "-{}\x00+{}\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r'),
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (Some(o), None) => {
                                    prev.push_str(&format!(
                                        "-{}\x00~\n",
                                        o.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, Some(n)) => {
                                    prev.push_str(&format!(
                                        "~\x00+{}\n",
                                        n.trim_end_matches('\n').trim_end_matches('\r')
                                    ));
                                }
                                (None, None) => {}
                            }
                        }
                    }
                }
            }
        }
        Some(cap_diff_lines(prev))
    } else {
        None
    }
}

/// Capture the pre-mutation text for operations whose tool output has no diff.
/// Callers run this only after authorization and in the execution worker's
/// ToolContext, so confirmation dialogs never expose file contents and path
/// resolution follows the mutation handler exactly.
pub(crate) fn capture_text_mutation_before(
    name: &str,
    args: &serde_json::Value,
    context: &rustcode_tools::ToolContext,
) -> Option<(std::path::PathBuf, String)> {
    const MAX_DELETE_DIFF_FILE_BYTES: u64 = 50 * 1024;

    let path_arg = match name {
        "delete_file" | "write_file_chunk" => "path",
        "copy_file" => "dest",
        _ => return None,
    };
    if context.workspace_root.is_none() && !context.allow_task_scope_escape {
        return None;
    }
    let raw_path = args.get(path_arg)?.as_str()?;
    let target = canonical_mutation_target(raw_path, context)?;
    validate_mutation_target(&target, context)?;
    let Ok(metadata) = target.metadata() else {
        // New copy/chunk targets have an empty before-state.
        if matches!(name, "copy_file" | "write_file_chunk") {
            return Some((target, String::new()));
        }
        return None;
    };
    if !metadata.is_file() || metadata.len() > MAX_DELETE_DIFF_FILE_BYTES {
        return None;
    }
    let before = read_bounded_text_file(&target)?;
    Some((target, before))
}

pub(crate) fn finish_text_mutation_diff(
    snapshot: Option<(std::path::PathBuf, String)>,
    context: &rustcode_tools::ToolContext,
) -> Option<String> {
    let (target, before) = snapshot?;
    let target = canonical_mutation_target(&target.to_string_lossy(), context)?;
    validate_mutation_target(&target, context)?;
    let after = if target.exists() {
        read_bounded_text_file(&target)?
    } else {
        String::new()
    };
    let diff = rustcode_tools::filesystem::generate_unified_diff(&before, &after);
    (!diff.trim().is_empty()).then_some(diff)
}

fn canonical_mutation_target(
    raw_path: &str,
    context: &rustcode_tools::ToolContext,
) -> Option<std::path::PathBuf> {
    let resolved = rustcode_tools::resolve_tool_path_with_context(raw_path, context);
    if let Ok(canonical) = resolved.canonicalize() {
        return Some(canonical);
    }
    if std::fs::symlink_metadata(&resolved).is_ok() {
        return None;
    }
    let parent = resolved.parent()?.canonicalize().ok()?;
    Some(parent.join(resolved.file_name()?))
}

fn validate_mutation_target(
    target: &std::path::Path,
    context: &rustcode_tools::ToolContext,
) -> Option<()> {
    if let Some(workspace_root) = context.workspace_root.as_deref() {
        let root = workspace_root.canonicalize().ok()?;
        if !target.starts_with(root) {
            return None;
        }
    } else if !context.allow_task_scope_escape {
        return None;
    }
    Some(())
}

fn read_bounded_text_file(path: &std::path::Path) -> Option<String> {
    const MAX_MUTATION_DIFF_FILE_BYTES: u64 = 50 * 1024;
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_MUTATION_DIFF_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() as u64 > MAX_MUTATION_DIFF_FILE_BYTES || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

pub(crate) fn extract_diff_block(content: &str) -> Option<String> {
    let after_fence = content.split_once("```diff\n")?.1;
    let (body, _) = after_fence.split_once("\n```")?;
    if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

pub(crate) fn final_tool_diff(result: &str, preview_fallback: Option<String>) -> Option<String> {
    extract_diff_block(result).or_else(|| preview_fallback.filter(|d| !d.trim().is_empty()))
}

pub(crate) fn tool_result_precludes_preview_fallback(content: &str) -> bool {
    let lower = content.trim_start().to_ascii_lowercase();
    lower.starts_with("error") || lower.contains("already applied")
}

pub(crate) fn get_file_preview(name: &str, args: &serde_json::Value) -> Option<(String, String)> {
    if name != "write_to_file" {
        return None;
    }
    Some((
        args.get("path")?.as_str()?.to_string(),
        args.get("content")?.as_str()?.to_string(),
    ))
}

pub(crate) fn get_tool_project_root(
    _name: &str,
    args: &serde_json::Value,
) -> Option<std::path::PathBuf> {
    let raw_path = if let Some(p) = args.get("path").and_then(|p| p.as_str()) {
        Some(p)
    } else if let Some(s) = args.get("src").and_then(|s| s.as_str()) {
        Some(s)
    } else {
        args.get("dest").and_then(|d| d.as_str())
    };

    let resolved = if let Some(rp) = raw_path {
        let p = crate::tools::resolve_tool_path(rp);
        if p.is_relative() {
            std::env::current_dir().unwrap_or_default().join(p)
        } else {
            p
        }
    } else {
        return None;
    };

    let mut current = if resolved.is_dir() {
        resolved.clone()
    } else {
        resolved
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(resolved)
    };

    loop {
        if current.join("Cargo.toml").exists() || current.join("tsconfig.json").exists() {
            return Some(current.canonicalize().unwrap_or(current));
        }
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    None
}
