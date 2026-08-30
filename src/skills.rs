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

pub fn skill_routing_hint(prompt: &str, skills: &[SkillMetadata]) -> Option<String> {
    let skill = skills
        .iter()
        .find(|skill| prompt_mentions_skill(prompt, &skill.name))?;
    Some(format!(
        "# Priority skill route\nThe latest user prompt explicitly names available skill `{}`. Call `use_skill` first with the exact name `{}` before any filesystem, web, or exploration tool.",
        skill.name, skill.name
    ))
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
                let (name, description) = parse_frontmatter(&frontmatter);
                skills.push(SkillMetadata {
                    name,
                    description,
                    path: path.clone(),
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

fn parse_frontmatter(content: &str) -> (String, String) {
    if !content.starts_with("---") {
        return (
            "unnamed".to_string(),
            "No description available".to_string(),
        );
    }

    let end = content[3..].find("---");
    if let Some(end_pos) = end {
        let frontmatter = &content[3..3 + end_pos];
        let mut name = String::new();
        let mut description = String::new();

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("name:") {
                name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("description:") {
                description = rest.trim().to_string();
            }
        }

        if name.is_empty() {
            name = "unnamed".to_string();
        }
        if description.is_empty() {
            description = "No description available".to_string();
        }

        return (name, description);
    }

    (
        "unnamed".to_string(),
        "No description available".to_string(),
    )
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

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = "---\nname: my-skill\ndescription: A test skill\n---\nSkill content here";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "A test skill");
    }

    #[test]
    fn test_parse_frontmatter_missing_fields() {
        let content = "---\n---\nContent";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "unnamed");
        assert_eq!(desc, "No description available");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just plain content";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, "unnamed");
        assert_eq!(desc, "No description available");
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
        let (name, desc) = parse_frontmatter(&content);
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "My skill");
    }

    #[test]
    fn skill_routing_hint_matches_an_explicit_available_skill_name() {
        let skills = [SkillMetadata {
            name: "solidtime".to_string(),
            description: "Solidtime workflow".to_string(),
            path: PathBuf::from("/skills/solidtime"),
        }];

        let hint = skill_routing_hint("Please check Solidtime for this week.", &skills)
            .expect("explicitly named skill should route");

        assert!(hint.contains("use_skill"));
        assert!(hint.contains("solidtime"));
    }

    #[test]
    fn skill_routing_hint_ignores_unrelated_prompts() {
        let skills = [SkillMetadata {
            name: "solidtime".to_string(),
            description: "Solidtime workflow".to_string(),
            path: PathBuf::from("/skills/solidtime"),
        }];

        assert!(skill_routing_hint("Please inspect the time module.", &skills).is_none());
        assert!(skill_routing_hint("Please inspect solidtimes.", &skills).is_none());
        assert!(skill_routing_hint("Please inspect solidtime-like behavior.", &skills).is_none());
    }

    #[test]
    fn skill_routing_hint_does_not_guess_from_a_name_component() {
        let skills = [SkillMetadata {
            name: "release-automation".to_string(),
            description: "Release workflow".to_string(),
            path: PathBuf::from("/skills/release-automation"),
        }];

        assert!(skill_routing_hint("Clean this up and release it.", &skills).is_none());
    }

    #[test]
    fn skill_routing_hint_does_not_route_email_to_cloudflare_email_service() {
        let skills = [SkillMetadata {
            name: "cloudflare-email-service".to_string(),
            description: "Cloudflare email workflow".to_string(),
            path: PathBuf::from("/skills/cloudflare-email-service"),
        }];

        assert!(
            skill_routing_hint("Build a Bun API that stores email addresses.", &skills).is_none()
        );
        assert!(
            skill_routing_hint("Use cloudflare-email-service for delivery.", &skills).is_some()
        );
    }
}
