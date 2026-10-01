//! One search rule for every panel filter, so the slash-command popup, the
//! command palette and the model picker agree on what a query matches.
//!
//! Prefix-only matching hid registered commands from ordinary typos (`/modle`
//! matched nothing, because `/model` is not a prefix of it), and plain
//! `contains` in the pickers made every other misspelling equally invisible.
//! Matching therefore accepts a subsequence or a small edit budget and reports
//! which characters matched, so a panel can highlight why a row matched
//! (#1588).

/// Edits a query may spend before it stops matching. Short queries get one,
/// longer ones two, so `/modle` still finds `/model` while a two-letter query
/// stays narrow enough to be useful.
fn edit_budget(len: usize) -> usize {
    if len <= 4 { 1 } else { 2 }
}

/// Case-fold one character without changing how many characters the string
/// has, so a match position stays valid in the original text.
fn fold(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// Character positions in `haystack` matched by `needle`, in ascending order,
/// or [`None`] when the query does not match.
///
/// Matching is case-insensitive and ignores whitespace in the query. An empty
/// query matches with no positions, so callers can highlight unconditionally
/// instead of special-casing "no search yet".
pub fn fuzzy_match_positions(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let needle: Vec<char> = needle
        .chars()
        .filter(|character| !character.is_whitespace())
        .map(fold)
        .collect();
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let haystack: Vec<char> = haystack.chars().map(fold).collect();
    subsequence_positions(&haystack, &needle).or_else(|| edit_budget_positions(&haystack, &needle))
}

/// True when `haystack` matches `needle` under [`fuzzy_match_positions`].
/// Single owner of the predicate, so a filter and the highlight it renders
/// never disagree about what matched.
pub fn fuzzy_matches(haystack: &str, needle: &str) -> bool {
    fuzzy_match_positions(haystack, needle).is_some()
}

/// Leftmost greedy subsequence: every query character appears in the haystack
/// in order, which is the cheap tier and covers `/mod` against `/model`.
fn subsequence_positions(haystack: &[char], needle: &[char]) -> Option<Vec<usize>> {
    let mut positions = Vec::with_capacity(needle.len());
    let mut cursor = 0;
    for character in needle {
        let offset = haystack[cursor..].iter().position(|c| c == character)?;
        positions.push(cursor + offset);
        cursor += offset + 1;
    }
    Some(positions)
}

/// Bounded edit-distance match with traceback, the tier that catches transposed
/// or dropped characters such as `modle` for `model`. Returns the aligned
/// positions so the caller highlights the same characters the match used.
fn edit_budget_positions(haystack: &[char], needle: &[char]) -> Option<Vec<usize>> {
    let budget = edit_budget(needle.len());
    // Each surplus haystack character costs an edit, so a haystack longer than
    // the query by more than the budget cannot match. Skipping the matrix keeps
    // a long description from costing O(query x description) per keystroke.
    if haystack.len() > needle.len() + budget {
        return None;
    }

    let mut rows = vec![(0..=haystack.len()).collect::<Vec<usize>>()];
    for (row_index, character) in needle.iter().enumerate() {
        let mut row = Vec::with_capacity(haystack.len() + 1);
        row.push(row_index + 1);
        for (column, candidate) in haystack.iter().enumerate() {
            let substitution = usize::from(character != candidate);
            row.push(
                (rows[row_index][column] + substitution)
                    .min(rows[row_index][column + 1] + 1)
                    .min(row[column] + 1),
            );
        }
        rows.push(row);
    }
    if rows[needle.len()][haystack.len()] > budget {
        return None;
    }

    // Traceback: a diagonal step aligned a query character with a haystack
    // character, which is exactly the character the highlight marks.
    let mut positions = Vec::with_capacity(needle.len());
    let (mut row, mut column) = (needle.len(), haystack.len());
    while row > 0 && column > 0 {
        let substitution = usize::from(needle[row - 1] != haystack[column - 1]);
        if rows[row][column] == rows[row - 1][column - 1] + substitution {
            positions.push(column - 1);
            row -= 1;
            column -= 1;
        } else if rows[row][column] == rows[row - 1][column] + 1 {
            row -= 1;
        } else {
            column -= 1;
        }
    }
    positions.reverse();
    Some(positions)
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_match_positions, fuzzy_matches};

    #[test]
    fn subsequence_matches_report_their_positions_in_order() {
        assert_eq!(
            fuzzy_match_positions("/model", "/mdl"),
            Some(vec![0, 1, 3, 5]),
            "a subsequence match reports every matched character"
        );
        assert!(fuzzy_matches("/compact", "/ct"));
        assert!(
            !fuzzy_matches("/model", "/moo"),
            "a subsequence must not match characters out of order"
        );
    }

    #[test]
    fn transposed_and_dropped_characters_still_match() {
        // The typo that motivated this: `/modle` is neither an exact nor a
        // prefix match for `/model`, so it used to match nothing at all.
        let positions = fuzzy_match_positions("/model", "/modle").expect("/modle matches /model");
        assert_eq!(positions.len(), 6, "one position per query character");
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));

        assert!(fuzzy_matches("/model", "/modl"));
        assert!(fuzzy_matches("/models", "/mdel"));
        assert!(fuzzy_matches("Show RAM usage", "ram usge"));
        assert!(fuzzy_matches("/sandbox", "/santbox"));
    }

    #[test]
    fn a_short_query_keeps_a_narrow_budget() {
        // One edit is all a one- or two-character query may spend, so a short
        // prefix does not drag in unrelated commands.
        assert!(fuzzy_matches("/model", "/mod"));
        assert!(!fuzzy_matches("/status", "/mod"));
        // A query one character longer than the name is a near miss the budget
        // still covers, so it keeps matching.
        assert!(fuzzy_matches("/context", "/contexts"));
    }

    #[test]
    fn unrelated_queries_do_not_match() {
        assert!(!fuzzy_matches("/model", "/stats"));
        assert!(!fuzzy_matches("read_only", "/sandbox"));
        assert!(!fuzzy_matches("/model", "/mdls"));
    }

    #[test]
    fn empty_queries_match_everything_without_positions() {
        assert_eq!(fuzzy_match_positions("/model", ""), Some(Vec::new()));
        assert_eq!(fuzzy_match_positions("/model", "  "), Some(Vec::new()));
        assert!(fuzzy_matches("/model", ""));
    }

    #[test]
    fn matching_ignores_case_and_query_whitespace() {
        assert!(fuzzy_matches("/Model", "/MODEL"));
        assert_eq!(
            fuzzy_match_positions("/model", "/ M O"),
            Some(vec![0, 1, 2])
        );
    }

    #[test]
    fn positions_index_the_original_text() {
        let positions = fuzzy_match_positions("Grüße/model", "/mod").expect("match");
        assert_eq!(
            positions,
            vec![5, 6, 7, 8],
            "positions must address the original characters, not a folded copy"
        );
    }
}
