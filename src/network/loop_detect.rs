//! Loop detection for the agent's tool-call loop.
//!
//! The orchestrator can spin — especially in continuous mode — retrying the
//! same intent with cosmetic variations, or alternating between two useless
//! actions. Exact-repeat matching alone misses those. This detector runs four
//! independent signals and reports the worst:
//!
//! 1. **Exact** — identical tool signature back-to-back (trivial loops)
//! 2. **Category** — normalized signature (same intent, different flags/quotes)
//! 3. **Output** — identical tool output despite varied commands (stagnation)
//! 4. **Frequency** — one action dominating a sliding window (A→B→A→B churn)
//!
//! Adapted from the 4-tier detector in the sibling `rust-code` project, keyed
//! to this crate's `(tool_name, args)` model instead of `bash:` strings.

use serde_json::Value;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Search binaries collapsed into a single `search:` category so
/// `grep`/`rg`/`ag` variants of the same query count as one intent.
const SEARCH_BINS: &[&str] = &["rg", "grep", "ag", "ack", "fgrep", "egrep"];

/// Build `(exact_signature, category)` for a tool call.
///
/// `exact` distinguishes every distinct call; `category` strips syntactic
/// noise so semantically-identical retries collapse together. For
/// `run_command` the shell string is normalized (flags/quotes/chains removed);
/// other tools reuse their exact signature as the category.
pub fn signatures(name: &str, args: &Value) -> (String, String) {
    let exact = format!("{name}:{}", serde_json::to_string(args).unwrap_or_default());
    let category = if name == "run_command" {
        match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => normalize_command(cmd),
            None => exact.clone(),
        }
    } else {
        exact.clone()
    };
    (exact, category)
}

/// Reduce a shell command to its semantic core: primary command before any
/// `||`/`&&`/`;`/`|`, flags dropped, arguments unquoted and de-slashed.
/// Search tools normalize to `search:<args>` so all grep/rg variants match.
fn normalize_command(cmd: &str) -> String {
    // Isolate the primary command (spaces around separators avoid matching
    // operators inside quoted patterns like 'TODO|FIXME').
    let core = [" || ", " && ", " ; ", " | "]
        .iter()
        .fold(cmd, |acc, sep| acc.split(sep).next().unwrap_or(acc))
        .trim();

    let tokens: Vec<&str> = core.split_whitespace().collect();
    if tokens.is_empty() {
        return "cmd:".into();
    }
    let bin = tokens[0];
    let arg_str = tokens[1..]
        .iter()
        .filter(|t| !t.starts_with('-'))
        .map(|t| {
            t.trim_matches(|c: char| c == '\'' || c == '"')
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if SEARCH_BINS.contains(&bin) {
        format!("search:{arg_str}")
    } else if arg_str.is_empty() {
        format!("cmd:{bin}")
    } else {
        format!("cmd:{bin}:{arg_str}")
    }
}

/// Tracks consecutive repeats of a string value.
#[derive(Default)]
struct ConsecutiveTracker {
    last: Option<String>,
    count: usize,
}

impl ConsecutiveTracker {
    fn record(&mut self, value: &str) -> usize {
        if self.last.as_deref() == Some(value) {
            self.count += 1;
        } else {
            self.last = Some(value.to_string());
            self.count = 1;
        }
        self.count
    }
}

/// Tracks consecutive repeats by hash (for large values like tool output).
#[derive(Default)]
struct HashTracker {
    last: Option<u64>,
    count: usize,
}

impl HashTracker {
    fn record(&mut self, value: &str) -> usize {
        let mut h = DefaultHasher::new();
        value.hash(&mut h);
        let hash = h.finish();
        if self.last == Some(hash) {
            self.count += 1;
        } else {
            self.last = Some(hash);
            self.count = 1;
        }
        self.count
    }
}

/// Tracks the max frequency of any value in a sliding window — catches
/// alternating loops that consecutive tracking misses.
struct FrequencyTracker {
    window: Vec<String>,
    size: usize,
}

impl FrequencyTracker {
    fn new(size: usize) -> Self {
        Self { window: Vec::new(), size }
    }

    fn record(&mut self, value: &str) -> usize {
        self.window.push(value.to_string());
        if self.window.len() > self.size {
            self.window.remove(0);
        }
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for v in &self.window {
            *counts.entry(v.as_str()).or_insert(0) += 1;
        }
        counts.values().copied().max().unwrap_or(0)
    }
}

/// Outcome of a detector check.
#[derive(Debug, PartialEq)]
pub enum LoopStatus {
    /// No repetition worth acting on.
    Ok,
    /// Repeating, past the warn threshold — nudge the model. Holds repeat count.
    Warning(usize),
    /// Repeating past the abort threshold — stop auto-execution. Holds count.
    Abort(usize),
}

impl LoopStatus {
    /// Ordering rank so callers can keep the worst status across tool calls.
    pub fn rank(&self) -> u8 {
        match self {
            LoopStatus::Ok => 0,
            LoopStatus::Warning(_) => 1,
            LoopStatus::Abort(_) => 2,
        }
    }
}

/// Four-signal repetition detector. One instance per user task.
pub struct LoopDetector {
    exact: ConsecutiveTracker,
    category: ConsecutiveTracker,
    output: HashTracker,
    frequency: FrequencyTracker,
    warn: usize,
    abort: usize,
}

impl LoopDetector {
    /// Warns at `⌈abort/2⌉`, aborts at `abort`. Frequency window is `abort*2`
    /// so alternating patterns have room to build up.
    pub fn new(abort: usize) -> Self {
        Self {
            exact: ConsecutiveTracker::default(),
            category: ConsecutiveTracker::default(),
            output: HashTracker::default(),
            frequency: FrequencyTracker::new(abort * 2),
            warn: abort.div_ceil(2),
            abort,
        }
    }

    /// Record one tool call. Returns the worst of the exact, category, and
    /// frequency signals.
    pub fn check(&mut self, exact: &str, category: &str) -> LoopStatus {
        let n = self
            .exact
            .record(exact)
            .max(self.category.record(category))
            .max(self.frequency.record(category));
        self.classify(n)
    }

    /// Record a tool output and check for stagnation (same result repeatedly).
    pub fn record_output(&mut self, output: &str) -> LoopStatus {
        let n = self.output.record(output);
        self.classify(n)
    }

    fn classify(&self, n: usize) -> LoopStatus {
        if n >= self.abort {
            LoopStatus::Abort(n)
        } else if n >= self.warn {
            LoopStatus::Warning(n)
        } else {
            LoopStatus::Ok
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_variants_share_category() {
        let (_, a) = signatures("run_command", &json!({"command": "rg -n 'TODO|FIXME' src/"}));
        let (_, b) = signatures("run_command", &json!({"command": "grep -rnE \"TODO|FIXME\" src/ || echo none"}));
        assert_eq!(a, b);
        assert_eq!(a, "search:TODO|FIXME src");
    }

    #[test]
    fn non_bash_tool_uses_exact() {
        let (exact, cat) = signatures("view_file", &json!({"path": "src/main.rs"}));
        assert_eq!(exact, cat);
        assert!(exact.starts_with("view_file:"));
    }

    #[test]
    fn exact_repeat_warns_then_aborts() {
        let mut d = LoopDetector::new(6); // warn at 3, abort at 6
        for _ in 0..2 {
            assert_eq!(d.check("x", "x"), LoopStatus::Ok);
        }
        assert_eq!(d.check("x", "x"), LoopStatus::Warning(3));
        for _ in 0..2 {
            let _ = d.check("x", "x");
        }
        assert_eq!(d.check("x", "x"), LoopStatus::Abort(6));
    }

    #[test]
    fn semantic_loop_caught_across_syntax() {
        let mut d = LoopDetector::new(4); // warn at 2, abort at 4
        let cmds = [
            "rg -n 'TODO' src/",
            "rg 'TODO' src/",
            "rg -i 'TODO' src/",
            "grep -rn 'TODO' src/",
        ];
        let results: Vec<LoopStatus> = cmds
            .iter()
            .map(|c| {
                let (e, cat) = signatures("run_command", &json!({ "command": c }));
                d.check(&e, &cat)
            })
            .collect();
        assert_eq!(results[0], LoopStatus::Ok);
        assert_eq!(results[3], LoopStatus::Abort(4));
    }

    #[test]
    fn alternating_churn_caught_by_frequency() {
        let mut d = LoopDetector::new(4); // window = 8
        let mut last = LoopStatus::Ok;
        for i in 0..8 {
            let cmd = if i % 2 == 0 { "cat a.rs" } else { "pwd" };
            let (e, cat) = signatures("run_command", &json!({ "command": cmd }));
            last = d.check(&e, &cat);
        }
        assert_eq!(last, LoopStatus::Abort(4));
    }

    #[test]
    fn output_stagnation() {
        let mut d = LoopDetector::new(4);
        assert_eq!(d.record_output("no matches"), LoopStatus::Ok);
        assert_eq!(d.record_output("no matches"), LoopStatus::Warning(2));
        assert_eq!(d.record_output("no matches"), LoopStatus::Warning(3));
        assert_eq!(d.record_output("no matches"), LoopStatus::Abort(4));
    }
}
