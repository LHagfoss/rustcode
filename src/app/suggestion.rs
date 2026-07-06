/// Command-suggestion cycling logic for the input buffer.
/// When the user types `/`, Tab cycles through matching commands.

#[allow(clippy::empty_line_after_doc_comments)]
pub const COMMANDS: &[&str] = &["/help", "/clear", "/new", "/cancel", "/exit", "/quit"];

#[derive(Debug, Default)]
pub struct SuggestionCycle {
    /// Original prefix the user typed before cycling started.
    pub original_prefix: Option<String>,
    /// Current index into the filtered match list (None = no active cycle).
    pub suggestion_index: Option<usize>,
}

impl SuggestionCycle {
    pub fn new() -> Self { Self::default() }

    /// Cycle the match list forward, updating internal state. Returns true if advanced.
    pub fn cycle(&mut self, input_buffer: &str) -> bool {
        if !input_buffer.starts_with('/') || input_buffer.is_empty() { return false; }

        let prefix = if let Some(ref p) = self.original_prefix {
            p.clone()
        } else {
            let p = input_buffer.to_string();
            self.original_prefix = Some(p.clone());
            p
        };

        let matches: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(&prefix)).collect();
        if matches.is_empty() { return false; }

        let next_idx = match self.suggestion_index {
            Some(idx) => (idx + 1) % matches.len(),
            None => 0,
        };

        self.suggestion_index = Some(next_idx);
        true
    }

    /// Returns the suffix to render as a completion hint (text after `input_buffer`).
    pub fn get_completion_suffix(&self, input_buffer: &str) -> Option<String> {
        if !input_buffer.starts_with('/') || input_buffer.is_empty() { return None; }

        let prefix = self.original_prefix.as_deref().unwrap_or(input_buffer);
        let matches: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(prefix)).collect();
        if matches.is_empty() || self.suggestion_index.is_none() { return None; }

        let idx = self.suggestion_index.unwrap();
        Some(matches[idx].strip_prefix(input_buffer).unwrap_or("").to_string())
    }

    /// Reset the cycle state (called on any keypress other than Tab).
    pub fn reset(&mut self) {
        self.original_prefix = None;
        self.suggestion_index = None;
    }
}
