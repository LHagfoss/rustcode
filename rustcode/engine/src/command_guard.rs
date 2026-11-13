//! The command guard: a per-project shell policy that rejects or rewrites the
//! commands an agent runs (`docs/command-guard.md`).
//!
//! A project opts in with `.rustcode/command-policy.toml`. Each `[[rule]]`
//! names a program (and optionally a subcommand and flags) and either denies
//! it with a message or rewrites it with a prefix, such as running heavy
//! `cargo` at background priority. The matcher reads the command the way a
//! shell would at the level that matters here: quotes, `&&`/`;`/`|` lists,
//! leading `VAR=value` assignments, wrappers such as `env`, `timeout`,
//! `nice` or `taskpolicy`, and `sh -c '…'` scripts.
//!
//! The built-in harness applies the verdict in its exec tool. A harness
//! RustCode does not execute for gets the same verdict through its approval
//! requests, and Claude Code through a `PreToolUse` hook that runs
//! `rustcode guard-command`. Every violation is counted per thread in the
//! data store as a `command_guard.violation` operational event.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Where a project keeps its policy, relative to the project root.
pub const POLICY_PATH: &str = ".rustcode/command-policy.toml";

/// The operational event a violation is recorded as.
pub const VIOLATION_EVENT: &str = "command_guard.violation";

/// The thread a hook process attributes its violations to.
pub const THREAD_ENV: &str = "RUSTCODE_THREAD_ID";

/// How many `sh -c '…'` layers the matcher looks into.
const MAX_NESTING: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Deny,
    Rewrite,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    name: String,
    action: Action,
    /// The program, by its file name: `cargo`, `git`.
    program: String,
    /// Match only when the first non-option argument is one of these
    /// (`test` in `cargo +nightly test`). Empty matches any.
    #[serde(default)]
    subcommand: Vec<String>,
    /// Match only when one of these flags is present, as `--flag` or
    /// `--flag=value`. Empty matches any.
    #[serde(default)]
    flags: Vec<String>,
    /// What the agent is told: why, and what to run instead.
    #[serde(default)]
    message: String,
    /// An environment variable that lets the command through when set to a
    /// value other than empty, `0` or `false`, in the command or in the
    /// environment RustCode runs in.
    #[serde(default)]
    allow_env: Option<String>,
    /// Rewrite: the words put before the command, on any system without a
    /// more specific one below.
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    prefix_macos: Option<String>,
    #[serde(default)]
    prefix_linux: Option<String>,
    /// Rewrite: leave the command alone when it already runs under one of
    /// these wrappers (`taskpolicy`, `nice`).
    #[serde(default)]
    skip_if_wrapped: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default, rename = "rule")]
    rules: Vec<Rule>,
}

fn enabled_by_default() -> bool {
    true
}

/// A project's command policy.
#[derive(Debug, Clone)]
pub struct Policy {
    pub path: PathBuf,
    enabled: bool,
    rules: Vec<Rule>,
}

/// What the policy says about one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Run `command` instead; `rules` are the rules that changed it.
    Rewrite {
        command: String,
        rules: Vec<String>,
    },
    Deny {
        rule: String,
        message: String,
    },
}

/// What a verdict depends on outside the command. Fields are plain function
/// pointers so a test can stand in for the machine.
#[derive(Clone, Copy)]
pub struct Host {
    pub os: &'static str,
    pub var: fn(&str) -> Option<String>,
    pub has_program: fn(&str) -> bool,
}

impl Host {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS,
            var: |name| std::env::var(name).ok(),
            has_program: |name| {
                let Some(path) = std::env::var_os("PATH") else {
                    return false;
                };
                std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
            },
        }
    }
}

impl Policy {
    pub fn parse(text: &str, path: PathBuf) -> Result<Self, String> {
        let file: PolicyFile = toml::from_str(text).map_err(|error| error.to_string())?;
        for rule in &file.rules {
            if rule.name.trim().is_empty() {
                return Err("a rule has no name".to_owned());
            }
            if rule.program.trim().is_empty() {
                return Err(format!("rule `{}` has no program", rule.name));
            }
            if rule.action == Action::Rewrite
                && rule.prefix.is_none()
                && rule.prefix_macos.is_none()
                && rule.prefix_linux.is_none()
            {
                return Err(format!("rewrite rule `{}` has no prefix", rule.name));
            }
        }
        Ok(Self {
            path,
            enabled: file.enabled,
            rules: file.rules,
        })
    }

    /// The policy that applies to `dir`: the nearest policy file at or above
    /// it, not looking past the repository root. A file that does not parse
    /// is logged and ignored, so a typo never blocks every command.
    pub fn find(dir: &Path) -> Option<Self> {
        for ancestor in dir.ancestors() {
            let path = ancestor.join(POLICY_PATH);
            if path.is_file() {
                return match std::fs::read_to_string(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|text| Self::parse(&text, path.clone()))
                {
                    Ok(policy) => Some(policy).filter(|policy| policy.enabled),
                    Err(error) => {
                        crate::dbg_log!("command guard: {} ignored: {error}", path.display());
                        None
                    }
                };
            }
            if ancestor.join(".git").exists() {
                break;
            }
        }
        None
    }

    /// What to do with `command`. `extra_env` is the environment the caller
    /// adds to the command (the exec tool's `env` argument).
    pub fn evaluate(&self, command: &str, extra_env: &[(String, String)], host: &Host) -> Verdict {
        let mut found = Vec::new();
        scan(command, None, 0, &Context::default(), &mut found);
        let mut rewrites: Vec<(usize, String, String)> = Vec::new();
        for invocation in &found {
            for rule in &self.rules {
                if !rule.matches(invocation) || rule.allowed(invocation, extra_env, host) {
                    continue;
                }
                match rule.action {
                    Action::Deny => {
                        return Verdict::Deny {
                            rule: rule.name.clone(),
                            message: self.deny_message(rule),
                        };
                    }
                    Action::Rewrite => {
                        let Some(prefix) = rule.prefix_for(host.os) else {
                            continue;
                        };
                        if rule
                            .skip_if_wrapped
                            .iter()
                            .any(|wrapper| invocation.wrappers.contains(wrapper))
                        {
                            continue;
                        }
                        let tool = prefix.split_whitespace().next().unwrap_or_default();
                        if !(host.has_program)(tool) {
                            continue;
                        }
                        if rewrites.iter().all(|(at, ..)| *at != invocation.offset) {
                            rewrites.push((
                                invocation.offset,
                                prefix.to_owned(),
                                rule.name.clone(),
                            ));
                        }
                    }
                }
            }
        }
        if rewrites.is_empty() {
            return Verdict::Allow;
        }
        // Back to front, so an insertion never moves the ones before it.
        rewrites.sort_by(|left, right| right.0.cmp(&left.0));
        let mut rewritten = command.to_owned();
        let mut rules = Vec::new();
        for (offset, prefix, name) in rewrites {
            rewritten.insert_str(offset, &format!("{prefix} "));
            if !rules.contains(&name) {
                rules.push(name);
            }
        }
        rules.reverse();
        Verdict::Rewrite {
            command: rewritten,
            rules,
        }
    }

    fn deny_message(&self, rule: &Rule) -> String {
        let mut message = format!("Command guard: rule `{}` blocked this command.", rule.name);
        if !rule.message.trim().is_empty() {
            message.push(' ');
            message.push_str(rule.message.trim());
        }
        if let Some(variable) = &rule.allow_env {
            message.push_str(&format!(
                " To run it anyway, set {variable}=1 on the command."
            ));
        }
        message.push_str(&format!(" ({POLICY_PATH})"));
        message
    }
}

impl Rule {
    fn matches(&self, invocation: &Invocation) -> bool {
        if invocation.program != self.program {
            return false;
        }
        if !self.subcommand.is_empty()
            && !invocation
                .subcommand
                .as_ref()
                .is_some_and(|sub| self.subcommand.contains(sub))
        {
            return false;
        }
        self.flags.is_empty()
            || invocation.args.iter().any(|arg| {
                self.flags.iter().any(|flag| {
                    arg == flag
                        || arg
                            .strip_prefix(flag.as_str())
                            .is_some_and(|rest| rest.starts_with('='))
                })
            })
    }

    fn allowed(
        &self,
        invocation: &Invocation,
        extra_env: &[(String, String)],
        host: &Host,
    ) -> bool {
        let Some(variable) = &self.allow_env else {
            return false;
        };
        let truthy = |value: &str| !matches!(value.trim(), "" | "0" | "false");
        invocation
            .env
            .iter()
            .chain(extra_env)
            .any(|(name, value)| name == variable && truthy(value))
            || (host.var)(variable).is_some_and(|value| truthy(&value))
    }

    fn prefix_for(&self, os: &str) -> Option<&str> {
        let specific = match os {
            "macos" => self.prefix_macos.as_deref(),
            "linux" => self.prefix_linux.as_deref(),
            _ => None,
        };
        specific.or(self.prefix.as_deref())
    }
}

/// One program run found in a command.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    program: String,
    /// Arguments after the program.
    args: Vec<String>,
    /// The first non-option argument.
    subcommand: Option<String>,
    /// Assignments in front of it, and ones an `env` wrapper adds.
    env: Vec<(String, String)>,
    /// Wrappers it runs under: `env`, `nice`, `taskpolicy`.
    wrappers: Vec<String>,
    /// Where the command it belongs to starts in the text given to
    /// [`Policy::evaluate`], so a rewrite can put its prefix there.
    offset: usize,
}

#[derive(Default, Clone)]
struct Context {
    env: Vec<(String, String)>,
    wrappers: Vec<String>,
}

struct Word {
    text: String,
    start: usize,
}

fn end_word(current: &mut Option<Word>, words: &mut Vec<Word>) {
    if let Some(word) = current.take() {
        words.push(word);
    }
}

/// The word being built, started at `offset` when there is none yet.
fn open_word(current: &mut Option<Word>, offset: usize) -> &mut Word {
    current.get_or_insert_with(|| Word {
        text: String::new(),
        start: offset,
    })
}

/// Split shell text into its simple commands. This is not a shell parser: it
/// keeps quotes together, ends a command at `;`, `&`, `|`, a newline or a
/// bracket, drops comments, and leaves redirections such as `2>&1` inside the
/// command they belong to.
fn simple_commands(text: &str) -> Vec<Vec<Word>> {
    let mut commands: Vec<Vec<Word>> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut current: Option<Word> = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut comment = false;
    let chars: Vec<(usize, char)> = text.char_indices().collect();

    for (position, &(offset, ch)) in chars.iter().enumerate() {
        if comment {
            if ch == '\n' {
                comment = false;
                end_word(&mut current, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            continue;
        }
        if escaped {
            escaped = false;
            open_word(&mut current, offset).text.push(ch);
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else if ch == '\\' && open == '"' {
                escaped = true;
            } else {
                open_word(&mut current, offset).text.push(ch);
            }
            continue;
        }
        let previous = position.checked_sub(1).map(|index| chars[index].1);
        let next = chars.get(position + 1).map(|(_, ch)| *ch);
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                open_word(&mut current, offset);
            }
            '\\' => {
                escaped = true;
                open_word(&mut current, offset);
            }
            '#' if current.is_none() => comment = true,
            // `2>&1`, `>&2` and `&>file` are redirections, not a new command.
            '&' if matches!(previous, Some('>' | '<')) || next == Some('>') => {
                open_word(&mut current, offset).text.push(ch);
            }
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' => {
                end_word(&mut current, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            ch if ch.is_whitespace() => end_word(&mut current, &mut words),
            ch => open_word(&mut current, offset).text.push(ch),
        }
    }
    end_word(&mut current, &mut words);
    commands.push(words);
    commands.retain(|command| !command.is_empty());
    commands
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn file_name(program: &str) -> String {
    let name = program.rsplit('/').next().unwrap_or(program);
    name.strip_suffix(".exe").unwrap_or(name).to_owned()
}

/// Shell keywords that may open a command: `do cargo test`, `! git stash`.
const COMMAND_OPENERS: &[&str] = &[
    "{", "}", "!", "if", "then", "else", "elif", "do", "while", "until",
];

/// How many of `words` an option of a wrapper takes, given the options that
/// take a value.
fn skip_options(words: &[String], mut index: usize, with_value: &[&str]) -> usize {
    while let Some(word) = words.get(index) {
        if word == "--" {
            return index + 1;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        index += if with_value.contains(&word.as_str()) {
            2
        } else {
            1
        };
    }
    index
}

/// Peel the wrappers and assignments off a command: the index of the program
/// that actually runs, with what they set and which wrappers they were.
fn peel(words: &[String], context: &mut Context) -> usize {
    let mut index = 0;
    loop {
        while let Some(word) = words.get(index) {
            if !is_assignment(word) {
                break;
            }
            if let Some((name, value)) = word.split_once('=') {
                context.env.push((name.to_owned(), value.to_owned()));
            }
            index += 1;
        }
        let Some(word) = words.get(index) else {
            return index;
        };
        let name = file_name(word);
        let wrapper = match name.as_str() {
            "env" => {
                index = skip_options(words, index + 1, &["-u", "--unset", "-C", "--chdir", "-S"]);
                true
            }
            "command" | "exec" | "builtin" | "nohup" | "setsid" | "time" => {
                index = skip_options(words, index + 1, &[]);
                true
            }
            "nice" => {
                index = skip_options(words, index + 1, &["-n", "--adjustment"]);
                true
            }
            "ionice" => {
                index = skip_options(words, index + 1, &["-c", "-n", "-p", "-P", "-u"]);
                true
            }
            "taskpolicy" => {
                index = skip_options(words, index + 1, &["-d", "-g", "-c", "-p", "-a"]);
                true
            }
            "caffeinate" => {
                index = skip_options(words, index + 1, &["-t", "-w"]);
                true
            }
            "sudo" => {
                index = skip_options(
                    words,
                    index + 1,
                    &["-u", "-g", "-h", "-p", "-C", "-r", "-t", "-T", "-U"],
                );
                true
            }
            "timeout" => {
                index = skip_options(words, index + 1, &["-s", "--signal", "-k", "--kill-after"]);
                // The duration.
                index += 1;
                true
            }
            _ => false,
        };
        if !wrapper {
            return index;
        }
        context.wrappers.push(name);
    }
}

/// Git's and cargo's options that come before the subcommand and take a value.
fn subcommand_options(program: &str) -> &'static [&'static str] {
    match program {
        "git" => &[
            "-C",
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--exec-path",
        ],
        "cargo" => &["-Z", "--config", "--color", "--manifest-path"],
        _ => &[],
    }
}

fn subcommand_of(program: &str, args: &[String]) -> Option<String> {
    let value_options = subcommand_options(program);
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if value_options.contains(&arg.as_str()) {
            index += 2;
        } else if arg.starts_with('-') || (program == "cargo" && index == 0 && arg.starts_with('+'))
        {
            index += 1;
        } else {
            return Some(arg.clone());
        }
    }
    None
}

/// The script of an `sh -c '…'`, `bash -lc '…'` style command.
fn shell_script(program: &str, args: &[String]) -> Option<String> {
    if !matches!(program, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish") {
        return None;
    }
    let position = args.iter().position(|arg| {
        arg.len() > 1
            && arg.starts_with('-')
            && !arg.starts_with("--")
            && arg[1..].chars().all(|ch| ch.is_ascii_alphabetic())
            && arg.contains('c')
    })?;
    args.get(position + 1).cloned()
}

fn scan(
    text: &str,
    outer_offset: Option<usize>,
    depth: usize,
    inherited: &Context,
    found: &mut Vec<Invocation>,
) {
    for command in simple_commands(text) {
        let mut start = 0;
        while command
            .get(start)
            .is_some_and(|word| COMMAND_OPENERS.contains(&word.text.as_str()))
        {
            start += 1;
        }
        let Some(first) = command.get(start) else {
            continue;
        };
        let offset = outer_offset.unwrap_or(first.start);
        let words: Vec<String> = command[start..]
            .iter()
            .map(|word| word.text.clone())
            .collect();
        let mut context = inherited.clone();
        let index = peel(&words, &mut context);
        let Some(program_word) = words.get(index) else {
            continue;
        };
        let program = file_name(program_word);
        let args = words[index + 1..].to_vec();
        if depth < MAX_NESTING
            && let Some(script) = shell_script(&program, &args)
        {
            scan(&script, Some(offset), depth + 1, &context, found);
        }
        found.push(Invocation {
            subcommand: subcommand_of(&program, &args),
            program,
            args,
            env: context.env,
            wrappers: context.wrappers,
            offset,
        });
    }
}

/// The verdict of the policy at or above `dir` for `command`, if the project
/// has one.
pub fn check(dir: &Path, command: &str, extra_env: &[(String, String)]) -> Verdict {
    match Policy::find(dir) {
        Some(policy) => policy.evaluate(command, extra_env, &Host::current()),
        None => Verdict::Allow,
    }
}

/// Whether a project at `dir` has a policy the guard applies.
pub fn has_policy(dir: &Path) -> bool {
    Policy::find(dir).is_some()
}

/// What an agent is told when a command is refused. A rewrite is refused with
/// the command to run instead, for a path that cannot change the command.
pub fn refusal(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::Allow => None,
        Verdict::Deny { message, .. } => Some(message.clone()),
        Verdict::Rewrite { command, rules } => Some(format!(
            "Command guard: rule `{}` requires this command to run as: {command} ({POLICY_PATH})",
            rules.join("`, `")
        )),
    }
}

/// Count a violation against a thread: one `command_guard.violation` event
/// with the rule, what the guard did (`deny` or `rewrite`) and which harness
/// ran the command. No command text is stored.
pub fn record_violation(thread_id: &str, harness: &str, verdict: &Verdict) {
    let (rules, action): (Vec<&str>, &str) = match verdict {
        Verdict::Allow => return,
        Verdict::Deny { rule, .. } => (vec![rule.as_str()], "deny"),
        Verdict::Rewrite { rules, .. } => (rules.iter().map(String::as_str).collect(), "rewrite"),
    };
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return;
    }
    for rule in rules {
        let fields = json!({"rule": rule, "action": action, "harness": harness});
        let row = crate::data_store::OperationalEventRow {
            thread_id: thread_id.to_owned(),
            turn_id: None,
            event_type: VIOLATION_EVENT.to_owned(),
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            fields_json: Some(fields.to_string()),
            original_bytes: None,
            captured_bytes: None,
            payload_truncated: false,
        };
        crate::data_store::record_operational_event(&row);
    }
}

/// Quote one word for a POSIX shell.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-_./=:@%+,".contains(ch))
    {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The command an approval request asks to run, when it is a shell command:
/// the `command` of its raw input, a string or an argv array.
pub fn requested_command(raw_input: &Value) -> Option<String> {
    match raw_input.get("command")? {
        Value::String(command) => Some(command.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .map(shell_quote)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

/// The Claude Code settings that run [`hook_response`] before each Bash call
/// of thread `thread_id`, as an inline SDK settings object.
/// The ACP adapter treats string settings as file paths, not serialized JSON.
/// `None` where the hook cannot run (not the `rustcode` binary, not Unix).
pub fn claude_code_settings(thread_id: Option<&str>) -> Option<Value> {
    if !cfg!(unix) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    if exe.file_stem().and_then(|stem| stem.to_str()) != Some("rustcode") {
        return None;
    }
    let mut command = String::new();
    if let Some(thread_id) = thread_id.filter(|id| !id.is_empty()) {
        command.push_str(&format!("{THREAD_ENV}={} ", shell_quote(thread_id)));
    }
    command.push_str(&format!(
        "{} guard-command",
        shell_quote(&exe.to_string_lossy())
    ));
    Some(claude_hook_settings(command))
}

fn claude_hook_settings(command: String) -> Value {
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": command}],
            }]
        }
    })
}

/// Answer a Claude Code `PreToolUse` hook call: the JSON to print when the
/// command is refused, nothing when it may run. Input that is not a Bash call
/// or does not parse is let through.
pub fn hook_response(input: &str) -> Option<String> {
    let call: Value = serde_json::from_str(input).ok()?;
    if call.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return None;
    }
    let command = call.get("tool_input")?.get("command")?.as_str()?;
    let dir = call
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    let verdict = check(&dir, command, &[]);
    let reason = refusal(&verdict)?;
    if let Ok(thread_id) = std::env::var(THREAD_ENV) {
        record_violation(&thread_id, "claude-code", &verdict);
    }
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        })
        .to_string(),
    )
}

#[cfg(test)]
#[path = "command_guard_tests.rs"]
mod tests;

#[cfg(test)]
mod settings_regression {
    #[test]
    fn inline_hook_settings_are_an_object_not_a_file_path() {
        let settings = serde_json::json!(super::claude_hook_settings(
            "rustcode guard-command".to_owned()
        ));
        assert!(settings.is_object(), "SDK string settings are file paths");
        assert_eq!(settings["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "rustcode guard-command"
        );
    }
}
