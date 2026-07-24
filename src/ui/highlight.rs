//! Rust syntax highlighting and diff-line rendering for the chat viewport.
//!
//! Pure span/line builders extracted from `ui/mod.rs`. Colour constants and
//! `get_themed_style` live in the parent module and are reached via `super::`.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{COLOR_BG, COLOR_ELEMENT, COLOR_TEXT, get_themed_style};

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

pub(super) fn highlight_diff_line<'a>(line: &str, width: usize, show_picker: bool) -> Line<'a> {
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
