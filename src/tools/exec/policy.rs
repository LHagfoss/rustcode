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

fn command_basename(command: &str) -> &str {
    let basename = command.rsplit(['/', '\\']).next().unwrap_or(command);
    basename
        .rsplit_once('.')
        .filter(|(_, extension)| extension.eq_ignore_ascii_case("exe"))
        .map(|(stem, _)| stem)
        .unwrap_or(basename)
}

/// Canonicalize command invocation prefixes for deny matching. Git and Cargo
/// accept global options before their subcommand, and executable paths can
/// hide the same command behind a directory or Windows `.exe` suffix.
fn normalized_deny_command(tokens: &[String], start: usize) -> Vec<String> {
    let Some(executable) = tokens.get(start) else {
        return Vec::new();
    };
    let executable = match command_basename(executable).to_ascii_lowercase().as_str() {
        "cargo" => "cargo",
        "git" => "git",
        _ => command_basename(executable),
    };
    let args = &tokens[start + 1..];
    let mut normalized = vec![executable.to_owned()];

    if executable == "git" {
        let mut git_tokens = Vec::with_capacity(args.len() + 1);
        git_tokens.push("git");
        git_tokens.extend(args.iter().map(String::as_str));
        if let Some((subcommand, index)) = git_subcommand(&git_tokens) {
            normalized.push(subcommand.to_owned());
            normalized.extend(
                git_tokens[index + 1..]
                    .iter()
                    .map(|token| (*token).to_owned()),
            );
        } else {
            normalized.extend(args.iter().cloned());
        }
        return normalized;
    }

    if executable == "cargo" {
        let mut index = 0;
        while index < args.len() {
            let argument = args[index].as_str();
            if argument.starts_with('+') && argument.len() > 1 {
                index += 1;
                continue;
            }
            if matches!(argument, "--color" | "--config" | "-Z" | "-C") {
                index = (index + 2).min(args.len());
                continue;
            }
            if matches!(
                argument,
                "--verbose" | "-v" | "--quiet" | "-q" | "--frozen" | "--locked" | "--offline"
            ) || argument.starts_with("--color=")
                || argument.starts_with("--config=")
                || argument.starts_with("-Z")
            {
                index += 1;
                continue;
            }
            break;
        }
        normalized.extend(args[index..].iter().cloned());
        return normalized;
    }

    normalized.extend(args.iter().cloned());
    normalized
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
            "arch" | "basename" | "blkid" | "cat" | "column" | "cut" | "date" | "df" | "dirname"
            | "du" | "echo" | "false" | "file" | "free" | "getent" | "grep" | "groups" | "head"
            | "hexdump" | "id" | "jq" | "less" | "logname" | "ls" | "lsblk" | "lscpu" | "lsmod"
            | "lspci" | "lsusb" | "modinfo" | "more" | "netstat" | "nl" | "nproc" | "od" | "printf"
            | "ps" | "pwd" | "readlink" | "realpath" | "rg" | "sort" | "ss" | "stat" | "strings"
            | "tac" | "tail" | "test" | "tr" | "true" | "type" | "uname" | "uniq" | "uptime" | "w"
            | "wc" | "which" | "who" | "whoami" | "xxd",
        ) => true,
        Some("dmesg") => {
            // Reads the kernel ring buffer; `-C`/`--clear`/`--read-clear`
            // discard it.
            !tokens[1..].iter().any(|argument| {
                matches!(
                    *argument,
                    "-C" | "--clear" | "--read-clear" | "-c" | "--console-off"
                )
            })
        }
        Some("hostname") => {
            // Bare `hostname` prints the name; `hostname <name>` sets it.
            let rest = tokens.get(1..).unwrap_or_default();
            rest.is_empty() || rest.iter().all(|argument| argument.starts_with('-'))
        }
        Some("flatpak") => {
            matches!(
                tokens.get(1..).unwrap_or_default(),
                ["list", ..] | ["info", ..] | ["--version", ..] | ["--help", ..] | ["-h", ..]
            )
        }
        Some("hostnamectl") => {
            // `hostnamectl` with no subcommand prints status; `set-hostname`
            // and friends mutate. Only the status form is inspection.
            let rest = tokens.get(1..).unwrap_or_default();
            rest.is_empty()
                || rest.iter().all(|argument| argument.starts_with('-'))
                || matches!(rest.first(), Some(first) if *first == "status")
        }
        Some("pacman") | Some("paru") => {
            // Package queries (`-Q*`, optionally with package names) are
            // local inspection; `-S`/`-R`/`-U` sync, install, or remove.
            // Help/version flags are also safe.
            let rest = tokens.get(1..).unwrap_or_default();
            rest.iter().any(|argument| argument.starts_with("-Q"))
                && rest.iter().all(|argument| {
                    argument.starts_with("-Q")
                        || !argument.starts_with('-')
                        || matches!(
                            *argument,
                            "-h" | "--help" | "-V" | "--version" | "-v" | "--verbose"
                        )
                })
        }
        Some("sysctl") => {
            // Reads kernel state; `-w`/`--write` and `key=value` assignments
            // change it.
            !tokens[1..].iter().any(|argument| {
                *argument == "-w"
                    || *argument == "--write"
                    || (argument.contains('=') && !argument.starts_with('-'))
            })
        }
        Some("systemctl") => {
            matches!(
                tokens.get(1..).unwrap_or_default(),
                ["status", ..]
                    | ["show", ..]
                    | ["cat", ..]
                    | ["is-active", ..]
                    | ["is-enabled", ..]
                    | ["is-failed", ..]
                    | ["list-units", ..]
                    | ["list-unit-files", ..]
                    | ["list-sockets", ..]
                    | ["list-timers", ..]
                    | ["help", ..]
                    | ["--version", ..]
                    | ["--help", ..]
            )
        }
        Some("timedatectl") => {
            // Bare `timedatectl` prints status; `set-time`/`set-timezone`
            // mutate. Only the status form is inspection.
            let rest = tokens.get(1..).unwrap_or_default();
            rest.is_empty()
                || rest.iter().all(|argument| argument.starts_with('-'))
                || matches!(
                    rest.first(),
                    Some(first) if *first == "status" || *first == "show" || *first == "show-timesync"
                )
        }
        Some("yq") => {
            // Like sed: reads unless editing in place.
            !tokens[1..]
                .iter()
                .any(|argument| *argument == "-i" || *argument == "--in-place")
        }
        Some("sed") => is_read_only_sed(segment),
        Some("npm") => {
            matches!(tokens.get(1..).unwrap_or_default(), ["config", "get", key] if !key.starts_with('-'))
        }
        _ => false,
    }
}

fn is_read_only_sed(segment: &str) -> bool {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let arguments = tokens.get(1..).unwrap_or_default();
    let Some(script_index) = arguments
        .iter()
        .position(|argument| !argument.starts_with('-'))
    else {
        return false;
    };
    let script = arguments[script_index];
    let has_file = arguments[script_index + 1..]
        .iter()
        .any(|argument| !argument.starts_with('-') && *argument != "-" && !argument.contains('='));
    let writes = arguments.iter().any(|argument| {
        *argument == "-i" || *argument == "--in-place" || argument.starts_with("-i")
    }) || script.contains('w')
        || script.contains('W')
        || script.contains('e');
    has_file && !writes
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

/// Match a redirection that cannot clobber files at the start of `segment`,
/// returning its byte length: sinks into `/dev/null` (`>/dev/null`,
/// `2>>/dev/null`, `&>/dev/null`, `< /dev/null`) and fd duplications
/// (`2>&1`, `>&2`, `2>&-`). Anything else — including `> file`, `>> log`,
/// and heredocs (`<<EOF`) — returns `None` so the caller stays conservative.
fn match_null_redirect(segment: &str) -> Option<usize> {
    let bytes = segment.as_bytes();
    let mut index = 0;
    let amp_prefix = bytes.first() == Some(&b'&');
    if amp_prefix {
        index += 1;
    } else {
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
    }
    if segment[index..].starts_with(">>") {
        index += 2;
    } else if segment[index..].starts_with('>') || segment[index..].starts_with('<') {
        index += 1;
    } else {
        return None;
    }
    while segment[index..].starts_with(' ') || segment[index..].starts_with('\t') {
        index += 1;
    }
    let target = &segment[index..];
    if target.starts_with("/dev/null")
        && target["/dev/null".len()..]
            .chars()
            .next()
            .map_or(true, |c| {
                !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/')
            })
    {
        return Some(index + "/dev/null".len());
    }
    // `2>&1` / `>&2` duplicate one fd onto another; `2>&-` closes one.
    // No path is involved, so no file can be clobbered.
    if target.starts_with('&') {
        let after_amp = &target[1..];
        if after_amp.starts_with('-') {
            return Some(index + 2);
        }
        if let Some(digit) = after_amp.chars().next()
            && digit.is_ascii_digit()
        {
            return Some(index + 1 + digit.len_utf8());
        }
    }
    if amp_prefix && target.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        let digit_len = target.chars().next().unwrap().len_utf8();
        return Some(index + digit_len);
    }
    None
}

/// Strip redirections that cannot clobber files (see [`match_null_redirect`])
/// so `lscpu 2>/dev/null | head` classifies by its commands, not its sink.
/// Real redirections (`> file`, `<<EOF`) survive, keeping the policy fail-closed.
fn without_null_redirects(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut index = 0;
    while index < command.len() {
        if let Some(len) = match_null_redirect(&command[index..]) {
            out.push_str("  ");
            index += len;
        } else {
            let next_len = command[index..].chars().next().unwrap().len_utf8();
            out.push_str(&command[index..index + next_len]);
            index += next_len;
        }
    }
    out
}

pub(crate) fn command_confirmation_scope(command: &str) -> Option<String> {
    // Strip `/dev/null` sinks and fd duplications first so `|`/`&` inside
    // them (`2>&1`) don't split phantom segments below.
    let scannable = without_null_redirects(command);
    let segments = split_command_segments(&scannable);
    let git_scopes = segments
        .iter()
        .filter_map(|segment| destructive_git_scope(segment))
        .collect::<Vec<_>>();
    if !git_scopes.is_empty() {
        return Some(git_scopes.join("; "));
    }
    // Redirections into `/dev/null` and fd duplications (`2>&1`) cannot
    // clobber files, so they don't force confirmation on their own.
    if scannable
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

/// Parse the narrow, plain-token subset used by reusable allow and deny rules.
/// The command still runs through the normal shell, so shell syntax and
/// commands with obvious external or destructive effects stay one-time only.
fn reusable_rule_tokens(command: &str, allow: bool) -> Option<Vec<String>> {
    if command.is_empty()
        || command.chars().any(|ch| {
            matches!(
                ch,
                '\n' | '\r'
                    | ';'
                    | '|'
                    | '&'
                    | '<'
                    | '>'
                    | '`'
                    | '$'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '\\'
                    | '\''
                    | '"'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '!'
                    | '~'
                    | '^'
                    | '%'
            )
        })
    {
        return None;
    }
    let tokens = command
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if tokens.is_empty()
        || (allow && tokens.len() < 2)
        || tokens.iter().any(|token| token.is_empty())
    {
        return None;
    }
    let binary = tokens[0].rsplit(['/', '\\']).next()?;
    if !allow {
        return Some(tokens);
    }
    if matches!(
        binary,
        "sudo"
            | "doas"
            | "env"
            | "command"
            | "exec"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "curl"
            | "wget"
            | "ssh"
            | "scp"
            | "sftp"
            | "rsync"
            | "nc"
            | "ncat"
            | "telnet"
            | "ftp"
            | "ping"
            | "dig"
            | "nslookup"
            | "nmap"
            | "gh"
            | "aws"
            | "gcloud"
            | "az"
            | "docker"
            | "podman"
            | "kubectl"
            | "terraform"
            | "rm"
            | "mv"
            | "cp"
            | "touch"
            | "mkdir"
            | "rmdir"
            | "install"
            | "chmod"
            | "chown"
            | "truncate"
            | "tee"
            | "sed"
            | "yq"
            | "tar"
            | "unzip"
            | "7z"
            | "zip"
            | "kill"
            | "pkill"
            | "killall"
            | "service"
            | "systemctl"
            | "launchctl"
            | "nohup"
            | "dd"
            | "shred"
            | "wipefs"
            | "fdisk"
            | "sfdisk"
            | "parted"
            | "mkfs"
            | "diskutil"
            | "mount"
            | "umount"
    ) {
        return None;
    }
    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "clean"
                | "destroy"
                | "delete"
                | "wipe"
                | "purge"
                | "prune"
                | "reset"
                | "push"
                | "publish"
                | "deploy"
                | "release"
        )
    }) {
        return None;
    }
    if matches!(binary, "pip" | "pip3")
        || matches!(binary, "npm" | "pnpm" | "yarn" | "bun")
            && tokens
                .iter()
                .any(|token| matches!(token.as_str(), "install" | "add" | "publish" | "link"))
        || binary == "cargo"
            && tokens
                .iter()
                .any(|token| matches!(token.as_str(), "install" | "publish" | "login" | "owner"))
    {
        return None;
    }
    if tokens[0].contains('=')
        || tokens
            .get(1)
            .is_some_and(|arg| arg == "-c" || arg == "--command")
    {
        return None;
    }
    // Do not turn an interpreter/module prefix into a broad reusable rule.
    if matches!(binary, "python" | "python3") {
        if tokens
            .get(1)
            .is_some_and(|arg| matches!(arg.as_str(), "-c" | "-e"))
            || tokens.len() == 1
        {
            return None;
        }
        if tokens.get(1).is_some_and(|arg| arg == "-m")
            && !tokens
                .get(2)
                .is_some_and(|module| matches!(module.as_str(), "pytest" | "unittest"))
        {
            return None;
        }
    } else if matches!(binary, "node" | "ruby" | "perl")
        && (tokens
            .get(1)
            .is_some_and(|arg| matches!(arg.as_str(), "-c" | "-m" | "-e"))
            || tokens.len() == 1)
    {
        return None;
    }
    // Shell flags that redirect output, affect global state, or force
    // destructive behavior must always be reviewed again.
    if tokens.iter().any(|token| {
        let lower = token.to_ascii_lowercase();
        matches!(
            token.as_str(),
            "-f" | "--force" | "--global" | "-g" | "--output" | "-o"
        ) || lower.starts_with("--output=")
            || lower.starts_with("--prefix=")
            || lower.starts_with("--registry=")
    }) {
        return None;
    }
    Some(tokens)
}

/// The short prefix shown to the user for a persistent reusable approval.
pub(crate) fn rememberable_command_prefix(command: &str) -> Option<String> {
    let tokens = reusable_rule_tokens(command, true)?;
    Some(tokens.join(" "))
}

pub(crate) fn rememberable_command_prefix_for_call(args: &Value) -> Option<String> {
    if args
        .get("env")
        .is_some_and(|env| !env.as_object().is_some_and(|values| values.is_empty()))
        || ["background", "detached"].iter().any(|name| {
            args.get(*name)
                .is_some_and(|value| value.as_bool() != Some(false))
        })
    {
        return None;
    }
    rememberable_command_prefix(args.get("command")?.as_str()?)
}

/// Whether a saved reusable rule explicitly covers this plain command call.
/// Keep the argument-shape checks shared between parent and subagent paths.
pub(crate) fn approved_command_prefix_covers_call(
    name: &str,
    args: &Value,
    prefixes: &[String],
) -> bool {
    if name != "run_command" || rememberable_command_prefix_for_call(args).is_none() {
        return false;
    }
    args.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            prefixes
                .iter()
                .any(|prefix| command_prefix_rule_matches(prefix, command))
        })
}

/// Match exactly the normalized argv the user reviewed. Without an enforced
/// OS sandbox, allowing additional operands or flags could widen the effect.
/// Rules with shell syntax or a high-risk command family are ignored even if
/// present in config.
pub(crate) fn command_prefix_rule_matches(rule: &str, command: &str) -> bool {
    let Some(rule_tokens) = reusable_rule_tokens(rule, true) else {
        return false;
    };
    let Some(command_tokens) = reusable_rule_tokens(command, true) else {
        return false;
    };
    command_tokens == rule_tokens
}

pub(crate) fn rememberable_command_forbid_prefix(command: &str) -> Option<String> {
    let tokens = plain_deny_rule_tokens(command)?;
    Some(normalized_deny_command(&tokens, 0).join(" "))
}

pub(crate) fn rememberable_command_forbid_prefix_for_call(args: &Value) -> Option<String> {
    rememberable_command_forbid_prefix(args.get("command")?.as_str()?)
}

/// Normalize a plain command for a persistent deny rule. Quoted words are
/// accepted and normalized because deny rules only block matching calls.
/// Shell composition and expansion syntax cannot form a stored rule.
fn plain_deny_rule_tokens(command: &str) -> Option<Vec<String>> {
    let command_start = command
        .trim_start()
        .strip_prefix('"')
        .or_else(|| command.trim_start().strip_prefix('\''))
        .unwrap_or_else(|| command.trim_start());
    let windows_executable_path = command_start.as_bytes().get(0..3).is_some_and(|prefix| {
        prefix[0].is_ascii_alphabetic() && prefix[1] == b':' && prefix[2] == b'\\'
    });
    if command.is_empty()
        || command.chars().any(|ch| {
            matches!(
                ch,
                '\n' | '\r'
                    | ';'
                    | '|'
                    | '&'
                    | '<'
                    | '>'
                    | '`'
                    | '$'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '*'
                    | '?'
                    | '['
                    | ']'
                    | '!'
                    | '~'
                    | '^'
                    | '%'
            ) || ch == '\\' && !windows_executable_path
        })
    {
        return None;
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut started = false;
    for character in command.chars() {
        match (quote, character) {
            (Some(active), ch) if active == ch => quote = None,
            (Some(_), ch) => token.push(ch),
            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }
            (None, ch) if ch.is_whitespace() => {
                if started {
                    tokens.push(std::mem::take(&mut token));
                    started = false;
                }
            }
            (None, ch) => {
                token.push(ch);
                started = true;
            }
        }
    }
    if quote.is_some() || !started && tokens.is_empty() {
        return None;
    }
    if started {
        tokens.push(token);
    }
    (!tokens.is_empty()).then_some(tokens)
}

/// Match a persistent deny prefix before regular command approval. Known
/// command paths and global options are normalized, and composed segments and
/// nested wrapper payloads are checked so they cannot hide a saved rule.
pub(crate) fn denied_command_prefix_covers_call(
    name: &str,
    args: &Value,
    prefixes: &[String],
) -> bool {
    if name != "run_command" || prefixes.is_empty() {
        return false;
    }
    let Some(command) = args.get("command").and_then(Value::as_str) else {
        return false;
    };
    let rule_tokens = prefixes
        .iter()
        .filter_map(|prefix| plain_deny_rule_tokens(prefix))
        .map(|tokens| normalized_deny_command(&tokens, 0))
        .collect::<Vec<_>>();
    if rule_tokens.is_empty() {
        return false;
    }
    denied_command_contains_rule(command, &rule_tokens, 0)
}

fn denied_command_contains_rule(command: &str, rules: &[Vec<String>], depth: usize) -> bool {
    if depth > 4 {
        return true;
    }
    // This splitter deliberately separates operators even inside quotes. For
    // deny decisions that is conservative: any segment matching a saved rule
    // blocks the whole composed command.
    for segment in split_command_segments(command) {
        let Some(tokens) = plain_deny_rule_tokens(&segment) else {
            // We cannot safely understand shell syntax with active deny rules;
            // fail closed so wrappers/redirections cannot hide a denied argv.
            if segment.chars().any(|ch| !ch.is_whitespace()) {
                return true;
            }
            continue;
        };
        for start in 0..tokens.len() {
            let normalized = normalized_deny_command(&tokens, start);
            if rules
                .iter()
                .any(|rule| !rule.is_empty() && normalized.starts_with(rule))
            {
                return true;
            }
        }
        // `sh -c`, `bash -lc`, and equivalent wrappers store the payload as
        // one argv token. Inspect it recursively; unparseable payloads fail
        // closed above.
        for (index, token) in tokens.iter().enumerate() {
            let binary = command_basename(token);
            if matches!(
                binary.to_ascii_lowercase().as_str(),
                "sh" | "bash"
                    | "zsh"
                    | "fish"
                    | "dash"
                    | "ksh"
                    | "env"
                    | "sudo"
                    | "doas"
                    | "command"
                    | "exec"
                    | "time"
                    | "nice"
                    | "nohup"
                    | "setsid"
                    | "xargs"
                    | "python"
                    | "python3"
                    | "node"
                    | "ruby"
                    | "perl"
                    | "cmd"
                    | "powershell"
                    | "pwsh"
            ) {
                for payload in tokens.iter().skip(index + 1) {
                    if denied_command_contains_rule(payload, rules, depth + 1) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod command_prefix_tests {
    use super::{
        approved_command_prefix_covers_call, command_prefix_rule_matches,
        denied_command_prefix_covers_call, rememberable_command_forbid_prefix,
        rememberable_command_forbid_prefix_for_call, rememberable_command_prefix,
        rememberable_command_prefix_for_call,
    };

    #[test]
    fn saved_allow_rules_match_exact_normalized_argv_only() {
        assert!(command_prefix_rule_matches(
            "cargo test --lib",
            "cargo   test --lib"
        ));
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "cargo test --lib"
        ));
        assert!(!command_prefix_rule_matches("cargo test", "cargo testing"));
        assert!(!command_prefix_rule_matches("cargo test", "cargo check"));
        assert!(!command_prefix_rule_matches(
            "git add src/main.rs",
            "git add src/main.rs ."
        ));
        assert!(!command_prefix_rule_matches(
            "make test",
            "make test upload-prod"
        ));
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "cargo test --all-features"
        ));
    }

    #[test]
    fn saved_prefix_never_matches_shell_composition_or_privileged_commands() {
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "cargo test; rm -rf /"
        ));
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "cargo test | sh"
        ));
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "cargo test clean"
        ));
        assert!(!command_prefix_rule_matches(
            "cargo test",
            "sudo cargo test"
        ));
        assert!(rememberable_command_prefix("cargo test").is_some());
        assert!(
            rememberable_command_prefix_for_call(
                &serde_json::json!({"command":"cargo test", "background":true})
            )
            .is_none()
        );
        assert!(rememberable_command_prefix("rm file").is_none());
        assert!(rememberable_command_prefix("curl https://example.com").is_none());
        assert!(rememberable_command_prefix("git diff --output=report.txt").is_none());
        for command in [
            "cargo test $FLAGS",
            "cargo test ${FLAGS}",
            "cargo test $(echo --all-features)",
            "cargo test `echo --all-features`",
            "cargo test *.rs",
            "cargo test ~/workspace",
        ] {
            assert!(rememberable_command_prefix(command).is_none(), "{command}");
        }
        assert!(rememberable_command_prefix("make clean").is_none());
        assert!(rememberable_command_prefix("npm run clean").is_none());
        assert!(!command_prefix_rule_matches(
            "git diff",
            "git diff --output=report.txt"
        ));
    }

    #[test]
    fn reusable_prefixes_bind_to_vetted_command_actions() {
        assert_eq!(
            rememberable_command_prefix("cargo +stable test"),
            Some("cargo +stable test".to_owned())
        );
        assert!(!command_prefix_rule_matches(
            "cargo +stable test",
            "cargo +stable publish"
        ));

        assert_eq!(
            rememberable_command_prefix("python -m pytest"),
            Some("python -m pytest".to_owned())
        );
        assert!(!command_prefix_rule_matches(
            "python -m pytest",
            "python -m http.server"
        ));
        assert!(!command_prefix_rule_matches(
            "python -m pytest",
            "pip install package"
        ));
        assert!(!command_prefix_rule_matches(
            "python -m pytest",
            "python -m pip install package"
        ));
        assert!(rememberable_command_prefix("python -m").is_none());

        for command in ["pip install package", "pip3 install package"] {
            assert!(rememberable_command_prefix(command).is_none(), "{command}");
        }
        for command in [
            "npm install -g package",
            "npm publish",
            "npm install package --global",
            "pnpm add -g package",
            "yarn global add package",
            "yarn publish",
        ] {
            assert!(rememberable_command_prefix(command).is_none(), "{command}");
        }
        for command in [
            "make test",
            "git add src/main.rs",
            "npm test",
            "go test ./...",
        ] {
            assert_eq!(
                rememberable_command_prefix(command).as_deref(),
                Some(command)
            );
        }
    }

    #[test]
    fn approved_rules_cover_only_the_exact_plain_call() {
        let prefixes = vec!["cargo test".to_owned()];
        let args = |command: &str, extra: serde_json::Value| {
            let mut args = serde_json::json!({"command": command});
            args.as_object_mut().unwrap().extend(
                extra
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
            args
        };
        assert!(!approved_command_prefix_covers_call(
            "run_command",
            &args("cargo test --lib", serde_json::json!({})),
            &prefixes,
        ));
        assert!(approved_command_prefix_covers_call(
            "run_command",
            &args("cargo test", serde_json::json!({})),
            &prefixes,
        ));
        assert!(!approved_command_prefix_covers_call(
            "run_command",
            &args(
                "cargo test",
                serde_json::json!({"env":{"RUSTFLAGS":"-Dwarnings"}})
            ),
            &prefixes,
        ));
        assert!(!approved_command_prefix_covers_call(
            "run_command",
            &args("cargo +stable test", serde_json::json!({})),
            &prefixes,
        ));
        assert!(!approved_command_prefix_covers_call(
            "write_to_file",
            &args("cargo test", serde_json::json!({})),
            &prefixes,
        ));
    }

    #[test]
    fn saved_forbid_cannot_be_bypassed_by_modifiers_wrappers_or_composition() {
        let forbidden = vec!["cargo test".to_owned()];
        assert_eq!(
            rememberable_command_forbid_prefix("cargo test"),
            Some("cargo test".to_owned())
        );
        for (command, expected) in [
            ("git -C . push", "git push"),
            ("cargo --color=always test", "cargo test"),
            ("cargo +stable test", "cargo test"),
            ("/usr/bin/cargo test", "cargo test"),
            ("C:\\Rust\\cargo.exe test", "cargo test"),
            ("\"C:\\Program Files\\Rust\\cargo.exe\" test", "cargo test"),
            ("C:\\Rust\\CARGO.ExE test", "cargo test"),
        ] {
            assert_eq!(
                rememberable_command_forbid_prefix(command).as_deref(),
                Some(expected),
                "deny rule normalization for {command:?}"
            );
        }
        assert!(denied_command_prefix_covers_call(
            "run_command",
            &serde_json::json!({"command":"cargo test --lib"}),
            &forbidden,
        ));
        assert!(denied_command_prefix_covers_call(
            "run_command",
            &serde_json::json!({"command":"cargo test; echo unexpected"}),
            &forbidden,
        ));
        for args in [
            serde_json::json!({"command":"cargo   test"}),
            serde_json::json!({"command":"cargo 'test'"}),
            serde_json::json!({"command":"env FOO=1 cargo test"}),
            serde_json::json!({"command":"sudo cargo test"}),
            serde_json::json!({"command":"sh -c 'cargo test'"}),
            serde_json::json!({"command":"echo okay; cargo test"}),
            serde_json::json!({"command":"ca^rgo test"}),
            serde_json::json!({"command":"cargo %FLAGS% test"}),
            serde_json::json!({"command":"cargo !FLAGS! test"}),
            serde_json::json!({"command":"cargo test", "env":{"RUSTFLAGS":"-Dwarnings"}}),
            serde_json::json!({"command":"cargo test", "background":true}),
            serde_json::json!({"command":"cargo test", "detached":true}),
        ] {
            assert!(
                denied_command_prefix_covers_call("run_command", &args, &forbidden),
                "deny rule should cover {args}"
            );
        }
        for (rule, command) in [
            ("git push", "git -C . push"),
            ("git push", "git -c color.ui=always push"),
            ("cargo test", "cargo --color=always test"),
            ("cargo test", "cargo --color always test"),
            ("cargo test", "cargo +stable test"),
            ("cargo test", "cargo --config config.toml test"),
            ("cargo test", "/usr/bin/cargo test"),
            ("cargo test", "C:\\Rust\\cargo.exe test"),
            ("cargo test", "cmd.exe /C \"cargo test\""),
            ("cargo test", "CMD /c \"cargo test\""),
            ("cargo test", "powershell -Command \"cargo test\""),
            ("cargo test", "pwsh -Command \"cargo test\""),
        ] {
            assert!(
                denied_command_prefix_covers_call(
                    "run_command",
                    &serde_json::json!({"command":command}),
                    &[rule.to_owned()],
                ),
                "deny rule {rule:?} should cover {command:?}"
            );
        }
        assert!(!denied_command_prefix_covers_call(
            "run_command",
            &serde_json::json!({"command":"cargo testing"}),
            &forbidden,
        ));
        assert_eq!(
            rememberable_command_forbid_prefix("cargo 'test'"),
            Some("cargo test".to_owned())
        );
        for command in ["ca^rgo test", "cargo %FLAGS% test", "cargo !FLAGS! test"] {
            assert_eq!(rememberable_command_forbid_prefix(command), None);
        }
        for args in [
            serde_json::json!({"command":"cargo test", "env":{"RUSTFLAGS":"-Dwarnings"}}),
            serde_json::json!({"command":"cargo test", "background":true}),
            serde_json::json!({"command":"cargo test", "detached":true}),
        ] {
            assert_eq!(
                rememberable_command_forbid_prefix_for_call(&args),
                Some("cargo test".to_owned()),
                "deny option should remain available for {args}"
            );
        }
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

/// Return the explicitly requested base branch from a `gh pr create` command.
///
/// The command is still executed by the normal shell path; this narrow parser
/// only gives the harness enough information to avoid asking GitHub to create
/// a PR when the local repository has no corresponding remote base.
pub(crate) fn pull_request_base(command: &str) -> Option<String> {
    split_command_segments(command)
        .into_iter()
        .find_map(|segment| {
            let tokens = segment.split_whitespace().collect::<Vec<_>>();
            let binary = tokens.first()?.rsplit(['/', '\\']).next()?;
            if binary != "gh"
                || tokens.get(1).copied() != Some("pr")
                || tokens.get(2).copied() != Some("create")
            {
                return None;
            }

            tokens[3..]
                .windows(2)
                .find_map(|pair| {
                    (pair[0] == "--base").then(|| pair[1].trim_matches(['\'', '"']).to_string())
                })
                .or_else(|| {
                    tokens[3..].iter().find_map(|token| {
                        token
                            .strip_prefix("--base=")
                            .map(|base| base.trim_matches(['\'', '"']).to_string())
                    })
                })
                .filter(|base| !base.is_empty())
        })
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
