---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive component for the RustCode chat. Use when the prompt's Smart UI section says it is on and the user asks for an interactive view, or controls would help them explore the answer, even unasked.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, html, chart, slider, filter, sort, toggle
---

# Smart UI

One small HTML component inside a chat reply, reading like the rest of the
reply: no card, border or background of its own.

## When

- Read once: Markdown. A quick plain chart, table or tree: `rustcode-viz`.
- Smart UI when the user asks for a visualization, a graph, a dashboard or
  anything you would design: give it a real design, a short title, the finding
  in a sentence and big numbers where the point is a number.
- Also when a control changes what the reader sees (filter, sort, slider,
  toggle, step) or a button asks a follow-up.
- One component per question. Put the key finding in a sentence before it.

## Format

````markdown
```rustcode-ui v1
<title>Kristiansand, next 3 days</title>
<label>Show <select id="m"><option value="0">Temperature</option><option value="1">Wind</option></select></label>
<div style="height:200px"><canvas id="c" role="img" aria-label="Forecast"></canvas></div>
<button id="d">Tuesday by hour</button>
<script src="rustcode-lib:chart.js"></script>
<script>
const series = [["°C", [8, 10, 7]], ["m/s", [5, 9, 4]]];
const chart = new Chart(document.getElementById("c"), {type: "line",
  data: {labels: ["Mon", "Tue", "Wed"], datasets: [{label: series[0][0], data: series[0][1]}]},
  options: {maintainAspectRatio: false}});
document.getElementById("m").onchange = (e) => {
  const [label, data] = series[e.target.value];
  Object.assign(chart.data.datasets[0], {label, data}); chart.update();
};
document.getElementById("d").onclick = () => window.rustcode?.send("Show Tuesday by hour");
</script>
```
````

- `<title>` labels the view; do not repeat it as a heading.
- Inline `<style>` and `<script>` only, data embedded. Network, imports,
  storage and navigation are blocked. Limits: 64 KB source, 5,000 elements.

## Runtime (already on the page)

- Headings, tables, inputs, buttons and code are already styled with the
  app's typography, so write little CSS.
- CSS variables: `--rc-text --rc-muted --rc-faint --rc-accent --rc-surface
  --rc-separator --rc-danger --rc-success --rc-warning --rc-font --rc-mono
  --rc-radius --rc-series-1` … `--rc-series-8`.
- Classes: `.rc-row` `.rc-col` (flex), `.rc-grid`, `.rc-gap-1` … `.rc-gap-4`,
  `.rc-muted`, `.rc-mono`, `.rc-num` (tabular figures), `.rc-card` (an inner
  group), `.rc-scroll`.
- Chart.js 4 (global `Chart`) via `<script src="rustcode-lib:chart.js">`.
  `Chart.defaults` already use the variables, so set no colours. If you must,
  read one: `getComputedStyle(document.documentElement).getPropertyValue('--rc-accent')`.
  Never give a canvas a system colour (`CanvasText`, `AccentColor`): it draws
  nothing.
- `window.rustcode`: `send(text)` sends `text` as the user's next message,
  only from a click or key press (follow-ups such as "Show details for X");
  `copy(text)`; `theme` (`"light"` or `"dark"`); `onThemeChange(callback)`.
- Older apps have no `rustcode` object and no runtime: call
  `window.rustcode?.send(...)`, give variables a fallback
  (`var(--rc-muted, GrayText)`), and keep the view readable without classes.

## Design

- No outer card, border, background, shadow or title banner.
- Compact (aim under 300 px tall) and fits a 320 px column: relative widths,
  wrapping rows.
- The first state is useful without interaction; a control's effect shows
  next to it.
- Accessible: real `<button>`, `<select>`, `<input>` and `<label>`,
  `aria-label` where a control has no visible label, keyboard operable.
  Respect `prefers-reduced-motion`.
- Pre-aggregate; never embed thousands of rows.

## Before you send

When a `smart_ui_check` tool is listed, run it on the HTML, fix what it
reports, then write the fence with that HTML.
