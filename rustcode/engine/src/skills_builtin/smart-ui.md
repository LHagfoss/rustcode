---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) view in the RustCode chat from its `::` component catalog. Use when the prompt says Smart UI is on and a table, comparison, chart, plan, numbers or controls would help, asked or not.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, catalog, html, chart, slider, filter, sort, toggle, form, tabs
---

# Smart UI

## When

Answer visually, asked or not: data, numbers, results, status, comparisons,
plans or 4+ items go in a view, never a Markdown table. Only short
explanations, one fact, code and diffs stay Markdown, as does a report to
another thread.

Be brief: one short line before the view with the finding, nothing after it.
Caveats go in the view (`.rc-callout`, `.rc-muted`); sources go last in it,
folded (Kit › Sources), never as a Markdown list after it.

Pick by question: a trend → `:: chart line`; parts → `:: chart stacked` or
`donut`; a ranking → `:: bars`; options → `:: cards` or `:: compare`; many
exact rows → `:: table`; numbers → the dashboard below; a choice the user
makes → `:: form`.

## Format

````markdown
```rustcode-ui v1
:: title Test runs
:: stats
- 128 | Runs | +12 today {ok} | 20,24,22,31
- 3 | Failed {bad}
:: chart line Runs per day | end
day | Runs | Failed
Mon | 24 | 1
Tue | 31 | 2
:: bars Slowest crates
- engine | 42 | 42 s | 7 tests over 1 s
- tui | 10.5 | 10.5 s
```
````

The title labels the view; do not repeat it. Build it from the catalog: you
name a block and give its data, the app draws it, styles it and makes it
interactive, so every view of one kind looks alike. HTML only for what no
block covers; they mix.

## Catalog

A `::` line opens a block, its `- ` items split at `|` (not in backticks), and
a blank line or a line starting `<` ends it. An item may end `{ok}`, `{warn}`,
`{bad}` or `{info}` (or put it after any cell); `code`, **bold** and HTML work
in text. A name after a block is an `h3` over it. Keep a card's or item's text
to one short line: status goes in its tint, size or effort in its badge.

- Text: `:: title T` · `:: head Title | caption` · `:: h Heading` · `:: p` text ·
  `:: callout info|ok|warn|bad` then text · `:: divider` ·
  `:: details Summary | aside` then text or items (a fold)
- Tabs: `:: tab Name` before each part, `:: end` after the last (two to four)
- Numbers: `:: hero Label` `- 503 | 3.4x | one sentence` ·
  `:: stats` `- value | label | note | 1,4,2,8` (last cell: a sparkline) ·
  `:: progress` `- label | 72% | note` (or `3/5`)
- `:: bars` `- label | value | shown | tip`, scaled to the largest (`%` as written)
- `:: chart line|area|bar|stacked|hbar|donut|pie Title | size=sm|lg|xl | line=150 normal | band=75-270 | end`,
  then a table: `x | series…` heading row, one row per x. Themed, hover
  tooltips, legend toggles; a donut gets its total and a legend with values.
- Rows: `:: table` then `a | b` rows, the first the heading; sorts past eight rows ·
  `:: compare` the same, `yes`/`no`/`partial` cells become marks ·
  `:: heatmap` the same, numbers shade their cells
- `:: list` `- Title | detail | value` · `:: kv` `- Key | value` · `:: cards` `- Title | body | badge`
- `:: steps` `- [x]` done, `[>]` active, `[!]` failed, `[ ]` to do ·
  `:: timeline` `- 10:02 | event | detail`
- `:: findings` `- high|medium|low | file:line · who | finding | note` (`{ok}`: fixed) ·
  `:: files` `- path | +12 -3 | note` · `:: tags` `- label {tint}` ·
  `:: term` and `:: code lang`, lines as written
- `:: actions` `- Label | message` (follow-up chips; the message is sent as the user)
- `:: form Message | Button` then fields: `- select name | Label | 7d, *30d` (`*`
  preselects), `choice` (segmented), `text|number|date name | Label | placeholder`,
  `check name | Label`, `range name | Label | 0-100 = 40`. Sends Message, then
  `name: value` lines.
- `:: sources` `- Title | url`, last

No network, imports or storage; a link opens in the browser, never in the
view. Limits: 64 KB, 5,000 elements. The app draws it as it streams: lead with
the part worth reading.

## Theme

Every colour, font, radius and size is the app's. Write none of your own: a
literal is what makes two views of the same kind look unlike, and
`smart_ui_check` reports it. In HTML use `--rc-text --rc-muted --rc-faint
--rc-accent --rc-bg --rc-surface --rc-separator --rc-danger --rc-success
--rc-warning`, `--rc-font --rc-mono --rc-text-size` (`-sm -xs -title
-value`), `--rc-radius` (`-sm -xs -pill`) and `--rc-series-1` …
`--rc-series-8`.

## HTML, when no block fits

The kit is on the page; the blocks above are written in it. The page spaces
blocks itself (16 px; 28 px in `.rc-dash`), so add no margins, and never nest
a `.rc-card`.

- Layout: `.rc-row .rc-col .rc-grid .rc-gap-1` … `.rc-gap-4 .rc-card .rc-scroll .rc-num .rc-muted .rc-label`.
- Status: `.rc-badge .rc-callout`, tinted by `.rc-ok .rc-warn .rc-bad .rc-info`.
- `<table class="rc-table" data-rc-sort>` sorts on a heading.
- Actions, no script: `data-rc-send="Show X"` sends a follow-up, `data-rc-copy`
  copies; a `<form data-rc-send>` sends its named fields. `.rc-tip` is
  `<span class="rc-tip" tabindex="0">1<span>Source</span></span>`.
- Chart.js by hand, only for what `:: chart` cannot draw:
  `<script src="rustcode-lib:chart.js"></script>` and
  `<div class="rc-chart"><canvas role="img" aria-label="…"></canvas></div>`,
  then `new Chart(canvas, config)` in a script; themed already, no colours.
  `options.plugins.rcMarks = { band, line, lineLabel, end }`.

From a script: `window.rustcode?.send(text, context?)` only in a click or key
press (`context`, an object, follows as a JSON fence), `copy(text)`, `theme`,
`onThemeChange(callback)`. Older apps have no runtime, so fall back:
`window.rustcode?.send(...)`, `var(--rc-muted, GrayText)`.

## Dashboard (numbers, trends)

When the answer is a number that moved, build the layout the reader scans
top-down: `:: head` (title and source), `:: hero` (the value, its multiplier
or delta, one plain sentence with the key figures in **bold**), `:: stats`
(three or four, the note is the comparison), then
`:: chart line … | size=lg | band=… | line=… | end`.

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, relative widths, no `vh`. Hover is an extra, never the only way to read a value.
- Each row: a bold lead, label or number, then at most two short sentences.
- Body text full strength; `.rc-lead`, `.rc-muted`, `.rc-stat-note` for one aside only.
- One idea per row. A panel's parts: an `h3`, then its block, no wrapper `div`.
- Tabs only for two to four parts; actions last, the recommended one `data-primary`.

## Before you send

Run `smart_ui_check` whenever it is listed and fix what it reports.
