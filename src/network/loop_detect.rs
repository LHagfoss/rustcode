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
    if category.starts_with("read:") {
        return true;
    }
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
    let category = if let Some(category) = read_category(name, args) {
        category
    } else if name == "run_command" {
        match args.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => normalize_command(cmd),
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

/// Normalize native file reads and common read-only shell commands into one
/// semantic category. This prevents a model from evading repeat detection by
/// switching from `view_file` to `cat`, `sed`, or `awk` while inspecting the
/// same region. Regions are bucketed per 200 lines so paging through a large
/// file remains legitimate work.
fn read_category(name: &str, args: &Value) -> Option<String> {
    if name == "view_file" || name == "read_file" {
        let path = args.get("path")?.as_str()?.trim();
        if path.is_empty() {
            return None;
        }
        let start = args
            .get("start_line")
            .and_then(|value| value.as_u64())
            .unwrap_or(1);
        return Some(format!(
            "read:{}#{}",
            normalize_read_path(path),
            start / 200
        ));
    }
    if name != "run_command" {
        return None;
    }

    let command = args.get("command")?.as_str()?;
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let command_index = tokens
        .iter()
        .position(|token| matches!(token.rsplit('/').next(), Some("cat" | "sed" | "awk" | "nl")))?;
    let bin = tokens[command_index].rsplit('/').next()?;
    if !matches!(bin, "cat" | "sed" | "awk" | "nl") {
        return None;
    }
    let path = tokens
        .iter()
        .rev()
        .map(|token| token.trim_matches(|c: char| c == '\'' || c == '"'))
        .find(|token| !token.is_empty() && !token.starts_with('-'))?;
    let start = match bin {
        "sed" => tokens[command_index + 1..]
            .iter()
            .find_map(|token| parse_sed_start(token)),
        "awk" => parse_awk_start(command),
        _ => None,
    }
    .unwrap_or(1);
    Some(format!(
        "read:{}#{}",
        normalize_read_path(path),
        start / 200
    ))
}

fn normalize_read_path(path: &str) -> &str {
    path.trim_matches(|c: char| c == '\'' || c == '"')
        .trim_start_matches("./")
        .trim_end_matches('/')
}

fn parse_sed_start(token: &str) -> Option<u64> {
    token
        .trim_matches(|c: char| c == '\'' || c == '"')
        .split_once(',')
        .and_then(|(start, _)| start.parse().ok())
}

fn parse_awk_start(command: &str) -> Option<u64> {
    let marker = "NR>=";
    let tail = command.split_once(marker)?.1;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn primary_command(cmd: &str) -> &str {
    cmd.split(" && ")
        .flat_map(|segment| segment.split(" || "))
        .flat_map(|segment| segment.split(" ; "))
        .flat_map(|segment| segment.split(" | "))
        .map(str::trim)
        .find(|segment| {
            let mut tokens = segment.split_whitespace();
            !matches!(tokens.next(), Some("cd"))
        })
        .unwrap_or("")
}

/// Reduce a shell command to its semantic core: primary command before any
/// `||`/`&&`/`;`/`|`, flags dropped, arguments unquoted and de-slashed.
/// Search tools normalize to `search:<args>` so all grep/rg variants match.
fn normalize_command(cmd: &str) -> String {
    // Isolate the primary substantive command (spaces around separators avoid
    // matching operators inside quoted patterns like 'TODO|FIXME'). A leading
    // `cd … &&` is setup, not the action: collapsing every workspace command
    // to `cmd:cd:<path>` creates false loop warnings across unrelated checks.
    let core = primary_command(cmd);

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
    cross_read_category: Option<String>,
    cross_read_methods: HashSet<String>,
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
            cross_read_category: None,
            cross_read_methods: HashSet::new(),
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
        if category.starts_with("read:") {
            if self.cross_read_category.as_deref() != Some(category) {
                self.cross_read_category = Some(category.to_string());
                self.cross_read_methods.clear();
            }
            self.cross_read_methods.insert(read_method(name, exact));
            if self.cross_read_methods.len() >= 3 {
                return LoopStatus::Abort(self.cross_read_methods.len());
            }
        } else {
            self.cross_read_category = None;
            self.cross_read_methods.clear();
        }
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
        self.cross_read_category = None;
        self.cross_read_methods.clear();
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

fn read_method(name: &str, exact: &str) -> String {
    if name != "run_command" {
        return name.to_string();
    }
    ["cat", "sed", "awk", "nl"]
        .into_iter()
        .find(|bin| exact.contains(&format!("\"command\":\"{bin} ")))
        .map(|bin| format!("run_command:{bin}"))
        .unwrap_or_else(|| name.to_string())
}

#[path = "loop_detect/reasoning.rs"]
mod reasoning;
pub use reasoning::*;

#[cfg(test)]
#[path = "loop_detect/tests.rs"]
mod tests;
