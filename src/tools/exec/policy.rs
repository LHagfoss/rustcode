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

fn split_deny_command_segments(command: &str) -> Vec<(String, bool)> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut follows_pipe = false;
    for character in command.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if quote.is_some() {
            current.push(character);
            if quote == Some(character) {
                quote = None;
            } else if character == '\\' && quote == Some('"') {
                escaped = true;
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                current.push(character);
            }
            '\\' => {
                escaped = true;
                current.push(character);
            }
            ';' | '\n' | '\r' | '|' | '&' => {
                segments.push((std::mem::take(&mut current), follows_pipe));
                follows_pipe = character == '|';
            }
            _ => current.push(character),
        }
    }
    segments.push((current, follows_pipe));
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
        .filter(|(_, extension)| {
            ["exe", "cmd", "bat", "com"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
        .map(|(stem, _)| stem)
        .unwrap_or(basename)
}

fn normalized_deny_executable(command: &str) -> String {
    let basename = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let is_windows_shim = basename.rsplit_once('.').is_some_and(|(_, extension)| {
        ["exe", "cmd", "bat", "com"]
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
    });
    let basename = command_basename(command);
    if is_windows_shim {
        return basename.to_ascii_lowercase();
    }
    match basename.to_ascii_lowercase().as_str() {
        "cargo" => "cargo".to_owned(),
        "git" => "git".to_owned(),
        _ => basename.to_owned(),
    }
}

/// Canonicalize command invocation prefixes for deny matching. Git and Cargo
/// accept global options before their subcommand, and executable paths can
/// hide the same command behind a directory or Windows `.exe` suffix.
fn normalized_deny_command(tokens: &[String]) -> Vec<String> {
    let Some(executable) = tokens.first() else {
        return Vec::new();
    };
    let executable = normalized_deny_executable(executable);
    let args = &tokens[1..];
    let mut normalized = vec![executable.clone()];

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
            return normalized;
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

    if executable == "make" || executable == "gmake" {
        let mut index = 0;
        while index < args.len() {
            let argument = args[index].as_str();
            if matches!(
                argument,
                "-C" | "-f"
                    | "-I"
                    | "-o"
                    | "-W"
                    | "--directory"
                    | "--file"
                    | "--include-dir"
                    | "--old-file"
                    | "--what-if"
            ) {
                index = (index + 2).min(args.len());
            } else if argument.starts_with("--directory=")
                || argument.starts_with("--file=")
                || argument.starts_with("--include-dir=")
                || argument.starts_with("--old-file=")
                || argument.starts_with("--what-if=")
            {
                index += 1;
            } else if argument.starts_with('-') || argument.contains('=') {
                // Skip make options and variable assignments before the goal.
                // Unknown options may hide an action, so treating their next
                // plain token as a goal is conservative for deny matching.
                index += 1;
            } else {
                break;
            }
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
    Some(normalized_deny_command(&tokens).join(" "))
}

pub(crate) fn rememberable_command_forbid_prefix_for_call(args: &Value) -> Option<String> {
    rememberable_command_forbid_prefix(args.get("command")?.as_str()?)
}

/// Normalize a plain command for a persistent deny rule. Quoted words are
/// accepted and normalized because deny rules only block matching calls.
/// Shell composition and expansion syntax cannot form a stored rule.
fn plain_deny_rule_tokens(command: &str) -> Option<Vec<String>> {
    parse_deny_tokens(command, false)
}

fn deny_invocation_tokens(command: &str) -> Option<Vec<String>> {
    parse_deny_tokens(command, true)
}

fn parse_deny_tokens(command: &str, allow_simple_variables: bool) -> Option<Vec<String>> {
    let trimmed = command.trim_start();
    let command_start = trimmed
        .strip_prefix('"')
        .or_else(|| trimmed.strip_prefix('\''))
        .unwrap_or(trimmed);
    let windows_executable_path = command_start.as_bytes().get(0..3).is_some_and(|prefix| {
        prefix[0].is_ascii_alphabetic() && prefix[1] == b':' && prefix[2] == b'\\'
    });
    if command.is_empty() {
        return None;
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut started = false;
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some(active), ch) if active == ch => quote = None,
            (Some('\''), ch) => token.push(ch),
            (Some('"'), '\\')
                if characters
                    .peek()
                    .is_some_and(|next| matches!(next, '$' | '`' | '"' | '\\')) =>
            {
                token.push(characters.next().unwrap());
            }
            (Some('"'), ch) => {
                let assignment_value = is_leading_assignment_value(&tokens, &token);
                if ch == '`'
                    || ch == '$' && characters.peek() == Some(&'(')
                    || ch == '$' && !assignment_value && !allow_simple_variables
                    || ch == '^' && !assignment_value
                    || is_paired_expansion_marker(command, ch, '!') && !assignment_value
                    || is_paired_expansion_marker(command, ch, '%') && !assignment_value
                {
                    return None;
                }
                token.push(ch);
            }
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
            (None, ch)
                if ch == '`'
                    || ch == '$' && characters.peek() == Some(&'(')
                    || matches!(ch, '(' | ')')
                    || ch == '$'
                        && !is_leading_assignment_value(&tokens, &token)
                        && !allow_simple_variables
                    || (matches!(ch, ';' | '|' | '&' | '<' | '>' | '{' | '}')
                        || matches!(ch, '*' | '?' | '[' | ']' | '~' | '^'))
                        && !is_leading_assignment_value(&tokens, &token)
                    || ch == '\\' && !windows_executable_path
                    || is_paired_expansion_marker(command, ch, '!')
                        && !is_leading_assignment_value(&tokens, &token)
                    || is_paired_expansion_marker(command, ch, '%')
                        && !is_leading_assignment_value(&tokens, &token) =>
            {
                return None;
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

fn is_leading_assignment_value(tokens: &[String], token: &str) -> bool {
    !token.is_empty()
        && tokens
            .iter()
            .all(|token| is_posix_environment_assignment(token))
        && token
            .split_once('=')
            .is_some_and(|(name, _)| is_valid_environment_name(name))
}

fn is_valid_environment_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().enumerate().all(|(index, ch)| {
            ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
        })
}

fn is_paired_expansion_marker(command: &str, current: char, marker: char) -> bool {
    current == marker && command.matches(marker).count() > 1
}

/// Remove simple file redirects before tokenizing a command. This keeps paths
/// and descriptors from looking like command arguments while leaving compound
/// or expandable redirect forms fail-closed.
fn strip_simple_redirects(command: &str) -> Option<String> {
    let characters = command.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;
    let mut quote = None;
    while index < characters.len() {
        let character = characters[index];
        if let Some(active_quote) = quote {
            output.push(character);
            if character == active_quote {
                quote = None;
            } else if character == '\\' && active_quote == '"' {
                index += 1;
                if index < characters.len() {
                    output.push(characters[index]);
                }
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            output.push(character);
            index += 1;
            continue;
        }
        if !matches!(character, '<' | '>') {
            output.push(character);
            index += 1;
            continue;
        }

        let fd_suffix_start = output
            .char_indices()
            .rev()
            .take_while(|(_, ch)| ch.is_ascii_digit())
            .map(|(index, _)| index)
            .last();
        let has_separate_fd = fd_suffix_start.is_some_and(|start| {
            start == 0
                || output[..start]
                    .chars()
                    .last()
                    .is_some_and(char::is_whitespace)
        });
        if has_separate_fd {
            while output.chars().last().is_some_and(|ch| ch.is_ascii_digit()) {
                output.pop();
            }
        }
        let operator = character;
        index += 1;
        if characters.get(index) == Some(&operator) {
            // Here-documents and here-strings have their own parsing rules.
            return None;
        }
        if operator == '>' && characters.get(index) == Some(&'|') {
            return None;
        }
        while characters.get(index).is_some_and(|ch| ch.is_whitespace()) {
            index += 1;
        }
        if characters.get(index) == Some(&'&') {
            index += 1;
            if characters
                .get(index)
                .is_none_or(|ch| !ch.is_ascii_digit() && *ch != '-')
            {
                return None;
            }
            while characters
                .get(index)
                .is_some_and(|ch| ch.is_ascii_digit() || *ch == '-')
            {
                index += 1;
            }
            continue;
        }
        let Some(&first) = characters.get(index) else {
            return None;
        };
        if first == '\'' || first == '"' {
            let target_quote = first;
            index += 1;
            let target_start = index;
            while index < characters.len() && characters[index] != target_quote {
                if matches!(characters[index], '$' | '`' | '<' | '>') {
                    return None;
                }
                index += 1;
            }
            if index >= characters.len() {
                return None;
            }
            index += 1;
            if index == target_start {
                return None;
            }
        } else {
            let target_start = index;
            while index < characters.len() && !characters[index].is_whitespace() {
                if matches!(characters[index], '$' | '`' | '<' | '>') {
                    return None;
                }
                index += 1;
            }
            if index == target_start {
                return None;
            }
        }
    }
    quote.is_none().then_some(output)
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
        .map(|tokens| normalized_deny_command(&tokens))
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
    // Split shell operators only outside quotes so punctuation in search
    // patterns, messages, and other literal arguments stays literal.
    let mut previous_pipeline_segment: Option<String> = None;
    for (segment, follows_pipe) in split_deny_command_segments(command) {
        let Some(scannable_segment) = strip_simple_redirects(&segment) else {
            if segment.chars().any(|ch| !ch.is_whitespace()) {
                return true;
            }
            previous_pipeline_segment = None;
            continue;
        };
        let Some(tokens) = deny_invocation_tokens(&scannable_segment) else {
            // We cannot safely understand shell syntax with active deny rules;
            // fail closed so wrappers/redirections cannot hide a denied argv.
            if segment.chars().any(|ch| !ch.is_whitespace()) {
                return true;
            }
            previous_pipeline_segment = None;
            continue;
        };
        let mut resolved_xargs_input = false;
        if follows_pipe
            && let Some(input) = previous_pipeline_segment
            && let Some(expanded) = xargs_with_literal_input(&tokens, &input)
        {
            resolved_xargs_input = true;
            if denied_command_tokens_cover(&expanded, rules, depth + 1) {
                return true;
            }
        }
        if !resolved_xargs_input && denied_command_tokens_cover(&tokens, rules, depth + 1) {
            return true;
        }
        previous_pipeline_segment = Some(segment);
    }
    false
}

fn denied_command_tokens_cover(tokens: &[String], rules: &[Vec<String>], depth: usize) -> bool {
    if depth > 4 {
        return true;
    }
    let command_index = tokens
        .iter()
        .position(|token| !is_posix_environment_assignment(token))
        .unwrap_or(tokens.len());
    if command_index > 0 {
        return denied_command_tokens_cover(&tokens[command_index..], rules, depth + 1);
    }
    if tokens.first().is_some_and(|token| {
        matches!(
            token.as_str(),
            "if" | "then"
                | "elif"
                | "else"
                | "fi"
                | "while"
                | "until"
                | "do"
                | "done"
                | "for"
                | "select"
                | "case"
                | "esac"
                | "function"
                | "coproc"
        )
    }) {
        // Shell control words can make later segments conditional or repeat
        // them. We do not attempt to interpret that grammar for saved denies.
        return true;
    }
    if tokens.first().is_some_and(|executable| {
        executable
            .chars()
            .any(|character| matches!(character, '$' | '`' | '^' | '%' | '!'))
    }) {
        return true;
    }
    if git_command_sets_inline_alias(tokens)
        && rules
            .iter()
            .any(|rule| rule.first().is_some_and(|token| token == "git"))
    {
        return true;
    }
    let normalized = normalized_deny_command(tokens);
    if rules.iter().any(|rule| {
        !rule.is_empty()
            && normalized.first() == rule.first()
            && normalized
                .iter()
                .take(rule.len())
                .skip(1)
                .any(|token| token.contains('$'))
    }) {
        return true;
    }
    if rules
        .iter()
        .any(|rule| !rule.is_empty() && normalized.starts_with(rule))
    {
        return true;
    }
    match wrapped_command_payload(tokens) {
        Some(WrappedCommandPayload::Shell(payload)) => {
            denied_command_contains_rule(&payload, rules, depth + 1)
        }
        Some(WrappedCommandPayload::Arguments(payload)) => {
            denied_command_tokens_cover(&payload, rules, depth + 1)
        }
        Some(WrappedCommandPayload::DynamicArguments(payload, placeholder)) => {
            denied_command_tokens_cover(&payload, rules, depth + 1)
                || dynamic_target_may_match_rule(&payload, rules, placeholder.as_deref(), depth + 1)
        }
        Some(WrappedCommandPayload::Ambiguous) => true,
        None => false,
    }
}

fn is_posix_environment_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    is_valid_environment_name(name)
}

fn xargs_with_literal_input(tokens: &[String], input: &str) -> Option<Vec<String>> {
    let WrappedCommandPayload::DynamicArguments(mut payload, Some(placeholder)) =
        wrapped_command_payload(tokens)?
    else {
        return None;
    };
    let input_tokens = plain_deny_rule_tokens(input)?;
    let values = match normalized_deny_command(&input_tokens)
        .first()
        .map(String::as_str)
    {
        Some("printf") => input_tokens.into_iter().skip(1).collect::<Vec<_>>(),
        Some("echo") => input_tokens.into_iter().skip(1).collect::<Vec<_>>(),
        _ => return None,
    };
    if values.len() != 1 {
        return None;
    }
    for token in &mut payload {
        *token = token.replace(&placeholder, &values[0]);
    }
    Some(payload)
}

fn dynamic_target_may_match_rule(
    tokens: &[String],
    rules: &[Vec<String>],
    placeholder: Option<&str>,
    depth: usize,
) -> bool {
    if depth > 4 {
        return true;
    }
    let normalized = normalized_deny_command(tokens);
    if normalized.len() == 1
        && normalized
            .first()
            .is_some_and(|executable| rules.iter().any(|rule| rule.first() == Some(executable)))
    {
        return true;
    }
    if let Some(placeholder) = placeholder
        && rules.iter().any(|rule| {
            !rule.is_empty()
                && normalized.first() == rule.first()
                && normalized
                    .iter()
                    .take(rule.len())
                    .zip(rule)
                    .all(|(actual, denied)| actual == denied || actual.contains(placeholder))
                && normalized.len() >= rule.len()
        })
    {
        return true;
    }
    match wrapped_command_payload(tokens) {
        Some(WrappedCommandPayload::Arguments(payload))
        | Some(WrappedCommandPayload::DynamicArguments(payload, _)) => {
            dynamic_target_may_match_rule(&payload, rules, placeholder, depth + 1)
        }
        _ => false,
    }
}

fn git_command_sets_inline_alias(tokens: &[String]) -> bool {
    if !tokens
        .first()
        .is_some_and(|executable| normalized_deny_executable(executable) == "git")
    {
        return false;
    }
    let args = &tokens[1..];
    let mut index = 0;
    while index < args.len() {
        let config = if args[index] == "-c" {
            index += 1;
            args.get(index).map(String::as_str)
        } else if args[index].starts_with("-c") && args[index].len() > 2 {
            Some(&args[index][2..])
        } else {
            None
        };
        if config.is_some_and(|value| {
            value
                .split_once('=')
                .is_some_and(|(key, _)| key.starts_with("alias."))
        }) {
            return true;
        }
        index += 1;
    }
    false
}

enum WrappedCommandPayload {
    Shell(String),
    Arguments(Vec<String>),
    DynamicArguments(Vec<String>, Option<String>),
    Ambiguous,
}

fn wrapped_command_payload(tokens: &[String]) -> Option<WrappedCommandPayload> {
    let executable = normalized_deny_executable(tokens.first()?);
    let args = &tokens[1..];
    let args_from = |index: usize| {
        let payload = args.get(index..)?.to_vec();
        (!payload.is_empty()).then_some(payload)
    };
    let shell_from = |index: usize| {
        let payload = args.get(index..)?.join(" ");
        (!payload.is_empty()).then_some(WrappedCommandPayload::Shell(payload))
    };

    match executable.to_ascii_lowercase().as_str() {
        "git" => {
            let normalized = normalized_deny_command(tokens);
            if normalized.get(1).is_some_and(|token| token == "submodule")
                && normalized.get(2).is_some_and(|token| token == "foreach")
            {
                let mut index = 3;
                while index < normalized.len() && normalized[index].starts_with('-') {
                    if normalized[index] == "--jobs" {
                        index = (index + 2).min(normalized.len());
                    } else {
                        index += 1;
                    }
                }
                return normalized
                    .get(index)
                    .cloned()
                    .map(WrappedCommandPayload::Shell);
            }
            None
        }
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" => {
            let command_flag = args.iter().position(|arg| {
                arg == "-c"
                    || arg.starts_with('-')
                        && !arg.starts_with("--")
                        && arg.chars().skip(1).any(|flag| flag == 'c')
            })?;
            args.get(command_flag + 1)
                .cloned()
                .map(WrappedCommandPayload::Shell)
        }
        "cmd" => args
            .iter()
            .position(|arg| matches!(arg.to_ascii_lowercase().as_str(), "/c" | "/k"))
            .and_then(|index| shell_from(index + 1)),
        "powershell" | "pwsh" => args
            .iter()
            .position(|arg| matches!(arg.to_ascii_lowercase().as_str(), "-command" | "-c" | "/c"))
            .and_then(|index| shell_from(index + 1)),
        "python" | "python3" | "node" | "ruby" | "perl" => args
            .iter()
            .position(|arg| matches!(arg.as_str(), "-c" | "-e"))
            .and_then(|index| shell_from(index + 1)),
        "env" => {
            let mut index = 0;
            while index < args.len() {
                match args[index].as_str() {
                    "--" => {
                        index += 1;
                        break;
                    }
                    "-i" | "--ignore-environment" => index += 1,
                    "-u" | "--unset" | "-C" | "--chdir" => index += 2,
                    "-S" | "--split-string" => return Some(WrappedCommandPayload::Ambiguous),
                    argument if argument.starts_with('-') => index += 1,
                    argument if argument.contains('=') => index += 1,
                    _ => break,
                }
            }
            args_from(index).map(WrappedCommandPayload::Arguments)
        }
        "sudo" | "doas" => {
            let mut index = 0;
            while index < args.len() && args[index].starts_with('-') {
                let argument = args[index].as_str();
                if argument == "--" {
                    index += 1;
                    break;
                }
                if let Some(long) = argument.strip_prefix("--") {
                    let name = long.split('=').next().unwrap_or(long);
                    if SUDO_LONG_OPTS_WITH_VALUE.contains(&name) {
                        index += if long.contains('=') { 1 } else { 2 };
                    } else if matches!(
                        name,
                        "non-interactive"
                            | "stdin"
                            | "preserve-env"
                            | "login"
                            | "set-home"
                            | "shell"
                            | "bell"
                            | "close-from"
                    ) {
                        index += 1;
                    } else {
                        return Some(WrappedCommandPayload::Ambiguous);
                    }
                } else if let Some(short) = argument.strip_prefix('-') {
                    if short.is_empty() {
                        return Some(WrappedCommandPayload::Ambiguous);
                    }
                    let mut consumes_value = false;
                    for (position, option) in short.chars().enumerate() {
                        if SUDO_SHORT_OPTS_WITH_VALUE.contains(option) {
                            consumes_value = position + option.len_utf8() == short.len();
                            break;
                        }
                        if !matches!(
                            option,
                            'n' | 'S' | 'b' | 'E' | 'H' | 'K' | 'k' | 'V' | 'v' | 'l' | 'N' | 'P'
                        ) {
                            return Some(WrappedCommandPayload::Ambiguous);
                        }
                    }
                    index += if consumes_value { 2 } else { 1 };
                } else {
                    break;
                }
            }
            if args.get(index).is_some_and(|arg| arg == "--") {
                index += 1;
            }
            args_from(index).map(WrappedCommandPayload::Arguments)
        }
        "command" | "exec" | "time" | "nice" | "nohup" | "setsid" => {
            let mut index = 0;
            while index < args.len() && args[index].starts_with('-') {
                if matches!(args[index].as_str(), "-n" | "-u" | "--adjustment") {
                    index = (index + 2).min(args.len());
                } else {
                    index += 1;
                }
            }
            if args.get(index).is_some_and(|arg| arg == "--") {
                index += 1;
            }
            args_from(index).map(WrappedCommandPayload::Arguments)
        }
        "eval" => {
            let payload = args.join(" ");
            (!payload.is_empty()).then_some(WrappedCommandPayload::Shell(payload))
        }
        "builtin" => match args.first().map(String::as_str) {
            Some("eval") => {
                let payload = args[1..].join(" ");
                (!payload.is_empty()).then_some(WrappedCommandPayload::Shell(payload))
            }
            Some("source") | Some(".") => Some(WrappedCommandPayload::Ambiguous),
            Some("exec") | Some("command") => args_from(1).map(WrappedCommandPayload::Arguments),
            _ => None,
        },
        "xargs" => {
            let mut index = 0;
            let mut placeholder = None;
            while index < args.len() && args[index].starts_with('-') {
                if matches!(args[index].as_str(), "-0" | "-r" | "-t" | "-x") {
                    index += 1;
                } else if matches!(
                    args[index].as_str(),
                    "-I" | "-J" | "-d" | "-n" | "-P" | "-s" | "-a" | "-E" | "-L" | "-l"
                ) {
                    if matches!(args[index].as_str(), "-I" | "-J") {
                        placeholder = args.get(index + 1).cloned();
                    }
                    index = (index + 2).min(args.len());
                } else if args[index].starts_with("-I") || args[index].starts_with("-J") {
                    placeholder = Some(args[index][2..].to_owned());
                    index += 1;
                } else if ["-L", "-l", "-d", "-n", "-P", "-s", "-a", "-E"]
                    .iter()
                    .any(|option| {
                        args[index].starts_with(option) && args[index].len() > option.len()
                    })
                {
                    index += 1;
                } else if args[index].starts_with("--") {
                    let option = args[index].split('=').next().unwrap_or(&args[index]);
                    if matches!(
                        option,
                        "--no-run-if-empty" | "--null" | "--verbose" | "--exit" | "--replace"
                    ) {
                        index += 1;
                    } else if matches!(
                        option,
                        "--delimiter"
                            | "--max-args"
                            | "--max-procs"
                            | "--max-chars"
                            | "--arg-file"
                            | "--eof"
                            | "--max-lines"
                    ) {
                        index += if args[index].contains('=') { 1 } else { 2 };
                    } else {
                        return Some(WrappedCommandPayload::Ambiguous);
                    }
                } else {
                    return Some(WrappedCommandPayload::Ambiguous);
                }
            }
            args_from(index)
                .map(|payload| WrappedCommandPayload::DynamicArguments(payload, placeholder))
        }
        _ => None,
    }
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
            ("C:\\Tools\\NPM.CMD install", "npm install"),
            ("C:\\Tools\\npm.BAT install", "npm install"),
            ("C:\\Tools\\npm.Com install", "npm install"),
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
            ("git push", "git submodule foreach 'git push'"),
            ("git push", "git submodule foreach --recursive 'git push'"),
            ("git push", "git -c alias.ship=!git push ship"),
            ("git push", "git -c alias.ship=push ship"),
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
            ("git push", "sh -c 'git push' marker"),
            ("git push", "bash --norc -c 'git push' marker"),
            ("git push", "env sh -c 'git push' marker"),
            ("git push", "printf push | xargs git"),
            ("git push", "printf push | xargs -I_ git _ --force"),
            ("git push", "printf push | xargs -J_ git _ --force"),
            ("git push", "printf push | xargs -J _ git _ --force"),
            ("git push", "printf origin | xargs -L 1 git push"),
            ("git push", "printf origin | xargs -L1 git push"),
            ("git push", "FOO=bar git push"),
            ("git push", "FOO='static value' git push"),
            ("git push", "FOO=1 BAR=\"$VALUE\" git push"),
            ("git push", "git push 2>/dev/null"),
            ("git push", "FOO=$(git push) echo harmless"),
            ("git push", "FOO=\"$(git push)\" echo harmless"),
            ("git push", "FOO=`git push` echo harmless"),
            ("git push", "$CMD push"),
            ("git push", "\"$CMD\" push"),
            ("git push", "env \"$CMD\" push"),
            ("git push", "sh -c '$CMD push'"),
            ("git push", "sh -c \"$CMD push\""),
            ("git push", "git \"$SUBCOMMAND\""),
            ("git push", "if git push; then echo ok; fi"),
            ("git push", "! git push"),
            ("git push", "builtin eval 'git push'"),
            ("git push", "builtin exec git push"),
            ("git push", "eval git push"),
            ("git push", "eval 'git push'"),
            ("git push", "sudo --user root git push"),
            ("git push", "sudo --user=root git push"),
            ("git push", "sudo --group wheel git push"),
            ("git push", "env -S 'sh -c \"git push\"'"),
            ("make test", "make -C . test"),
            ("make test", "make -f Makefile test"),
            ("npm install", "npm.cmd install"),
            ("npm install", "NPM.CMD install"),
            ("npm install", "npm.BAT install"),
            ("npm install", "npm.com install"),
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
        for (rule, command) in [
            ("git push", "git log --oneline push"),
            ("cargo test", "rg cargo unrelated test"),
            ("cargo test", "rg 'foo[0-9]'"),
            ("cargo test", "echo 'hello!'"),
            ("cargo test", "printf 'a;b'"),
            ("git push", "printf push | xargs echo"),
            ("git push", "printf log | xargs -J_ git _ -1"),
            ("git push", "rg \"foo\\sbar\""),
            ("git push", "git status 2>/dev/null"),
            ("git push", "wc -l < README.md"),
            ("git push", "echo ok > /tmp/file"),
            ("git push", "echo \"$HOME\""),
            ("git push", "rg \"$pattern\" README.md"),
            ("git push", "FOO=bar git log"),
            ("git push", "FOO='static value' git log"),
            ("git push", "FOO=\"$VALUE\" echo harmless"),
            ("git push", "FOO=1 BAR=\"$VALUE\" git log"),
            ("git push", "sudo --user root git log"),
            ("git push", "sudo --group wheel git log"),
            ("git push", "git submodule foreach 'echo push'"),
            ("git push", "git ship"),
        ] {
            assert!(
                !denied_command_prefix_covers_call(
                    "run_command",
                    &serde_json::json!({"command":command}),
                    &[rule.to_owned()],
                ),
                "deny rule {rule:?} should not match unrelated invocation {command:?}"
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
