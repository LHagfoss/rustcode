# Visualizations in chat

The agent can put a small, structured visualization in a reply. The terminal
UI draws it with native ratatui widgets; every other client shows it as an
ordinary JSON code block. This is the MVP of
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
| `version` | yes      | Schema version. Only `1` is supported.                   |
| `type`    | yes      | Widget type from the allowlist below.                    |
| `title`   | no       | Heading. Defaults to the type name.                      |
| `summary` | no       | One-line takeaway, shown under the heading as plain text. |

### Why a fence

- It stays in ordinary assistant text. Providers, stored sessions, `/copy`,
  export, ACP and the macOS/iOS clients see no new message shape. Old
  sessions render as before, and a session holding a visualization still
  opens in an older build, where it shows as a code block.
- The block's edges are explicit and versioned, so the renderer never has to
  guess a visualization from prose.
- Streaming already treats fences as units: scrollback holds an open fence in
  the live viewport until its closing fence arrives.

A typed content block or a tool-result artifact would need protocol changes
in every provider adapter and client. That is left for a later milestone,
for example a sandboxed HTML viewer in a GUI frontend.

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
  boolean, or null. Optional `sort_by: {"column": index, "descending": bool}`
  sorts numbers numerically and text lexically. Columns whose cells are all
  numbers are right-aligned. Cell text is escaped and laid out by the
  transcript's existing Markdown table renderer, so the table gets the same
  column fitting, narrow key/value fallback, and theme styles as a
  hand-written one.
- **`tree`** shows hierarchies: module layout, call trees, dependency or
  ownership trees. Fields: `nodes: [{"label", "note"?, "children"?}]`.
  Branches use box-drawing guides, and `note` is shown muted after the label.

A general box-and-arrow diagram was left out. It needs a layout engine to be
legible in a terminal, and a tree or table covers most coding cases.

All three widgets are non-interactive. Keyboard focus inside a widget
(column re-sorting, collapsing tree nodes) would need a focus model in the
transcript, which does not exist yet. That is deferred. The rendered rows are
already the textual representation: selecting them copies readable text.

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

Every row is also cut to the viewport width. Control characters and newlines
in any text are replaced with spaces, so a payload cannot send terminal
escapes or break the layout. Because of these limits a widget is at most
about 60 rows tall. The 16 KiB cap keeps parsing cheap.

## Fallback

The block is rendered as its JSON source, with the normal code-block styling
and one muted note line (`◇ Visualization not rendered (reason); showing
source`), when:

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
width and theme like any other assistant message.

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

The root agent's system prompt includes a short `# Visualizations` section
(about 120 tokens). It is left out of subagent prompts, which are not shown
to the user directly, and out of compact prompts for small models. The hint
goes to every frontend: clients without a renderer show a readable JSON code
block, which is an acceptable fallback, and keeping one prompt avoids
splitting the prompt cache by frontend.

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
