use super::stable_hash;
use std::collections::{HashMap, HashSet, VecDeque};

/// Canonical diagnostic reason identifiers for reasoning loop intervention.
pub const DIAG_REPEATED_BLOCK: &str = "reasoning_loop.repeated_block";
pub const DIAG_CYCLE: &str = "reasoning_loop.cycle";
pub const DIAG_CROSS_TURN_SAME_PLAN: &str = "reasoning_loop.cross_turn_same_plan";
pub const DIAG_SAME_FILES_NO_PROGRESS: &str = "reasoning_loop.same_files_no_progress";
pub const DIAG_SEMANTIC_NO_PROGRESS: &str = "reasoning_loop.semantic_no_progress";
pub const DIAG_RECOVERY_EXHAUSTED: &str = "reasoning_loop.recovery_exhausted";

/// Status returned by reasoning repetition checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningLoopStatus {
    Ok,
    /// Repetition was confidently detected in reasoning/thinking. Holds the diagnostic reason.
    LoopDetected(&'static str),
}

/// Compact summary of a single turn's reasoning and actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnReasoningRecord {
    pub content_words: HashSet<String>,
    pub plan_hash: u64,
    pub target_files: HashSet<String>,
    pub has_ready_intent: bool,
    pub has_hesitation_intent: bool,
    pub had_edits: bool,
    pub tool_count: usize,
}

/// Input evidence for evaluating cross-turn reasoning behavior.
#[derive(Debug, Clone)]
pub struct TurnEvidence<'a> {
    pub reasoning: &'a str,
    pub target_files: &'a [&'a str],
    pub made_progress: bool,
    pub had_edits: bool,
    pub tool_count: usize,
    pub no_progress_streak: usize,
}

/// Detects pathological repetition in model reasoning/thinking streams and cross-turn plans.
#[derive(Debug, Clone, Default)]
pub struct ReasoningLoopDetector {
    /// In-flight unparsed reasoning text buffer.
    stream_buffer: String,
    /// Sliding window of recent normalized sentence hashes.
    recent_sentences: VecDeque<u64>,
    /// Counts of sentence hashes in the sliding window.
    sentence_counts: HashMap<u64, usize>,
    /// Tracks consecutive repeats of a single sentence: (hash, count).
    consecutive_sentence: (Option<u64>, usize),
    /// Sliding window of recent normalized paragraph info: (hash, content_words).
    recent_paragraphs: VecDeque<(u64, HashSet<String>)>,
    /// Counts of paragraph hashes in the recent paragraph window.
    paragraph_counts: HashMap<u64, usize>,
    /// Total reasoning characters processed in current stream.
    stream_reasoning_chars: usize,
    /// History of recent turns without workspace changes.
    recent_turns: VecDeque<TurnReasoningRecord>,
    /// Consecutive turns with the same or strongly equivalent plan without workspace progress.
    consecutive_same_plan_turns: usize,
    /// Consecutive turns exhibiting ready-to-implement hesitation without editing.
    consecutive_hesitation_turns: usize,
    /// Consecutive turns re-inspecting the same small set of files without editing.
    consecutive_small_file_set_turns: usize,
}

impl ReasoningLoopDetector {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all reasoning and loop tracking state. Called when the agent makes real progress (e.g. successful edit).
    pub fn reset(&mut self) {
        self.stream_buffer.clear();
        self.recent_sentences.clear();
        self.sentence_counts.clear();
        self.consecutive_sentence = (None, 0);
        self.recent_paragraphs.clear();
        self.paragraph_counts.clear();
        self.stream_reasoning_chars = 0;
        self.recent_turns.clear();
        self.consecutive_same_plan_turns = 0;
        self.consecutive_hesitation_turns = 0;
        self.consecutive_small_file_set_turns = 0;
    }

    /// Feed a newly streamed chunk of reasoning text and check for intra-stream loops.
    pub fn feed_chunk(&mut self, chunk: &str) -> ReasoningLoopStatus {
        self.stream_buffer.push_str(chunk);
        self.stream_reasoning_chars = self.stream_reasoning_chars.saturating_add(chunk.len());

        // Check for paragraph boundaries (\n\n)
        while let Some(pos) = self.stream_buffer.find("\n\n") {
            let paragraph = self.stream_buffer[..pos].to_string();
            self.stream_buffer.drain(..pos + 2);
            let status = self.observe_paragraph(&paragraph);
            if status != ReasoningLoopStatus::Ok {
                return status;
            }
            let s_status = self.observe_text_sentences(&paragraph);
            if s_status != ReasoningLoopStatus::Ok {
                return s_status;
            }
        }

        // Check sentence boundaries (. , ! , ? , \n)
        let mut search_start = 0;
        while let Some(rel_pos) = find_sentence_boundary(&self.stream_buffer[search_start..]) {
            let boundary = search_start + rel_pos;
            let sentence = self.stream_buffer[..boundary].to_string();
            self.stream_buffer.drain(..boundary);
            search_start = 0;
            let status = self.observe_sentence(&sentence);
            if status != ReasoningLoopStatus::Ok {
                return status;
            }
        }

        ReasoningLoopStatus::Ok
    }

    /// Evaluate complete reasoning text directly.
    #[allow(dead_code)]
    pub fn check_text(&mut self, text: &str) -> ReasoningLoopStatus {
        for p in text.split("\n\n") {
            let status = self.observe_paragraph(p);
            if status != ReasoningLoopStatus::Ok {
                return status;
            }
            let s_status = self.observe_text_sentences(p);
            if s_status != ReasoningLoopStatus::Ok {
                return s_status;
            }
        }
        ReasoningLoopStatus::Ok
    }

    /// Record turn reasoning with full evidence to detect behavioral loops across turns.
    pub fn record_turn_evidence(&mut self, evidence: &TurnEvidence<'_>) -> ReasoningLoopStatus {
        // A workspace mutation is decisive forward progress and clears the
        // behavioral history. A fresh read is useful information, but must not
        // erase an "I will edit now" hesitation streak: local models can keep
        // finding one more fact forever while never performing the announced
        // change.
        if evidence.made_progress && evidence.had_edits {
            self.reset();
            return ReasoningLoopStatus::Ok;
        }

        let words = extract_content_words(evidence.reasoning);
        let mut target_files = extract_target_files(evidence.reasoning);
        for f in evidence.target_files {
            target_files.insert(f.to_lowercase());
        }

        let sentences: Vec<String> = evidence
            .reasoning
            .split(&['\n', '.', '!', '?'][..])
            .filter_map(normalize_sentence)
            .collect();

        let plan_hash = if sentences.is_empty() {
            0
        } else {
            let plan_text = sentences
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            stable_hash(&plan_text)
        };

        let has_ready_intent = detect_ready_intent(evidence.reasoning);
        let has_hesitation_intent = detect_hesitation_intent(evidence.reasoning);

        // 1. Cross-turn plan repetition (exact hash or semantic word overlap)
        if let Some(prev) = self.recent_turns.back() {
            let jaccard = jaccard_similarity(&words, &prev.content_words);
            let exact_plan = prev.plan_hash == plan_hash && plan_hash != 0;
            let files_overlap = !target_files.is_disjoint(&prev.target_files)
                || (target_files.is_empty() && prev.target_files.is_empty());
            let sim_threshold = if files_overlap {
                if evidence.no_progress_streak >= 2 {
                    0.35
                } else {
                    0.45
                }
            } else if evidence.no_progress_streak >= 3 {
                0.55
            } else {
                0.65
            };

            if exact_plan
                || (files_overlap
                    && jaccard >= sim_threshold
                    && words.len() >= 4
                    && prev.content_words.len() >= 4)
            {
                self.consecutive_same_plan_turns += 1;
                if self.consecutive_same_plan_turns >= 2 {
                    return ReasoningLoopStatus::LoopDetected(DIAG_CROSS_TURN_SAME_PLAN);
                }
            } else {
                self.consecutive_same_plan_turns = 1;
            }
        } else {
            self.consecutive_same_plan_turns = 1;
        }

        // 2. Ready-to-implement hesitation loop: "ready to implement -> one more check -> same plan"
        if (has_ready_intent || (self.consecutive_hesitation_turns > 0 && has_hesitation_intent))
            && !evidence.had_edits
            && evidence.tool_count > 0
        {
            if let Some(prev) = self.recent_turns.back() {
                if prev.has_ready_intent || prev.has_hesitation_intent {
                    self.consecutive_hesitation_turns += 1;
                    if self.consecutive_hesitation_turns >= 2 {
                        return ReasoningLoopStatus::LoopDetected(DIAG_SEMANTIC_NO_PROGRESS);
                    }
                } else {
                    self.consecutive_hesitation_turns = 1;
                }
            } else {
                self.consecutive_hesitation_turns = 1;
            }
        } else if evidence.had_edits {
            self.consecutive_hesitation_turns = 0;
        }

        // 3. Repeated reads over same small set of files without workspace edits
        if !evidence.had_edits && evidence.tool_count > 0 && !target_files.is_empty() {
            let mut all_targets = target_files.clone();
            let mut total_tools = evidence.tool_count;
            for record in &self.recent_turns {
                all_targets.extend(record.target_files.iter().cloned());
                total_tools += record.tool_count;
            }

            if self.recent_turns.len() >= 2 && all_targets.len() <= 2 && total_tools >= 3 {
                return ReasoningLoopStatus::LoopDetected(DIAG_SAME_FILES_NO_PROGRESS);
            }
        } else if evidence.had_edits {
            self.consecutive_small_file_set_turns = 0;
        }

        self.recent_turns.push_back(TurnReasoningRecord {
            content_words: words,
            plan_hash,
            target_files,
            has_ready_intent,
            has_hesitation_intent,
            had_edits: evidence.had_edits,
            tool_count: evidence.tool_count,
        });

        const MAX_RECENT_TURNS: usize = 8;
        while self.recent_turns.len() > MAX_RECENT_TURNS {
            self.recent_turns.pop_front();
        }

        ReasoningLoopStatus::Ok
    }

    /// Record turn reasoning to detect "plan -> inspect -> same plan" cycles across turns.
    pub fn record_turn_reasoning(
        &mut self,
        reasoning: &str,
        made_progress: bool,
    ) -> ReasoningLoopStatus {
        self.record_turn_evidence(&TurnEvidence {
            reasoning,
            target_files: &[],
            made_progress,
            had_edits: made_progress,
            tool_count: if made_progress { 1 } else { 1 },
            no_progress_streak: if made_progress { 0 } else { 1 },
        })
    }

    fn observe_sentence(&mut self, s: &str) -> ReasoningLoopStatus {
        let Some(norm) = normalize_sentence(s) else {
            return ReasoningLoopStatus::Ok;
        };
        let hash = stable_hash(&norm);

        // 1. Consecutive repetition
        if self.consecutive_sentence.0 == Some(hash) {
            self.consecutive_sentence.1 += 1;
            if self.consecutive_sentence.1 >= 3 {
                return ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK);
            }
        } else {
            self.consecutive_sentence = (Some(hash), 1);
        }

        // 2. Sliding window recording
        self.recent_sentences.push_back(hash);
        *self.sentence_counts.entry(hash).or_insert(0) += 1;

        const SENTENCE_WINDOW_SIZE: usize = 16;
        if self.recent_sentences.len() > SENTENCE_WINDOW_SIZE {
            if let Some(old) = self.recent_sentences.pop_front()
                && let std::collections::hash_map::Entry::Occupied(mut entry) =
                    self.sentence_counts.entry(old)
            {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
        }

        // 3. Sliding window frequency check
        if let Some(&count) = self.sentence_counts.get(&hash)
            && count >= 3
        {
            return ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK);
        }

        // 4. Alternating cycle checks
        let n = self.recent_sentences.len();
        if n >= 6 {
            let s0 = self.recent_sentences[n - 1];
            let s1 = self.recent_sentences[n - 2];
            let s2 = self.recent_sentences[n - 3];
            let s3 = self.recent_sentences[n - 4];
            let s4 = self.recent_sentences[n - 5];
            let s5 = self.recent_sentences[n - 6];

            // 2-cycle: A, B, A, B, A, B
            if s0 == s2 && s2 == s4 && s1 == s3 && s1 == s5 && s0 != s1 {
                return ReasoningLoopStatus::LoopDetected(DIAG_CYCLE);
            }

            // 3-cycle: A, B, C, A, B, C
            if s0 == s3 && s1 == s4 && s2 == s5 && s0 != s1 && s1 != s2 && s0 != s2 {
                if n >= 9 {
                    let s6 = self.recent_sentences[n - 7];
                    let s7 = self.recent_sentences[n - 8];
                    let s8 = self.recent_sentences[n - 9];
                    if s0 == s6 && s1 == s7 && s2 == s8 {
                        return ReasoningLoopStatus::LoopDetected(DIAG_CYCLE);
                    }
                }
            }
        }

        ReasoningLoopStatus::Ok
    }

    fn observe_paragraph(&mut self, p: &str) -> ReasoningLoopStatus {
        let trimmed = p.trim();
        if trimmed.len() < 60 {
            return ReasoningLoopStatus::Ok;
        }
        let cleaned: String = trimmed
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let hash = stable_hash(&cleaned);
        let p_words = extract_content_words(trimmed);

        // Check semantic similarity against recent paragraphs
        for (prev_hash, prev_words) in &self.recent_paragraphs {
            if *prev_hash != hash && p_words.len() >= 8 && prev_words.len() >= 8 {
                let sim = jaccard_similarity(&p_words, prev_words);
                if sim >= 0.80 {
                    return ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK);
                }
            }
        }

        self.recent_paragraphs.push_back((hash, p_words));
        *self.paragraph_counts.entry(hash).or_insert(0) += 1;

        const PARAGRAPH_WINDOW_SIZE: usize = 8;
        if self.recent_paragraphs.len() > PARAGRAPH_WINDOW_SIZE {
            if let Some((old_hash, _)) = self.recent_paragraphs.pop_front()
                && let std::collections::hash_map::Entry::Occupied(mut entry) =
                    self.paragraph_counts.entry(old_hash)
            {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
        }

        if let Some(&count) = self.paragraph_counts.get(&hash) {
            if cleaned.len() >= 150 && count >= 2 {
                return ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK);
            }
            if count >= 3 {
                return ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK);
            }
        }

        // Alternating paragraph cycle check
        let n = self.recent_paragraphs.len();
        if n >= 4 {
            let p0 = &self.recent_paragraphs[n - 1].1;
            let p1 = &self.recent_paragraphs[n - 2].1;
            let p2 = &self.recent_paragraphs[n - 3].1;
            let p3 = &self.recent_paragraphs[n - 4].1;
            if p0.len() >= 6 && p1.len() >= 6 && p2.len() >= 6 && p3.len() >= 6 {
                let sim_0_2 = jaccard_similarity(p0, p2);
                let sim_1_3 = jaccard_similarity(p1, p3);
                let sim_0_1 = jaccard_similarity(p0, p1);
                if sim_0_2 >= 0.70 && sim_1_3 >= 0.70 && sim_0_1 < 0.50 {
                    return ReasoningLoopStatus::LoopDetected(DIAG_CYCLE);
                }
            }
        }

        ReasoningLoopStatus::Ok
    }

    fn observe_text_sentences(&mut self, text: &str) -> ReasoningLoopStatus {
        for part in text.split(&['\n', '.', '!', '?'][..]) {
            let status = self.observe_sentence(part);
            if status != ReasoningLoopStatus::Ok {
                return status;
            }
        }
        ReasoningLoopStatus::Ok
    }
}

fn find_sentence_boundary(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            return Some(i + 1);
        }
        if (b == b'.' || b == b'!' || b == b'?') && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            if next.is_ascii_whitespace() {
                return Some(i + 2);
            }
        }
        if b == b';' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            return Some(i + 2);
        }
    }
    None
}

fn normalize_sentence(s: &str) -> Option<String> {
    let trimmed = s.trim();
    let stripped = trimmed
        .trim_start_matches(|c: char| {
            c == '-' || c == '*' || c == '#' || c.is_ascii_digit() || c == '.' || c == ')'
        })
        .trim();
    let cleaned: String = stripped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let cleaned = cleaned.trim_matches(|c: char| {
        c == '.'
            || c == ','
            || c == '!'
            || c == '?'
            || c == ':'
            || c == ';'
            || c == '"'
            || c == '\''
            || c == '`'
    });

    if cleaned.len() >= 25 && cleaned.split_whitespace().count() >= 4 {
        Some(cleaned.to_string())
    } else {
        None
    }
}

/// Extract significant content words from text by lowercasing, stripping punctuation,
/// and removing standard stop words.
pub fn extract_content_words(text: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a",
        "about",
        "above",
        "after",
        "again",
        "against",
        "all",
        "am",
        "an",
        "and",
        "any",
        "are",
        "aren't",
        "as",
        "at",
        "be",
        "because",
        "been",
        "before",
        "being",
        "below",
        "between",
        "both",
        "but",
        "by",
        "can",
        "can't",
        "cannot",
        "could",
        "couldn't",
        "did",
        "didn't",
        "do",
        "does",
        "doesn't",
        "doing",
        "don't",
        "down",
        "during",
        "each",
        "few",
        "for",
        "from",
        "further",
        "had",
        "hadn't",
        "has",
        "hasn't",
        "have",
        "haven't",
        "having",
        "he",
        "he'd",
        "he'll",
        "he's",
        "her",
        "here",
        "here's",
        "hers",
        "herself",
        "him",
        "himself",
        "his",
        "how",
        "how's",
        "i",
        "i'd",
        "i'll",
        "i'm",
        "i've",
        "if",
        "in",
        "into",
        "is",
        "isn't",
        "it",
        "it's",
        "its",
        "itself",
        "let",
        "let's",
        "me",
        "more",
        "most",
        "mustn't",
        "my",
        "myself",
        "no",
        "nor",
        "not",
        "of",
        "off",
        "on",
        "once",
        "only",
        "or",
        "other",
        "ought",
        "our",
        "ours",
        "ourselves",
        "out",
        "over",
        "own",
        "same",
        "shan't",
        "she",
        "she'd",
        "she'll",
        "she's",
        "should",
        "shouldn't",
        "so",
        "some",
        "such",
        "than",
        "that",
        "that's",
        "the",
        "their",
        "theirs",
        "them",
        "themselves",
        "then",
        "there",
        "there's",
        "these",
        "they",
        "they'd",
        "they'll",
        "they're",
        "they've",
        "this",
        "those",
        "through",
        "to",
        "too",
        "under",
        "until",
        "up",
        "very",
        "was",
        "wasn't",
        "we",
        "we'd",
        "we'll",
        "we're",
        "we've",
        "were",
        "weren't",
        "what",
        "what's",
        "when",
        "when's",
        "where",
        "where's",
        "which",
        "while",
        "who",
        "who's",
        "whom",
        "why",
        "why's",
        "with",
        "won't",
        "would",
        "wouldn't",
        "you",
        "you'd",
        "you'll",
        "you're",
        "you've",
        "your",
        "yours",
        "yourself",
        "yourselves",
        "will",
        "just",
        "also",
        "now",
        "well",
        "see",
        "okay",
        "first",
        "next",
    ];

    let mut words = HashSet::new();
    for raw in text.split_whitespace() {
        let cleaned = raw
            .trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '_' && c != '/' && c != '.' && c != '-'
            })
            .to_lowercase();
        if cleaned.len() >= 3 && !STOP_WORDS.contains(&cleaned.as_str()) {
            let normalized = match cleaned.as_str() {
                "modify" | "modified" | "modifying" | "alter" | "altered" | "altering" | "edit"
                | "edited" | "editing" | "update" | "updated" | "updating" | "change"
                | "changed" | "changing" | "implement" | "implemented" | "implementing"
                | "patch" | "patching" | "apply" | "applying" | "applied" | "write" | "writing"
                | "written" | "add" | "adding" | "added" => "__edit_action__".to_string(),
                "inspect" | "inspecting" | "inspected" | "examine" | "examining" | "examined"
                | "check" | "checking" | "checked" | "verify" | "verifying" | "verified"
                | "view" | "viewing" | "viewed" | "read" | "reading" | "review" | "reviewing"
                | "reviewed" | "analyze" | "analyzing" | "analyzed" | "explore" | "exploring"
                | "explored" => "__inspect_action__".to_string(),
                w if w.len() > 4
                    && w.ends_with('s')
                    && !w.ends_with("ss")
                    && !w.ends_with(".rs")
                    && !w.ends_with(".ts")
                    && !w.ends_with(".js") =>
                {
                    w[..w.len() - 1].to_string()
                }
                w => w.to_string(),
            };
            words.insert(normalized);
        }
    }
    words
}

/// Compute Jaccard similarity between two sets of content words.
pub fn jaccard_similarity(set_a: &HashSet<String>, set_b: &HashSet<String>) -> f64 {
    if set_a.is_empty() || set_b.is_empty() {
        return 0.0;
    }
    let intersection = set_a.intersection(set_b).count();
    let union = set_a.union(set_b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Extract candidate target file paths from text.
pub fn extract_target_files(text: &str) -> HashSet<String> {
    let mut files = HashSet::new();
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            c == '`'
                || c == '\''
                || c == '"'
                || c == '('
                || c == ')'
                || c == '['
                || c == ']'
                || c == '{'
                || c == '}'
                || c == '<'
                || c == '>'
                || c == ','
                || c == ';'
                || c == ':'
        });
        if (trimmed.contains('/')
            || trimmed.ends_with(".rs")
            || trimmed.ends_with(".ts")
            || trimmed.ends_with(".js")
            || trimmed.ends_with(".py")
            || trimmed.ends_with(".go")
            || trimmed.ends_with(".toml")
            || trimmed.ends_with(".json")
            || trimmed.ends_with(".md"))
            && !trimmed.starts_with("http://")
            && !trimmed.starts_with("https://")
            && trimmed.len() >= 3
        {
            files.insert(trimmed.to_lowercase());
        }
    }
    files
}

/// Check if text expresses explicit readiness to implement or make workspace changes.
pub fn detect_ready_intent(text: &str) -> bool {
    let lower = text.to_lowercase();
    const READY_PHRASES: &[&str] = &[
        "ready to implement",
        "ready to apply",
        "ready to edit",
        "ready to write",
        "ready to make changes",
        "ready to modify",
        "proceed with implementation",
        "proceed with editing",
        "proceed with modifying",
        "proceed with changes",
        "proceed with the edit",
        "now make the change",
        "now implement",
        "time to edit",
        "time to implement",
        "let's implement",
        "let's apply",
        "let's edit",
        "will now implement",
        "will now edit",
        "will now modify",
        "will now apply",
        "start implementing",
        "ready to make the change",
        "let me write the code",
        "let me write the implementation",
        "write the code now",
        "write the implementation now",
        "create the files now",
        "now let me create the file",
    ];
    READY_PHRASES.iter().any(|phrase| lower.contains(phrase))
}

/// Check if text expresses hesitation or "one more check" before acting.
pub fn detect_hesitation_intent(text: &str) -> bool {
    let lower = text.to_lowercase();
    const HESITATION_PHRASES: &[&str] = &[
        "one more check",
        "one last check",
        "quick check",
        "double check",
        "verify before",
        "check first",
        "let me verify",
        "before modifying",
        "before editing",
        "before applying",
        "check again",
        "just to be sure",
        "let's verify",
        "let's check",
        "confirm before",
    ];
    HESITATION_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
}
