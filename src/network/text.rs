//! Pure response-text helpers: detecting cut-off/incomplete replies, stripping
//! `<think>` blocks and tool-call syntax, and small formatting utilities.
//!
//! Extracted from `network.rs`. All functions here are side-effect free and
//! depend only on `crate::tools` / `crate::config` for tool-call parsing.

pub(crate) fn has_intended_tool_call(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("```tool")
        || lower.contains("<tool_call>")
        || lower.contains("<tool_call ")
        || lower.contains("[tool_calls]")
        || lower.contains("<function_call>")
}

pub(crate) fn is_cut_off(content: &str, finish_reason: Option<&str>) -> bool {
    if finish_reason == Some("reasoning_loop") {
        return false;
    }

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

    // The model narrated an intended next action ("let me fix X, then Y")
    // but emitted no tool call at all — not even a malformed one. This has
    // real prose, so none of the checks above catch it, and the turn just
    // stalls silently waiting on the user. Treat a stated-but-unexecuted
    // intent as cut off so it gets nudged to actually act.
    if !has_intended_tool_call(content) && ends_with_stated_intent(content) {
        return true;
    }

    false
}

/// True when the prose (outside `<think>` blocks) ends on language that
/// promises an upcoming action rather than delivering one — "let me create
/// the README", "I'll fix the bug now" — with no trailing question that
/// would mark it as an actual final answer awaiting user input.
fn ends_with_stated_intent(content: &str) -> bool {
    let prose = strip_think_blocks(content);
    let tail = prose.trim();
    if tail.is_empty() || tail.ends_with('?') {
        return false;
    }
    let lower_tail: String = tail
        .chars()
        .rev()
        .take(200)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .to_lowercase();
    if lower_tail.contains("let me know") {
        return false;
    }
    const INTENT_PHRASES: &[&str] = &[
        "let me also",
        "let me now",
        "let me fix",
        "let me create",
        "let me write",
        "let me read",
        "let me inspect",
        "let me check",
        "let me verify",
        "let's fix",
        "let's create",
        "let's read",
        "let's check",
        "let's verify",
        "i'll fix",
        "i'll create",
        "i'll read",
        "i'll check",
        "i'll verify",
        "i will fix",
        "i will create",
        "i will read",
        "i will inspect",
        "i will check",
        "i will verify",
        "i need to fix",
        "i need to create",
        "i need to check",
        "i need to read",
        "going to fix",
        "going to create",
        "going to read",
        "going to check",
    ];
    INTENT_PHRASES.iter().any(|p| lower_tail.contains(p))
}

/// Wrap bare `thought` markers in `<think>` spans.
///
/// Some providers return their reasoning in the ordinary content channel with a
/// literal `thought` marker in front of it instead of in a `reasoning` delta.
/// Nothing downstream recognises that, so the reasoning was stored and shown as
/// the model's answer — transcripts read `thoughtThe grep command confirms...`
/// where a reply should be. Promoting the markers puts that text back under the
/// same `<think>` handling every other provider's reasoning goes through.
///
/// A span runs from its marker to the next marker, the next fenced block, or
/// the end of the text. Content that already carries `<think>` is left alone.
pub(crate) fn promote_bare_thought_markers(content: &str) -> String {
    const MARKER: &str = "thought";

    if content.contains("<think>") || !content.contains(MARKER) {
        return content.to_string();
    }

    // Only a marker that opens a line and is glued to the start of a sentence
    // counts. Ordinary prose about a "thought" sits mid-sentence, is followed by
    // a space, or continues the word in lower case ("thoughtful").
    let starts_span = |line: &str| {
        line.strip_prefix(MARKER)
            .is_some_and(|rest| rest.chars().next().is_some_and(char::is_uppercase))
    };

    let mut out = String::with_capacity(content.len() + 32);
    let mut open = false;
    for line in content.lines() {
        if starts_span(line) {
            if open {
                out.push_str("</think>\n");
            }
            out.push_str("<think>\n");
            open = true;
            out.push_str(&line[MARKER.len()..]);
            out.push('\n');
            continue;
        }
        if open && line.trim_start().starts_with("```") {
            out.push_str("</think>\n");
            open = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    if open {
        out.push_str("</think>\n");
    }
    out
}

/// Remove top-level `<think>...</think>` spans outside code blocks so we can
/// inspect the model's actual answer/tool output without corrupting code that
/// contains literal `<think>` tags.
pub(crate) fn strip_think_blocks(content: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    let mut fence_marker = "";
    let mut in_think = false;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !in_think && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            let marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
            if !in_fence {
                in_fence = true;
                fence_marker = marker;
            } else if trimmed.starts_with(fence_marker) {
                in_fence = false;
            }
            out.push_str(line);
            continue;
        }

        if in_fence {
            out.push_str(line);
            continue;
        }

        let mut rest = line;
        while !rest.is_empty() {
            if in_think {
                if let Some(end) = rest.find("</think>") {
                    in_think = false;
                    rest = &rest[end + "</think>".len()..];
                } else {
                    break;
                }
            } else {
                if let Some(start) = rest.find("<think>") {
                    out.push_str(&rest[..start]);
                    in_think = true;
                    rest = &rest[start + "<think>".len()..];
                } else {
                    out.push_str(rest);
                    break;
                }
            }
        }
    }
    out
}

/// Strip tool-call syntax from model text, leaving only human-readable prose.
/// Under the JSON tool protocol a model emits tool calls as text (```tool
/// fences or Mistral `[TOOL_CALLS]...[ARGS]{...}`), so on the forced wrap-up
/// turn — where we refuse to execute anything — we must remove that syntax
/// before saving, or the "answer" is a raw tool call. Returns the trimmed prose.
pub(crate) fn strip_tool_call_syntax(content: &str) -> String {
    let mut out = strip_think_blocks(content);

    // Remove ```tool ... ``` / ```json ... ``` fenced blocks (for ```json, only if it's a tool call).
    for fence in ["```tool", "```json"] {
        let mut search_from = 0;
        while let Some(relative_start) = out[search_from..].to_lowercase().find(fence) {
            let start = search_from + relative_start;
            let block_start = start + fence.len();
            let after_tag = &out[block_start..];
            let (rel_end, next_rel) = crate::tools::find_closing_tool_fence(after_tag);
            let block = &after_tag[..rel_end];

            let is_tool = fence == "```tool"
                || crate::tools::parse_tool_call(block, crate::config::ToolProtocol::Json)
                    .is_some();

            if is_tool {
                if rel_end < after_tag.len() {
                    let end = block_start + next_rel;
                    out.replace_range(start..end, "");
                    search_from = start;
                } else {
                    out.truncate(start); // unclosed fence — drop the remainder
                    break;
                }
            } else if rel_end < after_tag.len() {
                search_from = block_start + next_rel;
            } else {
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

/// Formats the assistant message payload sent on a continuation request.
/// Strips completed `<think>` blocks and bounds unclosed reasoning scratchpads
/// so massive reasoning traces (e.g. 20k-32k tokens) are not amplified and
/// resent verbatim on every continuation round.
pub(crate) fn format_continuation_assistant_message(previous: &str) -> String {
    let has_unclosed_think = previous.contains("<think>") && !previous.contains("</think>");
    if has_unclosed_think {
        // If cut off mid-thought, keep only the most recent thought suffix if long.
        const MAX_UNCLOSED_THINK_CHARS: usize = 1500;
        let think_start = previous.find("<think>").unwrap_or(0);
        let think_body = &previous[think_start + "<think>".len()..];
        if think_body.len() > MAX_UNCLOSED_THINK_CHARS {
            let tail = &think_body[think_body.len() - 1000..];
            return format!("<think>\n... [earlier thoughts omitted for brevity] ...\n{tail}");
        }
        return previous.to_string();
    }

    let prose = strip_think_blocks(previous);
    let trimmed = prose.trim();
    if !trimmed.is_empty() {
        trimmed.to_string()
    } else if previous.contains("<think>") {
        "(completed reasoning scratchpad)".to_string()
    } else {
        previous.to_string()
    }
}

/// Nudge sent to resume a cut-off turn, tailored to the reason the turn was interrupted.
pub(crate) fn continuation_nudge_for_category(
    previous: &str,
    finish_reason: Option<&str>,
) -> &'static str {
    if finish_reason == Some("length") {
        "Your previous response was cut off by the token limit. Continue directly from where you left off."
    } else if is_reasoning_only(previous) {
        "Stop planning and do not restate your plan again. Call the tool now."
    } else if previous.matches("```").count() % 2 != 0
        || (previous.contains("<tool_call>") && !previous.contains("</tool_call>"))
    {
        "Your tool call syntax was cut off. Continue the tool syntax directly."
    } else if !has_intended_tool_call(previous) && ends_with_stated_intent(previous) {
        "You stated your intended action. Please execute the tool call now."
    } else {
        "continue"
    }
}

/// Nudge sent to resume a cut-off turn.
#[allow(dead_code)]
pub(crate) fn continuation_nudge(previous: &str) -> &'static str {
    continuation_nudge_for_category(previous, None)
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

    // Regression: Gemini-family models return reasoning in the content channel
    // behind a bare `thought` marker, which used to be stored and displayed as
    // the model's answer ("thoughtThe grep command confirms...").
    #[test]
    fn bare_thought_markers_become_think_spans() {
        let raw = "thoughtThe grep confirms duct is used.\n```tool\n{\"name\": \"grep\"}\n```\nthoughtNow I will check Cargo.toml.";

        let promoted = promote_bare_thought_markers(raw);

        assert!(
            promoted.starts_with("<think>\nThe grep confirms"),
            "got: {promoted}"
        );
        // The fence closes the span so the tool call stays outside it.
        assert!(promoted.contains("</think>\n```tool"), "got: {promoted}");
        // The trailing span is closed at the end of the text.
        assert!(promoted.trim_end().ends_with("</think>"), "got: {promoted}");
        assert!(!strip_think_blocks(&promoted).contains("grep confirms"));
    }

    #[test]
    fn thought_promotion_leaves_ordinary_text_alone() {
        // Mid-sentence use of the word, and a marker followed by a space, are prose.
        let prose = "I had a thought about this.\nthought about the design";
        assert_eq!(
            promote_bare_thought_markers(prose),
            prose.to_string() + "\n"
        );

        // A word that merely starts with the marker is not a marker.
        let word = "thoughtful answers only";
        assert_eq!(promote_bare_thought_markers(word), word.to_string() + "\n");

        // Providers that already delimit reasoning are untouched.
        let delimited = "<think>plan</think>\n\nthoughtful answer here";
        assert_eq!(promote_bare_thought_markers(delimited), delimited);
    }

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

    #[test]
    fn test_is_cut_off_stated_intent_without_action() {
        let content = "<think>\nreview stuff\n</think>\n\nI need to fix some bugs in `main.js` — undefined variables (`ny`, `ny2`) in\nthe tunnel/bridge wall rendering, and simplify the finish line logic. Let me\nalso create the README.";
        assert!(is_cut_off(content, None));
    }

    #[test]
    fn test_is_cut_off_ignores_genuine_final_answers() {
        let content = "<think>done</think>\n\nAll files are created and the game runs end to end. Let me know if you'd like any tweaks.";
        assert!(!is_cut_off(content, None));
    }

    #[test]
    fn strip_think_blocks_preserves_code_fences_with_think_tags() {
        let input = "<think>\nThinking about xml structure\n</think>\n\nHere is the config:\n```xml\n<think>This is literal XML</think>\n```\nDone.";
        let stripped = strip_think_blocks(input);
        assert!(!stripped.contains("Thinking about xml structure"));
        assert!(stripped.contains("```xml\n<think>This is literal XML</think>\n```"));
        assert!(stripped.contains("Done."));
    }

    #[test]
    fn strip_think_blocks_drops_unclosed_thought_at_end() {
        let input = "<think>\nUnfinished thought...";
        let stripped = strip_think_blocks(input);
        assert_eq!(stripped.trim(), "");
    }

    #[test]
    fn test_has_intended_tool_call_distinguishes_json_prose_from_tool_calls() {
        assert!(has_intended_tool_call(
            "```tool\n{\"name\": \"run_command\"}\n```"
        ));
        assert!(has_intended_tool_call(
            "<tool_call>{\"name\": \"run_command\"}</tool_call>"
        ));
        assert!(has_intended_tool_call("[TOOL_CALLS]run_command[ARGS]{}"));
        assert!(has_intended_tool_call(
            "<function_call>run_command()</function_call>"
        ));

        // Regular markdown json blocks should not trigger malformed tool call handling
        assert!(!has_intended_tool_call(
            "Here is your decrypted savegame:\n```json\n{\"seeds\": 580, \"potatoes\": 2423}\n```"
        ));
        assert!(!has_intended_tool_call(
            "```json\n{\"key\": \"value\"}\n```"
        ));
    }

    #[test]
    fn test_strip_tool_call_syntax_preserves_plain_json_blocks() {
        let input = "Here is the data:\n```json\n{\"seeds\": 580, \"potatoes\": 2423}\n```\nDone.";
        let stripped = strip_tool_call_syntax(input);
        assert!(stripped.contains("```json\n{\"seeds\": 580, \"potatoes\": 2423}\n```"));
        assert!(stripped.contains("Done."));

        let tool_input = "Running tool:\n```json\n{\"name\": \"run_command\", \"arguments\": {\"command_line\": \"ls\"}}\n```\nDone.";
        let stripped_tool = strip_tool_call_syntax(tool_input);
        assert!(!stripped_tool.contains("run_command"));
        assert_eq!(stripped_tool.trim(), "Running tool:\n\nDone.");
    }
}
