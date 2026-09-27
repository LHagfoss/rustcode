//! Fenced-code highlighting without a second tree-sitter runtime.
use std::{ops::Range, sync::OnceLock};

use gpui_kit::{HighlightStyle, rgb};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

// Highlighting runs during layout. Large blocks remain readable plain text.
const MAX_CODE_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 4 * 1024;
const MAX_LINES: usize = 1_000;
static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

pub fn install(cx: &mut gpui_kit::App) {
    gpui_kit::base::TextViewDefaults::global(cx)
        .with_code_block_highlighter(|block| {
            let language = block.lang();
            highlight(language.as_deref(), &block.code())
        })
        .install(cx);
}

fn highlight(language: Option<&str>, code: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let Some(language) = language else {
        return Vec::new();
    };
    if code.len() > MAX_CODE_BYTES
        || code.lines().take(MAX_LINES + 1).count() > MAX_LINES
        || code.lines().any(|line| line.len() > MAX_LINE_BYTES)
    {
        return Vec::new();
    }
    let language = language.split_whitespace().next().unwrap_or_default();
    let language = match language {
        "rs" => "rust",
        "sh" | "shell" | "console" => "bash",
        "py" => "python",
        "js" => "javascript",
        "yml" => "yaml",
        other => other,
    };
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let Some(syntax) = syntaxes.find_syntax_by_token(language) else {
        return Vec::new();
    };
    let theme = THEME.get_or_init(|| {
        ThemeSet::load_defaults()
            .themes
            .remove("base16-ocean.dark")
            .expect("Syntect bundles the base16 ocean dark theme")
    });
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut highlights = Vec::new();
    let mut offset = 0;
    for line in LinesWithEndings::from(code) {
        let Ok(spans) = highlighter.highlight_line(line, syntaxes) else {
            return Vec::new();
        };
        for (style, text) in spans {
            let end = offset + text.len();
            if offset != end {
                let color = style.foreground;
                highlights.push((
                    offset..end,
                    HighlightStyle {
                        color: Some(
                            rgb((u32::from(color.r) << 16)
                                | (u32::from(color.g) << 8)
                                | u32::from(color.b))
                            .into(),
                        ),
                        font_weight: style
                            .font_style
                            .contains(FontStyle::BOLD)
                            .then_some(gpui_kit::FontWeight::BOLD),
                        font_style: style
                            .font_style
                            .contains(FontStyle::ITALIC)
                            .then_some(gpui_kit::FontStyle::Italic),
                        ..Default::default()
                    },
                ));
            }
            offset = end;
        }
    }
    highlights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_fences_have_distinct_syntax_colors() {
        let code = "fn main() { let answer = 42; println!(\"hello\"); }\n";
        let spans = highlight(Some("rust"), code);
        assert!(!spans.is_empty());
        let first_color = spans[0].1.color;
        assert!(spans.iter().any(|(_, style)| style.color != first_color));
        assert_eq!(spans.first().unwrap().0.start, 0);
        assert_eq!(spans.last().unwrap().0.end, code.len());
    }

    #[test]
    fn multiline_unicode_ranges_are_contiguous_byte_offsets() {
        let code = "// blåbær 🦀\nfn main() {\n    println!(\"こんにちは\");\n}\n";
        let spans = highlight(Some("rs"), code);
        let mut cursor = 0;
        for (range, _) in &spans {
            assert_eq!(range.start, cursor);
            assert!(code.is_char_boundary(range.start));
            assert!(code.is_char_boundary(range.end));
            cursor = range.end;
        }
        assert_eq!(cursor, code.len());
    }

    #[test]
    fn unknown_unlabelled_and_oversized_blocks_stay_plain() {
        assert!(highlight(Some("unknown-rustcode-language"), "fn main() {}").is_empty());
        assert!(highlight(None, "fn main() {}").is_empty());
        assert!(highlight(Some("rust"), &"x".repeat(MAX_CODE_BYTES + 1)).is_empty());
        assert!(highlight(Some("rust"), &"x".repeat(MAX_LINE_BYTES + 1)).is_empty());
        assert!(highlight(Some("rust"), &"x\n".repeat(MAX_LINES + 1)).is_empty());
    }
}
