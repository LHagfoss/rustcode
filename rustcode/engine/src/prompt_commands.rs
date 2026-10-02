use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::PathBuf;

const MAX_TEMPLATE_BYTES: u64 = 64 * 1024;
const MAX_EXPANDED_BYTES: usize = 256 * 1024;

/// Search roots for reusable prompt templates. Workspace templates take
/// precedence over user templates with the same name.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    pub workspace: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCommand {
    pub name: String,
    pub path: PathBuf,
}

/// List direct-child Markdown templates without creating either directory.
pub fn list(roots: &Roots) -> Result<Vec<PromptCommand>, String> {
    let mut names = BTreeMap::new();
    for directory in command_dirs(roots) {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not list prompt commands in {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "Could not read a prompt command entry in {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            if !valid_name(name) {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| {
                format!(
                    "Could not inspect prompt command {}: {error}",
                    path.display()
                )
            })?;
            if file_type.is_file() {
                names.entry(name.to_owned()).or_insert(path);
            }
        }
    }
    Ok(names
        .into_iter()
        .map(|(name, path)| PromptCommand { name, path })
        .collect())
}

/// Read and expand a template on each invocation, so edits are visible
/// immediately and contents are not cached.
pub fn load(roots: &Roots, name: &str, arguments: Option<&str>) -> Result<String, String> {
    if !valid_name(name) {
        return Err(format!(
            "Invalid prompt command name {name:?}. Use lowercase ASCII letters, numbers, `-`, or `_`."
        ));
    }

    let path = find_template(roots, name)?.ok_or_else(|| {
        let roots = command_dirs(roots)
            .into_iter()
            .map(|directory| format!("`{}`", directory.display()))
            .collect::<Vec<_>>()
            .join(" and ");
        format!("Prompt command `{name}` was not found. Add `{name}.md` to {roots}.")
    })?;
    let file_type = fs::symlink_metadata(&path)
        .map_err(|error| format!("Could not inspect prompt command `{name}`: {error}"))?
        .file_type();
    if !file_type.is_file() {
        return Err(format!(
            "Prompt command `{name}` must be a regular Markdown file: {}",
            path.display()
        ));
    }

    let file = File::open(&path)
        .map_err(|error| format!("Could not read prompt command `{name}`: {error}"))?;
    let mut bytes = Vec::new();
    read_bounded(file, &mut bytes)
        .map_err(|error| format!("Could not read prompt command `{name}`: {error}"))?;
    if bytes.len() as u64 > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "Prompt command `{name}` exceeds the 64 KiB size limit."
        ));
    }
    let template = String::from_utf8(bytes)
        .map_err(|_| format!("Prompt command `{name}` is not valid UTF-8."))?;
    if template.trim().is_empty() {
        return Err(format!("Prompt command `{name}` is empty."));
    }
    let expanded = match arguments {
        Some(arguments) if template.contains("$ARGUMENTS") => {
            let occurrences = template.matches("$ARGUMENTS").count();
            let expanded_len = template
                .len()
                .checked_sub(occurrences * "$ARGUMENTS".len())
                .and_then(|length| length.checked_add(occurrences.checked_mul(arguments.len())?))
                .ok_or_else(|| format!("Expanded prompt command `{name}` is too large."))?;
            if expanded_len > MAX_EXPANDED_BYTES {
                return Err(format!(
                    "Expanded prompt command `{name}` exceeds the 256 KiB size limit."
                ));
            }
            template.replace("$ARGUMENTS", arguments)
        }
        None if template.contains("$ARGUMENTS") => template.replace("$ARGUMENTS", ""),
        Some(arguments) => {
            let expanded_len = template
                .len()
                .checked_add(2)
                .and_then(|length| length.checked_add(arguments.len()))
                .ok_or_else(|| format!("Expanded prompt command `{name}` is too large."))?;
            if expanded_len > MAX_EXPANDED_BYTES {
                return Err(format!(
                    "Expanded prompt command `{name}` exceeds the 256 KiB size limit."
                ));
            }
            format!("{template}\n\n{arguments}")
        }
        None => template,
    };
    if expanded.trim().is_empty() {
        return Err(format!("Expanded prompt command `{name}` is empty."));
    }
    if expanded.trim_start().starts_with('/') {
        return Err(format!(
            "Expanded prompt command `{name}` starts with `/` and could run as a slash command. Edit the template or arguments so it starts with prompt text."
        ));
    }
    Ok(expanded)
}

fn read_bounded(file: File, bytes: &mut Vec<u8>) -> std::io::Result<()> {
    let mut file: Take<File> = file.take(MAX_TEMPLATE_BYTES + 1);
    file.read_to_end(bytes)?;
    Ok(())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn command_dirs(roots: &Roots) -> Vec<PathBuf> {
    let mut directories = Vec::with_capacity(2);
    if let Some(workspace) = &roots.workspace {
        directories.push(workspace.join(".rustcode/commands"));
    }
    if let Some(config_dir) = &roots.config_dir {
        directories.push(config_dir.join("commands"));
    }
    directories
}

fn find_template(roots: &Roots, name: &str) -> Result<Option<PathBuf>, String> {
    for directory in command_dirs(roots) {
        let path = directory.join(format!("{name}.md"));
        match fs::symlink_metadata(&path) {
            Ok(_) => return Ok(Some(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Could not inspect prompt command `{name}` in {}: {error}",
                    directory.display()
                ));
            }
        }
    }
    Ok(None)
}

/// Parse `/prompt <name> [arguments]` while preserving argument bytes after
/// the single separator that follows the name.
pub fn parse_request(input: &str) -> Option<(&str, Option<&str>)> {
    let input = input.trim_start();
    let after_command = input.strip_prefix("/prompt")?;
    let separator = after_command.chars().next()?;
    if !separator.is_whitespace() {
        return None;
    }
    let after_separator = after_command[separator.len_utf8()..].trim_start();
    let name_end = after_separator
        .find(char::is_whitespace)
        .unwrap_or(after_separator.len());
    let name = &after_separator[..name_end];
    if name_end == after_separator.len() {
        return Some((name, None));
    }
    let remainder = &after_separator[name_end..];
    let argument_separator_len = remainder.chars().next().unwrap().len_utf8();
    Some((name, Some(&remainder[argument_separator_len..])))
}

#[cfg(test)]
mod tests {
    use super::{Roots, list, load, parse_request};
    use std::fs;

    fn roots(workspace: &std::path::Path, config: &std::path::Path) -> Roots {
        Roots {
            workspace: Some(workspace.to_path_buf()),
            config_dir: Some(config.to_path_buf()),
        }
    }

    #[test]
    fn workspace_templates_override_user_templates_and_list_in_name_order() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let project_commands = workspace.path().join(".rustcode/commands");
        let user_commands = config.path().join("commands");
        fs::create_dir_all(&project_commands).unwrap();
        fs::create_dir_all(&user_commands).unwrap();
        fs::write(user_commands.join("review.md"), "global review").unwrap();
        fs::write(user_commands.join("zeta.md"), "zeta").unwrap();
        fs::write(project_commands.join("review.md"), "project review").unwrap();
        fs::write(project_commands.join("alpha.md"), "alpha").unwrap();

        let roots = roots(workspace.path(), config.path());
        let listed = list(&roots).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "review", "zeta"]
        );
        assert_eq!(listed[1].path, project_commands.join("review.md"));
        assert_eq!(load(&roots, "review", None).unwrap(), "project review");
    }

    #[test]
    fn listing_does_not_create_missing_command_directories() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let roots = roots(workspace.path(), config.path());

        assert!(list(&roots).unwrap().is_empty());
        assert!(!workspace.path().join(".rustcode/commands").exists());
        assert!(!config.path().join("commands").exists());
    }

    #[test]
    fn command_names_reject_traversal_and_non_lowercase_ascii_names() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let roots = roots(workspace.path(), config.path());

        for name in ["../secret", "nested/review", "Upper", "é", "", "."] {
            assert!(load(&roots, name, None).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn load_reads_current_file_contents_instead_of_caching_templates() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        let path = command_dir.join("review.md");
        fs::write(&path, "first").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert_eq!(load(&roots, "review", None).unwrap(), "first");
        fs::write(&path, "edited").unwrap();
        assert_eq!(load(&roots, "review", None).unwrap(), "edited");
    }

    #[test]
    fn arguments_replace_the_placeholder_without_normalizing_whitespace_or_unicode() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("review.md"), "Review:\n$ARGUMENTS\nDone").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert_eq!(
            load(&roots, "review", Some("  café\nline two  ")).unwrap(),
            "Review:\n  café\nline two  \nDone"
        );
    }

    #[test]
    fn arguments_without_a_placeholder_are_appended_after_a_blank_line() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("review.md"), "Review this change.").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert_eq!(
            load(&roots, "review", Some("  diff.md\n  ")).unwrap(),
            "Review this change.\n\n  diff.md\n  "
        );
    }

    #[test]
    fn missing_arguments_remove_the_placeholder_instead_of_leaving_template_syntax() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("review.md"), "Review $ARGUMENTS now.").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert_eq!(load(&roots, "review", None).unwrap(), "Review  now.");
    }

    #[test]
    fn repeated_placeholders_cannot_expand_past_the_prompt_size_limit() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("review.md"), "$ARGUMENTS $ARGUMENTS").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert!(load(&roots, "review", Some(&"x".repeat(140_000))).is_err());
    }

    #[test]
    fn slash_arguments_cannot_stage_a_builtin_command_as_the_expansion() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("review.md"), "$ARGUMENTS").unwrap();
        let roots = roots(workspace.path(), config.path());

        assert!(load(&roots, "review", Some("/clear")).is_err());
    }

    #[test]
    fn request_parser_accepts_extra_separator_whitespace_and_preserves_arguments() {
        assert_eq!(
            parse_request("/prompt   review  café\nnext  "),
            Some(("review", Some(" café\nnext  ")))
        );
        assert_eq!(parse_request("/prompt review"), Some(("review", None)));
        assert_eq!(parse_request("/promptly review"), None);
    }

    #[test]
    fn empty_invalid_utf8_oversized_and_slash_templates_return_errors() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let command_dir = workspace.path().join(".rustcode/commands");
        fs::create_dir_all(&command_dir).unwrap();
        fs::write(command_dir.join("empty.md"), " \n\t").unwrap();
        fs::write(command_dir.join("invalid.md"), [0xff, 0xfe]).unwrap();
        fs::write(command_dir.join("large.md"), vec![b'x'; 65_537]).unwrap();
        fs::write(command_dir.join("slash.md"), "  /clear\nPlease inspect").unwrap();
        let roots = roots(workspace.path(), config.path());

        for name in ["empty", "invalid", "large", "slash"] {
            assert!(load(&roots, name, None).is_err(), "accepted {name}");
        }
    }

    #[test]
    fn broken_project_override_does_not_fall_back_to_a_user_template() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let project_commands = workspace.path().join(".rustcode/commands");
        let user_commands = config.path().join("commands");
        fs::create_dir_all(&project_commands).unwrap();
        fs::create_dir_all(&user_commands).unwrap();
        fs::write(user_commands.join("review.md"), "global review").unwrap();
        fs::write(project_commands.join("review.md"), [0xff]).unwrap();
        let roots = roots(workspace.path(), config.path());

        assert!(load(&roots, "review", None).is_err());
    }
}
