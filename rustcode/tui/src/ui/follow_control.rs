//! Return-to-latest affordance painted into the composer gap.
//!
//! Codex's model, adapted to RustCode's full-height inline viewport: the
//! transcript is projected from history rather than appended to native
//! scrollback, so "stay where you are" is the state the TUI holds and
//! "return to the newest row" is an explicit action instead of a teleport.
//! The control is hidden whenever the final transcript row is already visible,
//! degrades to shorter labels on a narrow terminal, and a hidden control owns
//! no rectangle at all, so it can never intercept pointer input (#1595).

use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use super::{COLOR_BG, COLOR_PRIMARY, get_themed_style};

/// The affordance's rendered rectangle for the last frame that showed it.
///
/// `None` whenever the control was hidden, which is what keeps a hidden
/// control from swallowing a click.
#[derive(Default)]
pub(crate) struct FollowControl {
    area: Option<Rect>,
}

impl FollowControl {
    pub(crate) fn area(&self) -> Option<Rect> {
        self.area
    }

    pub(crate) fn clear(&mut self) {
        self.area = None;
    }

    /// Choose the label that fits `width`, preferring the "new activity"
    /// wording so an arriving stream is announced rather than implied.
    pub(crate) fn label(width: u16, unseen_activity: bool) -> Option<&'static str> {
        let labels: &[&str] = if unseen_activity {
            &[
                " New activity · ↓ Back to bottom · esc ",
                " New activity · ↓ Bottom ",
                " New · ↓ Bottom ",
                " ↓ Bottom ",
                " ↓ ",
            ]
        } else {
            &[
                " ↓ Back to bottom · esc ",
                " ↓ Back to bottom ",
                " ↓ Bottom · esc ",
                " ↓ Bottom ",
                " ↓ ",
            ]
        };
        labels
            .iter()
            .copied()
            .find(|label| label.width() <= usize::from(width))
    }

    /// Paint the control centered on `gap`, or forget it when there is no gap
    /// or no label that fits.
    pub(crate) fn render(
        &mut self,
        gap: Option<Rect>,
        width: u16,
        unseen_activity: bool,
        buffer: &mut ratatui::buffer::Buffer,
        show_picker: bool,
    ) {
        let Some(gap) = gap.filter(|gap| !gap.is_empty()) else {
            self.clear();
            return;
        };
        let Some(label) = Self::label(width, unseen_activity) else {
            self.clear();
            return;
        };
        let label_width = label.width() as u16;
        if label_width > gap.width {
            self.clear();
            return;
        }
        let target = Rect::new(gap.x + (gap.width - label_width) / 2, gap.y, label_width, 1);
        let style = get_themed_style(COLOR_PRIMARY(), COLOR_BG(), Modifier::BOLD, show_picker);
        Paragraph::new(Line::from(Span::styled(label, style))).render(target, buffer);
        self.area = Some(target);
    }
}

#[cfg(test)]
mod tests {
    use super::FollowControl;

    #[test]
    fn a_label_is_always_chosen_and_degrades_to_the_shortest_form() {
        assert_eq!(
            FollowControl::label(80, false),
            Some(" ↓ Back to bottom · esc ")
        );
        assert_eq!(FollowControl::label(24, true), Some(" New · ↓ Bottom "));
        assert_eq!(FollowControl::label(9, true), Some(" ↓ "));
        assert_eq!(FollowControl::label(9, false), Some(" ↓ "));
    }

    #[test]
    fn new_activity_is_announced_before_it_fits() {
        // The unseen-activity wording is strictly longer, so a narrow terminal
        // has to drop it rather than truncate the label mid-word. "New" itself
        // survives down to 16 columns.
        assert_eq!(
            FollowControl::label(30, true),
            Some(" New activity · ↓ Bottom ")
        );
        assert!(FollowControl::label(24, true).unwrap().contains("New"));
        assert!(FollowControl::label(12, true).unwrap().contains("Bottom"));
        assert!(!FollowControl::label(12, true).unwrap().contains("New"));
        assert!(
            FollowControl::label(30, false)
                .unwrap()
                .contains("Back to bottom")
        );
    }
}
