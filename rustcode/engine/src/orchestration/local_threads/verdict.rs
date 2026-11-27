//! Classification and plain-text verdict extraction for scheduled job replies.

/// How a Loop run's answer starts when it found nothing for the user; the
/// run's fresh thread is then archived.
pub(crate) const LOOP_QUIET_MARKER: &str = "Nothing to report";

const MAX_VERDICT_CHARS: usize = 180;

/// What a finished scheduled-job turn actually reported, separate from whether
/// the underlying harness turn succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerdictKind {
    Quiet,
    Clear,
    Attention,
    Blocked,
}

impl VerdictKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Clear => "clear",
            Self::Attention => "attention",
            Self::Blocked => "blocked",
        }
    }
}

/// Remove inline Markdown formatting so verdicts render cleanly in plain-text
/// previews without stray `**`, backticks, or raw link URLs.
pub(crate) fn strip_inline_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '['
            && let Some((label, next)) = parse_markdown_link(&chars, i)
        {
            out.push_str(&strip_inline_markdown(&label));
            i = next;
            continue;
        }
        if (ch == '*' || ch == '_' || ch == '~') && chars.get(i + 1) == Some(&ch) {
            i += 2;
            continue;
        }
        if ch == '`' {
            i += 1;
            continue;
        }
        if ch == '*' {
            let prev_space = i == 0 || chars[i - 1].is_whitespace();
            let next_space = i + 1 == chars.len() || chars[i + 1].is_whitespace();
            if !(prev_space && next_space) {
                i += 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_markdown_link(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut close_bracket = start + 1;
    while close_bracket < chars.len() && chars[close_bracket] != ']' {
        if chars[close_bracket] == '\n' {
            return None;
        }
        close_bracket += 1;
    }
    if close_bracket + 1 >= chars.len() || chars[close_bracket + 1] != '(' {
        return None;
    }
    let mut close_paren = close_bracket + 2;
    let mut depth = 1usize;
    while close_paren < chars.len() && depth > 0 {
        match chars[close_paren] {
            '\n' => return None,
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        close_paren += 1;
    }
    if depth != 0 {
        return None;
    }
    let label: String = chars[start + 1..close_bracket].iter().collect();
    Some((label, close_paren))
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

fn trim_label_separator(rest: &str) -> &str {
    rest.trim_start_matches([':', '—', '–', '-', ' ']).trim()
}

fn has_word_boundary(rest: &str) -> bool {
    rest.chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

fn clip_verdict(text: &str) -> String {
    let clipped: String = text.chars().take(MAX_VERDICT_CHARS).collect();
    if text.chars().count() > MAX_VERDICT_CHARS {
        format!("{}…", clipped.trim_end())
    } else {
        clipped
    }
}

fn extract_opening_line(reply: &str) -> Option<String> {
    let mut in_fence = false;
    let mut candidate: Option<String> = None;
    let mut view_title: Option<String> = None;
    for raw in reply.lines() {
        let line = raw.trim();
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            if view_title.is_none() {
                if let Some(rest) = line.strip_prefix(":: title ") {
                    let t = rest.trim();
                    if !t.is_empty() {
                        view_title = Some(t.to_owned());
                    }
                } else if let Some(start) = line.find("<title>") {
                    let rest = &line[start + 7..];
                    if let Some(end) = rest.find("</title>") {
                        let t = rest[..end].trim();
                        if !t.is_empty() {
                            view_title = Some(t.to_owned());
                        }
                    }
                }
            }
            continue;
        }
        if !line.is_empty() {
            candidate = Some(line.to_owned());
            break;
        }
    }
    let line = candidate.or(view_title)?;
    let trimmed = line.trim_start_matches(['#', '>', '-', ' ']);
    let cleaned = strip_inline_markdown(trimmed);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Classify a job reply into its structured outcome and cleaned one-line verdict.
pub(crate) fn classify_reply(reply: &str) -> (VerdictKind, Option<String>) {
    let Some(cleaned) = extract_opening_line(reply) else {
        return (VerdictKind::Attention, None);
    };
    if strip_prefix_ci(&cleaned, LOOP_QUIET_MARKER).is_some()
        || strip_prefix_ci(&cleaned, "quiet:").is_some()
        || cleaned.eq_ignore_ascii_case("quiet")
    {
        return (VerdictKind::Quiet, None);
    }
    if let Some(rest) = strip_prefix_ci(&cleaned, "blocked")
        && (rest.starts_with(':') || rest.starts_with(" —") || rest.starts_with(" -"))
    {
        let body = trim_label_separator(rest);
        let text = if body.is_empty() { "Blocked" } else { body };
        return (VerdictKind::Blocked, Some(clip_verdict(text)));
    }
    if let Some(rest) = strip_prefix_ci(&cleaned, "all clear")
        && has_word_boundary(rest)
    {
        let body = trim_label_separator(rest);
        let text = if body.is_empty() { "All clear" } else { body };
        return (VerdictKind::Clear, Some(clip_verdict(text)));
    }
    if (strip_prefix_ci(&cleaned, "nothing urgent").is_some_and(has_word_boundary))
        || (strip_prefix_ci(&cleaned, "nothing is broken").is_some_and(has_word_boundary))
    {
        return (VerdictKind::Clear, Some(clip_verdict(&cleaned)));
    }
    if let Some(rest) = strip_prefix_ci(&cleaned, "needs attention:")
        .or_else(|| strip_prefix_ci(&cleaned, "attention:"))
    {
        let body = trim_label_separator(rest);
        let text = if body.is_empty() { &cleaned } else { body };
        return (VerdictKind::Attention, Some(clip_verdict(text)));
    }
    (VerdictKind::Attention, Some(clip_verdict(&cleaned)))
}

/// Whether a Loop run's answer says it found nothing for the user.
#[cfg(test)]
pub(crate) fn loop_reply_is_quiet(reply: &str) -> bool {
    classify_reply(reply).0 == VerdictKind::Quiet
}

/// The one-line verdict a non-quiet job run opens with, bounded for the Jobs
/// page and Review list (where [`loop_prompt`] asks for it).
#[cfg(test)]
pub(crate) fn loop_reply_verdict(reply: &str) -> Option<String> {
    classify_reply(reply).1
}

/// The first message of a job's thread: the user's prompt, with what a
/// scheduled run is for and, read-only, what it may not do.
pub(crate) fn loop_prompt(name: &str, prompt: &str, read_only: bool) -> String {
    let mut text = format!(
        "This is a scheduled run of the job “{name}”. Nobody is watching it run. \
         Start your first line in plain text (no Markdown) with one of: \
         “{LOOP_QUIET_MARKER}” when nothing needs the user (the thread is archived); \
         “All clear: <one-line summary>” when checks passed and nothing needs action; \
         “Blocked: <reason>” when a missing tool, MCP, path, or permission stopped the check; \
         or “Needs attention: <one-line verdict>” when something needs the user."
    );
    if read_only {
        text.push_str(
            " This run is read-only: do not edit files, commit or push; \
             anything that asks for approval is declined.",
        );
    }
    text.push_str("\n\n");
    text.push_str(prompt.trim());
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_inline_markdown_before_clipping() {
        assert_eq!(
            strip_inline_markdown(
                "Two things need you: **HEI's Meta ad account** failed on `prod.log` (see [run](https://example.com))."
            ),
            "Two things need you: HEI's Meta ad account failed on prod.log (see run)."
        );
        assert_eq!(
            strip_inline_markdown("2 * 3 = 6 and *fast*"),
            "2 * 3 = 6 and fast"
        );
        let long = format!("**{}**", "a".repeat(200));
        let (_, verdict) = classify_reply(&long);
        let verdict = verdict.unwrap();
        assert!(!verdict.contains('*'));
        assert!(verdict.ends_with('…'));
    }

    #[test]
    fn classifies_quiet_clear_blocked_and_attention() {
        assert_eq!(
            classify_reply("Quiet: no new failures."),
            (VerdictKind::Quiet, None)
        );
        assert_eq!(
            classify_reply("Nothing to report."),
            (VerdictKind::Quiet, None)
        );
        assert_eq!(
            classify_reply("All clear: all 28 sessions completed."),
            (
                VerdictKind::Clear,
                Some("all 28 sessions completed.".to_owned())
            )
        );
        assert_eq!(
            classify_reply("Nothing urgent. Two small items noted."),
            (
                VerdictKind::Clear,
                Some("Nothing urgent. Two small items noted.".to_owned())
            )
        );
        assert_eq!(
            classify_reply("Nothing is broken for customers."),
            (
                VerdictKind::Clear,
                Some("Nothing is broken for customers.".to_owned())
            )
        );
        assert_eq!(
            classify_reply("Blocked: the `daily-dev` MCP is not connected."),
            (
                VerdictKind::Blocked,
                Some("the daily-dev MCP is not connected.".to_owned())
            )
        );
        assert_eq!(
            classify_reply("Needs attention: **HEI Meta ads** refused updates."),
            (
                VerdictKind::Attention,
                Some("HEI Meta ads refused updates.".to_owned())
            )
        );
    }
}
