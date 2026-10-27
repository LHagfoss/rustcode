//! Small, inspectable project memory for reusable coding-agent facts.
//!
//! This is deliberately separate from session history and transient AppState.
//! Facts are explicit, bounded records keyed to a canonical repository/worktree
//! identity; the first version does not attempt autonomous transcript mining.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MEMORY_VERSION: u32 = 1;
const MAX_FACTS: usize = 64;
const MAX_FACT_VALUE_BYTES: usize = 768;
const MAX_MEMORY_BYTES: usize = 32 * 1024;
const STALE_AFTER_SECONDS: u64 = 180 * 24 * 60 * 60;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
static MEMORY_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFact {
    pub category: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub confidence: u8,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemory {
    pub version: u32,
    pub identity: String,
    pub facts: Vec<MemoryFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLocation {
    pub root: PathBuf,
    pub identity: String,
    pub path: PathBuf,
}

pub fn repository_root(root: Option<&Path>) -> PathBuf {
    let candidate = root
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let candidate = fs::canonicalize(&candidate).unwrap_or(candidate);
    git_value(&candidate, &["rev-parse", "--show-toplevel"])
        .and_then(|path| fs::canonicalize(path).ok())
        .unwrap_or(candidate)
}

/// The directory memory resolves against for a session: its task directory,
/// else its workspace root. Every entry point (per-turn injection, the
/// memory tools, `/memory`) goes through this so a fact saved in one is read
/// by the others, also in ACP sessions and worktrees (#2495).
pub fn scope_root(task: Option<PathBuf>, workspace: Option<PathBuf>) -> Option<PathBuf> {
    task.or(workspace)
}

/// [`scope_root`] for the session whose tool call is running.
pub(crate) fn tool_scope_root() -> Option<PathBuf> {
    scope_root(
        crate::tools::active_task_working_directory(),
        crate::tools::active_workspace_root(),
    )
}

pub fn location(root: Option<&Path>) -> MemoryLocation {
    let root = repository_root(root);
    let common = git_value(&root, &["rev-parse", "--git-common-dir"])
        .and_then(|path| fs::canonicalize(root.join(path)).ok())
        .unwrap_or_else(|| root.join(".git"));
    let identity = format!("root={}\ngit_common={}", root.display(), common.display());
    let file_key = stable_key(&identity);
    let base = crate::config::get_config_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("rustcode"))
        .join("project-memory");
    MemoryLocation {
        root,
        identity,
        path: base.join(format!("{file_key}.json")),
    }
}

pub fn load(root: Option<&Path>) -> Result<ProjectMemory, String> {
    let location = location(root);
    if fs::metadata(&location.path)
        .map(|metadata| metadata.len() as usize > MAX_MEMORY_BYTES)
        .unwrap_or(false)
    {
        return Err("project memory exceeds its bounded file size".to_string());
    }
    let raw = match fs::read_to_string(&location.path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectMemory {
                version: MEMORY_VERSION,
                identity: location.identity,
                facts: Vec::new(),
            });
        }
        Err(error) => return Err(format!("could not read project memory: {error}")),
    };
    let mut memory: ProjectMemory = serde_json::from_str(&raw).map_err(|error| {
        format!(
            "invalid project memory {}: {error}",
            location.path.display()
        )
    })?;
    if memory.version != MEMORY_VERSION || memory.identity != location.identity {
        return Err("project memory version or repository identity does not match".to_string());
    }
    memory.facts.retain(|fact| valid_fact(fact));
    for fact in &mut memory.facts {
        fact.confidence = fact.confidence.clamp(1, 100);
    }
    memory.facts.truncate(MAX_FACTS);
    Ok(memory)
}

#[cfg(test)]
pub fn save(root: Option<&Path>, memory: &ProjectMemory) -> Result<(), String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    save_unlocked(root, memory)
}

fn save_unlocked(root: Option<&Path>, memory: &ProjectMemory) -> Result<(), String> {
    let location = location(root);
    if memory.version != MEMORY_VERSION || memory.identity != location.identity {
        return Err("project memory identity/version mismatch".to_string());
    }
    if memory.facts.len() > MAX_FACTS || memory.facts.iter().any(|fact| !valid_fact(fact)) {
        return Err("project memory contains an invalid or oversized fact".to_string());
    }
    let json = serde_json::to_vec_pretty(memory).map_err(|error| error.to_string())?;
    if json.len() > MAX_MEMORY_BYTES {
        return Err("project memory is full; remove stale facts before adding more".to_string());
    }
    fs::create_dir_all(location.path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    let tmp = location.path.with_extension(format!(
        "json.{}.{}.{}.tmp",
        std::process::id(),
        now_nanos(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&tmp, json).map_err(|error| error.to_string())?;
    fs::rename(&tmp, &location.path).map_err(|error| error.to_string())
}

pub fn upsert(root: Option<&Path>, mut fact: MemoryFact) -> Result<(), String> {
    fact.category = clean_field(&fact.category, 48)?;
    fact.key = clean_field(&fact.key, 96)?;
    fact.value = clean_value(&fact.value)?;
    fact.source = clean_field(&fact.source, 160)?;
    fact.confidence = fact.confidence.clamp(1, 100);
    safe_fact(&fact)?;
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut memory = load(root)?;
    if let Some(existing) = memory
        .facts
        .iter_mut()
        .find(|existing| existing.category == fact.category && existing.key == fact.key)
    {
        *existing = fact;
    } else {
        memory.facts.push(fact);
    }
    memory
        .facts
        .sort_by(|a, b| a.category.cmp(&b.category).then_with(|| a.key.cmp(&b.key)));
    memory.facts.truncate(MAX_FACTS);
    save_unlocked(root, &memory)
}

/// Remove the facts `spec` names: `category/key` names one fact, a bare key
/// every fact with that key. A category alone matches nothing, so a key that
/// happens to equal a category name no longer empties that category.
pub fn remove(root: Option<&Path>, spec: &str) -> Result<usize, String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut memory = load(root)?;
    let before = memory.facts.len();
    memory.facts.retain(|fact| !names_fact(spec, fact));
    save_unlocked(root, &memory)?;
    Ok(before.saturating_sub(memory.facts.len()))
}

fn names_fact(spec: &str, fact: &MemoryFact) -> bool {
    let spec = spec.trim();
    match spec.split_once('/') {
        Some((category, key)) => fact.category == category.trim() && fact.key == key.trim(),
        None => fact.key == spec,
    }
}

pub fn reset(root: Option<&Path>) -> Result<(), String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    reset_unlocked(root)
}

fn reset_unlocked(root: Option<&Path>) -> Result<(), String> {
    let location = location(root);
    match fs::remove_file(location.path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Remove every RustCode project-memory file, but only inside the dedicated
/// inspectable storage directory. Session history and unrelated config files
/// are never part of this operation.
pub fn reset_all() -> Result<usize, String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    reset_all_unlocked()
}

fn reset_all_unlocked() -> Result<usize, String> {
    let base = crate::config::get_config_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("rustcode"))
        .join("project-memory");
    let Ok(entries) = fs::read_dir(&base) else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            fs::remove_file(path).map_err(|error| error.to_string())?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Handle the durable-memory subcommands shared by the TUI, raw CLI, and ACP.
/// An empty argument list returns `None` so callers can retain the historical
/// `/memory` RAM diagnostic without making the command ambiguous.
pub fn command(root: Option<&Path>, args: &[&str]) -> Option<String> {
    let message = match args.first().copied() {
        Some("show") | Some("list") => render(root),
        Some("path") => Ok(format!(
            "Project memory file: {}",
            location(root).path.display()
        )),
        Some("reset") => reset(root).map(|()| "Project memory reset.".to_string()),
        Some("reset-all") => {
            reset_all().map(|count| format!("Reset {count} project-memory file(s)."))
        }
        Some("forget") => match args.get(1) {
            Some(key) => {
                remove(root, key).map(|count| format!("Removed {count} project-memory fact(s)."))
            }
            None => Ok("Usage: /memory forget <key|category/key>".to_string()),
        },
        Some("add") if args.len() >= 4 => upsert(
            root,
            fact(args[1], args[2], &args[3..].join(" "), "explicit user note"),
        )
        .map(|()| format!("Saved project-memory fact '{}'.", args[2])),
        Some("add") => Ok("Usage: /memory add <category> <key> <short fact>".to_string()),
        Some(_) => Ok(
            "Usage: /memory [show|path|add <category> <key> <fact>|forget <key|category/key>|reset|reset-all]"
                .to_string(),
        ),
        None => return None,
    };
    Some(message.unwrap_or_else(|error| format!("Project memory error: {error}")))
}

pub fn render(root: Option<&Path>) -> Result<String, String> {
    let memory = load(root)?;
    if memory.facts.is_empty() {
        return Ok("Project memory is empty.".to_string());
    }
    let mut output = format!("Project memory ({} fact(s)):\n", memory.facts.len());
    for fact in memory.facts {
        output.push_str(&format!(
            "- [{}] {} = {} (source: {}, confidence: {})\n",
            fact.category, fact.key, fact.value, fact.source, fact.confidence
        ));
    }
    Ok(output.trim_end().to_string())
}

pub fn global_location() -> PathBuf {
    crate::config::get_config_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("rustcode"))
        .join("global-memory.json")
}

pub fn load_global() -> Result<ProjectMemory, String> {
    let path = global_location();
    if fs::metadata(&path)
        .map(|metadata| metadata.len() as usize > MAX_MEMORY_BYTES)
        .unwrap_or(false)
    {
        return Err("global memory exceeds its bounded file size".to_string());
    }
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectMemory {
                version: MEMORY_VERSION,
                identity: "global".to_string(),
                facts: Vec::new(),
            });
        }
        Err(error) => return Err(format!("could not read global memory: {error}")),
    };
    let mut memory: ProjectMemory = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid global memory {}: {error}", path.display()))?;
    if memory.version != MEMORY_VERSION || memory.identity != "global" {
        return Err("global memory version or identity does not match".to_string());
    }
    memory.facts.retain(|fact| valid_fact(fact));
    for fact in &mut memory.facts {
        fact.confidence = fact.confidence.clamp(1, 100);
    }
    memory.facts.truncate(MAX_FACTS);
    Ok(memory)
}

fn save_global_unlocked(memory: &ProjectMemory) -> Result<(), String> {
    let path = global_location();
    if memory.version != MEMORY_VERSION || memory.identity != "global" {
        return Err("global memory identity/version mismatch".to_string());
    }
    if memory.facts.len() > MAX_FACTS || memory.facts.iter().any(|fact| !valid_fact(fact)) {
        return Err("global memory contains an invalid or oversized fact".to_string());
    }
    let json = serde_json::to_vec_pretty(memory).map_err(|error| error.to_string())?;
    if json.len() > MAX_MEMORY_BYTES {
        return Err("global memory is full; remove stale facts before adding more".to_string());
    }
    fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
        .map_err(|error| error.to_string())?;
    let tmp = path.with_extension(format!(
        "json.{}.{}.{}.tmp",
        std::process::id(),
        now_nanos(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&tmp, json).map_err(|error| error.to_string())?;
    fs::rename(&tmp, &path).map_err(|error| error.to_string())
}

pub fn upsert_global(mut fact: MemoryFact) -> Result<(), String> {
    fact.category = clean_field(&fact.category, 48)?;
    fact.key = clean_field(&fact.key, 96)?;
    fact.value = clean_value(&fact.value)?;
    fact.source = clean_field(&fact.source, 160)?;
    fact.confidence = fact.confidence.clamp(1, 100);
    safe_fact(&fact)?;
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut memory = load_global()?;
    if let Some(existing) = memory
        .facts
        .iter_mut()
        .find(|existing| existing.category == fact.category && existing.key == fact.key)
    {
        *existing = fact;
    } else {
        memory.facts.push(fact);
    }
    memory
        .facts
        .sort_by(|a, b| a.category.cmp(&b.category).then_with(|| a.key.cmp(&b.key)));
    memory.facts.truncate(MAX_FACTS);
    save_global_unlocked(&memory)
}

pub fn remove_global(key: &str) -> Result<usize, String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut memory = load_global()?;
    let before = memory.facts.len();
    memory.facts.retain(|fact| !names_fact(key, fact));
    save_global_unlocked(&memory)?;
    Ok(before.saturating_sub(memory.facts.len()))
}

pub fn search_facts(root: Option<&Path>, query: &str, scope: &str) -> Vec<(String, MemoryFact)> {
    let query_words = query_terms(query);
    let mut results = Vec::new();
    let mut add = |scope_name: &str, facts: Vec<MemoryFact>| {
        for fact in facts {
            let score = if query_words.is_empty() {
                1
            } else {
                relevance(&query_words, &fact)
            };
            if score > 0 {
                results.push((scope_name.to_string(), score, fact));
            }
        }
    };
    if (scope == "all" || scope == "global")
        && let Ok(global_mem) = load_global()
    {
        add("global", global_mem.facts);
    }
    if (scope == "all" || scope == "project")
        && let Ok(proj_mem) = load(root)
    {
        add("project", proj_mem.facts);
    }

    results.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.confidence.cmp(&a.2.confidence))
            .then_with(|| b.2.updated_at.cmp(&a.2.updated_at))
    });

    results
        .into_iter()
        .take(16)
        .map(|(scope_name, _, fact)| (scope_name, fact))
        .collect()
}

#[cfg(test)]
pub fn render_relevant(root: Option<&Path>, query: &str, max_tokens: usize) -> Option<String> {
    select_relevant(root, query, max_tokens).text
}

/// What one memory selection injected, in counts only, for the
/// `memory.select` event (#2498). Keys are hashed; no fact value or query
/// text is kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemorySelectionStats {
    pub facts: usize,
    pub matched: usize,
    pub injected: usize,
    pub bytes: usize,
    pub cut_by_budget: usize,
    pub stale: usize,
    pub conflicts: usize,
    pub injected_key_hashes: Vec<String>,
}

impl MemorySelectionStats {
    pub fn event_fields(&self) -> serde_json::Value {
        serde_json::json!({
            "facts": self.facts,
            "matched": self.matched,
            "injected": self.injected,
            "bytes": self.bytes,
            "cut_by_budget": self.cut_by_budget,
            "stale": self.stale,
            "conflicts": self.conflicts,
            "injected_keys": self.injected_key_hashes,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemorySelection {
    pub text: Option<String>,
    pub stats: MemorySelectionStats,
}

const RELEVANT_MEMORY_HEADER: &str =
    "# Relevant memory (AGENTS.md/CLAUDE.md and the current task remain authoritative)\n";
const CONFLICT_MARKER: &str = "Conflicting; verify before relying:\n";
const STALE_FLAG: &str = " [stale; verify before relying]";

/// Rank project and global facts against `query` and render the ones that
/// fit `max_tokens`. Matching is by whole word, so "port" no longer matches
/// "imports"; stopwords never match, a key word outweighs a value word, and a
/// stale fact is flagged instead of dropped (#2496). Each line says who saved
/// the fact and how old it is, facts that share a key are grouped under a
/// conflict marker, and a closing note counts what the budget cut (#2497).
pub fn select_relevant(root: Option<&Path>, query: &str, max_tokens: usize) -> MemorySelection {
    let mut selection = MemorySelection::default();
    if max_tokens == 0 {
        return selection;
    }
    let query_words = query_terms(query);
    let mut all_facts = Vec::new();
    if let Ok(memory) = load(root) {
        all_facts.extend(memory.facts);
    }
    if let Ok(global_memory) = load_global() {
        all_facts.extend(global_memory.facts);
    }
    selection.stats.facts = all_facts.len();
    if all_facts.is_empty() {
        return selection;
    }
    let now = now();
    let mut ranked = all_facts
        .iter()
        .filter_map(|fact| {
            let score = relevance(&query_words, fact);
            (score > 0 || (query_words.is_empty() && fact.category == "global"))
                .then_some((score, fact))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.confidence.cmp(&a.1.confidence))
            .then_with(|| b.1.updated_at.cmp(&a.1.updated_at))
    });
    selection.stats.matched = ranked.len();
    if ranked.is_empty() {
        return selection;
    }
    // A matched key that another stored fact also uses is a conflict: show
    // every fact under it together, so the model sees both values.
    let mut key_counts = std::collections::HashMap::<&str, usize>::new();
    for fact in &all_facts {
        *key_counts.entry(fact.key.as_str()).or_default() += 1;
    }
    let mut plain = Vec::new();
    let mut conflicting = Vec::new();
    let mut conflict_keys = HashSet::new();
    for (_, fact) in &ranked {
        if key_counts.get(fact.key.as_str()).copied().unwrap_or(0) > 1 {
            if conflict_keys.insert(fact.key.as_str()) {
                conflicting.extend(all_facts.iter().filter(|other| other.key == fact.key));
            }
        } else {
            plain.push(*fact);
        }
    }
    selection.stats.conflicts = conflict_keys.len();

    let max_bytes = max_tokens.saturating_mul(4).min(8 * 1024);
    // Room kept for the closing note, so a cut is never silent.
    const NOTE_RESERVE: usize = 64;
    let mut output = String::from(RELEVANT_MEMORY_HEADER);
    let candidates = plain.len() + conflicting.len();
    let mut injected = 0;
    let mut stale_injected = 0;
    let mut add = |output: &mut String, fact: &MemoryFact, in_conflict: bool| -> bool {
        let room = max_bytes.saturating_sub(output.len() + NOTE_RESERVE);
        let (mut line, stale) = fact_line(fact, in_conflict, now, FACT_LINE_VALUE_BYTES);
        let fits = line.len() <= room;
        if !fits {
            // Shorten the value rather than lose the fact, when a useful
            // part of it still fits; nothing more is added after it.
            let overhead = line.len() - fact_line_value_len(fact);
            if room < overhead + 32 {
                return false;
            }
            line = fact_line(fact, in_conflict, now, room - overhead).0;
        }
        output.push_str(&line);
        injected += 1;
        stale_injected += usize::from(stale);
        selection
            .stats
            .injected_key_hashes
            .push(stable_key(&format!("{}/{}", fact.category, fact.key)));
        fits
    };
    let mut full = false;
    for fact in &plain {
        if !add(&mut output, fact, false) {
            full = true;
            break;
        }
    }
    if !full
        && !conflicting.is_empty()
        && output.len() + CONFLICT_MARKER.len() + NOTE_RESERVE < max_bytes
    {
        output.push_str(CONFLICT_MARKER);
        for fact in &conflicting {
            if !add(&mut output, fact, true) {
                break;
            }
        }
    }
    let cut = candidates.saturating_sub(injected);
    if cut > 0 {
        output.push_str(&format!(
            "({cut} more matching fact(s) cut by the memory budget; use recall_memory)\n"
        ));
    }
    selection.stats.injected = injected;
    selection.stats.stale = stale_injected;
    selection.stats.cut_by_budget = cut;
    if injected == 0 {
        return selection;
    }
    let text = output.trim_end().to_string();
    selection.stats.bytes = text.len();
    selection.text = Some(text);
    selection
}

/// One rendered fact: its value on a single line, who saved it, its age, and
/// its category when it sits under the conflict marker.
fn fact_line(
    fact: &MemoryFact,
    show_category: bool,
    now: u64,
    max_value_bytes: usize,
) -> (String, bool) {
    let age_seconds = now.saturating_sub(fact.updated_at);
    let stale = age_seconds > STALE_AFTER_SECONDS;
    let saved_by = if fact.source.to_lowercase().contains("user") {
        "user"
    } else {
        "agent"
    };
    let days = age_seconds / (24 * 60 * 60);
    let age = match days {
        0 => "today".to_string(),
        1 => "1 day ago".to_string(),
        days => format!("{days} days ago"),
    };
    let category = if show_category {
        format!("{}; ", fact.category)
    } else {
        String::new()
    };
    let value = truncate_utf8_bytes(&single_line(&fact.value), max_value_bytes);
    let line = format!(
        "- {}: {} ({category}saved by {saved_by}, {age}){}\n",
        fact.key,
        value,
        if stale { STALE_FLAG } else { "" }
    );
    (line, stale)
}

/// The longest value one injected line carries.
const FACT_LINE_VALUE_BYTES: usize = 320;

fn fact_line_value_len(fact: &MemoryFact) -> usize {
    truncate_utf8_bytes(&single_line(&fact.value), FACT_LINE_VALUE_BYTES).len()
}

/// Resolve memory without running repository discovery on the async executor.
/// The synchronous form remains useful for commands and deterministic tests;
/// turn preparation uses this wrapper because identity discovery invokes git.
pub async fn select_relevant_async(
    root: Option<PathBuf>,
    query: String,
    max_tokens: usize,
) -> MemorySelection {
    tokio::task::spawn_blocking(move || select_relevant(root.as_deref(), &query, max_tokens))
        .await
        .unwrap_or_default()
}

/// Tentative facts (low confidence) older than 7 days are garbage-collected;
/// confirmed facts persist. Returns the number of removed facts.
#[cfg(test)]
pub const TENTATIVE_CONFIDENCE_MAX: u8 = 30;
#[cfg(test)]
pub const TENTATIVE_GC_AFTER_SECONDS: u64 = 7 * 24 * 60 * 60;

#[cfg(test)]
pub fn gc_tentative_facts(memory: &mut ProjectMemory) -> usize {
    let before = memory.facts.len();
    let cutoff = now().saturating_sub(TENTATIVE_GC_AFTER_SECONDS);
    memory
        .facts
        .retain(|fact| !(fact.confidence <= TENTATIVE_CONFIDENCE_MAX && fact.updated_at < cutoff));
    before.saturating_sub(memory.facts.len())
}

/// Render facts within a byte budget, highest confidence + most recent first.
/// Returns the rendered text and the number of facts dropped by the budget.
#[cfg(test)]
pub fn to_system_message_with_budget(
    facts: &[MemoryFact],
    max_bytes: usize,
) -> (Option<String>, usize) {
    if facts.is_empty() || max_bytes == 0 {
        return (None, facts.len());
    }
    let mut ranked: Vec<&MemoryFact> = facts.iter().collect();
    ranked.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let mut output = String::from("# Project memory\n");
    let mut dropped = 0;
    for fact in ranked {
        let line = format!("- [{}] {} = {}\n", fact.category, fact.key, fact.value);
        if output.len() + line.len() <= max_bytes {
            output.push_str(&line);
        } else {
            dropped += 1;
        }
    }
    if output.lines().count() <= 1 {
        return (None, facts.len());
    }
    (Some(output.trim_end().to_string()), dropped)
}

/// How every refusal of a fact's content starts, so the tool layer can report
/// it as a validation error rather than an internal one.
const REFUSAL_PREFIX: &str = "project memory refuses";

pub(crate) fn is_refusal(error: &str) -> bool {
    error.starts_with(REFUSAL_PREFIX)
}

/// Refuse a literal credential, not a note about one: naming the environment
/// variable or file a credential comes from is what memory is for. The error
/// names the rule that matched and never the text it matched.
fn safe_fact(fact: &MemoryFact) -> Result<(), String> {
    let combined = format!(
        "{} {} {} {}",
        fact.category, fact.key, fact.value, fact.source
    );
    let lowered = combined.to_lowercase();
    if lowered.contains("agents.md") || lowered.contains("claude.md") {
        return Err(format!(
            "{REFUSAL_PREFIX} AGENTS.md/CLAUDE.md facts (rule: instruction file name)"
        ));
    }
    // The key and value are an assignment of their own. A lone word under a
    // credential-named key is the credential, as in an env file, unless it is
    // shaped like the name of an environment variable.
    let value = fact.value.trim();
    let variable_name = value.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
    });
    let separator = if value.contains(char::is_whitespace) || variable_name {
        ": "
    } else {
        "="
    };
    let rule = crate::network::output::literal_credential_rule(&combined).or_else(|| {
        crate::network::output::literal_credential_rule(&format!("{}{separator}{value}", fact.key))
    });
    if let Some(rule) = rule {
        return Err(format!(
            "{REFUSAL_PREFIX} a literal credential (rule: {rule}); name where it comes from instead, such as an environment variable or a file path"
        ));
    }
    Ok(())
}

fn valid_fact(fact: &MemoryFact) -> bool {
    clean_field(&fact.category, 48).is_ok()
        && clean_field(&fact.key, 96).is_ok()
        && clean_value(&fact.value).is_ok()
        && clean_field(&fact.source, 160).is_ok()
        && safe_fact(fact).is_ok()
}

fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    if max_bytes < "…".len() {
        return String::new();
    }
    let limit = max_bytes - "…".len();
    let mut output = String::new();
    for character in value.chars() {
        if output.len() + character.len_utf8() > limit {
            break;
        }
        output.push(character);
    }
    output.push('…');
    output
}

fn memory_write_lock() -> &'static Mutex<()> {
    MEMORY_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

fn clean_field(value: &str, max: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > max || value.contains('\n') {
        return Err("memory category/key/source is empty, multiline, or too long".to_string());
    }
    Ok(value.to_string())
}

fn clean_value(value: &str) -> Result<String, String> {
    let value = single_line(value);
    if value.is_empty() || value.len() > MAX_FACT_VALUE_BYTES || value.contains('\0') {
        return Err("memory fact value is empty or too large".to_string());
    }
    Ok(value)
}

/// `value` with every run of whitespace, line breaks included, as one space,
/// so a stored value can never start a line of the context tail.
fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words too common to say what a request is about. They never match.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as", "at",
    "be", "been", "before", "but", "by", "can", "could", "did", "do", "does", "doing", "done",
    "for", "from", "get", "got", "had", "has", "have", "help", "here", "hi", "how", "i", "if",
    "in", "into", "is", "it", "its", "just", "let", "like", "make", "may", "me", "might", "my",
    "need", "no", "not", "now", "of", "ok", "okay", "on", "one", "or", "our", "out", "please",
    "run", "should", "so", "some", "than", "thank", "thanks", "that", "the", "their", "them",
    "then", "there", "these", "they", "this", "those", "to", "too", "up", "us", "use", "very",
    "want", "was", "we", "were", "what", "when", "where", "which", "while", "who", "why", "will",
    "with", "would", "yes", "you", "your",
];

/// Lowercase words of `value`, split on anything that is not a letter or
/// digit (so `test_runner` is `test` and `runner`), in any script.
fn tokens(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// The distinct non-stopword words of a query.
fn query_terms(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    tokens(value)
        .filter(|word| {
            word.chars().count() >= 2
                && !STOPWORDS.contains(&word.as_str())
                && seen.insert(word.clone())
        })
        .collect()
}

/// How well `fact` matches the query words: each word scores 3 when it is a
/// word of the key, 2 of the category and 1 of the value, best place only.
fn relevance(query_words: &[String], fact: &MemoryFact) -> usize {
    let key = tokens(&fact.key).collect::<HashSet<_>>();
    let category = tokens(&fact.category).collect::<HashSet<_>>();
    let value = tokens(&fact.value).collect::<HashSet<_>>();
    query_words
        .iter()
        .map(|word| {
            if key.contains(word) {
                3
            } else if category.contains(word) {
                2
            } else if value.contains(word) {
                1
            } else {
                0
            }
        })
        .sum()
}

fn stable_key(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn git_value(root: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string()))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

pub fn fact(category: &str, key: &str, value: &str, source: &str) -> MemoryFact {
    MemoryFact {
        category: category.to_string(),
        key: key.to_string(),
        value: value.to_string(),
        source: source.to_string(),
        confidence: 80,
        updated_at: now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_is_scoped_and_deduplicated() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        fs::create_dir_all(&path).unwrap();
        upsert(Some(&path), fact("command", "test", "cargo test", "user")).unwrap();
        upsert(
            Some(&path),
            fact("command", "test", "cargo test --all", "user"),
        )
        .unwrap();
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.facts[0].value, "cargo test --all");
    }

    #[test]
    fn memory_rejects_authoritative_docs_and_secret_like_facts() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            upsert(
                Some(root.path()),
                fact("convention", "docs", "follow the repo", "AGENTS.md")
            )
            .is_err()
        );
        for (key, value) in [
            ("api_key", "0a1b2c3d4e5f"),
            ("password", "hunter"),
            ("deploy", "export DEPLOY_TOKEN=abcdefgh before running"),
            ("deploy", "the password is hunter2"),
            ("ci", "curl -H 'Authorization: Bearer abc123def' https://ci"),
            ("openai", "key sk-proj-0a1b2c3d4e5f6g7h"),
            ("github", "ghp_0123456789abcdefghij"),
            ("ssh", "-----BEGIN OPENSSH PRIVATE KEY-----"),
        ] {
            let error = upsert(Some(root.path()), fact("environment", key, value, "user"))
                .expect_err(value);
            assert!(is_refusal(&error), "{error}");
            assert!(error.contains("(rule: "), "{error}");
            // The rule is named; the text that matched it is not repeated.
            assert!(
                !value
                    .split_whitespace()
                    .any(|word| word.len() > 5 && error.contains(word)),
                "{error}"
            );
        }
        assert!(load(Some(root.path())).unwrap().facts.is_empty());
    }

    #[test]
    fn memory_stores_notes_that_name_a_credential_source() {
        let root = tempfile::tempdir().unwrap();
        // The note refused in session 01a11ffc8dd0: no credential is in it.
        let session_note = "Time-logging MCP at /Users/lagos/code/worklog_mcp (Rust, rmcp 3.2, stdio). Registered in ~/.config/rustcode/config.toml as [[mcp_servers]] name=\"worklog\". Token via SOLIDTIME_API_KEY, or WORKLOG_ENV_FILE=.env containing TOKEN. Times are Europe/Oslo local in/out.";
        let notes = [
            ("worklog_mcp", session_note),
            ("api_key", "SOLIDTIME_API_KEY"),
            ("solidtime token", "read from $SOLIDTIME_API_KEY"),
            ("db password", "/run/secrets/db_password"),
            (
                "deploy",
                "export TOKEN=$DEPLOY_TOKEN, then run the task-runner",
            ),
            (
                "skills",
                "risk-review and ask-first are skills; max_tokens=4096",
            ),
        ];
        for (key, value) in notes {
            upsert(Some(root.path()), fact("project", key, value, "user")).expect(value);
        }
        assert_eq!(load(Some(root.path())).unwrap().facts.len(), notes.len());
    }

    #[test]
    fn relevant_rendering_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        upsert(
            Some(root.path()),
            fact("architecture", "agent loop", &"x".repeat(700), "user"),
        )
        .unwrap();
        let rendered = render_relevant(Some(root.path()), "agent loop", 128).unwrap();
        assert!(rendered.len() <= 8 * 1024);
    }

    /// The task directory differs from the process directory, as in an ACP
    /// session or a worktree: a fact the tool saves is the fact the next
    /// turn injects (#2495).
    #[test]
    fn memory_tools_resolve_against_the_session_task_directory() {
        let task = tempfile::tempdir().unwrap();
        let task_root = fs::canonicalize(task.path()).unwrap();
        assert_ne!(Some(task_root.clone()), std::env::current_dir().ok());
        crate::tools::set_active_workspace_context(
            Some(task_root.clone()),
            Some(task_root.clone()),
            false,
            None,
        );
        let saved = crate::tools::execute_with_metadata(
            "remember",
            &serde_json::json!({
                "key": "staging_host",
                "value": "Deploy previews go to staging-7",
                "category": "deploy",
            }),
        );
        assert!(saved.success, "{}", saved.content);
        let recalled = crate::tools::execute_with_metadata(
            "recall_memory",
            &serde_json::json!({"query": "staging"}),
        )
        .content;
        crate::tools::set_active_workspace_context(None, None, false, None);

        assert!(recalled.contains("staging_host"), "{recalled}");
        // The per-turn injection resolves the same root the request uses.
        let injected = render_relevant(Some(&task_root), "staging host", 192).unwrap();
        assert!(injected.contains("staging_host"));
        assert!(
            render_relevant(std::env::current_dir().ok().as_deref(), "staging host", 192).is_none()
        );
    }

    #[test]
    fn memory_select_event_carries_counts_and_hashed_keys_only() {
        let root = tempfile::tempdir().unwrap();
        upsert(
            Some(root.path()),
            fact(
                "secretive",
                "launch_codename",
                "Project Bluebird ships in May",
                "user",
            ),
        )
        .unwrap();
        let query = "when does bluebird launch codename ship";
        let selection = select_relevant(Some(root.path()), query, 192);
        assert_eq!(selection.stats.injected, 1);
        let event = selection.stats.event_fields().to_string();
        for private in [
            "Bluebird",
            "bluebird",
            "launch_codename",
            "secretive",
            "May",
            query,
        ] {
            assert!(!event.contains(private), "{private} leaked into {event}");
        }
        assert_eq!(
            selection.stats.injected_key_hashes,
            vec![stable_key("secretive/launch_codename")]
        );
        let fields = selection.stats.event_fields();
        for count in [
            "facts",
            "matched",
            "injected",
            "bytes",
            "cut_by_budget",
            "stale",
            "conflicts",
        ] {
            assert!(fields[count].is_u64(), "{count}");
        }
    }

    #[test]
    fn relevant_rendering_does_not_split_utf8_or_exceed_byte_budget() {
        let root = tempfile::tempdir().unwrap();
        upsert(
            Some(root.path()),
            fact("architecture", "parser", &"é".repeat(300), "user"),
        )
        .unwrap();
        let rendered = render_relevant(Some(root.path()), "parser", 64).unwrap();
        assert!(rendered.len() <= 256);
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }

    #[test]
    fn facts_can_be_removed_and_project_memory_can_be_reset() {
        let root = tempfile::tempdir().unwrap();
        upsert(
            Some(root.path()),
            fact("command", "build", "cargo check", "explicit user note"),
        )
        .unwrap();
        assert_eq!(remove(Some(root.path()), "build").unwrap(), 1);
        upsert(
            Some(root.path()),
            fact(
                "decision",
                "layout",
                "keep modules small",
                "explicit user note",
            ),
        )
        .unwrap();
        let loaded = load(Some(root.path())).unwrap();
        save(Some(root.path()), &loaded).unwrap();
        let path = location(Some(root.path())).path;
        assert!(path.exists());
        reset(Some(root.path())).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn durable_command_interface_requires_an_explicit_subcommand() {
        let root = tempfile::tempdir().unwrap();
        assert!(command(Some(root.path()), &[]).is_none());
        assert!(
            command(
                Some(root.path()),
                &["add", "command", "test", "cargo", "test"]
            )
            .unwrap()
            .contains("Saved")
        );
        assert!(
            command(Some(root.path()), &["show"])
                .unwrap()
                .contains("cargo test")
        );
    }

    #[test]
    fn corrupt_or_mismatched_memory_is_not_injected() {
        let root = tempfile::tempdir().unwrap();
        let path = location(Some(root.path())).path;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert!(load(Some(root.path())).is_err());
        assert!(render_relevant(Some(root.path()), "anything", 256).is_none());
    }

    #[test]
    fn unrelated_high_confidence_facts_are_not_injected() {
        let root = tempfile::tempdir().unwrap();
        let mut unrelated = fact(
            "environment",
            "docker host",
            "use the staging container",
            "explicit user note",
        );
        unrelated.confidence = 100;
        upsert(Some(root.path()), unrelated).unwrap();
        assert!(render_relevant(Some(root.path()), "fix parser", 256).is_none());

        let mut global = fact("global", "language", "Rust project", "explicit user note");
        global.confidence = 100;
        upsert(Some(root.path()), global).unwrap();
        assert!(render_relevant(Some(root.path()), "", 256).is_some());
    }

    #[test]
    fn repository_memory_identity_is_worktree_and_symlink_safe() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let worktree_a = temp.path().join("worktree-a");
        let worktree_b = temp.path().join("worktree-b");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "root\n").unwrap();
        let git = |cwd: &Path, args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(cwd)
                .status()
                .unwrap();
            assert!(status.success(), "git command failed: {args:?}");
        };
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "README.md"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=RustCode Test",
                "-c",
                "user.email=rustcode@example.test",
                "commit",
                "-qm",
                "initial",
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "work-a",
                worktree_a.to_str().unwrap(),
                "HEAD",
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "work-b",
                worktree_b.to_str().unwrap(),
                "HEAD",
            ],
        );

        let main_location = location(Some(&repo));
        let a_location = location(Some(&worktree_a));
        let b_location = location(Some(&worktree_b));
        assert_eq!(main_location.root, repository_root(Some(&repo)));
        assert_ne!(a_location.identity, b_location.identity);
        assert!(a_location.identity.contains(worktree_a.to_str().unwrap()));
        assert!(b_location.identity.contains(worktree_b.to_str().unwrap()));
        assert_eq!(
            a_location.identity.lines().nth(1),
            b_location.identity.lines().nth(1)
        );

        #[cfg(unix)]
        {
            let symlink = temp.path().join("repo-link");
            std::os::unix::fs::symlink(&repo, &symlink).unwrap();
            assert_eq!(location(Some(&symlink)).identity, main_location.identity);
        }

        let non_git = temp.path().join("not-a-repo");
        fs::create_dir_all(&non_git).unwrap();
        assert_eq!(
            repository_root(Some(&non_git)),
            fs::canonicalize(&non_git).unwrap()
        );
        assert_ne!(location(Some(&non_git)).identity, main_location.identity);
    }

    #[test]
    fn global_memory_lifecycle_and_search() {
        upsert_global(fact("preference", "editor", "kakoune", "user")).unwrap();
        let loaded = load_global().unwrap();
        assert!(
            loaded
                .facts
                .iter()
                .any(|f| f.key == "editor" && f.value == "kakoune")
        );

        let results = search_facts(None, "kakoune", "global");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.key, "editor");

        remove_global("editor").unwrap();
        let after = load_global().unwrap();
        assert!(!after.facts.iter().any(|f| f.key == "editor"));
    }

    #[test]
    fn search_facts_multi_scope() {
        let root = tempfile::tempdir().unwrap();
        upsert(
            Some(root.path()),
            fact("build", "package_manager", "pnpm", "user"),
        )
        .unwrap();
        upsert_global(fact("style", "indent", "4 spaces", "user")).unwrap();

        let proj_results = search_facts(Some(root.path()), "pnpm", "project");
        assert_eq!(proj_results.len(), 1);
        assert_eq!(proj_results[0].1.key, "package_manager");

        let global_results = search_facts(Some(root.path()), "indent", "global");
        assert_eq!(global_results.len(), 1);
        assert_eq!(global_results[0].1.key, "indent");

        let all_results = search_facts(Some(root.path()), "", "all");
        assert!(all_results.len() >= 2);

        // cleanup
        remove(Some(root.path()), "package_manager").unwrap();
        remove_global("indent").unwrap();
    }

    fn tentative_fact(key: &str, confidence: u8, updated_at: u64) -> MemoryFact {
        MemoryFact {
            category: "note".to_string(),
            key: key.to_string(),
            value: "v".to_string(),
            source: "test".to_string(),
            confidence,
            updated_at,
        }
    }

    #[test]
    fn gc_removes_only_stale_tentative_facts() {
        let old = now().saturating_sub(TENTATIVE_GC_AFTER_SECONDS + 60);
        let mut memory = ProjectMemory {
            version: MEMORY_VERSION,
            identity: "test".to_string(),
            facts: vec![
                tentative_fact("old-tentative", 10, old),
                tentative_fact("fresh-tentative", 10, now()),
                tentative_fact("old-confirmed", 90, old),
            ],
        };
        assert_eq!(gc_tentative_facts(&mut memory), 1);
        assert_eq!(memory.facts.len(), 2);
        assert!(memory.facts.iter().all(|f| f.key != "old-tentative"));
    }

    #[test]
    fn budget_rendering_prefers_confidence_and_reports_dropped() {
        let facts = vec![
            tentative_fact("low", 10, now()),
            tentative_fact("high", 95, now()),
        ];
        // Header (17B) + one 19B line fits in 40B; the second fact drops.
        let (rendered, dropped) = to_system_message_with_budget(&facts, 40);
        let text = rendered.expect("budget fits at least one fact");
        assert!(text.contains("high"));
        assert_eq!(dropped, 1);
        let (none, all_dropped) = to_system_message_with_budget(&facts, 5);
        assert!(none.is_none());
        assert_eq!(all_dropped, 2);
    }
}

#[cfg(test)]
#[path = "memory_eval.rs"]
mod memory_eval;
