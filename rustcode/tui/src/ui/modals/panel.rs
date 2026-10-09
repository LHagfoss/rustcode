//! Shared label/value row helper for the slash-command info panels
//! (`/status`, `/stats`+`/usage`, `/session`, `/context`).
//!
//! The panels previously built every row with hand-rolled `format!` strings,
//! so marked-up values (strong emphasis, inline code) rendered as raw text.
//! These helpers parse `` `code` `` and `**strong**` spans out of values and
//! style them through the existing theme/highlight path: emphasis colors
//! mirror `markdown::text_style` (strong → primary) and inline code is
//! rendered with `highlight::highlight_code_line`, whose syntect theme is
//! built from the active palette. Panel rows keep their exact text and
//! column alignment; only the styling is themed.
//!
//! [`render_panel_content`] applies the same row format to the scrollable
//! command panels, whose content is still a `String` (#1588).

use super::*;
use crate::ui::categorical::{self, CATEGORY_COUNT};
use crate::ui::highlight::highlight_code_line;

/// How strongly a panel value should stand out.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelEmphasis {
    /// Plain value in the panel's base style.
    #[default]
    Normal,
    /// Bold value in the theme primary color (matches markdown `**strong**`).
    Strong,
    /// Bold value in the theme tip color, marking an over-threshold category.
    OverThreshold,
    /// Inline code rendered through the highlight path.
    Code,
}

/// Context-window share (percent) above which a `/context` category counts as
/// over-threshold and gets [`PanelEmphasis::OverThreshold`].
pub const OVER_THRESHOLD_PCT: f64 = 20.0;

/// Overall usage (percent) above which the `/context` summary line counts as
/// high and gets [`PanelEmphasis::Strong`].
pub const HIGH_USAGE_PCT: f64 = 80.0;

/// Map a context-window share to its row emphasis.
pub fn emphasis_for_share(pct: f64) -> PanelEmphasis {
    if pct >= OVER_THRESHOLD_PCT {
        PanelEmphasis::OverThreshold
    } else {
        PanelEmphasis::Normal
    }
}

/// Theme-derived colors for the `/context` usage categories in legend order
/// (user, agent, tool calls, system prompt, system tools, skills, subagents)
/// plus the free-space color at index [`CATEGORY_COUNT`].
///
/// Every entry comes from the active palette, but not by reading one token per
/// role: shipped palettes expose fewer distinct hues than the panel needs, so
/// `categorical::ramp` derives a ramp that keeps all eight roles separated
/// (see `ui::categorical`). Filled and empty blocks are therefore told apart by
/// colour as well as by the `●`/`□` glyph.
pub fn context_category_colors() -> [Color; CATEGORY_COUNT + 1] {
    categorical::ramp()
}

/// Render one `label  value` row. `label` carries its own trailing padding so
/// each panel keeps its existing column alignment; the value supports inline
/// `` `code` `` / `**strong**` markup plus whole-value `emphasis`.
pub fn panel_line(label: &str, value: &str, emphasis: PanelEmphasis) -> Line<'static> {
    let mut spans = vec![Span::styled(
        label.to_owned(),
        Style::default().fg(COLOR_TEXT()),
    )];
    spans.extend(panel_value_spans(
        value,
        emphasis,
        Style::default().fg(COLOR_TEXT()),
    ));
    Line::from(spans)
}

/// Build the value spans for a panel row with an explicit base style, so rows
/// with their own label/icon prefix (like the `/context` categories) can
/// share the markup path. `base` needs no background: the panel paragraph
/// already paints `COLOR_PANEL()`.
pub fn panel_value_spans(value: &str, emphasis: PanelEmphasis, base: Style) -> Vec<Span<'static>> {
    match emphasis {
        PanelEmphasis::Strong => vec![Span::styled(
            value.to_owned(),
            Style::default()
                .fg(COLOR_PRIMARY())
                .add_modifier(Modifier::BOLD),
        )],
        PanelEmphasis::OverThreshold => vec![Span::styled(
            value.to_owned(),
            Style::default()
                .fg(COLOR_TIP())
                .add_modifier(Modifier::BOLD),
        )],
        PanelEmphasis::Code => code_spans(value),
        PanelEmphasis::Normal => inline_markup_spans(value, base),
    }
}

/// Inline code through the highlight path. The syntect theme is built from
/// the active palette, and the background is patched to the panel surface so
/// the row stays opaque like the rest of the panel.
fn code_spans(value: &str) -> Vec<Span<'static>> {
    let spans: Vec<Span<'static>> = highlight_code_line(value, "", false)
        .into_iter()
        .map(|span| Span::styled(span.content, span.style.bg(COLOR_PANEL())))
        .collect();
    if spans.is_empty() {
        vec![Span::styled(
            value.to_owned(),
            Style::default().fg(COLOR_TEXT()).bg(COLOR_PANEL()),
        )]
    } else {
        spans
    }
}

/// Split a value into plain, `**strong**` and `` `code` `` spans. Markers
/// without a closing partner stay literal so values like `a ** b` or an
/// unmatched backtick render exactly as written.
fn inline_markup_spans(value: &str, base: Style) -> Vec<Span<'static>> {
    let strong_style = Style::default()
        .fg(COLOR_PRIMARY())
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut rest = value;
    loop {
        let tick = rest.find('`');
        let strong = rest.find("**");
        let pos = match (tick, strong) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(pos) = pos else {
            if !rest.is_empty() {
                spans.push(Span::styled(rest.to_owned(), base));
            }
            break;
        };
        if pos > 0 {
            spans.push(Span::styled(rest[..pos].to_owned(), base));
            rest = &rest[pos..];
        }
        if rest.starts_with("**") {
            let inner = &rest[2..];
            match inner.find("**") {
                Some(end) => {
                    spans.push(Span::styled(inner[..end].to_owned(), strong_style));
                    rest = &inner[end + 2..];
                }
                None => {
                    spans.push(Span::styled("**".to_owned(), base));
                    rest = inner;
                }
            }
        } else {
            let inner = &rest[1..];
            match inner.find('`') {
                Some(end) => {
                    if inner[..end].is_empty() {
                        spans.push(Span::styled(String::new(), base));
                    } else {
                        spans.extend(code_spans(&inner[..end]));
                    }
                    rest = &inner[end + 1..];
                }
                None => {
                    spans.push(Span::styled("`".to_owned(), base));
                    rest = inner;
                }
            }
        }
        if rest.is_empty() {
            break;
        }
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_rows_keep_their_cells_and_explicit_colours() {
        let content = "Scan this code:\n\n\u{2060}    \n\u{2060} ▀▄█\n\u{2060}    \n\naddress  10.0.0.2:17879\n";
        let lines = render_panel_content(content, 60);
        let code: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.style.bg == Some(Color::Indexed(231)))
            })
            .collect();
        let text: Vec<String> = code
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        // Blank quiet-zone rows survive, nothing is reflowed or trimmed.
        assert_eq!(text, ["    ", " ▀▄█", "    "]);
        for line in code {
            assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(16)));
            assert_eq!(line.spans[0].style.bg, Some(Color::Indexed(231)));
        }

        let narrow = render_panel_content(content, 3);
        let narrow: String = narrow
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect();
        assert!(narrow.contains("needs 4 columns"), "{narrow}");
        assert!(!narrow.contains('▀'));
    }

    use super::*;
    use crate::ui::tests::THEME_TEST_LOCK;

    fn plain_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn panel_line_preserves_label_padding_and_text() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let line = panel_line("Model       ", "mock-model", PanelEmphasis::Normal);
        assert_eq!(plain_text(&line), "Model       mock-model");
        assert_eq!(line.spans[0].style.fg, Some(COLOR_TEXT()));
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn strong_and_threshold_emphasis_use_theme_colors() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let strong = panel_line("L ", "v", PanelEmphasis::Strong);
        assert_eq!(strong.spans[1].style.fg, Some(COLOR_PRIMARY()));
        assert!(strong.spans[1].style.add_modifier.contains(Modifier::BOLD));
        let warn = panel_line("L ", "v", PanelEmphasis::OverThreshold);
        assert_eq!(warn.spans[1].style.fg, Some(COLOR_TIP()));
        assert!(warn.spans[1].style.add_modifier.contains(Modifier::BOLD));
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn code_values_route_through_highlight_path_with_panel_bg() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let line = panel_line("L ", "cargo test", PanelEmphasis::Code);
        assert_eq!(plain_text(&line), "L cargo test");
        assert!(line.spans.len() > 1, "code spans: {line:?}");
        for span in line.spans.iter().skip(1) {
            assert_eq!(span.style.bg, Some(COLOR_PANEL()), "span: {span:?}");
        }
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn inline_markup_parses_code_and_strong() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let line = panel_line(
            "L ",
            "run `cargo test` for **fast** checks",
            PanelEmphasis::Normal,
        );
        assert_eq!(plain_text(&line), "L run cargo test for fast checks");
        assert!(
            line.spans
                .iter()
                .any(|s| s.content == "**fast**".replace("**", "")
                    && s.style.fg == Some(COLOR_PRIMARY())
                    && s.style.add_modifier.contains(Modifier::BOLD)),
            "strong span missing: {line:?}"
        );
        for span in &line.spans {
            assert!(
                span.style.bg.is_none() || span.style.bg == Some(COLOR_PANEL()),
                "panel bg leak: {span:?}"
            );
        }
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn unclosed_markers_stay_literal() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let line = panel_line("L ", "a ** b `c", PanelEmphasis::Normal);
        assert_eq!(plain_text(&line), "L a ** b `c");
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn emphasis_for_share_respects_threshold() {
        assert_eq!(emphasis_for_share(19.9), PanelEmphasis::Normal);
        assert_eq!(emphasis_for_share(20.0), PanelEmphasis::OverThreshold);
        assert_eq!(emphasis_for_share(85.5), PanelEmphasis::OverThreshold);
    }

    #[test]
    fn category_colors_come_from_the_theme_ramp() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        for theme in ["default", "light", "nord", "dracula", "tokyo-night", "sky"] {
            crate::ui::theme::set_active_theme(theme);
            assert_eq!(
                context_category_colors(),
                categorical::ramp(),
                "theme {theme} must go through the categorical ramp, not raw tokens"
            );
            assert_eq!(
                context_category_colors().len(),
                CATEGORY_COUNT + 1,
                "seven categories plus the free block"
            );
        }
        crate::ui::theme::set_active_theme("default");
    }

    /// The crux of #1588: the label column has to survive rendering. Markdown
    /// reflows the rows into one wrapped paragraph, so this asserts the column
    /// directly instead of trusting the row format.
    #[test]
    fn command_panel_rows_keep_the_label_column_aligned() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let content = "Keys\n  short         one\n  a-much-longer-label    two\n  mid    three\n";
        let lines = render_panel_content(content, 60);
        let rows: Vec<String> = lines.iter().map(plain_text).collect();

        assert_eq!(
            rows,
            vec![
                "Keys".to_owned(),
                "  short                one".to_owned(),
                "  a-much-longer-label  two".to_owned(),
                "  mid                  three".to_owned(),
            ],
            "every row must start its value on the same cell"
        );
        let columns: Vec<usize> = rows[1..]
            .iter()
            .zip(["one", "two", "three"])
            .map(|(row, value)| row.find(value).expect("value column"))
            .collect();
        assert!(
            columns.windows(2).all(|pair| pair[0] == pair[1]),
            "values must start on one column, got {columns:?} in {rows:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }

    /// What the Markdown path the panels used before does to the same rows: it
    /// merges them into one reflowed paragraph, so the row structure, the
    /// indent and the value column are all gone (#1588).
    #[test]
    fn markdown_reflows_the_rows_into_one_paragraph() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let content = "  short    one\n  longer-label   two\n  mid    three\n";
        let collapsed = crate::ui::markdown::render_markdown(content, 28, false, false);
        let rows: Vec<String> = collapsed
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(
            rows.len() < 3,
            "markdown merges the source lines, so a table stops being rows: {rows:?}"
        );
        assert!(
            rows.iter()
                .skip(1)
                .any(|row| row.starts_with(|c: char| c != ' ')),
            "a wrapped continuation starts at the panel edge, not the value column: {rows:?}"
        );

        // The same content through the panel renderer keeps one row per source
        // line, indented, with its value on the shared column.
        let rendered: Vec<String> = render_panel_content(content, 28)
            .iter()
            .map(plain_text)
            .collect();
        assert!(
            rendered.iter().all(|row| row.starts_with("  ")),
            "rows keep their indent: {rendered:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn command_panel_rows_mark_over_threshold_percentages() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let lines = render_panel_content("  small    12.5%\n  large   85.5%\n", 60);

        assert_eq!(panel_value_emphasis("12.5%"), PanelEmphasis::Normal);
        assert_eq!(panel_value_emphasis("85.5%"), PanelEmphasis::OverThreshold);
        assert_eq!(
            panel_value_emphasis("85.5% of the window"),
            PanelEmphasis::OverThreshold
        );
        assert_eq!(panel_value_emphasis("model gpt-5"), PanelEmphasis::Normal);
        assert_eq!(panel_value_emphasis("2x faster"), PanelEmphasis::Normal);

        let colors: Vec<Option<Color>> = lines[0]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.style.fg)
            .collect();
        assert_eq!(colors[0], Some(COLOR_TEXT()), "under threshold stays plain");
        assert!(
            lines[1]
                .spans
                .iter()
                .skip(1)
                .any(|span| span.style.fg == Some(COLOR_TIP())),
            "over-threshold share must be marked: {:?}",
            lines[1]
        );
        crate::ui::theme::set_active_theme("default");
    }

    /// `/about` writes its labels with a trailing colon, and its bullets with
    /// `•`; both belong in the same column as everything else (#1588).
    #[test]
    fn colon_labels_and_bullets_are_rows() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let content = "• Version:      v0.56.2\n• Repository:   https://example.test/repo\n";
        let rows: Vec<String> = render_panel_content(content, 60)
            .iter()
            .map(plain_text)
            .collect();

        assert_eq!(
            rows,
            vec![
                "• Version:     v0.56.2".to_owned(),
                "• Repository:  https://example.test/repo".to_owned(),
            ],
            "colon labels share one column: {rows:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn a_single_double_space_line_stays_prose() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let lines = render_panel_content("Nothing to see here.  Move on.\n", 60);

        assert!(
            plain_text(&lines[0]).contains("Move on."),
            "a lone double space is prose, not a table: {:?}",
            lines
        );
        assert!(
            split_panel_row("Done.  Ready.").is_none(),
            "a label ending a sentence is prose"
        );
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn markdown_structures_are_left_to_the_markdown_renderer() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        for line in [
            "# Heading",
            "> quoted",
            "| a | b |",
            "```rust",
            "- bullet   item",
            "* bullet   item",
            "1. ordered   item",
        ] {
            assert!(
                split_panel_row(line).is_none(),
                "{line:?} is Markdown, not a label/value row"
            );
        }
        // A one-character label is a row: the shortcuts block names the help key
        // `?`, and it belongs to the same column as every other shortcut.
        assert!(split_panel_row("  ?                Show help").is_some());
        crate::ui::theme::set_active_theme("default");
    }

    /// A label too wide for the frame is truncated in place: past half the
    /// frame the value would have no room and the wrap would break the row into
    /// one-character fragments.
    #[test]
    fn a_label_too_wide_for_the_frame_is_truncated() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let content = "  short  one\n  a-much-longer-label  two\n";
        let rows: Vec<String> = render_panel_content(content, 20)
            .iter()
            .map(plain_text)
            .collect();

        assert_eq!(
            rows.len(),
            2,
            "a narrow frame must not fragment rows: {rows:?}"
        );
        // Cell, not byte: a truncated label ends in a multi-byte ellipsis.
        let column = |row: &str, value: &str| row.chars().count() - value.chars().count();
        assert_eq!(
            column(&rows[1], "two"),
            column(&rows[0], "one"),
            "both values stay on one column: {rows:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }

    #[test]
    fn a_wrapped_value_stays_under_the_value_column() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        crate::ui::theme::set_active_theme("default");
        let content =
            "  key    a description long enough that it has to wrap somewhere\n  other  short\n";
        let lines = render_panel_content(content, 40);
        let rows: Vec<String> = lines.iter().map(plain_text).collect();

        let first_value = rows[0].find("a description").expect("value column");
        let continuation = rows[1]
            .find(|character: char| character != ' ')
            .expect("wrap");
        assert_eq!(
            first_value, continuation,
            "a wrapped value must continue under the value column: {rows:?}"
        );
        crate::ui::theme::set_active_theme("default");
    }
}

/// Cells between a command panel's label column and its value column.
///
/// The panel owns the padding, so the value column lands on the same cell for
/// every row no matter how the content spaced its own columns.
const PANEL_COLUMN_GAP: usize = 2;

/// Widest label a panel row may claim before its value is pushed out of view.
const MAX_LABEL_WIDTH: usize = 28;

/// Shortest run of consecutive label/value rows treated as a table. One stray
/// double space in a sentence is not a table, so a single row falls back to
/// Markdown instead of claiming a label column of its own.
const MIN_TABLE_ROWS: usize = 2;

/// Rows a command panel claims above the composer. Bound to the rows the modal
/// layer reserves, so a panel never paints over the transcript (#1588).
pub(in crate::ui) const COMMAND_PANEL_HEIGHT: u16 = 18;

/// Remote pairing needs the entire viewport for its QR code and quiet zone.
/// Other command output keeps its bounded position above the composer.
pub(in crate::ui) fn command_panel_area(
    f: &Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    if state
        .command_panel()
        .is_some_and(|panel| panel.title == "Remote")
    {
        f.area()
    } else {
        input_anchor_rect(f, input_area, COMMAND_PANEL_HEIGHT)
    }
}

/// One `label<gap>value` row recovered from a raw panel line.
struct PanelRow<'a> {
    /// Leading whitespace, kept so an indented table keeps its indent.
    indent: &'a str,
    label: &'a str,
    value: &'a str,
    /// The source line, so a run too short to be a table can fall back to
    /// Markdown with its own spacing intact.
    raw: &'a str,
}

/// Start of the first whitespace run that separates a label from a value: two
/// or more spaces, or a single tab, with a value behind it.
///
/// Two spaces are the delimiter because Markdown treats them as ordinary prose
/// whitespace, so an author writing an aligned table needs no new syntax. The
/// scan is over bytes, which is safe here because ASCII whitespace bytes never
/// occur inside a multi-byte UTF-8 sequence.
fn panel_column_gap(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b' ' && bytes[index] != b'\t' {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
            index += 1;
        }
        if index < bytes.len() && (index - run_start >= 2 || bytes[run_start] == b'\t') {
            return Some(run_start);
        }
    }
    None
}

/// Split a raw panel line into its label and value columns, or [`None`] when the
/// line is prose or a Markdown block.
///
/// Block markers stay with the Markdown renderer: a leading `#`, `>`, `|`,
/// fence, bullet or ordered marker is prose or a list, not a table row.
fn split_panel_row(line: &str) -> Option<PanelRow<'_>> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let body = trimmed.trim_end();
    if body.is_empty() {
        return None;
    }
    let first = body.chars().next()?;
    if matches!(first, '#' | '>' | '|' | '`' | '~' | '-' | '+' | '*') {
        return None;
    }
    if body.starts_with(|character: char| character.is_ascii_digit())
        && body
            .find(|character: char| !character.is_ascii_digit())
            .is_some_and(|index| body[index..].starts_with(". "))
    {
        return None;
    }

    let separator = panel_column_gap(body)?;
    let label = &body[..separator];
    let value = body[separator..].trim_start();
    // A multi-character label that ends a sentence is prose that happened to
    // carry two spaces, not a table row, and must keep its Markdown rendering.
    // A trailing colon is allowed, because that is how `/about` writes its
    // labels, and a one-character label stays a row: `?` is how the shortcuts
    // block names the help key.
    if label.is_empty()
        || value.is_empty()
        || (label.chars().count() > 1 && label.ends_with(['.', ',', ';', '!', '?']))
        || label.width() > MAX_LABEL_WIDTH
    {
        return None;
    }
    Some(PanelRow {
        indent,
        label,
        value,
        raw: line,
    })
}

/// Render a scrollable command panel's content.
///
/// Label/value rows bypass Markdown and render through [`panel_line`], so the
/// label column survives. Markdown cannot carry a column: it reflows a
/// paragraph, so consecutive source lines merge into one wrapped line, the
/// indent is dropped, and a wrap restarts at the panel edge instead of the
/// value column (#1588). Everything else still goes through `render_markdown`,
/// so prose, lists, tables and fenced code keep their formatting, and a panel
/// that mixes the two gets both.
///
/// Percentage values take their emphasis from [`emphasis_for_share`], so an
/// over-threshold share in any command panel is marked like `/context`.
pub(in crate::ui) fn render_panel_content(content: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut prose = String::new();
    let mut rows: Vec<PanelRow<'_>> = Vec::new();
    let mut code: Vec<&str> = Vec::new();
    for line in content.lines() {
        if let Some(row) = line.strip_prefix(QR_ROW_MARK) {
            flush_panel_prose(&mut lines, &mut prose, width);
            flush_panel_rows(&mut lines, &mut rows, width);
            code.push(row);
            continue;
        }
        flush_qr_rows(&mut lines, &mut code, width);
        match split_panel_row(line) {
            Some(row) => {
                flush_panel_prose(&mut lines, &mut prose, width);
                rows.push(row);
            }
            None => {
                flush_panel_rows(&mut lines, &mut rows, width);
                prose.push_str(line);
                prose.push('\n');
            }
        }
    }
    flush_panel_prose(&mut lines, &mut prose, width);
    flush_panel_rows(&mut lines, &mut rows, width);
    flush_qr_rows(&mut lines, &mut code, width);
    lines
}

/// Prefix of a QR code row in panel text: the zero-width mark the engine's
/// pairing text puts in front of each row of half blocks.
const QR_ROW_MARK: char = '\u{2060}';

/// Render a run of QR code rows. A scanner needs dark modules on a light
/// ground with an intact quiet zone, and no theme guarantees either, so the
/// rows are painted black on white from the fixed 256-colour cube and are
/// never wrapped or reflowed. A panel too narrow for the code says so instead
/// of drawing something that cannot be scanned.
fn flush_qr_rows(lines: &mut Vec<Line<'static>>, code: &mut Vec<&str>, width: usize) {
    let Some(columns) = code.iter().map(|row| row.width()).max() else {
        return;
    };
    if columns > width {
        lines.push(Line::from(Span::styled(
            format!(
                "(The QR code needs {columns} columns. Widen the terminal, or pair with the address and code.)"
            ),
            Style::default().fg(COLOR_TEXT()),
        )));
    } else {
        let style = Style::default()
            .fg(Color::Indexed(16))
            .bg(Color::Indexed(231));
        lines.extend(
            code.iter()
                .map(|row| Line::from(Span::styled((*row).to_owned(), style))),
        );
    }
    code.clear();
}

fn flush_panel_prose(lines: &mut Vec<Line<'static>>, prose: &mut String, width: usize) {
    if prose.trim().is_empty() {
        prose.clear();
        return;
    }
    lines.extend(crate::ui::markdown::render_markdown(
        prose.trim_end_matches('\n'),
        width,
        false,
        true,
    ));
    prose.clear();
}

fn flush_panel_rows(lines: &mut Vec<Line<'static>>, rows: &mut Vec<PanelRow<'_>>, width: usize) {
    if rows.len() < MIN_TABLE_ROWS {
        // Too short to be a table: hand the line back to Markdown unchanged
        // rather than inventing a column for it.
        for row in rows.drain(..) {
            lines.extend(crate::ui::markdown::render_markdown(
                row.raw, width, false, true,
            ));
        }
        return;
    }

    // One column for the whole run, measured from the widest label including
    // its indent, so every value starts on the same cell.
    let column = rows
        .iter()
        .map(|row| row.indent.width() + row.label.width())
        .max()
        .unwrap_or(0)
        .min(MAX_LABEL_WIDTH)
        + PANEL_COLUMN_GAP;
    // A label column may claim at most half the frame. Past that the value has
    // no room left, and the wrap breaks the row into one-character fragments.
    let column = column.min((width / 2).max(1));
    let continuation = Span::styled(" ".repeat(column), Style::default().fg(COLOR_TEXT()));
    for row in rows.drain(..) {
        let line = panel_line(
            &panel_label_column(row.indent, row.label, column),
            row.value,
            panel_value_emphasis(row.value),
        );
        // A value wider than the frame wraps under the value column instead of
        // back at the panel edge, so a long description stays readable.
        crate::ui::markdown::push_wrapped_with_continuation(
            lines,
            line.spans,
            width,
            Some(continuation.clone()),
        );
    }
}

/// Indent plus a label padded to `column`, truncated when the label alone
/// would push the value off the frame.
fn panel_label_column(indent: &str, label: &str, column: usize) -> String {
    let budget = column.saturating_sub(indent.width());
    let label = truncate_to_width(label, budget);
    format!(
        "{indent}{label}{}",
        " ".repeat(budget.saturating_sub(label.width()))
    )
}

/// Emphasis for a command-panel value.
///
/// A leading `NN%` is a share of some budget, so it takes the same threshold
/// marking `/context` uses instead of rendering as plain text (#1588). A panel
/// that reports a *remaining* percentage stays out of the row format on
/// purpose: the threshold runs the other way there, as `/quota` does.
fn panel_value_emphasis(value: &str) -> PanelEmphasis {
    match percentage_prefix(value) {
        Some(share) => emphasis_for_share(share),
        None => PanelEmphasis::Normal,
    }
}

/// Parse a leading `NN%` value, with or without decimals.
fn percentage_prefix(value: &str) -> Option<f64> {
    value
        .split_whitespace()
        .next()?
        .strip_suffix('%')?
        .parse()
        .ok()
}

/// Scrollable command output, with a full-height surface for remote pairing.
///
/// The panel is an output surface, not a picker: it has no rows to activate, so
/// it carries no selection marker and no selection state, and the footer below
/// is the whole of its affordance (#1588).
pub(in crate::ui) fn render_command_panel(
    f: &mut Frame,
    state: &RenderSnapshot,
    input_area: ratatui::layout::Rect,
) {
    let Some(panel) = state.command_panel() else {
        return;
    };
    let area = command_panel_area(f, state, input_area);
    let inner = render_padded_panel(f, area).inner(Margin {
        vertical: 0,
        horizontal: 2,
    });
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(inner);
    f.render_widget(
        Paragraph::new(Line::styled(
            panel.title,
            Style::default()
                .fg(COLOR_TEXT())
                .add_modifier(Modifier::BOLD),
        )),
        chunks[0],
    );
    let mut lines = render_panel_content(&panel.content, inner.width as usize);
    // Use the panel as the fallback; QR spans keep their explicit white ground.
    let background = Style::default().bg(COLOR_PANEL());
    for line in &mut lines {
        line.style = background.patch(line.style);
        for span in &mut line.spans {
            span.style = background.patch(span.style);
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.modal_scroll_row(), 0)),
        chunks[2],
    );
    f.render_widget(
        Paragraph::new("↑/↓ scroll · enter / esc close").style(Style::default().fg(COLOR_MUTED())),
        chunks[3],
    );
}
