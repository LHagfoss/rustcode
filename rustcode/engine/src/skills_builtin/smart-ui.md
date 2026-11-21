---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive view for the RustCode chat. Use when the prompt says Smart UI is on and a table, comparison, chart, plan, numbers or controls would help, asked or not.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, html, chart, slider, filter, sort, toggle
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

Pick by question: a trend → line chart; parts → stacked bar or donut; a
ranking → bars; options → cards; many exact rows → a table; numbers → the
dashboard below. Prefer bars, cards and charts to a flat table; asked for
graphs or hover, use Chart.js.

## Format

````markdown
```rustcode-ui v1
:: title Test runs
:: stats
- 128 | Runs | +12 today {ok}
- 3 | Failed {bad}
:: bars Slowest crates
- engine | 42 | 42 s | 7 tests over 1 s
- tui | 10.5 | 10.5 s
```
````

The title labels the view; do not repeat it. Shorthand for the kit (the app
writes its markup), HTML for the rest; they mix.

## Shorthand

A `::` line opens a block, its `- ` items split at `|` (not in backticks), and
a blank line or a line starting `<` ends it. An item may end `{ok}`, `{warn}`,
`{bad}` or `{info}` (a titled item may put it after its title);
`code`, **bold** and HTML work in text. Keep a card's or item's text to one
short line: status goes in its tint, size or effort in its badge. A name after a
block is an `h3` over it.

- `:: title T` · `:: h Heading` · `:: p` text
- `:: bars` `- label | value | shown | tip`, scaled to the largest (`%` as written)
- `:: stats` `- value | label | note` · `:: steps` `- [x]` done, `[>]` active, `[!]` failed, `[ ]` to do
- `:: list` `- Title | detail | value` · `:: cards` `- Title | body | badge`
- `:: actions` `- Label | message` (follow-up chips; the message is sent as the user)
- `:: table` then `a | b` rows, the first the heading; sorts past eight rows
- `:: callout info|ok|warn|bad` then text · `:: sources` `- Title | url`, last

Optional inline `<script>`, data embedded. No network, imports or storage; a link opens in the browser, never in the view. Limits: 64 KB, 5,000
elements. The app draws it as it streams: lead with the part worth reading.

## Theme

Every colour, font, radius and size is the app's. Write none of your own: a
literal is what makes two views of the same kind look unlike, and
`smart_ui_check` reports it.

- Ink: `--rc-text --rc-muted --rc-faint --rc-accent --rc-accent-text --rc-bg
  --rc-surface --rc-separator --rc-danger --rc-success --rc-warning`.
- Type: `--rc-font --rc-mono --rc-text-size` and its `-sm -xs -title -value`.
- Shape: `--rc-radius --rc-radius-sm --rc-radius-xs --rc-radius-pill`.
- Charts: `--rc-series-1` … `--rc-series-8` (the first is the accent).

## Kit (already on the page)

Use the classes; write as little CSS as you can, and none for colour, font,
height or spacing. The page spaces one block from the next by itself (12 px),
so leave out margins (16 px; 28 px inside `.rc-dash`). A `.rc-card` is already a card: never nest one.

- Text: `.rc-title .rc-lead .rc-label .rc-muted .rc-mono .rc-num`.
- Numbers: `.rc-stats` > `.rc-stat` > `.rc-stat-value .rc-stat-label .rc-stat-note`;
  `.rc-flat` on `.rc-stats` drops the boxes for big bare values.
- Headline: `.rc-head` (a title, then a muted caption pushed right) and `.rc-hero`
  (`.rc-label`, `.rc-hero-value` huge, `.rc-hero-delta` accent, then `p.rc-lead`
  whose `<b>` reads bold).
- Status: `.rc-badge .rc-callout`, tinted by `.rc-ok .rc-warn .rc-bad .rc-info`
  (also on a stat, a bar or a line in `.rc-term`);
  `<div class="rc-progress"><span style="width:72%"></span></div>`
- Rows: `<table class="rc-table" data-rc-sort>` sorts on a click or Enter in a
  heading (`data-rc-sort="text"` on one `th` compares it as text) and
  highlights the row under the pointer; in `.rc-scroll` its header sticks.
  `.rc-list > .rc-item`: a label row, an optional trailing value or badge, an optional `p`.
- Review: `.rc-findings > .rc-finding[data-severity=high|medium|low]` holding `<div><code>a.ts:21</code> · Codex</div><p>…</p><p class="rc-finding-note rc-ok">Fixed</p>`; a set-aside note drops `rc-ok` and ends with a `data-rc-send` Reopen chip.
- Folding: `<details class="rc-details"><summary>Files</summary>…</details>`,
  `.rc-tree` around nested ones, `.rc-file` rows of name and value.
- Output: `<div class="rc-term"><div>$ cargo test</div><div class="rc-ok">ok</div></div>`
- Actions, no script: `data-rc-send="Show X"` sends a follow-up, `data-rc-copy`
  copies and says Copied; a form's named fields ride along:
  `<form data-rc-send="Apply filters"><select name="range">7d</select><button>Apply</button></form>`
  sends "Apply filters", then `range: 7d`. `.rc-tip` is
  `<span class="rc-tip" tabindex="0">1<span>Source</span></span>`.
- Tabs: `.rc-tabs` > `[role=tablist]` > `button[role=tab][data-rc-tab=a]` and `section[data-rc-panel=a]`.
- Layout: `.rc-row .rc-col .rc-grid .rc-gap-1` … `.rc-gap-4 .rc-card .rc-scroll`.
  `.rc-num` keeps a short column on one line.

## Charts (Chart.js, bundled)

`<script src="rustcode-lib:chart.js"></script>`, then a chart in
`<div class="rc-chart"><canvas role="img" aria-label="…"></canvas></div>`.
One height, 200 px: step it with `data-size="sm"` (140 px) or `data-size="lg"`
(260 px), `data-size="xl"` (340 px), never with an inline height. Themed: series colours, hover tooltips,
a hovered bar a step lighter, legend toggles, a line filled from its colour
down. A canvas draws nothing with `CanvasText`, and its figures are not
tabular, so keep exact numbers in `.rc-num` text beside it.

```html
<script src="rustcode-lib:chart.js"></script>
<div class="rc-chart"><canvas role="img" aria-label="Runs per day"></canvas></div>
<script>
  new Chart(document.querySelector(".rc-chart canvas"), { type: "line",
    data: { labels: ["Mon"], datasets: [{ label: "Runs", data: [24] }] } });
</script>
```

- Stacked bar: `type: "bar"`, `options: { scales: { x: { stacked: true }, y: { stacked: true } } }`.
- Donut with a centre total: `type: "doughnut"` and
  `<div class="rc-donut-total"><b>128</b><span>runs</span></div>` in the same `.rc-chart`.
- Line marks: `options.plugins.rcMarks = { band: [75, 270], line: 150, lineLabel: "normal", end: true }`
  shades a y range, draws a labelled reference line, dots the last point with its value.
- Sparkline, no script: `<span class="rc-spark" data-values="1,4,2,8"></span>`.

From a script: `window.rustcode?.send(text, context?)` only in a click or key
press (`context`, an object, follows as a JSON fence), `copy(text)`, `theme`,
`onThemeChange(callback)`. Older apps have no runtime, so fall back:
`window.rustcode?.send(...)`, `var(--rc-muted, GrayText)`.

## Dashboard (numbers, trends)

When the answer is a number that moved, build the layout the reader scans
top-down: headline, context, stats, chart. A small `.rc-head` over everything,
one `.rc-hero` (the value, its multiplier or delta, one or two plain
sentences with the key figures in `<b>`), an `hr`, a `.rc-stats.rc-flat` of
three or four stats whose `.rc-stat-note` is the comparison, then a `.rc-head`
and a large `data-size="xl"` line chart with `rcMarks`. All in one `.rc-dash`.

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, relative widths, no `vh`. Hover is an extra, never the only way to read a value.
- Each row: a bold lead, label or number, then at most two short sentences.
- Body text full strength; `.rc-lead`, `.rc-muted`, `.rc-stat-note` for one aside only.
- One idea per row. A panel's parts: an `h3`, then its block, no wrapper `div`.
- Tabs only for two to four parts; actions last, the recommended one `data-primary`.

## Before you send

Run `smart_ui_check` whenever it is listed and fix what it reports.
