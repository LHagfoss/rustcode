# TUI slash-command presentation audit

Audited the complete `COMMANDS` registry in
`rustcode/engine/src/app/suggestion.rs` and its dispatch in
`rustcode/engine/src/app/actions/enter.rs`.
The command palette uses that same dispatch, avoiding a second implementation
that could publish informational output into history.

Command panels and settings pickers are user-owned presentation state. Streaming,
tool lifecycle updates and turn completion cannot replace their open state.
Panels reserve the rows directly above the composer, below live activity and
queued-message previews, and are bounded to those rows even on short terminals.
Informational output never enters provider history. Long output scrolls with
Up/Down or PageUp/PageDown; Enter, Escape or q closes it. Completion menus are
hidden while a panel owns input. Approval/question state remains intact beneath
an open user panel and resumes its input surface after dismissal.

| Command | Presentation / action |
| --- | --- |
| `/cancel` | Immediate cancellation; preserves queued work. |
| `/changelog` | Scrollable Changelog panel. |
| `/change_title` | Immediate rename with an argument; missing argument opens usage panel. |
| `/clear` | Immediate transcript display reset. |
| `/context` | Live context panel; direct token arguments update configuration and show result/help in a panel. |
| `/copy` | Immediate clipboard action with existing feedback. |
| `/continue` | Immediate restored-work queue action. |
| `/exit` | Immediate exit. |
| `/goal` | Immediate task submission with an argument; missing argument opens usage panel. |
| `/about` | Scrollable About RustCode panel (alias of /info). |
| `/info` | Scrollable About RustCode panel. |
| `/help` | Scrollable Help panel. |
| `/history` | Session picker; empty list uses a History panel. |
| `/memory` | Scrollable Memory panel for RAM, project-memory inspection, updates and help. |
| `/mcp` | Existing MCP configuration/editor panel. |
| `/model` | Existing model picker; direct profile/model arguments apply immediately and show panel feedback. |
| `/models` | Alias of /model, including arguments. |
| `/new` | Immediate new session. |
| `/fork` | Immediate session fork. |
| `/archive` | Immediate session persistence. |
| `/agents` | Existing subagent context picker. |
| `/delete_chat` | Immediate session deletion and replacement. |
| `/delegate` | Immediate next-task delegation setting; panel feedback. |
| `/workspace` | Usage/status use a Workspace panel; create/archive/cleanup remain immediate actions. |
| `/ollama` | Scrollable configuration/help and asynchronous model-list panel; arguments still update the profile. |
| `/parser` | Alias of /protocol. |
| `/provider` | Immediate profile configuration with arguments; result/help uses a Provider panel. |
| `/prompt` | Loads a Markdown template into the composer for editing; it is not submitted until the next Enter. |
| `/prompts` | Scrollable Prompt commands panel listing templates and their source paths. |
| `/protocol` | Existing protocol chooser; direct arguments apply immediately with panel result/help. |
| `/ps` | Scrollable Background terminals panel; repeated polls do not enter history. |
| `/quit` | Alias of /exit; immediate exit. |
| `/quota` | Scrollable Model quota panel; asynchronous data refresh preserves navigation and respects dismissal. |
| `/resume` | Immediate latest-session restoration. |
| `/session` | Existing session details panel. |
| `/skills` | Scrollable Skills catalog panel. |
| `/stats` | Existing usage panel with captured monthly data. |
| `/stop` | Immediate background-terminal stop action. |
| `/status` | Existing session status panel. |
| `/compact` | Immediate asynchronous conversation compaction; retains its existing task/result lifecycle. |
| `/summarize` | Immediate asynchronous conversation summary; the resulting summary belongs to the transcript. |
| `/sync` | Immediate asynchronous configuration repository synchronization. |
| `/update` | Immediate asynchronous update check/upgrade; retains update decision UI. |
| `/tools` | Scrollable Tools reference panel. |
| `/usage` | Alias of /stats. |
| `/verbosity` | Existing low/high chooser; direct arguments apply immediately with panel result/help. |
| `/yolo` | Existing on/off chooser; direct arguments apply immediately with transient confirmation; invalid arguments use a panel. |
| `/sandbox` | Scrollable supported-mode panel, retaining mode descriptions and user-level configuration guidance; direct arguments apply immediately with panel result/help. |
| `/effort` | Existing effort chooser; direct arguments apply immediately with panel result/help. |
| `/theme` | Existing theme picker; direct arguments apply immediately with panel result/help. |
| `/thinking` | Existing thinking chooser; direct arguments apply immediately with panel result/help. |

`/upgrade` is a dispatched alias of `/update` outside the completion registry.
Unknown commands use the Command help/error panel. Session mutations, turn
cancellation, clipboard actions, compaction, summaries, sync and upgrades are
immediate actions; opening a result/help panel never delays their execution.
Prompt templates are read fresh from `<workspace>/.rustcode/commands` and
`<config dir>/commands`; workspace files override user files with the same
name. User command files are included in config sync, while workspace files
stay with the project.
The desktop controller has its own smaller native command parser; this audit
covers the terminal frontend registry.

## Panel rendering

Every panel renders through the one row format in `ui/modals/panel.rs`, the same
format `/status`, `/stats`, `/session` and `/context` build by hand. A content
line carrying a label, two or more spaces (or a tab) and a value is a
label/value row: the panel pads the label to the widest label of the run so
every value starts on the same cell, renders `` `code` `` and `**strong**` in
the value, and takes the emphasis of a leading `NN%` from `emphasis_for_share`,
so an over-threshold share is marked wherever it appears. A value wider than the
frame wraps under the value column, and a label wider than half the frame is
truncated rather than fragmented.

Two spaces are the delimiter because Markdown treats them as prose whitespace,
so a panel gets an aligned column by writing one instead of by hand-padding a
`format!` width that Markdown would reflow away. The widest row of a block must
therefore keep two spaces before its value too, not one. `/help`, `/about` and
`/sandbox` are written that way; `/sandbox` marks the current mode with `•`
rather than `*`, which would be read as a list bullet. A run shorter than two
rows, or a line that opens a Markdown block (heading, quote, table, fence,
bullet, ordered item), stays with the Markdown renderer, so prose, lists, tables
and fenced code keep their formatting.

A leading `NN%` in a value is read as a share of a budget and takes
`emphasis_for_share`, which crosses its threshold at 20%. A panel that reports a
*remaining* percentage stays out of the row format on purpose, because the
threshold runs the other way there; `/quota` remains a bullet list for that
reason.

Panels are output surfaces, not pickers. They scroll with Up/Down and close with
Enter, Escape or q, and deliberately carry no `› ` selection marker and no
selection state: there is no row to activate and no action bound to it. Every
surface that does take a selection (the completion popup, `@file` popup, the
inline pickers, the command palette, the question and confirmation prompts) uses
the shared marker, column gap and column budget in `ui/modals.rs`, so the
absence here is a deliberate distinction and not a missing convention.

## Panel search

The slash-command popup, the command palette (Ctrl+P) and the model picker share
one search rule (`app::fuzzy`): a query matches a row when its characters appear
in order, or when a small edit budget covers a typo, so `/modle` finds `/model`
and `show ram usge` finds "Show RAM usage". Command results are ranked exact,
then prefix, then fuzzy. The popup and the palette mark the matched characters in
the primary color on unselected rows, so a fuzzy hit shows why it matched; a
single-character query marks nothing, and the selected row is already marked in
full by its background.
