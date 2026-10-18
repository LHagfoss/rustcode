//! Native rendering for agent-authored `rustcode-viz` fenced blocks.
//!
//! The canonical assistant text keeps the fence unchanged; this module only
//! decides how a complete, recognized block is drawn. Anything it does not
//! accept (bad JSON, unknown version or type, oversized payloads, missing
//! fields) is returned as an error so the caller renders the ordinary source
//! with a short note. See `docs/visualizations.md` for the format.

use super::*;
use serde_json::Value;

/// Reserved fence info string. Matched exactly (after trimming).
pub(super) const VIZ_INFO_STRING: &str = "rustcode-viz";
/// The only schema version this renderer understands.
pub(super) const VIZ_VERSION: u64 = 1;

/// Largest accepted JSON payload. Larger blocks fall back to source.
pub(super) const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
pub(super) const MAX_TITLE_CHARS: usize = 80;
pub(super) const MAX_SUMMARY_CHARS: usize = 300;
pub(super) const MAX_LABEL_CHARS: usize = 40;
pub(super) const MAX_CELL_CHARS: usize = 60;
pub(super) const MAX_BAR_ITEMS: usize = 30;
pub(super) const MAX_TABLE_ROWS: usize = 50;
pub(super) const MAX_TABLE_COLUMNS: usize = 8;
pub(super) const MAX_TREE_NODES: usize = 120;
pub(super) const MAX_TREE_DEPTH: usize = 8;

/// Terminal-only view state for one widget: the sort a click chose for a
/// table, or the tree nodes whose open state was flipped. It lives in
/// [`super::TranscriptState`] keyed by `(history index, viz ordinal)` and is
/// never persisted, sent to a provider, or written into the message text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct VizView {
    /// [`source_hash`] of the fence body this state was made for. A message
    /// whose body changed under the same key ignores stale state.
    pub(crate) source: u64,
    /// `(column, descending)` picked by clicking a header. Overrides the
    /// payload's `sort_by`.
    pub(crate) sort: Option<(usize, bool)>,
    /// Preorder ids of tree nodes whose open state differs from the default
    /// (every node starts open).
    pub(crate) toggled: std::collections::BTreeSet<usize>,
}

impl VizView {
    /// Apply a click to this state.
    pub(crate) fn apply(&mut self, action: VizAction) {
        match action {
            VizAction::Sort { column, descending } => self.sort = Some((column, descending)),
            VizAction::ToggleNode(id) => {
                if !self.toggled.remove(&id) {
                    self.toggled.insert(id);
                }
            }
        }
    }
}

/// What a click on a widget row does. The renderer resolves the target state
/// (for a header, the direction the next click picks), so applying it never
/// needs the payload again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum VizAction {
    Sort { column: usize, descending: bool },
    ToggleNode(usize),
}

/// A clickable span of one rendered message row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VizHit {
    /// Row of the rendered message.
    pub(crate) line: usize,
    /// Display columns of that row.
    pub(crate) columns: std::ops::Range<u16>,
    /// Which `rustcode-viz` fence of the message, counting from 0.
    pub(crate) ordinal: usize,
    /// [`source_hash`] of that fence body.
    pub(crate) source: u64,
    pub(crate) action: VizAction,
}

/// Interactive inputs and outputs of one assistant message render: the view
/// state of its widgets in, the clickable spans out. Only finalized messages
/// get one, so an open fence or a live stream is never interactive.
#[derive(Default)]
pub(crate) struct VizContext {
    pub(crate) views: Vec<(usize, VizView)>,
    pub(crate) hits: Vec<VizHit>,
}

impl VizContext {
    pub(super) fn view(&self, ordinal: usize, source: u64) -> Option<&VizView> {
        self.views
            .iter()
            .find(|(index, view)| *index == ordinal && view.source == source)
            .map(|(_, view)| view)
    }
}

/// Stable fingerprint of a fence body for [`VizView::source`].
pub(crate) fn source_hash(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Clickable spans of a widget, relative to its own rows and columns.
type WidgetHits = Vec<(usize, std::ops::Range<u16>, VizAction)>;

pub(super) fn is_viz_info(info: &str) -> bool {
    info.trim() == VIZ_INFO_STRING
}

/// One-row placeholder drawn while a visualization fence is still open in the
/// live stream. The body is not parsed until the closing fence arrives.
pub(super) fn streaming_placeholder(body_lines: usize, show_picker: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "◇ ",
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            format!(
                "Visualization: waiting for the closing fence ({body_lines} line{})",
                if body_lines == 1 { "" } else { "s" }
            ),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
        ),
    ])
}

/// Muted note shown above the source when a block is not rendered.
pub(super) fn fallback_note(reason: &str, show_picker: bool) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("◇ Visualization not rendered ({reason}); showing source"),
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
    )])
}

/// Render a complete `rustcode-viz` body to lines no wider than `width`.
/// `Err` carries a short reason for the source fallback note.
#[cfg(test)]
pub(super) fn render_visualization(
    source: &str,
    width: usize,
    show_picker: bool,
) -> Result<Vec<Line<'static>>, String> {
    render_visualization_with_view(source, width, show_picker, None).map(|(lines, _)| lines)
}

/// [`render_visualization`] under a widget's view state, also returning the
/// clickable spans of the rendered rows.
pub(super) fn render_visualization_with_view(
    source: &str,
    width: usize,
    show_picker: bool,
    view: Option<&VizView>,
) -> Result<(Vec<Line<'static>>, WidgetHits), String> {
    if source.trim().is_empty() {
        return Err("empty block".into());
    }
    if source.len() > MAX_PAYLOAD_BYTES {
        return Err(format!(
            "payload is {} KiB, limit is {} KiB",
            source.len().div_ceil(1024),
            MAX_PAYLOAD_BYTES / 1024
        ));
    }
    let value: Value = serde_json::from_str(source).map_err(|_| "invalid JSON".to_owned())?;
    let object = value.as_object().ok_or("payload is not a JSON object")?;
    match object.get("version").and_then(Value::as_u64) {
        Some(VIZ_VERSION) => {}
        Some(other) => return Err(format!("unsupported version {other}")),
        None => return Err("missing \"version\"".into()),
    }
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or("missing \"type\"")?;
    let title = optional_text(object, "title")?;
    let summary = optional_text(object, "summary")?;

    let width = width.max(10);
    let (body, hits) = match kind {
        "bar_chart" => (render_bar_chart(object, width, show_picker)?, Vec::new()),
        "table" => render_table(object, width, show_picker, view)?,
        "tree" => render_tree(object, width, show_picker, view)?,
        other => {
            return Err(format!(
                "unsupported type \"{}\"",
                truncate_chars(other, MAX_LABEL_CHARS)
            ));
        }
    };

    let mut lines = Vec::with_capacity(body.len() + 3);
    let heading = title.unwrap_or_else(|| kind.replace('_', " "));
    lines.push(Line::from(vec![
        Span::styled(
            "◆ ",
            get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
        Span::styled(
            fit_width(&truncate_chars(&heading, MAX_TITLE_CHARS), width - 2),
            get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::BOLD, show_picker),
        ),
    ]));
    if let Some(summary) = summary {
        let summary = truncate_chars(&summary, MAX_SUMMARY_CHARS);
        lines.extend(wrap_styled_spans(
            vec![Span::styled(
                summary,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            )],
            width,
        ));
    }
    let offset = lines.len();
    lines.extend(body);
    let hits = hits
        .into_iter()
        .map(|(line, columns, action)| (line + offset, columns, action))
        .collect();
    Ok((lines, hits))
}

fn optional_text(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let text = single_line(text);
            Ok((!text.is_empty()).then_some(text))
        }
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

/// Collapse control characters and newlines so model text cannot inject
/// terminal escapes or break the widget layout.
fn single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(character);
        }
    }
    out.trim().to_owned()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Truncate to a display width, marking the cut with an ellipsis.
fn fit_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 1 {
            break;
        }
        used += character_width;
        out.push(character);
    }
    out.push('…');
    out
}

fn pad_right(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(used)))
}

fn pad_left(text: &str, width: usize) -> String {
    let used = UnicodeWidthStr::width(text);
    format!("{}{text}", " ".repeat(width.saturating_sub(used)))
}

fn more_line(text: String, show_picker: bool) -> Line<'static> {
    Line::from(vec![Span::styled(
        text,
        get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
    )])
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn render_bar_chart(
    object: &serde_json::Map<String, Value>,
    width: usize,
    show_picker: bool,
) -> Result<Vec<Line<'static>>, String> {
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .ok_or("bar_chart needs an \"items\" array")?;
    if items.is_empty() {
        return Err("bar_chart has no items".into());
    }
    let unit = optional_text(object, "unit")?
        .map(|unit| truncate_chars(&unit, 12))
        .unwrap_or_default();

    let mut rows = Vec::new();
    for item in items.iter().take(MAX_BAR_ITEMS) {
        let label = item
            .get("label")
            .and_then(Value::as_str)
            .ok_or("every bar_chart item needs a string \"label\"")?;
        let value = item
            .get("value")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or("every bar_chart item needs a finite numeric \"value\"")?;
        rows.push((truncate_chars(&single_line(label), MAX_LABEL_CHARS), value));
    }

    let max_abs = rows
        .iter()
        .map(|(_, value)| value.abs())
        .fold(0.0_f64, f64::max);
    let value_texts: Vec<String> = rows
        .iter()
        .map(|(_, value)| {
            if unit.is_empty() {
                format_number(*value)
            } else {
                format!("{} {unit}", format_number(*value))
            }
        })
        .collect();
    let value_width = value_texts
        .iter()
        .map(|text| UnicodeWidthStr::width(text.as_str()))
        .max()
        .unwrap_or(0);
    let label_width = rows
        .iter()
        .map(|(label, _)| UnicodeWidthStr::width(label.as_str()))
        .max()
        .unwrap_or(0)
        .min(width * 2 / 5)
        .max(1);
    // label, space, bar, space, value
    let bar_width = width.saturating_sub(label_width + value_width + 2);
    let stacked = bar_width < 6;

    let label_style = get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker);
    let value_style = get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker);
    let mut lines = Vec::new();
    for ((label, value), value_text) in rows.iter().zip(value_texts) {
        let color = if *value < 0.0 {
            COLOR_DIFF_REMOVE_FG()
        } else {
            COLOR_PRIMARY()
        };
        let bar_style = get_themed_style(color, COLOR_BG(), Modifier::empty(), show_picker);
        if stacked {
            let bar_width = width.saturating_sub(value_width + 1).max(1);
            lines.push(Line::from(vec![Span::styled(
                fit_width(label, width),
                label_style,
            )]));
            lines.push(Line::from(vec![
                Span::styled(
                    pad_right(&bar(value.abs(), max_abs, bar_width), bar_width),
                    bar_style,
                ),
                Span::styled(" ".to_owned(), value_style),
                Span::styled(fit_width(&value_text, width - bar_width - 1), value_style),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    pad_right(&fit_width(label, label_width), label_width),
                    label_style,
                ),
                Span::styled(" ".to_owned(), label_style),
                Span::styled(
                    pad_right(&bar(value.abs(), max_abs, bar_width), bar_width),
                    bar_style,
                ),
                Span::styled(" ".to_owned(), value_style),
                Span::styled(pad_left(&value_text, value_width), value_style),
            ]));
        }
    }
    if items.len() > MAX_BAR_ITEMS {
        lines.push(more_line(
            format!(
                "… {} more items not shown (limit {MAX_BAR_ITEMS})",
                items.len() - MAX_BAR_ITEMS
            ),
            show_picker,
        ));
    }
    Ok(lines)
}

/// A horizontal bar with eighth-cell precision.
fn bar(value: f64, max: f64, width: usize) -> String {
    const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    if max <= 0.0 || width == 0 {
        return String::new();
    }
    let eighths = ((value / max) * (width * 8) as f64).round() as usize;
    let eighths = eighths.min(width * 8);
    let mut out = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        out.push(PARTIAL[eighths % 8]);
    }
    if out.is_empty() && value > 0.0 {
        out.push('▏');
    }
    out
}

fn cell_text(value: &Value) -> Result<(String, bool), String> {
    match value {
        Value::Null => Ok((String::new(), false)),
        Value::Bool(flag) => Ok((flag.to_string(), false)),
        Value::Number(number) => Ok((
            number
                .as_f64()
                .map(format_number)
                .unwrap_or_else(|| number.to_string()),
            true,
        )),
        Value::String(text) => Ok((truncate_chars(&single_line(text), MAX_CELL_CHARS), false)),
        _ => Err("table cells must be strings, numbers, booleans, or null".into()),
    }
}

/// Escape Markdown punctuation so cell text is shown literally by the table
/// renderer instead of being interpreted as inline markup.
fn escape_markdown_cell(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(character);
    }
    if out.is_empty() { " ".to_owned() } else { out }
}

/// Tables reuse the transcript's Markdown table layout (column fitting,
/// narrow key/value records, theme styles) so a structured table looks the
/// same as a hand-written one, while the payload stays validated, bounded,
/// and optionally sorted. Clicking a header sorts by that column.
fn render_table(
    object: &serde_json::Map<String, Value>,
    width: usize,
    show_picker: bool,
    view: Option<&VizView>,
) -> Result<(Vec<Line<'static>>, WidgetHits), String> {
    let columns = object
        .get("columns")
        .and_then(Value::as_array)
        .ok_or("table needs a \"columns\" array")?;
    if columns.is_empty() {
        return Err("table has no columns".into());
    }
    let rows = object
        .get("rows")
        .and_then(Value::as_array)
        .ok_or("table needs a \"rows\" array")?;

    let shown_columns = columns.len().min(MAX_TABLE_COLUMNS);
    let headers = columns[..shown_columns]
        .iter()
        .map(|column| {
            column
                .as_str()
                .map(|text| truncate_chars(&single_line(text), MAX_LABEL_CHARS))
                .ok_or_else(|| "table columns must be strings".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut parsed_rows = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = row.as_array().ok_or("table rows must be arrays")?;
        let mut parsed = Vec::with_capacity(shown_columns);
        for index in 0..shown_columns {
            parsed.push(cell_text(cells.get(index).unwrap_or(&Value::Null))?);
        }
        let sort_key = cells.iter().map(Value::as_f64).collect::<Vec<_>>();
        parsed_rows.push((parsed, sort_key));
    }

    let payload_sort = match object.get("sort_by") {
        None => None,
        Some(sort) => {
            let column = sort
                .get("column")
                .and_then(Value::as_u64)
                .map(|column| column as usize)
                .filter(|column| *column < shown_columns)
                .ok_or("\"sort_by.column\" must be a shown column index")?;
            let descending = sort
                .get("descending")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some((column, descending))
        }
    };
    // A header click overrides the payload's order. Every row is sorted, so
    // the shown rows are the top of the whole table, not of the first 50.
    let sort = view
        .and_then(|view| view.sort)
        .filter(|(column, _)| *column < shown_columns)
        .or(payload_sort);
    if let Some((column, descending)) = sort {
        parsed_rows.sort_by(|(left, left_keys), (right, right_keys)| {
            let ordering = match (
                left_keys.get(column).copied().flatten(),
                right_keys.get(column).copied().flatten(),
            ) {
                (Some(left), Some(right)) => left.total_cmp(&right),
                _ => left[column].0.cmp(&right[column].0),
            };
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    let numeric_columns: Vec<bool> = (0..shown_columns)
        .map(|column| {
            !parsed_rows.is_empty()
                && parsed_rows
                    .iter()
                    .all(|(cells, _)| cells[column].1 || cells[column].0.is_empty())
        })
        .collect();

    let mut markdown = String::new();
    markdown.push('|');
    for (column, header) in headers.iter().enumerate() {
        let header = match sort {
            Some((sorted, descending)) if sorted == column => {
                format!("{header} {}", if descending { '▼' } else { '▲' })
            }
            _ => header.clone(),
        };
        markdown.push_str(&format!(" {} |", escape_markdown_cell(&header)));
    }
    markdown.push_str("\n|");
    for numeric in &numeric_columns {
        markdown.push_str(if *numeric { " ---: |" } else { " --- |" });
    }
    for (cells, _) in parsed_rows.iter().take(MAX_TABLE_ROWS) {
        markdown.push_str("\n|");
        for (text, _) in cells {
            markdown.push_str(&format!(" {} |", escape_markdown_cell(text)));
        }
    }

    let mut lines = render_markdown(&markdown, width, show_picker, true);
    // Clicking a header sorts ascending, and clicking the sorted header
    // again flips it, like the macOS app.
    let hits = table_header_columns(&lines, shown_columns)
        .into_iter()
        .map(|(line, columns, column)| {
            let descending = sort == Some((column, false));
            (line, columns, VizAction::Sort { column, descending })
        })
        .collect();
    let mut notes = Vec::new();
    if parsed_rows.len() > MAX_TABLE_ROWS {
        notes.push(format!(
            "{} more rows not shown (limit {MAX_TABLE_ROWS})",
            parsed_rows.len() - MAX_TABLE_ROWS
        ));
    }
    if columns.len() > shown_columns {
        notes.push(format!(
            "{} more columns not shown (limit {MAX_TABLE_COLUMNS})",
            columns.len() - shown_columns
        ));
    }
    if parsed_rows.is_empty() {
        notes.push("no rows".to_owned());
    }
    if !notes.is_empty() {
        lines.push(more_line(format!("… {}", notes.join("; ")), show_picker));
    }
    Ok((lines, hits))
}

/// The display columns each header cell covers in a table drawn by the
/// Markdown renderer, as `(row, columns, column index)`.
///
/// A grid row is, per column, a padding span, the cell's styled spans, a
/// padding span, and a gap span before the next column; padding is unstyled
/// and every header span is styled. The header is every row above the `━`
/// rule. A table drawn as narrow key/value records has no header row, and
/// anything else that does not have this shape yields no targets rather than
/// a wrong one.
fn table_header_columns(
    lines: &[Line<'static>],
    columns: usize,
) -> Vec<(usize, std::ops::Range<u16>, usize)> {
    let is_rule = |line: &Line<'_>| {
        line.width() > 0
            && line
                .spans
                .iter()
                .all(|span| span.content.chars().all(|character| character == '━'))
    };
    let header_rows = lines.iter().position(is_rule).unwrap_or(lines.len().min(1));
    let is_padding = |span: &Span<'_>| {
        span.style == Style::default() && span.content.chars().all(|character| character == ' ')
    };
    let mut hits = Vec::new();
    for (row, line) in lines[..header_rows].iter().enumerate() {
        let mut spans = line.spans.iter().peekable();
        let mut x = 0usize;
        let mut row_hits = Vec::with_capacity(columns);
        for column in 0..columns {
            let Some(left) = spans.next().filter(|span| is_padding(span)) else {
                return Vec::new();
            };
            let start = x;
            x += left.width();
            while let Some(span) = spans.next_if(|span| span.style != Style::default()) {
                x += span.width();
            }
            let Some(right) = spans.next().filter(|span| is_padding(span)) else {
                return Vec::new();
            };
            x += right.width();
            row_hits.push((row, start as u16..x as u16, column));
            if column + 1 < columns {
                let Some(gap) = spans.next().filter(|span| is_padding(span)) else {
                    return Vec::new();
                };
                x += gap.width();
            }
        }
        if spans.next().is_some() {
            return Vec::new();
        }
        hits.extend(row_hits);
    }
    hits
}

struct TreeWalk<'a> {
    lines: Vec<Line<'static>>,
    hits: WidgetHits,
    shown: usize,
    hidden: usize,
    /// Preorder id of the next node in the whole tree, shown or not, so a
    /// node keeps its id however much above it is collapsed.
    next_id: usize,
    /// Nodes whose open state was flipped from the default (open).
    toggled: Option<&'a std::collections::BTreeSet<usize>>,
    width: usize,
    show_picker: bool,
}

fn validate_tree(nodes: &[Value], depth: usize, total: &mut usize) -> Result<(), String> {
    if depth > MAX_TREE_DEPTH * 4 {
        return Err("tree is nested too deeply".into());
    }
    for node in nodes {
        node.get("label")
            .and_then(Value::as_str)
            .ok_or("every tree node needs a string \"label\"")?;
        if let Some(note) = node.get("note")
            && !note.is_string()
            && !note.is_null()
        {
            return Err("tree \"note\" must be a string".into());
        }
        *total += 1;
        match node.get("children") {
            None | Some(Value::Null) => {}
            Some(Value::Array(children)) => validate_tree(children, depth + 1, total)?,
            Some(_) => return Err("tree \"children\" must be an array".into()),
        }
    }
    Ok(())
}

fn count_nodes(nodes: &[Value]) -> usize {
    nodes
        .iter()
        .map(|node| {
            1 + node
                .get("children")
                .and_then(Value::as_array)
                .map(|children| count_nodes(children))
                .unwrap_or(0)
        })
        .sum()
}

impl TreeWalk<'_> {
    fn walk(&mut self, nodes: &[Value], prefix: &str, depth: usize) {
        for (index, node) in nodes.iter().enumerate() {
            if self.shown >= MAX_TREE_NODES {
                self.hidden += count_nodes(&nodes[index..]);
                return;
            }
            let id = self.next_id;
            self.next_id += 1;
            let last = index + 1 == nodes.len();
            let branch = if depth == 0 {
                ""
            } else if last {
                "└─ "
            } else {
                "├─ "
            };
            let label = node.get("label").and_then(Value::as_str).unwrap_or("");
            let label = truncate_chars(&single_line(label), MAX_CELL_CHARS);
            let note = node
                .get("note")
                .and_then(Value::as_str)
                .map(|note| truncate_chars(&single_line(note), MAX_CELL_CHARS))
                .filter(|note| !note.is_empty());
            let children = node
                .get("children")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            // Children past the depth limit are never drawn, so that node is
            // not a toggle.
            let collapsible = !children.is_empty() && depth < MAX_TREE_DEPTH;
            let collapsed =
                collapsible && self.toggled.is_some_and(|toggled| toggled.contains(&id));

            let guide = format!("{prefix}{branch}");
            let guide_width = UnicodeWidthStr::width(guide.as_str());
            let mut available = self.width.saturating_sub(guide_width).max(1);
            let muted = get_themed_style(
                COLOR_MUTED(),
                COLOR_BG(),
                Modifier::empty(),
                self.show_picker,
            );
            let mut spans = vec![Span::styled(guide, muted)];
            if collapsible && available > 3 {
                spans.push(Span::styled(
                    if collapsed { "▸ " } else { "▾ " },
                    get_themed_style(
                        COLOR_PRIMARY(),
                        COLOR_BG(),
                        Modifier::empty(),
                        self.show_picker,
                    ),
                ));
                available -= 2;
            }
            let label_text = fit_width(&label, available);
            spans.push(Span::styled(
                label_text.clone(),
                get_themed_style(
                    COLOR_TEXT(),
                    COLOR_BG(),
                    if children.is_empty() {
                        Modifier::empty()
                    } else {
                        Modifier::BOLD
                    },
                    self.show_picker,
                ),
            ));
            let mut remaining =
                available.saturating_sub(UnicodeWidthStr::width(label_text.as_str()));
            if collapsible {
                let label_end = self.width - remaining;
                self.hits.push((
                    self.lines.len(),
                    guide_width as u16..label_end as u16,
                    VizAction::ToggleNode(id),
                ));
            }
            if collapsed {
                let hint = format!("  {} hidden", count_nodes(children));
                if remaining >= UnicodeWidthStr::width(hint.as_str()) {
                    remaining -= UnicodeWidthStr::width(hint.as_str());
                    spans.push(Span::styled(
                        hint,
                        get_themed_style(
                            COLOR_MUTED(),
                            COLOR_BG(),
                            Modifier::ITALIC,
                            self.show_picker,
                        ),
                    ));
                }
            }
            if let Some(note) = note
                && remaining > 4
            {
                spans.push(Span::styled(
                    format!("  {}", fit_width(&note, remaining - 2)),
                    muted,
                ));
            }
            self.lines.push(Line::from(spans));
            self.shown += 1;

            if children.is_empty() {
                continue;
            }
            if !collapsible {
                self.hidden += count_nodes(children);
                self.next_id += count_nodes(children);
                continue;
            }
            if collapsed {
                self.next_id += count_nodes(children);
                continue;
            }
            let child_prefix = if depth == 0 {
                String::new()
            } else if last {
                format!("{prefix}   ")
            } else {
                format!("{prefix}│  ")
            };
            self.walk(children, &child_prefix, depth + 1);
        }
    }
}

fn render_tree(
    object: &serde_json::Map<String, Value>,
    width: usize,
    show_picker: bool,
    view: Option<&VizView>,
) -> Result<(Vec<Line<'static>>, WidgetHits), String> {
    let nodes = object
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("tree needs a \"nodes\" array")?;
    if nodes.is_empty() {
        return Err("tree has no nodes".into());
    }
    let mut total = 0;
    validate_tree(nodes, 0, &mut total)?;

    let mut walk = TreeWalk {
        lines: Vec::new(),
        hits: Vec::new(),
        shown: 0,
        hidden: 0,
        next_id: 0,
        toggled: view.map(|view| &view.toggled),
        width,
        show_picker,
    };
    walk.walk(nodes, "", 0);
    let mut lines = walk.lines;
    if walk.hidden > 0 {
        lines.push(more_line(
            format!(
                "… {} more nodes not shown (limits: {MAX_TREE_NODES} nodes, depth {MAX_TREE_DEPTH})",
                walk.hidden
            ),
            show_picker,
        ));
    }
    Ok((lines, walk.hits))
}

#[cfg(test)]
mod tests {
    use super::super::history_cell::{AssistantMarkdownCell, HistoryCell};
    use super::*;

    const BAR: &str = r#"{"version":1,"type":"bar_chart","title":"Build time","summary":"engine dominates","unit":"s","items":[{"label":"engine","value":42},{"label":"tui","value":10.5},{"label":"core","value":3}]}"#;

    fn fenced(body: &str) -> String {
        format!("Here is the breakdown:\n\n```rustcode-viz\n{body}\n```\n\nDone.")
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(content: &str, width: u16, generating: bool) -> (Vec<Line<'static>>, Vec<String>) {
        let mut lines = Vec::new();
        let mut copies = Vec::new();
        render_assistant_message(
            content,
            &mut lines,
            &mut copies,
            AssistantRenderOptions {
                token_usage: None,
                response_time_ms: None,
                thought_time_ms: None,
                thought_tokens: None,
                is_generating: generating,
                viewport_width: width,
                show_picker: false,
                last_copy_text: None,
            },
        );
        (
            lines.iter().map(own_line).collect(),
            copies.into_iter().map(|(_, text)| text).collect(),
        )
    }

    fn trimmed(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        while lines.last().is_some_and(|line| line.spans.is_empty()) {
            lines.pop();
        }
        lines
    }

    #[test]
    fn bar_chart_renders_natively_and_copies_the_source_fence() {
        let content = fenced(BAR);
        let (lines, copies) = render(&content, 60, false);
        let rendered = text(&lines);
        assert!(rendered.contains("◆ Build time"), "{rendered}");
        assert!(rendered.contains("engine dominates"));
        assert!(rendered.contains("42 s"));
        assert!(rendered.contains('█'));
        assert!(
            !rendered.contains("\"version\""),
            "JSON must not leak: {rendered}"
        );
        assert!(rendered.contains("Here is the breakdown:"));
        assert!(rendered.contains("Done."));
        assert_eq!(copies, [format!("```rustcode-viz\n{BAR}\n```")]);
        assert!(lines.iter().all(|line| line.width() <= 60), "{rendered}");
    }

    #[test]
    fn bars_scale_to_the_largest_value() {
        let lines = render_visualization(BAR, 50, false).unwrap();
        let bar_cells = |row: &Line<'_>| row.to_string().chars().filter(|c| *c == '█').count();
        let row = |label: &str| {
            lines
                .iter()
                .find(|line| {
                    let line = line.to_string();
                    line.starts_with(label) && line.contains('█')
                })
                .map(bar_cells)
                .unwrap_or(0)
        };
        let (engine, tui, core) = (row("engine "), row("tui "), row("core "));
        assert!(engine > 3 * tui && tui > core, "{}", text(&lines));
    }

    #[test]
    fn table_renders_sorted_with_escaped_cells() {
        let body = r#"{"version":1,"type":"table","title":"Slow tests","columns":["Test","ms"],"rows":[["a_*bold*",5],["b|pipe",900],["c",40]],"sort_by":{"column":1,"descending":true}}"#;
        let lines = render_visualization(body, 60, false).unwrap();
        let rendered = text(&lines);
        assert!(
            rendered.contains("a_*bold*"),
            "markup must stay literal: {rendered}"
        );
        assert!(rendered.contains("b|pipe"), "{rendered}");
        let b = rendered.find("b|pipe").unwrap();
        let c = rendered.find("c ").unwrap_or(usize::MAX);
        let a = rendered.find("a_*bold*").unwrap();
        assert!(b < c && c < a, "rows must be sorted descending: {rendered}");
    }

    #[test]
    fn tree_renders_guides_and_notes() {
        let body = r#"{"version":1,"type":"tree","title":"ui","nodes":[{"label":"ui/","children":[{"label":"markdown.rs","note":"tables"},{"label":"modals/","children":[{"label":"theme.rs"}]}]}]}"#;
        let rendered = text(&render_visualization(body, 60, false).unwrap());
        assert!(rendered.contains("├─ markdown.rs  tables"), "{rendered}");
        assert!(rendered.contains("└─ ▾ modals/"), "{rendered}");
        assert!(rendered.contains("   └─ theme.rs"), "{rendered}");
    }

    const TABLE: &str = r#"{"version":1,"type":"table","title":"Slow tests","columns":["Test","ms"],"rows":[["b",5],["a",900],["c",40]]}"#;
    const TREE: &str = r#"{"version":1,"type":"tree","nodes":[{"label":"ui/","children":[{"label":"modals/","children":[{"label":"theme.rs"},{"label":"picker.rs"}]},{"label":"tests/","children":[{"label":"x.rs"}]}]}]}"#;

    fn with_view(
        body: &str,
        width: usize,
        view: &VizView,
    ) -> (Vec<Line<'static>>, WidgetHits, String) {
        let (lines, hits) = render_visualization_with_view(body, width, false, Some(view)).unwrap();
        let rendered = text(&lines);
        (lines, hits, rendered)
    }

    #[test]
    fn clicked_sort_orders_numbers_numerically_and_marks_the_header() {
        let ascending = VizView {
            sort: Some((1, false)),
            ..Default::default()
        };
        let (_, _, rendered) = with_view(TABLE, 60, &ascending);
        assert!(rendered.contains("ms ▲"), "{rendered}");
        let at = |needle: &str| rendered.find(needle).unwrap();
        // 5 < 40 < 900, which a text sort would get wrong.
        assert!(at(" b ") < at(" c ") && at(" c ") < at(" a "), "{rendered}");

        let by_name = VizView {
            sort: Some((0, true)),
            ..Default::default()
        };
        let (_, _, rendered) = with_view(TABLE, 60, &by_name);
        assert!(rendered.contains("Test ▼") && !rendered.contains('▲'));
        let at = |needle: &str| rendered.find(needle).unwrap();
        assert!(at(" c ") < at(" b ") && at(" b ") < at(" a "), "{rendered}");

        // The payload's own sort_by is marked too, and a click overrides it.
        let sorted = TABLE.replace(
            r#""rows""#,
            r#""sort_by":{"column":1,"descending":true},"rows""#,
        );
        let rendered = text(&render_visualization(&sorted, 60, false).unwrap());
        assert!(rendered.contains("ms ▼"), "{rendered}");
        let (_, _, rendered) = with_view(&sorted, 60, &by_name);
        assert!(rendered.contains("Test ▼") && !rendered.contains("ms ▼"));
    }

    #[test]
    fn header_targets_cover_their_column_and_pick_the_next_direction() {
        let (lines, hits, _) = with_view(TABLE, 60, &VizView::default());
        assert_eq!(hits.len(), 2, "{hits:?}");
        for (line, columns, action) in &hits {
            let row = lines[*line].to_string();
            let cell: String = row
                .chars()
                .skip(usize::from(columns.start))
                .take(usize::from(columns.end - columns.start))
                .collect();
            let VizAction::Sort { column, descending } = action else {
                panic!("{action:?}");
            };
            assert!(!descending);
            assert_eq!(cell.trim(), ["Test", "ms"][*column], "{row:?}");
        }

        let ascending = VizView {
            sort: Some((1, false)),
            ..Default::default()
        };
        let (_, hits, _) = with_view(TABLE, 60, &ascending);
        let actions = hits
            .iter()
            .map(|(_, _, action)| *action)
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            [
                VizAction::Sort {
                    column: 0,
                    descending: false
                },
                VizAction::Sort {
                    column: 1,
                    descending: true
                }
            ]
        );

        // Narrow key/value records have no header row to click.
        let wide = r#"{"version":1,"type":"table","columns":["Name","Purpose","Owner"],"rows":[["rustcode","a terminal coding agent","core"]]}"#;
        let (_, hits, rendered) = with_view(wide, 24, &VizView::default());
        assert!(rendered.contains("Name: rustcode"), "{rendered}");
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn collapsed_tree_nodes_hide_their_subtree_with_a_count() {
        let (_, hits, rendered) = with_view(TREE, 60, &VizView::default());
        assert!(rendered.contains("▾ ui/"), "{rendered}");
        assert!(rendered.contains("├─ ▾ modals/"), "{rendered}");
        assert!(rendered.contains("│  ├─ theme.rs"), "{rendered}");
        // Only nodes with children are targets: ui/, modals/, tests/ in
        // preorder.
        let ids = hits
            .iter()
            .map(|(_, _, action)| *action)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                VizAction::ToggleNode(0),
                VizAction::ToggleNode(1),
                VizAction::ToggleNode(4)
            ]
        );

        let mut view = VizView::default();
        view.apply(VizAction::ToggleNode(1));
        let (_, hits, rendered) = with_view(TREE, 60, &view);
        assert!(rendered.contains("├─ ▸ modals/  2 hidden"), "{rendered}");
        assert!(!rendered.contains("theme.rs"), "{rendered}");
        // tests/ keeps its id although fewer rows come before it.
        assert!(
            hits.iter()
                .any(|(line, _, action)| *action == VizAction::ToggleNode(4)
                    && text(&with_view(TREE, 60, &view).0[*line..=*line]).contains("tests/"))
        );

        view.apply(VizAction::ToggleNode(0));
        let (_, _, rendered) = with_view(TREE, 60, &view);
        assert!(rendered.contains("▸ ui/  5 hidden"), "{rendered}");
        assert!(!rendered.contains("modals/"), "{rendered}");

        view.apply(VizAction::ToggleNode(0));
        view.apply(VizAction::ToggleNode(1));
        assert!(view.toggled.is_empty());
        let (_, _, rendered) = with_view(TREE, 60, &view);
        assert!(rendered.contains("theme.rs"));
    }

    #[test]
    fn message_targets_sit_on_widget_rows_past_the_gutter_and_skip_stale_state() {
        let content = fenced(TABLE);
        let cell = AssistantMarkdownCell::committed(&content, None, None, None, None);
        let (lines, hits) = cell.interactive_display_lines(60, Vec::new());
        assert_eq!(hits.len(), 2);
        for hit in &hits {
            assert_eq!(hit.ordinal, 0);
            assert_eq!(hit.source, source_hash(TABLE));
            let row = lines[hit.line].to_string();
            let cell: String = row
                .chars()
                .skip(usize::from(hit.columns.start))
                .take(usize::from(hit.columns.end - hit.columns.start))
                .collect();
            assert!(["Test", "ms"].contains(&cell.trim()), "{row:?} {hit:?}");
        }
        // Without view state the interactive render matches the cached one.
        assert_eq!(text(&lines), text(&cell.display_lines(60)));

        // State made for another body is ignored.
        let stale = VizView {
            source: source_hash("something else"),
            sort: Some((1, false)),
            ..Default::default()
        };
        let (lines, _) = cell.interactive_display_lines(60, vec![(0, stale)]);
        assert!(!text(&lines).contains('▲'));
        let fresh = VizView {
            source: source_hash(TABLE),
            sort: Some((1, false)),
            ..Default::default()
        };
        let (lines, _) = cell.interactive_display_lines(60, vec![(0, fresh)]);
        assert!(text(&lines).contains("ms ▲"));

        // A live stream is never interactive, even after the fence closes.
        let live = AssistantMarkdownCell::streaming(&content, false, None, None, None);
        assert!(!text(&live.display_lines(60)).contains('▲'));
    }

    #[test]
    fn invalid_blocks_fall_back_to_source_with_a_note() {
        for (body, reason) in [
            ("{not json", "invalid JSON"),
            (r#"{"type":"bar_chart","items":[]}"#, "missing \"version\""),
            (
                r#"{"version":2,"type":"bar_chart"}"#,
                "unsupported version 2",
            ),
            (
                r#"{"version":1,"type":"pie","items":[]}"#,
                "unsupported type \"pie\"",
            ),
            (
                r#"{"version":1,"type":"bar_chart","items":[{"label":"x","value":"big"}]}"#,
                "finite numeric",
            ),
            (r#"[1,2,3]"#, "not a JSON object"),
        ] {
            let (lines, _) = render(&fenced(body), 80, false);
            let rendered = text(&lines);
            assert!(
                rendered.contains("Visualization not rendered") && rendered.contains(reason),
                "{body}: {rendered}"
            );
            assert!(
                rendered.contains(body.split('"').next().unwrap()),
                "source kept: {rendered}"
            );
        }
    }

    #[test]
    fn oversized_payloads_fall_back_and_large_lists_truncate_visibly() {
        let huge = format!(
            r#"{{"version":1,"type":"bar_chart","items":[{{"label":"{}","value":1}}]}}"#,
            "x".repeat(MAX_PAYLOAD_BYTES)
        );
        let error = render_visualization(&huge, 80, false).unwrap_err();
        assert!(error.contains("limit is 16 KiB"), "{error}");

        let items = (0..45)
            .map(|index| format!(r#"{{"label":"item {index}","value":{index}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(r#"{{"version":1,"type":"bar_chart","items":[{items}]}}"#);
        let lines = render_visualization(&body, 80, false).unwrap();
        // heading + 30 bars + truncation note
        assert_eq!(lines.len(), 1 + MAX_BAR_ITEMS + 1);
        assert!(text(&lines).contains("… 15 more items not shown"));

        let rows = (0..80)
            .map(|index| format!(r#"["r{index}",{index}]"#))
            .collect::<Vec<_>>()
            .join(",");
        let columns = (0..12)
            .map(|index| format!(r#""c{index}""#))
            .collect::<Vec<_>>()
            .join(",");
        let body =
            format!(r#"{{"version":1,"type":"table","columns":[{columns}],"rows":[{rows}]}}"#);
        let rendered = text(&render_visualization(&body, 200, false).unwrap());
        assert!(rendered.contains("30 more rows not shown"), "{rendered}");
        assert!(rendered.contains("4 more columns not shown"), "{rendered}");

        let leaves = (0..200)
            .map(|index| format!(r#"{{"label":"n{index}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(
            r#"{{"version":1,"type":"tree","nodes":[{{"label":"root","children":[{leaves}]}}]}}"#
        );
        let lines = render_visualization(&body, 80, false).unwrap();
        assert!(lines.len() <= MAX_TREE_NODES + 2);
        assert!(text(&lines).contains("81 more nodes not shown"));

        let long_label = "y".repeat(500);
        let body = format!(
            r#"{{"version":1,"type":"bar_chart","title":"{long_label}","items":[{{"label":"{long_label}","value":1}}]}}"#
        );
        let lines = render_visualization(&body, 80, false).unwrap();
        assert!(lines.iter().all(|line| line.width() <= 80));
        assert!(text(&lines).contains('…'));
    }

    #[test]
    fn control_characters_cannot_reach_the_terminal() {
        let body = r#"{"version":1,"type":"tree","title":"a\u001b[31mred","nodes":[{"label":"x\u0007\nbell"}]}"#;
        let rendered = text(&render_visualization(body, 80, false).unwrap());
        assert!(
            !rendered.chars().any(|c| c.is_control() && c != '\n'),
            "{rendered:?}"
        );
    }

    #[test]
    fn partial_stream_shows_a_placeholder_and_never_parses_the_body() {
        let partial = "Intro\n\n```rustcode-viz\n{\"version\":1,\"type\":\"bar_chart\",\"items\":[";
        let (lines, copies) = render(partial, 80, true);
        let rendered = text(&lines);
        assert!(
            rendered.contains("waiting for the closing fence (1 line)"),
            "{rendered}"
        );
        assert!(!rendered.contains("bar_chart"), "{rendered}");
        assert!(!rendered.contains("not rendered"), "{rendered}");
        assert!(copies.is_empty());

        // A finalized message whose fence never closed shows its source.
        let (lines, _) = render(partial, 80, false);
        let rendered = text(&lines);
        assert!(
            rendered.contains("incomplete block") && rendered.contains("bar_chart"),
            "{rendered}"
        );

        // Scrollback keeps the open block in the mutable viewport.
        let cursor = super::super::scrollback::TranscriptCursor::default();
        assert_eq!(cursor.pending_stable_source(partial).trim_end(), "Intro");
    }

    #[test]
    fn live_and_committed_renders_match_across_widths() {
        let content = fenced(BAR);
        for width in [16u16, 24, 40, 80, 140] {
            let live = trimmed(
                AssistantMarkdownCell::streaming(&content, false, None, None, None)
                    .display_lines(width),
            );
            let committed = trimmed(
                AssistantMarkdownCell::committed(&content, None, None, None, None)
                    .display_lines(width),
            );
            assert_eq!(text(&live), text(&committed), "width {width}");
            assert!(
                live.iter().all(|line| line.width() <= width as usize),
                "width {width}: {}",
                text(&live)
            );
        }
        // Narrow widths stack labels above bars instead of overflowing.
        let narrow =
            AssistantMarkdownCell::committed(&content, None, None, None, None).display_lines(16);
        assert!(text(&narrow).contains("engine\n"), "{}", text(&narrow));
    }

    #[test]
    fn widget_colors_follow_the_active_theme() {
        let _theme_guard = super::super::tests::THEME_TEST_LOCK
            .lock()
            .expect("theme test lock");
        let content = fenced(BAR);
        let cell = AssistantMarkdownCell::committed(&content, None, None, None, None);
        let bar_color = |lines: &[Line<'_>]| {
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .find(|span| span.content.contains('█'))
                .and_then(|span| span.style.fg)
        };
        theme::set_active_theme("default");
        let first = bar_color(&cell.display_lines(80));
        assert_eq!(first, Some(COLOR_PRIMARY()));
        theme::set_active_theme("nord");
        let second = bar_color(&cell.display_lines(80));
        assert_eq!(second, Some(COLOR_PRIMARY()));
        theme::set_active_theme("default");
        assert_ne!(first, second, "theme change must re-render the widget");
    }

    #[test]
    fn documented_example_renders() {
        let body = r#"{"version":1,"type":"bar_chart","title":"Test wall time","summary":"engine lib tests take 70% of the run","unit":"s",
 "items":[{"label":"rustcode (lib)","value":41.8},{"label":"rustcode-tui","value":12.1},{"label":"rustcode-core","value":2.4}]}"#;
        let rendered = text(&render_visualization(body, 56, false).unwrap());
        println!("{rendered}");
        assert!(rendered.starts_with(
            "◆ Test wall time\nengine lib tests take 70% of the run\nrustcode (lib) █"
        ));
        assert!(rendered.contains("2.4 s"));
    }

    #[test]
    fn ordinary_fences_and_markdown_tables_are_unchanged() {
        let content = "```json\n{\"version\":1,\"type\":\"bar_chart\",\"items\":[]}\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |";
        let (lines, copies) = render(content, 80, false);
        let rendered = text(&lines);
        assert!(rendered.contains("\"type\":\"bar_chart\""), "{rendered}");
        assert!(!rendered.contains("Visualization"), "{rendered}");
        assert_eq!(
            copies,
            ["{\"version\":1,\"type\":\"bar_chart\",\"items\":[]}"]
        );
        assert!(
            rendered.contains('━') && !rendered.contains("| --- |"),
            "{rendered}"
        );
        assert!(
            !is_viz_info("rustcode-viz2") && !is_viz_info("json") && is_viz_info(" rustcode-viz ")
        );
    }
}
