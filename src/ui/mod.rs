mod assistant_render;
mod composer;
mod composer_render;
mod conversation_render;
mod events;
mod frame_requester;
mod highlight;
mod history_cell;
mod keymap;
mod lru;
mod markdown;
mod modals;
mod render;
pub(crate) mod render_snapshot;
mod status_render;
mod terminal_runtime;
mod transcript;

pub(crate) use composer::{Composer, ComposerAction};
pub(crate) use events::{TuiEvent, TuiEventStream};
pub(crate) use frame_requester::{FrameRequester, FrameStream};
pub(crate) use history_cell::TranscriptState;
use history_cell::{HistoryCell, is_live_tool_call_visible};
pub(crate) use transcript::TranscriptModel;
pub(crate) mod scrollback;
mod tool_result;
mod tool_transcript;
pub(crate) use terminal_runtime::TerminalRuntime;

use assistant_render::*;
use composer_render::*;
pub(crate) use conversation_render::*;
pub use render::*;
pub(crate) use status_render::*;
pub(crate) use tool_transcript::*;

use highlight::{
    highlight_code_block, highlight_code_line, highlight_diff_line, highlight_shell_command,
    render_unified_diff, wrap_code_spans,
};
use markdown::{
    push_wrapped_with_continuation, render_markdown, unwrap_markdown_table_fences,
    wrap_styled_spans,
};
pub use modals::{PALETTE_ITEMS, PaletteItem};
pub mod theme;
pub(crate) use modals::{
    approval_event_for_key, question_answer_event, question_cancel_event,
    question_custom_answer_event,
};
use modals::{
    question_height, render_at_popup_menu, render_command_picker_modal, render_context_modal,
    render_effort_picker_modal, render_history_picker_modal, render_mcp_config_modal,
    render_model_picker_modal, render_popup_menu, render_protocol_picker_modal,
    render_question_modal, render_subagent_picker_modal, render_theme_picker_modal,
    render_thinking_picker_modal, render_tool_confirmation_modal, render_update_prompt_modal,
    render_verbosity_picker_modal, render_yolo_picker_modal, tool_confirmation_height,
};
use tool_result::render_tool_result;

use crate::app::activity::{
    ActivityKind, ActivitySnapshot, classify_activity, classify_live_tools,
};
use crate::app::{AppState, AppStatus, ChatMessage};
use crate::inline_terminal::Frame;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};
use render_snapshot::RenderSnapshot;
use std::hash::{Hash, Hasher};
#[cfg(not(test))]
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(not(test))]
use std::time::Instant;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

fn safe_byte_index(s: &str, byte_pos: usize) -> usize {
    let mut position = byte_pos.min(s.len());
    while !s.is_char_boundary(position) {
        position = position.saturating_sub(1);
    }
    position
}

/// Max visible rows in the slash-command popup; longer lists scroll internally.
const MAX_POPUP_ROWS: u16 = 10;

#[allow(non_snake_case)]
#[inline]
pub fn COLOR_BG() -> Color {
    theme::color_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_PANEL() -> Color {
    theme::color_panel()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_ELEMENT() -> Color {
    theme::color_element()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TEXT() -> Color {
    theme::color_text()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_MUTED() -> Color {
    theme::color_muted()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_PRIMARY() -> Color {
    theme::color_primary()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_SECONDARY() -> Color {
    theme::color_secondary()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_GREEN() -> Color {
    theme::color_green()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_SELECTION() -> Color {
    theme::color_selection()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TIP() -> Color {
    theme::color_tip()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_STATUS_BORDER() -> Color {
    theme::color_status_border()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_TURN_SEPARATOR() -> Color {
    theme::color_turn_separator()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_HOVER_BG() -> Color {
    theme::color_hover_bg()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_ADD_BG() -> Color {
    theme::color_diff_add_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_ADD_FG() -> Color {
    theme::color_diff_add_fg()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_REMOVE_BG() -> Color {
    theme::color_diff_remove_bg()
}
#[allow(non_snake_case)]
#[inline]
pub fn COLOR_DIFF_REMOVE_FG() -> Color {
    theme::color_diff_remove_fg()
}
#[allow(non_snake_case, dead_code)]
#[inline]
pub fn COLOR_DIFF_ABSENT_BG() -> Color {
    theme::color_diff_absent_bg()
}

pub use crate::app::suggestion::CommandInfo;

fn get_themed_style(fg: Color, bg: Color, modifier: Modifier, _show_picker: bool) -> Style {
    Style::default().fg(fg).bg(bg).add_modifier(modifier)
}

/// Collapse pasted image and long-text markers into compact display chips.
/// The raw markers stay in the underlying buffer / history so
/// `parse_multimodal_content` can still attach the content when the message is
/// sent — this only affects what the user sees.
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;

thread_local! {
    static MARKER_CACHE: RefCell<(u64, String)> = const { RefCell::new((0, String::new())) };
}

#[cfg(test)]
mod tests;
