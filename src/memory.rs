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
use std::time::{SystemTime, UNIX_EPOCH};

const MEMORY_VERSION: u32 = 1;
const MAX_FACTS: usize = 64;
const MAX_FACT_VALUE_BYTES: usize = 768;
const MAX_MEMORY_BYTES: usize = 32 * 1024;
const STALE_AFTER_SECONDS: u64 = 180 * 24 * 60 * 60;
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    let Ok(raw) = fs::read_to_string(&location.path) else {
        return Ok(ProjectMemory {
            version: MEMORY_VERSION,
            identity: location.identity,
            facts: Vec::new(),
        });
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
    memory.facts.retain(|fact| safe_fact(fact).is_ok());
    memory.facts.truncate(MAX_FACTS);
    Ok(memory)
}

pub fn save(root: Option<&Path>, memory: &ProjectMemory) -> Result<(), String> {
    let location = location(root);
    if memory.version != MEMORY_VERSION || memory.identity != location.identity {
        return Err("project memory identity/version mismatch".to_string());
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
    save(root, &memory)
}

pub fn remove(root: Option<&Path>, key: &str) -> Result<usize, String> {
    let mut memory = load(root)?;
    let before = memory.facts.len();
    memory
        .facts
        .retain(|fact| fact.key != key && fact.category != key);
    save(root, &memory)?;
    Ok(before.saturating_sub(memory.facts.len()))
}

pub fn reset(root: Option<&Path>) -> Result<(), String> {
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

pub fn render_relevant(root: Option<&Path>, query: &str, max_tokens: usize) -> Option<String> {
    let memory = load(root).ok()?;
    if memory.facts.is_empty() || max_tokens == 0 {
        return None;
    }
    let query_words = words(query);
    let mut ranked = memory
        .facts
        .into_iter()
        .filter_map(|fact| {
            let stale = now().saturating_sub(fact.updated_at) > STALE_AFTER_SECONDS;
            let haystack = format!("{} {} {}", fact.category, fact.key, fact.value).to_lowercase();
            let score = query_words
                .iter()
                .filter(|word| haystack.contains(*word))
                .count() as i32
                - i32::from(stale);
            (score > 0 || fact.confidence >= 90).then_some((score, stale, fact))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.2.confidence.cmp(&a.2.confidence))
    });
    let max_bytes = max_tokens.saturating_mul(4).clamp(256, 8 * 1024);
    let mut output = String::from(
        "# Relevant project memory (AGENTS.md/CLAUDE.md and the current task remain authoritative)\n",
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
            let mut truncated = line
                .chars()
                .take(remaining.saturating_sub(1))
                .collect::<String>();
            truncated.push('…');
            output.push_str(&truncated);
            break;
        }
    }
    (output.lines().count() > 1).then_some(output.trim_end().to_string())
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
        || combined.contains("agents.md")
        || combined.contains("claude.md")
    {
        return Err("project memory refuses secrets and AGENTS.md/CLAUDE.md facts".to_string());
    }
    Ok(())
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
}
