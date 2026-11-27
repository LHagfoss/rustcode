//! The `review_guide` text generation kind: a guided tour of a pull request.
//!
//! The desktop sends the pull request's title, description and its diff with
//! every line numbered; the generator answers with JSON stops that the
//! desktop checks against that diff before drawing them.

use std::time::Duration;

/// The kind name `_rustcode/textGeneration/generate` takes.
pub(crate) const KIND: &str = "review_guide";

/// A guide reads the whole change, so it may send more than a commit message does.
pub(crate) const INPUT_CHARS: usize = 120_000;

/// Room for a stop per concern across a large pull request.
pub(crate) const MAX_OUTPUT_TOKENS: u32 = 8192;

/// A large pull request takes a model a few minutes to read.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(240);

/// What the generator is asked for. The schema here is what
/// `apps/desktop/src/state/reviewGuide.ts` parses.
pub(crate) const INSTRUCTION: &str = r#"You are preparing a guided review of a pull request for its maintainer, who has not read it yet. Read the title, description and the numbered diff, then answer with only a JSON object, no Markdown fence and no other text:
{"stops":[{"title":"...","summary":"...","lookClosely":null,"excerpts":[{"path":"...","side":"new","start":1,"end":1}],"files":["..."],"questions":["..."]}]}
Rules:
- Order the stops as a reader should take them: the core change first, then what depends on it, then tests and docs. Group mechanical changes (renames, moved code, formatting, snapshots, lockfiles, generated files) into one stop at the end.
- Every changed file appears in at least one stop's "files".
- "title": a few words naming the concern, not a file name. "summary": two to four sentences of plain Markdown saying what changed and why it matters.
- "lookClosely": null, or one sentence naming a call only a maintainer can make (a trade-off, a contract or API change, a product decision, a risk, an accessibility concern). Use it sparingly.
- "excerpts": at most three per stop, each the few lines worth reading, at most 30. Use the numbers printed in the diff: "side":"new" for lines the change adds or keeps, "side":"old" for removed lines. Never cite a line the diff does not show.
- "questions": zero to two questions a reviewer might ask an agent about this stop.
- Write for the reader of the diff: no praise, no restating the title, no line-by-line narration."#;

/// `text` cut to `INPUT_CHARS` characters, marked when cut.
pub(crate) fn bounded(text: &str) -> String {
    let mut result: String = text.chars().take(INPUT_CHARS).collect();
    if result.len() < text.len() {
        result.push_str("\n[truncated]");
    }
    result
}

/// The time a generation of `kind` may take.
pub(crate) fn timeout_for(kind: &str) -> Duration {
    if kind == KIND {
        TIMEOUT
    } else {
        Duration::from_secs(45)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instruction_names_every_field_the_desktop_parses() {
        for field in [
            "\"stops\"",
            "\"title\"",
            "\"summary\"",
            "\"lookClosely\"",
            "\"excerpts\"",
            "\"path\"",
            "\"side\"",
            "\"start\"",
            "\"end\"",
            "\"files\"",
            "\"questions\"",
        ] {
            assert!(INSTRUCTION.contains(field), "{field}");
        }
        assert!(INSTRUCTION.contains("Every changed file"));
    }

    #[test]
    fn a_guide_keeps_more_input_and_time_than_other_kinds() {
        let long = "x".repeat(INPUT_CHARS + 10);
        assert_eq!(
            bounded(&long).chars().count(),
            INPUT_CHARS + "\n[truncated]".len()
        );
        assert_eq!(bounded("short"), "short");
        assert_eq!(timeout_for(KIND), TIMEOUT);
        assert_eq!(timeout_for("commit_message"), Duration::from_secs(45));
    }
}
