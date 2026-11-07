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
single fact, a few bullets, code and diffs stay Markdown. Put the finding in a
sentence before the view. A report to another thread stays plain Markdown.

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

`<title>` labels the view; do not repeat it. Optional inline `<style>` and
`<script>`, data embedded; omit either when empty. No network, imports, storage
or navigation. Limits: 64 KB, 5,000 elements. The app draws it as it streams: lead
with the part worth reading.

## Kit (already on the page)

Set no colours and write as little CSS as you can.

- Text: `.rc-title .rc-lead .rc-label .rc-muted .rc-mono .rc-num`.
- Numbers: `.rc-stats` > `.rc-stat` > `.rc-stat-value .rc-stat-label`.
- Status: `.rc-badge .rc-callout`, tinted by `.rc-ok .rc-warn .rc-bad .rc-info`
  (also on a stat, a bar or a line in `.rc-term`);
  `<div class="rc-progress"><span style="width:72%"></span></div>`
- Bars: `<div class="rc-bars"><div class="rc-bar" data-rc-tip="7 failed"><span>Name</span><i style="width:40%"></i><b>40</b></div></div>`
  Each bar highlights and shows its `data-rc-tip` on hover.
- Rows: `<table class="rc-table" data-rc-sort>` sorts on a click or Enter in a
  heading (`data-rc-sort="text"` on one `th` compares it as text) and
  highlights the row under the pointer; in `.rc-scroll` its header sticks.
  `<ul class="rc-list"><li class="rc-item"><span>Name</span><span class="rc-badge rc-ok">Ready</span></li></ul>`
- Plans: `<ol class="rc-steps"><li class="rc-step rc-done">Built</li></ol>`, also `rc-active rc-failed rc-todo`.
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
- Variables: `--rc-text --rc-muted --rc-accent --rc-surface --rc-bg --rc-separator --rc-series-1` … `--rc-series-8`.

## Charts (Chart.js, bundled)

`<script src="rustcode-lib:chart.js"></script>`, then a chart in
`<div class="rc-chart"><canvas role="img" aria-label="…"></canvas></div>`
(`style="height:160px"` sizes it). Themed: series colours, hover tooltips, a
hovered bar a step lighter, legend toggles, a line filled from its colour down.
A canvas draws nothing with `CanvasText`, and its figures are not tabular, so
keep exact numbers in `.rc-num` text beside it.

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

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, aim under 300 px tall: relative widths, no `vh`.
- Useful before any interaction; a control's effect shows next to it.
- Real controls with `aria-label` where a label is missing; pre-aggregate.
- Hover is an extra, never the only way to read a value.

## Before you send

If `smart_ui_check` is listed, run it on the HTML, fix what it reports, then
write the fence.
