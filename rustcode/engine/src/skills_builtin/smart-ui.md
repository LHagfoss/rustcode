---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive view for the RustCode chat. Use when the prompt says Smart UI is on and a table, comparison, plan, numbers or controls would help, asked or not.
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

`<title>` labels the view; do not repeat it. Optional inline `<style>` and
`<script>`, data embedded; omit either when empty. No network, imports, storage
or navigation. Limits: 64 KB, 5,000 elements. The app draws it as it streams: lead
with the part worth reading.

## Kit (already on the page)

Set no colours and write as little CSS as you can.

- Text: `.rc-title .rc-lead .rc-label .rc-muted .rc-mono .rc-num`.
- Numbers: `.rc-stats` > `.rc-stat` > `.rc-stat-value .rc-stat-label`.
- Status: tints `.rc-ok .rc-warn .rc-bad .rc-info` on `.rc-badge .rc-callout`,
  a stat, a bar or a line in `.rc-term`. `<div class="rc-progress"><span style="width:72%"></span></div>`
- Bars: `<div class="rc-bars"><div class="rc-bar"><span>Name</span><i style="width:40%"></i><b>40</b></div></div>`
- Rows: `<table class="rc-table">` (in `.rc-scroll` its header sticks);
  `<ul class="rc-list"><li class="rc-item"><span>Name</span><span class="rc-badge rc-ok">Ready</span></li></ul>`
- Plans: `<ol class="rc-steps"><li class="rc-step rc-done">Built</li></ol>`, also `rc-active rc-failed rc-todo`.
- Folding: `<details class="rc-details"><summary>Files</summary>…</details>`;
  `.rc-tree` around nested ones, `.rc-file` rows of name and value.
- Output: `<div class="rc-term"><div>$ cargo test</div><div class="rc-ok">ok</div></div>`
- Citation: `<span class="rc-tip" tabindex="0">1<span>Source</span></span>`
- Tabs: `.rc-tabs` > `[role=tablist]` > `button[role=tab][data-rc-tab=a]` and `section[data-rc-panel=a]`.
- Actions, no script: `data-rc-send="Show X"` sends a follow-up, `data-rc-copy`
  copies and says Copied. A form's named fields ride along:
  `<form data-rc-send="Apply filters"><select name="range">7d</select>
  <button>Apply</button></form>` sends "Apply filters", then `range: 7d`.
- Layout: `.rc-row .rc-col .rc-grid .rc-gap-1` … `.rc-gap-4 .rc-card .rc-scroll`.
- Variables: `--rc-text --rc-muted --rc-accent --rc-surface --rc-bg
  --rc-separator --rc-series-1` … `--rc-series-8`.
- Charts: Chart.js 4 via `<script src="rustcode-lib:chart.js"></script>`, in
  `<div style="height:200px"><canvas role="img" aria-label="…"></canvas></div>`.
  Themed; a canvas draws nothing with `CanvasText`.
- `window.rustcode`: `send(text, context?)` only from a click or key press
  (`context`, an object, follows as a JSON fence), `copy(text)`, `theme`,
  `onThemeChange(callback)`.
- Older apps have no runtime: `window.rustcode?.send(...)`,
  `var(--rc-muted, GrayText)`.

## Design

- No outer card, border, background or title banner. One view per question.
- Fits a 320 px column, aim under 300 px tall: relative widths, no `vh`.
- Useful before any interaction; a control's effect shows next to it.
- Real controls with `aria-label` where a label is missing; pre-aggregate.

## Before you send

If `smart_ui_check` is listed, run it on the HTML, fix what it reports, then
write the fence.
