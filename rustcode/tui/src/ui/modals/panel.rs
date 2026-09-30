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

use super::*;
use crate::ui::highlight::highlight_code_line;

/// How strongly a panel value should stand out.
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
/// plus the free-space color. Every entry comes from a theme accessor so the
/// panel follows the active theme in light and dark modes alike.
pub fn context_category_colors() -> [Color; 8] {
    [
        COLOR_PRIMARY(),
        COLOR_GREEN(),
        COLOR_TIP(),
        COLOR_SECONDARY(),
        COLOR_MUTED(),
        COLOR_TEXT(),
        COLOR_DIFF_ADD_FG(),
        COLOR_TURN_SEPARATOR(),
    ]
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
    fn category_colors_come_from_theme_accessors() {
        let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
        for theme in ["default", "light", "nord", "dracula", "tokyo-night", "sky"] {
            crate::ui::theme::set_active_theme(theme);
            assert_eq!(
                context_category_colors(),
                [
                    COLOR_PRIMARY(),
                    COLOR_GREEN(),
                    COLOR_TIP(),
                    COLOR_SECONDARY(),
                    COLOR_MUTED(),
                    COLOR_TEXT(),
                    COLOR_DIFF_ADD_FG(),
                    COLOR_TURN_SEPARATOR(),
                ],
                "theme {theme} must flow through accessors, not literals"
            );
        }
        crate::ui::theme::set_active_theme("default");
    }
}
