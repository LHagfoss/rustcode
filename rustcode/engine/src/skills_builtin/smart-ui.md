---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive component for the RustCode chat. Use when the prompt's Smart UI section says it is on and the user asks for an interactive view, or controls would help them explore the answer, even unasked.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, html, chart, slider, filter, sort, toggle
---

# Smart UI

## When

Default to a view for anything structured: a table, comparison, breakdown,
status overview, plan, numbers, a chart or a dashboard, asked or not. Prose, a
single fact, a few bullets, code and diffs stay Markdown. Put the finding in a
sentence before the view. A report to another thread stays plain Markdown.

## Format

````markdown
```rustcode-ui v1
<title>Test runs</title>
<div class="rc-stats"><div class="rc-stat"><span class="rc-stat-value">128</span><span class="rc-stat-label">Runs</span></div></div>
```
````

`<title>` labels the view; do not repeat it. Inline `<style>` and `<script>`
only, data embedded; no network, imports, storage or navigation.
Limits: 64 KB source, 5,000 elements.

## Kit (already on the page)

Set no colours and write as little CSS as you can.

- Text: `.rc-title .rc-lead .rc-label .rc-muted .rc-mono .rc-num`.
- Numbers: `.rc-stats` > `.rc-stat` > `.rc-stat-value .rc-stat-label .rc-stat-note`.
- Status: tints `.rc-ok .rc-warn .rc-bad .rc-info` on `.rc-badge .rc-callout`,
  a stat, a bar or a line in `.rc-term`. `<div class="rc-progress"><span style="width:72%"></span></div>`
- Bars: `<div class="rc-bars"><div class="rc-bar"><span>Name</span><i style="width:40%"></i><b>40</b></div></div>`
- Rows: `<table class="rc-table">` (in `.rc-scroll` its header sticks);
  `<ul class="rc-list"><li class="rc-item"><span>Name</span><span class="rc-badge rc-ok">Ready</span></li></ul>`
- Plans: `<ol class="rc-steps"><li class="rc-step rc-done">Built</li></ol>`, also `rc-active rc-failed rc-todo`.
- Folding: `<details class="rc-details"><summary>Files</summary>…</details>`;
  `.rc-tree` around nested ones, `.rc-file` rows of name and value.
- Output: `<div class="rc-term"><div>$ cargo test</div><div class="rc-ok">ok</div></div>`
- Citation: `<span class="rc-tip" tabindex="0">1<span>Source text</span></span>`
- Tabs: `<div class="rc-tabs"><div role="tablist"><button role="tab" data-rc-tab="a">A</button></div><section data-rc-panel="a">…</section></div>`
- Actions, no script: `<button class="rc-chip" data-rc-send="Show X">Show X</button>`
  sends a follow-up; `data-rc-copy="text"` copies and says Copied.
- Layout: `.rc-row .rc-col .rc-grid .rc-gap-1` … `.rc-gap-4 .rc-card .rc-scroll`.
- Variables: `--rc-text --rc-muted --rc-faint --rc-accent
  --rc-surface --rc-bg --rc-separator --rc-danger --rc-success --rc-warning
  --rc-font --rc-mono --rc-radius --rc-series-1` … `--rc-series-8`.
- Charts: Chart.js 4 via `<script src="rustcode-lib:chart.js"></script>`, in
  `<div style="height:200px"><canvas role="img" aria-label="…"></canvas></div>`.
  Themed already; a canvas draws nothing with `CanvasText`.
- `window.rustcode`: `send(text)` only from a click or key press, `copy(text)`,
  `theme`, `onThemeChange(callback)`.
- Older apps have no runtime: call `window.rustcode?.send(...)`, use
  `var(--rc-muted, GrayText)`.

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, aim under 300 px tall: relative widths, no `vh`.
- Useful before any interaction; a control's effect shows next to it.
- Real `<button>`, `<select>`, `<input>`, `<label>`; `aria-label` where a
  control has no visible label. Pre-aggregate; no thousands of rows.

## Before you send

When a `smart_ui_check` tool is listed, run it on the HTML, fix what it
reports, then write the fence with that HTML.
