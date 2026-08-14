//! Structured Markdown rendering for assistant messages.
//!
//! Markdown is parsed as a document rather than interpreted one physical line
//! at a time. This keeps nested emphasis, links, lists, blockquotes, and
//! escaped text from leaking their syntax into the chat viewport.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use unicode_width::UnicodeWidthStr;

use super::lru::LruCache;
use super::{
    COLOR_BG, COLOR_GREEN, COLOR_MUTED, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_TEXT,
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
        if self.link {
            modifier |= Modifier::UNDERLINED;
        }
        modifier
    }
}

fn text_style(style: InlineStyle, show_picker: bool) -> ratatui::style::Style {
    let fg = if style.code {
        COLOR_SECONDARY()
    } else if style.link {
        COLOR_PRIMARY()
    } else {
        COLOR_TEXT()
    };
    let bg = COLOR_BG();
    get_themed_style(fg, bg, style.modifier(), show_picker)
}

fn heading_style(level: HeadingLevel, show_picker: bool) -> ratatui::style::Style {
    let modifier = match level {
        HeadingLevel::H1 => Modifier::BOLD | Modifier::UNDERLINED,
        HeadingLevel::H2 => Modifier::BOLD,
        HeadingLevel::H3 => Modifier::BOLD | Modifier::ITALIC,
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => Modifier::ITALIC,
    };
    get_themed_style(COLOR_TEXT(), COLOR_BG(), modifier, show_picker)
}

fn push_wrapped(lines: &mut Vec<Line<'static>>, spans: Vec<Span<'static>>, width: usize) {
    push_wrapped_with_continuation(lines, spans, width, None);
}

#[derive(Clone, Default)]
struct MarkdownTableCell {
    spans: Vec<Span<'static>>,
}

impl MarkdownTableCell {
    fn push_text(&mut self, text: &str, style: ratatui::style::Style) {
        if text.is_empty() {
            return;
        }
        self.spans.push(Span::styled(text.to_owned(), style));
    }

    fn plain_text(&self) -> String {
        self.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| span.content.width())
            .sum()
    }
}

fn push_table_wrapped_text(
    lines: &mut Vec<Vec<Span<'static>>>,
    current_width: &mut usize,
    text: &str,
    style: ratatui::style::Style,
    width: usize,
) {
    let width = width.max(1);
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    for character in text.chars() {
        if character == '\n' {
            lines.push(Vec::new());
            *current_width = 0;
            continue;
        }
        let character_width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
        if *current_width > 0 && *current_width + character_width > width {
            lines.push(Vec::new());
            *current_width = 0;
        }
        if character_width > width {
            continue;
        }
        if let Some(last) = lines.last_mut()
            && last.last().is_some_and(|span| span.style == style)
        {
            last.last_mut().expect("last span exists").content.to_mut().push(character);
        } else {
            lines
                .last_mut()
                .expect("table line exists")
                .push(Span::styled(character.to_string(), style));
        }
        *current_width += character_width;
    }
}

fn wrapped_table_cell(
    cell: &MarkdownTableCell,
    width: usize,
    header: bool,
) -> Vec<Vec<Span<'static>>> {
    let cell_style = |style: ratatui::style::Style| {
        if header {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    };
    let mut lines = vec![Vec::new()];
    let mut current_width = 0;
    for span in &cell.spans {
        push_table_wrapped_text(
            &mut lines,
            &mut current_width,
            span.content.as_ref(),
            cell_style(span.style),
            width,
        );
    }
    lines
}

fn continuation_line(prefix: Option<&Span<'static>>) -> (Vec<Span<'static>>, usize) {
    let Some(prefix) = prefix else {
        return (Vec::new(), 0);
    };
    let width = prefix.content.width();
    (vec![prefix.clone()], width)
}

fn push_wrapped_with_continuation(
    lines: &mut Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    width: usize,
    continuation: Option<Span<'static>>,
) {
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
                        (current, current_width) = continuation_line(continuation.as_ref());
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
                    (current, current_width) = continuation_line(continuation.as_ref());
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

/// Unwrap a closed `md`/`markdown` fence only when its body contains a real
/// Markdown table. Models sometimes fence tables for presentation, which
/// would otherwise turn them into literal code instead of the width-aware
/// table rendering used by the transcript.
pub(super) fn unwrap_markdown_table_fences(content: &str) -> std::borrow::Cow<'_, str> {
    if !content.contains("```") && !content.contains("~~~") {
        return std::borrow::Cow::Borrowed(content);
    }

    #[derive(Clone, Copy)]
    struct Fence {
        marker: char,
        len: usize,
        blockquoted: bool,
    }

    fn strip_blockquote_prefix(line: &str) -> &str {
        let mut rest = line.trim_start();
        while let Some(stripped) = rest.strip_prefix('>') {
            rest = stripped.trim_start();
        }
        rest
    }

    fn scan_fence(line: &str) -> Option<(char, usize, &str, bool)> {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let leading = line.bytes().take_while(|byte| *byte == b' ').count();
        if leading > 3 {
            return None;
        }
        let trimmed = &line[leading..];
        let blockquoted = trimmed.starts_with('>');
        let scan = strip_blockquote_prefix(trimmed);
        let marker = scan.chars().next()?;
        if marker != '`' && marker != '~' {
            return None;
        }
        let len = scan
            .chars()
            .take_while(|character| *character == marker)
            .count();
        (len >= 3).then_some((marker, len, &scan[len..], blockquoted))
    }

    let contains_table = |body: &[&str]| {
        body.windows(2).any(|pair| {
            let header = strip_blockquote_prefix(pair[0]).trim();
            header.contains('|')
                && !is_table_delimiter_line(header)
                && is_table_delimiter_line(pair[1])
        })
    };

    let lines = content.split_inclusive('\n').collect::<Vec<_>>();
    let mut output = String::with_capacity(content.len());
    let mut changed = false;
    let mut index = 0;
    while index < lines.len() {
        let Some((marker, len, info, blockquoted)) = scan_fence(lines[index]) else {
            output.push_str(lines[index]);
            index += 1;
            continue;
        };
        let info = info.split_whitespace().next().unwrap_or_default();
        if !info.eq_ignore_ascii_case("md") && !info.eq_ignore_ascii_case("markdown") {
            output.push_str(lines[index]);
            index += 1;
            continue;
        }
        let fence = Fence {
            marker,
            len,
            blockquoted,
        };

        let close = (index + 1..lines.len()).find(|&line_index| {
            scan_fence(lines[line_index]).is_some_and(
                |(close_marker, close_len, trailing, close_blockquoted)| {
                    close_marker == fence.marker
                        && close_len >= fence.len
                        && trailing.trim().is_empty()
                        && (!fence.blockquoted || close_blockquoted)
                },
            )
        });
        let Some(close) = close else {
            for line in &lines[index..] {
                output.push_str(line);
            }
            break;
        };
        if contains_table(&lines[index + 1..close]) {
            for line in &lines[index + 1..close] {
                output.push_str(line);
            }
            changed = true;
        } else {
            for line in &lines[index..=close] {
                output.push_str(line);
            }
        }
        index = close + 1;
    }

    if changed {
        std::borrow::Cow::Owned(output)
    } else {
        std::borrow::Cow::Borrowed(content)
    }
}

fn is_table_dash(character: char) -> bool {
    matches!(
        character,
        '-'
            | '\u{2010}'
            | '\u{2011}'
            | '\u{2012}'
            | '\u{2013}'
            | '\u{2014}'
            | '\u{2015}'
            | '\u{2212}'
            | '\u{2500}'
            | '\u{FE58}'
            | '\u{FE63}'
            | '\u{FF0D}'
    )
}

fn strip_table_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    while let Some(stripped) = rest.strip_prefix('>') {
        rest = stripped.trim_start();
    }
    rest
}

fn is_table_delimiter_line(line: &str) -> bool {
    let trimmed = strip_table_blockquote_prefix(line).trim().trim_matches('|');
    if !trimmed.contains('|') {
        return false;
    }
    let cells = trimmed.split('|').map(str::trim).collect::<Vec<_>>();
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let dashes = cell.trim_matches(':');
            let dash_count = dashes.chars().count();
            let has_unicode_dash = dashes.chars().any(|character| character != '-');
            (dash_count >= 3 || (has_unicode_dash && dash_count >= 2))
                && dashes.chars().all(is_table_dash)
        })
}

/// Normalize the dash glyphs models commonly use for Markdown table separator
/// rows (`––––`/`——`) to ASCII hyphens, which CommonMark recognizes as a table
/// delimiter. Non-separator content is returned byte-for-byte unchanged.
fn normalize_table_delimiters(content: &str) -> std::borrow::Cow<'_, str> {
    let mut normalized = None::<String>;
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        if is_table_delimiter_line(line) {
            let output = normalized.get_or_insert_with(|| String::with_capacity(content.len()));
            if output.is_empty() {
                output.push_str(&content[..offset]);
            }
            for character in line.chars() {
                if is_table_dash(character) && character != '-' {
                    output.push_str("---");
                } else {
                    output.push(character);
                }
            }
        } else if let Some(output) = normalized.as_mut() {
            output.push_str(line);
        }
        offset += line.len();
    }

    normalized.map_or(std::borrow::Cow::Borrowed(content), std::borrow::Cow::Owned)
}

fn sanitize_markdown(content: &str) -> std::borrow::Cow<'_, str> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return std::borrow::Cow::Borrowed(content);
    }
    let is_table_line = |line: &str| -> bool {
        let t = line.trim();
        t.starts_with('|') || (t.contains('|') && t.contains("---"))
    };
    let is_list_item = |line: &str| -> bool {
        let t = line.trim_start();
        t.starts_with("- ")
            || t.starts_with("* ")
            || t.starts_with("+ ")
            || t.starts_with("• ")
            || t.starts_with("•")
            || t.starts_with("· ")
            || t.starts_with("·")
            || t.starts_with("● ")
            || t.starts_with("●")
            || (t.chars().next().is_some_and(|c| c.is_ascii_digit())
                && t.find(". ").is_some_and(|idx| idx <= 4))
    };

    let mut output = String::with_capacity(content.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            let next_non_empty = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty());
            let prev_non_empty = (0..i).rfind(|&j| !lines[j].trim().is_empty());
            if let (Some(prev_idx), Some(next_idx)) = (prev_non_empty, next_non_empty) {
                // Drop blank lines between table rows
                if is_table_line(lines[prev_idx]) && is_table_line(lines[next_idx]) {
                    i += 1;
                    continue;
                }
                // Drop blank lines between list items
                if is_list_item(lines[prev_idx]) && is_list_item(lines[next_idx]) {
                    i += 1;
                    continue;
                }
            }
        }

        if !output.is_empty() {
            output.push('\n');
        }

        // Normalize bullet character to '- ' so pulldown_cmark parses list items
        let trimmed_start = line.trim_start();
        let bullet_prefixes = ["• ", "•", "· ", "·", "● ", "●"];
        if let Some(matched_prefix) = bullet_prefixes.iter().find(|&&p| trimmed_start.starts_with(p)) {
            let indent_len = line.len() - trimmed_start.len();
            let indent = &line[..indent_len];
            let rest = trimmed_start[matched_prefix.len()..].trim_start();
            output.push_str(indent);
            output.push_str("- ");
            output.push_str(rest);
        } else {
            output.push_str(line);
        }

        i += 1;
    }
    std::borrow::Cow::Owned(output)
}

fn render_markdown_uncached(content: &str, width: usize, show_picker: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut paragraph = Vec::<Span<'static>>::new();
    let mut inline = InlineStyle::default();
    let mut heading: Option<HeadingLevel> = None;
    let mut quote_depth = 0usize;
    let mut list_depth = 0usize;
    let mut ordered_index = Vec::<Option<u64>>::new();
    let mut list_continuation = None::<String>;

    let mut in_table = false;
    let mut table_rows: Vec<(Vec<MarkdownTableCell>, bool)> = Vec::new();
    let mut current_row: Vec<MarkdownTableCell> = Vec::new();
    let mut current_cell = MarkdownTableCell::default();
    let mut table_alignments = Vec::<Alignment>::new();
    let flush = |lines: &mut Vec<Line<'static>>,
                 paragraph: &mut Vec<Span<'static>>,
                 quote_depth: usize,
                 list_continuation: Option<&str>| {
        if !paragraph.is_empty() {
            let continuation = list_continuation
                .map(|prefix| {
                    Span::styled(
                        prefix.to_owned(),
                        get_themed_style(
                            COLOR_MUTED(),
                            COLOR_BG(),
                            Modifier::empty(),
                            show_picker,
                        ),
                    )
                })
                .or_else(|| {
                    (quote_depth > 0).then(|| {
                        Span::styled(
                            "> ".repeat(quote_depth),
                            get_themed_style(
                                COLOR_GREEN(),
                                COLOR_BG(),
                                Modifier::empty(),
                                show_picker,
                            ),
                        )
                    })
                });
            push_wrapped_with_continuation(
                lines,
                std::mem::take(paragraph),
                width,
                continuation,
            );
        }
    };
    let flush_table = |lines: &mut Vec<Line<'static>>,
                       rows: &[(Vec<MarkdownTableCell>, bool)],
                       width: usize,
                       show_picker: bool,
                       alignments: &[Alignment]| {
        if rows.is_empty() {
            return;
        }
        let cols = rows.iter().map(|(r, _)| r.len()).max().unwrap_or(0);
        let grid_is_cramped = cols > 1 && width < cols.saturating_mul(14).saturating_add(4);
        if grid_is_cramped {
            if let Some(header) = rows
                .iter()
                .find_map(|(cells, is_header)| is_header.then_some(cells))
            {
                let body_rows = rows
                    .iter()
                    .filter(|(_, is_header)| !is_header)
                    .collect::<Vec<_>>();
                for (row_index, (cells, _)) in body_rows.iter().enumerate() {
                    for (column, cell) in cells.iter().enumerate() {
                        let label = header
                            .get(column)
                            .map(MarkdownTableCell::plain_text)
                            .filter(|label| !label.is_empty())
                            .unwrap_or_else(|| format!("Field {}", column + 1));
                        let mut value_spans = vec![Span::styled(
                            format!("  {label}: "),
                            get_themed_style(
                                COLOR_MUTED(),
                                COLOR_BG(),
                                Modifier::BOLD,
                                show_picker,
                            ),
                        )];
                        value_spans.extend(cell.spans.clone());
                        push_wrapped(
                            lines,
                            value_spans,
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
        // Content drives width. Keep compact tables compact instead of expanding them to
        // the viewport; when a table is wider than the viewport, shrink its columns and
        // wrap the cells below without dropping their inline styles.
        let mut col_widths = vec![3usize; cols];
        for (cells, _) in rows {
            for (i, c) in cells.iter().enumerate() {
                col_widths[i] = col_widths[i].max(c.width());
            }
        }
        // Two spaces between columns match Codex's readable table rhythm without
        // adding a box around every cell.
        const TABLE_COLUMN_GAP: usize = 2;
        const TABLE_CELL_PADDING: usize = 1;
        let mut total: usize = col_widths
            .iter()
            .map(|width| width + TABLE_CELL_PADDING * 2)
            .sum::<usize>()
            + cols.saturating_sub(1) * TABLE_COLUMN_GAP;
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
                let min_w = 3;
                let can_shrink = col_widths[i].saturating_sub(min_w);
                let take = can_shrink.min(excess);
                col_widths[i] -= take;
                excess -= take;
            }
            total = col_widths
                .iter()
                .map(|width| width + TABLE_CELL_PADDING * 2)
                .sum::<usize>()
                + cols.saturating_sub(1) * TABLE_COLUMN_GAP;
        }
        for (idx, (cells, is_header)) in rows.iter().enumerate() {
            let mut cell_lines: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
            let mut max_cell_height = 1usize;
            for i in 0..cols {
                let cell = cells.get(i).cloned().unwrap_or_default();
                let lines_for_cell = wrapped_table_cell(&cell, col_widths[i], *is_header);
                max_cell_height = max_cell_height.max(lines_for_cell.len());
                cell_lines.push(lines_for_cell);
            }

            for line_idx in 0..max_cell_height {
                let mut row_spans = Vec::new();
                for i in 0..cols {
                    let w = col_widths[i];
                    let spans = cell_lines[i].get(line_idx).cloned().unwrap_or_default();
                    let visible_width: usize = spans
                        .iter()
                        .map(|span| span.content.width())
                        .sum();
                    let remaining = w.saturating_sub(visible_width);
                    let alignment = alignments
                        .get(i)
                        .copied()
                        .unwrap_or(Alignment::None);
                    let left_padding = match alignment {
                        Alignment::Center => remaining / 2,
                        Alignment::Right => remaining,
                        Alignment::Left | Alignment::None => 0,
                    };
                    let right_padding = remaining.saturating_sub(left_padding);
                    row_spans.push(Span::raw(" ".repeat(TABLE_CELL_PADDING + left_padding)));
                    row_spans.extend(spans.into_iter().map(|mut span| {
                        if *is_header {
                            span.style = span.style.add_modifier(Modifier::BOLD);
                        }
                        span
                    }));
                    row_spans.push(Span::raw(" ".repeat(TABLE_CELL_PADDING + right_padding)));
                    if i + 1 < cols {
                        row_spans.push(Span::raw(" ".repeat(TABLE_COLUMN_GAP)));
                    }
                }
                lines.push(Line::from(row_spans));
            }

            // A strong rule separates the header; lighter rules keep body rows scannable.
            if idx + 1 < rows.len() {
                let separator = if idx == 0 && rows[0].1 { '━' } else { '─' };
                lines.push(Line::from(Span::styled(
                    separator.to_string().repeat(total),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
        }
    };

    let normalized = unwrap_markdown_table_fences(content);
    let normalized = normalize_table_delimiters(&normalized);
    let sanitized = sanitize_markdown(&normalized);
    for event in Parser::new_ext(&sanitized, Options::all()) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                if list_depth == 0 {
                    lines.push(Line::from(""));
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
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
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                quote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                quote_depth = quote_depth.saturating_sub(1);
                lines.push(Line::from(""));
            }
            Event::Start(Tag::List(first)) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                list_depth += 1;
                ordered_index.push(first);
            }
            Event::End(TagEnd::List(_)) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                list_depth = list_depth.saturating_sub(1);
                ordered_index.pop();
                if list_depth == 0 && lines.last().is_some_and(|l| !l.spans.is_empty()) {
                    lines.push(Line::from(""));
                }
            }
            Event::Start(Tag::Item) => {
                if list_depth > 0 && lines.last().is_some_and(|l| l.spans.is_empty()) {
                    lines.pop();
                }
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                let marker = if let Some(Some(index)) = ordered_index.last() {
                    format!("{}{index}. ", indent)
                } else {
                    format!("{}• ", indent)
                };
                let quote_prefix = "> ".repeat(quote_depth);
                list_continuation = Some(format!("{quote_prefix}{}", " ".repeat(marker.width())));
                if !quote_prefix.is_empty() {
                    paragraph.push(Span::styled(
                        quote_prefix,
                        get_themed_style(
                            COLOR_MUTED(),
                            COLOR_BG(),
                            Modifier::empty(),
                            show_picker,
                        ),
                    ));
                }
                paragraph.push(Span::styled(
                    marker,
                    get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
                ));
            }
            Event::End(TagEnd::Item) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                if let Some(Some(index)) = ordered_index.last_mut() {
                    *index += 1;
                }
                list_continuation = None;
            }
            Event::Start(Tag::Strong) => inline.bold = true,
            Event::End(TagEnd::Strong) => inline.bold = false,
            Event::Start(Tag::Emphasis) => inline.italic = true,
            Event::End(TagEnd::Emphasis) => inline.italic = false,
            Event::Start(Tag::Strikethrough) => inline.strike = true,
            Event::End(TagEnd::Strikethrough) => inline.strike = false,
            Event::Code(text) => {
                if in_table {
                    current_cell.push_text(
                        &text,
                        text_style(
                            InlineStyle {
                                code: true,
                                ..inline
                            },
                            show_picker,
                        ),
                    );
                } else {
                    paragraph.push(Span::styled(
                        text.to_string(),
                        text_style(
                            InlineStyle {
                                code: true,
                                ..inline
                            },
                            show_picker,
                        ),
                    ));
                }
            }
            Event::Start(Tag::Link { .. }) => inline.link = true,
            Event::End(TagEnd::Link) => inline.link = false,
            Event::Text(text) | Event::InlineHtml(text) => {
                if in_table {
                    // Cap per-cell content so a large command result cannot become an
                    // unbounded table. Keep each parser span separate so inline Markdown
                    // styles survive into the table renderer.
                    let style = if heading.is_some() {
                        heading_style(heading.unwrap_or(HeadingLevel::H3), show_picker)
                    } else {
                        text_style(inline, show_picker)
                    };
                    let current_len = current_cell.plain_text().len();
                    let remaining = 400usize.saturating_sub(current_len);
                    if remaining > 0 {
                        let end = text
                            .char_indices()
                            .take_while(|(index, _)| *index < remaining)
                            .map(|(index, character)| index + character.len_utf8())
                            .last()
                            .unwrap_or(0)
                            .min(text.len());
                        current_cell.push_text(&text[..end], style);
                        if end < text.len() {
                            current_cell.push_text("…", style);
                        }
                    }
                    continue;
                }
                let mut style = inline;
                if heading.is_some() {
                    style.bold = true;
                }
                if quote_depth > 0 && paragraph.is_empty() {
                    paragraph.push(Span::styled(
                        "> ".repeat(quote_depth),
                        get_themed_style(COLOR_GREEN(), COLOR_BG(), Modifier::empty(), show_picker),
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
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
            }
            Event::Rule => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                lines.push(Line::from(Span::styled(
                    "─".repeat(width.max(1)),
                    get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                )));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
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
            Event::Start(Tag::Table(alignments)) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                in_table = true;
                table_rows.clear();
                table_alignments = alignments;
            }
            Event::End(TagEnd::Table) => {
                flush(
                    &mut lines,
                    &mut paragraph,
                    quote_depth,
                    list_continuation.as_deref(),
                );
                flush_table(
                    &mut lines,
                    &table_rows,
                    width,
                    show_picker,
                    &table_alignments,
                );
                table_rows.clear();
                in_table = false;
                current_row.clear();
                current_cell = MarkdownTableCell::default();
                table_alignments.clear();
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
                current_cell = MarkdownTableCell::default();
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
    flush(
        &mut lines,
        &mut paragraph,
        quote_depth,
        list_continuation.as_deref(),
    );
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
        assert!(all.contains("Header 1"));
        assert!(all.contains("Header 2"));
        assert!(all.contains('━'));
        assert!(!all.contains('┌') && !all.contains('│'));
    }

    #[test]
    fn table_cells_keep_inline_markdown_styles() {
        let md = "| Name | Value |\n|---|---|\n| **bold** | `code` |";
        let lines = render_markdown(md, 80, false, false);
        let bold = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("bold"))
            .expect("bold table cell should be rendered");
        let code = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("code"))
            .expect("code table cell should be rendered");
        assert_eq!(bold.style.add_modifier(Modifier::BOLD), bold.style);
        assert_ne!(bold.style, code.style, "inline code should retain its style");
    }

    #[test]
    fn table_alignment_and_header_emphasis_survive_terminal_layout() {
        let md = concat!(
            "| Left | Center | Right |\n",
            "|:---|:---:|---:|\n",
            "| a | b | c |"
        );
        let lines = render_markdown(md, 50, false, false);
        let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();
        let header = rendered.first().expect("table header");
        let body = rendered
            .iter()
            .find(|line| line.contains('a') && line.contains('b') && line.contains('c'))
            .expect("table body");
        assert!(
            header.contains("Left") && header.contains("Center") && header.contains("Right")
        );
        assert!(
            body.starts_with(" a"),
            "cells should have readable padding: {body:?}"
        );
        assert!(lines[0]
            .spans
            .iter()
            .filter(|span| span.content.contains("Left") || span.content.contains("Center"))
            .all(|span| span.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn renders_latest_commits_table_fixture_as_markdown() {
        let md = concat!(
            "| Commit | Message |\n",
            "|--------|---------|\n",
            "| `3d6a1a5` | fix(ui): restore single-line bulleted tool call items matching screenshot design (#581) |\n",
            "| `1089c53` | fix(ui): space consecutive thinking blocks, add working padding, and pin chat composer (#580) |\n",
            "| `840ea2b` | Merge pull request #579 from LHagfoss/fix/working-status-final-frame |\n",
            "| `10e8085` | fix(ui): keep working through final frame |\n",
            "| `ad1ab89` | Merge pull request #578 from LHagfoss/fix/tool-confirmation-small-terminal |\n",
            "| `d763994` | fix(ui): cover compact modal boundary |\n",
            "| `f12b0e6` | fix(ui): preserve compact modal scope |\n",
            "| `55180aa` | fix(ui): keep short modal actions visible |\n",
            "| `b1df637` | fix(ui): guard short confirmation modals |\n",
            "| `764fe6b` | fix(ui): use compact welcome box and clean thought block preambles (#577) |"
        );
        let rendered = render_markdown(md, 80, false, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let table_index = rendered
            .iter()
            .position(|line| line.contains("Commit"))
            .expect("table should render with a header");

        assert!(rendered[table_index..].iter().any(|line| line.contains("3d6a1a5")));
        assert!(rendered[table_index..].iter().any(|line| line.contains("764fe6b")));
        assert!(
            rendered[..table_index]
                .iter()
                .all(|line| !line.contains("3d6a1a5")),
            "inline code from a table cell escaped above the table: {rendered:?}"
        );
        assert!(rendered.iter().all(|line| !line.contains("| Commit |")));
    }

    #[test]
    fn wrapped_list_items_keep_a_hanging_indent() {
        let lines = render_markdown("- one two three four five six seven", 24, false, false);
        let rendered = lines
            .iter()
            .map(Line::to_string)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();

        assert!(rendered.len() > 1, "fixture must wrap: {rendered:?}");
        assert!(rendered[0].starts_with("• "));
        assert!(rendered[1].starts_with("  "));
        assert!(!rendered[1].starts_with("• "));
    }

    #[test]
    fn wrapped_blockquotes_keep_their_gutter() {
        let lines = render_markdown(
            "> one two three four five six seven eight",
            24,
            false,
            false,
        );
        let rendered = lines
            .iter()
            .map(Line::to_string)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();

        assert!(rendered.len() > 1, "fixture must wrap: {rendered:?}");
        assert!(rendered.iter().all(|line| line.starts_with("> ")));
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
    fn renders_lists_and_styles_headings_without_markdown_markers() {
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
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
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

    #[test]
    fn renders_loose_table_with_blank_lines() {
        let md = "Here are my skills:\n\n| Skill | Purpose |\n| --- | --- |\n\n| agents-sdk | Build AI agents |\n| clockify | Time tracking |\n";
        let lines = render_markdown(md, 80, false, false);
        let text = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(text.contains("agents-sdk"));
        assert!(text.contains("clockify"));
        assert!(text.contains("┌") || text.contains("Skill"));
    }

    #[test]
    fn markdown_fenced_table_renders_as_a_table() {
        let md = "```markdown\n| Tool | Purpose |\n| --- | --- |\n| grep | Search files |\n```";
        let rendered = render_markdown(md, 80, false, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered
            .iter()
            .any(|line| line.contains("Tool") && line.contains("Purpose")));
        assert!(rendered.iter().any(|line| line.contains("grep")));
        assert!(rendered.iter().all(|line| !line.contains("```")));
    }

    #[test]
    fn unicode_dash_table_separators_render_as_tables() {
        let md = concat!(
            "| Category | Name | Purpose |\n",
            "|––––––|——|———————|\n",
            "| Skill | clockify | Manage Clockify time entries |"
        );
        let rendered = render_markdown(md, 80, false, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| {
            line.contains("Category") && line.contains("Name") && line.contains("Purpose")
        }));
        assert!(rendered.iter().any(|line| line.contains("clockify")));
        assert!(rendered.iter().all(|line| !line.contains("| Category |")));
    }

    #[test]
    fn longer_blockquoted_markdown_fence_unwraps_a_table() {
        let md =
            "> ````markdown\n> | Tool | Purpose |\n> | --- | --- |\n> | grep | Search |\n> ````";
        let normalized = super::unwrap_markdown_table_fences(md);

        assert!(!normalized.contains("````"));
        assert!(normalized.contains("> | Tool | Purpose |"));
    }

    #[test]
    fn renders_loose_bullet_lists_without_intermediate_gaps() {
        let md = "Shell & System\n\n• run_command — run any shell command\n\n• get_time — get current date/time\n\n• manage_task — manage background tasks\n";
        let lines = render_markdown(md, 80, false, false);
        let non_empty: Vec<_> = lines.iter().filter(|l| !l.spans.is_empty()).collect();
        // 1 heading + 3 bullet lines = 4 non-empty lines
        assert_eq!(non_empty.len(), 4);
        // Ensure all 3 bullet lines have bullet marker
        let bullet_count = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.contains('•')))
            .count();
        assert_eq!(bullet_count, 3);
    }

    #[test]
    fn renders_mixed_spaced_bullet_lists_without_intermediate_gaps() {
        let md = "File System Operations\n\n• Read/write/edit files\n• Create directories\n\n\n• Move, copy, delete files\n• List directory contents\n";
        let lines = render_markdown(md, 80, false, false);
        let non_empty: Vec<_> = lines.iter().filter(|l| !l.spans.is_empty()).collect();
        // 1 heading + 4 bullet lines = 5 non-empty lines
        assert_eq!(non_empty.len(), 5);
        let bullet_count = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content.contains('•')))
            .count();
        assert_eq!(bullet_count, 4);
    }
}
