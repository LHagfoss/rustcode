/// Search state for the currently rendered conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationSearch {
    query: String,
    matches: Vec<usize>,
    current: Option<usize>,
}

impl ConversationSearch {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: impl Into<String>, rows: &[(usize, String)]) {
        let query = query.into();
        if self.query == query {
            return;
        }
        self.query = query;
        self.rebuild(rows, true);
    }

    pub fn update_rows(&mut self, rows: &[(usize, String)]) {
        self.rebuild(rows, false);
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.matches.clear();
        self.current = None;
    }

    pub fn count(&self) -> usize {
        self.matches.len()
    }

    pub fn current_number(&self) -> Option<usize> {
        self.current.map(|index| index + 1)
    }

    pub fn current_row(&self) -> Option<usize> {
        self.current
            .and_then(|index| self.matches.get(index).copied())
    }

    pub fn next(&mut self) -> Option<usize> {
        self.move_by(1)
    }

    pub fn previous(&mut self) -> Option<usize> {
        self.move_by(-1)
    }

    fn move_by(&mut self, direction: isize) -> Option<usize> {
        let len = self.matches.len();
        if len == 0 {
            self.current = None;
            return None;
        }
        let current = self.current.unwrap_or(0) as isize;
        let next = (current + direction).rem_euclid(len as isize) as usize;
        self.current = Some(next);
        self.matches.get(next).copied()
    }

    fn rebuild(&mut self, rows: &[(usize, String)], reset: bool) {
        let selected = (!reset)
            .then(|| {
                let index = self.current?;
                let row = *self.matches.get(index)?;
                let occurrence = self.matches[..=index]
                    .iter()
                    .filter(|candidate| **candidate == row)
                    .count();
                Some((row, occurrence))
            })
            .flatten();
        let previous_matches = std::mem::take(&mut self.matches);
        let previous_current = self.current;
        let needle = self.query.to_lowercase();
        if needle.is_empty() {
            self.current = None;
            return;
        }
        let mut matches = Vec::new();
        for (row_index, text) in rows {
            let haystack = text.to_lowercase();
            let mut offset = 0;
            while let Some(relative) = haystack[offset..].find(&needle) {
                matches.push(*row_index);
                offset += relative + needle.len();
            }
        }
        self.current = if matches.is_empty() {
            None
        } else if !reset && previous_matches == matches {
            previous_current
                .filter(|index| *index < matches.len())
                .or(Some(0))
        } else if let Some((row, occurrence)) = selected {
            matches
                .iter()
                .enumerate()
                .filter(|(_, candidate)| **candidate == row)
                .nth(occurrence.saturating_sub(1))
                .map(|(index, _)| index)
                .or(Some(0))
        } else {
            Some(0)
        };
        self.matches = matches;
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationSearch;

    fn rows() -> Vec<(usize, String)> {
        vec![
            (0, "A user asks about RustCode".into()),
            (
                1,
                "RustCode searches this transcript; RustCode is native".into(),
            ),
            (2, "Unrelated".into()),
        ]
    }

    #[test]
    fn counts_case_insensitive_occurrences_and_wraps_navigation() {
        let mut search = ConversationSearch::default();
        search.set_query("rustcode", &rows());

        assert_eq!(search.count(), 3);
        assert_eq!(search.current_number(), Some(1));
        assert_eq!(search.current_row(), Some(0));
        assert_eq!(search.next(), Some(1));
        assert_eq!(search.next(), Some(1));
        search.update_rows(&rows());
        assert_eq!(search.current_number(), Some(3));
        assert_eq!(search.current_row(), Some(1));
        assert_eq!(search.next(), Some(0));
        assert_eq!(search.previous(), Some(1));
    }

    #[test]
    fn empty_and_unmatched_queries_have_no_current_hit() {
        let mut search = ConversationSearch::default();
        search.set_query("missing", &rows());
        assert_eq!(search.count(), 0);
        assert_eq!(search.next(), None);
        assert_eq!(search.previous(), None);

        search.set_query("", &rows());
        assert_eq!(search.count(), 0);
        assert_eq!(search.current_number(), None);
    }

    #[test]
    fn changing_query_starts_at_the_first_match_and_clear_resets_state() {
        let mut search = ConversationSearch::default();
        search.set_query("rustcode", &rows());
        search.next();
        search.set_query("native", &rows());

        assert_eq!(search.count(), 1);
        assert_eq!(search.current_number(), Some(1));
        assert_eq!(search.current_row(), Some(1));

        search.clear();
        assert_eq!(search.query(), "");
        assert_eq!(search.current_row(), None);
    }
}
