# Visualizations in chat

The agent can put a small, structured visualization in a reply. The terminal
UI draws it with native ratatui widgets. Only frontends that can draw it are
told about the format (see [Teaching the agent](#teaching-the-agent)); any
other client that receives one shows an ordinary JSON code block. This is
the MVP of
[#2088](https://github.com/LHagfoss/rustcode/issues/2088).

## Format

A visualization is a fenced code block with the reserved info string
`rustcode-viz`, holding one JSON object:

````markdown
```rustcode-viz
{"version":1,"type":"bar_chart","title":"Test time by crate","summary":"engine dominates the run","unit":"s",
 "items":[{"label":"engine","value":42},{"label":"tui","value":10.5},{"label":"core","value":3}]}
```
````

Shared fields:

| Field     | Required | Meaning                                                  |
| --------- | -------- | -------------------------------------------------------- |
| `version` | yes      | Payload schema version. Only `1` is supported. New widget types are additive and keep `1`; see [Levels](#widget-levels). |
| `type`    | yes      | Widget type from the allowlist below.                    |
| `title`   | no       | Heading. Defaults to the type name.                      |
| `summary` | no       | One-line takeaway, shown under the heading as plain text. |

### Why a fence

- It stays in ordinary assistant text. Providers, stored sessions, `/copy`,
  export, ACP and other clients see no new message shape. Old
  sessions render as before, and a session holding a visualization still
  opens in an older build, where it shows as a code block.
- The block's edges are explicit and versioned, so the renderer never has to
  guess a visualization from prose.
- Streaming already treats fences as units: scrollback holds an open fence in
  the live viewport until its closing fence arrives.

A typed content block or a tool-result artifact would need protocol changes
in every provider adapter and client. Interactive HTML uses the same fence
approach instead: the macOS app's opt-in [Smart UI](smart-ui.md) runs
`rustcode-ui v1` blocks in a sandboxed web view.

## Widget allowlist (version 1)

These three types cover what coding answers most often push into prose,
ASCII art, or wide Markdown tables:

- **`bar_chart`** compares magnitudes: build or test times, file sizes,
  benchmark deltas, counts per module. Horizontal bars are drawn with
  eighth-cell precision, scaled to the largest absolute value. Negative
  values use the theme's removal color. Fields: `items: [{"label", "value"}]`
  and optional `unit`. At narrow widths each label sits above its bar.
- **`table`** holds ranked or tabular results with validated cells. Fields:
  `columns: [string]` and `rows: [[cell]]`, where a cell is a string, number,
  boolean, or null. Optional `sort_by: {"column": index or column name, "descending": bool}`
  sets the initial order, sorting numbers numerically and text lexically.
  `column` is an index, a numeric string, or a column name (trimmed,
  case-insensitive) among the shown columns; an unknown or hidden column is
  ignored and the table keeps the payload's order. Columns whose cells are all
  numbers are right-aligned. Cell text is escaped and laid out by the
  transcript's existing Markdown table renderer, so the table gets the same
  column fitting, narrow key/value fallback, and theme styles as a
  hand-written one.
- **`tree`** shows hierarchies: module layout, call trees, dependency or
  ownership trees. Fields: `nodes: [{"label", "note"?, "children"?}]`.
  Branches use box-drawing guides, and `note` is shown muted after the label.
  Nodes with children carry a `▾`/`▸` fold marker.

A general box-and-arrow diagram was left out. It needs a layout engine to be
legible in a terminal, and a tree or table covers most coding cases.

## Level 2 widgets (macOS app)

[#2217](https://github.com/LHagfoss/rustcode/issues/2217) adds three types
for answers the first three draw poorly: trends, a few headline numbers, and
progress. They use the same fence, the same `version: 1` payload and the same
shared fields. Only the macOS app draws them so far. The TUI and older apps
show them as source with `unsupported type`, and the engine teaches them only
to a frontend that advertises level 2 (see [Teaching the agent](#teaching-the-agent)).

- **`line_chart`** shows a trend: benchmark or binary size across commits,
  releases or runs.

  | Field    | Required | Meaning |
  | -------- | -------- | ------- |
  | `labels` | yes      | Shared x axis, in order. Strings or numbers. |
  | `series` | yes      | `[{"name"?, "values": [number \| null]}]`. One value per label; `null` or a missing trailing value is a gap. A missing `name` becomes `series N`. |
  | `unit`   | no       | Shown after every value. |

  Hovering draws a rule at the nearest label and lists every series' value
  there. A legend is shown for two or more series.

  ````markdown
  ```rustcode-viz
  {"version":1,"type":"line_chart","title":"Release binary size","unit":"MB",
   "labels":["v0.83","v0.84","v0.85"],"series":[{"name":"macOS","values":[23.1,18.2,18.4]}]}
  ```
  ````

- **`stat_list`** holds a few headline numbers as labeled tiles: coverage,
  counts, sizes, durations. Fields: `items: [{"label", "value", "unit"?,
  "note"?}]`, where `value` is a string or a finite number. Tiles wrap to the
  width.
- **`checklist`** shows read-only progress: migration steps, CI checks,
  review items. Fields: `items: [{"label", "status"?, "note"?}]`, where
  `status` is `done`, `todo` (the default), `in_progress`, `failed` or
  `skipped`. A bar shows how many items are done, counted over every item,
  shown or not. Items cannot be ticked: the widget is data, and nothing is
  sent back to the agent.

Each fails to the source view, with its reason, when a required field is
missing or has the wrong type, a `status` is not one of the five, or a
`line_chart` has no number at all.

## Interacting in the terminal

Tables and trees in a finished reply respond to the mouse, like the macOS
app:

- **Table headers sort.** Click a header to sort by that column ascending,
  and click it again to flip the direction. Numeric columns sort by value
  and text columns lexically. The sorted header shows `▲` or `▼`. A click
  overrides the payload's `sort_by`, which is marked the same way. Every row
  is sorted, then the first 50 are shown. A table narrow enough to fall back
  to key/value records has no header row and cannot be re-sorted.
- **Tree nodes fold.** A node with children shows `▾` when open and `▸` when
  closed. Click it (marker or label) to toggle it. A closed node shows how
  many nodes it hides (`▸ modals/  3 hidden`). Every node starts open.
  Nodes cut off by the depth limit have no marker.

Targets light up under the pointer. A press and a release on the same cell
counts as a click, so dragging across a widget still selects text. If you
are scrolled up, the row you clicked stays in place while rows below it come
or go.

This view state lives only in the TUI's transcript state. It is keyed by
the message's history index and the widget's position among the message's
`rustcode-viz` fences, together with a hash of the fence body, so state made
for a different body is ignored. It is never written to the session,
sent to the provider, or applied to `/copy` or export. The canonical text
keeps the original fence. The committed block cache includes the view state
in its key, so a click redraws the widget and nothing stale stays painted.
Only finalized messages are interactive. A reply that is still streaming,
an open fence, and rows already written to terminal scrollback always show
the default view.

Bar charts are not interactive. There is no keyboard path into a widget
yet. The transcript has no focus model for items inside a message (the
`ctrl+shift+o` step walks tool output only), so keyboard sorting and folding
are left for a follow-up. The rendered rows are still the textual
representation: selecting them copies readable text.

## Limits

| Limit                         | Value  | When exceeded                       |
| ----------------------------- | ------ | ----------------------------------- |
| Payload size                  | 16 KiB | Source fallback                     |
| Title / summary               | 80 / 300 chars | Truncated with `…`          |
| Labels, column headers        | 40 chars | Truncated with `…`                |
| Table cells, tree labels/notes| 60 chars | Truncated with `…`                |
| Bar chart items               | 30     | `… N more items not shown`          |
| Table rows / columns          | 50 / 8 | `… N more rows/columns not shown`   |
| Tree nodes / depth            | 120 / 8 | `… N more nodes not shown`         |
| Line chart points / series    | 100 / 4 | `… N more points/series not shown` (level 2) |
| Stat list items               | 12     | `… N more items not shown` (level 2) |
| Checklist items               | 50     | `… N more items not shown` (level 2) |
| Stat values, series names     | 40 chars | Truncated with `…` (level 2)     |
| Checklist labels and notes, stat notes | 60 chars | Truncated with `…` (level 2) |

Every row is also cut to the viewport width. Control characters and newlines
in any text are replaced with spaces, so a payload cannot send terminal
escapes or break the layout. Because of these limits a widget is at most
about 60 rows tall. The 16 KiB cap keeps parsing cheap.

## Fallback

The block is rendered as its JSON source, with the normal code-block styling
and one muted note line (`◇ Couldn't draw this visualization (reason) — showing
its source.`), when:

- the JSON is malformed or is not an object,
- `version` is missing or not `1`,
- `type` is not on the allowlist,
- a required field is missing or has the wrong type,
- the payload is over 16 KiB, or
- a finalized message ends before the closing fence.

Ordinary fences (including ```` ```json ````) and Markdown tables are
unchanged. The info string must be exactly `rustcode-viz`.

## Streaming

While the closing fence has not arrived, the live viewport shows a single
placeholder row (`◇ Visualization: waiting for the closing fence (N lines)`)
and the body is not parsed. Terminal scrollback already holds an unfinished
fence, so a partial widget is never committed. Once the fence closes, the
live stream, committed scrollback chunks, and finalized history cells all
draw the same rows through the same renderer. That renderer is cached by
width and theme like any other assistant message, and in the transcript
also by the widget view state.

## macOS app

The macOS app (`macos/`) renders the same schema with the same limits and
fallback reasons. `RustCodeKit/Visualization.swift` parses and bounds the
payload, and `RustCode/VisualizationView.swift` draws it in SwiftUI:

- `bar_chart` uses Swift Charts. Hovering a bar highlights it and shows its
  exact value.
- `table` headers sort it: click once for ascending, again for descending.
  Every row is kept so a sort covers the whole table, but at most 50 are
  shown. `sort_by` sets the initial order.
- `tree` branches collapse and expand. The first two levels start open
  (the TUI starts with every node open).
- `line_chart`, `stat_list` and `checklist` are the
  [level 2 widgets](#level-2-widgets-macos-app) above.

The card's Copy button copies the source fence. Its context menu has
**Copy Source** and **Copy as Text**, a plain-text form of the data
(`RustCodeKit/VisualizationText.swift`): Markdown tables for tables and line
charts, an indented list for trees, `[x]`-style lines for checklists, and
`label: value` lines otherwise. VoiceOver reads the card's title, then a
one-sentence summary of its data (item and row counts, the largest bar, each
line's first, last, low and high values, checklist progress), and each bar,
point, tile and checklist item has its own label. A block that is still
streaming shows a "Building visualization…" placeholder. Invalid or
unfinished blocks show the source with the same note as the TUI. Only
assistant replies render widgets: command output, user messages and tool
output keep the fence as code. The app advertises
`clientCapabilities._meta.rustcode.visualizations = 2` in its ACP
`initialize`.

### Finding them in the app

Users do not need this page to find the widgets. The app's guide lists each
type with what it is for, how to use it, a live sample drawn by the real
renderer, and a request with an **Insert** button that puts it in the message
field. It opens from:

- the `/visualize` slash command (an app command, so it works before the
  engine starts),
- **Help ▸ Charts, Tables & Trees in Chat…**,
- **Ask for a Chart, Table or Tree…** in the composer's + menu, and
- the **Ask for a chart, table, or tree** tip when it appears on an empty chat.

The guide's entries, prompts and samples live in
`RustCodeKit/VisualizationGuide.swift`; a test checks that it covers every
supported type and that every sample renders. `--visualization-demo` shows
the same samples in a standalone window.

## Assistant-only

Only the assistant message renderer recognizes the fence. User messages,
tool output, and tool-result transcripts never render widgets, and nothing in
a payload is executed. It is data drawn by fixed widget code.

## Copy and export

The canonical message text is unchanged. `/copy`, session export, and
provider history keep the fence as written. The copy registry for a widget
holds the whole source fence, so pasting it elsewhere reproduces the widget.
Selecting rendered rows with the mouse copies their text.

## Teaching the agent

The agent learns the format from a short `# Visualizations` section (about
120 tokens) that is appended to the system prompt **only when the frontend
renders the blocks**. `AppState::visualization_level` holds that decision.
It is set once, when the frontend creates the session state, so the prompt
and its cached prefix do not change during a session. The default is `0`.

### Widget levels

A frontend's level says which types it draws, so the agent is never taught a
type the user would see as JSON:

| Level | Types | Prompt |
| ----- | ----- | ------ |
| 0 | none | no hint |
| 1 | `bar_chart`, `table`, `tree` | the level 1 hint, unchanged |
| 2 | level 1 plus `line_chart`, `stat_list`, `checklist` | the level 1 hint plus one more paragraph (about 75 tokens) |

| Frontend                          | Level | How it is decided                                   |
| --------------------------------- | ----- | --------------------------------------------------- |
| TUI (local)                       | 1     | Set at startup                                      |
| macOS app                         | 2     | Advertises the ACP capability below                 |
| ACP client advertising support    | 1–2   | `clientCapabilities._meta.rustcode.visualizations` at `initialize`, capped at 2 |
| Any other ACP client              | 0     | Capability absent, malformed, or `0`                |
| Remote gateway, iOS, Rust desktop, remote TUI | 0 | Default                                |

The engine announces its highest level in its own `initialize` response
(`agentCapabilities._meta.rustcode.visualizations = 2`), so a client can tell
which types the agent knows. An older app that advertises `1` keeps the
level 1 prompt.

The hint is also left out of subagent prompts, which are not shown to the
user directly, and out of compact prompts for small models. It is appended
after the cached base prompt, so frontends without the capability send
exactly the same prompt as before. If a model writes a block anyway, a client
without a renderer shows it as a JSON code fence.

### Example

> **User:** Which test binaries are slowest in this workspace?
>
> **Assistant:** Ran `cargo test --workspace` with timing; the engine lib
> tests dominate.
>
> ````markdown
> ```rustcode-viz
> {"version":1,"type":"bar_chart","title":"Test wall time","summary":"engine lib tests take 70% of the run","unit":"s",
>  "items":[{"label":"rustcode (lib)","value":41.8},{"label":"rustcode-tui","value":12.1},{"label":"rustcode-core","value":2.4}]}
> ```
> ````

At 56 columns the TUI draws:

```text
◆ Test wall time
engine lib tests take 70% of the run
rustcode (lib) ██████████████████████████████████ 41.8 s
rustcode-tui   █████████▉                         12.1 s
rustcode-core  ██                                  2.4 s
```

Use prose or a Markdown table for a handful of values. Use a visualization
when there are many numbers to compare, results to rank, or a hierarchy to
show.

## Future GUI renderers

A frontend that wants to run agent-authored HTML needs a separate design and
security review first. It must have strict origin and path authorization,
size limits, CSP/sandbox isolation, no ambient filesystem or network access,
safe bridge messages, and explicit navigation and focus handling.
Sanitization alone is not a security boundary.
