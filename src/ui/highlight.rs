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

use super::{
    COLOR_BG, COLOR_DIFF_ADD_FG, COLOR_DIFF_REMOVE_FG, COLOR_ELEMENT, COLOR_GREEN, COLOR_MUTED,
    COLOR_PRIMARY, COLOR_SECONDARY, COLOR_TEXT, COLOR_TIP, get_themed_style,
};

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static CURRENT_SYNTAX_THEME: std::sync::RwLock<Option<(String, Theme)>> =
    std::sync::RwLock::new(None);

fn color_to_hex(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{:02X}{:02X}{:02X}", r, g, b),
        _ => fallback.to_string(),
    }
}

pub fn create_syntect_theme(palette: &super::theme::ThemePalette) -> Theme {
    let bg_hex = color_to_hex(palette.bg, "#15171A");
    let fg_hex = color_to_hex(palette.text, "#F0E5DE");
    let primary_hex = color_to_hex(palette.primary, "#EC6E5D");
    let secondary_hex = color_to_hex(palette.secondary, "#3C5865");
    let green_hex = color_to_hex(palette.green, "#A6E3A1");
    let tip_hex = color_to_hex(palette.tip, "#E0A96D");
    let muted_hex = color_to_hex(palette.muted, "#88929A");

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>name</key>
	<string>{name}</string>
	<key>settings</key>
	<array>
		<dict>
			<key>settings</key>
			<dict>
				<key>background</key>
				<string>{bg_hex}</string>
				<key>foreground</key>
				<string>{fg_hex}</string>
				<key>caret</key>
				<string>{primary_hex}</string>
				<key>selection</key>
				<string>{primary_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Comments</string>
			<key>scope</key>
			<string>comment, punctuation.definition.comment</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{muted_hex}</string>
				<key>fontStyle</key>
				<string>italic</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Keywords</string>
			<key>scope</key>
			<string>keyword, keyword.control, storage, storage.type, storage.modifier, keyword.operator.logical, keyword.operator.pipe</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{primary_hex}</string>
				<key>fontStyle</key>
				<string>bold</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Functions and Commands</string>
			<key>scope</key>
			<string>entity.name.function, support.function, variable.function</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{fg_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Types and Classes</string>
			<key>scope</key>
			<string>entity.name.type, entity.name.class, entity.other.inherited-class, support.type, support.class</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{secondary_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Strings</string>
			<key>scope</key>
			<string>string, string.quoted, string.quoted.single, string.quoted.double</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{green_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Numbers and Constants</string>
			<key>scope</key>
			<string>constant.numeric, constant.language, constant.character, constant.other</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{primary_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Flags and Parameters</string>
			<key>scope</key>
			<string>variable.parameter, variable.parameter.option, punctuation.definition.parameter</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{tip_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Variables</string>
			<key>scope</key>
			<string>variable, variable.other, punctuation.definition.variable</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{fg_hex}</string>
			</dict>
		</dict>
		<dict>
			<key>name</key>
			<string>Operators and Punctuation</string>
			<key>scope</key>
			<string>keyword.operator, punctuation.separator, punctuation.terminator, punctuation.definition.tag</string>
			<key>settings</key>
			<dict>
				<key>foreground</key>
				<string>{secondary_hex}</string>
			</dict>
		</dict>
	</array>
</dict>
</plist>"#,
        name = palette.name,
        bg_hex = bg_hex,
        fg_hex = fg_hex,
        primary_hex = primary_hex,
        secondary_hex = secondary_hex,
        green_hex = green_hex,
        tip_hex = tip_hex,
        muted_hex = muted_hex,
    );

    let mut cursor = std::io::Cursor::new(xml.as_bytes());
    ThemeSet::load_from_reader(&mut cursor).unwrap_or_else(|_| {
        let mut themes = ThemeSet::load_defaults().themes;
        themes
            .remove("base16-eighties.dark")
            .or_else(|| themes.into_values().next())
            .expect("syntect ships at least one default theme")
    })
}

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn syntax_theme() -> Theme {
    let active_name = super::theme::active_palette().name;
    if let Ok(guard) = CURRENT_SYNTAX_THEME.read() {
        if let Some((cached_name, theme)) = guard.as_ref() {
            if cached_name == &active_name {
                return theme.clone();
            }
        }
    }

    let palette = super::theme::active_palette();
    let theme = create_syntect_theme(&palette);
    if let Ok(mut guard) = CURRENT_SYNTAX_THEME.write() {
        *guard = Some((palette.name, theme.clone()));
    }
    theme
}

fn syntect_style(style: SyntectStyle, show_picker: bool) -> Style {
    syntect_style_with_bg(style, COLOR_BG(), show_picker)
}

fn syntect_style_with_bg(style: SyntectStyle, background: Color, show_picker: bool) -> Style {
    let mut modifier = Modifier::empty();
    if style.font_style.contains(FontStyle::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    get_themed_style(
        Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
        background,
        modifier,
        show_picker,
    )
}

/// Highlight a shell command with Bash grammar while preserving its exact text.
/// Callers choose the surface background so transcript and modal cells remain opaque.
pub(super) fn highlight_shell_command(
    command: &str,
    background: Color,
    show_picker: bool,
) -> Vec<Line<'static>> {
    const MAX_COMMAND_BYTES: usize = 512 * 1024;
    const MAX_COMMAND_LINES: usize = 10_000;
    const MAX_COMMAND_LINE_BYTES: usize = 4 * 1024;

    let plain = || {
        let mut lines = command
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_owned(),
                    get_themed_style(COLOR_TEXT(), background, Modifier::empty(), show_picker),
                ))
            })
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                String::new(),
                get_themed_style(COLOR_TEXT(), background, Modifier::empty(), show_picker),
            )));
        }
        lines
    };

    if command.len() > MAX_COMMAND_BYTES
        || command.lines().count() > MAX_COMMAND_LINES
        || command
            .lines()
            .any(|line| line.len() > MAX_COMMAND_LINE_BYTES)
    {
        return plain();
    }

    let Some(syntax) = syntax_set().find_syntax_by_token("bash") else {
        return plain();
    };
    let theme = syntax_theme();
    let mut highlighter = HighlightLines::new(syntax, &theme);
    let mut lines = Vec::new();
    for line in command.lines() {
        let Ok(ranges) = highlighter.highlight_line(line, syntax_set()) else {
            return plain();
        };
        lines.push(Line::from(
            ranges
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(
                        text.to_owned(),
                        syntect_style_with_bg(style, background, show_picker),
                    )
                })
                .collect::<Vec<_>>(),
        ));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            String::new(),
            get_themed_style(COLOR_TEXT(), background, Modifier::empty(), show_picker),
        )));
    }
    lines
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
    let theme = syntax_theme();
    let mut highlighter = HighlightLines::new(syntax, &theme);
    match highlighter.highlight_line(line, syntax_set()) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style, show_picker)))
            .collect(),
        Err(_) => vec![Span::styled(
            line.to_string(),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
        )],
    }
}

/// Highlight a complete fenced block with one parser state, preserving
/// multiline comments and strings across line boundaries.
pub(super) fn highlight_code_block(
    code: &str,
    language: &str,
    show_picker: bool,
) -> Vec<Vec<Span<'static>>> {
    let syntax: &SyntaxReference = syntax_set()
        .find_syntax_by_token(language)
        .unwrap_or_else(|| syntax_set().find_syntax_plain_text());
    let theme = syntax_theme();
    let mut highlighter = HighlightLines::new(syntax, &theme);
    code.lines()
        .map(
            |line| match highlighter.highlight_line(line, syntax_set()) {
                Ok(ranges) => ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), syntect_style(style, show_picker))
                    })
                    .collect(),
                Err(_) => vec![Span::styled(
                    line.to_string(),
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                )],
            },
        )
        .collect()
}

#[allow(dead_code)]
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

    let color_keyword = COLOR_PRIMARY();
    let color_type = COLOR_SECONDARY();
    let color_string = COLOR_GREEN();
    let color_comment = COLOR_MUTED();
    let color_number = COLOR_TIP();
    let color_macro = COLOR_PRIMARY();
    let color_fn = COLOR_TEXT();

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
    let pad_style = get_themed_style(COLOR_TEXT(), bg, Modifier::empty(), show_picker);
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
        '+' => (COLOR_BG(), COLOR_DIFF_ADD_FG(), false),
        '-' => (COLOR_BG(), COLOR_DIFF_REMOVE_FG(), false),
        '~' => (COLOR_BG(), COLOR_MUTED(), true),
        _ => (COLOR_BG(), COLOR_TEXT(), false),
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
            get_themed_style(COLOR_TEXT(), bg, Modifier::empty(), show_picker),
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
        if prefix == '+' || prefix == '-' {
            spans.push(Span::styled(
                code.to_string(),
                get_themed_style(fg, bg, Modifier::empty(), show_picker),
            ));
        } else {
            spans.extend(highlight_rust_line_with_colors(code, fg, bg, show_picker));
        }
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
            get_themed_style(
                Color::Rgb(90, 90, 90),
                COLOR_BG(),
                Modifier::empty(),
                show_picker,
            ),
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

    let bg_color = COLOR_BG();

    let default_fg = match prefix {
        '+' => COLOR_DIFF_ADD_FG(),
        '-' => COLOR_DIFF_REMOVE_FG(),
        '~' => COLOR_MUTED(),
        _ => COLOR_TEXT(),
    };

    let spans = if prefix == '~' {
        Vec::new()
    } else if prefix == '+' || prefix == '-' {
        vec![Span::styled(
            code.to_string(),
            get_themed_style(default_fg, bg_color, Modifier::empty(), show_picker),
        )]
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
pub(super) fn render_unified_diff<'a>(
    diff: &str,
    width: usize,
    show_picker: bool,
) -> Vec<Line<'a>> {
    let gutter = 6usize;
    let body_width = width.saturating_sub(gutter).max(1);
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut rendered = Vec::new();

    for raw in diff.lines() {
        if let Some((old, new)) = parse_hunk_header(raw) {
            old_line = old;
            new_line = new;
            rendered.push(Line::from(vec![
                Span::styled(
                    "      ",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
                Span::styled(
                    raw.to_string(),
                    get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ),
            ]));
            continue;
        }

        let line_number = match raw.chars().next() {
            Some('+') => {
                let n = new_line;
                new_line += 1;
                n
            }
            Some('-') => {
                let n = old_line;
                old_line += 1;
                n
            }
            Some(' ') => {
                let n = new_line;
                old_line += 1;
                new_line += 1;
                n
            }
            _ => 0,
        };
        let prefix = if line_number == 0 {
            "      ".to_string()
        } else {
            format!("{line_number:>5} ")
        };
        let gutter_bg = COLOR_BG();
        let mut line = vec![Span::styled(
            prefix,
            get_themed_style(COLOR_MUTED(), gutter_bg, Modifier::empty(), show_picker),
        )];
        let body = highlight_diff_line(raw, body_width, show_picker);
        line.extend(body.spans);
        rendered.push(Line::from(line));
    }
    rendered
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let old_part = parts.next()?;
    let new_part = parts.next()?;
    let old = old_part
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = new_part
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_width(line: &Line) -> usize {
        line.spans.iter().map(|s| s.content.width()).sum()
    }

    #[test]
    fn shell_highlighting_preserves_text_and_applies_token_styles() {
        let cmd = "cd /Users/lagos/code/lcli && git status --short --branch; echo \"===\"; wc -l src/commands/ls.rs src/main.rs src/cli.rs; echo \"===\"; cargo check 2>&1 | head -40";
        let test_lines = highlight_shell_command(cmd, COLOR_BG(), false);
        let rendered = test_lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rendered, cmd);

        let spans: Vec<&Span> = test_lines.iter().flat_map(|l| &l.spans).collect();
        assert!(spans.iter().all(|s| s.style.bg == Some(COLOR_BG())));

        // Bold orange keywords/operators like &&, ;, |
        assert!(spans.iter().any(|s| {
            s.content.as_ref() == "&&"
                && s.style.fg == Some(Color::Rgb(236, 110, 93))
                && s.style.add_modifier.contains(Modifier::BOLD)
        }));

        // Green strings like "==="
        assert!(spans.iter().any(|s| {
            s.content.as_ref() == "===" && s.style.fg == Some(Color::Rgb(166, 227, 161))
        }));

        // Amber flags like short, branch, l, 40
        assert!(spans.iter().any(|s| {
            s.content.as_ref() == "short" && s.style.fg == Some(Color::Rgb(224, 169, 109))
        }));

        // Clean white/cream command names
        assert!(spans.iter().any(|s| {
            s.content.as_ref() == "git" && s.style.fg == Some(Color::Rgb(240, 229, 222))
        }));
    }

    #[test]
    fn shell_highlighting_keeps_multiline_commands_and_bounds_long_lines() {
        let multiline = "printf '%s\\n' one\nprintf '%s\\n' two";
        let lines = highlight_shell_command(multiline, COLOR_BG(), false);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].to_string(), "printf '%s\\n' one");
        assert_eq!(lines[1].to_string(), "printf '%s\\n' two");

        let long = "x".repeat(4 * 1024 + 1);
        let fallback = highlight_shell_command(&long, COLOR_BG(), false);
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0].to_string(), long);
        assert_eq!(fallback[0].spans.len(), 1);
    }

    #[test]
    fn wraps_and_pads_every_row_to_width() {
        // A single long span must split across rows, each padded to full width.
        let spans = vec![Span::raw("abcdefghijklmnop")];
        let rows = wrap_code_spans(spans, 6, COLOR_ELEMENT(), false);
        assert_eq!(rows.len(), 3); // 16 chars / 6 = 3 rows
        for row in &rows {
            assert_eq!(row_width(row), 6);
        }
    }

    #[test]
    fn pads_empty_input_to_one_full_row() {
        let rows = wrap_code_spans(Vec::new(), 8, COLOR_ELEMENT(), false);
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
        assert!(
            !text.contains('│'),
            "unified fallback must not add a separator"
        );
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
        let text: String = lines[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.starts_with("    4 - "));
        assert!(lines[1].spans[0].style.bg.is_some());
        assert_eq!(lines[1].spans[1].style.fg, Some(COLOR_DIFF_REMOVE_FG()));

        let text: String = lines[2]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.starts_with("    7 + "));
        assert!(lines[2].spans[0].style.bg.is_some());
        assert_eq!(lines[2].spans[1].style.fg, Some(COLOR_DIFF_ADD_FG()));

        assert!(lines[3].spans[0].style.bg.is_some());
    }
}
