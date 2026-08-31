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
use std::path::Path;

fn parse_json_number(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        Some(n)
    } else if let Some(s) = v.as_str() {
        s.parse::<u64>().ok()
    } else {
        None
    }
}

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
            .and_then(parse_json_number)
            .or_else(|| {
                args.get("edits")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("start_line"))
                    .and_then(parse_json_number)
            })
            .or_else(|| {
                args.get("replacements")
                    .and_then(|arr| arr.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|item| item.get("start_line"))
                    .and_then(parse_json_number)
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
    let (path, start, _) = read_target(name, args)?;
    Some(format!("read:{path}#{}", start / 200))
}

/// Return the normalized path and requested line range for a native read or
/// a safe shell probe. `wc` and `od` represent whole-file checks.
pub fn read_target(name: &str, args: &Value) -> Option<(String, usize, Option<usize>)> {
    if name == "view_file" || name == "read_file" {
        let path = args.get("path")?.as_str()?.trim();
        if path.is_empty() {
            return None;
        }
        let start = args
            .get("start_line")
            .and_then(parse_json_number)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(1);
        let end = args
            .get("end_line")
            .and_then(parse_json_number)
            .and_then(|value| usize::try_from(value).ok());
        return Some((normalize_read_path(path), start, end));
    }
    if name != "run_command" {
        return None;
    }
    let command = args.get("command")?.as_str()?;
    let tokens = shell_tokens(command);
    let command_index = tokens.iter().position(|token| {
        matches!(
            token.rsplit('/').next(),
            Some("cat" | "sed" | "awk" | "nl" | "wc" | "od")
        )
    })?;
    let bin = tokens[command_index].rsplit('/').next()?;
    let segment_end = tokens[command_index + 1..]
        .iter()
        .position(|token| is_shell_operator(token))
        .map_or(tokens.len(), |offset| command_index + 1 + offset);
    let path = tokens
        .get(command_index + 1..segment_end)?
        .iter()
        .enumerate()
        .rev()
        .find(|(_, token)| {
            !token.is_empty()
                && !token.starts_with('-')
                && !token.contains('=')
                && !token.contains('>')
        })
        .map(|(_, token)| token)?;
    let segment = &tokens[command_index..segment_end];
    let (start, end) = match bin {
        "sed" => segment[1..]
            .iter()
            .find_map(|token| parse_sed_range(token))
            .unwrap_or((1, None)),
        "awk" => parse_awk_range(&segment.join(" ")).unwrap_or((1, None)),
        _ => (1, None),
    };
    Some((normalize_shell_read_path(&tokens, path), start, end))
}

/// Whether a recognized read command returns file content. `wc` and `od`
/// provide useful integrity metadata but must not make recovery claim that
/// the complete source range was shown to the model.
pub fn read_returns_content(name: &str, args: &Value) -> bool {
    if name == "view_file" || name == "read_file" {
        return true;
    }
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return false;
    };
    shell_tokens(command)
        .iter()
        .find_map(|token| token.rsplit('/').next())
        .is_some_and(|bin| matches!(bin, "cat" | "sed" | "awk" | "nl"))
}

fn is_shell_operator(token: &str) -> bool {
    matches!(token, "|" | "||" | "&&" | ";" | "&")
        || token.starts_with('>')
        || token.starts_with("2>")
}

fn normalize_read_path(path: &str) -> String {
    path.trim_matches(|c: char| c == '\'' || c == '"')
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Tokenize the small shell subset used by read-only probes. This deliberately
/// does not attempt to execute shell syntax; it only keeps quoted paths (and
/// awk/sed expressions) together so equivalent commands share a key.
fn shell_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                token.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(ch);
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn normalize_shell_read_path(tokens: &[String], path: &str) -> String {
    let mut path = normalize_read_path(path);
    // `cd /project && cat src/x` and `cat /project/src/x` are the same
    // inspection. Relative `cd` prefixes are retained too: they are stable
    // within the command and avoid treating `cd src && cat config.ts` as an
    // unrelated root-level read.
    if !Path::new(&path).is_absolute()
        && tokens.first().is_some_and(|token| token == "cd")
        && let Some(cwd) = tokens.get(1)
        && !cwd.is_empty()
    {
        path = Path::new(cwd).join(path).to_string_lossy().into_owned();
        path = normalize_read_path(&path);
    }
    path
}

fn parse_sed_range(token: &str) -> Option<(usize, Option<usize>)> {
    let token = token.trim_matches(|c: char| c == '\'' || c == '"');
    let (start, end) = token.split_once(',')?;
    let start = start.parse().ok()?;
    let end = end.strip_suffix('p').unwrap_or(end).parse().ok()?;
    Some((start, Some(end)))
}

fn parse_awk_start(command: &str) -> Option<u64> {
    let marker = "NR>=";
    let tail = command.split_once(marker)?.1;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn parse_awk_range(command: &str) -> Option<(usize, Option<usize>)> {
    let start = parse_awk_start(command)?.try_into().ok()?;
    let marker = "NR<=";
    let end = command.split_once(marker)?.1;
    let digits: String = end.chars().take_while(char::is_ascii_digit).collect();
    Some((start, digits.parse().ok()))
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

/// Authoritative, compact evidence for a file that was successfully mutated.
/// This is intentionally metadata-only: recovery messages can identify the
/// verified path and range without copying source back into the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEvidence {
    pub revision: u64,
    pub content_digest: Option<u64>,
    pub byte_count: Option<usize>,
    pub line_count: Option<usize>,
    pub last_mutation: u64,
    ranges: HashSet<(usize, usize)>,
    content_ranges: HashSet<(usize, usize)>,
    repeated_reads: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedReadRecovery {
    pub path: String,
    pub revision: u64,
    pub line_count: Option<usize>,
    pub byte_count: Option<usize>,
    pub start_line: usize,
    pub end_line: usize,
    pub repeated_reads: usize,
    pub content_returned: bool,
}

impl GroundedReadRecovery {
    pub fn message(&self) -> String {
        let counts = match (self.line_count, self.byte_count) {
            (Some(lines), Some(bytes)) => format!("; {lines} lines, {bytes} bytes"),
            (Some(lines), None) => format!("; {lines} lines"),
            (None, Some(bytes)) => format!("; {bytes} bytes"),
            (None, None) => String::new(),
        };
        let evidence = if self.content_returned {
            format!(
                "complete range {}-{} was already returned{}",
                self.start_line, self.end_line, counts
            )
        } else {
            format!(
                "the same file metadata was already checked{} (range {}-{})",
                counts, self.start_line, self.end_line
            )
        };
        format!(
            "{} is unchanged since successful write (revision {}); {evidence}; do not re-check it unless a specific missing line or a new diagnostic is identified.",
            self.path, self.revision
        )
    }
}

/// Tracks read evidence by path and mutation revision. A different range or
/// any successful edit starts a new evidence sequence, which keeps ordinary
/// paging and post-edit reads out of the recovery path.
#[derive(Debug, Default, Clone)]
pub struct FileEvidenceLedger {
    generation: u64,
    files: HashMap<String, FileEvidence>,
}

impl FileEvidenceLedger {
    const RECOVERY_REPEATS: usize = 2;

    pub fn record_mutation(&mut self, path: &str, content: Option<&str>) {
        self.generation = self.generation.saturating_add(1);
        let (content_digest, byte_count, line_count) = content
            .map(|content| {
                (
                    Some(stable_hash(content)),
                    Some(content.len()),
                    Some(content.lines().count()),
                )
            })
            .unwrap_or((None, None, None));
        self.files.insert(
            normalize_read_path(path),
            FileEvidence {
                revision: self.generation,
                content_digest,
                byte_count,
                line_count,
                last_mutation: self.generation,
                ranges: HashSet::new(),
                content_ranges: HashSet::new(),
                repeated_reads: 0,
            },
        );
    }

    /// Record one successful native or shell read. The semantic category is
    /// supplied by `signatures`, while this method retains exact ranges so a
    /// non-overlapping page is progress even when it falls in one bucket.
    pub fn record_read(
        &mut self,
        path: &str,
        start_line: usize,
        end_line: Option<usize>,
        successful: bool,
    ) -> Option<GroundedReadRecovery> {
        self.record_read_with_kind(path, start_line, end_line, successful, true)
    }

    pub fn record_read_with_kind(
        &mut self,
        path: &str,
        start_line: usize,
        end_line: Option<usize>,
        successful: bool,
        content_read: bool,
    ) -> Option<GroundedReadRecovery> {
        if !successful || start_line == 0 {
            return None;
        }
        let path = normalize_read_path(path);
        let evidence = self.files.get_mut(&path)?;
        let end_line = end_line.or(evidence.line_count).unwrap_or(start_line);
        let range = (start_line, end_line.max(start_line));
        let complete = evidence
            .line_count
            .is_some_and(|line_count| start_line == 1 && range.1 >= line_count);
        if evidence.ranges.insert(range) {
            evidence.repeated_reads = 0;
        } else {
            evidence.repeated_reads = evidence.repeated_reads.saturating_add(1);
        }
        if content_read {
            evidence.content_ranges.insert(range);
        }
        (complete && evidence.repeated_reads >= Self::RECOVERY_REPEATS).then(|| {
            GroundedReadRecovery {
                path,
                revision: evidence.revision,
                line_count: evidence.line_count,
                byte_count: evidence.byte_count,
                start_line: range.0,
                end_line: range.1,
                repeated_reads: evidence.repeated_reads,
                content_returned: evidence.content_ranges.contains(&range),
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressReason {
    WorkspaceChanged,
    NewInformation,
    FreshRead,
    Verification,
    RepeatedVerification,
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
            Self::RepeatedVerification => "repeated_verification",
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
    seen_reads: HashSet<u64>,
    seen_verifications: HashSet<u64>,
    seen_failures: HashSet<u64>,
    recent_states: VecDeque<u64>,
    no_progress_streak: usize,
}

impl Default for ProgressLedger {
    fn default() -> Self {
        Self {
            seen_outputs: HashSet::new(),
            seen_reads: HashSet::new(),
            seen_verifications: HashSet::new(),
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
        if observation.changed_workspace {
            self.seen_verifications.clear();
            self.seen_reads.clear();
        }
        let new_read = observation.read_only
            && remember(&mut self.seen_reads, stable_hash(&observation.action));
        let stable_verification = observation.verification && observation.success;
        // Verification novelty is keyed to the normalized action, not its
        // stdout. Test runners commonly vary elapsed-time text between runs;
        // that does not make the same check new evidence.
        let verification_action = stable_hash(&observation.action);
        let fresh_verification =
            stable_verification && remember(&mut self.seen_verifications, verification_action);
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

        let (meaningful, reason) = if churn {
            (false, ProgressReason::Churn)
        } else if observation.changed_workspace {
            (true, ProgressReason::WorkspaceChanged)
        } else if observation.read_only && observation.replayed {
            (false, ProgressReason::NoNewInformation)
        } else if observation.read_only && observation.success {
            if observation.fresh_read && new_read {
                (true, ProgressReason::FreshRead)
            } else {
                (false, ProgressReason::NoNewInformation)
            }
        } else if observation.fresh_read && new_output {
            (true, ProgressReason::FreshRead)
        } else if observation.search_result && observation.success && observation.no_result {
            (false, ProgressReason::NoNewInformation)
        } else if observation.search_result && new_output {
            (true, ProgressReason::NewInformation)
        } else if fresh_verification {
            (true, ProgressReason::Verification)
        } else if stable_verification {
            (false, ProgressReason::RepeatedVerification)
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
        } else if !fresh_verification {
            self.no_progress_streak = self.no_progress_streak.saturating_add(1);
        }

        ProgressAssessment {
            meaningful,
            reason,
            streak: self.no_progress_streak,
            // The first successful verification for an unchanged workspace is
            // useful evidence. Repeating the same successful check is not: it
            // must remain visible to stagnation guards so a model cannot spend
            // the turn re-proving an already established result.
            suppress_stagnation: fresh_verification,
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
            // Do not abort before the result is available. The result handler
            // may have authoritative write/range evidence that can ground a
            // recovery notice; a pre-execution abort would only emit the
            // generic loop prompt and lose that context.
            if self.cross_read_methods.len() >= 3 && status == LoopStatus::Ok {
                return LoopStatus::Warning(self.cross_read_methods.len());
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

mod reasoning;
pub use reasoning::*;

#[cfg(test)]
mod tests;
