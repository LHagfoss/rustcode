---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive view for the RustCode chat. Use when the prompt says Smart UI is on and a table, comparison, chart, plan, numbers or controls would help, asked or not.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, html, chart, slider, filter, sort, toggle
---

# Smart UI

## When

Default to a view for anything structured: a table, comparison, breakdown,
status overview, plan, numbers, a chart or a dashboard, asked or not. Prose, a
single fact, a few bullets, code and diffs stay Markdown. A report to another
thread stays plain Markdown.

Around the view: one short line before it with the finding, nothing after it.
Caveats go in the view (`.rc-callout`, `.rc-muted`); sources go last in it,
folded (Kit › Sources), never as a Markdown list after it.

Pick by question: a trend over time → line chart; parts of a whole → stacked
bar or donut; a ranking → `.rc-bars`; exact values the user will scan → a table
(sortable past eight rows); a headline number → `.rc-stats`. Comparisons are a
table, the form the user likes best. Asked for graphs or hover, chart with
Chart.js: tooltips, legend toggles and the eight series colours come free, and
hovering a column shows that column.

## Format

````markdown
```rustcode-ui v1
<title>Test runs</title>
<div class="rc-stats"><div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span></div></div>
```
````

`<title>` labels the view; do not repeat it. Optional inline `<script>`, data
embedded. No network, imports or storage; a link opens in the browser, never in the view. Limits: 64 KB, 5,000
elements. The app draws it as it streams: lead with the part worth reading.

## Theme

Every colour, font, radius and size is the app's. Write none of your own: a
literal is what makes two views of the same kind look unlike, and
`smart_ui_check` reports it.

- Ink: `--rc-text --rc-muted --rc-faint --rc-accent --rc-accent-text --rc-bg
  --rc-surface --rc-separator --rc-danger --rc-success --rc-warning`.
- Type: `--rc-font --rc-mono --rc-text-size`, also `-sm` (table, bar, chip),
  `-xs` (label, badge, tip), `--rc-text-size-title` and
  `--rc-text-size-value` (a big number).
- Shape: `--rc-radius --rc-radius-sm --rc-radius-xs --rc-radius-pill`.
- Charts: `--rc-series-1` … `--rc-series-8` (the first is the accent).

## Kit (already on the page)

Use the classes; write as little CSS as you can, and none for colour, font,
height or spacing. The page spaces one block from the next by itself (12 px),
so leave out margins. A `.rc-card` is already a card: never nest one.

- Text: `.rc-title .rc-lead .rc-label .rc-muted .rc-mono .rc-num`.
- Numbers: `.rc-stats` > `.rc-stat` > `.rc-stat-value .rc-stat-label .rc-stat-note`.
- Status: `.rc-badge .rc-callout`, tinted by `.rc-ok .rc-warn .rc-bad .rc-info`
  (also on a stat, a bar or a line in `.rc-term`);
  `<div class="rc-progress"><span style="width:72%"></span></div>`
- Bars: `<div class="rc-bars"><div class="rc-bar" data-rc-tip="7 failed"><span>Name</span><i style="width:40%"></i><b>40</b></div></div>`
  Each bar highlights and shows its `data-rc-tip` on hover.
- Rows: `<table class="rc-table" data-rc-sort>` sorts on a click or Enter in a
  heading (`data-rc-sort="text"` on one `th` compares it as text) and
  highlights the row under the pointer; in `.rc-scroll` its header sticks.
  An item is a label row with an optional trailing value and may carry a description block (`div` or `p`).
  `<ul class="rc-list"><li class="rc-item"><span>Name</span><span class="rc-badge rc-ok">Ready</span></li></ul>`
- Plans: `<ol class="rc-steps"><li class="rc-step rc-done">Built</li></ol>`, also `rc-active rc-failed rc-todo`.
- Sources: `<details class="rc-details rc-sources"><summary>Sources</summary><ol><li><a href="https://…">Title</a></li></ol></details>`; a link opens in the browser.
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

## Charts (Chart.js, bundled)

`<script src="rustcode-lib:chart.js"></script>`, then a chart in
`<div class="rc-chart"><canvas role="img" aria-label="…"></canvas></div>`.
One height, 200 px: step it with `data-size="sm"` (140 px) or `data-size="lg"`
(260 px), never with an inline height. Themed: series colours, hover tooltips,
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
- Sparkline, no script: `<span class="rc-spark" data-values="1,4,2,8"></span>`.

From a script: `window.rustcode?.send(text, context?)` only in a click or key
press (`context`, an object, follows as a JSON fence), `copy(text)`, `theme`,
`onThemeChange(callback)`. Older apps have no runtime, so fall back:
`window.rustcode?.send(...)`, `var(--rc-muted, GrayText)`.

## One example per intent

All kit, no CSS: the same intent reads the same way in every turn.

- Status: `<div class="rc-stats"><div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span></div><div class="rc-stat rc-bad"><span class="rc-stat-value">7</span><span class="rc-stat-label">Failed</span></div></div>`
  then a `.rc-callout rc-warn` with the cause.
- Comparison: `<table class="rc-table" data-rc-sort"><thead><tr><th>Suite</th><th>Status</th></tr></thead><tbody><tr><td>engine</td><td><span class="rc-badge rc-ok">Passing</span></td></tr></tbody></table>`
- Ranking: `<div class="rc-bars"><div class="rc-bar"><span>engine</span><i style="width:100%"></i><b>42 s</b></div><div class="rc-bar rc-bad"><span>auth</span><i style="width:25%"></i><b>10.5 s</b></div></div>`
- Trend: `<p class="rc-lead">Failures fell to zero.</p><div class="rc-chart"><canvas role="img" aria-label="Failures per day"></canvas></div>`
- Plan: `<ol class="rc-steps"><li class="rc-step rc-done">Bump the version</li><li class="rc-step rc-active">Refresh the fixture</li><li class="rc-step rc-todo">Publish</li></ol>`

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, and grows to the height its content needs: relative widths, no `vh`.
- Useful before any interaction; a control's effect shows next to it.
- Real controls with `aria-label` where a label is missing; pre-aggregate.
- Hover is an extra, never the only way to read a value.

## Before you send

Run `smart_ui_check` whenever it is listed, fix every problem it reports, then
write the fence. It catches the sandbox's refusals and the style the kit
already sets: a colour, a font size, a fixed height, a card inside a card.
