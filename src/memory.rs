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

pub fn remove(root: Option<&Path>, key: &str) -> Result<usize, String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut memory = load(root)?;
    let before = memory.facts.len();
    memory
        .facts
        .retain(|fact| fact.key != key && fact.category != key);
    save_unlocked(root, &memory)?;
    Ok(before.saturating_sub(memory.facts.len()))
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
            None => Ok("Usage: /memory forget <key-or-category>".to_string()),
        },
        Some("add") if args.len() >= 4 => upsert(
            root,
            fact(args[1], args[2], &args[3..].join(" "), "explicit user note"),
        )
        .map(|()| format!("Saved project-memory fact '{}'.", args[2])),
        Some("add") => Ok("Usage: /memory add <category> <key> <short fact>".to_string()),
        Some(_) => Ok(
            "Usage: /memory [show|path|add <category> <key> <fact>|forget <key>|reset|reset-all]"
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
    let mut memory: ProjectMemory = serde_json::from_str(&raw).map_err(|error| {
        format!("invalid global memory {}: {error}", path.display())
    })?;
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

#[allow(dead_code)]
pub fn save_global(memory: &ProjectMemory) -> Result<(), String> {
    let _guard = memory_write_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    save_global_unlocked(memory)
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
    memory
        .facts
        .retain(|fact| fact.key != key && fact.category != key);
    save_global_unlocked(&memory)?;
    Ok(before.saturating_sub(memory.facts.len()))
}

pub fn search_facts(
    root: Option<&Path>,
    query: &str,
    scope: &str,
) -> Vec<(String, MemoryFact)> {
    let query_words = words(query);
    let mut results = Vec::new();

    if scope == "all" || scope == "global" {
        if let Ok(global_mem) = load_global() {
            for fact in global_mem.facts {
                let haystack = format!("{} {} {}", fact.category, fact.key, fact.value).to_lowercase();
                let score = if query_words.is_empty() {
                    1
                } else {
                    query_words.iter().filter(|w| haystack.contains(*w)).count()
                };
                if score > 0 {
                    results.push(("global".to_string(), score, fact));
                }
            }
        }
    }

    if scope == "all" || scope == "project" {
        if let Ok(proj_mem) = load(root) {
            for fact in proj_mem.facts {
                let haystack = format!("{} {} {}", fact.category, fact.key, fact.value).to_lowercase();
                let score = if query_words.is_empty() {
                    1
                } else {
                    query_words.iter().filter(|w| haystack.contains(*w)).count()
                };
                if score > 0 {
                    results.push(("project".to_string(), score, fact));
                }
            }
        }
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

pub fn render_relevant(root: Option<&Path>, query: &str, max_tokens: usize) -> Option<String> {
    if max_tokens == 0 {
        return None;
    }
    let query_words = words(query);
    let mut all_facts = Vec::new();
    if let Ok(memory) = load(root) {
        all_facts.extend(memory.facts);
    }
    if let Ok(global_memory) = load_global() {
        all_facts.extend(global_memory.facts);
    }
    if all_facts.is_empty() {
        return None;
    }
    let mut ranked = all_facts
        .into_iter()
        .filter_map(|fact| {
            let stale = now().saturating_sub(fact.updated_at) > STALE_AFTER_SECONDS;
            let haystack = format!("{} {} {}", fact.category, fact.key, fact.value).to_lowercase();
            let score = query_words
                .iter()
                .filter(|word| haystack.contains(*word))
                .count() as i32
                - i32::from(stale);
            (score > 0 || (query_words.is_empty() && fact.category == "global"))
                .then_some((score, stale, fact))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.2.confidence.cmp(&a.2.confidence))
    });
    let max_bytes = max_tokens.saturating_mul(4).min(8 * 1024);
    let mut output = String::from(
        "# Relevant memory (AGENTS.md/CLAUDE.md and the current task remain authoritative)\n",
    );
    for (_, stale, fact) in ranked {
        let freshness = if stale {
            " [stale; verify before relying]"
        } else {
            ""
        };
        let line = format!("- {}: {}{}\n", fact.key, fact.value, freshness);
        let remaining = max_bytes.saturating_sub(output.len());
        if remaining == 0 {
            break;
        }
        if line.len() <= remaining {
            output.push_str(&line);
        } else {
            output.push_str(&truncate_utf8_bytes(&line, remaining));
            break;
        }
    }
    (output.lines().count() > 1).then_some(output.trim_end().to_string())
}

/// Resolve memory without running repository discovery on the async executor.
/// The synchronous form remains useful for commands and deterministic tests;
/// turn preparation uses this wrapper because identity discovery invokes git.
pub async fn render_relevant_async(
    root: Option<PathBuf>,
    query: String,
    max_tokens: usize,
) -> Option<String> {
    tokio::task::spawn_blocking(move || render_relevant(root.as_deref(), &query, max_tokens))
        .await
        .ok()
        .flatten()
}

fn safe_fact(fact: &MemoryFact) -> Result<(), String> {
    let combined = format!(
        "{} {} {} {}",
        fact.category, fact.key, fact.value, fact.source
    )
    .to_lowercase();
    if combined.contains("password")
        || combined.contains("secret")
        || combined.contains("api_key")
        || combined.contains("access_token")
        || combined.contains("bearer ")
        || combined.contains("token=")
        || combined.contains("token:")
        || combined.contains("private key")
        || combined.contains("-----begin")
        || combined.contains("ghp_")
        || combined.contains("github_pat_")
        || combined.contains("sk-")
        || combined.contains("xoxb-")
        || combined.contains("akia")
        || combined.contains("agents.md")
        || combined.contains("claude.md")
    {
        return Err("project memory refuses secrets and AGENTS.md/CLAUDE.md facts".to_string());
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
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_FACT_VALUE_BYTES || value.contains('\0') {
        return Err("memory fact value is empty or too large".to_string());
    }
    Ok(value.to_string())
}

fn words(value: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .map(str::to_ascii_lowercase)
        .filter(|word| word.len() >= 3 && seen.insert(word.clone()))
        .collect()
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
        assert!(
            upsert(
                Some(root.path()),
                fact("environment", "api_key", "do not save", "user")
            )
            .is_err()
        );
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
        assert!(loaded.facts.iter().any(|f| f.key == "editor" && f.value == "kakoune"));

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
        upsert(Some(root.path()), fact("build", "package_manager", "pnpm", "user")).unwrap();
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
}
