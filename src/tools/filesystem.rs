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
    let requested_end = args.get("end_line").and_then(parse_json_number).map(|v| v as usize);
    let end_line = requested_end.unwrap_or(start_line + 2000);

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
        // A read that stopped where the caller asked it to is not truncated.
        // Calling it truncated tells the model it is missing something, and a
        // model that thinks it is missing something reads the file again — which
        // is exactly the loop this produced when a one-line read reported itself
        // as cut short.
        if requested_end.is_some() {
            out.push_str(&format!(
                "... end of requested range; the file continues to line {total} ...\n"
            ));
        } else {
            out.push_str(
                "... content truncated (use end_line or content_offset to read more) ...\n",
            );
        }
    }

    Ok(out)
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
    if !content.contains(target) {
        // The replacement is present and the original anchor is gone: this
        // transformation already happened.
        return true;
    }
    if !replacement.contains(target) {
        // Target still present and replacement doesn't subsume it — this
        // isn't the insert-shaped case, so don't guess; let normal matching
        // decide (it may be a genuine remaining occurrence to edit).
        return false;
    }
    // Every remaining occurrence of `target` must fall entirely inside an
    // occurrence of `replacement` for this to count as already applied —
    // otherwise there is a genuine, separate site still needing the edit.
    let repl_ranges: Vec<(usize, usize)> = content
        .match_indices(replacement)
        .map(|(i, _)| (i, i + replacement.len()))
        .collect();
    content.match_indices(target).all(|(i, _)| {
        let end = i + target.len();
        repl_ranges.iter().any(|&(rs, re)| rs <= i && end <= re)
    })
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

fn apply_single_edit_to_content(content: &str, path: &str, edit: &SingleEdit) -> Result<EditOutcome, String> {
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

fn apply_single_edit_to_content_inner(content: &str, path: &str, edit: &SingleEdit) -> Result<EditOutcome, String> {
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
        let outcome = apply_single_edit_to_content(&current_content, path, edit)
            .map_err(|e| if chunks.len() > 1 { format!("Edit #{}: {}", idx + 1, e) } else { e })?;

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
            format!("already applied; no changes made to '{path}' (target_content already reflects replacement_content)")
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
        format!("successfully replaced target_content in '{path}'\n\n```diff\n{combined_diffs}\n```")
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

    // Regression: session 1785594233488. A read of exactly lines 1-1 ended with
    // "content truncated (use end_line or content_offset to read more)", so the
    // model believed it had missed something and re-read the same file four
    // times, tripping the loop detector.
    #[test]
    fn a_read_that_ends_where_asked_is_not_reported_as_truncated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("symbols.rs");
        std::fs::write(&file, "// scratch\nuse std::fs;\nfn main() {}\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let ranged = view_file_tool(&serde_json::json!({
            "path": path,
            "start_line": 1,
            "end_line": 1,
        }))
        .expect("read");
        assert!(ranged.contains("1: // scratch"), "got: {ranged}");
        assert!(!ranged.contains("truncated"), "got: {ranged}");
        assert!(ranged.contains("the file continues to line 3"), "got: {ranged}");

        // A read the tool itself cut short still says so.
        let whole = view_file_tool(&serde_json::json!({ "path": path })).expect("read");
        assert!(!whole.contains("truncated"), "got: {whole}");
        assert!(!whole.contains("continues"), "got: {whole}");
    }

    // Regression: session 1785594233488. The model tried to prepend a line by
    // sending an empty old_string; it matched at every offset and came back as
    // "found 13573 matches", which reads like the file is at fault. The model
    // then retried the same shape through multi_replace and gave up on editing.
    #[test]
    fn empty_edit_anchor_is_rejected_with_insert_guidance() {
        let edit = SingleEdit {
            target: String::new(),
            replacement: "// scratch\n".to_string(),
            start_line: None,
            end_line: None,
        };

        let error = apply_single_edit_to_content("line one\nline two\n", "src/symbols.rs", &edit)
            .expect_err("an empty anchor cannot identify an edit site");

        assert!(error.contains("empty"), "got: {error}");
        assert!(error.contains("matches everywhere"), "got: {error}");
        // Points at what to do instead, since the model wanted to insert.
        assert!(error.contains("prepend"), "got: {error}");
        assert!(!error.contains("found 2 matches"), "got: {error}");
    }

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
        let out = match apply_single_edit_to_content(file, "src/ui/mod.rs", &edit)
            .expect("should resolve via block-anchor fuzzy match")
        {
            EditOutcome::Changed(content) => content,
            EditOutcome::Unchanged => panic!("expected a change"),
        };
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

    #[test]
    fn closest_region_is_bounded_for_large_inputs() {
        let content = (0..10_000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let target = (0..100)
            .map(|i| format!("target {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(closest_region(&content, &target).is_none());
    }

    // Regression for the benchmark session: a prepend-shaped edit (old_string
    // is a suffix of new_string) kept matching after it had already been
    // applied, so repeating the exact same tool call stacked the inserted
    // line forever.
    #[test]
    fn repeating_the_same_prepend_edit_twice_does_not_duplicate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "    s.discord_rpc.set_activity(\"Idle\", ...);\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_string": "    s.discord_rpc.set_activity(\"Idle\", ...);",
            "new_string": "    let model_name = ...;\n    s.discord_rpc.set_activity(\"Idle\", ...);",
        });

        let first = replace_file_content_tool(&args).expect("first apply should succeed");
        assert!(first.contains("successfully"), "got: {first}");
        let after_first = std::fs::read_to_string(&file).expect("read");
        assert_eq!(
            after_first.matches("let model_name").count(),
            1,
            "got: {after_first}"
        );

        let second = replace_file_content_tool(&args).expect("repeat should not error");
        assert!(second.contains("already applied"), "got: {second}");
        let after_second = std::fs::read_to_string(&file).expect("read");
        assert_eq!(
            after_second, after_first,
            "repeating the edit must not change the file further"
        );
        assert_eq!(
            after_second.matches("let model_name").count(),
            1,
            "the inserted line must not be duplicated: {after_second}"
        );
    }

    // Repeating a plain (non-insert-shaped) replacement after it already
    // landed must be a reported no-op, not an error and not a second write.
    #[test]
    fn repeating_a_plain_replacement_is_a_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let status = Idle;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_string": "let status = Idle;",
            "new_string": "let status = Active;",
        });

        let first = replace_file_content_tool(&args).expect("first apply should succeed");
        assert!(first.contains("successfully"), "got: {first}");

        let second = replace_file_content_tool(&args).expect("repeat should be a no-op");
        assert!(second.contains("already applied"), "got: {second}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "let status = Active;\n");
    }

    // A different, still-valid edit must keep working after the file has
    // already been changed by a prior edit — the idempotency guard must not
    // block genuine follow-up edits.
    #[test]
    fn a_legitimate_second_edit_still_works_after_the_file_changed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let first = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "let a = 1;",
            "new_string": "let a = 100;",
        }))
        .expect("first edit should succeed");
        assert!(first.contains("successfully"), "got: {first}");

        let second = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "let b = 2;",
            "new_string": "let b = 200;",
        }))
        .expect("second, distinct edit should succeed");
        assert!(second.contains("successfully"), "got: {second}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "let a = 100;\nlet b = 200;\n");
    }

    // Ambiguous edits must still be rejected — the idempotency guard must
    // never paper over a genuinely ambiguous target.
    #[test]
    fn multiple_matches_remain_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "dup();\ndup();\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let err = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "dup();",
            "new_string": "single();",
        }))
        .expect_err("ambiguous target must be rejected");
        assert!(err.contains("matches"), "got: {err}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "dup();\ndup();\n", "file must be untouched");
    }

    // A failed edit (target genuinely not present) must return Err, never a
    // success message — success-shaped failures are what let a broken edit
    // loop pass the harness's "did the tool report success" check.
    #[test]
    fn failed_edits_do_not_report_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let err = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "let z = 999;",
            "new_string": "let z = 1000;",
        }))
        .expect_err("target not present must fail");
        assert!(err.contains("not found"), "got: {err}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "let a = 1;\n", "file must be untouched on failure");
    }

    // multi_replace_file_content_tool must share the same idempotency
    // protection as the single-edit path.
    #[test]
    fn multi_replace_repeated_call_is_a_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let args = serde_json::json!({
            "path": path,
            "replacements": [
                { "start_line": 1, "end_line": 1, "target_content": "let a = 1;", "replacement_content": "let a = 100;" },
            ],
        });

        let first = multi_replace_file_content_tool(&args).expect("first apply should succeed");
        assert!(first.contains("successfully"), "got: {first}");

        let second = multi_replace_file_content_tool(&args).expect("repeat should be a no-op");
        assert!(second.contains("already applied"), "got: {second}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "let a = 100;\nlet b = 2;\n");
    }

    fn diff_block_of(result: &str) -> &str {
        result
            .split_once("```diff\n")
            .and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(diff, _)| diff)
            .unwrap_or_else(|| panic!("expected a ```diff block in: {result}"))
    }

    // Feature 4 regression: the diff shown to the caller must reflect the real
    // file content before/after the edit, not the tool call's raw arguments.
    // A one-line replacement must produce a real, correctly numbered diff.
    #[test]
    fn one_line_replacement_produces_real_diff_with_correct_line_numbers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\nlet c = 3;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "let b = 2;",
            "new_string": "let b = 200;",
        }))
        .expect("edit should succeed");

        let diff = diff_block_of(&result);
        assert!(diff.contains("@@ -1,3 +1,3 @@"), "got: {diff}");
        assert!(diff.contains("-let b = 2;"), "got: {diff}");
        assert!(diff.contains("+let b = 200;"), "got: {diff}");
        // Unrelated lines are untouched context, not fabricated changes.
        assert!(diff.contains(" let a = 1;"), "got: {diff}");
        assert!(diff.contains(" let c = 3;"), "got: {diff}");
    }

    // An insertion (prepend-shaped edit, PR #306's example) must produce a
    // diff showing only the inserted line as `+`, with the untouched anchor
    // line shown as context — not as a fabricated replacement of the whole
    // target block.
    #[test]
    fn insertion_produces_real_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "    s.discord_rpc.set_activity(\"Idle\", ...);\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "    s.discord_rpc.set_activity(\"Idle\", ...);",
            "new_string": "    let model_name = ...;\n    s.discord_rpc.set_activity(\"Idle\", ...);",
        }))
        .expect("insertion should succeed");

        let diff = diff_block_of(&result);
        assert!(diff.contains("+    let model_name = ...;"), "got: {diff}");
        assert!(
            diff.contains(" ") && diff.contains("s.discord_rpc.set_activity"),
            "unchanged anchor line should appear as context, not a fabricated change: {diff}"
        );
        assert!(
            !diff.contains("-    s.discord_rpc.set_activity"),
            "the anchor line was never removed, so it must not appear as deleted: {diff}"
        );
    }

    // A deletion must produce a diff with only `-` lines for the removed
    // content, reflecting the real file, not argument text.
    #[test]
    fn deletion_produces_real_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\nlet c = 3;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "let b = 2;\n",
            "new_string": "",
        }))
        .expect("deletion should succeed");

        let diff = diff_block_of(&result);
        assert!(diff.contains("-let b = 2;"), "got: {diff}");
        assert!(!diff.contains("+let b"), "got: {diff}");

        let content = std::fs::read_to_string(&file).expect("read");
        assert_eq!(content, "let a = 1;\nlet c = 3;\n");
    }

    // Multiple edits in one call must produce ONE diff reflecting the
    // combined real before/after content, with correct line numbers for
    // each hunk — not a per-edit fabrication stitched from arguments.
    #[test]
    fn multiple_edits_produce_one_combined_diff_with_correct_hunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n") + "\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "edits": [
                { "target_content": "line 2\n", "replacement_content": "LINE TWO\n" },
                { "target_content": "line 18", "replacement_content": "LINE EIGHTEEN" },
            ],
        }))
        .expect("multi-edit should succeed");

        let diff = diff_block_of(&result);
        // Two separate hunks, far enough apart that they don't share context.
        let hunk_count = diff.matches("@@").count() / 2;
        assert_eq!(hunk_count, 2, "expected two separate hunks, got: {diff}");
        assert!(diff.contains("-line 2"), "got: {diff}");
        assert!(diff.contains("+LINE TWO"), "got: {diff}");
        assert!(diff.contains("-line 18"), "got: {diff}");
        assert!(diff.contains("+LINE EIGHTEEN"), "got: {diff}");
        // The second hunk's line numbers must reflect its real position near
        // line 18, not line 1 or an argument-derived number.
        assert!(
            diff.contains("@@ -16,") || diff.contains("@@ -15,"),
            "second hunk should be anchored near line 18, got: {diff}"
        );
    }

    // A repeated no-op edit (PR #306's idempotency case) must produce NO
    // diff at all in the response, not a fake one derived from arguments.
    #[test]
    fn repeated_noop_edit_produces_no_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let status = Idle;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let args = serde_json::json!({
            "path": path,
            "old_string": "let status = Idle;",
            "new_string": "let status = Active;",
        });

        let first = replace_file_content_tool(&args).expect("first apply should succeed");
        assert!(
            first.contains("```diff"),
            "first real edit should carry a diff: {first}"
        );

        let second = replace_file_content_tool(&args).expect("repeat should be a no-op");
        assert!(second.contains("already applied"), "got: {second}");
        assert!(
            !second.contains("```diff"),
            "a no-op edit must not fabricate a diff: {second}"
        );
    }

    // Core regression: an edit far from the start of the file must report
    // truthful line numbers in the diff hunk header — not numbers derived
    // from the argument text (which would always start at line 1).
    #[test]
    fn diff_line_numbers_reflect_real_position_in_file_not_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("big.rs");
        let lines: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n") + "\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = replace_file_content_tool(&serde_json::json!({
            "path": path,
            "old_string": "line 50",
            "new_string": "line fifty",
        }))
        .expect("edit should succeed");

        let diff = diff_block_of(&result);
        // Context radius is 3, so the hunk should start around line 47, and
        // must NOT claim to start at line 1 as argument-derived numbering
        // would (target text "line 50" alone has no line context).
        assert!(
            diff.contains("@@ -47,"),
            "expected a hunk anchored near line 50, got: {diff}"
        );
        assert!(
            !diff.contains("@@ -1,"),
            "must not fabricate line 1 from arguments: {diff}"
        );
    }

    // multi_replace_file_content_tool must also produce a real diff from the
    // true before/after file content, not per-replacement argument text.
    #[test]
    fn multi_replace_produces_real_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n") + "\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let result = multi_replace_file_content_tool(&serde_json::json!({
            "path": path,
            "replacements": [
                { "start_line": 18, "end_line": 18, "target_content": "line 18", "replacement_content": "LINE EIGHTEEN" },
            ],
        }))
        .expect("replacement should succeed");

        let diff = diff_block_of(&result);
        assert!(diff.contains("-line 18"), "got: {diff}");
        assert!(diff.contains("+LINE EIGHTEEN"), "got: {diff}");
        assert!(
            !diff.contains("@@ -1,"),
            "must reflect real position, not line 1: {diff}"
        );
    }

    // A no-op multi_replace call (all chunks already applied) must not
    // fabricate a diff either.
    #[test]
    fn multi_replace_repeated_noop_produces_no_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("state.rs");
        std::fs::write(&file, "let a = 1;\nlet b = 2;\n").expect("write");
        let path = file.to_string_lossy().to_string();

        let args = serde_json::json!({
            "path": path,
            "replacements": [
                { "start_line": 1, "end_line": 1, "target_content": "let a = 1;", "replacement_content": "let a = 100;" },
            ],
        });

        let first = multi_replace_file_content_tool(&args).expect("first apply should succeed");
        assert!(first.contains("```diff"), "got: {first}");

        let second = multi_replace_file_content_tool(&args).expect("repeat should be a no-op");
        assert!(second.contains("already applied"), "got: {second}");
        assert!(
            !second.contains("```diff"),
            "a no-op must not fabricate a diff: {second}"
        );
    }
}
