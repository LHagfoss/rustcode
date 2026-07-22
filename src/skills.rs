use std::fs;
use std::path::{Path, PathBuf};

pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub content: String,
}

pub fn discover_skills() -> Vec<SkillInfo> {
    let mut skills = Vec::new();

    let local_dirs = [
        ".rustcode/skills",
        ".agents/skills",
        ".claude/skills",
    ];

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return skills,
    };

    let global_dirs = [
        home.join(".config/rustcode/skills"),
        home.join(".claude/skills"),
        home.join(".agents/skills"),
    ];

    for dir in local_dirs.iter().map(PathBuf::from).chain(global_dirs.into_iter()) {
        scan_skill_dir(&dir, &mut skills);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn scan_skill_dir(dir: &Path, skills: &mut Vec<SkillInfo>) {
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
            if skill_md.exists() {
                if let Ok(content) = fs::read_to_string(&skill_md) {
                    let (name, description) = parse_frontmatter(&content);
                    skills.push(SkillInfo {
                        name,
                        description,
                        path: path.clone(),
                        content,
                    });
                }
            }
        }
    }
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
            if line.starts_with("name:") {
                name = line[5..].trim().to_string();
            } else if line.starts_with("description:") {
                description = line[12..].trim().to_string();
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

pub fn get_skill_content(name: &str) -> Option<SkillInfo> {
    discover_skills().into_iter().find(|s| s.name == name)
}

pub fn list_skill_files(skill_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(skill_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(fname) = path.file_name().and_then(|f| f.to_str()) {
                    files.push(fname.to_string());
                }
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
        let dir = std::env::temp_dir().join(format!("rustcode_test_{}_{}", name, std::process::id()));
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
}
