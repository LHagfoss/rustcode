use super::super::text::cap_diff_lines;

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
