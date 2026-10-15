//! Mouse selection in the composer, matching the transcript's gestures:
//! double-click selects a word, triple-click a line, a drag after either
//! extends by that unit, and shift-click extends the current selection.
//!
//! Offsets are byte offsets into the composer buffer, like
//! `composer_selection_anchor` and `cursor_position`.

use std::time::{Duration, Instant};

use unicode_segmentation::UnicodeSegmentation;

/// Presses this close in time and on the same cell count as one gesture,
/// the same window the transcript selection uses.
const MULTI_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Unit {
    #[default]
    Character,
    Word,
    Line,
}

#[derive(Debug, Default)]
pub(crate) struct ComposerMouse {
    last_click: Option<(Instant, u16, u16, u8)>,
    unit: Unit,
    /// The word or line the gesture started on; a drag never shrinks below it.
    origin: Option<(usize, usize)>,
}

impl ComposerMouse {
    /// A plain left press at `offset`. Returns the `(anchor, cursor)` the
    /// composer should take.
    pub(crate) fn press(
        &mut self,
        text: &str,
        column: u16,
        row: u16,
        offset: usize,
    ) -> (usize, usize) {
        let now = Instant::now();
        let count = match self.last_click {
            Some((when, x, y, count))
                if x == column && y == row && now.duration_since(when) <= MULTI_CLICK =>
            {
                count % 3 + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, column, row, count));
        self.unit = match count {
            2 => Unit::Word,
            3 => Unit::Line,
            _ => Unit::Character,
        };
        let range = unit_range(text, offset, self.unit);
        self.origin = (self.unit != Unit::Character).then_some(range);
        range
    }

    /// A shift-press: extend the current selection character by character
    /// from its anchor, as the transcript does.
    pub(crate) fn shift_press(&mut self) {
        self.last_click = None;
        self.unit = Unit::Character;
        self.origin = None;
    }

    /// Where a drag or release at `offset` puts `(anchor, cursor)`. A word or
    /// line gesture keeps the unit it started on selected and grows by whole
    /// units; a character gesture only moves the cursor.
    pub(crate) fn extend(&self, text: &str, anchor: usize, offset: usize) -> (usize, usize) {
        let Some((start, end)) = self.origin else {
            return (anchor, offset);
        };
        let (start, end) = (start.min(text.len()), end.min(text.len()));
        let (unit_start, unit_end) = unit_range(text, offset, self.unit);
        if unit_start < start {
            (end, unit_start)
        } else {
            (start, unit_end.max(end))
        }
    }
}

fn unit_range(text: &str, offset: usize, unit: Unit) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    match unit {
        Unit::Character => (offset, offset),
        Unit::Word => word_range(text, offset),
        Unit::Line => line_range(text, offset),
    }
}

/// The word at `offset`, or the single grapheme there when it is not part of
/// a word. A press past the end selects what ends there.
fn word_range(text: &str, offset: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }
    let probe = if offset >= text.len() {
        text.len() - text[..offset].chars().next_back().map_or(0, char::len_utf8)
    } else {
        offset
    };
    let Some((start, segment)) = text
        .split_word_bound_indices()
        .find(|(start, segment)| probe >= *start && probe < start + segment.len())
    else {
        return (offset, offset);
    };
    if segment.chars().any(|ch| ch.is_alphanumeric() || ch == '_') {
        return (start, start + segment.len());
    }
    segment
        .grapheme_indices(true)
        .map(|(at, grapheme)| (start + at, start + at + grapheme.len()))
        .find(|(from, through)| probe >= *from && probe < *through)
        .unwrap_or((offset, offset))
}

/// The logical line at `offset`, without its newline.
fn line_range(text: &str, offset: usize) -> (usize, usize) {
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(text: &str, (anchor, cursor): (usize, usize)) -> &str {
        &text[anchor.min(cursor)..anchor.max(cursor)]
    }

    #[test]
    fn single_press_places_a_collapsed_caret() {
        let mut mouse = ComposerMouse::default();
        assert_eq!(mouse.press("hello world", 3, 0, 3), (3, 3));
    }

    #[test]
    fn double_press_selects_the_word_and_triple_the_line() {
        let text = "first line\nsecond wörd here";
        let mut mouse = ComposerMouse::default();
        let at = text.find("wörd").unwrap() + 1;
        mouse.press(text, 5, 1, at);
        assert_eq!(selected(text, mouse.press(text, 5, 1, at)), "wörd");
        assert_eq!(
            selected(text, mouse.press(text, 5, 1, at)),
            "second wörd here"
        );
        // A fourth press starts over.
        let range = mouse.press(text, 5, 1, at);
        assert_eq!(range.0, range.1);
    }

    #[test]
    fn double_press_on_punctuation_selects_one_grapheme() {
        let text = "a, b";
        let mut mouse = ComposerMouse::default();
        mouse.press(text, 1, 0, 1);
        assert_eq!(selected(text, mouse.press(text, 1, 0, 1)), ",");
    }

    #[test]
    fn press_past_the_end_selects_the_last_word() {
        let text = "go now";
        let mut mouse = ComposerMouse::default();
        mouse.press(text, 9, 0, text.len());
        assert_eq!(selected(text, mouse.press(text, 9, 0, text.len())), "now");
    }

    #[test]
    fn presses_on_different_cells_do_not_combine() {
        let text = "alpha beta";
        let mut mouse = ComposerMouse::default();
        mouse.press(text, 1, 0, 1);
        let range = mouse.press(text, 7, 0, 7);
        assert_eq!(range.0, range.1);
    }

    #[test]
    fn word_drag_grows_by_whole_words_in_both_directions() {
        let text = "one two three four";
        let mut mouse = ComposerMouse::default();
        let at = text.find("three").unwrap();
        mouse.press(text, 9, 0, at);
        let (anchor, _) = mouse.press(text, 9, 0, at);
        let forward = mouse.extend(text, anchor, text.find("four").unwrap() + 1);
        assert_eq!(selected(text, forward), "three four");
        let backward = mouse.extend(text, anchor, 1);
        assert_eq!(selected(text, backward), "one two three");
        // The cursor sits at the far end the pointer moved toward.
        assert_eq!(backward.1, 0);
    }

    #[test]
    fn character_drag_and_shift_press_only_move_the_cursor() {
        let text = "hello world";
        let mut mouse = ComposerMouse::default();
        mouse.press(text, 0, 0, 0);
        assert_eq!(mouse.extend(text, 0, 7), (0, 7));
        mouse.shift_press();
        assert_eq!(mouse.extend(text, 2, 9), (2, 9));
    }
}
