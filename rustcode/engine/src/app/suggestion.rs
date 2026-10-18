#[derive(Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub desc: &'static str,
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "/cancel",
        desc: "Cancel active stream or queued prompt",
    },
    CommandInfo {
        name: "/changelog",
        desc: "Show recent changelog updates",
    },
    CommandInfo {
        name: "/change_title",
        desc: "Rename the current session title",
    },
    CommandInfo {
        name: "/clear",
        desc: "Clear conversation history",
    },
    CommandInfo {
        name: "/context",
        desc: "Show context usage or set context window",
    },
    CommandInfo {
        name: "/copy",
        desc: "Copy last assistant reply to clipboard",
    },
    CommandInfo {
        name: "/continue",
        desc: "Continue restored pending session work",
    },
    CommandInfo {
        name: "/diff",
        desc: "Show staged and unstaged Git changes",
    },
    CommandInfo {
        name: "/discord",
        desc: "Toggle Discord Rich Presence, or use on|off|status",
    },
    CommandInfo {
        name: "/exit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/goal",
        desc: "Run a task in continuous autoloop mode until complete_task is called",
    },
    CommandInfo {
        name: "/about",
        desc: "Show version and basic info about rustcode",
    },
    CommandInfo {
        name: "/info",
        desc: "Show version and basic info about rustcode",
    },
    CommandInfo {
        name: "/help",
        desc: "Show help info",
    },
    CommandInfo {
        name: "/history",
        desc: "Pick a previous session to resume",
    },
    CommandInfo {
        name: "/memory",
        desc: "Inspect or update bounded project memory (no option shows RAM)",
    },
    CommandInfo {
        name: "/mcp",
        desc: "Configure Model Context Protocol (MCP) servers",
    },
    CommandInfo {
        name: "/model",
        desc: "Open model picker, switch profile, or override model",
    },
    CommandInfo {
        name: "/login",
        desc: "Sign in or configure a provider account",
    },
    CommandInfo {
        name: "/auth",
        desc: "Show provider authentication status",
    },
    CommandInfo {
        name: "/logout",
        desc: "Sign out of a provider account",
    },
    CommandInfo {
        name: "/accounts",
        desc: "List saved provider accounts",
    },
    CommandInfo {
        name: "/account",
        desc: "Show provider account status",
    },
    CommandInfo {
        name: "/refresh",
        desc: "Reload the provider account's model catalog",
    },
    CommandInfo {
        name: "/models",
        desc: "Open the model picker",
    },
    CommandInfo {
        name: "/new",
        desc: "Start a new conversation",
    },
    CommandInfo {
        name: "/fork",
        desc: "Fork the current conversation into a new session",
    },
    CommandInfo {
        name: "/scroll-speed",
        desc: "Choose how many lines a mouse wheel tick scrolls",
    },
    CommandInfo {
        name: "/export",
        desc: "Export this session as a zip into Downloads",
    },
    CommandInfo {
        name: "/archive",
        desc: "Persist the current session",
    },
    CommandInfo {
        name: "/agents",
        desc: "Browse subagent conversation contexts",
    },
    CommandInfo {
        name: "/delete_chat",
        desc: "Delete current session and start fresh",
    },
    CommandInfo {
        name: "/delegate",
        desc: "Allow subagents for the next task (on/off: whole session)",
    },
    CommandInfo {
        name: "/workspace",
        desc: "Manage an isolated task workspace",
    },
    CommandInfo {
        name: "/ollama",
        desc: "Configure or list Ollama models",
    },
    CommandInfo {
        name: "/parser",
        desc: "Show or set current tool protocol (json only)",
    },
    CommandInfo {
        name: "/prompt",
        desc: "Load a prompt template into the composer for review",
    },
    CommandInfo {
        name: "/prompts",
        desc: "List available prompt templates",
    },
    CommandInfo {
        name: "/provider",
        desc: "Add/update model provider profile",
    },
    CommandInfo {
        name: "/pwd",
        desc: "Show the effective workspace path",
    },
    CommandInfo {
        name: "/protocol",
        desc: "Show or set current tool protocol (json only)",
    },
    CommandInfo {
        name: "/ps",
        desc: "Alias for /tasks",
    },
    CommandInfo {
        name: "/quit",
        desc: "Exit the app",
    },
    CommandInfo {
        name: "/quota",
        desc: "Show model quota percentages",
    },
    CommandInfo {
        name: "/resume",
        desc: "Resume most recent session",
    },
    CommandInfo {
        name: "/session",
        desc: "Show current session ID, token budget, and active model",
    },
    CommandInfo {
        name: "/remote",
        desc: "Turn remote access on: every session is shared with paired devices (pair|status|off)",
    },
    CommandInfo {
        name: "/skills",
        desc: "Show available skills and their locations",
    },
    CommandInfo {
        name: "/stats",
        desc: "Show token usage and context statistics",
    },
    CommandInfo {
        name: "/stop",
        desc: "Stop all running tasks",
    },
    CommandInfo {
        name: "/tasks",
        desc: "Show running and finished tasks",
    },
    CommandInfo {
        name: "/perf",
        desc: "Inspect latest turn performance telemetry",
    },
    CommandInfo {
        name: "/status",
        desc: "Show token usage and context statistics",
    },
    CommandInfo {
        name: "/test",
        desc: "Preview a fixed TUI demo (Tab question, Esc close)",
    },
    CommandInfo {
        name: "/compact",
        desc: "Manually compact session history to save context space",
    },
    CommandInfo {
        name: "/recap",
        desc: "Generate a brief conversation catch-up and next step",
    },
    CommandInfo {
        name: "/summarize",
        desc: "Summarize the current chat session now",
    },
    CommandInfo {
        name: "/sync",
        desc: "Sync config, skills, themes, and prompt commands with Git",
    },
    CommandInfo {
        name: "/update",
        desc: "Check the Homebrew tap and upgrade rustcode if a newer version exists",
    },
    CommandInfo {
        name: "/tools",
        desc: "List available tools",
    },
    CommandInfo {
        name: "/usage",
        desc: "Show token usage and context stats",
    },
    CommandInfo {
        name: "/verbosity",
        desc: "Show or set verbosity (low|high|edits|toggle)",
    },
    CommandInfo {
        name: "/yolo",
        desc: "Show or set automatic tool confirmation (on|off|toggle)",
    },
    CommandInfo {
        name: "/sandbox",
        desc: "Show or set OS sandbox mode (read_only|workspace_write|workspace_write_network|trusted)",
    },
    CommandInfo {
        name: "/effort",
        desc: "Show or set model reasoning effort (low|medium|high|off)",
    },
    CommandInfo {
        name: "/pi",
        desc: "Toggle the prompt improver (on|off)",
    },
    CommandInfo {
        name: "/theme",
        desc: "Show or set UI color theme (default|light|nord|dracula|tokyo-night)",
    },
    CommandInfo {
        name: "/thinking",
        desc: "Show or set model thinking (on|off|default)",
    },
];

/// Return the slash command token from the first input line. Arguments remain
/// outside the token so the popup can continue to describe `/model foo` (and
/// similar commands) without replacing or discarding the user's arguments.
pub fn command_token(input: &str) -> Option<&str> {
    let first_line = input.lines().next().unwrap_or("");
    if !first_line.starts_with('/') {
        return None;
    }
    let end = first_line
        .char_indices()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(first_line.len());
    Some(&first_line[..end])
}

/// Commands matching the token in the composer popup, best match first.
///
/// The tiers are the search rule of `app::fuzzy`: an exact name, then
/// prefixes in alphabetical order, then anything the fuzzy matcher accepts. The fuzzy tier is what
/// makes `/modle` find `/model` instead of matching nothing (#1588).
pub fn filtered_commands(input: &str) -> Vec<&'static CommandInfo> {
    let Some(token) = command_token(input) else {
        return Vec::new();
    };
    let token = token.to_lowercase();
    let mut exact = Vec::new();
    let mut prefixes = Vec::new();
    let mut fuzzy = Vec::new();
    for command in COMMANDS {
        let name = command.name.to_lowercase();
        if name == token {
            exact.push(command);
        } else if name.starts_with(&token) {
            prefixes.push(command);
        } else if crate::app::fuzzy::fuzzy_matches(command.name, &token) {
            fuzzy.push(command);
        }
    }
    // A typed prefix says nothing about which command is meant, so that tier
    // reads alphabetically, the order a reader scans a completion list in. A
    // bare `/` keeps the curated order of the full menu.
    if token.len() > 1 {
        prefixes.sort_by_key(|command| command.name);
    }
    exact.into_iter().chain(prefixes).chain(fuzzy).collect()
}

/// One value offered for a slash command's first argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSuggestion {
    pub value: String,
    pub desc: String,
}

/// Argument values only the running session knows. Everything else comes from
/// [`STATIC_ARGUMENTS`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ArgumentSources<'a> {
    /// Configured model profiles, offered to `/model`.
    pub models: &'a [crate::config::ModelProfile],
    /// Theme names supplied by the terminal frontend, offered to `/theme`.
    pub themes: &'a [String],
}

/// Commands whose first argument is one of a closed set of words the command
/// handler accepts. A command that is not listed here (or in
/// [`argument_values`]) takes free-form arguments and completes only its name.
const STATIC_ARGUMENTS: &[(&str, &[(&str, &str)])] = &[
    (
        "/delegate",
        &[
            ("on", "Allow subagents for the whole session"),
            ("off", "Disable subagents"),
        ],
    ),
    (
        "/discord",
        &[
            ("on", "Enable Discord Rich Presence"),
            ("off", "Disable Discord Rich Presence"),
            ("status", "Show the Rich Presence setting and socket"),
        ],
    ),
    (
        "/effort",
        &[
            ("low", "Low reasoning effort"),
            ("medium", "Medium reasoning effort"),
            ("high", "High reasoning effort"),
            ("off", "Clear the reasoning effort"),
        ],
    ),
    (
        "/pi",
        &[
            ("on", "Enable the prompt improver"),
            ("off", "Disable the prompt improver"),
        ],
    ),
    (
        "/remote",
        &[
            ("status", "Show whether this session is shared"),
            ("off", "Stop sharing this session"),
        ],
    ),
    (
        "/thinking",
        &[
            ("on", "Enable model thinking"),
            ("off", "Disable model thinking"),
            ("default", "Use the model's default"),
        ],
    ),
    (
        "/verbosity",
        &[
            ("low", "Compact output"),
            ("high", "Tool calls only"),
            ("edits", "Tool calls only, file edits open"),
            ("toggle", "Cycle low, high, and edits"),
        ],
    ),
    (
        "/yolo",
        &[
            ("on", "Confirm tools automatically"),
            ("off", "Ask before running tools"),
            ("toggle", "Switch the current setting"),
        ],
    ),
];

/// Every value the first argument of `command` can complete to, or `None` for
/// a command that completes only its name.
fn argument_values(command: &str, sources: &ArgumentSources) -> Option<Vec<ArgumentSuggestion>> {
    let suggestion = |value: &str, desc: &str| ArgumentSuggestion {
        value: value.to_owned(),
        desc: desc.to_owned(),
    };
    if let Some((_, values)) = STATIC_ARGUMENTS.iter().find(|(name, _)| *name == command) {
        return Some(
            values
                .iter()
                .map(|(value, desc)| suggestion(value, desc))
                .collect(),
        );
    }
    match command {
        "/sandbox" => Some(
            crate::config::SandboxMode::ALL
                .iter()
                .map(|mode| suggestion(mode.as_str(), mode.description()))
                .collect(),
        ),
        // A profile name with whitespace cannot be passed as one argument, so
        // it stays reachable through the picker only.
        "/model" => Some(
            sources
                .models
                .iter()
                .filter(|profile| !profile.name.is_empty())
                .filter(|profile| !profile.name.contains(char::is_whitespace))
                .map(|profile| suggestion(&profile.name, &profile.model))
                .collect(),
        ),
        "/scroll-speed" => Some(
            [
                ("1", "Precise: one line per wheel tick (default)"),
                ("3", "Three lines per wheel tick"),
                ("5", "Five lines per wheel tick"),
                ("10", "Ten lines per wheel tick"),
            ]
            .iter()
            .map(|(value, desc)| suggestion(value, desc))
            .collect(),
        ),
        "/theme" => Some(
            sources
                .themes
                .iter()
                .filter(|theme| !theme.is_empty() && !theme.contains(char::is_whitespace))
                .map(|theme| suggestion(theme, "Color theme"))
                .collect(),
        ),
        _ => None,
    }
}

/// True when Tab should leave a space after `command` so its argument values
/// are offered next.
pub fn command_completes_arguments(command: &str, sources: &ArgumentSources) -> bool {
    argument_values(command, sources).is_some_and(|values| !values.is_empty())
}

/// The first argument of a slash command, while the cursor is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentQuery<'a> {
    /// Canonical name of the command the argument belongs to.
    pub command: &'static str,
    /// Byte offset where the argument starts; it runs to the end of the input.
    pub start: usize,
    pub query: &'a str,
}

/// Locate the argument being typed. Only the first argument of a single-line
/// draft completes, and only once the command itself is spelled out: a typo in
/// the command is the command popup's to fix first.
pub fn argument_query(input: &str, cursor: usize) -> Option<ArgumentQuery<'_>> {
    if input.contains('\n') {
        return None;
    }
    let token = command_token(input)?;
    let command = COMMANDS
        .iter()
        .find(|command| command.name.eq_ignore_ascii_case(token))?
        .name;
    let rest = &input[token.len()..];
    let query = rest.trim_start();
    if query.len() == rest.len() || query.contains(char::is_whitespace) {
        return None;
    }
    let start = input.len() - query.len();
    (cursor >= start).then_some(ArgumentQuery {
        command,
        start,
        query,
    })
}

/// What the composer popup offers for a slash-command draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCompletion<'a> {
    /// Command names matching the typed token.
    Commands {
        token: &'a str,
        commands: Vec<&'static CommandInfo>,
    },
    /// Values for the command's first argument.
    Arguments {
        query: ArgumentQuery<'a>,
        values: Vec<ArgumentSuggestion>,
    },
}

/// Resolve the slash completion for a draft. The rules, in order:
///
/// - Only a single-line draft completes. In a multi-line draft the arrow keys
///   belong to the cursor, so a popup there would trap them.
/// - With the cursor in the first argument of a command that has a closed
///   value set, the matching values are offered: exact, then prefixes in
///   declared order, then fuzzy matches.
/// - Otherwise the command names matching the token are offered, which also
///   keeps describing a command while free-form arguments are typed.
///
/// `None` means there is nothing to show, so no popup state exists to dismiss.
pub fn slash_completion<'a>(
    input: &'a str,
    cursor: usize,
    sources: &ArgumentSources,
) -> Option<SlashCompletion<'a>> {
    if input.contains('\n') {
        return None;
    }
    let token = command_token(input)?;
    if let Some(query) = argument_query(input, cursor)
        && let Some(values) = argument_values(query.command, sources)
    {
        let needle = query.query.to_lowercase();
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        let mut fuzzy = Vec::new();
        for value in values {
            let name = value.value.to_lowercase();
            if name == needle {
                exact.push(value);
            } else if name.starts_with(&needle) {
                prefixes.push(value);
            } else if crate::app::fuzzy::fuzzy_matches(&value.value, &needle) {
                fuzzy.push(value);
            }
        }
        let values: Vec<_> = exact.into_iter().chain(prefixes).chain(fuzzy).collect();
        if !values.is_empty() {
            return Some(SlashCompletion::Arguments { query, values });
        }
    }
    let commands = filtered_commands(input);
    (!commands.is_empty()).then_some(SlashCompletion::Commands { token, commands })
}

/// The completion popup as a frontend draws it; see [`completion_popup`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CompletionPopup {
    #[default]
    None,
    Commands(Vec<&'static CommandInfo>),
    Arguments {
        /// The typed part of the argument, for marking matched characters.
        query: String,
        values: Vec<ArgumentSuggestion>,
    },
    /// An `@` file query. Listing the files stays with the frontend.
    Files {
        query: String,
    },
}

/// The `@` file query under the cursor, when it is the active completion.
/// It wins over slash completion once the cursor has left the command name,
/// so a file can be attached to a command's free-form arguments.
pub fn file_completion_query(input: &str, cursor: usize) -> Option<(usize, String)> {
    let (start, query) = rustcode_core::input::get_at_word_query(input, cursor)?;
    let command_end = command_token(input).map_or(0, |token| token.len());
    (start >= command_end).then_some((start, query))
}

/// Names the completion under the cursor. Two drafts with the same identity
/// offer the same rows, so a dismissal or a selection made in one holds for
/// the other.
pub fn completion_identity(
    input: &str,
    cursor: usize,
    sources: &ArgumentSources,
) -> Option<String> {
    if let Some((start, query)) = file_completion_query(input, cursor) {
        return Some(format!("file:{start}:{query}"));
    }
    match slash_completion(input, cursor, sources)? {
        SlashCompletion::Commands { token, .. } => Some(format!("command:{token}")),
        SlashCompletion::Arguments { query, .. } => {
            Some(format!("argument:{}:{}", query.command, query.query))
        }
    }
}

/// What the completion popup shows for a draft; nothing once `dismissed`
/// names the completion under the cursor.
pub fn completion_popup(
    input: &str,
    cursor: usize,
    dismissed: Option<&str>,
    sources: &ArgumentSources,
) -> CompletionPopup {
    let identity = completion_identity(input, cursor, sources);
    if identity.is_none() || identity.as_deref() == dismissed {
        return CompletionPopup::None;
    }
    if let Some((_, query)) = file_completion_query(input, cursor) {
        return CompletionPopup::Files { query };
    }
    match slash_completion(input, cursor, sources) {
        Some(SlashCompletion::Commands { commands, .. }) => CompletionPopup::Commands(commands),
        Some(SlashCompletion::Arguments { query, values }) => CompletionPopup::Arguments {
            query: query.query.to_owned(),
            values,
        },
        None => CompletionPopup::None,
    }
}

/// True when `candidate` only fuzzily matches `typed`, i.e. accepting it
/// rewrites what the user typed instead of extending it.
pub fn is_fuzzy_hit(candidate: &str, typed: &str) -> bool {
    !candidate.to_lowercase().starts_with(&typed.to_lowercase())
}

fn matching_command_names(prefix: &str) -> Vec<&'static str> {
    let commands = filtered_commands(prefix);
    // Once the typed token names a command, Tab keeps that command selected.
    // A longer alias may still appear in the popup, but it must not silently
    // replace a valid command when cycling or dismissing completion.
    if let Some(exact) = commands
        .iter()
        .find(|command| command.name.eq_ignore_ascii_case(prefix))
    {
        return vec![exact.name];
    }
    commands.iter().map(|command| command.name).collect()
}

#[derive(Debug, Default)]
pub struct SuggestionCycle {
    pub original_prefix: Option<String>,
    pub suggestion_index: Option<usize>,
}

impl SuggestionCycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cycle the match list forward, updating internal state. Returns true if advanced.
    pub fn cycle(&mut self, input_buffer: &str) -> bool {
        let Some(command) = command_token(input_buffer) else {
            return false;
        };

        let prefix = if let Some(ref p) = self.original_prefix {
            p.clone()
        } else {
            let p = command.to_owned();
            self.original_prefix = Some(p.clone());
            p
        };

        let matches = matching_command_names(&prefix);
        if matches.is_empty() {
            return false;
        }

        let next_idx = match self.suggestion_index {
            Some(idx) => (idx + 1) % matches.len(),
            None => 0,
        };

        self.suggestion_index = Some(next_idx);
        true
    }

    /// Returns the suffix to render as a completion hint (text after `input_buffer`).
    pub fn get_completion_suffix(&self, input_buffer: &str) -> Option<String> {
        let command = command_token(input_buffer)?;

        let prefix = self.original_prefix.as_deref().unwrap_or(command);
        let matches = matching_command_names(prefix);
        if matches.is_empty() {
            return None;
        }

        let idx = self.suggestion_index?;
        Some(matches[idx].get(command.len()..).unwrap_or("").to_string())
    }

    /// Reset the cycle state (called on any keypress other than Tab).
    pub fn reset(&mut self) {
        self.original_prefix = None;
        self.suggestion_index = None;
    }
}

/// How long one `@` listing is reused. The popup asks for the same query on
/// every frame it is open, and each answer is a directory walk; a second is
/// short enough that a file created meanwhile still shows up promptly.
const FILE_LISTING_TTL: std::time::Duration = std::time::Duration::from_secs(1);

type FileListing = (std::path::PathBuf, String, std::time::Instant, Vec<String>);

pub fn list_project_file_paths(query: &str) -> Vec<String> {
    static LAST: std::sync::Mutex<Option<FileListing>> = std::sync::Mutex::new(None);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_cwd, cached_query, at, files)) = last.as_ref()
        && *cached_cwd == cwd
        && cached_query == query
        && at.elapsed() < FILE_LISTING_TTL
    {
        return files.clone();
    }
    let files = walk_project_file_paths(&cwd, query);
    *last = Some((
        cwd,
        query.to_owned(),
        std::time::Instant::now(),
        files.clone(),
    ));
    files
}

fn walk_project_file_paths(cwd: &std::path::Path, query: &str) -> Vec<String> {
    let mut files = Vec::new();
    let query_lower = query.to_lowercase();

    let walker = ignore::WalkBuilder::new(cwd)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(6))
        .build();

    for result in walker {
        if let Ok(entry) = result
            && entry.file_type().is_some_and(|ft| ft.is_file())
            && let Ok(rel) = entry.path().strip_prefix(cwd)
        {
            let rel_str = rel.to_string_lossy().to_string();
            if query_lower.is_empty() || rel_str.to_lowercase().contains(&query_lower) {
                files.push(format!("@{}", rel_str));
                if files.len() >= 25 {
                    break;
                }
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::{
        ArgumentSources, SlashCompletion, argument_query, command_token, filtered_commands,
        slash_completion,
    };

    /// The composer's `/sandbox` hint is the first thing a user reads when
    /// searching for the mode they need. Omitting `trusted` here hid the only
    /// unrestricted mode from the palette while `/sandbox trusted` worked
    /// (#1540).
    #[test]
    fn project_file_walk_filters_by_query_case_insensitively() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/Main.rs"), "").unwrap();
        std::fs::write(root.path().join("README.md"), "").unwrap();

        let mut all = super::walk_project_file_paths(root.path(), "");
        all.sort();
        assert_eq!(all, ["@README.md", "@src/Main.rs"]);
        assert_eq!(
            super::walk_project_file_paths(root.path(), "main"),
            ["@src/Main.rs"]
        );
        assert!(super::walk_project_file_paths(root.path(), "absent").is_empty());
    }

    #[test]
    fn sandbox_completion_hint_names_every_mode() {
        let command = filtered_commands("/sandbox")
            .into_iter()
            .find(|command| command.name == "/sandbox")
            .expect("/sandbox is a slash command");
        for mode in crate::config::SandboxMode::ALL {
            assert!(
                command.desc.contains(mode.as_str()),
                "/sandbox hint must offer {}: {}",
                mode.as_str(),
                command.desc
            );
        }
    }

    #[test]
    fn command_completion_stays_active_while_arguments_are_present() {
        assert_eq!(command_token("/model --fast"), Some("/model"));
        assert_eq!(command_token("/mo --fast"), Some("/mo"));
        assert!(
            filtered_commands("/model --fast")
                .iter()
                .any(|command| command.name == "/model")
        );
        assert!(command_token("plain text").is_none());
    }

    #[test]
    fn workspace_inspection_commands_are_discoverable() {
        for (input, expected) in [("/dif", "/diff"), ("/pw", "/pwd")] {
            assert!(
                filtered_commands(input)
                    .iter()
                    .any(|command| command.name == expected),
                "{expected} should be suggested for {input}"
            );
        }
    }

    #[test]
    fn discord_presence_command_is_discoverable() {
        let command = filtered_commands("/dis")
            .into_iter()
            .find(|command| command.name == "/discord")
            .expect("/discord should be suggested");
        assert!(command.desc.contains("status"));
    }

    #[test]
    fn prompt_template_commands_are_discoverable_and_document_argument_entry() {
        let matches = filtered_commands("/prom");
        assert_eq!(
            matches
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>(),
            ["/prompt", "/prompts"]
        );
        assert!(matches[0].desc.contains("composer for review"));
        assert!(matches[1].desc.contains("List available prompt templates"));
    }

    #[test]
    fn provider_auth_commands_are_discoverable() {
        for input in ["/log", "/au", "/logo", "/acc"] {
            let matches = filtered_commands(input);
            assert!(matches.iter().any(|command| {
                matches!(
                    command.name,
                    "/login" | "/auth" | "/logout" | "/accounts" | "/account"
                )
            }));
        }
    }

    #[test]
    fn command_completion_matches_case_insensitively() {
        let commands = filtered_commands("/MODEL");

        assert!(commands.iter().any(|command| command.name == "/model"));
    }

    /// A misspelled token used to match nothing, so a command nobody spelled
    /// exactly was invisible in the popup (#1588).
    #[test]
    fn command_completion_falls_back_to_fuzzy_matching() {
        for typo in ["/modle", "/mdel", "/compact"] {
            let commands = filtered_commands(typo);
            let expected = match typo {
                "/modle" | "/mdel" => "/model",
                _ => "/compact",
            };
            assert_eq!(
                commands.first().map(|command| command.name),
                Some(expected),
                "{typo} must rank {expected} first: {:?}",
                commands.iter().map(|c| c.name).collect::<Vec<_>>()
            );
        }
    }

    /// Prefix matches still win over fuzzy ones, so the closest command keeps
    /// the selection.
    #[test]
    fn prefix_matches_outrank_fuzzy_matches() {
        let commands = filtered_commands("/mo");

        assert_eq!(
            commands.first().map(|command| command.name),
            Some("/model"),
            "{:?}",
            commands.iter().map(|c| c.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fuzzy_completion_keeps_the_exact_token_winning() {
        let commands = filtered_commands("/models");

        assert_eq!(
            commands.first().map(|command| command.name),
            Some("/models")
        );
    }

    #[test]
    fn suggestion_cycle_matches_case_insensitively() {
        let mut cycle = super::SuggestionCycle::new();

        assert!(cycle.cycle("/MODEL --fast"));
        assert_eq!(
            cycle.get_completion_suffix("/MODEL --fast"),
            Some(String::new())
        );
    }

    #[test]
    fn typed_prefix_matches_are_alphabetical_and_the_bare_menu_keeps_its_order() {
        let names = |input: &str| {
            super::filtered_commands(input)
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
        };
        // Prefix matches come first and in order; fuzzy matches follow them.
        let typed = names("/p");
        let prefixes = typed
            .iter()
            .take_while(|name| name.starts_with("/p"))
            .copied()
            .collect::<Vec<_>>();
        assert!(prefixes.len() > 2, "{typed:?}");
        let mut sorted = prefixes.clone();
        sorted.sort_unstable();
        assert_eq!(prefixes, sorted);
        assert!(
            typed[prefixes.len()..]
                .iter()
                .all(|name| !name.starts_with("/p")),
            "{typed:?}"
        );

        let menu = names("/");
        let declared = super::COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(menu, declared);

        // An exact name still leads its longer siblings.
        assert_eq!(names("/prompt").first(), Some(&"/prompt"));
    }

    fn argument_values(input: &str, sources: &ArgumentSources) -> Option<Vec<String>> {
        match slash_completion(input, input.len(), sources)? {
            SlashCompletion::Arguments { values, .. } => {
                Some(values.into_iter().map(|value| value.value).collect())
            }
            SlashCompletion::Commands { .. } => None,
        }
    }

    #[test]
    fn argument_query_covers_only_the_first_argument_of_a_spelled_out_command() {
        let query = argument_query("/thinking of", 12).expect("first argument");
        assert_eq!(
            (query.command, query.start, query.query),
            ("/thinking", 10, "of")
        );
        // The space after the command opens the argument, before any letter.
        let query = argument_query("/thinking ", 10).expect("empty argument");
        assert_eq!((query.start, query.query), (10, ""));
        // Case and extra spacing do not change which command it is.
        let query = argument_query("/THINKING   on", 14).expect("canonical command");
        assert_eq!(
            (query.command, query.start, query.query),
            ("/thinking", 12, "on")
        );

        // No argument yet, a second argument, a cursor still in the command
        // name, a misspelled command and a multi-line draft are not arguments.
        assert_eq!(argument_query("/thinking", 9), None);
        assert_eq!(argument_query("/thinking on now", 16), None);
        assert_eq!(argument_query("/thinking on ", 13), None);
        assert_eq!(argument_query("/thinking on", 4), None);
        assert_eq!(argument_query("/thinkin on", 11), None);
        assert_eq!(argument_query("/thinking on\nmore", 12), None);
        assert_eq!(argument_query("plain text", 10), None);
    }

    #[test]
    fn argument_values_rank_exact_then_prefix_then_fuzzy() {
        let sources = ArgumentSources::default();
        assert_eq!(
            argument_values("/thinking ", &sources).unwrap(),
            ["on", "off", "default"],
            "an empty argument lists every value in declared order"
        );
        assert_eq!(
            argument_values("/thinking o", &sources).unwrap(),
            ["on", "off"]
        );
        // The prefix leads; `on` is one edit away and follows as a fuzzy row.
        assert_eq!(
            argument_values("/thinking OF", &sources).unwrap(),
            ["off", "on"]
        );
        assert_eq!(
            argument_values("/sandbox trustd", &sources).unwrap(),
            ["trusted"]
        );
        assert_eq!(
            argument_values("/sandbox workspace_write", &sources).unwrap(),
            ["workspace_write", "workspace_write_network"],
            "an exact value leads the longer value it prefixes"
        );
        assert_eq!(
            argument_values("/sandbox ", &sources).unwrap(),
            crate::config::SandboxMode::ALL
                .iter()
                .map(|mode| mode.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn session_values_complete_model_and_theme_arguments() {
        let profile = |name: &str| crate::config::ModelProfile {
            name: name.to_owned(),
            model: format!("{name}-id"),
            ..Default::default()
        };
        let models = [profile("fast"), profile("two words"), profile("smart")];
        let themes = ["nord".to_owned(), "dracula".to_owned()];
        let sources = ArgumentSources {
            models: &models,
            themes: &themes,
        };

        assert_eq!(
            argument_values("/model ", &sources).unwrap(),
            ["fast", "smart"],
            "a name that cannot be typed as one argument is not offered"
        );
        assert_eq!(argument_values("/model sm", &sources).unwrap(), ["smart"]);
        assert_eq!(argument_values("/theme n", &sources).unwrap(), ["nord"]);
        // Without any known value the command row keeps describing itself.
        assert_eq!(
            argument_values("/theme n", &ArgumentSources::default()),
            None
        );
    }

    #[test]
    fn free_form_arguments_keep_the_command_row() {
        let sources = ArgumentSources::default();
        for input in [
            "/context 32000",
            "/change_title New name",
            "/thinking maybe",
        ] {
            match slash_completion(input, input.len(), &sources) {
                Some(SlashCompletion::Commands { token, commands }) => {
                    assert_eq!(commands.first().map(|command| command.name), Some(token));
                }
                other => panic!("{input} should keep its command row: {other:?}"),
            }
        }
    }

    #[test]
    fn nothing_completes_without_a_match_or_in_a_multi_line_draft() {
        let sources = ArgumentSources::default();
        assert_eq!(
            slash_completion("/usr/local/bin is missing", 25, &sources),
            None
        );
        assert_eq!(slash_completion("/model\nsecond line", 6, &sources), None);
        assert_eq!(
            slash_completion("/thinking o\nsecond line", 11, &sources),
            None
        );
        assert_eq!(slash_completion("plain", 5, &sources), None);
    }

    /// Every value offered for an argument must be one its command accepts,
    /// so the table cannot drift from the documented options.
    #[test]
    fn static_argument_values_are_documented_by_their_command() {
        for (command, values) in super::STATIC_ARGUMENTS {
            let info = super::COMMANDS
                .iter()
                .find(|info| info.name == *command)
                .unwrap_or_else(|| panic!("{command} is not a slash command"));
            assert!(!values.is_empty(), "{command}");
            for (value, desc) in *values {
                assert!(!value.contains(char::is_whitespace), "{command} {value}");
                assert!(!desc.is_empty(), "{command} {value}");
            }
            // The popup description names the same options where it lists any.
            if let Some(listed) = info.desc.split('(').nth(1).filter(|l| l.contains('|')) {
                for (value, _) in *values {
                    assert!(listed.contains(value), "{command} hint omits {value}");
                }
            }
        }
    }
}
