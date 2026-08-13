//! Structured Markdown rendering for assistant messages.
//!
//! Markdown is parsed as a document rather than interpreted one physical line
//! at a time. This keeps nested emphasis, links, lists, blockquotes, and
//! escaped text from leaking their syntax into the chat viewport.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use unicode_width::UnicodeWidthStr;

use super::lru::LruCache;
use super::{
    COLOR_BG, COLOR_GREEN, COLOR_MUTED, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_TEXT, COLOR_TIP,
    get_themed_style,
};

/// Maximum number of rendered documents kept in [`RENDER_CACHE`].
const RENDER_CACHE_CAP: usize = 256;

type CacheKey = (u64, usize, bool, u64);

/// Bounded render cache with least-recently-used eviction.
type MarkdownCache = LruCache<CacheKey, Vec<Line<'static>>>;

static RENDER_CACHE: OnceLock<Mutex<MarkdownCache>> = OnceLock::new();

fn render_cache() -> &'static Mutex<MarkdownCache> {
    RENDER_CACHE.get_or_init(|| Mutex::new(MarkdownCache::new(RENDER_CACHE_CAP)))
}

fn cache_key(content: &str, width: usize, show_picker: bool) -> CacheKey {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    let mut theme_hasher = std::collections::hash_map::DefaultHasher::new();
    super::theme::active_palette().name.hash(&mut theme_hasher);
    (hasher.finish(), width, show_picker, theme_hasher.finish())
}

#[derive(Clone, Copy, Debug, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

impl InlineStyle {
    fn modifier(self) -> Modifier {
        let mut modifier = Modifier::empty();
        if self.bold {
            modifier |= Modifier::BOLD;
        }
        if self.italic {
            modifier |= Modifier::ITALIC;
        }
        if self.strike {
            modifier |= Modifier::CROSSED_OUT;
        }
        modifier
    }
}

fn text_style(style: InlineStyle, show_picker: bool) -> ratatui::style::Style {
    let fg = if style.code {
        COLOR_GREEN()
    } else if style.link {
        COLOR_PRIMARY()
    } else {
        COLOR_TEXT()
    };
    let bg = COLOR_BG();
    get_themed_style(fg, bg, style.modifier(), show_picker)
}

fn heading_style(level: HeadingLevel, show_picker: bool) -> ratatui::style::Style {
    let fg = match level {
        HeadingLevel::H1 => COLOR_PRIMARY(),
        HeadingLevel::H2 => COLOR_SECONDARY(),
        HeadingLevel::H3 => COLOR_TIP(),
        _ => COLOR_TEXT(),
    };
    get_themed_style(fg, COLOR_BG(), Modifier::BOLD, show_picker)
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>, width: usize) {
    if spans.is_empty() {
        lines.push(Line::from(""));
        return;
    }
    let width = width.max(10);
    let mut current = Vec::new();
    let mut current_width = 0;
    for span in spans {
        let style = span.style;
        let text = span.content.into_owned();
        for word in text.split_inclusive(|c: char| c.is_whitespace()) {
            let word_width = word.width();
            if word_width > width {
                // Break long unspaced tokens (like path lists app/foo.rsapp/bar.rs) at '/' or '.' boundary if available
                let mut chunk = String::new();
                let mut chunk_w = 0;
                for ch in word.chars() {
                    let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                    if current_width + chunk_w + ch_w > width && (current_width > 0 || chunk_w > 0)
                    {
                        if !chunk.is_empty() {
                            current.push(Span::styled(std::mem::take(&mut chunk), style));
                        }
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                        chunk_w = 0;
                    }
                    chunk.push(ch);
                    chunk_w += ch_w;
                    if (ch == '/' || ch == '.') && current_width + chunk_w >= width / 2 {
                        current.push(Span::styled(std::mem::take(&mut chunk), style));
                        current_width += chunk_w;
                        chunk_w = 0;
                    }
                }
                if !chunk.is_empty() {
                    current.push(Span::styled(chunk, style));
                    current_width += chunk_w;
                }
            } else {
                if current_width > 0 && current_width + word_width > width {
                    lines.push(Line::from(std::mem::take(&mut current)));
                    current_width = 0;
                }
                current.push(Span::styled(word.to_string(), style));
                current_width += word_width;
            }
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
}

/// Render CommonMark into ratatui lines. Fenced code blocks are returned as
/// ordinary tagged lines so the existing code-panel/highlighter path remains
/// the single owner of code block rendering.
///
/// `use_cache` must be `false` for transient content such as the live
/// streaming buffer: that text changes on every frame, so caching it would
/// insert one throwaway entry per redraw and evict genuinely reusable renders.
pub(super) fn render_markdown<'a>(
    content: &str,
    width: usize,
    show_picker: bool,
    use_cache: bool,
) -> Vec<Line<'a>> {
    if !use_cache {
        return render_markdown_uncached(content, width, show_picker);
    }
    let key = cache_key(content, width, show_picker);
    let cache = render_cache();
    if let Some(lines) = cache.lock().unwrap().get(&key).cloned() {
        return lines;
    }
    let lines = render_markdown_uncached(content, width, show_picker);
    cache.lock().unwrap().insert(key, lines.clone());
    lines
}

fn render_markdown_uncached(content: &str, width: usize, show_picker: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut paragraph = Vec::<Span<'static>>::new();
    let mut inline = InlineStyle::default();
    let mut heading: Option<HeadingLevel> = None;
    let mut quote_depth = 0usize;
    let mut list_depth = 0usize;
    let mut ordered_index = Vec::<Option<u64>>::new();

    let mut in_table = false;
    let mut table_rows: Vec<(Vec<String>, bool)> = Vec::new(); // (cells, is_header)
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let flush = |lines: &mut Vec<Line<'static>>,
                 paragraph: &mut Vec<Span<'static>>,
                 quote_depth: usize,
                 list_depth: usize| {
        if !paragraph.is_empty() {
            push_wrapped(
                lines,
                std::mem::take(paragraph),
                width.saturating_sub(quote_depth * 2 + list_depth * 2),
            );
        }
    };
    let flush_table = |lines: &mut Vec<Line<'static>>,
                       rows: &[(Vec<String>, bool)],
                       width: usize,
                       show_picker: bool| {
        if rows.is_empty() {
            return;
        }
        let cols = rows.iter().map(|(r, _)| r.len()).max().unwrap_or(0);
        let header = rows
            .iter()
            .find_map(|(cells, is_header)| is_header.then_some(cells));
        let grid_is_cramped = cols > 1 && width < cols.saturating_mul(14).saturating_add(4);
        if grid_is_cramped {
            if let Some(header) = header {
                let body_rows = rows
                    .iter()
                    .filter(|(_, is_header)| !is_header)
                    .collect::<Vec<_>>();
                for (row_index, (cells, _)) in body_rows.iter().enumerate() {
                    for (column, value) in cells.iter().enumerate() {
                        let label = header
                            .get(column)
                            .filter(|label| !label.is_empty())
                            .cloned()
                            .unwrap_or_else(|| format!("Field {}", column + 1));
                        push_wrapped(
                            lines,
                            vec![
                                Span::styled(
                                    format!("  {label}: "),
                                    get_themed_style(
                                        COLOR_MUTED(),
                                        COLOR_BG(),
                                        Modifier::BOLD,
                                        show_picker,
                                    ),
                                ),
                                Span::styled(
                                    value.clone(),
                                    get_themed_style(
                                        COLOR_TEXT(),
                                        COLOR_BG(),
                                        Modifier::empty(),
                                        show_picker,
                                    ),
                                ),
                            ],
                            width,
                        );
                    }
                    if row_index + 1 < body_rows.len() {
                        lines.push(Line::from(""));
                    }
                }
                return;
            }
        }
        // Ideal widths capped per-column: content drives width, header truncated to cap
        let caps = [18usize, 10, 18, 14, 18]; // last col widened so "Replay or Conversation?" content-driven
        let mut col_widths = vec![3usize; cols];
        // Content drives width; headers are truncated like any other cell (capped) so
        // a long header like "Replay or Conversation?" never widens the column past
        // the widest content cell.
        for (cells, is_header) in rows {
            for (i, c) in cells.iter().enumerate() {
                let cap = caps.get(i).copied().unwrap_or(22);
                let effective = if *is_header {
                    c.width().min(cap)
                } else {
                    c.width().min(cap)
                };
                col_widths[i] = col_widths[i].max(effective);
            }
        }
        // Weighted shrink: large token columns shrink first, Session last
        let mut total: usize = col_widths.iter().sum::<usize>() + cols.saturating_sub(1) * 3 + 4; // +4 outer borders
        if total > width && cols > 0 {
            let mut excess = total.saturating_sub(width);
            let order: Vec<usize> = {
                let mut idxs: Vec<usize> = (0..cols).collect();
                idxs.sort_by_key(|&i| {
                    if i == 0 {
                        100
                    } else if i == 2 {
                        0
                    } else {
                        1
                    }
                }); // shrink Total first
                idxs
            };
            for &i in &order {
                if excess == 0 {
                    break;
                }
                let min_w = caps
                    .get(i)
                    .map(|_| 6)
                    .unwrap_or(5)
                    .min(col_widths[i].saturating_sub(1));
                let can_shrink = col_widths[i].saturating_sub(min_w);
                let take = can_shrink.min(excess);
                col_widths[i] -= take;
                excess -= take;
            }
            total = col_widths.iter().sum::<usize>() + cols.saturating_sub(1) * 3 + 4;
        }
        // Expand: if table is narrower than available width, give remainder to the
        // widest-content column so short headers like "Module"/"Purpose" don't leave
        // 80 cols empty on the right (Core Modules case). Content drives width.
        if total < width && cols > 0 {
            let remainder = width.saturating_sub(total);
            // pick column with widest uncapped content (usually Purpose/last col)
            let mut uncapped = vec![0usize; cols];
            for (cells, _) in rows.iter() {
                for (i, c) in cells.iter().enumerate() {
                    if i < cols {
                        uncapped[i] = uncapped[i].max(c.width());
                    }
                }
            }
            let target = uncapped
                .iter()
                .enumerate()
                .max_by_key(|&(_, &w)| w)
                .map(|(i, _)| i)
                .unwrap_or(cols - 1);
            col_widths[target] += remainder;
        }
        // outer borders: 1 padding spaces per cell side: "─" * (w + 2)
        let top = format!(
            "┌{}┐",
            col_widths
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("┬")
        );
        let bottom = format!(
            "└{}┘",
            col_widths
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("┴")
        );
        lines.push(Line::from(Span::styled(
            top,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )));
        for (idx, (cells, is_header)) in rows.iter().enumerate() {
            let style = if *is_header {
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker)
            } else {
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker)
            };

            // Wrap each cell's text into lines fitting col_widths[i]
            let mut cell_lines: Vec<Vec<String>> = Vec::new();
            let mut max_cell_height = 1usize;

            for i in 0..cols {
                let txt = cells.get(i).map(|s| s.as_str()).unwrap_or("");
                let w = col_widths[i];
                let mut lines_for_cell = Vec::new();

                if txt.is_empty() {
                    lines_for_cell.push(String::new());
                } else {
                    for word in txt.split_inclusive(|c: char| c.is_whitespace()) {
                        if lines_for_cell.is_empty() {
                            lines_for_cell.push(String::new());
                        }
                        let last_line = lines_for_cell.last_mut().unwrap();
                        let last_w = last_line.width();
                        let word_w = word.width();

                        if last_w + word_w <= w || last_w == 0 {
                            last_line.push_str(word);
                        } else {
                            lines_for_cell.push(word.trim_start().to_string());
                        }
                    }
                }
                max_cell_height = max_cell_height.max(lines_for_cell.len());
                cell_lines.push(lines_for_cell);
            }

            // Render row line by line for multi-line cells
            for line_idx in 0..max_cell_height {
                let mut row_spans = Vec::new();
                row_spans.push(Span::styled(
                    "│".to_string(),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                ));

                for i in 0..cols {
                    let cell_txt = cell_lines[i]
                        .get(line_idx)
                        .map(|s| s.trim_end())
                        .unwrap_or("");
                    let w = col_widths[i];
                    let formatted = if cell_txt.width() > w {
                        let mut end = 0;
                        let mut current_w = 0;
                        let target_w = w.saturating_sub(1);
                        for ch in cell_txt.chars() {
                            let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                            if current_w + ch_w > target_w {
                                break;
                            }
                            current_w += ch_w;
                            end += ch.len_utf8();
                        }
                        let truncated = &cell_txt[..end];
                        let pad = w.saturating_sub(current_w + 1);
                        format!(" {}…{:pad$} ", truncated, "", pad = pad)
                    } else {
                        let pad = w.saturating_sub(cell_txt.width());
                        format!(" {}{:pad$} ", cell_txt, "", pad = pad)
                    };
                    row_spans.push(Span::styled(formatted, style));
                    row_spans.push(Span::styled(
                        "│".to_string(),
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ));
                }
                lines.push(Line::from(row_spans));
            }

            // Draw divider only under header row
            if idx == 0 && rows[0].1 {
                let div = format!(
                    "├{}┤",
                    col_widths
                        .iter()
                        .map(|w| "─".repeat(w + 2))
                        .collect::<Vec<_>>()
                        .join("┼")
                );
                lines.push(Line::from(Span::styled(
                    div,
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
        }
        lines.push(Line::from(Span::styled(
            bottom,
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
        )));
    };

    for event in Parser::new_ext(content, Options::all()) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                lines.push(Line::from(""));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                heading = Some(level);
            }
            Event::End(TagEnd::Heading { .. }) => {
                if !paragraph.is_empty() {
                    let style = heading_style(heading.unwrap_or(HeadingLevel::H3), show_picker);
                    let text: Vec<Span<'static>> = std::mem::take(&mut paragraph)
                        .into_iter()
                        .map(|s| Span::styled(s.content.into_owned(), style))
                        .collect();
                    lines.push(Line::from(text));
                }
                heading = None;
                lines.push(Line::from(""));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                quote_depth = quote_depth.saturating_sub(1);
                lines.push(Line::from(""));
            }
            Event::Start(Tag::List(first)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                list_depth += 1;
                ordered_index.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                list_depth = list_depth.saturating_sub(1);
                ordered_index.pop();
                lines.push(Line::from(""));
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                let marker = if let Some(Some(index)) = ordered_index.last() {
                    format!("{}{index}. ", indent)
                } else {
                    format!("{}• ", indent)
                };
                paragraph.push(Span::styled(
                    marker,
                    get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
                ));
            }
            Event::End(TagEnd::Item) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                if let Some(Some(index)) = ordered_index.last_mut() {
                    *index += 1;
                }
            }
            Event::Start(Tag::Strong) => inline.bold = true,
            Event::End(TagEnd::Strong) => inline.bold = false,
            Event::Start(Tag::Emphasis) => inline.italic = true,
            Event::End(TagEnd::Emphasis) => inline.italic = false,
            Event::Start(Tag::Strikethrough) => inline.strike = true,
            Event::End(TagEnd::Strikethrough) => inline.strike = false,
            Event::Code(text) => paragraph.push(Span::styled(
                text.to_string(),
                text_style(
                    InlineStyle {
                        code: true,
                        ..inline
                    },
                    show_picker,
                ),
            )),
            Event::Start(Tag::Link { .. }) => inline.link = true,
            Event::End(TagEnd::Link) => inline.link = false,
            Event::Text(text) | Event::InlineHtml(text) => {
                if in_table {
                    // Cap per-cell content so a 55KB curl dump doesn't become a single wide row
                    // and blow out column widths (the "green stuff" overflow in 1786013456760).
                    if current_cell.len() + text.len() > 400 {
                        let remaining = 400usize.saturating_sub(current_cell.len());
                        if remaining > 3 {
                            current_cell.push_str(
                                &text[..text.floor_char_boundary(remaining.saturating_sub(1))],
                            );
                            current_cell.push('…');
                        }
                    } else {
                        current_cell.push_str(&text);
                    }
                    continue;
                }
                let mut style = inline;
                if heading.is_some() {
                    style.bold = true;
                }
                if quote_depth > 0 && paragraph.is_empty() {
                    paragraph.push(Span::styled(
                        "│ ".repeat(quote_depth),
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ));
                }
                let span_style = if let Some(level) = heading {
                    heading_style(level, show_picker)
                } else {
                    text_style(style, show_picker)
                };
                paragraph.push(Span::styled(text.to_string(), span_style));
            }
            Event::SoftBreak => {
                // CommonMark: a soft break is just whitespace — the paragraph
                // keeps reflowing to the pane width via `push_wrapped`. Only a
                // hard break forces a real line. Treating both as a hard flush
                // (as this used to) let the model's own source-line wrapping
                // dictate line breaks instead of the actual terminal width.
                if let Some(last) = paragraph.last() {
                    if !last.content.ends_with(char::is_whitespace) {
                        paragraph.push(Span::styled(" ", text_style(inline, show_picker)));
                    }
                }
            }
            Event::HardBreak => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
            }
            Event::Rule => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                lines.push(Line::from(Span::styled(
                    "─".repeat(width.max(1)),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                let language = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                lines.push(Line::from(Span::styled(
                    format!("```{language}"),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
            Event::End(TagEnd::CodeBlock) => {
                lines.push(Line::from(Span::styled(
                    "```",
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                flush_table(&mut lines, &table_rows, width, show_picker);
                table_rows.clear();
                in_table = false;
                current_row.clear();
                current_cell.clear();
                lines.push(Line::from(""));
            }
            Event::Start(Tag::TableHead) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                if !current_row.is_empty() {
                    table_rows.push((std::mem::take(&mut current_row), true));
                }
            }
            Event::Start(Tag::TableRow) => {
                current_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !current_row.is_empty() {
                    table_rows.push((std::mem::take(&mut current_row), false));
                }
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                current_row.push(std::mem::take(&mut current_cell));
            }
            Event::Html(text) => paragraph.push(Span::styled(
                text.to_string(),
                text_style(inline, show_picker),
            )),
            _ => {}
        }
    }
    flush(&mut lines, &mut paragraph, quote_depth, list_depth);
    while lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{MarkdownCache, cache_key, render_cache, render_markdown};
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    #[test]
    fn renders_markdown_tables_with_column_separators() {
        let md = "| Header 1 | Header 2 |\n|---|---|\n| Cell 1 | Cell 2 |";
        let lines = render_markdown(md, 80, false, false);
        assert!(!lines.is_empty());
        let all: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(all.contains("Header 1 │ Header 2"));
        assert!(all.contains('┌') && all.contains('┐') && all.contains('└'));
    }

    #[test]
    fn narrow_tables_render_as_key_value_records() {
        let md = concat!(
            "| Name | Purpose |\n",
            "|---|---|\n",
            "| rustcode | a terminal coding agent with a readable narrow table layout |"
        );
        let lines = render_markdown(md, 28, false, false);
        let rendered = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>();

        assert!(
            rendered.iter().any(|line| line.contains("Name: rustcode")),
            "unexpected narrow table rendering: {rendered:?}"
        );
        assert!(rendered.iter().any(|line| line.contains("Purpose:")));
        assert!(rendered.len() > 3, "the long value should wrap: {rendered:?}");
        assert!(
            rendered.iter().all(|line| !line.contains('┌')),
            "narrow tables must not use a cramped grid: {rendered:?}"
        );
    }

    #[test]
    fn parses_nested_inline_markup() {
        let lines = render_markdown("**bold _italic_** and `code`", 80, false, false);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "bold italic and code");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn renders_lists_and_headings_without_markdown_tokens() {
        let lines = render_markdown("# Title\n\n- one\n- two", 80, false, false);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("Title"));
        assert!(text.contains('•'));
        assert!(text.contains("one"));
        assert!(!text.contains("# "));
    }

    #[test]
    fn soft_breaks_reflow_instead_of_forcing_a_new_line() {
        // Source text hard-wrapped at ~20 cols, like a model that mimics
        // fixed-width prose. A soft break (plain newline) must reflow to the
        // requested width, not reproduce the source's own line breaks.
        let md = "one two three\nfour five six\nseven eight nine";
        let lines = render_markdown(md, 80, false, false);
        assert_eq!(lines.len(), 1, "soft breaks must not force new lines");
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "one two three four five six seven eight nine");
    }

    #[test]
    fn hard_breaks_still_force_a_new_line() {
        let md = "one two  \nthree four";
        let lines = render_markdown(md, 80, false, false);
        assert_eq!(lines.len(), 2, "trailing double-space is a real hard break");
    }

    #[test]
    fn cache_evicts_least_recently_used_entry_at_cap() {
        let cap = 8;
        let mut cache = MarkdownCache::new(cap);
        for i in 0..cap {
            cache.insert(
                cache_key(&format!("doc {i}"), 80, false),
                vec![Line::from("")],
            );
        }
        assert_eq!(cache.entries.len(), cap);

        // Touch the oldest entry so it becomes the most recently used one.
        let oldest = cache_key("doc 0", 80, false);
        assert!(cache.get(&oldest).is_some());

        // Overflow the cap; the cache must stay bounded and drop the entries
        // that have gone longest without an access ("doc 1" onwards) while the
        // freshly touched "doc 0" survives.
        for i in cap..(cap + 3) {
            cache.insert(
                cache_key(&format!("doc {i}"), 80, false),
                vec![Line::from("")],
            );
        }
        assert_eq!(cache.entries.len(), cap);
        assert!(cache.get(&oldest).is_some());
        assert!(cache.get(&cache_key("doc 1", 80, false)).is_none());
    }

    /// Number of globally cached entries rendered at `width`. Other tests share
    /// the process-wide cache, so the streaming assertion below is scoped to a
    /// width no other test uses instead of to the total entry count.
    fn global_entries_at_width(width: usize) -> usize {
        render_cache()
            .lock()
            .unwrap()
            .entries
            .keys()
            .filter(|(_, w, _, _)| *w == width)
            .count()
    }

    #[test]
    fn uncached_render_does_not_touch_the_global_cache() {
        let width = 4242;
        assert_eq!(global_entries_at_width(width), 0);
        for i in 0..64 {
            let content = format!("streaming frame {i} of a growing response");
            let lines = render_markdown(&content, width, false, false);
            assert!(!lines.is_empty());
        }
        assert_eq!(global_entries_at_width(width), 0);

        // Same content through the cached path does populate the cache, so the
        // assertion above is meaningful rather than vacuous.
        render_markdown("settled message", width, false, true);
        assert_eq!(global_entries_at_width(width), 1);
    }
}
