use serde_json::Value;
use std::path::PathBuf;

// Re-exports needed by filesystem tools
pub(crate) use super::coerce_array;
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
    if resolved_path.is_dir() {
        return super::search::list_directory(args);
    }
    let start_line = args
        .get("start_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(1);
    let end_line = args
        .get("end_line")
        .and_then(parse_json_number)
        .map(|v| v as usize)
        .unwrap_or(start_line + 2000);

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

pub fn generate_unified_diff(target: &str, replacement: &str, start_line: Option<usize>) -> String {
    let diff = similar::TextDiff::from_lines(target, replacement);
    let mut out = String::new();
    let old_count = target.lines().count();
    let new_count = replacement.lines().count();
    let line = start_line.unwrap_or(1);
    out.push_str(&format!("@@ -{line},{old_count} +{line},{new_count} @@\n"));
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => "-",
            similar::ChangeTag::Insert => "+",
            similar::ChangeTag::Equal => " ",
        };
        let val = change.value();
        out.push_str(sign);
        out.push_str(val);
        if !val.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

struct SingleEdit {
    target: String,
    replacement: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

fn extract_edit_chunks(args: &Value) -> Result<Vec<SingleEdit>, String> {
    let get_alias = |v: &Value, keys: &[&str]| -> Option<String> {
        for &k in keys {
            if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
                return Some(s.to_string());
            }
        }
        None
    };

    let target_keys = &["target_content", "target", "old_string", "old_text", "oldString", "oldText"];
    let replacement_keys = &["replacement_content", "replacement", "new_string", "new_text", "newString", "newText"];

    if let Some(edits_arr) = args.get("edits").and_then(coerce_array) {
        if edits_arr.is_empty() {
            return Err("edits array cannot be empty".to_string());
        }
        let mut chunks = Vec::new();
        for (i, item) in edits_arr.iter().enumerate() {
            let target = get_alias(item, target_keys)
                .ok_or_else(|| format!("edits[{i}] is missing target_content/old_string"))?;
            let replacement = get_alias(item, replacement_keys)
                .ok_or_else(|| format!("edits[{i}] is missing replacement_content/new_string"))?;
            let start_line = item.get("start_line").and_then(parse_json_number).map(|v| v as usize);
            let end_line = item.get("end_line").and_then(parse_json_number).map(|v| v as usize);
            chunks.push(SingleEdit { target, replacement, start_line, end_line });
        }
        Ok(chunks)
    } else {
        let target = get_alias(args, target_keys)
            .ok_or("missing 'target_content' (or 'old_string') argument")?;
        let replacement = get_alias(args, replacement_keys)
            .ok_or("missing 'replacement_content' (or 'new_string') argument")?;
        let start_line = args.get("start_line").and_then(parse_json_number).map(|v| v as usize);
        let end_line = args.get("end_line").and_then(parse_json_number).map(|v| v as usize);
        Ok(vec![SingleEdit { target, replacement, start_line, end_line }])
    }
}

fn apply_single_edit_to_content(content: &str, path: &str, edit: &SingleEdit) -> Result<String, String> {
    if edit.target == edit.replacement {
        return Err("old_string and new_string are identical".to_string());
    }

    let target_content = &edit.target;
    let replacement_content = &edit.replacement;

    // 1. Line range matching (with +-15 tolerance window)
    if let (Some(start), Some(end)) = (edit.start_line, edit.end_line) {
        let file_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let total = file_lines.len();

        let window_start = start.saturating_sub(15).max(1);
        let window_end = (end + 15).min(total);

        if window_start <= window_end {
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
                    return Ok(new_content);
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
                return Ok(new_content);
            }
        }
    }

    // 2. Exact semantic matching anywhere in content
    let occurrences: Vec<_> = content.match_indices(target_content).collect();
    if occurrences.len() == 1 {
        let (index, _) = occurrences[0];
        let mut new_content = content.to_string();
        new_content.replace_range(index..index + target_content.len(), replacement_content);
        return Ok(new_content);
    } else if occurrences.len() > 1 {
        return Err(format!(
            "Error: found {} matches for target_content in '{path}'. Either include more surrounding context lines to make it unique, or pass `start_line`/`end_line` to target the specific occurrence you mean (the edit is anchored within that range).",
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
        return Ok(new_content);
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
            return Ok(new_content);
        }
    }

    // 5. Failure feedback
    let mut err_msg = format!(
        "Error: target_content not found in '{path}'.\n\
         Please check that your target content matches the file."
    );
    if let (Some(start), Some(end)) = (edit.start_line, edit.end_line) {
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

/// Slide a target-sized window over the file and return the byte-line range
/// (start_idx, end_idx_exclusive, ratio) of the most similar region, comparing
/// lines by trimmed equality. Used purely to build actionable error feedback.
fn closest_region(content: &str, target: &str) -> Option<(usize, usize, f32)> {
    let content_lines: Vec<&str> = content.lines().collect();
    let target_lines: Vec<&str> = target.lines().collect();
    if content_lines.is_empty() || target_lines.is_empty() {
        return None;
    }
    let win = target_lines.len().min(content_lines.len());
    let mut best: Option<(usize, usize, f32)> = None;
    for i in 0..=(content_lines.len() - win) {
        // Char-level similarity so near-misses (an escape or a typo) rank above
        // unrelated lines — line-equality alone scores every mismatch as 0 and
        // would just return the first window.
        let window = content_lines[i..i + win].join("\n");
        let ratio = similar::TextDiff::from_chars(window.as_str(), target).ratio();
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

    let chunks = extract_edit_chunks(args)?;
    let mut current_content = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("cannot read '{path}': {e}"))?;

    let mut combined_diffs = String::new();
    for (idx, edit) in chunks.iter().enumerate() {
        let diff = generate_unified_diff(&edit.target, &edit.replacement, edit.start_line);
        if !combined_diffs.is_empty() {
            combined_diffs.push_str("\n");
        }
        combined_diffs.push_str(&diff);

        let new_content = apply_single_edit_to_content(&current_content, path, edit)
            .map_err(|e| if chunks.len() > 1 { format!("Edit #{}: {}", idx + 1, e) } else { e })?;
        current_content = new_content;
    }

    std::fs::write(&resolved_path, &current_content)
        .map_err(|e| format!("cannot write '{path}': {e}"))?;

    let msg = if chunks.len() == 1 {
        format!("successfully replaced target_content in '{path}'\n\n```diff\n{combined_diffs}\n```")
    } else {
        format!("successfully applied {} edits in '{path}'\n\n```diff\n{combined_diffs}\n```", chunks.len())
    };

    Ok(msg)
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

        let mut anchor_matches: Vec<(usize, usize, f32)> = Vec::new();
        for i in 0..content_lines.len() {
            if content_lines[i].trim() != first_anchor {
                continue;
            }
            // Consider EVERY line matching the closing anchor, not just the
            // first. The first `last_anchor` is frequently a nested delimiter
            // (e.g. an inner `}` closing a loop before the block's own `}`), so
            // breaking on it discards the real end and the whole match fails.
            for j in (i + 2)..content_lines.len() {
                if content_lines[j].trim() != last_anchor {
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
                        .filter(|(c, t)| c.trim() == t.trim())
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

    let lines = content.lines().count();
    Ok(format!(
        "wrote '{path}' ({lines} lines, {} bytes)",
        content.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression: session 1785315367588 looped because the model's target
    // block had a single backslash in the char literal (`'\'`) where the file
    // has an escaped one (`'\\'`), and the block-anchor fallback bailed on the
    // inner `}` before reaching the function's closing `}`.
    #[test]
    fn resolves_block_with_single_line_escape_drift() {
        let file = "pub fn show_spinner() {\n\
                        let spinner = vec!['|', '/', '-', '\\\\'];\n\
                    let mut i = 0;\n\
                    while running {\n\
                    print!(\"x\");\n\
                    }\n\
                    flush();\n\
                    }\n";
        // Target differs only on the spinner line (one backslash, not two).
        let edit = SingleEdit {
            target: "pub fn show_spinner() {\n\
                     let spinner = vec!['|', '/', '-', '\\'];\n\
                     let mut i = 0;\n\
                     while running {\n\
                     print!(\"x\");\n\
                     }\n\
                     flush();\n\
                     }"
                .to_string(),
            replacement: "REPLACED".to_string(),
            start_line: None,
            end_line: None,
        };
        let out = apply_single_edit_to_content(file, "src/ui/mod.rs", &edit)
            .expect("should resolve via block-anchor fuzzy match");
        assert!(out.contains("REPLACED"), "got: {out}");
        assert!(!out.contains("show_spinner"), "old block should be gone: {out}");
    }

    #[test]
    fn missing_target_error_shows_closest_region() {
        let file = "line one\nlet x = 1;\nline three\n";
        let edit = SingleEdit {
            target: "let x = 2;".to_string(),
            replacement: "let x = 3;".to_string(),
            start_line: None,
            end_line: None,
        };
        let err = apply_single_edit_to_content(file, "f.rs", &edit).unwrap_err();
        assert!(err.contains("Closest region"), "got: {err}");
        assert!(err.contains("let x = 1;"), "should quote actual line: {err}");
    }
}
