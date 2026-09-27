//! Composer input parsing shared by frontends.
//!
//! Pure cursor/mention analysis: `@`-mention query extraction without
//! splitting unicode. Filesystem completion and the command registry stay in
//! the engine.

fn safe_byte_index(s: &str, byte_pos: usize) -> usize {
    let mut position = byte_pos.min(s.len());
    while !s.is_char_boundary(position) {
        position = position.saturating_sub(1);
    }
    position
}

pub fn get_at_word_query(input_buffer: &str, cursor_pos: usize) -> Option<(usize, String)> {
    let pos = safe_byte_index(input_buffer, cursor_pos);
    let before = &input_buffer[..pos];
    if let Some(at_idx) = before.rfind('@') {
        if at_idx > 0
            && before[..at_idx]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_whitespace())
        {
            return None;
        }
        let query = &before[at_idx + 1..];
        if !query.contains(' ') && !query.contains('\n') {
            return Some((at_idx, query.to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_completion_uses_the_byte_cursor_without_splitting_unicode() {
        let input = "inspect café @src/ma";
        let cursor = input.len();

        assert_eq!(
            get_at_word_query(input, cursor),
            Some((14, "src/ma".to_owned()))
        );
    }

    #[test]
    fn at_completion_is_scoped_to_the_token_before_the_cursor() {
        assert_eq!(
            get_at_word_query("look @src/main.rs then", 17),
            Some((5, "src/main.rs".to_owned()))
        );
        assert_eq!(get_at_word_query("email a@b.com", 13), None);
    }
}
