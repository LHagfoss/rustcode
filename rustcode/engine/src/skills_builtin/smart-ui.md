---
name: smart-ui
description: Write a Smart UI (rustcode-ui v1) interactive component for the RustCode macOS chat. Use only when the prompt's Smart UI section says it is on and the user asks for an interactive view, or sorting, filtering, sliders or toggles clearly help.
triggers: smart ui, interactive view, rustcode-ui
keywords: interactive, widget, component, html, slider, filter, sort, toggle
---

# Smart UI authoring

Smart UI runs one small HTML component inside a chat reply. It is for
interaction the reader actually uses, not for decoration.

## Pick the simplest form first

1. **Prose or Markdown** (lists, a short table) when the answer is read once.
2. **`rustcode-viz`** for a chart, table or tree that needs no controls. It is
   native, runs no code and works with Smart UI off.
3. **Smart UI** only when a control changes what the reader sees: filter or
   sort many rows, drag a slider to explore a formula, toggle series,
   step through states.

If you cannot name the control and what it changes, do not write Smart UI.

## Size it to the task

- One component per reply. The app runs one interactive view at a time; an
  earlier one in the chat stops and shows a Run button when a later one
  starts. For several independent views, give the interactive one Smart UI
  and the others `rustcode-viz` or Markdown.
- Aim for under 300 px tall and a handful of controls. The card is the chat
  column's width (it can be narrow, ~320 px) and 48–720 pt tall; taller
  content scrolls inside it.
- No app chrome: no navigation bars, tabs of unrelated content, splash
  headers, footers, logos or instructions the controls already make obvious.
- Show the result of a control next to it. Keep the first state useful
  without any interaction.
- Keep data to what the view needs. Limits: 64 KB of source, 5,000 elements.
  Pre-aggregate large data; never embed thousands of table rows.

## Format

A fenced block whose info string is exactly `rustcode-ui v1`:

````markdown
```rustcode-ui v1
<title>Slowest tests</title>
<meta name="description" content="Filter by name, click a heading to sort">
<style>
  body { font: 13px -apple-system, system-ui; color: CanvasText; background: Canvas; margin: 0; }
  input { width: 100%; box-sizing: border-box; }
  th { color: AccentColor; cursor: pointer; text-align: left; }
</style>
<input id="q" placeholder="Filter" aria-label="Filter tests">
<table id="t"></table>
<script>
  const rows = [["parse_config", 812], ["load_session", 640]];
  /* plain JavaScript that renders rows and wires the controls */
</script>
```
````

- `<title>` becomes the card heading; `<meta name="description">` the line
  under it. Both short.
- Inline `<style>` and plain `<script>` only. No frameworks, imports, CDN,
  fetch, external files, storage or navigation: all are blocked. Embed the
  data.
- Use system colors (`Canvas`, `CanvasText`, `AccentColor`, `GrayText`) and
  the system font so it matches light and dark mode and the transcript.
  No fixed background colors, gradients or shadows.
- Fit any width: relative widths, `flex-wrap`, no horizontal scrolling.
- Accessible: real `<button>`, `<input>`, `<select>` and `<label>`
  elements, `aria-label` where a control has no visible label, keyboard
  operable, visible focus. Respect `prefers-reduced-motion`.

## Before you send

- When the `smart_ui_preview` tool is offered, render the block's HTML with
  it first. Check the screenshot at the card's width and read any page
  errors; fix them, then write the fence with the HTML you checked.
- Is a control doing real work? Otherwise use `rustcode-viz` or Markdown.
- Is it one compact component, readable at 320 px wide?
- Does the reply still make sense if the view does not run? Put the key
  finding in a sentence before the block.
