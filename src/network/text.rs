//! Pure response-text helpers: detecting cut-off/incomplete replies, stripping
//! `<think>` blocks and tool-call syntax, and small formatting utilities.
//!
//! Extracted from `network.rs`. All functions here are side-effect free and
//! depend only on `crate::tools` / `crate::config` for tool-call parsing.

pub(crate) fn has_intended_tool_call(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("```tool") || lower.contains("```json") || lower.contains("[tool_calls]")
}

pub(crate) fn is_cut_off(content: &str, finish_reason: Option<&str>) -> bool {
    // If the model already produced a valid tool call, we don't need to continue text generation.
    // We should execute the tool and get its output first.
    if !crate::tools::parse_tool_calls(content, crate::config::ToolProtocol::Native).is_empty() {
        return false;
    }

    if finish_reason == Some("length") {
        return true;
    }

    // Check for unclosed <think> tag
    let has_think = content.contains("<think>");
    let has_think_end = content.contains("</think>");
    if has_think && !has_think_end {
        return true;
    }

    // Check for unclosed tool block
    let triple_backticks_count = content.matches("```").count();
    if !triple_backticks_count.is_multiple_of(2) {
        return true;
    }

    // Check for unclosed <tool_call> tag
    let has_tool_call = content.contains("<tool_call>");
    let has_tool_call_end = content.contains("</tool_call>");
    if has_tool_call && !has_tool_call_end {
        return true;
    }

    // Qwen-family open models often close </think> and then emit a stop token
    // with no actual answer or tool call. Treat that as incomplete so the
    // continuation path nudges the model instead of stalling for a manual
    // "continue".
    if is_reasoning_only(content) {
        return true;
    }

    false
}

/// Remove every `<think>...</think>` span so we can inspect the model's actual
/// answer/tool output.
pub(crate) fn strip_think_blocks(content: &str) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            // unclosed — drop the remainder (handled by the unclosed-think check)
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Strip tool-call syntax from model text, leaving only human-readable prose.
/// Under the JSON tool protocol a model emits tool calls as text (```tool
/// fences or Mistral `[TOOL_CALLS]...[ARGS]{...}`), so on the forced wrap-up
/// turn — where we refuse to execute anything — we must remove that syntax
/// before saving, or the "answer" is a raw tool call. Returns the trimmed prose.
pub(crate) fn strip_tool_call_syntax(content: &str) -> String {
    let mut out = strip_think_blocks(content);

    // Remove ```tool ... ``` / ```json ... ``` fenced blocks.
    for fence in ["```tool", "```json"] {
        while let Some(start) = out.to_lowercase().find(fence) {
            if let Some(rel_end) = out[start + fence.len()..].find("```") {
                let end = start + fence.len() + rel_end + 3;
                out.replace_range(start..end, "");
            } else {
                out.truncate(start); // unclosed fence — drop the remainder
                break;
            }
        }
    }

    // Remove Mistral-style `[TOOL_CALLS]name[ARGS]{...}` spans (drop to end of
    // the JSON object if we can find the matching brace, else to end of string).
    while let Some(start) = out.find("[TOOL_CALLS]") {
        let after = &out[start..];
        let end = after
            .find('{')
            .and_then(|b| {
                let mut depth = 0i32;
                for (i, c) in after[b..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(start + b + i + 1);
                            }
                        }
                        _ => {}
                    }
                }
                None
            })
            .unwrap_or(out.len());
        out.replace_range(start..end, "");
    }

    out.trim().to_string()
}

/// True when the turn is nothing but reasoning: a non-empty response whose only
/// content is `<think>` blocks, leaving no answer or tool call to act on.
pub(crate) fn is_reasoning_only(content: &str) -> bool {
    if content.trim().is_empty() {
        return false;
    }
    strip_think_blocks(content).trim().is_empty()
}

/// Cap a diff preview at 10 lines, appending a "... (N more lines changed)"
/// footer so long edits don't flood the status stream.
pub(crate) fn cap_diff_lines(prev: String) -> String {
    if prev.trim().is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = prev.lines().collect();
    if lines.len() > 10 {
        let total = lines.len();
        let mut capped = lines[..10].join("\n");
        capped.push_str(&format!("\n ... ({} more lines changed)", total - 10));
        capped
    } else {
        prev
    }
}

/// Strip ANSI colour/escape sequences from compiler or command output.
pub(crate) fn strip_ansi_escapes(s: &str) -> String {
    static ANSI_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1B\[[0-9;?]*[a-zA-Z]").unwrap());
    ANSI_RE.replace_all(s, "").into_owned()
}

/// Drop a single leading `<think>...</think>` block, returning the prose that
/// follows. Leaves text untouched when there is no leading block.
pub(crate) fn strip_leading_think(text: &str) -> &str {
    match (
        text.trim_start().starts_with("<think>"),
        text.find("</think>"),
    ) {
        (true, Some(i)) => text[i + "</think>".len()..].trim_start(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_leading_think() {
        assert_eq!(
            strip_leading_think("<think>\nreasoning here\n</think>\n\nfinal answer"),
            "final answer"
        );
        assert_eq!(strip_leading_think("plain reply"), "plain reply");
        // </think> mentioned mid-text without a leading block: untouched
        assert_eq!(
            strip_leading_think("text about </think> tags"),
            "text about </think> tags"
        );
    }

    #[test]
    fn test_is_reasoning_only() {
        // pure reasoning, no answer → stall we want to auto-continue
        assert!(is_reasoning_only("<think>\nlet me plan\n</think>"));
        assert!(is_reasoning_only("<think>plan</think>\n\n  \n"));
        // reasoning followed by a real answer → complete
        assert!(!is_reasoning_only(
            "<think>plan</think>\n\nhere is the answer"
        ));
        // reasoning followed by a tool call → complete
        assert!(!is_reasoning_only(
            "<think>plan</think>\n```tool\n{\"name\":\"get_time\"}\n```"
        ));
        assert!(!is_reasoning_only(
            "<think>plan</think>\n<tool_call>{\"name\":\"get_time\"}</tool_call>"
        ));
        // empty content is handled by the caller, not treated as reasoning-only
        assert!(!is_reasoning_only("   "));
        assert!(!is_reasoning_only("just a normal reply"));
    }

    #[test]
    fn test_is_cut_off_reasoning_only() {
        assert!(is_cut_off("<think>thinking</think>", None));
        assert!(!is_cut_off("<think>thinking</think>\n\nthe answer", None));
    }

    #[test]
    fn test_strip_ansi_escapes() {
        let input = "\x1B[31mError\x1B[0m: compile failed \x1B[1mline 5\x1B[0m";
        let output = strip_ansi_escapes(input);
        assert_eq!(output, "Error: compile failed line 5");
    }
}
