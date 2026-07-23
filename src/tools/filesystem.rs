use serde_json::Value;
use std::path::PathBuf;

// Re-exports needed by filesystem tools
pub(crate) use super::parse_json_number;
pub(crate) use super::resolve_tool_path;

struct ReplacementChunk {
    start_line: usize,
    end_line: usize,
    target_content: String,
    replacement_content: String,
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
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let resolved_path = resolve(path);
    let start_line = args
        .get("start_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(1);
    let end_line = args
        .get("end_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(start_line + 500);

    let content_bytes =
        std::fs::read(&resolved_path).map_err(|e| format!("cannot read '{path}': {e}"))?;

    let byte_offset = args
        .get("content_offset")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(0);

    if byte_offset >= content_bytes.len() && !content_bytes.is_empty() {
        return Err(format!(
            "content_offset {} exceeds file size {}",
            byte_offset,
            content_bytes.len()
        ));
    }

    let sliced_content = String::from_utf8_lossy(&content_bytes[byte_offset..]);
    let lines: Vec<&str> = sliced_content.lines().collect();
    let total = lines.len();

    if total == 0 {
        return Ok(format!(
            "[File: {}, Empty file, Bytes offset: {}]",
            path, byte_offset
        ));
    }

    if start_line < 1 || start_line > total {
        return Err(format!(
            "start_line {} is out of bounds (1 to {})",
            start_line, total
        ));
    }

    let actual_end = end_line.min(total);
    let mut out = format!(
        "[File: {}, Lines {} to {} of {}, Bytes offset: {}]\n",
        path, start_line, actual_end, total, byte_offset
    );

    for (idx, line) in lines[start_line - 1..actual_end].iter().enumerate() {
        out.push_str(&format!("{}: {}\n", start_line + idx, line));
    }

    if actual_end < total {
        out.push_str("... content truncated (use end_line or content_offset to read more) ...\n");
    }

    Ok(out)
}

pub fn generate_unified_diff(target: &str, replacement: &str) -> String {
    let diff = similar::TextDiff::from_lines(target, replacement);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => "-",
            similar::ChangeTag::Insert => "+",
            similar::ChangeTag::Equal => " ",
        };
        out.push_str(&format!("{}{}", sign, change));
    }
    out
}

pub fn replace_file_content_tool(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|p| p.as_str())
        .ok_or("missing 'path' argument")?;
    let start_line = args
        .get("start_line")
        .and_then(parse_json_number)
        .map(|v| v as usize);
    let end_line = args
        .get("end_line")
        .and_then(parse_json_number)
        .map(|v| v as usize);
    let target_content = args
        .get("target_content")
        .and_then(|t| t.as_str())
        .ok_or("missing 'target_content' argument")?;
    let replacement_content = args
        .get("replacement_content")
        .and_then(|r| r.as_str())
        .ok_or("missing 'replacement_content' argument")?;

    let resolved_path = resolve(path);
    let content = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?;

    let diff_text = generate_unified_diff(target_content, replacement_content);

    // 1. If start_line and end_line are provided, try exact matching in that line range first
    if let (Some(start), Some(end)) = (start_line, end_line) {
        let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let total = file_lines.len();

        if start >= 1 && start <= total && end >= start && end <= total {
            let segment = file_lines[start - 1..end].join("\n");
            if segment.trim_end() == target_content.trim_end() {
                let has_trailing_newline = content.ends_with('\n');
                let mut new_lines = Vec::new();
                new_lines.extend_from_slice(&file_lines[..start - 1]);
                new_lines.push(replacement_content.to_string());
                new_lines.extend_from_slice(&file_lines[end..]);

                let mut new_content = new_lines.join("\n");
                if has_trailing_newline && !new_content.is_empty() {
                    new_content.push('\n');
                }
                std::fs::write(&resolved_path, &new_content)
                    .map_err(|e| format!("cannot write '{path}': {e}"))?;

                return Ok(format!(
                    "successfully replaced lines {start}-{end} in '{path}'\n\n```diff\n{diff_text}\n```"
                ));
            }
        }
    }

    // 2. Exact semantic matching anywhere in the file (helps if line numbers shifted)
    let occurrences: Vec<_> = content.match_indices(target_content).collect();
    if occurrences.len() == 1 {
        let (index, _) = occurrences[0];
        let mut new_content = content.clone();
        new_content.replace_range(index..index + target_content.len(), replacement_content);
        std::fs::write(&resolved_path, &new_content)
            .map_err(|e| format!("cannot write '{path}': {e}"))?;
        return Ok(format!(
            "successfully replaced target_content in '{path}' (uniquely located in file)\n\n```diff\n{diff_text}\n```"
        ));
    } else if occurrences.len() > 1 {
        return Err(format!(
            "Error: found {} matches for target_content in '{path}'. Please include more surrounding context lines to make it unique.",
            occurrences.len()
        ));
    }

    // 3. Normalized matching (ignoring line endings CRLF vs LF)
    let clean_content = content.replace("\r\n", "\n");
    let clean_target = target_content.replace("\r\n", "\n");
    let clean_occurrences: Vec<_> = clean_content.match_indices(&clean_target).collect();

    if clean_occurrences.len() == 1 {
        let (index, _) = clean_occurrences[0];
        let mut new_content = clean_content.clone();
        new_content.replace_range(index..index + clean_target.len(), replacement_content);
        std::fs::write(&resolved_path, &new_content)
            .map_err(|e| format!("cannot write '{path}': {e}"))?;
        return Ok(format!(
            "successfully replaced target_content in '{path}' (matched with normalized line endings)\n\n```diff\n{diff_text}\n```"
        ));
    } else if clean_occurrences.len() > 1 {
        return Err(format!(
            "Error: found {} matches for target_content (with normalized newlines) in '{path}'. Please include more surrounding context.",
            clean_occurrences.len()
        ));
    }

    // 3.5. Fuzzy matching (line-trimmed & block anchor fallback ala OpenCode)
    if let Some((start_byte, end_byte)) = find_fuzzy_span(&clean_content, &clean_target) {
        let mut new_content = clean_content.clone();
        let end_byte = end_byte.min(new_content.len());
        if start_byte <= end_byte {
            new_content.replace_range(start_byte..end_byte, replacement_content);
            std::fs::write(&resolved_path, &new_content)
                .map_err(|e| format!("cannot write '{path}': {e}"))?;
            return Ok(format!(
                "successfully replaced target_content in '{path}' (matched via fuzzy block/line-trimmed alignment)\n\n```diff\n{diff_text}\n```"
            ));
        }
    }

    // 4. Mismatch feedback
    let mut err_msg = format!(
        "Error: target_content not found in '{path}'.\n\
         Please ensure the code block matches exactly (including indentation and spaces)."
    );
    if let (Some(start), Some(end)) = (start_line, end_line) {
        let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let total = file_lines.len();
        if start >= 1 && start <= total && end >= start && end <= total {
            let segment = file_lines[start - 1..end].join("\n");
            err_msg.push_str("\n=== Found in File at specified lines ===\n");
            err_msg.push_str(&segment);
            err_msg.push_str("\n========================================\n");
        }
    }
    Err(err_msg)
}

fn find_fuzzy_span(content: &str, target: &str) -> Option<(usize, usize)> {
    let content_lines: Vec<&str> = content.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();

    if target_lines.is_empty() || content_lines.is_empty() {
        return None;
    }

    // 1. Line-trimmed match (ignores per-line leading/trailing whitespace)
    let mut matches = Vec::new();
    if content_lines.len() >= target_lines.len() {
        for i in 0..=(content_lines.len() - target_lines.len()) {
            let window = &content_lines[i..i + target_lines.len()];
            let matches_trimmed = window
                .iter()
                .zip(target_lines.iter())
                .all(|(c, t)| c.trim() == t.trim());
            if matches_trimmed {
                matches.push((i, i + target_lines.len()));
            }
        }
    }

    if matches.len() == 1 {
        let (start_idx, end_idx) = matches[0];
        let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
        let byte_end = get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
        return Some((byte_start, byte_end));
    }

    // 2. Block-anchor match for multi-line blocks (>= 3 lines)
    if target_lines.len() >= 3 {
        let first_anchor = target_lines[0].trim();
        let last_anchor = target_lines[target_lines.len() - 1].trim();
        let target_len = target_lines.len();

        let mut anchor_matches = Vec::new();
        for i in 0..content_lines.len() {
            if content_lines[i].trim() != first_anchor {
                continue;
            }
            for j in (i + 2)..content_lines.len() {
                if content_lines[j].trim() == last_anchor {
                    let block_len = j - i + 1;
                    if (block_len as isize - target_len as isize).abs() <= 2 {
                        let inner_content = &content_lines[i + 1..j];
                        let inner_target = &target_lines[1..target_lines.len() - 1];
                        let min_len = inner_content.len().min(inner_target.len());
                        if min_len > 0 {
                            let matched_count = inner_content
                                .iter()
                                .take(min_len)
                                .zip(inner_target.iter().take(min_len))
                                .filter(|(c, t)| c.trim() == t.trim())
                                .count();
                            let ratio = matched_count as f32 / min_len as f32;
                            if ratio >= 0.6 {
                                anchor_matches.push((i, j + 1));
                            }
                        } else {
                            anchor_matches.push((i, j + 1));
                        }
                    }
                    break;
                }
            }
        }

        if anchor_matches.len() == 1 {
            let (start_idx, end_idx) = anchor_matches[0];
            let byte_start = get_byte_offset_of_line(&content_lines, start_idx);
            let byte_end = get_byte_offset_of_line_end(&content_lines, end_idx - 1, content.len());
            return Some((byte_start, byte_end));
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
        .and_then(|r| r.as_array())
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

    // Validate matching target contents
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
        let segment = file_lines[chunk.start_line - 1..chunk.end_line].join("\n");
        if segment.trim_end() != chunk.target_content.trim_end() {
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
    }

    // Apply descending edits
    for chunk in chunks {
        let mut new_lines = Vec::new();
        new_lines.extend_from_slice(&file_lines[..chunk.start_line - 1]);
        new_lines.push(chunk.replacement_content);
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

    Ok(format!(
        "successfully applied {} replacements to '{path}'",
        replacements_val.len()
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
        .unwrap_or(false);

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

    let lines = content.lines().count().max(1);
    Ok(format!(
        "wrote '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}
