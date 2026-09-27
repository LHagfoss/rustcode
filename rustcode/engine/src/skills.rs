use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct SkillInfo {
    pub name: String,
    #[allow(dead_code)]
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
    let mut skills = Vec::new();

    // rustcode's own skill locations. We deliberately do NOT scan `.claude/skills`
    // anymore: that is Claude Code's directory, and inheriting it dumped unrelated
    // plugin skills (Cloudflare Workers, etc.) into every prompt — which derailed
    // agents into believing this project was something it isn't. Users who really
    // want to share those can opt in via RUSTCODE_EXTRA_SKILL_DIRS (using the
    // platform's native path-list separator).
    let local_dirs = [".rustcode/skills", ".agents/skills"];

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return skills,
    };

    let global_dirs = [
        home.join(".config/rustcode/skills"),
        home.join(".agents/skills"),
    ];

    let extra_dirs = std::env::var_os("RUSTCODE_EXTRA_SKILL_DIRS")
        .as_deref()
        .map(split_skill_dirs)
        .unwrap_or_default();

    for dir in local_dirs
        .iter()
        .map(PathBuf::from)
        .chain(global_dirs)
        .chain(extra_dirs)
    {
        scan_skill_dir(&dir, &mut skills);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills.dedup_by(|a, b| a.name == b.name);
    skills
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
    if ranked.is_empty() {
        return None;
    }
    let mut out = String::from(
        "# Relevant skills\nThe prompt matches these skills by trigger/keyword. Consider `list_skills`, then `use_skill` for the best match before exploring:",
    );
    for (skill, score) in ranked {
        out.push_str(&format!(
            "\n- {} (score {score}): {}",
            skill.name, skill.description
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
}
