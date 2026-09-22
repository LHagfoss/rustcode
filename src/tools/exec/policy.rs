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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellClassification {
    ReadOnly,
    WorkspaceMutation,
    ProcessControl,
    NetworkOrExternal,
    Unclassified,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellPolicyFacts {
    pub(crate) classification: ShellClassification,
    pub(crate) has_redirection: bool,
    pub(crate) has_backgrounding: bool,
    pub(crate) has_privilege_escalation: bool,
    pub(crate) known_destructive: bool,
    pub(crate) has_network_effect: bool,
    pub(crate) has_mixed_list: bool,
    pub(crate) has_command_substitution: bool,
    pub(crate) explicit_mutation: bool,
    pub(crate) unclassified: bool,
}

impl ShellPolicyFacts {
    pub(crate) fn has_hazard(&self) -> bool {
        self.has_redirection
            || self.has_backgrounding
            || self.has_privilege_escalation
            || self.known_destructive
            || self.has_network_effect
            || self.has_mixed_list
            || self.has_command_substitution
            || self.explicit_mutation
            || self.classification == ShellClassification::Unknown
    }

    pub(crate) fn eligible_for_relaxed_advisory(&self) -> bool {
        self.classification == ShellClassification::Unclassified
            && self.unclassified
            && !self.has_hazard()
    }
}

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

fn command_binary(segment: &str) -> Option<&str> {
    segment
        .split_whitespace()
        .next()
        .map(|token| token.rsplit(['/', '\\']).next().unwrap_or(token))
}

fn has_network_effect(command: &str) -> bool {
    command_confirmation_segments(command)
        .iter()
        .any(|segment| {
            let tokens = segment.split_whitespace().collect::<Vec<_>>();
            let Some(binary) = command_binary(segment) else {
                return false;
            };
            matches!(
                binary,
                "curl"
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
            ) || (binary == "git"
                && matches!(
                    git_subcommand(&tokens).map(|(subcommand, _)| subcommand),
                    Some("clone" | "fetch" | "pull" | "push" | "ls-remote" | "submodule")
                ))
                || (binary == "gh"
                    && matches!(
                        tokens.get(1..),
                        Some(
                            ["issue", "create", ..]
                                | ["issue", "close", ..]
                                | ["pr", "create", ..]
                                | ["pr", "merge", ..]
                        )
                    ))
        })
}

fn has_process_effect(command: &str) -> bool {
    command_confirmation_segments(command)
        .iter()
        .any(|segment| {
            matches!(
                command_binary(segment),
                Some(
                    "kill"
                        | "pkill"
                        | "killall"
                        | "service"
                        | "systemctl"
                        | "launchctl"
                        | "jobs"
                        | "fg"
                        | "bg"
                        | "disown"
                        | "nohup"
                )
            )
        })
}

fn has_explicit_mutation(command: &str) -> bool {
    command_confirmation_segments(command)
        .iter()
        .any(|segment| {
            let tokens = segment.split_whitespace().collect::<Vec<_>>();
            let Some(binary) = command_binary(segment) else {
                return false;
            };
            let git_mutation = binary == "git"
                && git_subcommand(&tokens).is_some_and(|(subcommand, _)| {
                    matches!(
                        subcommand,
                        "add"
                            | "am"
                            | "apply"
                            | "bisect"
                            | "branch"
                            | "checkout"
                            | "cherry-pick"
                            | "clean"
                            | "commit"
                            | "config"
                            | "fetch"
                            | "merge"
                            | "mv"
                            | "pull"
                            | "push"
                            | "rebase"
                            | "reset"
                            | "restore"
                            | "rm"
                            | "stash"
                            | "switch"
                            | "tag"
                    )
                });
            git_mutation
                || matches!(
                    binary,
                    "rm" | "mv"
                        | "cp"
                        | "touch"
                        | "mkdir"
                        | "rmdir"
                        | "install"
                        | "chmod"
                        | "chown"
                        | "truncate"
                        | "tee"
                        | "cargo"
                        | "make"
                        | "ninja"
                        | "npm"
                        | "pnpm"
                        | "yarn"
                        | "pip"
                        | "pip3"
                )
                || (matches!(binary, "sed" | "yq")
                    && tokens.iter().any(|token| {
                        *token == "-i" || *token == "--in-place" || token.starts_with("-i")
                    }))
        })
}

fn is_bounded_unclassified_candidate(command: &str) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let Some(first_token) = tokens.first().copied() else {
        return false;
    };
    if first_token.rsplit(['/', '\\']).next() != Some(first_token) {
        return false;
    }
    let binary = Some(first_token);
    matches!(
        binary,
        Some("python" | "python3" | "node" | "ruby" | "perl")
    ) && tokens.get(1..).is_some_and(|arguments| {
        !arguments.is_empty()
            && arguments
                .iter()
                .all(|argument| matches!(*argument, "--help" | "-h" | "--version" | "-V"))
    })
}

fn command_confirmation_segments(command: &str) -> Vec<String> {
    split_command_segments(&without_null_redirects(command))
}

pub(crate) fn shell_policy_facts(command: &str) -> ShellPolicyFacts {
    let scannable = without_null_redirects(command);
    let segments = command_confirmation_segments(command);
    let has_redirection = scannable
        .chars()
        .any(|character| matches!(character, '<' | '>'));
    let has_backgrounding = scannable.contains('&');
    let has_mixed_list = scannable
        .chars()
        .any(|character| matches!(character, ';' | '\n' | '|' | '&'));
    let has_command_substitution =
        scannable.contains("$()") || scannable.contains("$(") || scannable.contains('`');
    let has_privilege_escalation = segments
        .iter()
        .any(|segment| matches!(command_binary(segment), Some("sudo" | "doas")));
    let known_destructive = segments
        .iter()
        .any(|segment| destructive_git_scope(segment).is_some());
    let has_network_effect = has_network_effect(command);
    let explicit_mutation = has_explicit_mutation(command);
    let process_effect = has_process_effect(command);
    let unclassified = segments
        .iter()
        .any(|segment| !segment.trim().is_empty() && !is_read_only_segment(segment));
    let classification = if known_destructive || explicit_mutation {
        ShellClassification::WorkspaceMutation
    } else if has_network_effect {
        ShellClassification::NetworkOrExternal
    } else if process_effect || has_privilege_escalation || has_backgrounding {
        ShellClassification::ProcessControl
    } else if has_redirection || has_command_substitution || has_mixed_list {
        ShellClassification::Unknown
    } else if !unclassified && command_confirmation_scope(command).is_none() {
        ShellClassification::ReadOnly
    } else if is_bounded_unclassified_candidate(command) {
        ShellClassification::Unclassified
    } else {
        ShellClassification::Unknown
    };

    ShellPolicyFacts {
        classification,
        has_redirection,
        has_backgrounding,
        has_privilege_escalation,
        known_destructive,
        has_network_effect,
        has_mixed_list,
        has_command_substitution,
        explicit_mutation,
        unclassified: matches!(
            classification,
            ShellClassification::Unclassified | ShellClassification::Unknown
        ) && unclassified,
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
