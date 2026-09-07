use serde_json::Value;

// Keep command classification separate from process execution: these helpers
// decide whether a command is safe to run without confirmation, but never
// execute or mutate anything themselves.

/// Short sudo options that consume a value (either attached or following).
const SUDO_SHORT_OPTS_WITH_VALUE: &str = "CghpRTtUu";
/// Long sudo options that consume a value unless written as `--opt=value`.
const SUDO_LONG_OPTS_WITH_VALUE: &[&str] = &[
    "close-from",
    "group",
    "host",
    "prompt",
    "chroot",
    "command-timeout",
    "type",
    "other-user",
    "user",
    "role",
];

/// Conservatively split shell text at boundaries that may introduce another
/// command. This is intentionally not a complete shell parser: splitting too
/// eagerly can only make the policy require confirmation, never bypass it.
fn split_command_segments(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for ch in cmd.chars() {
        match ch {
            ';' | '\n' | '|' | '&' | '`' | '(' | ')' | '{' | '}' => {
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    segments.push(current);
    segments
}

fn git_subcommand<'a>(tokens: &'a [&'a str]) -> Option<(&'a str, usize)> {
    let first = tokens.first()?.rsplit(['/', '\\']).next()?;
    if first != "git" {
        return None;
    }
    let mut index = 1;
    while index < tokens.len() {
        let token = tokens[index];
        if token == "--" {
            return tokens
                .get(index + 1)
                .copied()
                .map(|subcommand| (subcommand, index + 1));
        }
        if !token.starts_with('-') {
            return Some((token, index));
        }
        if matches!(
            token,
            "-C" | "-c"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--exec-path"
                | "--config"
                | "--super-prefix"
        ) && !token.contains('=')
        {
            index += 2;
        } else {
            index += 1;
        }
    }
    None
}

fn destructive_git_scope(segment: &str) -> Option<String> {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let (subcommand, subcommand_index) = git_subcommand(&tokens)?;
    let arguments = &tokens[subcommand_index + 1..];
    if arguments.iter().any(|token| {
        *token == "-f" || *token == "-ff" || *token == "-D" || token.starts_with("--force")
    }) {
        return Some(format!("git {subcommand} force operation"));
    }
    let scope = match subcommand {
        "restore" => "working-tree or index paths",
        "checkout" => "checked-out paths or branch state",
        "reset" => "HEAD, index, and possibly working-tree paths",
        "clean" => "untracked files and directories",
        "branch"
            if arguments
                .iter()
                .any(|arg| *arg == "-d" || *arg == "--delete") =>
        {
            "deleted local branch"
        }
        _ => return None,
    };
    Some(format!("git {subcommand}: {scope}"))
}

fn is_read_only_git(tokens: &[&str]) -> bool {
    let Some((subcommand, subcommand_index)) = git_subcommand(tokens) else {
        return false;
    };
    if destructive_git_scope(&tokens.join(" ")).is_some() {
        return false;
    }
    let arguments = &tokens[subcommand_index + 1..];
    if arguments.iter().any(|argument| {
        *argument == "-o"
            || *argument == "--output"
            || argument.starts_with("--output=")
            || *argument == "--ext-diff"
    }) {
        return false;
    }
    matches!(
        subcommand,
        "status" | "diff" | "log" | "show" | "rev-parse" | "describe"
    ) || (subcommand == "branch"
        && arguments.iter().all(|argument| {
            matches!(
                *argument,
                "-a" | "--all" | "-r" | "--remotes" | "-v" | "--verbose" | "--show-current"
            )
        }))
}

fn is_read_only_gh(tokens: &[&str]) -> bool {
    let Some(binary) = tokens.first() else {
        return true;
    };
    if binary.rsplit(['/', '\\']).next() != Some("gh") {
        return false;
    }
    matches!(
        tokens.get(1..).unwrap_or_default(),
        ["help", ..]
            | ["--help", ..]
            | ["-h", ..]
            | ["auth", "status", ..]
            | ["auth", "help", ..]
            | ["issue", "list", ..]
            | ["issue", "view", ..]
            | ["pr", "list", ..]
            | ["pr", "view", ..]
    )
}

fn is_read_only_segment(segment: &str) -> bool {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let Some(binary) = tokens.first().map(|token| token.rsplit(['/', '\\']).next()) else {
        return true;
    };
    match binary {
        Some("git") => is_read_only_git(&tokens),
        Some("gh") => is_read_only_gh(&tokens),
        Some("command") => tokens.get(1) == Some(&"-v"),
        Some("find") => !tokens[1..].iter().any(|argument| {
            matches!(
                *argument,
                "-delete"
                    | "-exec"
                    | "-execdir"
                    | "-ok"
                    | "-okdir"
                    | "-fprint"
                    | "-fprintf"
                    | "-fls"
            )
        }),
        Some(
            "cat" | "date" | "echo" | "false" | "grep" | "head" | "less" | "ls" | "more" | "printf"
            | "pwd" | "rg" | "stat" | "tail" | "test" | "true" | "type" | "uname" | "which",
        ) => true,
        Some("npm") => {
            matches!(tokens.get(1..).unwrap_or_default(), ["config", "get", key] if !key.starts_with('-'))
        }
        _ => false,
    }
}

fn split_inspection_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in command.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' && quote != Some('\'') {
            current.push(character);
            escaped = true;
        } else if let Some(delimiter) = quote {
            current.push(character);
            if character == delimiter {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            current.push(character);
            quote = Some(character);
        } else if matches!(character, ';' | '\n' | '|' | '&' | '(' | ')' | '{' | '}') {
            segments.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    segments.push(current);
    segments
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileInspectionKind {
    View,
    Search,
}

fn has_file_operand(arguments: &[&str], options_with_values: &[&str]) -> bool {
    let mut skip_next = false;
    for argument in arguments {
        if skip_next {
            skip_next = false;
            continue;
        }
        if options_with_values.contains(argument) {
            skip_next = true;
            continue;
        }
        if *argument == "--" {
            continue;
        }
        if !argument.starts_with('-') && *argument != "-" && !argument.contains('=') {
            return true;
        }
    }
    false
}

fn pure_file_inspection_kind(segment: &str) -> Option<FileInspectionKind> {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let binary = tokens.first()?.rsplit(['/', '\\']).next()?;
    let arguments = &tokens[1..];
    match binary {
        "cat" => has_file_operand(arguments, &[]).then_some(FileInspectionKind::View),
        "head" => has_file_operand(arguments, &["-n", "--lines", "-c", "--bytes"])
            .then_some(FileInspectionKind::View),
        "tail" => (!arguments
            .iter()
            .any(|argument| matches!(*argument, "-f" | "-F" | "--follow")))
        .then_some(())
        .filter(|_| {
            has_file_operand(
                arguments,
                &["-n", "--lines", "-c", "--bytes", "-s", "--sleep-interval"],
            )
        })
        .map(|_| FileInspectionKind::View),
        "sed" => {
            let read_range = arguments.iter().any(|argument| {
                *argument == "-n"
                    || *argument == "--quiet"
                    || *argument == "--silent"
                    || argument.starts_with("-n")
            });
            let script_index = arguments
                .iter()
                .position(|argument| !argument.starts_with('-'))?;
            let script = arguments[script_index];
            let file = arguments[script_index + 1..].iter().any(|argument| {
                !argument.starts_with('-') && *argument != "-" && !argument.contains('=')
            });
            let writes = arguments.iter().any(|argument| {
                *argument == "-i" || *argument == "--in-place" || argument.starts_with("-i")
            }) || script.contains('w')
                || script.contains('W')
                || script.contains('e');
            (read_range && !writes && file).then_some(FileInspectionKind::View)
        }
        "awk" => {
            let file = arguments.last().is_some_and(|argument| {
                !argument.starts_with('-')
                    && *argument != "-"
                    && !argument.contains(['=', '{', '}', '$', '\'', '"'])
            });
            let script_reads_file =
                segment.contains("NR") || segment.contains("$0") || segment.contains("print");
            let script_writes = segment.contains("system")
                || segment.contains("getline")
                || segment.contains('>')
                || segment.contains('|');
            (file && script_reads_file && !script_writes).then_some(FileInspectionKind::View)
        }
        "grep" | "rg" => {
            let operands = arguments
                .iter()
                .filter(|argument| !argument.starts_with('-'))
                .count();
            (arguments.iter().all(|argument| !argument.starts_with('-')) && operands >= 1)
                .then_some(FileInspectionKind::Search)
        }
        _ => None,
    }
}

/// Prevent simple file-content probes from consuming a shell turn. Native
/// `view_file` preserves line ranges and metadata, while native `grep` is the
/// structured path for content search. Keep this routing rule conservative:
/// shell syntax, writes, and any non-read-only command leave the shell path
/// available for legitimate workflows.
pub(crate) fn reject_pure_file_inspection(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty()
        || command
            .chars()
            .any(|character| matches!(character, '<' | '>' | '`'))
    {
        return None;
    }

    let mut inspected = false;
    let mut search = false;
    for segment in split_inspection_segments(command) {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if let Some(kind) = pure_file_inspection_kind(segment) {
            inspected = true;
            search |= kind == FileInspectionKind::Search;
        } else if segment
            .split_whitespace()
            .next()
            .is_some_and(|token| token.rsplit(['/', '\\']).next() == Some("cd"))
        {
            // A leading `cd` is shell setup, not file inspection itself.
        } else {
            // Any other command keeps the complete shell workflow available,
            // including Git observations and advanced grep/rg modes.
            return None;
        }
    }

    inspected.then(|| {
        format!(
            "Pure file inspection through `run_command` is not supported for this command. {} Keep `run_command` for build/test/git commands and other shell workflows; advanced ripgrep flags, counts, and file-list modes remain supported there.",
            if search {
                "Use native `grep` with `pattern` and `path` to search file contents, and `view_file` with `path` and optional `start_line`/`end_line` when you need to read a file range."
            } else {
                "Use native `view_file` with `path` and optional `start_line`/`end_line` to read file contents, or native `grep` with `pattern` and `path` to search."
            }
        )
    })
}

pub(super) fn is_short_discovery_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty()
        || command
            .chars()
            .any(|character| matches!(character, ';' | '\n' | '|' | '&' | '<' | '>' | '`' | '$'))
    {
        return false;
    }
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() || tokens.len() > 8 || !is_read_only_segment(command) {
        return false;
    }
    let binary = tokens[0].rsplit(['/', '\\']).next().unwrap_or(tokens[0]);
    let arguments = &tokens[1..];
    match binary {
        "find" => {
            let path = arguments.first().copied().unwrap_or("");
            let bounded_depth = arguments.windows(2).any(|window| {
                window[0] == "-maxdepth" && window[1].parse::<u8>().is_ok_and(|depth| depth <= 3)
            });
            !path.starts_with('/') && (path != "." || bounded_depth)
        }
        "ls" | "rg" | "stat" => !arguments
            .iter()
            .any(|argument| *argument == "/" || argument.starts_with('/')),
        _ => true,
    }
}

pub(crate) fn command_confirmation_scope(command: &str) -> Option<String> {
    let segments = split_command_segments(command);
    let git_scopes = segments
        .iter()
        .filter_map(|segment| destructive_git_scope(segment))
        .collect::<Vec<_>>();
    if !git_scopes.is_empty() {
        return Some(git_scopes.join("; "));
    }
    if command
        .chars()
        .any(|character| matches!(character, '<' | '>'))
    {
        return Some("shell redirection".to_string());
    }
    if segments.iter().all(|segment| is_read_only_segment(segment)) {
        None
    } else {
        Some("unclassified or potentially mutating shell command".to_string())
    }
}

pub(crate) fn command_requires_confirmation(args: &Value) -> bool {
    args.get("command")
        .and_then(Value::as_str)
        .map(|command| command_confirmation_scope(command).is_some())
        .unwrap_or(true)
}

pub(crate) fn command_confirmation_preview(command: &str) -> String {
    let scope = command_confirmation_scope(command).unwrap_or("command execution".to_string());
    format!("resolved command: {command}\nscope: {scope}")
}

fn segment_is_interactive_sudo(segment: &str) -> bool {
    let mut tokens = segment.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    if first.rsplit(['/', '\\']).next() != Some("sudo") {
        return false;
    }
    let mut non_interactive = false;
    let mut reads_stdin = false;
    while let Some(token) = tokens.next() {
        if token == "--" {
            break;
        }
        if let Some(long) = token.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            match name {
                "non-interactive" => non_interactive = true,
                "stdin" => reads_stdin = true,
                _ => {
                    if SUDO_LONG_OPTS_WITH_VALUE.contains(&name) && !long.contains('=') {
                        tokens.next();
                    }
                }
            }
            continue;
        }
        if let Some(short) = token.strip_prefix('-')
            && !short.is_empty()
        {
            let mut chars = short.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    'n' => non_interactive = true,
                    'S' => reads_stdin = true,
                    c if SUDO_SHORT_OPTS_WITH_VALUE.contains(c) => {
                        if chars.next().is_none() {
                            tokens.next();
                        }
                        break;
                    }
                    _ => {}
                }
            }
            continue;
        }
        break;
    }
    reads_stdin || !non_interactive
}

pub(super) fn has_interactive_sudo(cmd: &str) -> bool {
    split_command_segments(cmd)
        .iter()
        .any(|segment| segment_is_interactive_sudo(segment))
}

pub(crate) fn reject_broad_git_stage(cmd: &str) -> Option<&'static str> {
    for segment in split_command_segments(cmd) {
        let tokens = segment.split_whitespace().collect::<Vec<_>>();
        if tokens.len() >= 3
            && tokens[0] == "git"
            && tokens[1] == "commit"
            && tokens[2..]
                .iter()
                .any(|token| *token == "-a" || *token == "--all")
        {
            return Some(
                "Refusing `git commit -a/--all`. Stage explicit feature paths first so unrelated user changes cannot enter the commit.",
            );
        }
        if tokens.len() >= 3
            && tokens[0] == "git"
            && tokens[1] == "add"
            && (tokens[2] == "."
                || tokens[2] == "-A"
                || tokens[2] == "--all"
                || (tokens[2] == "--" && tokens.get(3) == Some(&".")))
        {
            return Some(
                "Refusing broad git staging. Stage explicit feature paths (for example, `git add src/network.rs`) so unrelated user changes cannot enter the commit.",
            );
        }
    }
    None
}
