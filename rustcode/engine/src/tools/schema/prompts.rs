use super::*;

/// Regression budget for the invariant prompt prefix. Tool schemas are sent
/// separately for ApiNative requests and are intentionally not counted here.
#[cfg(test)]
pub(crate) const BASE_PROMPT_MAX_BYTES: usize = 4_700;
#[cfg(test)]
pub(crate) const BASE_PROMPT_MAX_TOKENS: usize = 1_020;

/// Append the resolved execution policy after the cached, profile-independent
/// prompt so model switches cannot retain another profile's mutation limit.
pub(crate) fn append_tool_response_limit(prompt: &mut String, max_mutating_calls: usize) {
    append_tool_response_policy(
        prompt,
        crate::config::ToolSchedulingPolicy {
            max_mutating_calls,
            ..Default::default()
        },
    );
}

pub(crate) fn append_tool_response_policy(
    prompt: &mut String,
    policy: crate::config::ToolSchedulingPolicy,
) {
    use std::fmt::Write;

    if policy.allow_batching {
        write!(
            prompt,
            "\n\n# Tool response limit\n\
This trusted profile permits a bounded batch of up to {} read-only calls and {} workspace-changing calls in one assistant response. \
Control-plane calls first, reads follow; workspace changes grounded and sequential; never assume an unexecuted call ran. \
Read-only calls do not consume the workspace-changing limit. \
Calls beyond the limit are held by the harness and executed automatically in a later round: never reissue them.\n",
            policy.max_read_only_calls, policy.max_mutating_calls
        )
    } else {
        write!(
            prompt,
            "\n\n# Tool response limit\n\
The effective max_mutating_calls_per_response is {}. \
Batch independent read-only calls freely in one assistant response and wait for their results before choosing the next action. \
Workspace-changing calls (mutating `run_command` calls, file writes/edits and other tools with side effects) run \
one after another in the order given, up to that limit per response; send several together only when a later one \
does not depend on an earlier result. Read-only inspection never consumes this limit. Never assume an unexecuted call ran. \
Calls beyond the limit are held by the harness and executed automatically in a later round: never reissue them.\n",
            policy.max_mutating_calls
        )
    }
    .expect("writing to a String cannot fail");
    // The interface folds each batch of calls into one count line, so the
    // sentence before a batch is what the reader follows the work by.
    prompt.push_str(
        "\n# Progress notes\n\
Before each batch of tool calls, write one short plain sentence saying what you are about to do or what the last results showed. \
One line, no headings or lists, and never one per call. Skip it only when the batch simply continues what the previous note already said.\n",
    );
}

/// How a root agent formats its replies (docs/output-style.md). Every
/// RustCode frontend renders CommonMark with tables and highlighted fences,
/// so this part is sent to all of them; the clause in
/// [`OUTPUT_STYLE_VIZ_HINT`] only to those that draw `rustcode-viz`.
pub(crate) const OUTPUT_STYLE_PROMPT_HINT: &str = "# Output style\n\
Replies render as Markdown. Lead with the answer or result; never restate the question or narrate what you are about to say. \
Keep paragraphs short. Use headings only for long, multi-part answers; bullets for parallel items, numbered steps for sequences, \
a table for comparisons. Fence code with its language and show changes as ```diff; put `path:line` citations in inline code. \
Link issues and pull requests by their full URL (https://github.com/owner/repo/pull/12), never a bare #12. \
No emoji, filler, or recap of what the reply already said. End with a next step only when the user has one to take.\n";

/// Added to [`OUTPUT_STYLE_PROMPT_HINT`] for frontends that draw
/// `rustcode-viz`: without it the reply repeated a chart as a table.
pub(crate) const OUTPUT_STYLE_VIZ_HINT: &str = "A rustcode-viz block replaces a table or list, never repeats one in prose; its summary carries the takeaway.\n";

/// Added to [`OUTPUT_STYLE_PROMPT_HINT`] while Smart UI is on: the view carries the
/// detail, so the text around it stays a line or two (its "a table" means a view's).
pub(crate) const OUTPUT_STYLE_SMART_UI_HINT: &str =
    "Keep prose brief. Use HTML anchors for links in views.\n";

/// The output-style text for a frontend. With Smart UI on the agent is taught
/// views instead of `rustcode-viz`; otherwise it depends on the viz level.
pub(crate) fn output_style_hint(visualization_level: u8, smart_ui: crate::app::SmartUi) -> String {
    let mut hint = String::from(OUTPUT_STYLE_PROMPT_HINT);
    if smart_ui == crate::app::SmartUi::On {
        hint.push_str(OUTPUT_STYLE_SMART_UI_HINT);
    } else if visualization_level > 0 {
        hint.push_str(OUTPUT_STYLE_VIZ_HINT);
    }
    hint.push('\n');
    hint
}

/// The output-style guidance for an external harness that takes developer
/// instructions (Codex `thread/start` `developerInstructions`). It never
/// gets RustCode's root prompt, so it also carries the citation rule from
/// the Workflow section.
pub(crate) fn harness_output_style(smart_ui: crate::app::SmartUi) -> String {
    let views = if smart_ui == crate::app::SmartUi::On {
        OUTPUT_STYLE_SMART_UI_HINT
    } else {
        ""
    };
    format!(
        "{OUTPUT_STYLE_PROMPT_HINT}{views}Cite code as `path:line` with the full repo-relative path.\n"
    )
}

/// What an external harness adds to its own system prompt: where it runs, so it can say so when asked and
/// link what it makes so the app opens it, that RustCode is what owns its threads and projects, then
/// either Smart UI when the user has it on or the `rustcode-viz` types the connected client draws (none
/// at level 0), then `skills` ([`crate::skills::harness_skills_note`], because this harness has no
/// `use_skill`), what this harness does that the model must work around (`harness_note`: what its
/// sandbox blocks on this platform, how it cuts tool output; empty when nothing), then [`harness_output_style`]. Codex takes this as
/// `developerInstructions` and Claude Code's adapter as its `systemPrompt`: it goes in every prompt,
/// so every byte here is paid on every turn.
pub(crate) fn harness_instructions(
    harness: &str,
    visualization_level: u8,
    smart_ui: crate::app::SmartUi,
    skills: Option<&str>,
    harness_note: &str,
) -> String {
    let harness = harness.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "<runtime_info>In case you're asked: you are running in RustCode through the {harness} harness. \
         No need to mention this otherwise. Asked to show something, take a screenshot into the working \
         directory or `$TMPDIR` and embed it with Markdown image syntax and its absolute path, outside any \
         view or fence, one per line. Link any other file you produce by its absolute path so the user can \
         open it from RustCode. RustCode is the app that owns this machine's threads, its projects and its \
         settings: when the `rustcode` tools (or `mcp__rustcode__*`) are listed, they answer what the user \
         asks about them; without them, `rustcode threads list --all` and `rustcode threads path <id>` do \
         the same from a shell. Thread data lives in `~/.config/rustcode/threads`, not in another tool's \
         database. Tokens and reported cost are `thread_usage` (or `rustcode usage`), and what the \
         harnesses recorded of turns and tool calls is `query_data` (or `rustcode data query \"<sql>\"`); \
         its description lists the tables.</runtime_info>\n\n{}{}{}{}{}",
        taught_visualization_hint(visualization_level, smart_ui),
        harness_smart_ui_hint(smart_ui),
        skills.unwrap_or_default(),
        harness_note,
        harness_output_style(smart_ui)
    )
}

/// What Codex's own Windows sandbox denies, for a Codex thread there (#3526). RustCode passes no
/// Windows-specific policy: the sandbox holds its shell in a restricted token either way, and that
/// token cannot reach the WMI service, so the model is told what to read instead of retrying.
pub(crate) const CODEX_WINDOWS_SANDBOX_NOTE: &str = r#"# Sandbox on Windows
This thread's shell runs in Codex's Windows sandbox unless the user picks Full access. WMI answers out of process, so the sandbox denies it and `Get-CimInstance`, `Get-WmiObject`, `wmic` and `systeminfo` fail with `Access denied` (0x80041003); retrying or wrapping it does not help. Read the same facts without WMI: `Get-ItemProperty HKLM:\HARDWARE\DESCRIPTION\System\CentralProcessor\0` and `HKLM:\HARDWARE\DESCRIPTION\System\BIOS` for the CPU and board, `[System.Environment]::ProcessorCount` and `[System.Environment]::OSVersion` for counts and version, `[System.IO.DriveInfo]::GetDrives()` for disks, and `Get-Counter` (performance counters, not WMI) for load and memory. Registry and .NET reads are not restricted.

"#;

/// Antigravity cuts a long tool result down to its head and tail and saves the rest in its own
/// folder (#4098). Models acted on the preview and missed the middle, so it is told to bound what a
/// command prints and to read the saved file when the middle matters.
pub(crate) const ANTIGRAVITY_OUTPUT_NOTE: &str = "# Long command output
This harness cuts a long tool result down to a preview with the middle removed, and saves the whole output to a file it names in the result (\"You can find the full output in a file at …\"). Bound what a command prints so the answer fits: `| head -n 100`, `rg -m 20`, `git diff --stat`, `wc -l` before reading, or a narrower path. When a result says it was truncated and the part you need is in the middle, read that saved file (with a line range or `rg`) instead of acting on the preview.

";

/// [`CODEX_WINDOWS_SANDBOX_NOTE`] on Windows, where Codex sandboxes the shell at all.
pub(crate) fn codex_windows_sandbox_note() -> &'static str {
    if cfg!(windows) {
        CODEX_WINDOWS_SANDBOX_NOTE
    } else {
        ""
    }
}

/// The tag [`runtime_reminder`] wraps [`harness_instructions`] in, and the
/// engine strips from an echo of it (`harness/acp_child.rs`).
pub(crate) const RUNTIME_REMINDER_START: &str = "<rustcode_runtime>";
pub(crate) const RUNTIME_REMINDER_END: &str = "</rustcode_runtime>";

/// [`harness_instructions`] as the opening block of a first prompt, for an
/// agent that has no `sessionCapabilities.systemPrompt` (pi-acp, OpenCode): it
/// still has to hear where it runs, what the client draws and what RustCode
/// owns. Tagged and labelled so the model does not read it as the user's
/// words; the engine drops the agent's echo of it (`strip_runtime_echo`).
pub(crate) fn runtime_reminder(instructions: &str) -> String {
    format!(
        "{RUNTIME_REMINDER_START}\nRustCode's setup instructions for this thread, not the user's message.\n\n{instructions}{RUNTIME_REMINDER_END}"
    )
}

/// The current skills note alone, as the opening block of the first prompt after a harness without a
/// client system prompt loaded a saved session: that session holds the setup text it was first sent,
/// so a skill turned off since is only heard of here. Tagged like [`runtime_reminder`] so the engine
/// drops the agent's echo of it.
pub(crate) fn skills_reminder(note: &str) -> String {
    format!(
        "{RUNTIME_REMINDER_START}\nRustCode's current skills for this thread (replaces the Skills section of its setup instructions), not the user's message.\n\n{note}{RUNTIME_REMINDER_END}"
    )
}

/// Smart UI for a harness that has no root prompt and no `use_skill`: the
/// fence, when to write a view, the kit's names, and what the sandbox blocks.
/// The format itself is `docs/smart-ui.md`.
pub(crate) const SMART_UI_HARNESS_HINT: &str = "\
# Smart UI
Views run in chat: a ```rustcode-ui v1 fence with a title, built from the `::` catalog (the app draws each block; HTML only for what it lacks), no imports or network, max 64 KB. \
Answer visually, asked or not: data, numbers, results, status, comparisons, plans or 4+ items go in a view (chart, bars, stats, table), never a Markdown table. Only short explanations, one fact, code and diffs stay Markdown. Be brief: one line with the finding before the view, nothing after it. Sources go last inside the view, folded: details.rc-sources > ol > li > a[href]. A report to another thread stays plain Markdown.
Pick: a trend → line chart; parts → stacked bar, donut; ranking → bars; options → cards; exact rows → table; a choice → form.
Catalog: `::` opens a block, `- ` items split at |, a blank line ends it; items may end {ok|warn|bad|info}; `code`, **bold** work. A name after a block is its heading.
:: title T · :: head Title | caption · :: h · :: p · :: callout info|ok|warn|bad · :: divider · :: details Summary · :: tab Name … :: end
:: hero Label / - 503 | 3.4x | sentence · :: stats / - 128 | Runs | +12 | 1,4,2 (spark) · :: bars / - label | 75 | 75 s | tip · :: progress / - EU | 72% | note
:: chart line|bar|stacked|hbar|donut Title | size=lg | line=150 normal | band=75-270 | end / x | series… heading row, then rows · :: heatmap, :: compare (yes|no|partial), :: table / a | b rows
:: list / - Title | detail | value · :: kv / - Key | value · :: cards / - Title | body | badge · :: steps / - [x] [>] [!] [ ] · :: timeline / - 10:02 | event | detail
:: findings / - high | file:line | finding | note · :: files / - path | +12 -3 · :: tags · :: term · :: code lang · :: actions / - Label | message · :: sources / - Title | url
:: form Message | Button / - select|choice name | Label | a, *b · - text|number|date name | Label · - check name | Label · - range name | Label | 0-100 = 40
Card and item text: one short line. HTML otherwise: kit classes (.rc-*), no colours, fonts or heights of your own; data-rc-send=\"text\" sends a follow-up. No outer border, background or repeated title; no vh. If `smart_ui_check` is listed, run it first.

";

/// [`SMART_UI_HARNESS_HINT`] when the setting is off, else the off note the
/// root prompt uses; nothing for a client that cannot run the blocks.
pub(crate) fn harness_smart_ui_hint(smart_ui: crate::app::SmartUi) -> &'static str {
    match smart_ui {
        crate::app::SmartUi::On => SMART_UI_HARNESS_HINT,
        crate::app::SmartUi::Off => SMART_UI_OFF_PROMPT_HINT,
        crate::app::SmartUi::Unsupported => "",
    }
}

/// Append the output-style guidance to a root prompt. Subagent replies are
/// read by the parent, not rendered, and compact prompts are for small
/// models where every token counts.
pub(crate) fn append_output_style_hint(
    prompt: &mut String,
    policy: ToolSchemaPolicy,
    visualization_level: u8,
    smart_ui: crate::app::SmartUi,
) {
    if !policy.is_subagent() && !policy.compact_text_prompt {
        prompt.push_str(&output_style_hint(visualization_level, smart_ui));
    }
}

/// Short guidance for `rustcode-viz` blocks; the full format lives in
/// `docs/visualizations.md` and the TUI renderer validates every payload.
pub(crate) const VISUALIZATION_PROMPT_HINT: &str = "# Visualizations\n\
When a chart, table, or tree beats prose (trends, comparisons, breakdowns, hierarchies), add a ```rustcode-viz fence even if not asked with one JSON object: \
{\"version\":1,\"type\":\"bar_chart\",\"title\":\"..\",\"summary\":\"..\",\"items\":[{\"label\":\"..\",\"value\":1}]}; \
\"table\" takes columns, rows (arrays), optional sort_by {column,descending}; \"tree\" takes nodes [{label,note,children}]. Max 30 bars, 50 rows, 120 nodes.\n\n";

/// The widget types added at visualization level 2, appended after
/// [`VISUALIZATION_PROMPT_HINT`] for frontends that draw them. Level 1
/// frontends keep exactly the level 1 prompt.
pub(crate) const VISUALIZATION_PROMPT_HINT_LEVEL_2: &str = "Also: \"line_chart\" takes labels [x] and series \
[{name,values:[number|null]}] for trends (max 4 series, 100 points); \"stat_list\" takes items \
[{label,value,unit,note}] for headline numbers (max 12); \"checklist\" takes items \
[{label,status:done|todo|in_progress|failed|skipped,note}] for progress (max 50).\n\n";

/// The visualization prompt text for a frontend's visualization level: none
/// for 0, the level 1 types for 1, and the level 2 types added from 2.
pub(crate) fn visualization_prompt_hint(level: u8) -> String {
    match level {
        0 => String::new(),
        1 => VISUALIZATION_PROMPT_HINT.to_owned(),
        _ => format!("{VISUALIZATION_PROMPT_HINT}{VISUALIZATION_PROMPT_HINT_LEVEL_2}"),
    }
}

/// What an agent is taught of `rustcode-viz`. With Smart UI on, nothing: one
/// tool per client state, so the rules never conflict. The blocks still render.
pub(crate) fn taught_visualization_hint(level: u8, smart_ui: crate::app::SmartUi) -> String {
    if smart_ui == crate::app::SmartUi::On {
        String::new()
    } else {
        visualization_prompt_hint(level)
    }
}

/// Append the visualization hint when the frontend renders `rustcode-viz`
/// blocks. Only the root agent writes user-facing replies, and compact prompts
/// are for small models where every token counts. Frontends that cannot draw
/// the blocks never get the hint, so they never see the JSON fences, and each
/// frontend is taught only the types its level draws.
pub(crate) fn append_visualization_hint(
    prompt: &mut String,
    policy: ToolSchemaPolicy,
    visualization_level: u8,
    smart_ui: crate::app::SmartUi,
) {
    if !policy.is_subagent() && !policy.compact_text_prompt {
        prompt.push_str(&taught_visualization_hint(visualization_level, smart_ui));
    }
}

/// Guidance for `rustcode-ui v1` blocks (#2484), sent only to frontends that
/// advertise `_meta.rustcode.smartUi`; the format lives in `docs/smart-ui.md`.
pub(crate) const SMART_UI_PROMPT_HINT: &str = "# Smart UI\n\
This app runs small views in replies: a ```rustcode-ui v1 fence of `::` catalog blocks. \
Answer visually, asked or not: data, numbers, results, status, comparisons, plans or 4+ items go in a view (chart, bars, stats, table), never a Markdown table. \
Only short explanations, one fact, code and diffs stay Markdown. Be brief: one line with the finding before the view, nothing after it. Sources go last inside the view, folded: details.rc-sources > ol > li > a[href]. A report to another thread stays plain Markdown. \
First call `use_skill` with `smart-ui` directly (no `list_skills`).\n\n";

/// Sent instead of [`SMART_UI_PROMPT_HINT`] when the frontend could run the
/// blocks but the user has the setting off. Without it an agent asked for
/// "Smart UI" guesses a fence the app shows as source.
pub(crate) const SMART_UI_OFF_PROMPT_HINT: &str = "# Smart UI\n\
This app has Smart UI (interactive HTML components in the chat), but it is turned off. \
If the user asks for Smart UI or an interactive view, tell them to turn on Settings › Chat › Smart UI \
and restart the engine; until then answer with Markdown or rustcode-viz, never an HTML fence.\n\n";

/// The Smart UI hint, if any, for a root prompt.
pub(crate) fn smart_ui_hint(smart_ui: crate::app::SmartUi) -> &'static str {
    match smart_ui {
        crate::app::SmartUi::On => SMART_UI_PROMPT_HINT,
        crate::app::SmartUi::Off => SMART_UI_OFF_PROMPT_HINT,
        crate::app::SmartUi::Unsupported => "",
    }
}

/// Append the Smart UI hint for a root prompt whose frontend runs the blocks.
pub(crate) fn append_smart_ui_hint(
    prompt: &mut String,
    policy: ToolSchemaPolicy,
    smart_ui: crate::app::SmartUi,
) {
    if !policy.is_subagent() && !policy.compact_text_prompt {
        prompt.push_str(smart_ui_hint(smart_ui));
    }
}

/// Append the `localhost_preview` guidance for a request that offers the
/// tool, and the other threads' for one that offers those.
pub(crate) fn append_preview_hint(prompt: &mut String, policy: ToolSchemaPolicy) {
    if policy.include_preview_tool {
        prompt.push_str(crate::preview::PREVIEW_PROMPT_HINT);
    }
    if policy.include_desktop_tool {
        prompt.push_str(crate::desktop_control::PROMPT_HINT);
    }
    if policy.include_smart_ui_preview_tool {
        prompt.push_str(crate::smart_ui::PREVIEW_PROMPT_HINT);
    }
    if policy.include_orchestration_tools {
        prompt.push_str(crate::orchestration::prompt_hint(
            policy.orchestration_version,
        ));
    }
}

/// MCP lines of a textual tool contract, bounded like the native schema
/// selection. Text protocols used to inline every connected MCP tool with its
/// full schema; with a dozen servers that was 18k of a 20k-token prompt for a
/// small model (#2587). Tools past the global or per-server byte budget are
/// listed by name only, and `list_mcp_tools` `select` returns their schemas.
pub(super) fn textual_mcp_listing(
    entries: &[(String, String, String, Value)],
    compact: bool,
    budget_bytes: usize,
) -> String {
    let mut listed = String::new();
    let mut used = 0usize;
    let mut per_server: HashMap<&str, usize> = HashMap::new();
    let mut names_only: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
    for (name, server, desc, schema) in entries {
        let args = serde_json::to_string(schema).unwrap_or_default();
        let line = if compact {
            format!("- {name} | Args: {args}\n")
        } else {
            format!("- {name} | Args: {args} | {desc}\n")
        };
        let server_used = per_server.entry(server.as_str()).or_default();
        if used + line.len() <= budget_bytes
            && *server_used + line.len() <= MAX_MCP_PER_SERVER_SCHEMA_BYTES
        {
            used += line.len();
            *server_used += line.len();
            listed.push_str(&line);
        } else {
            names_only
                .entry(server.as_str())
                .or_default()
                .push(name.as_str());
        }
    }
    if !names_only.is_empty() {
        listed.push_str(
            "- More MCP tools, listed by name without arguments; call `list_mcp_tools` with `select` to get a schema before using one:\n",
        );
        for (server, names) in names_only {
            listed.push_str(&format!("  - {server}: {}\n", names.join(", ")));
        }
    }
    listed
}

pub(crate) fn tool_system_prompt_for_policy(
    policy: ToolSchemaPolicy,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
) -> String {
    // The first-turn-only `set_session_title` tool must not vary the system
    // prompt: a turn-1/turn-2 difference here invalidates the whole cached
    // prefix. Its guidance (and, for text protocols, its call shape) rides in
    // the per-turn context tail instead; see `session_title_turn_hint`.
    // `localhost_preview` is described by its own hint, appended only for
    // sessions that have it (`append_preview_hint`), so the shared prompt
    // never lists a tool most clients cannot run.
    let policy = ToolSchemaPolicy {
        include_session_title_tool: false,
        include_preview_tool: false,
        include_desktop_tool: false,
        include_smart_ui_preview_tool: false,
        include_orchestration_tools: false,
        include_orchestration_v2_tools: false,
        include_orchestration_v3_tools: false,
        include_orchestration_v4_tools: false,
        include_orchestration_v6_tools: false,
        include_orchestration_v7_tools: false,
        include_orchestration_v9_tools: false,
        ..policy
    };
    let mut p = String::new();

    p.push_str(
        "\n# Skills\n\
Skills stay on demand; never probe speculatively. Call `list_skills` only when the request names a skill, the Relevant-skills hint names a match, or built-ins/shell are stuck. \
For local machine state check the shell first, never `list_skills` first. \
`list_skills` returns metadata only; `use_skill` loads the selected SKILL.md.\n\n",
    );

    if agent_mode == crate::config::AgentMode::Plan {
        p.push_str(
            "CRITICAL: You are operating in PLAN MODE (Read-only / Design mode).\n\
             - File writing, deletion, shell commands, delegation, and unknown tools are disabled; you can read, search, ask questions, and design, but CANNOT modify files or execute commands.\n\
             - Investigate before planning with `grep`, `glob`, and `view_file`: read the manifest, real call sites, crates, and existing patterns.\n\
             - Make the plan specific to THIS repository: name files, functions/structs, and inspected line ranges; resolve unknowns now, never guess dependencies or module layout, and state verified facts and uncertainties.\n\
             - Explain the plan and tell the user to switch to Build Mode (press Shift+Tab) to implement it.\n\n"
        );
    }

    p.push_str(
        "You are rustcode, a terminal coding agent.\n\
# Workflow\n\
- Act on change requests: locate, inspect, edit, verify. Prefer the smallest focused sequence once grounded; do not restate tasks or narrate tools. Finish with changed files, verification, and blockers.\n\
- For research/review answers, label verified findings, judgments, and gaps, and never claim comprehensive coverage without evidence. Avoid fixed long templates. Cite `path:line` with full repo-relative paths.\n\
- Use `sandbox/` for temporary work and `artifacts/` for persistent reports. Run commands expected to exceed 2s in the background and keep working; results arrive automatically, so never poll.\n\
- `run_command` uses the platform shell. Chained shell commands are fine when inspectable; use `&&` for dependent commands, show destructive operations, and never mask required failures. Check from the nearest project manifest. If in-scope dependencies are missing, run the deterministic lockfile install once, then retry. Never run `cargo check` on a standalone `.rs` file.\n\
- Prefer write tools for file creation; heredoc only for small appends/pipes.\n\
- Do not branch, commit, push or open a PR unless the user or the project's instructions ask; then, if `git-feature-workflow` is available, load and follow it. Preserve unrelated work; never use `git add .`, `git add -A`, or `git add --all`.\n\
- Tool results are authoritative: claim checks only after an observed exit code 0. Fix compiler/tool errors or warnings, then rerun failed or stale checks. Subagent reports are advisory; inspect the workspace yourself.\n\
- Use native `grep`/`glob` for exact discovery, `rg` through `run_command` for advanced searches, and SocratiCode `codebase_*` for semantic relationships. Inspect the exact range before editing; never guess lines, APIs, or dependencies.\n\
- Batch independent reads in one response; mutations run in order, control-plane calls first. Wait for results before the next calls. Give each call a short `summary` for the user.\n\
- Chained shell observations are fine when small and inspectable. `view_file` returns numbered text and continuation metadata; complete results are authoritative, so do not reread them—edit or verify next. For manual previews, use the user's exact port, do not start/probe/fallback, and let them run it after verification; do not start a server merely to inspect a static app.\n\
- Match neighboring signatures, state/lock, and error conventions.\n\
- Run focused checks and cover boundaries for complex logic.\n\
- Read-only tools run immediately; modifying/destructive operations require confirmation. Use `ask_question` only for ambiguous requirements or explicit validation, never routine confirmation. The UI supplies the write-in slot; do not include `Other`. Finish with a plain-text summary.\n\n\
# Avoiding loops\n\
- Avoid unchanged rereads; use `view_file` with `start_line`/`end_line` or existing evidence. Correct errors and change empty queries.
- Repeated reads, no-ops, and failed attempts are advisory loop signals, not a hard stop. Avoid endlessly repeating an identical call, but continue when needed with a different `view_file` range, `grep`, edit, or test. Use cached replay content when supplied; if a replayed read is incomplete or unavailable, choose another useful inspection. An edit that reports \"already applied\" changed nothing on disk, so do not re-issue that identical edit without new evidence.
- Use `todo_write` only for complex 3+ step work, not routine edits, git, or simple questions; update it at milestones.\n\n"
    );

    // Gate on the same condition as the agent tool list below: promising a
    // delegation workflow with zero callable subagent tools made the model
    // probe `run_command` and `list_mcp_tools` for a `spawn_agent` that was
    // never offered. (#1589)
    if policy.include_agent_tools && agent_mode != crate::config::AgentMode::Plan {
        p.push_str(
            "# Delegation Policy\n\\
- Subagent tools are available by default; use them when parallel work or independent review will help the task.\n\\
- Before delegating, identify the critical path and keep blockers in the main agent. Delegate only bounded, self-contained side tasks with clear outputs and disjoint write scopes.\n\\
- Review every subagent result and inspect its workspace changes before treating the task as complete.\n\\n",
        );
    } else if policy.profile == ToolSchemaProfile::Coding
        && agent_mode != crate::config::AgentMode::Plan
    {
        // Say why there is no agent tool instead of staying silent: with no
        // hint the model spent rounds searching MCP for one. (#1710)
        p.push_str(
            "# Delegation\n\
- No agent tools; don't search. `/delegate` unless config disables.\n\n",
        );
    }

    p.push_str("# Tool Format\n");
    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "Active tool protocol: textual JSON fence. Call tools only as fenced `tool` blocks containing one JSON object; emit no prose before/after.\n\n\
                ```tool\n\
                {\"name\": \"tool_name\", \"arguments\": {...}}\n\
                ```\n\n\
                Rules: keys are \"name\" and \"arguments\"; argument values use their proper JSON types. Use only the ```tool fence (never ```tool_code, ```json, or another fence) and never duplicate a call. Batch independent reads as multiple fences; mutations run in order. Wait for results before the next calls.\n\n"
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "Active tool protocol: textual native tags. Call tools only with native tags; emit no prose before/after.\n\n\
                [TOOL_CALLS]tool_name[ARGS]{\"arg_name\": \"value\"}\n\n\
                Rules: batch independent reads as multiple [TOOL_CALLS] markers; mutations run in order, control-plane calls first. Wait for results before the next calls. Arguments must be a valid JSON object matching the tool parameters.\n\n"
            );
        }
        crate::config::ToolProtocol::ApiNative => {
            p.push_str(
                "Active tool protocol: API-native. Tools use the API's native function-calling interface: invoke them directly; do NOT print tool calls as text or JSON. Batch independent reads together; mutations run in order, control-plane calls first. Wait for results before the next action. When complete, reply with a plain-text summary and no tool call.\n\n"
            );
        }
    }

    // Text protocols enumerate tools in the prompt. ApiNative carries the full
    // tool schema in the request's `tools` field instead, so listing them here
    // would only duplicate that and waste context. That also means a filtered
    // `tools` block is the model's only view of the surface, so state the
    // filtering and the discovery route before returning; it used to mention
    // neither and left a silent-drop rejection as the only feedback. (#1589)
    if matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        p.push_str(
            "# Tool Surface\n\
This request's tool list is filtered, not exhaustive. A listed tool is the only one carrying its argument schema here, and the list stays fixed for the whole turn. \
Never guess MCP server names: see MCP Servers, then `list_mcp_tools` `select` for schemas. Any registered tool works by its exact name.\n\n",
        );
        return p;
    }

    p.push_str("For connected services with a matching MCP server listed, prefer their tools over shell. With no matching server, check the shell first; never probe `list_mcp_tools` speculatively.\n\n");
    p.push_str("Available tools:\n");
    for t in TOOLS {
        if !text_builtin_is_advertised(t, policy, agent_mode) {
            continue;
        }
        if policy.compact_text_prompt {
            p.push_str(&format!("- {} | Args: {}\n", t.name, t.arguments));
        } else {
            p.push_str(&format!(
                "- {} | Args: {} | {}\n",
                t.name, t.arguments, t.description
            ));
        }
    }
    if policy.include_mcp_tools && agent_mode != crate::config::AgentMode::Plan {
        let entries = collect_mcp_tools_with_servers()
            .into_iter()
            .filter(|(name, ..)| policy.offers_mcp_tool(name))
            .map(|(name, server, _, desc, schema)| (name, server, desc, schema))
            .collect::<Vec<_>>();
        p.push_str(&textual_mcp_listing(
            &entries,
            policy.compact_text_prompt,
            mcp_schema_budget_bytes(),
        ));
    }
    if policy.include_agent_tools && agent_mode != crate::config::AgentMode::Plan {
        p.push_str("- spawn_agent | Args: {\"task\":\"task description\",\"context_inheritance\":\"minimal|evidence|recent|fork\",\"evidence\":\"selected facts\"} | Start a fresh child; default minimal. Optional agent_type explorer|worker|default. Writes require explicit write_access or agent_type worker, plus allowed_paths and verification. Returns JSON with agent_id.\n");
        p.push_str("- send_agent | Args: {\"id\": subagent_id, \"message\": \"message\", \"interrupt\": false} | Follow up an idle subagent or steer an active child at its next safe boundary; interrupt stops its current turn and redirects it now.\n");
        if policy.profile == ToolSchemaProfile::Coding {
            p.push_str("- set_goal | Args: {\"goal\": \"goal description\"} | Set a new long-running task and switch the agent to continuous autoloop mode.\n");
            p.push_str("- todo_write | Args: {\"todos\": [{\"id\": \"t1\", \"content\": \"step\", \"status\": \"pending|in_progress|completed|blocked\", \"priority\": \"high|medium|low\"}], \"merge\": false} | Replace the persistent task plan; with merge true, update only the steps whose id you give. Use this at the start of multi-step work and update it as steps finish.\n");
        }
    }

    if policy.compact_text_prompt {
        p.push_str("\nUse the exact tool names and JSON argument shapes listed above.\n");
        return p;
    }

    match protocol {
        crate::config::ToolProtocol::Json => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
```tool\n\
{\"name\": \"grep\", \"arguments\": {\"pattern\": \"agent loop\", \"include\": \"*.rs\"}}\n\
```\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        crate::config::ToolProtocol::Native => {
            p.push_str(
                "\nExample (task — needs a tool):\n\
User: Where is the agent loop implemented?\n\
Assistant:\n\
[TOOL_CALLS]grep[ARGS]{\"pattern\": \"agent loop\", \"include\": \"*.rs\"}\n\n\
Example (conversation — no tool):\n\
User: hello, how are you?\n\
Assistant: Hi! Ready to help with your code. What are you working on?\n",
            );
        }
        // ApiNative returns early above (tools come from the request schema, not
        // the prompt), so this arm is unreachable but keeps the match exhaustive.
        crate::config::ToolProtocol::ApiNative => {}
    }

    p
}

/// First-turn guidance for `set_session_title`, injected into the per-turn
/// context tail so the cached system prompt stays byte-identical across turns.
/// Text protocols list tools only in the system prompt, so the hint also
/// carries the call shape there; API-native requests get the schema from the
/// request's `tools` field.
pub(crate) fn session_title_turn_hint(protocol: crate::config::ToolProtocol) -> String {
    let mut hint = String::from(
        "First-turn session title: a concise title is already saved from the user's request. Call `set_session_title` before doing other work. Name the main task in 2 to 4 plain words when possible, using at most 32 characters. Do not copy the full prompt, transcript, or secrets into the title.",
    );
    if !matches!(protocol, crate::config::ToolProtocol::ApiNative) {
        let tool = &crate::tools::misc::SET_SESSION_TITLE;
        hint.push_str(&format!(
            "\nAvailable this turn only:\n- {} | Args: {} | {}",
            tool.name, tool.arguments, tool.description
        ));
    }
    hint
}

pub fn tool_system_prompt(
    include_agent_tools: bool,
    protocol: crate::config::ToolProtocol,
    agent_mode: crate::config::AgentMode,
) -> String {
    tool_system_prompt_for_policy(
        ToolSchemaPolicy::root_for_mode(include_agent_tools, agent_mode),
        protocol,
        agent_mode,
    )
}
