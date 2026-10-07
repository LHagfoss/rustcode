use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

static SKILL_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static SKILL_CATALOG_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_skill_catalog_tests() -> MutexGuard<'static, ()> {
    SKILL_CATALOG_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub content: String,
}

pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub triggers: Vec<String>,
    pub keywords: Vec<String>,
    pub priority: i32,
}

pub fn discover_skills() -> Vec<SkillMetadata> {
    discover_skills_in(current_root_inputs())
}

/// Generation of the live skill catalog. Prompt caches use this to refresh
/// after an explicit catalog read without rescanning roots on every turn.
pub fn skill_catalog_generation() -> u64 {
    SKILL_CATALOG_GENERATION.load(Ordering::Relaxed)
}

/// Invalidate cached routing metadata after a live skill lookup.
pub fn bump_skill_catalog_generation() {
    SKILL_CATALOG_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Discover skills for a user-facing catalog request and invalidate cached
/// routing metadata so the next model request observes the same catalog.
pub fn discover_skills_for_catalog() -> Vec<SkillMetadata> {
    let skills = discover_skills();
    bump_skill_catalog_generation();
    skills
}

/// The roots this process should search, read from the workspace, the
/// environment and the user config.
fn current_root_inputs() -> SkillRootInputs {
    let config_dir = crate::config::get_config_dir();
    // `load_config_from` reads the user config only, so a checked-out project
    // config can never widen skill discovery.
    let extra_dirs = config_dir
        .as_deref()
        .map(|dir| crate::config::load_config_from(dir).2.extra_skill_dirs)
        .unwrap_or_default();
    SkillRootInputs {
        // Prefer the explicit workspace the tool call is running against; fall
        // back to the process CWD when no session has established one.
        workspace: crate::tools::active_workspace_root().or_else(|| std::env::current_dir().ok()),
        config_dir,
        home: std::env::var_os("HOME").map(PathBuf::from),
        extra_dirs,
        extra_dirs_env: std::env::var_os("RUSTCODE_EXTRA_SKILL_DIRS"),
    }
}

/// Discovery over explicit roots. Roots are scanned in the order
/// [`skill_roots_from`] returns and the first definition of a name wins, so
/// project roots override user-level ones.
pub fn discover_skills_in(inputs: SkillRootInputs) -> Vec<SkillMetadata> {
    let mut skills: Vec<SkillMetadata> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for root in skill_roots_from(&inputs) {
        let mut found = Vec::new();
        scan_skill_dir(&root.path, &mut found);
        for skill in found {
            if seen.insert(skill.name.clone()) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Where a skill root came from. Used by `rustcode doctor` and the `/skills`
/// report so a user can tell which directory a skill was picked up from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillRootKind {
    /// `.rustcode/skills` or `.agents/skills` inside the workspace.
    Workspace,
    /// The tool-agnostic root shared with other agents (`~/.agents/skills`).
    Universal,
    /// RustCode's own root inside its config directory.
    Rustcode,
    /// `extra_skill_dirs` / `RUSTCODE_EXTRA_SKILL_DIRS`.
    Configured,
}

impl SkillRootKind {
    /// RustCode creates only the roots it owns; shared and workspace roots
    /// belong to whoever put them there.
    pub fn is_rustcode_owned(self) -> bool {
        matches!(self, Self::Rustcode)
    }

    /// Stable check name used by `rustcode doctor`, which keys its report on
    /// `&'static str`.
    pub fn check_name(self) -> &'static str {
        match self {
            Self::Workspace => "project-skills",
            Self::Universal => "skills-dir[universal]",
            Self::Rustcode => "skills-dir",
            Self::Configured => "skills-dir[configured]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub kind: SkillRootKind,
}

/// Everything [`skill_roots_from`] needs, so root resolution stays pure and
/// testable without touching the process environment.
#[derive(Debug, Clone, Default)]
pub struct SkillRootInputs {
    /// Workspace root holding `.rustcode/skills` and `.agents/skills`.
    pub workspace: Option<PathBuf>,
    /// RustCode's own config directory, normally [`crate::config::get_config_dir`].
    pub config_dir: Option<PathBuf>,
    /// The user's home directory, when it is known.
    pub home: Option<PathBuf>,
    /// `extra_skill_dirs` from the global config.
    pub extra_dirs: Vec<PathBuf>,
    /// `RUSTCODE_EXTRA_SKILL_DIRS`, using the platform path-list separator.
    pub extra_dirs_env: Option<OsString>,
}

/// Assembles every skill root in precedence order, highest first.
///
/// Explicit roots (`extra_skill_dirs`, `RUSTCODE_EXTRA_SKILL_DIRS`) override
/// everything, then project-local roots override user-level ones. We
/// deliberately do *not* scan `.claude/skills`: that is Claude Code's
/// directory, and inheriting it dumped unrelated plugin skills (Cloudflare
/// Workers, etc.) into every prompt. Users who really want to share those can
/// add the directory to `extra_skill_dirs`.
pub fn skill_roots_from(inputs: &SkillRootInputs) -> Vec<SkillRoot> {
    let mut roots: Vec<SkillRoot> = Vec::new();
    let mut push = |path: PathBuf, kind: SkillRootKind| {
        if !path.as_os_str().is_empty() && !roots.iter().any(|root| root.path == path) {
            roots.push(SkillRoot { path, kind });
        }
    };

    for dir in &inputs.extra_dirs {
        push(dir.clone(), SkillRootKind::Configured);
    }
    if let Some(value) = inputs.extra_dirs_env.as_deref() {
        for dir in split_skill_dirs(value) {
            push(dir, SkillRootKind::Configured);
        }
    }

    if let Some(workspace) = &inputs.workspace {
        push(workspace.join(".rustcode/skills"), SkillRootKind::Workspace);
        push(workspace.join(".agents/skills"), SkillRootKind::Workspace);
    }

    if let Some(home) = &inputs.home {
        push(home.join(".agents/skills"), SkillRootKind::Universal);
    }
    if let Some(config_dir) = &inputs.config_dir {
        push(config_dir.join("skills"), SkillRootKind::Rustcode);
    }

    roots
}

/// The skill roots searched by [`discover_skills`], resolved from the running
/// process.
pub fn skill_roots() -> Vec<SkillRoot> {
    skill_roots_from(&current_root_inputs())
}

/// The roots actually searched, formatted once so `list_skills`, both
/// `/skills` handlers and `rustcode doctor` cannot drift from discovery.
pub fn format_skill_roots(roots: &[SkillRoot]) -> String {
    let mut out = String::from("Skill roots searched (highest priority first):");
    if roots.is_empty() {
        out.push_str("\n  (none)");
    }
    for (index, root) in roots.iter().enumerate() {
        let exists = if root.path.is_dir() {
            ""
        } else {
            "  (missing)"
        };
        out.push_str(&format!(
            "\n  {}. {}{exists}",
            index + 1,
            root.path.display()
        ));
    }
    out
}

/// Message shown when no skills were found: what was searched, and where to
/// put new ones.
pub fn no_skills_message() -> String {
    format!(
        "No skills discovered.\nPut `SKILL.md` files in `<root>/<name>/SKILL.md` under any of:\n{}",
        format_skill_roots(&skill_roots())
    )
}

/// `/skills` report, shared by the engine and TUI slash-command handlers so
/// the wording and the root list cannot drift between frontends.
pub fn format_skill_catalog(skills: &[SkillMetadata]) -> String {
    if skills.is_empty() {
        return no_skills_message();
    }
    let mut out = format!("📦 Discovered Skills ({}):\n\n", skills.len());
    for skill in skills {
        out.push_str(&format!("  • {}\n", skill.name));
        out.push_str(&format!("    Description: {}\n", skill.description));
        out.push_str(&format!("    Path: {}\n\n", skill.path.display()));
    }
    out.push_str(&format_skill_roots(&skill_roots()));
    out
}

fn is_skill_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

fn prompt_mentions_skill(prompt: &str, skill_name: &str) -> bool {
    if skill_name.is_empty() {
        return false;
    }

    let prompt = prompt.to_ascii_lowercase();
    let skill_name = skill_name.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative_start) = prompt[search_from..].find(&skill_name) {
        let start = search_from + relative_start;
        let end = start + skill_name.len();
        let before = prompt[..start].chars().next_back();
        let after = prompt[end..].chars().next();
        if before.is_none_or(|c| !is_skill_name_char(c))
            && after.is_none_or(|c| !is_skill_name_char(c))
        {
            return true;
        }
        search_from = end;
    }
    false
}

pub fn skill_routing_hint(
    prompt: &str,
    skills: &[SkillMetadata],
    loaded_skills: &[String],
) -> Option<String> {
    let skill = skills.iter().find(|skill| {
        prompt_mentions_skill(prompt, &skill.name)
            && !loaded_skills
                .iter()
                .any(|loaded| loaded.eq_ignore_ascii_case(&skill.name))
    })?;
    Some(format!(
        "# Priority skill route\nThe latest user prompt explicitly names available skill `{}`. Call `use_skill` first with the exact name `{}` before any filesystem, web, or exploration tool.",
        skill.name, skill.name
    ))
}

/// Small deterministic vocabulary shared by all skills, not application names.
fn routing_terms(text: &str) -> std::collections::BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| {
            let normalized = match word {
                "song" | "songs" | "music" | "tracks" | "track" => "music",
                "listening" | "listen" | "playing" | "play" | "playback" => "playback",
                "meetings" | "meeting" | "calendar" => "calendar",
                "entries" | "entry" => "entry",
                "what" | "which" | "when" | "where" | "how" | "the" | "a" | "an" | "i" | "am"
                | "to" | "of" | "and" | "or" | "for" | "use" | "using" | "with" | "my" | "me"
                | "you" | "is" | "are" | "can" | "please" | "current" | "currently" | "manage"
                | "control" | "want" => return None,
                value => value,
            };
            (normalized.len() > 2).then(|| normalized.to_string())
        })
        .collect()
}

fn skill_relevance_score(prompt_lower: &str, skill: &SkillMetadata) -> i32 {
    let mut score: i32 = 0;
    for trigger in &skill.triggers {
        if !trigger.is_empty() && prompt_lower.contains(trigger) {
            score += 10;
        }
    }
    for keyword in &skill.keywords {
        if !keyword.is_empty() && prompt_lower.contains(keyword) {
            score += 5;
        }
    }
    // Description-only routing requires two distinct intent terms. Coding
    // requests cannot activate live-app workflows just because source mentions
    // music, email or calendar data. Explicit metadata remains authoritative.
    if score == 0 {
        let prompt_terms = routing_terms(prompt_lower);
        let coding = [
            "implement",
            "parser",
            "function",
            "struct",
            "code",
            "refactor",
            "bug",
            "api",
        ]
        .iter()
        .any(|word| prompt_terms.contains(*word));
        if !coding {
            let description_terms = routing_terms(&skill.description);
            let overlap = prompt_terms.intersection(&description_terms).count();
            if overlap >= 2 {
                score = (overlap.min(4) as i32) * 3;
            }
        }
    }
    // Priority nudges ordering but never promotes an irrelevant skill alone.
    if score > 0 {
        score += skill.priority.clamp(-10, 10);
    }
    score
}

/// Rank skills by trigger/keyword relevance for a prompt, excluding already
/// loaded skills. Explicit name mentions are handled by
/// [`skill_routing_hint`]; this covers the softer `triggers`/`keywords`
/// frontmatter path (fortunto2-style `SkillRegistry::select`).
pub fn select_skills_for_prompt<'a>(
    prompt: &str,
    skills: &'a [SkillMetadata],
    loaded_skills: &[String],
    limit: usize,
) -> Vec<(&'a SkillMetadata, i32)> {
    let prompt_lower = prompt.to_ascii_lowercase();
    let mut scored: Vec<(&SkillMetadata, i32)> = skills
        .iter()
        .filter(|skill| {
            !loaded_skills
                .iter()
                .any(|loaded| loaded.eq_ignore_ascii_case(&skill.name))
                && !prompt_mentions_skill(prompt, &skill.name)
        })
        .map(|skill| (skill, skill_relevance_score(&prompt_lower, skill)))
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(&b.0.name)));
    scored.truncate(limit);
    scored
}

pub fn relevant_skills_hint(
    prompt: &str,
    skills: &[SkillMetadata],
    loaded_skills: &[String],
) -> Option<String> {
    let ranked = select_skills_for_prompt(prompt, skills, loaded_skills, 3);
    crate::logger::operational_event(
        "skills.route",
        serde_json::json!({"considered": skills.len(), "shortlisted": ranked.iter().map(|(skill, score)| serde_json::json!({"name": skill.name, "score": score})).collect::<Vec<_>>() }),
    );
    if ranked.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Relevant skills\nThe prompt matches these skills by metadata or description intent. Consider `list_skills`, then `use_skill` for the best match before exploring:",
    );
    for (skill, score) in ranked {
        out.push_str(&format!(
            "\n- {} (score {score}): {}",
            skill.name,
            &skill.description[..skill
                .description
                .floor_char_boundary(320.min(skill.description.len()))]
        ));
    }
    Some(out)
}

/// Return the names successfully loaded after the latest explicit user prompt.
/// Keeping this boundary local avoids suppressing routing for a new request
/// merely because an earlier request used the same skill.
pub(crate) fn loaded_skills_since_latest_user(history: &[crate::app::ChatMessage]) -> Vec<String> {
    let Some(latest_user) = history.iter().rposition(|message| message.role == "user") else {
        return Vec::new();
    };

    history[latest_user + 1..]
        .iter()
        .filter_map(|message| {
            let result = message.tool_result.as_ref()?;
            if !result.tool_name.eq_ignore_ascii_case("use_skill")
                || !result.success
                || result.pending
            {
                return None;
            }
            let marker = "<skill_content name=\"";
            let start = message.content.find(marker)? + marker.len();
            let end = message.content[start..].find('\"')? + start;
            Some(message.content[start..end].to_string())
        })
        .collect()
}

fn split_skill_dirs(value: &OsStr) -> Vec<PathBuf> {
    std::env::split_paths(value)
        .filter(|p| !p.as_os_str().is_empty())
        .collect()
}

fn scan_skill_dir(dir: &Path, skills: &mut Vec<SkillMetadata>) {
    if !dir.is_dir() {
        return;
    }

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.exists()
                && let Ok(frontmatter) = read_frontmatter(&skill_md)
            {
                let parsed = parse_frontmatter(&frontmatter);
                skills.push(SkillMetadata {
                    name: parsed.name,
                    description: parsed.description,
                    path: path.clone(),
                    triggers: parsed.triggers,
                    keywords: parsed.keywords,
                    priority: parsed.priority,
                });
            }
        }
    }
    // `read_dir` order is filesystem-defined; sort so discovery is stable
    // across runs and a single root resolves duplicate names predictably.
    skills.sort_by(|a, b| a.path.cmp(&b.path));
}

fn read_frontmatter(path: &Path) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut frontmatter = String::new();
    let mut line = String::new();
    let mut found_opening = false;

    while reader.read_line(&mut line)? > 0 {
        if !found_opening {
            if line.trim() != "---" {
                break;
            }
            found_opening = true;
        } else if line.trim() == "---" {
            frontmatter.push_str(&line);
            break;
        }
        frontmatter.push_str(&line);
        line.clear();
    }

    Ok(frontmatter)
}

pub(crate) struct ParsedFrontmatter {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub keywords: Vec<String>,
    pub priority: i32,
}

fn parse_list_value(raw: &str) -> Vec<String> {
    let mut text = raw.trim().to_string();
    text = text.trim_matches(['\'', '"']).to_string();
    if text.starts_with('[') && text.ends_with(']') {
        text = text[1..text.len() - 1].to_string();
    }
    text.split(',')
        .map(|part| {
            part.trim()
                .trim_matches(['\'', '"', '[', ']'])
                .to_ascii_lowercase()
        })
        .filter(|part| !part.is_empty())
        .collect()
}

fn parse_frontmatter(content: &str) -> ParsedFrontmatter {
    let fallback = || ParsedFrontmatter {
        name: "unnamed".to_string(),
        description: "No description available".to_string(),
        triggers: Vec::new(),
        keywords: Vec::new(),
        priority: 0,
    };
    if !content.starts_with("---") {
        return fallback();
    }

    let end = content[3..].find("---");
    let Some(end_pos) = end else {
        return fallback();
    };
    let frontmatter = &content[3..3 + end_pos];
    let mut name = String::new();
    let mut description = String::new();
    let mut triggers = Vec::new();
    let mut keywords = Vec::new();
    let mut priority: i32 = 0;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            name = rest.trim().trim_matches(['\'', '"']).to_string();
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = rest.trim().trim_matches(['\'', '"']).to_string();
        } else if let Some(rest) = line.strip_prefix("triggers:") {
            triggers = parse_list_value(rest);
        } else if let Some(rest) = line.strip_prefix("keywords:") {
            keywords = parse_list_value(rest);
        } else if let Some(rest) = line.strip_prefix("priority:") {
            priority = rest
                .trim()
                .trim_matches(['\'', '"'])
                .parse::<i32>()
                .unwrap_or(0)
                .clamp(-100, 100);
        }
    }

    if name.is_empty() {
        name = "unnamed".to_string();
    }
    if description.is_empty() {
        description = "No description available".to_string();
    }

    ParsedFrontmatter {
        name,
        description,
        triggers,
        keywords,
        priority,
    }
}

const MAX_SKILL_CONTENT_BYTES: usize = 12_000;
const SKILL_CONTENT_TRUNCATED_NOTICE: &str = "\n\n[skill content truncated to 12k chars]";

fn truncate_skill_content(content: &mut String) {
    if content.len() <= MAX_SKILL_CONTENT_BYTES {
        return;
    }

    let mut end = MAX_SKILL_CONTENT_BYTES;
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    content.truncate(end);
    content.push_str(SKILL_CONTENT_TRUNCATED_NOTICE);
}

pub fn get_skill_content(name: &str) -> Option<SkillInfo> {
    let meta = discover_skills().into_iter().find(|s| s.name == name)?;
    let skill_md = meta.path.join("SKILL.md");
    let mut content = fs::read_to_string(&skill_md).ok()?;
    truncate_skill_content(&mut content);
    Some(SkillInfo {
        name: meta.name,
        description: meta.description,
        path: meta.path,
        content,
    })
}

pub fn list_skill_files(skill_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(skill_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && let Some(fname) = path.file_name().and_then(|f| f.to_str())
            {
                files.push(fname.to_string());
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static FIXTURE_ID: AtomicU32 = AtomicU32::new(0);

    /// A throwaway directory tree for root-resolution tests. Removed on drop so
    /// a failing assertion cannot leak skills into later runs.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let id = FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "rustcode_skills_{name}_{}_{id}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Fixture { root }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        /// Write `<root>/<relative>/SKILL.md` with the given frontmatter name.
        fn skill(&self, relative: &str, name: &str, description: &str) -> PathBuf {
            let dir = self.path(relative);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {description}\n---\nBody"),
            )
            .unwrap();
            dir
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustcode_test_{}_{}", name, std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    fn test_metadata(name: &str, description: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: description.to_string(),
            path: PathBuf::from(format!("/skills/{name}")),
            triggers: Vec::new(),
            keywords: Vec::new(),
            priority: 0,
        }
    }

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = "---\nname: my-skill\ndescription: A test skill\n---\nSkill content here";
        let parsed = parse_frontmatter(content);
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "A test skill");
    }

    #[test]
    fn test_parse_frontmatter_missing_fields() {
        let content = "---\n---\nContent";
        let parsed = parse_frontmatter(content);
        assert_eq!(parsed.name, "unnamed");
        assert_eq!(parsed.description, "No description available");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just plain content";
        let parsed = parse_frontmatter(content);
        assert_eq!(parsed.name, "unnamed");
        assert_eq!(parsed.description, "No description available");
    }

    #[test]
    fn test_parse_frontmatter_triggers_keywords_priority() {
        let content = "---\nname: deploy\ndescription: Deploy workflow\ntriggers: [deploy, ship it]\nkeywords: docker, k8s\npriority: 5\n---\nBody";
        let parsed = parse_frontmatter(content);
        assert_eq!(parsed.triggers, vec!["deploy", "ship it"]);
        assert_eq!(parsed.keywords, vec!["docker", "k8s"]);
        assert_eq!(parsed.priority, 5);
    }

    #[test]
    fn test_split_skill_dirs_uses_platform_path_separator() {
        let paths = [PathBuf::from("first"), PathBuf::from("second")];
        let joined = std::env::join_paths(&paths).unwrap();

        assert_eq!(split_skill_dirs(&joined), paths);
    }

    #[test]
    fn test_truncate_skill_content_preserves_utf8_boundary() {
        let mut content = "a".repeat(MAX_SKILL_CONTENT_BYTES - 1);
        content.push('é');
        content.push('z');

        truncate_skill_content(&mut content);

        assert_eq!(
            content,
            format!(
                "{}{}",
                "a".repeat(MAX_SKILL_CONTENT_BYTES - 1),
                SKILL_CONTENT_TRUNCATED_NOTICE
            )
        );
    }

    #[test]
    fn test_discover_skills_scans_dir() {
        let base = temp_dir("discover");
        let skill_dir = base.join("test-skill");
        let _ = fs::create_dir_all(&skill_dir);
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: Test skill\n---\nContent",
        )
        .unwrap();

        // Manually scan to test
        let mut skills = Vec::new();
        scan_skill_dir(&base, &mut skills);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
        assert_eq!(skills[0].description, "Test skill");
    }

    #[test]
    fn test_discovery_reads_frontmatter_without_loading_skill_body() {
        let base = temp_dir("frontmatter_only");
        let skill_dir = base.join("large-skill");
        let _ = fs::create_dir_all(&skill_dir);
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: large-skill\ndescription: Metadata only\n---\n{}",
                "body\n".repeat(100_000)
            ),
        )
        .unwrap();

        let mut skills = Vec::new();
        scan_skill_dir(&base, &mut skills);
        assert_eq!(skills[0].name, "large-skill");
        assert_eq!(skills[0].description, "Metadata only");
        assert!(read_frontmatter(&skill_dir.join("SKILL.md")).unwrap().len() < 1000);
    }

    #[test]
    fn test_list_skill_files() {
        let base = temp_dir("list_files");
        let _ = fs::create_dir_all(&base);
        fs::write(base.join("SKILL.md"), "content").unwrap();
        fs::write(base.join("helper.sh"), "#!/bin/bash").unwrap();

        let files = list_skill_files(&base);
        assert!(files.contains(&"SKILL.md".to_string()));
        assert!(files.contains(&"helper.sh".to_string()));
    }

    #[test]
    fn skill_roots_are_ordered_from_explicit_overrides_to_the_owned_root() {
        let fixture = Fixture::new("root_order");
        let inputs = SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: Some(fixture.path("config")),
            home: Some(fixture.path("home")),
            extra_dirs: vec![fixture.path("configured")],
            extra_dirs_env: None,
        };

        let roots = skill_roots_from(&inputs);

        assert_eq!(
            roots.iter().map(|r| r.path.clone()).collect::<Vec<_>>(),
            vec![
                fixture.path("configured"),
                fixture.path("workspace/.rustcode/skills"),
                fixture.path("workspace/.agents/skills"),
                fixture.path("home/.agents/skills"),
                fixture.path("config/skills"),
            ]
        );
        assert_eq!(roots[0].kind, SkillRootKind::Configured);
        assert_eq!(roots[1].kind, SkillRootKind::Workspace);
        assert_eq!(roots[3].kind, SkillRootKind::Universal);
        assert_eq!(roots[4].kind, SkillRootKind::Rustcode);
    }

    #[test]
    fn skill_roots_read_extra_dirs_from_the_environment_variable() {
        let fixture = Fixture::new("root_env");
        let first = fixture.path("first");
        let second = fixture.path("second");
        let inputs = SkillRootInputs {
            workspace: None,
            config_dir: None,
            home: None,
            extra_dirs: vec![first.clone()],
            extra_dirs_env: Some(std::env::join_paths([first.clone(), second.clone()]).unwrap()),
        };

        let roots = skill_roots_from(&inputs);

        // Duplicates collapse and the env var extends, it does not replace.
        assert_eq!(
            roots,
            vec![
                SkillRoot {
                    path: first,
                    kind: SkillRootKind::Configured
                },
                SkillRoot {
                    path: second,
                    kind: SkillRootKind::Configured
                }
            ]
        );
    }

    #[test]
    fn skill_roots_resolve_project_and_configured_roots_without_home() {
        let fixture = Fixture::new("root_no_home");
        let inputs = SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: Some(fixture.path("config")),
            home: None,
            extra_dirs: vec![fixture.path("configured")],
            extra_dirs_env: None,
        };

        let roots = skill_roots_from(&inputs);
        let paths: Vec<PathBuf> = roots.iter().map(|r| r.path.clone()).collect();

        assert!(paths.contains(&fixture.path("workspace/.rustcode/skills")));
        assert!(paths.contains(&fixture.path("workspace/.agents/skills")));
        assert!(paths.contains(&fixture.path("config/skills")));
        assert!(!roots.contains(&SkillRoot {
            path: fixture.path("home/.agents/skills"),
            kind: SkillRootKind::Universal,
        }));
    }

    #[test]
    fn discover_skills_scans_every_root_including_the_universal_one() {
        let fixture = Fixture::new("discover_all");
        fixture.skill(
            "workspace/.rustcode/skills/from-project",
            "project",
            "Project root",
        );
        fixture.skill(
            "workspace/.agents/skills/from-agents",
            "agents",
            "Project agents root",
        );
        fixture.skill(
            "home/.agents/skills/from-universal",
            "universal",
            "Shared root",
        );
        fixture.skill("config/skills/from-config", "owned", "RustCode root");
        fixture.skill("configured/from-extra", "extra", "Configured root");

        let skills = discover_skills_in(SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: Some(fixture.path("config")),
            home: Some(fixture.path("home")),
            extra_dirs: vec![fixture.path("configured")],
            extra_dirs_env: None,
        });

        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["agents", "extra", "owned", "project", "universal"]);
    }

    #[test]
    fn discover_skills_prefers_the_project_definition_over_the_user_one() {
        let fixture = Fixture::new("discover_precedence");
        let project_dir = fixture.skill("workspace/.rustcode/skills/shared", "shared", "Project");
        fixture.skill("home/.agents/skills/shared", "shared", "Universal");
        fixture.skill("config/skills/shared", "shared", "RustCode");

        let skills = discover_skills_in(SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: Some(fixture.path("config")),
            home: Some(fixture.path("home")),
            extra_dirs: vec![],
            extra_dirs_env: None,
        });

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].path, project_dir);
        assert_eq!(skills[0].description, "Project");
    }

    #[test]
    fn discover_skills_lets_an_explicit_extra_dir_override_the_project_root() {
        let fixture = Fixture::new("discover_extra_override");
        let extra_dir = fixture.skill("configured/shared", "shared", "Configured");

        let skills = discover_skills_in(SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: Some(fixture.path("config")),
            home: None,
            extra_dirs: vec![fixture.path("configured")],
            extra_dirs_env: None,
        });
        assert_eq!(skills[0].path, extra_dir);

        // The same root added through the environment variable also wins.
        let via_env = discover_skills_in(SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: None,
            home: None,
            extra_dirs: vec![],
            extra_dirs_env: Some(std::env::join_paths([fixture.path("configured")]).unwrap()),
        });
        assert_eq!(via_env[0].path, extra_dir);
    }

    #[test]
    fn discover_skills_still_finds_project_skills_without_home_or_config() {
        let fixture = Fixture::new("discover_minimal");
        let project_dir = fixture.skill("workspace/.rustcode/skills/local", "local", "Local only");

        let skills = discover_skills_in(SkillRootInputs {
            workspace: Some(fixture.path("workspace")),
            config_dir: None,
            home: None,
            extra_dirs: vec![],
            extra_dirs_env: None,
        });

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].path, project_dir);
    }

    #[test]
    fn only_the_rustcode_owned_root_is_created_by_fix() {
        assert!(SkillRootKind::Rustcode.is_rustcode_owned());
        assert!(!SkillRootKind::Universal.is_rustcode_owned());
        assert!(!SkillRootKind::Workspace.is_rustcode_owned());
        assert!(!SkillRootKind::Configured.is_rustcode_owned());
    }

    #[test]
    fn format_skill_roots_lists_every_root_and_marks_missing_ones() {
        let fixture = Fixture::new("format_roots");
        fs::create_dir_all(fixture.path("present")).unwrap();
        let roots = vec![
            SkillRoot {
                path: fixture.path("present"),
                kind: SkillRootKind::Universal,
            },
            SkillRoot {
                path: fixture.path("absent"),
                kind: SkillRootKind::Rustcode,
            },
        ];

        let rendered = format_skill_roots(&roots);

        assert!(rendered.contains("1. "));
        assert!(rendered.contains(&fixture.path("present").display().to_string()));
        assert!(!rendered.contains(&format!("{}(missing)", fixture.path("present").display())));
        assert!(rendered.contains("(missing)"));
    }

    #[test]
    fn test_get_skill_content_by_name() {
        let base = temp_dir("get_skill");
        let skill_dir = base.join("my-skill");
        let _ = fs::create_dir_all(&skill_dir);
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nBody",
        )
        .unwrap();

        // Test parse directly since discover_skills scans fixed paths
        let content = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
        let parsed = parse_frontmatter(&content);
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "My skill");
    }

    #[test]
    fn skill_routing_hint_matches_an_explicit_available_skill_name() {
        let skills = [test_metadata("solidtime", "Solidtime workflow")];

        let hint = skill_routing_hint("Please check Solidtime for this week.", &skills, &[])
            .expect("explicitly named skill should route");

        assert!(hint.contains("use_skill"));
        assert!(hint.contains("solidtime"));
    }

    #[test]
    fn skill_routing_hint_ignores_unrelated_prompts() {
        let skills = [test_metadata("solidtime", "Solidtime workflow")];

        assert!(skill_routing_hint("Please inspect the time module.", &skills, &[]).is_none());
        assert!(skill_routing_hint("Please inspect solidtimes.", &skills, &[]).is_none());
        assert!(
            skill_routing_hint("Please inspect solidtime-like behavior.", &skills, &[]).is_none()
        );
    }

    #[test]
    fn skill_routing_hint_does_not_guess_from_a_name_component() {
        let skills = [test_metadata("release-automation", "Release workflow")];

        assert!(skill_routing_hint("Clean this up and release it.", &skills, &[]).is_none());
    }

    #[test]
    fn skill_routing_hint_does_not_route_email_to_cloudflare_email_service() {
        let skills = [test_metadata(
            "cloudflare-email-service",
            "Cloudflare email workflow",
        )];

        assert!(
            skill_routing_hint("Build a Bun API that stores email addresses.", &skills, &[])
                .is_none()
        );
        assert!(
            skill_routing_hint("Use cloudflare-email-service for delivery.", &skills, &[],)
                .is_some()
        );
    }

    #[test]
    fn successful_load_suppresses_same_turn_routing_hint() {
        let skills = [test_metadata("solidtime", "Solidtime workflow")];

        assert!(
            skill_routing_hint(
                "Please check Solidtime for this week.",
                &skills,
                &["solidtime".to_string()],
            )
            .is_none()
        );
    }

    #[test]
    fn successful_load_only_suppresses_the_current_user_turn() {
        let loaded = |name: &str| {
            crate::app::ChatMessage::new(
                "tool",
                format!("use_skill: <skill_content name=\"{name}\">\ninstructions"),
            )
            .with_tool_result(crate::app::ToolResultRecord {
                workspace_generation: None,
                workspace_epoch: None,
                evidence_hash: None,
                tool_name: "use_skill".to_string(),
                success: true,
                ..Default::default()
            })
        };
        let history = vec![
            crate::app::ChatMessage::new("user", "old request"),
            loaded("solidtime"),
            crate::app::ChatMessage::new("user", "new request: use solidtime"),
        ];
        let skills = [test_metadata("solidtime", "Solidtime workflow")];

        let loaded_skills = loaded_skills_since_latest_user(&history);
        assert!(loaded_skills.is_empty());
        assert!(skill_routing_hint("use solidtime", &skills, &loaded_skills).is_some());
    }

    #[test]
    fn failed_load_does_not_suppress_routing_hint() {
        let history = vec![
            crate::app::ChatMessage::new("user", "use solidtime"),
            crate::app::ChatMessage::new("tool", "use_skill: Skill not found").with_tool_result(
                crate::app::ToolResultRecord {
                    workspace_generation: None,
                    workspace_epoch: None,
                    evidence_hash: None,
                    tool_name: "use_skill".to_string(),
                    success: false,
                    ..Default::default()
                },
            ),
        ];
        assert!(loaded_skills_since_latest_user(&history).is_empty());
    }

    fn trigger_metadata(
        name: &str,
        triggers: &[&str],
        keywords: &[&str],
        priority: i32,
    ) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: format!("{name} workflow"),
            path: PathBuf::from(format!("/skills/{name}")),
            triggers: triggers.iter().map(|s| s.to_string()).collect(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            priority,
        }
    }

    #[test]
    fn select_prefers_trigger_over_keyword_and_priority_breaks_ties() {
        let skills = vec![
            trigger_metadata("shipper", &["deploy"], &[], 0),
            trigger_metadata("docker-helper", &[], &["docker"], 0),
            trigger_metadata("shipper-prio", &["deploy"], &[], 5),
        ];
        let ranked = select_skills_for_prompt("please deploy with docker", &skills, &[], 3);
        assert_eq!(ranked[0].0.name, "shipper-prio");
        assert_eq!(ranked[1].0.name, "shipper");
        assert_eq!(ranked[2].0.name, "docker-helper");
    }

    #[test]
    fn select_excludes_loaded_and_explicit_name_mentions() {
        let skills = vec![trigger_metadata("deploy", &["deploy"], &[], 0)];
        assert!(
            select_skills_for_prompt("deploy now", &skills, &["deploy".to_string()], 3).is_empty()
        );
        // Explicit name mentions stay on the routing-hint path, not relevance.
        assert!(select_skills_for_prompt("use deploy now", &skills, &[], 3).is_empty());
        assert!(relevant_skills_hint("unrelated prompt", &skills, &[]).is_none());
    }
    #[test]
    fn description_routes_natural_music_state_without_explicit_metadata() {
        let mut spotify = trigger_metadata("spotify", &[], &[], 0);
        spotify.description =
            "Control Spotify playback, play music/artists, manage playlists, volume, or devices"
                .into();
        let skills = vec![spotify];
        assert_eq!(
            select_skills_for_prompt("what song am I listening to", &skills, &[], 3).len(),
            1
        );
        assert!(
            select_skills_for_prompt("write a song parser in Rust", &skills, &[], 3).is_empty()
        );
        assert!(select_skills_for_prompt("what is the time", &skills, &[], 3).is_empty());
        assert!(select_skills_for_prompt("play", &skills, &[], 3).is_empty());
    }
    #[test]
    fn description_routing_has_bounded_catalog_overhead() {
        let skills = (0..1000)
            .map(|i| {
                let mut skill = trigger_metadata(&format!("music-{i}"), &[], &[], 0);
                skill.description = "Control music playback".into();
                skill
            })
            .collect::<Vec<_>>();
        let hint = relevant_skills_hint("what song am I listening to", &skills, &[]).unwrap();
        assert_eq!(
            hint.lines().filter(|line| line.starts_with("- ")).count(),
            3
        );
        assert!(hint.len() < 2000);
    }
}
