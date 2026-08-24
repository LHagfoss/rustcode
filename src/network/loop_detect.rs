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
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// Search binaries collapsed into a single `search:` category so
/// `grep`/`rg`/`ag` variants of the same query count as one intent.
const SEARCH_BINS: &[&str] = &["rg", "grep", "ag", "ack", "fgrep", "egrep"];

/// Read-only tools inspect state without mutating it. Re-reading the region
/// you're actively editing — to find a unique anchor, or verify an edit landed
/// — is normal recovery, not a runaway loop. Repeats of these should nudge the
/// model, not disable its tools, so they're capped at `Warning` unless they
/// spin far past the abort threshold (a genuine hang).
pub fn is_read_only(name: &str) -> bool {
    matches!(
        name,
        "view_file" | "read_file" | "grep" | "list_directory" | "glob" | "find_symbol"
    )
}

/// Safe Git inspection commands are read-only even though they travel through
/// the general-purpose shell tool. Treating every `run_command` call as a
/// mutation makes harmless release inspection (`git log`, `git status`, etc.)
/// escalate into a tool shutdown when a model repeats a query.
fn is_read_only_category(name: &str, category: &str) -> bool {
    if is_read_only(name) {
        return true;
    }
    if name != "run_command" {
        return false;
    }

    let Some(git_args) = category.strip_prefix("cmd:git") else {
        return false;
    };
    let subcommand = git_args.trim_start_matches(':').split_whitespace().next();
    matches!(
        subcommand,
        None | Some(
            "branch"
                | "describe"
                | "diff"
                | "log"
                | "ls-files"
                | "remote"
                | "rev-parse"
                | "show"
                | "status"
                | "tag"
        )
    )
}

/// Classify a shell command that only inspects stable repository state. These
/// checks may legitimately repeat while the model is orienting itself, so the
/// progress ledger should not treat their identical output as a failed edit.
pub fn is_stable_inspection_command(command: &str) -> bool {
    is_read_only_category("run_command", &normalize_command(command))
}

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
    } else if name == "view_file" {
        // Re-reading the *same region* of a file is a loop; reading *different*
        // regions to collect scattered code is legitimate work. Bucket the start
        // line coarsely (per 200 lines) so cosmetic ±N range shifts over the same
        // area collapse to one category, while genuinely distinct parts of a big
        // file stay distinct and don't trip the detector prematurely.
        match args
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            Some(path) => {
                let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(0);
                format!("view_file:{path}#{}", start / 200)
            }
            None => exact.clone(),
        }
    } else if name == "grep" {
        let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let path = args
            .get("path")
            .or_else(|| args.get("include"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        format!("grep:{pattern}@{path}")
    } else if matches!(name, "list_directory" | "glob" | "find_symbol") {
        let target = args
            .get("pattern")
            .or_else(|| args.get("path"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        match target {
            Some(t) => format!("{name}:{t}"),
            None => exact.clone(),
        }
    } else if matches!(
        name,
        "replace_file_content"
            | "multi_replace_file_content"
            | "write_to_file"
            | "delete_file"
            | "move_file"
            | "copy_file"
    ) {
        let path = args
            .get("path")
            .or_else(|| args.get("target_file"))
            .or_else(|| args.get("TargetFile"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let start = args
            .get("start_line")
            .and_then(crate::tools::parse_json_number)
            .or_else(|| {
                args.get("edits")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("start_line"))
                    .and_then(crate::tools::parse_json_number)
            })
            .or_else(|| {
                args.get("replacements")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("start_line"))
                    .and_then(crate::tools::parse_json_number)
            });
        if let Some(st) = start {
            format!("edit:{path}#{}", st / 200)
        } else {
            format!("edit:{path}")
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
    // Isolate the primary substantive command (spaces around separators avoid
    // matching operators inside quoted patterns like 'TODO|FIXME'). A leading
    // `cd … &&` is setup, not the action: collapsing every workspace command
    // to `cmd:cd:<path>` creates false loop warnings across unrelated checks.
    let core = cmd
        .split(" && ")
        .flat_map(|segment| segment.split(" || "))
        .flat_map(|segment| segment.split(" ; "))
        .flat_map(|segment| segment.split(" | "))
        .map(str::trim)
        .find(|segment| {
            let mut tokens = segment.split_whitespace();
            !matches!(tokens.next(), Some("cd"))
        })
        .unwrap_or("");

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

/// Collapse outputs that differ only in the query they report on into one
/// stagnation key. A model re-rolling search terms ("no matches for 'foo'",
/// "no matches for 'bar'") is stuck exactly as surely as one repeating the
/// identical call, but exact-output hashing never sees it.
pub fn stagnation_key(output: &str) -> &str {
    if output.starts_with("no matches for '") {
        "grep:no-matches"
    } else {
        output
    }
}

/// Compact evidence from one tool result. The ledger deliberately receives
/// fingerprints and booleans, never the full result, so loop diagnostics do
/// not become a second transcript or leak source text into operational logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressObservation {
    pub action: String,
    pub output_fingerprint: u64,
    pub state_fingerprint: Option<u64>,
    pub failure_fingerprint: Option<u64>,
    pub changed_workspace: bool,
    pub fresh_read: bool,
    pub search_result: bool,
    pub no_result: bool,
    pub verification: bool,
    pub read_only: bool,
    pub replayed: bool,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressReason {
    WorkspaceChanged,
    NewInformation,
    FreshRead,
    Verification,
    RepeatedFailure,
    NoNewInformation,
    Churn,
}

impl ProgressReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::WorkspaceChanged => "workspace_changed",
            Self::NewInformation => "new_information",
            Self::FreshRead => "fresh_read",
            Self::Verification => "verification",
            Self::RepeatedFailure => "repeated_failure",
            Self::NoNewInformation => "no_new_information",
            Self::Churn => "edit_test_revert_churn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressAssessment {
    pub meaningful: bool,
    pub reason: ProgressReason,
    pub streak: usize,
    /// Stable successful verification is useful evidence but is not a reason
    /// to abort merely because a command printed the same output again.
    pub suppress_stagnation: bool,
}

/// A bounded cross-round ledger for evidence-aware progress detection.
///
/// `LoopDetector` remains responsible for repeated-call safety. This ledger
/// answers the complementary question: did the tool result add workspace or
/// task information? Keeping the two signals separate lets a legitimate
/// re-read after an edit count as progress without weakening exact-call
/// detection for mutating tools.
#[derive(Debug, Clone)]
pub struct ProgressLedger {
    seen_outputs: HashSet<u64>,
    seen_failures: HashSet<u64>,
    recent_states: VecDeque<u64>,
    no_progress_streak: usize,
}

impl Default for ProgressLedger {
    fn default() -> Self {
        Self {
            seen_outputs: HashSet::new(),
            seen_failures: HashSet::new(),
            recent_states: VecDeque::with_capacity(4),
            no_progress_streak: 0,
        }
    }
}

impl ProgressLedger {
    const MAX_FINGERPRINTS: usize = 128;
    pub const RECOVERY_STREAK: usize = 3;

    pub fn observe(&mut self, observation: &ProgressObservation) -> ProgressAssessment {
        // Include the normalized action in the novelty key. Two different
        // successful commands often produce the same empty/stdout text, and
        // treating that as a replay would misclassify legitimate progress.
        // Exact and semantic repeats are still handled by LoopDetector and by
        // the repeated key for the same action here.
        let action_output = stable_hash(&format!(
            "{}:{}",
            observation.action, observation.output_fingerprint
        ));
        let new_output = remember(&mut self.seen_outputs, action_output);
        let new_failure = observation
            .failure_fingerprint
            .is_some_and(|hash| remember(&mut self.seen_failures, hash));

        let state_hash = observation.state_fingerprint;
        let state_changed =
            state_hash.is_some_and(|state| self.recent_states.back().copied() != Some(state));
        let churn = state_hash.is_some_and(|state| {
            self.recent_states.len() >= 2
                && self.recent_states.iter().rev().nth(1).copied() == Some(state)
                && self.recent_states.back().copied() != Some(state)
        });
        if let Some(state) = state_hash {
            self.recent_states.push_back(state);
            while self.recent_states.len() > 4 {
                self.recent_states.pop_front();
            }
        }

        let stable_verification = observation.verification && observation.success;
        let (meaningful, reason) = if churn {
            (false, ProgressReason::Churn)
        } else if observation.changed_workspace {
            (true, ProgressReason::WorkspaceChanged)
        } else if observation.read_only && observation.replayed {
            (false, ProgressReason::NoNewInformation)
        } else if observation.fresh_read && new_output {
            (true, ProgressReason::FreshRead)
        } else if observation.search_result && observation.success && observation.no_result {
            (false, ProgressReason::NoNewInformation)
        } else if observation.search_result && new_output {
            (true, ProgressReason::NewInformation)
        } else if stable_verification && new_output {
            (true, ProgressReason::Verification)
        } else if !observation.success && new_failure {
            (true, ProgressReason::NewInformation)
        } else if !observation.success {
            (false, ProgressReason::RepeatedFailure)
        } else if new_output {
            (true, ProgressReason::NewInformation)
        } else if !state_changed {
            (false, ProgressReason::NoNewInformation)
        } else {
            (true, ProgressReason::NewInformation)
        };

        if meaningful {
            self.no_progress_streak = 0;
        } else if !stable_verification {
            self.no_progress_streak = self.no_progress_streak.saturating_add(1);
        }

        ProgressAssessment {
            meaningful,
            reason,
            streak: self.no_progress_streak,
            // A successful verification may repeat harmlessly. A failed
            // verification must remain visible to the failure/stagnation
            // guards so an agent cannot loop forever on the same broken test.
            suppress_stagnation: stable_verification,
        }
    }

    pub fn no_progress_streak(&self) -> usize {
        self.no_progress_streak
    }
}

fn remember(values: &mut HashSet<u64>, value: u64) -> bool {
    if values.len() >= ProgressLedger::MAX_FINGERPRINTS {
        // HashSet iteration order is intentionally irrelevant here; clear the
        // bounded novelty window rather than retaining unbounded state.
        values.clear();
    }
    values.insert(value)
}

pub fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub fn is_search_tool(name: &str) -> bool {
    matches!(name, "grep" | "glob" | "find_symbol" | "list_directory")
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
        Self {
            window: Vec::new(),
            size,
        }
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
    failed_exact: ConsecutiveTracker,
    failed_category: ConsecutiveTracker,
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
            failed_exact: ConsecutiveTracker::default(),
            failed_category: ConsecutiveTracker::default(),
            output: HashTracker::default(),
            frequency: FrequencyTracker::new(abort * 2),
            warn: abort.div_ceil(2),
            abort,
        }
    }

    /// Record one tool call. Returns the worst of the exact, category, and
    /// frequency signals.
    pub fn check(&mut self, exact: &str, category: &str) -> LoopStatus {
        let exact_count = self.exact.record(exact);
        if exact_count >= 3 {
            return LoopStatus::Abort(exact_count);
        }
        let n = exact_count
            .max(self.category.record(category))
            .max(self.frequency.record(category));
        self.classify(n)
    }

    /// Like [`check`], but softens the verdict for read-only tools: their
    /// repeats only *warn* (nudging the model to change approach) instead of
    /// aborting and disabling tools — unless they spin to 3× the abort
    /// threshold, which is a real hang rather than legitimate re-reading.
    pub fn check_tool(&mut self, name: &str, exact: &str, category: &str) -> LoopStatus {
        let status = self.check(exact, category);
        if is_read_only_category(name, category)
            && let LoopStatus::Abort(n) = status
            && n < self.abort.saturating_mul(3)
        {
            return LoopStatus::Warning(n);
        }
        status
    }

    /// Record a failed mutation independently from ordinary call repetition.
    /// Two equivalent failures are enough to require a replan: retrying the
    /// same failed edit is unlikely to discover new workspace facts, even when
    /// the model changes cosmetic arguments between attempts.
    pub fn record_failed_tool(&mut self, exact: &str, category: &str) -> LoopStatus {
        let exact_count = self.failed_exact.record(exact);
        let category_count = self.failed_category.record(category);
        let repeats = exact_count.max(category_count);
        if repeats >= 2 {
            LoopStatus::Abort(repeats)
        } else {
            LoopStatus::Ok
        }
    }

    /// Clear all repetition state. Called when the agent makes real progress —
    /// a successful mutating tool — so post-edit re-reads start from a clean
    /// slate instead of inheriting the pre-edit read history that would
    /// otherwise trip the frequency signal mid-task.
    pub fn reset(&mut self) {
        self.exact = ConsecutiveTracker::default();
        self.category = ConsecutiveTracker::default();
        self.failed_exact = ConsecutiveTracker::default();
        self.failed_category = ConsecutiveTracker::default();
        self.output = HashTracker::default();
        self.frequency = FrequencyTracker::new(self.abort * 2);
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
        let (_, a) = signatures(
            "run_command",
            &json!({"command": "rg -n 'TODO|FIXME' src/"}),
        );
        let (_, b) = signatures(
            "run_command",
            &json!({"command": "grep -rnE \"TODO|FIXME\" src/ || echo none"}),
        );
        assert_eq!(a, b);
        assert_eq!(a, "search:TODO|FIXME src");
    }

    #[test]
    fn view_file_range_shifting_shares_category() {
        // Same file, different line ranges = one intent. Range-shifting must not
        // dodge the loop detector.
        let (e1, c1) = signatures(
            "view_file",
            &json!({"path": "src/network.rs", "start_line": 1, "end_line": 100}),
        );
        let (e2, c2) = signatures(
            "view_file",
            &json!({"path": "src/network.rs", "start_line": 50, "end_line": 150}),
        );
        assert_ne!(e1, e2, "exact signatures should differ by range");
        assert_eq!(c1, c2, "same region should collapse to one category");
        assert_eq!(c1, "view_file:src/network.rs#0");
    }

    #[test]
    fn view_file_distinct_regions_stay_distinct() {
        // Reading far-apart parts of a big file is legit paging, not a loop.
        let (_, c1) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": 40, "end_line": 240}),
        );
        let (_, c2) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": 1400, "end_line": 1600}),
        );
        assert_ne!(c1, c2, "distinct regions must not share a category");
    }

    #[test]
    fn view_file_same_region_churn_aborts() {
        let mut d = LoopDetector::new(4); // warn at 2, abort at 4
        let mut last = LoopStatus::Ok;
        // Cosmetic shifts over the same ~250-region: all bucket 1.
        for start in [250, 260, 250, 255] {
            let (e, c) = signatures(
                "view_file",
                &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
            );
            last = d.check(&e, &c);
        }
        assert_eq!(last, LoopStatus::Abort(4));
    }

    #[test]
    fn edit_tool_normalizes_category_to_target_path() {
        let (exact, cat) = signatures(
            "replace_file_content",
            &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b"}),
        );
        assert_ne!(exact, cat);
        assert_eq!(cat, "edit:src/ui/mod.rs");
    }

    #[test]
    fn edit_tool_buckets_category_by_start_line() {
        let (_, cat1) = signatures(
            "replace_file_content",
            &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b", "start_line": 50}),
        );
        let (_, cat2) = signatures(
            "replace_file_content",
            &json!({"path": "src/ui/mod.rs", "old_string": "x", "new_string": "y", "start_line": 500}),
        );
        assert_eq!(cat1, "edit:src/ui/mod.rs#0");
        assert_eq!(cat2, "edit:src/ui/mod.rs#2");
    }

    #[test]
    fn edit_tool_buckets_string_start_line_like_the_handler() {
        // The edit handler parses string line numbers via parse_json_number;
        // the category signature must bucket them identically.
        let (_, cat) = signatures(
            "replace_file_content",
            &json!({"path": "src/ui/mod.rs", "old_string": "a", "new_string": "b", "start_line": "500"}),
        );
        assert_eq!(cat, "edit:src/ui/mod.rs#2");
    }

    #[test]
    fn alternating_edit_ping_pong_caught_by_category() {
        let mut d = LoopDetector::new(4);
        let edit1 = json!({"path": "src/ui/mod.rs", "old_string": "% 6", "new_string": "% 10"});
        let edit2 = json!({"path": "src/ui/mod.rs", "old_string": "% 10", "new_string": "% 6"});
        let (e1, c1) = signatures("replace_file_content", &edit1);
        let (e2, c2) = signatures("replace_file_content", &edit2);

        assert_eq!(d.check(&e1, &c1), LoopStatus::Ok);
        assert_eq!(d.check(&e2, &c2), LoopStatus::Warning(2));
        assert_eq!(d.check(&e1, &c1), LoopStatus::Warning(3));
        assert_eq!(d.check(&e2, &c2), LoopStatus::Abort(4));
    }

    #[test]
    fn grep_different_patterns_distinct_categories() {
        let (_e1, cat1) = signatures(
            "grep",
            &json!({ "pattern": "command", "path": "src/app/actions.rs" }),
        );
        let (_e2, cat2) = signatures(
            "grep",
            &json!({ "pattern": "/clear", "path": "src/app/actions.rs" }),
        );
        assert_ne!(cat1, cat2);
        assert_eq!(cat1, "grep:command@src/app/actions.rs");
        assert_eq!(cat2, "grep:/clear@src/app/actions.rs");
    }

    #[test]
    fn exact_repeat_warns_then_aborts() {
        let mut d = LoopDetector::new(6);
        assert_eq!(d.check("x", "x"), LoopStatus::Ok);
        assert_eq!(d.check("x", "x"), LoopStatus::Ok);
        assert_eq!(d.check("x", "x"), LoopStatus::Abort(3));
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
    fn read_only_repeats_warn_not_abort() {
        // A model paging around the same region it's editing must be nudged,
        // not hard-stopped: view_file repeats cap at Warning below 3× abort.
        let mut d = LoopDetector::new(4); // warn at 2, abort at 4
        let mut last = LoopStatus::Ok;
        for start in [250, 260, 250, 255, 252, 258] {
            let (e, c) = signatures(
                "view_file",
                &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
            );
            last = d.check_tool("view_file", &e, &c);
        }
        assert!(matches!(last, LoopStatus::Warning(_)), "got {last:?}");
    }

    #[test]
    fn mutating_tool_still_aborts_via_check_tool() {
        // check_tool must not soften non-read-only tools.
        let mut d = LoopDetector::new(4);
        let mut last = LoopStatus::Ok;
        for _ in 0..4 {
            last = d.check_tool("write_to_file", "write_to_file:x", "write_to_file:x");
        }
        assert_eq!(last, LoopStatus::Abort(4));
    }

    #[test]
    fn equivalent_failed_mutations_escalate_and_progress_resets_them() {
        let mut detector = LoopDetector::new(4);
        let first = detector.record_failed_tool("edit:a:1", "edit:src/state.ts");
        assert_eq!(first, LoopStatus::Ok);
        assert_eq!(
            detector.record_failed_tool("edit:a:2", "edit:src/state.ts"),
            LoopStatus::Abort(2)
        );

        detector.reset();
        assert_eq!(
            detector.record_failed_tool("edit:a:3", "edit:src/state.ts"),
            LoopStatus::Ok,
            "a successful mutation reset must clear the failed streak"
        );
    }

    #[test]
    fn safe_git_inspection_repeats_warn_not_abort() {
        let mut d = LoopDetector::new(4);
        let mut last = LoopStatus::Ok;
        for _ in 0..4 {
            let (exact, category) = signatures(
                "run_command",
                &json!({"command": "git log v0.6.0..HEAD --oneline --no-merges"}),
            );
            last = d.check_tool("run_command", &exact, &category);
        }
        assert!(matches!(last, LoopStatus::Warning(_)), "got {last:?}");
    }

    #[test]
    fn stable_git_inspection_is_progress_safe() {
        assert!(is_stable_inspection_command("git status --short"));
        assert!(is_stable_inspection_command("git diff --stat"));
        assert!(!is_stable_inspection_command("git restore -- src/lib.rs"));
    }

    #[test]
    fn leading_cd_does_not_collapse_distinct_shell_actions() {
        let (_, curl) = signatures(
            "run_command",
            &json!({"command": "cd /tmp/project && curl -s http://localhost:5199"}),
        );
        let (_, browser) = signatures(
            "run_command",
            &json!({"command": "cd /tmp/project && terminal-browser open http://localhost:5199"}),
        );

        assert_eq!(curl, "cmd:curl:http://localhost:5199");
        assert_eq!(browser, "cmd:terminal-browser:open http://localhost:5199");
        assert_ne!(curl, browser);
    }

    #[test]
    fn reset_clears_loop_state() {
        // After progress (reset), a previously-churning read starts fresh.
        let mut d = LoopDetector::new(4);
        for start in [250, 260, 250] {
            let (e, c) = signatures(
                "view_file",
                &json!({"path": "src/big.rs", "start_line": start, "end_line": start + 50}),
            );
            d.check(&e, &c);
        }
        d.reset();
        let (e, c) = signatures(
            "view_file",
            &json!({"path": "src/big.rs", "start_line": 255, "end_line": 305}),
        );
        assert_eq!(d.check(&e, &c), LoopStatus::Ok, "reset should clear counts");
    }

    #[test]
    fn output_stagnation() {
        let mut d = LoopDetector::new(4);
        assert_eq!(d.record_output("no matches"), LoopStatus::Ok);
        assert_eq!(d.record_output("no matches"), LoopStatus::Warning(2));
        assert_eq!(d.record_output("no matches"), LoopStatus::Warning(3));
        assert_eq!(d.record_output("no matches"), LoopStatus::Abort(4));
    }

    #[test]
    fn varied_no_match_searches_stagnate_as_one() {
        // Session 1785836601539: the model burned the whole tool-round budget
        // grepping for one hallucinated function name after another. Distinct
        // patterns produce distinct output strings, so exact hashing never
        // fired — the stagnation key must collapse them.
        let mut d = LoopDetector::new(4);
        let outputs = [
            "no matches for 'fn handle_input' under '.' (include filter: 'src/app/**/*.rs')",
            "no matches for 'fn handle_event' under '.' (include filter: 'src/**/*.rs')",
            "no matches for 'handle_key_event' under '.' (include filter: 'src/**/*.rs')",
            "no matches for 'on_key' under '.'",
        ];
        let mut last = LoopStatus::Ok;
        for out in outputs {
            last = d.record_output(stagnation_key(out));
        }
        assert_eq!(last, LoopStatus::Abort(4));
    }

    #[test]
    fn stagnation_key_leaves_real_output_untouched() {
        let out = "matches for 'foo' under '.' (1 file(s)):\n\n./a.rs:\n  1: foo";
        assert_eq!(stagnation_key(out), out);
    }

    fn observation(
        output: &str,
        state: Option<&str>,
        failure: Option<&str>,
    ) -> ProgressObservation {
        ProgressObservation {
            action: "test".to_string(),
            output_fingerprint: stable_hash(output),
            state_fingerprint: state.map(stable_hash),
            failure_fingerprint: failure.map(stable_hash),
            changed_workspace: state.is_some(),
            fresh_read: false,
            search_result: false,
            no_result: false,
            verification: false,
            read_only: false,
            replayed: false,
            success: true,
        }
    }

    #[test]
    fn progress_ledger_distinguishes_fresh_reads_from_cached_replays() {
        let mut ledger = ProgressLedger::default();
        let mut first = observation("file contents", None, None);
        first.fresh_read = true;
        assert_eq!(ledger.observe(&first).reason, ProgressReason::FreshRead);

        let mut replay = first.clone();
        replay.fresh_read = false;
        let assessment = ledger.observe(&replay);
        assert_eq!(assessment.reason, ProgressReason::NoNewInformation);
        assert!(!assessment.meaningful);
    }

    #[test]
    fn progress_ledger_treats_varied_no_result_searches_as_stagnation() {
        let mut ledger = ProgressLedger::default();
        for index in 0..ProgressLedger::RECOVERY_STREAK {
            let mut search = observation(&format!("no-match-{index}"), None, None);
            search.search_result = true;
            search.no_result = true;
            let assessment = ledger.observe(&search);
            assert_eq!(assessment.reason, ProgressReason::NoNewInformation);
        }
        assert_eq!(ledger.no_progress_streak(), ProgressLedger::RECOVERY_STREAK);
    }

    #[test]
    fn stable_successful_verification_suppresses_output_only_stagnation() {
        let mut ledger = ProgressLedger::default();
        let mut check = observation("cargo test: clean", None, None);
        check.verification = true;
        assert!(ledger.observe(&check).suppress_stagnation);
        assert!(ledger.observe(&check).suppress_stagnation);
        assert_eq!(ledger.no_progress_streak(), 0);
    }

    #[test]
    fn failed_verification_is_not_exempt_from_stagnation() {
        let mut ledger = ProgressLedger::default();
        let mut check = observation("cargo test: failed", None, Some("cargo test: failed"));
        check.verification = true;
        check.success = false;
        assert!(!ledger.observe(&check).suppress_stagnation);
        let repeated = ledger.observe(&check);
        assert_eq!(repeated.reason, ProgressReason::RepeatedFailure);
        assert!(ledger.no_progress_streak() > 0);
    }

    #[test]
    fn different_successful_actions_with_identical_output_are_progress() {
        let mut ledger = ProgressLedger::default();
        let first = observation("", None, None);
        assert!(ledger.observe(&first).meaningful);

        let mut second = first.clone();
        second.action = "different-action".to_string();
        assert!(ledger.observe(&second).meaningful);
    }

    #[test]
    fn replayed_reads_do_not_count_as_new_information() {
        let mut ledger = ProgressLedger::default();
        let mut first = observation("file", None, None);
        first.read_only = true;
        first.fresh_read = true;
        assert!(ledger.observe(&first).meaningful);

        let mut replay = first.clone();
        replay.replayed = true;
        replay.fresh_read = false;
        assert!(!ledger.observe(&replay).meaningful);
    }

    #[test]
    fn returning_to_a_previous_workspace_state_is_churn() {
        let mut ledger = ProgressLedger::default();
        assert_eq!(
            ledger.observe(&observation("a", Some("a"), None)).reason,
            ProgressReason::WorkspaceChanged
        );
        assert_eq!(
            ledger.observe(&observation("b", Some("b"), None)).reason,
            ProgressReason::WorkspaceChanged
        );
        let assessment = ledger.observe(&observation("a", Some("a"), None));
        assert_eq!(assessment.reason, ProgressReason::Churn);
        assert!(!assessment.meaningful);
    }

    #[test]
    fn reasoning_loop_detector_catches_consecutive_repeated_sentences() {
        let mut detector = ReasoningLoopDetector::default();
        let sentence =
            "We need to inspect the network module to check the turn engine implementation.\n";
        assert_eq!(detector.feed_chunk(sentence), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk(sentence), ReasoningLoopStatus::Ok);
        assert!(matches!(
            detector.feed_chunk(sentence),
            ReasoningLoopStatus::LoopDetected(_)
        ));
    }

    #[test]
    fn reasoning_loop_detector_catches_alternating_2_cycle() {
        let mut detector = ReasoningLoopDetector::default();
        let a = "First let us inspect the network engine to understand turn execution.\n";
        let b = "Now we should review the loop detection rules in loop_detect module.\n";
        assert_eq!(detector.feed_chunk(a), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk(b), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk(a), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk(b), ReasoningLoopStatus::Ok);
        assert!(matches!(
            detector.feed_chunk(a),
            ReasoningLoopStatus::LoopDetected(_)
        ));
    }

    #[test]
    fn reasoning_loop_detector_catches_paragraph_repetition() {
        let mut detector = ReasoningLoopDetector::default();
        let para = "In this step we are carefully inspecting the entire test suite to ensure that all tests pass without errors and no regressions are introduced.\n\n";
        assert_eq!(detector.feed_chunk(para), ReasoningLoopStatus::Ok);
        assert_eq!(detector.feed_chunk(para), ReasoningLoopStatus::Ok);
        assert!(matches!(
            detector.feed_chunk(para),
            ReasoningLoopStatus::LoopDetected(_)
        ));
    }

    #[test]
    fn reasoning_loop_detector_allows_legitimate_long_reasoning() {
        let mut detector = ReasoningLoopDetector::default();
        for i in 0..60 {
            let unique_thought = format!(
                "Step {i}: Considering function handler_{i} in module_{i} for comprehensive architectural refactoring.\n"
            );
            assert_eq!(
                detector.feed_chunk(&unique_thought),
                ReasoningLoopStatus::Ok,
                "legitimate unique reasoning step {i} should not trigger loop detector"
            );
        }
    }

    #[test]
    fn reasoning_loop_detector_ignores_short_common_phrases() {
        let mut detector = ReasoningLoopDetector::default();
        for _ in 0..10 {
            assert_eq!(detector.feed_chunk("Let's see.\n"), ReasoningLoopStatus::Ok);
            assert_eq!(detector.feed_chunk("Wait.\n"), ReasoningLoopStatus::Ok);
            assert_eq!(detector.feed_chunk("Okay.\n"), ReasoningLoopStatus::Ok);
        }
    }

    #[test]
    fn reasoning_loop_detector_catches_cross_turn_stagnant_plan() {
        let mut detector = ReasoningLoopDetector::default();
        let plan = "Plan: We need to inspect src/network/turn_engine.rs to check how single turns execute.";
        assert_eq!(
            detector.record_turn_reasoning(plan, false),
            ReasoningLoopStatus::Ok
        );
        assert_eq!(
            detector.record_turn_reasoning(plan, false),
            ReasoningLoopStatus::LoopDetected(DIAG_CROSS_TURN_SAME_PLAN)
        );

        // Workspace progress resets cross-turn plan tracking
        detector.record_turn_reasoning(plan, true);
        assert_eq!(
            detector.record_turn_reasoning(plan, false),
            ReasoningLoopStatus::Ok
        );
    }

    #[test]
    fn reasoning_loop_detector_catches_paraphrased_same_plan() {
        let mut detector = ReasoningLoopDetector::default();
        let turn1 = "I will modify src/network/turn_engine.rs to implement the loop recovery logic for reasoning loops.";
        let turn2 = "We are ready to alter src/network/turn_engine.rs to add the loop recovery behavior for reasoning streams.";

        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn1,
                target_files: &["src/network/turn_engine.rs"],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 1,
            }),
            ReasoningLoopStatus::Ok
        );

        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn2,
                target_files: &["src/network/turn_engine.rs"],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 2,
            }),
            ReasoningLoopStatus::LoopDetected(DIAG_CROSS_TURN_SAME_PLAN)
        );
    }

    #[test]
    fn reasoning_loop_detector_catches_ready_to_implement_hesitation_loop() {
        let mut detector = ReasoningLoopDetector::default();
        let turn1 = "The architecture is clear. I am ready to implement the changes in src/network.rs. Let's do one more check on the helper functions.";
        let turn2 = "We have confirmed the helper functions. Now proceed with implementation in src/network.rs. Let me do a quick check on the return types first.";

        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn1,
                target_files: &["src/network.rs"],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 1,
            }),
            ReasoningLoopStatus::Ok
        );

        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn2,
                target_files: &["src/network.rs"],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 2,
            }),
            ReasoningLoopStatus::LoopDetected(DIAG_SEMANTIC_NO_PROGRESS)
        );
    }

    #[test]
    fn reasoning_loop_detector_catches_same_files_no_progress() {
        let mut detector = ReasoningLoopDetector::default();
        let file = "src/app/state.rs";

        let turn0 = "Turn 0: Inspecting the AppState struct definition in src/app/state.rs.";
        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn0,
                target_files: &[file],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 1,
            }),
            ReasoningLoopStatus::Ok
        );

        let turn1 =
            "Turn 1: Checking TokenUsage calculations and prompt metrics in src/app/state.rs.";
        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn1,
                target_files: &[file],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 2,
            }),
            ReasoningLoopStatus::Ok
        );

        let turn2 = "Turn 2: Viewing session history storage vector in src/app/state.rs.";
        assert_eq!(
            detector.record_turn_evidence(&TurnEvidence {
                reasoning: turn2,
                target_files: &[file],
                made_progress: false,
                had_edits: false,
                tool_count: 1,
                no_progress_streak: 3,
            }),
            ReasoningLoopStatus::LoopDetected(DIAG_SAME_FILES_NO_PROGRESS)
        );
    }

    #[test]
    fn wide_repository_investigation_does_not_trigger_loop() {
        let mut detector = ReasoningLoopDetector::default();
        let files = [
            "src/app/state.rs",
            "src/network/turn_engine.rs",
            "src/tools/exec.rs",
            "src/ui/mod.rs",
            "src/config.rs",
            "src/main.rs",
        ];

        for (idx, file) in files.iter().enumerate() {
            let reasoning = format!(
                "Step {idx}: Exploring module {file} to map codebase architecture and relationships."
            );
            assert_eq!(
                detector.record_turn_evidence(&TurnEvidence {
                    reasoning: &reasoning,
                    target_files: &[file],
                    made_progress: false,
                    had_edits: false,
                    tool_count: 1,
                    no_progress_streak: idx + 1,
                }),
                ReasoningLoopStatus::Ok,
                "legitimate broad exploration of {file} should not trigger loop detector"
            );
        }
    }

    #[test]
    fn semantic_paragraph_similarity_in_stream() {
        let mut detector = ReasoningLoopDetector::default();
        let p1 = "We must carefully inspect the turn execution loop in src/network/turn_engine.rs to verify how recovery actions are triggered.\n\n";
        let p2 = "We should carefully inspect the turn execution loop in src/network/turn_engine.rs to verify how recovery actions are triggered.\n\n";

        assert_eq!(detector.feed_chunk(p1), ReasoningLoopStatus::Ok);
        assert_eq!(
            detector.feed_chunk(p2),
            ReasoningLoopStatus::LoopDetected(DIAG_REPEATED_BLOCK)
        );
    }
}

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
        if evidence.made_progress {
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
