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
                use unicode_width::UnicodeWidthChar;
                for ch in word.chars() {
                    let ch_width = ch.width().unwrap_or(1);
                    if current_width + ch_width > width && current_width > 0 {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                    }
                    current.push(Span::styled(ch.to_string(), style));
                    current_width += ch_width;
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
            Event::SoftBreak | Event::HardBreak => {
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
                // Buffer table rows for aligned rendering with header divider
                let mut table: Vec<Vec<String>> = Vec::new();
                let mut current_row: Vec<String> = Vec::new();
                let mut current_cell = String::new();
                let mut in_table = true;
                // Drain table events inline — pulldown already gives us cells
                // Instead, collect by continuing outer loop but intercept
                // For simplicity, handle alignment on row flush via paragraph buffering:
                // Use col widths computed from buffered table
                // Fallback: keep simple but add header divider on TableHead end
                let _ = (&mut table, &mut current_row, &mut current_cell, &mut in_table);
            }
            Event::End(TagEnd::Table) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                lines.push(Line::from(""));
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
                // Header divider: replicate last header row width with ─ and ┼
                if let Some(last) = lines.last() {
                    let text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
                    if text.contains('│') {
                        let cols: Vec<&str> = text.split(" │ ").collect();
                        let divider = cols.iter().map(|c| "─".repeat(c.width().max(3))).collect::<Vec<_>>().join("─┼─");
                        lines.push(Line::from(Span::styled(divider, get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker))));
                    }
                }
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                flush(&mut lines, &mut paragraph, quote_depth, list_depth);
            }
            Event::Start(Tag::TableCell) => {
                if !paragraph.is_empty() {
                    paragraph.push(Span::styled(
                        " │ ",
                        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
                    ));
                }
            }
            Event::End(TagEnd::TableCell) => {}
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
        let text: String = lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("Header 1 │ Header 2"));
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
