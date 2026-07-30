//! Language-aware syntax highlighting and diff-line rendering for the chat viewport.
//!
//! Pure span/line builders extracted from `ui/mod.rs`. Colour constants and
//! `get_themed_style` live in the parent module and are reached via `super::`.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{COLOR_BG, COLOR_ELEMENT, COLOR_GREEN, COLOR_MUTED, COLOR_TEXT, get_themed_style};

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static SYNTAX_THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn syntax_theme() -> &'static Theme {
    SYNTAX_THEME.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults().themes;
        themes
            .remove("base16-ocean.dark")
            .or_else(|| themes.into_values().next())
            .expect("syntect ships at least one default theme")
    })
}

fn syntect_style(style: SyntectStyle, show_picker: bool) -> Style {
    let mut modifier = Modifier::empty();
    if style.font_style.contains(FontStyle::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    get_themed_style(
        Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
        COLOR_ELEMENT,
        modifier,
        show_picker,
    )
}

/// Highlight one code line using the fenced block's language identifier.
/// Unknown languages deliberately fall back to plain code styling instead of
/// guessing Rust, which was the source of misleading colors in other blocks.
pub(super) fn highlight_code_line<'a>(
    line: &str,
    language: &str,
    show_picker: bool,
) -> Vec<Span<'a>> {
    let syntax: &SyntaxReference = syntax_set()
        .find_syntax_by_token(language)
        .unwrap_or_else(|| syntax_set().find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntax_theme());
    match highlighter.highlight_line(line, syntax_set()) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style, show_picker)))
            .collect(),
        Err(_) => vec![Span::styled(
            line.to_string(),
            get_themed_style(COLOR_TEXT, COLOR_ELEMENT, Modifier::empty(), show_picker),
        )],
    }
}

pub(super) fn pad_to_width(s: &str, width: usize) -> String {
    let current = s.width();
    if current < width {
        format!("{}{}", s, " ".repeat(width - current))
    } else {
        s.to_string()
    }
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "fn" | "let"
            | "mut"
            | "pub"
            | "use"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "match"
            | "if"
            | "else"
            | "return"
            | "loop"
            | "for"
            | "in"
            | "while"
            | "async"
            | "await"
            | "mod"
            | "crate"
            | "self"
            | "Self"
            | "true"
            | "false"
            | "const"
            | "static"
            | "type"
            | "where"
            | "dyn"
            | "as"
            | "ref"
            | "move"
            | "unsafe"
    )
}

fn is_type(s: &str) -> bool {
    matches!(
        s,
        "Option"
            | "Result"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
            | "String"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "bool"
            | "Vec"
            | "Arc"
            | "Rc"
            | "Mutex"
            | "Box"
            | "Pin"
            | "Future"
            | "Instant"
            | "Duration"
    ) || (!s.is_empty() && s.chars().next().unwrap().is_uppercase())
}

fn highlight_rust_line_with_colors<'a>(
    line: &str,
    default_fg: Color,
    bg_color: Color,
    show_picker: bool,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    let color_keyword = Color::Rgb(198, 120, 221); // Purple
    let color_type = Color::Rgb(229, 192, 123); // Yellow
    let color_string = Color::Rgb(152, 195, 121); // Green
    let color_comment = Color::Rgb(92, 99, 112); // Gray (muted)
    let color_number = Color::Rgb(209, 154, 102); // Orange
    let color_macro = Color::Rgb(97, 175, 239); // Blue
    let color_fn = Color::Rgb(97, 175, 239); // Blue

    while i < chars.len() {
        // Comments
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            let comment_text: String = chars[i..].iter().collect();
            spans.push(Span::styled(
                comment_text,
                get_themed_style(color_comment, bg_color, Modifier::empty(), show_picker),
            ));
            break;
        }

        // Strings
        if chars[i] == '"' {
            let mut s = String::new();
            s.push('"');
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                let c = chars[i];
                s.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(
                s,
                get_themed_style(color_string, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Characters
        if chars[i] == '\'' {
            let mut s = String::new();
            s.push('\'');
            i += 1;
            let mut escaped = false;
            while i < chars.len() {
                let c = chars[i];
                s.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '\'' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(
                s,
                get_themed_style(color_string, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit() {
            let mut num = String::new();
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || chars[i] == '.'
                    || chars[i] == '_'
                    || chars[i].is_ascii_alphabetic())
            {
                num.push(chars[i]);
                i += 1;
            }
            spans.push(Span::styled(
                num,
                get_themed_style(color_number, bg_color, Modifier::empty(), show_picker),
            ));
            continue;
        }

        // Identifiers
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let mut ident = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                ident.push(chars[i]);
                i += 1;
            }

            let is_macro = i < chars.len() && chars[i] == '!';
            let is_fn = !is_macro
                && ((i < chars.len() && chars[i] == '(')
                    || (i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] == ':'));

            let style = if is_macro {
                ident.push('!');
                i += 1;
                get_themed_style(color_macro, bg_color, Modifier::BOLD, show_picker)
            } else if is_keyword(&ident) {
                get_themed_style(color_keyword, bg_color, Modifier::BOLD, show_picker)
            } else if is_type(&ident) {
                get_themed_style(color_type, bg_color, Modifier::empty(), show_picker)
            } else if is_fn {
                get_themed_style(color_fn, bg_color, Modifier::empty(), show_picker)
            } else {
                get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker)
            };

            spans.push(Span::styled(ident, style));
            continue;
        }

        // Symbols
        let mut symbol = String::new();
        symbol.push(chars[i]);
        i += 1;
        spans.push(Span::styled(
            symbol,
            get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker),
        ));
    }

    spans
}

pub(super) fn highlight_rust_line<'a>(line: &str, show_picker: bool) -> Vec<Span<'a>> {
    highlight_rust_line_with_colors(line, COLOR_TEXT, COLOR_ELEMENT, show_picker)
}

/// Flow a sequence of styled spans across as many rows as needed to fit
/// `width` display columns, padding every row (including the last) out to the
/// full width with `bg`. This is what makes a code panel's background fill the
/// entire box edge to edge instead of stopping behind the last glyph, and it
/// hard-wraps over-long lines so nothing overflows the panel.
pub(super) fn wrap_code_spans<'a>(
    spans: Vec<Span<'a>>,
    width: usize,
    bg: Color,
    show_picker: bool,
) -> Vec<Line<'a>> {
    let width = width.max(1);
    let pad_style = get_themed_style(COLOR_TEXT, bg, Modifier::empty(), show_picker);
    let mut out: Vec<Line> = Vec::new();
    let mut row: Vec<Span> = Vec::new();
    let mut row_w = 0usize;

    for span in spans {
        let style = span.style;
        let mut seg = String::new();
        let mut seg_w = 0usize;
        for ch in span.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if row_w + seg_w + cw > width {
                if !seg.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut seg), style));
                    row_w += seg_w;
                    seg_w = 0;
                }
                if row_w < width {
                    row.push(Span::styled(" ".repeat(width - row_w), pad_style));
                }
                out.push(Line::from(std::mem::take(&mut row)));
                row_w = 0;
            }
            seg.push(ch);
            seg_w += cw;
        }
        if !seg.is_empty() {
            row.push(Span::styled(seg, style));
            row_w += seg_w;
        }
    }

    if row_w < width {
        row.push(Span::styled(" ".repeat(width - row_w), pad_style));
    }
    out.push(Line::from(row));
    out
}

/// Colours for a diff cell keyed on its leading sign char.
/// Returns `(bg, fg, is_empty)`. `~` marks a column with no line on that side.
fn diff_cell_colors(prefix: char) -> (Color, Color, bool) {
    match prefix {
        '+' => (Color::Rgb(24, 40, 24), Color::Rgb(160, 240, 160), false), // add
        '-' => (Color::Rgb(48, 20, 20), Color::Rgb(240, 150, 150), false), // delete
        '~' => (Color::Rgb(22, 22, 26), Color::Rgb(22, 22, 26), true),     // absent
        _ => (COLOR_BG, COLOR_TEXT, false),                                // context
    }
}

/// Truncate `spans` to exactly `width` display columns, then pad the remainder
/// with `bg` so the cell background fills edge to edge. Single row, no wrap.
fn fit_line_spans<'a>(
    spans: Vec<Span<'a>>,
    width: usize,
    bg: Color,
    show_picker: bool,
) -> Vec<Span<'a>> {
    let mut out: Vec<Span> = Vec::new();
    let mut w = 0usize;
    for span in spans {
        if w >= width {
            break;
        }
        let mut seg = String::new();
        let mut seg_w = 0usize;
        for ch in span.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if w + seg_w + cw > width {
                break;
            }
            seg.push(ch);
            seg_w += cw;
        }
        if !seg.is_empty() {
            out.push(Span::styled(seg, span.style));
            w += seg_w;
        }
    }
    if w < width {
        out.push(Span::styled(
            " ".repeat(width - w),
            get_themed_style(COLOR_TEXT, bg, Modifier::empty(), show_picker),
        ));
    }
    out
}

/// Render one side (old or new) of a side-by-side diff row into `col_width`
/// columns: a sign gutter, syntax-highlighted code, then background fill.
fn diff_cell_spans<'a>(cell: &str, col_width: usize, show_picker: bool) -> Vec<Span<'a>> {
    let (prefix, code) = {
        let mut chars = cell.chars();
        match chars.next() {
            Some(c @ ('+' | '-' | ' ' | '~')) => (c, chars.as_str()),
            _ => (' ', cell),
        }
    };
    let (bg, fg, is_empty) = diff_cell_colors(prefix);

    let sign = match prefix {
        '+' => '+',
        '-' => '-',
        _ => ' ',
    };
    let mut spans = vec![Span::styled(
        format!("{sign} "),
        get_themed_style(fg, bg, Modifier::BOLD, show_picker),
    )];
    if !is_empty {
        spans.extend(highlight_rust_line_with_colors(code, fg, bg, show_picker));
    }
    fit_line_spans(spans, col_width, bg, show_picker)
}

/// Render a diff row. Rows produced by `get_diff_preview` carry a `\0` that
/// splits the old (left) and new (right) columns — those render side by side,
/// each filling half the width with its own sign gutter and syntax highlight.
/// Rows without a `\0` (fenced ```diff blocks) fall back to a single column.
pub(super) fn highlight_diff_line<'a>(line: &str, width: usize, show_picker: bool) -> Line<'a> {
    if let Some((left, right)) = line.split_once('\0') {
        let sep_w = 3usize;
        let avail = width.saturating_sub(sep_w);
        let lw = avail / 2;
        let rw = avail - lw;
        let mut spans = diff_cell_spans(left, lw, show_picker);
        spans.push(Span::styled(
            " │ ".to_string(),
            get_themed_style(Color::Rgb(90, 90, 90), COLOR_BG, Modifier::empty(), show_picker),
        ));
        spans.extend(diff_cell_spans(right, rw, show_picker));
        return Line::from(spans);
    }

    // Single-column fallback for unified ```diff fenced blocks.
    let (prefix, code) = if line.is_empty() {
        (' ', "")
    } else {
        let mut chars = line.chars();
        let first = chars.next().unwrap();
        if first == '+' || first == '-' || first == ' ' || first == '~' {
            (first, chars.as_str())
        } else {
            (' ', line)
        }
    };

    let bg_color = match prefix {
        '+' => Color::Rgb(24, 40, 24), // Dark Green
        '-' => Color::Rgb(48, 20, 20), // Dark Red
        '~' => COLOR_BG,               // Matches bg color
        _ => COLOR_ELEMENT,            // Dark Gray
    };

    let default_fg = match prefix {
        '+' => Color::Rgb(160, 240, 160), // Light Green
        '-' => Color::Rgb(240, 150, 150), // Light Red
        '~' => COLOR_BG,                  // Invisible prefix
        _ => COLOR_TEXT,                  // Default text color
    };

    let spans = if prefix == '~' {
        Vec::new()
    } else {
        highlight_rust_line_with_colors(code, default_fg, bg_color, show_picker)
    };

    let prefix_str = if prefix == '~' {
        "  ".to_string()
    } else {
        format!("{} ", prefix)
    };
    let mut final_spans = vec![Span::styled(
        prefix_str,
        get_themed_style(default_fg, bg_color, Modifier::BOLD, show_picker),
    )];
    final_spans.extend(spans);

    let current_width: usize = final_spans.iter().map(|s| s.content.width()).sum();
    if current_width < width {
        let pad_width = width - current_width;
        final_spans.push(Span::styled(
            " ".repeat(pad_width),
            Style::default().bg(bg_color),
        ));
    }

    Line::from(final_spans)
}

/// Render a unified diff with a compact line-number gutter. Hunk headers reset
/// the old/new counters; ordinary rows then receive the appropriate source
/// line number while retaining the existing syntax-aware diff styling.
pub(super) fn render_unified_diff<'a>(diff: &str, width: usize, show_picker: bool) -> Vec<Line<'a>> {
    let gutter = 12usize;
    let body_width = width.saturating_sub(gutter).max(1);
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut rendered = Vec::new();

    for raw in diff.lines() {
        if let Some((old, new)) = parse_hunk_header(raw) {
            old_line = old;
            new_line = new;
            rendered.push(Line::from(vec![
                Span::styled(format!("{:>5} {:>5} ", "", ""), get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::BOLD, show_picker)),
                Span::styled(raw.to_string(), get_themed_style(Color::Rgb(100, 175, 235), COLOR_BG, Modifier::BOLD, show_picker)),
            ]));
            continue;
        }

        let (old_number, new_number) = match raw.chars().next() {
            Some('+') => { let n = new_line; new_line += 1; (String::new(), n.to_string()) }
            Some('-') => { let n = old_line; old_line += 1; (n.to_string(), String::new()) }
            Some(' ') => { let o = old_line; let n = new_line; old_line += 1; new_line += 1; (o.to_string(), n.to_string()) }
            _ => (String::new(), String::new()),
        };
        let prefix = format!("{:>5} {:>5} ", old_number, new_number);
        let mut line = vec![Span::styled(prefix, get_themed_style(COLOR_MUTED, COLOR_BG, Modifier::empty(), show_picker))];
        let body = highlight_diff_line(raw, body_width, show_picker);
        line.extend(body.spans);
        rendered.push(Line::from(line));
    }
    rendered
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "@@" { return None; }
    let old = parts.next()?.strip_prefix('-')?.split(',').next()?.parse().ok()?;
    let new = parts.next()?.strip_prefix('+')?.split(',').next()?.parse().ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_width(line: &Line) -> usize {
        line.spans.iter().map(|s| s.content.width()).sum()
    }

    #[test]
    fn wraps_and_pads_every_row_to_width() {
        // A single long span must split across rows, each padded to full width.
        let spans = vec![Span::raw("abcdefghijklmnop")];
        let rows = wrap_code_spans(spans, 6, COLOR_ELEMENT, false);
        assert_eq!(rows.len(), 3); // 16 chars / 6 = 3 rows
        for row in &rows {
            assert_eq!(row_width(&row), 6);
        }
    }

    #[test]
    fn pads_empty_input_to_one_full_row() {
        let rows = wrap_code_spans(Vec::new(), 8, COLOR_ELEMENT, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(row_width(&rows[0]), 8);
    }

    #[test]
    fn side_by_side_diff_fills_full_width() {
        // A `\0`-separated row renders old|new columns spanning the whole width.
        let line = highlight_diff_line("-let x = 1;\0+let x = 2;", 40, false);
        assert_eq!(row_width(&line), 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains('│'), "expected a column separator");
    }

    #[test]
    fn unified_diff_row_without_nul_still_single_column() {
        let line = highlight_diff_line("+added line", 40, false);
        assert_eq!(row_width(&line), 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains('│'), "unified fallback must not add a separator");
    }

    #[test]
    fn highlights_non_rust_fenced_languages() {
        let spans = highlight_code_line("def greet(name):", "python", false);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "def greet(name):");
        assert!(spans.len() > 1, "Python should receive token-level styling");
    }

    #[test]
    fn unknown_language_falls_back_without_guessing_rust() {
        let spans = highlight_code_line("for value in words", "made-up-language", false);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "for value in words");
    }

    #[test]
    fn unified_diff_adds_hunk_aware_line_numbers() {
        let lines = render_unified_diff("@@ -4,2 +7,2 @@\n-old\n+new\n context", 60, false);
        assert_eq!(lines.len(), 4);
        let text: String = lines[1].spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.starts_with("    4     "));
        let text: String = lines[2].spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.starts_with("          7 "));
    }
}

pub(super) fn format_markdown_spans<'a>(line_str: &str, show_picker: bool) -> Vec<Span<'a>> {
    let trimmed = line_str.trim();

    if trimmed.starts_with("# ") {
        let text = trimmed.trim_start_matches("# ").trim();
        let mut spans = vec![Span::styled(
            "# ".to_string(),
            get_themed_style(Color::Rgb(100, 175, 235), COLOR_BG, Modifier::BOLD, show_picker),
        )];
        parse_inline_markdown(text, show_picker, &mut spans);
        return spans;
    } else if trimmed.starts_with("## ") {
        let text = trimmed.trim_start_matches("## ").trim();
        let mut spans = vec![Span::styled(
            "## ".to_string(),
            get_themed_style(Color::Rgb(229, 192, 123), COLOR_BG, Modifier::BOLD, show_picker),
        )];
        parse_inline_markdown(text, show_picker, &mut spans);
        return spans;
    } else if trimmed.starts_with("### ") {
        let text = trimmed.trim_start_matches("### ").trim();
        let mut spans = vec![Span::styled(
            "### ".to_string(),
            get_themed_style(Color::Rgb(224, 169, 109), COLOR_BG, Modifier::BOLD, show_picker),
        )];
        parse_inline_markdown(text, show_picker, &mut spans);
        return spans;
    } else if trimmed.starts_with("#### ") {
        let text = trimmed.trim_start_matches("#### ").trim();
        let mut spans = Vec::new();
        parse_inline_markdown(text, show_picker, &mut spans);
        return spans;
    }

    let (prefix, text_content) = if trimmed.starts_with("- ") {
        ("• ", &trimmed[2..])
    } else if trimmed.starts_with("* ") {
        ("• ", &trimmed[2..])
    } else {
        ("", line_str)
    };

    let mut spans = Vec::new();
    if !prefix.is_empty() {
        let indent = line_str.len() - line_str.trim_start().len();
        if indent > 0 {
            spans.push(Span::styled(
                " ".repeat(indent),
                get_themed_style(COLOR_TEXT, COLOR_BG, Modifier::empty(), show_picker),
            ));
        }
        spans.push(Span::styled(
            prefix.to_string(),
            get_themed_style(Color::Rgb(100, 175, 235), COLOR_BG, Modifier::BOLD, show_picker),
        ));
    }

    parse_inline_markdown(text_content, show_picker, &mut spans);
    spans
}

pub(super) fn parse_inline_markdown<'a>(text: &str, show_picker: bool, spans: &mut Vec<Span<'a>>) {
    let mut chars = text.chars().peekable();
    let mut current = String::new();
    let mut in_inline_code = false;
    let mut in_bold = false;

    while let Some(c) = chars.next() {
        if c == '`' {
            if !current.is_empty() {
                let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                let style = if in_inline_code {
                    get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                } else {
                    get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                };
                spans.push(Span::styled(current.clone(), style));
                current.clear();
            }
            in_inline_code = !in_inline_code;
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !current.is_empty() {
                let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                let style = if in_inline_code {
                    get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                } else {
                    get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                };
                spans.push(Span::styled(current.clone(), style));
                current.clear();
            }
            in_bold = !in_bold;
        } else if c == '[' {
            let mut label = String::new();
            let mut found_closing_bracket = false;
            let mut sub_chars = chars.clone();
            while let Some(sc) = sub_chars.next() {
                if sc == ']' {
                    found_closing_bracket = true;
                    break;
                }
                label.push(sc);
            }

            if found_closing_bracket && sub_chars.peek() == Some(&'(') {
                sub_chars.next(); // consume '('
                let mut url = String::new();
                let mut found_closing_paren = false;
                while let Some(uc) = sub_chars.next() {
                    if uc == ')' {
                        found_closing_paren = true;
                        break;
                    }
                    url.push(uc);
                }

                if found_closing_paren {
                    if !current.is_empty() {
                        let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
                        let style = if in_inline_code {
                            get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
                        } else {
                            get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
                        };
                        spans.push(Span::styled(current.clone(), style));
                        current.clear();
                    }

                    chars = sub_chars;
                    spans.push(Span::styled(
                        label,
                        get_themed_style(Color::Rgb(100, 175, 235), COLOR_BG, Modifier::UNDERLINED | Modifier::BOLD, show_picker),
                    ));
                    continue;
                }
            }
            current.push(c);
        } else {
            current.push(c);
        }
    }

    if !current.is_empty() {
        let modifier = if in_bold { Modifier::BOLD } else { Modifier::empty() };
        let style = if in_inline_code {
            get_themed_style(COLOR_GREEN, COLOR_ELEMENT, modifier, show_picker)
        } else {
            get_themed_style(COLOR_TEXT, COLOR_BG, modifier, show_picker)
        };
        spans.push(Span::styled(current, style));
    }
}
