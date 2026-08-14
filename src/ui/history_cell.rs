//! Presentation-only cells for the mutable end of the conversation.
//!
//! Codex keeps one active history cell and mutates it as tool events arrive.
//! RustCode's canonical history remains `ChatMessage`; this small projection
//! gives the TUI the same lifecycle shape without serializing terminal state or
//! making provider code depend on ratatui.

use crate::app::LiveToolCall;
use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{COLOR_BG, COLOR_MUTED, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_TEXT, get_themed_style};

const MAX_LIVE_CHILDREN: usize = 8;

fn is_exploration_tool(name: &str) -> bool {
    crate::app::activity::is_exploration_tool(name)
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let suffix = '…';
    let budget = width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        output.push(character);
    }
    if width > 0 {
        output.push(suffix);
    }
    output
}

/// Render the single mutable live tool cell shown at the end of the transcript.
///
/// The cell deliberately contains only a bounded invocation summary. Tool
/// output belongs to the finalized semantic result and is rendered by the
/// existing verbosity-aware result cells once execution completes.
pub(super) fn render_live_tool_cell(
    calls: &[LiveToolCall],
    width: u16,
    show_picker: bool,
) -> Vec<Line<'static>> {
    if calls.is_empty() || width == 0 {
        return Vec::new();
    }

    let all_exploration = calls.iter().all(|call| is_exploration_tool(&call.tool_name));
    let label = if all_exploration {
        "Exploring"
    } else if calls.iter().any(|call| call.tool_name == "run_command") {
        "Running"
    } else {
        "Using"
    };
    let title_style = get_themed_style(
        COLOR_PRIMARY(),
        COLOR_BG(),
        Modifier::BOLD,
        show_picker,
    );
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", title_style),
        Span::styled(label, title_style),
    ])];

    let child_width = (width as usize).saturating_sub(6).max(1);
    for (index, call) in calls.iter().take(MAX_LIVE_CHILDREN).enumerate() {
        let target = if call.target.is_empty() || call.target == "?" {
            String::new()
        } else {
            format!(" {}", truncate_to_width(&call.target, child_width))
        };
        let prefix = if index == 0 { "  └ " } else { "    " };
        lines.push(Line::from(vec![
            Span::styled(
                prefix,
                get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                call.action.clone(),
                get_themed_style(COLOR_SECONDARY(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
            Span::styled(
                target,
                get_themed_style(COLOR_TEXT(), COLOR_BG(), Modifier::empty(), show_picker),
            ),
        ]));
    }
    if calls.len() > MAX_LIVE_CHILDREN {
        lines.push(Line::from(Span::styled(
            format!("    … +{} more", calls.len() - MAX_LIVE_CHILDREN),
            get_themed_style(COLOR_MUTED(), COLOR_BG(), Modifier::ITALIC, show_picker),
        )));
    }

    lines
}
