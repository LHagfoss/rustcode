//! Editing operations shared by every text field: the composer, the picker
//! search fields, a question's custom answer and the MCP server form. A field
//! is a `String` and a cursor, which is a byte offset on a character boundary.
//! One implementation keeps the keys meaning the same thing everywhere.

/// One edit a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEdit<'a> {
    Insert(&'a str),
    Left,
    Right,
    /// To the start of the word before the cursor.
    WordLeft,
    /// To the end of the word after the cursor.
    WordRight,
    LineStart,
    LineEnd,
    /// To the same column of the line above; to the start of the text from
    /// the first line.
    LineUp,
    /// To the same column of the line below; to the end of the text from the
    /// last line.
    LineDown,
    DeleteBackward,
    DeleteForward,
    DeleteWordBackward,
    DeleteWordForward,
    KillToLineStart,
    KillToLineEnd,
}

/// The nearest character boundary at or before `cursor`.
pub fn clamp(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// Apply `edit` to a field. Returns whether the text itself changed.
pub fn apply(text: &mut String, cursor: &mut usize, edit: TextEdit<'_>) -> bool {
    *cursor = clamp(text, *cursor);
    let at = *cursor;
    let target = match edit {
        TextEdit::Insert(inserted) => {
            text.insert_str(at, inserted);
            *cursor = at + inserted.len();
            return !inserted.is_empty();
        }
        TextEdit::Left | TextEdit::DeleteBackward => left(text, at),
        TextEdit::Right | TextEdit::DeleteForward => right(text, at),
        TextEdit::WordLeft | TextEdit::DeleteWordBackward => word_left(text, at),
        TextEdit::WordRight | TextEdit::DeleteWordForward => word_right(text, at),
        TextEdit::LineStart | TextEdit::KillToLineStart => line_start(text, at),
        TextEdit::LineEnd | TextEdit::KillToLineEnd => line_end(text, at),
        TextEdit::LineUp => line_up(text, at),
        TextEdit::LineDown => line_down(text, at),
    };
    let deletes = matches!(
        edit,
        TextEdit::DeleteBackward
            | TextEdit::DeleteForward
            | TextEdit::DeleteWordBackward
            | TextEdit::DeleteWordForward
            | TextEdit::KillToLineStart
            | TextEdit::KillToLineEnd
    );
    if !deletes {
        *cursor = target;
        return false;
    }
    let (start, end) = (at.min(target), at.max(target));
    text.replace_range(start..end, "");
    *cursor = start;
    start != end
}

/// Pasted text for a field that holds one line: line breaks become spaces and
/// other control characters are dropped.
pub fn single_line(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter_map(|character| match character {
            '\n' | '\r' | '\t' => Some(' '),
            character if character.is_control() => None,
            character => Some(character),
        })
        .collect()
}

pub fn left(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

pub fn right(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map_or(cursor, |character| cursor + character.len_utf8())
}

pub fn word_left(text: &str, cursor: usize) -> usize {
    let before = &text[..cursor];
    let word_end = before.trim_end_matches(char::is_whitespace).len();
    before[..word_end]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8())
}

pub fn word_right(text: &str, cursor: usize) -> usize {
    let after = &text[cursor..];
    let word_start = after.len() - after.trim_start_matches(char::is_whitespace).len();
    after[word_start..]
        .find(char::is_whitespace)
        .map_or(text.len(), |length| cursor + word_start + length)
}

pub fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

pub fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map_or(text.len(), |index| cursor + index)
}

fn line_up(text: &str, cursor: usize) -> usize {
    let start = line_start(text, cursor);
    if start == 0 {
        return 0;
    }
    let column = text[start..cursor].chars().count();
    let previous_end = start - 1;
    at_column(text, line_start(text, previous_end), previous_end, column)
}

fn line_down(text: &str, cursor: usize) -> usize {
    let end = line_end(text, cursor);
    if end == text.len() {
        return text.len();
    }
    let column = text[line_start(text, cursor)..cursor].chars().count();
    let next_start = end + 1;
    at_column(text, next_start, line_end(text, next_start), column)
}

/// The offset of `column` characters into the line, or the line's end.
fn at_column(text: &str, line_start: usize, line_end: usize, column: usize) -> usize {
    text[line_start..line_end]
        .char_indices()
        .nth(column)
        .map_or(line_end, |(index, _)| line_start + index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(text: &str, cursor: usize, edit: TextEdit<'_>) -> (String, usize) {
        let mut text = text.to_owned();
        let mut cursor = cursor;
        apply(&mut text, &mut cursor, edit);
        (text, cursor)
    }

    #[test]
    fn word_motion_stops_at_word_edges_in_both_directions() {
        let text = "alpha  beta gamma";
        assert_eq!(word_right(text, 0), 5);
        assert_eq!(word_right(text, 5), 11);
        assert_eq!(word_right(text, 13), text.len());
        assert_eq!(word_left(text, text.len()), 12);
        assert_eq!(word_left(text, 12), 7);
        assert_eq!(word_left(text, 6), 0);
        assert_eq!(word_left(text, 0), 0);
    }

    #[test]
    fn deleting_words_removes_what_the_motion_would_cross() {
        assert_eq!(
            edited("alpha beta gamma", 5, TextEdit::DeleteWordForward),
            ("alpha gamma".to_owned(), 5)
        );
        assert_eq!(
            edited("alpha beta gamma", 11, TextEdit::DeleteWordBackward),
            ("alpha gamma".to_owned(), 6)
        );
    }

    #[test]
    fn line_keys_stay_on_the_line_the_cursor_is_on() {
        let text = "first\nsecond line\nthird";
        assert_eq!(edited(text, 9, TextEdit::LineStart).1, 6);
        assert_eq!(edited(text, 9, TextEdit::LineEnd).1, 17);
        assert_eq!(
            edited(text, 9, TextEdit::KillToLineStart),
            ("first\nond line\nthird".to_owned(), 6)
        );
        assert_eq!(
            edited(text, 9, TextEdit::KillToLineEnd),
            ("first\nsec\nthird".to_owned(), 9)
        );
    }

    #[test]
    fn vertical_motion_keeps_the_column_and_ends_at_the_text_edges() {
        let text = "ab\nlonger line\nxy";
        assert_eq!(edited(text, 10, TextEdit::LineUp).1, 2);
        assert_eq!(edited(text, 10, TextEdit::LineDown).1, text.len());
        assert_eq!(edited(text, 4, TextEdit::LineDown).1, 16);
        assert_eq!(edited(text, 1, TextEdit::LineUp).1, 0);
        assert_eq!(edited(text, 15, TextEdit::LineDown).1, text.len());
    }

    #[test]
    fn a_stale_cursor_is_moved_to_a_character_boundary_before_editing() {
        // Inside the three bytes of `界`, and past the end.
        assert_eq!(
            edited("a界b", 2, TextEdit::Insert("x")),
            ("ax界b".to_owned(), 2)
        );
        assert_eq!(
            edited("a界b", 99, TextEdit::DeleteBackward),
            ("a界".to_owned(), 4)
        );
        assert_eq!(edited("a界b", 4, TextEdit::Left).1, 1);
    }

    #[test]
    fn single_line_paste_has_no_line_breaks_or_control_characters() {
        assert_eq!(
            single_line("npx\r\n-y\tserver\u{7}\nnext"),
            "npx -y server next"
        );
    }
}
