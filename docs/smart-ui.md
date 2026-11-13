# Smart UI

> The SwiftUI app this was first built for has been removed. Paths under
> `macos/` below describe that host and are kept for history; the desktop
> app's host is `apps/desktop/src/ui/SmartUIView.tsx` with `src/smartui/`.

Smart UI ([#2484](https://github.com/LHagfoss/rustcode/issues/2484)) lets
the agent answer with a small interactive component, such as a filterable
table or a view driven by a few controls, that the client runs in the
transcript of the desktop app (`apps/desktop`). A view reads
like the rest of the reply: no card chrome, border or background of its own. It is **on by default** (it shipped opt-in, and agents almost
never reached it): Settings › Chat › Smart UI turns it off. The engine learns the setting when it starts, so after
switching it Settings offers **Restart Engine** (shown once no chat is
running).

While it is on, a view is the default for anything structured: a table,
comparison, breakdown, status overview, plan, numbers, a chart or a dashboard,
asked or not. The page ships a component kit (below), so views look consistent
and agents write little CSS. Native `rustcode-viz` widgets
([visualizations.md](visualizations.md)) are what agents are taught while Smart
UI is off or the client has none; a block already written keeps rendering
either way.

## Format

A fenced block whose info string is exactly `rustcode-ui v1`, holding one
self-contained HTML fragment:

````markdown
```rustcode-ui v1
<title>Slowest tests</title>
<meta name="description" content="Filter by name, click a heading to sort">
<style> table { width: 100%; } th { color: var(--rc-accent, AccentColor); cursor: pointer; } </style>
<input id="q" placeholder="Filter"><table class="rc-num">…</table>
<script> /* plain JavaScript, data embedded */ </script>
```
````

- `<title>` labels the view (the macOS card's heading, default "Interactive
  view"), and `<meta name="description">` is the line under it. The body
  does not repeat the title.
- Styles and scripts are inline, and both are optional: most views need no
  `<script>`, and one that only uses the kit needs no `<style>` either. An
  empty `<style></style>` or `<script></script>` is dropped from the page before
  it runs (`stripEmptyTags`), since agents that write one by habit pay for the
  bytes. No framework, package or build step. Colors come from the runtime's
  variables (below), with system colors as the fallback for clients without the
  runtime.
- Bundled libraries: `<script src="rustcode-lib:chart.js"></script>` is
  replaced with Chart.js 4.4.7 inline before the page loads
  (`SmartUI.expandingLibraries`). The code ships in the app
  (`macos/Sources/RustCodeKit/Vendor/ChartJS.swift`, MIT, with its license
  and checksum). Nothing is fetched and the policy is unchanged. It loads once
  per page, even with several tags. It doesn't count against the 64 KB limit,
  which applies to the block as written. An unknown name stays as written, and
  the policy blocks it.
- Limits: 64 KB of source and 5,000 elements. The view is at least 48 pt
  tall and grows to its content's full height; it scrolls only where the view
  asks, with `.rc-scroll` (320 pt) or its own overflow.

Only assistant replies run blocks, and only while Smart UI is on. User
messages, tool output, `html` fences, a fence without `v1`, and a fence
with another version all show as text. A malformed, oversized, empty or
unfinished block shows its source with the reason, except that an unfinished
block whose reply is still streaming in front of the reader is drawn as it
arrives (see Streaming below). Copy and Show Source are
on every card.

## Streaming

The desktop app draws a view while its fence is still arriving, so a long view
is not a blank "Building an interactive view…" for a whole turn.

- The fragment the app has so far is closed before it is drawn
  (`closeOpenTags`, `apps/desktop/src/smartui/parse.ts`): a closing tag for each
  element still open, a half-written tag dropped rather than shown as text, an
  unfinished `<style>` closed after its partial rules, an unfinished comment
  cut. One pass over at most 64 KB, so it costs nothing worth measuring.
- The app starts one frame per streaming block from `buildStreamPage`: the same
  policy, variables, stylesheet and runtime as a finished page, plus an empty
  `#rc-stream` that `stream` messages fill. Injected HTML never runs a script,
  so the view's own script waits for the fence to close, and a half-written
  view cannot loop. The delegated kit behaviour (tabs, sends) does work, since
  it comes from the runtime.
- Updates are coalesced to at most one per animation frame and one per 100 ms
  (`StreamPump`), and the last fragment wins: a fast stream costs a few paints
  a second, not one per token.
- The frame reports its content height like any page, but the app only ever
  grows it while streaming, so what follows in the transcript does not jump as
  a table fills in. Nothing is remembered for a partial source; the height is
  remembered for the finished one.
- The element limit is enforced on the finished page, not on the preview.
- When the fence closes, the finished page replaces the preview in the same
  frame slot: Chart.js inlined, the script run once, the height kept. Anything
  typed into the preview is lost with it, since a preview runs no script.
- A block that names `rustcode-lib:chart.js` is **not** previewed: a preview
  runs no script and carries no library, so its canvases could only be empty
  boxes where the chart is the content. It shows the one-line "Building an
  interactive view…" until its finished page is ready (`previewable`). A
  canvas in any other preview is a quiet box of the same size, not a blank one.

The limits and the sandbox are the same for both: 64 KB, 5,000 elements, an
opaque origin, and the policy in [Security model](#security-model).

## Runtime

The client puts these on every page before the fragment. The names are a
contract between the engine's `smart-ui` skill and the clients; change them
in both.

| Part | What the page gets |
| --- | --- |
| Base | A transparent body in the app's font and text color. The fragment is wrapped in `#rc-root`, which spaces one block from the next by 16 px (28 px inside `.rc-dash`), so a view needs no margins of its own. Headings, paragraphs, tables, inputs, selects, buttons and code are already styled like the chat, so a view needs little CSS. |
| CSS variables on `:root` | `--rc-text`, `--rc-bg`, `--rc-muted`, `--rc-faint`, `--rc-accent`, `--rc-accent-text`, `--rc-surface`, `--rc-separator`, `--rc-danger`, `--rc-success`, `--rc-warning`, `--rc-font`, `--rc-mono`; the type scale `--rc-text-size`, `--rc-text-size-sm`, `--rc-text-size-xs`, `--rc-text-size-title`, `--rc-text-size-value`; the shape `--rc-radius`, `--rc-radius-sm`, `--rc-radius-xs`, `--rc-radius-pill`; and `--rc-series-1` … `--rc-series-8`, whose first colour is the accent. All of them are read from the app's tokens on every platform (`pageTheme`), so a theme's radius, font, accent and text sizes reach the view; they follow light and dark mode. |
| Layout classes | `.rc-row` and `.rc-col` (flex row / column, wrapping), `.rc-grid` (auto-fit grid), `.rc-gap-1` … `.rc-gap-4`, `.rc-muted`, `.rc-mono`, `.rc-num` (tabular figures, right-aligned in tables), `.rc-card` (a quiet inner group, not an outer frame), `.rc-scroll` (bounded, scrolls inside). |
| Kit: text and numbers | `.rc-title`, `.rc-lead` (the finding, muted), `.rc-label`; `.rc-stats` (auto-fit grid) of `.rc-stat` with `.rc-stat-value`, `.rc-stat-label`, `.rc-stat-note`. The headline layout: `.rc-head` (a title and a muted caption pushed to the right), `.rc-hero` (a `.rc-label`, a huge `.rc-hero-value`, an accent `.rc-hero-delta`, then a `<p class="rc-lead">` whose `<b>` is bold), `.rc-stats.rc-flat` (borderless stats whose `.rc-stat-note` is the subline), and `.rc-dash` around them all for the wider rhythm. |
| Kit: status | `.rc-badge`, `.rc-callout`, tinted by `.rc-ok`, `.rc-warn`, `.rc-bad`, `.rc-info` (also on a stat, a bar, a progress bar or a line in `.rc-term`); `.rc-progress` around `<span style="width:72%">`. |
| Kit: bars, tables, lists | `.rc-bars` of `<div class="rc-bar" data-rc-tip="7 failed"><span>Name</span><i style="width:40%"></i><b>40</b></div>`, a bar chart with no canvas; each bar highlights on hover and shows its `data-rc-tip`, and `table.rc-table[data-rc-sort]` sorts on a heading (below); `table.rc-table` keeps its header when it is inside `.rc-scroll`, and elsewhere the runtime wraps it in a box that scrolls sideways on a narrow window; `.rc-list` of `.rc-item`: each item is a label row with an optional trailing value and may carry a description block (`div` or `p`). |
| Kit: traces and folding | `.rc-steps` of `li.rc-step` with `.rc-done`, `.rc-active`, `.rc-failed` or `.rc-todo`; `details.rc-details` with a chevron drawn in CSS, `.rc-tree` for nested ones, `.rc-file` rows; `.rc-term`, a terminal block. |
| Kit: sources | `<details class="rc-details rc-sources"><summary>Sources</summary><ol><li><a href="https://…">Title</a></li></ol></details>`, last in the view and closed: a quiet fold whose summary the runtime gives the number of links, with each link's site (`www.` dropped) after it in faint text unless the link text already is the site. |
| Links | A link never navigates the frame, which would replace the view. A click on an `http(s)` or `mailto:` link posts `open` to the app, which opens it in the browser (`openExternal`) only during a user gesture with focus in the view, at most once a second; any other link does nothing. |
| Kit: actions and citations | `.rc-chip`, a pill follow-up button; `.rc-tabs`; `<span class="rc-tip" tabindex="0">1<span>Source</span></span>`, opening on hover and focus inside the page's width. |
| Kit: behaviour, no script | One delegated listener: click, Enter or Space on a `th` of `table.rc-table[data-rc-sort]` sorts that column in place (numbers by value, otherwise the locale's collation; `data-rc-sort="text"` on a `th` forces text), `aria-sort` marks the column and its direction, and the rows are moved, not rebuilt, so ids and controls in them survive. Every `.rc-spark[data-values="1,4,2,8"]` gets an inline SVG line from its numbers (`.rc-spark-line`, `.rc-spark-area`), once, and again after a streamed update. Then: `data-rc-send="text"` calls `rustcode.send`, with the same gesture check; on a `<form data-rc-send="text">`, or on a control inside one, the form's named fields are sent after the text as `name: value` lines (a form sends when submitted, never on a click into it, and never navigates). `data-rc-copy="text"` calls `rustcode.copy`, and the element says "Copied" for 1.5 s. `<div class="rc-tabs"><div role="tablist"><button role="tab" data-rc-tab="a">A</button></div><section data-rc-panel="a">…</section></div>`: the first tab is active at load, click and arrow keys switch, and `aria-selected` and `hidden` follow. |
| Height | The page reports its content, not the document: the in-flow `#rc-root`'s height plus its margins. A hover overlay (`.rc-tip > span`, a bar's `data-rc-tip`) is `position: absolute` and only revealed with `visibility`, so it never changes that number; The document never scrolls (`html { overflow: hidden }`) and the body has no cap, so a scrollbar cannot appear, narrow the layout and re-wrap the text under the pointer: the frame grows to the content's full height, never below 48 px. Only `.rc-scroll` (320 px) or the view's own overflow scrolls. The page also says when the pointer is in it, and the app holds a measurement taken then until it leaves. A measurement within a second of a press in the page applies at once: the page marks it `interacted`, since an arrow key that switches a tab grants the app no user activation, and a fold that opens or closes reports on its `toggle`, so moving the pointer over a view never moves the chat below it. Collapsing a `<details>` or moving to a shorter tab shrinks the frame. A page sized to the frame (`html, body { height: 100% }`) still cannot shrink, and stays at the reserved height. |
| Chart.js | `<script src="rustcode-lib:chart.js"></script>` as before. `Chart.defaults` are themed from the variables (text, grid lines, font, the eight series colours), so agents set no colours; a page that needs one reads it with `getComputedStyle(document.documentElement).getPropertyValue('--rc-…')`. Tooltips appear on hover with a dot in each series' own colour, a bar under the pointer is a step lighter, the legend toggles a series on click, and the chart sizes to its box (200 px, or 140/260 with `data-size=\"sm\"/\"lg\"/\"xl\"` for 140/260/340). Axes are quiet: no axis line or ticks, faint horizontal rules, no vertical ones on a category axis. A line is filled from its colour down (`.rc-chart`). Nothing is tweened (`animation: false`): eased tooltips and hover points trailed the pointer and read as slow and jagged, so a hover, a legend toggle or a revealed chart answers at once. A chart in a tab or a fold that was hidden when it was built is resized the moment it is shown, before the frame paints, instead of a frame later by Chart.js's own observer. Colour changes are applied at once rather than tweened: Chart.js tweens colours through interpolators keyed by type, and a canvas gradient is an object with none, so the animator threw `this._fn is not a function` on its first tick and the chart never drew. `options.plugins.rcMarks` draws on a line chart: `band: [low, high]` shades a range of the y scale, `line: 150` with `lineLabel: "normal"` draws a reference line, and `end: true` (or a text) puts a dot and the value on the last point. System colour keywords (`CanvasText`, `AccentColor`) work in CSS but not on a canvas, which draws nothing with them. |
| `window.rustcode` | `send(text, context?)`: sends `text` as the user's next message in the thread, for follow-ups such as "Show details for X"; `context`, a JSON-serialisable object, follows the text as a fenced `json` block, so a view sends values back structured rather than in prose. The whole message is capped at 2,000 characters, and it works only inside a click or key press handler. `copy(text)`: copies to the clipboard. `theme`: `"light"` or `"dark"`. `onThemeChange(callback)`: called with the new theme. |

### Charts, presets

Chart.js draws on a canvas, so the DOM's figures are tabular (`.rc-num`,
`.rc-table`, `.rc-stat-value`) and a canvas's are not; a number the user scans
belongs in text beside the chart. The four shapes agents ask for, all themed
without a line of CSS:

- **Line with an area fill** is the default: `type: "line"`, a dataset per
  series, and the runtime fills it with a gradient of its own colour.
- **Stacked bar**: `type: "bar"` with
  `options: { scales: { x: { stacked: true }, y: { stacked: true } } }`.
- **Donut with a centre total**: `type: "doughnut"`, and inside the same
  `.rc-chart` a `<div class="rc-donut-total"><b>128</b><span>runs</span></div>`,
  positioned over the middle and drawn in the app's type (no script).
- **Sparkline**: `<span class="rc-spark" data-values="1,4,2,8"></span>`, no
  Chart.js at all, sized 64 x 20 px unless the span sets `width` and `height`.

Interaction follows the pointer without a script, and is never the only way to
read a value: the table row under the pointer is filled, a bar shows its
`data-rc-tip`, a sparkline is a picture of the numbers in its attribute, and a
sortable table keeps every row's text in place.

### Sending values back

The controls a view draws are only worth something if their state can reach the
thread. Two ways, and both go through `send`, so both carry the user's gesture,
the rate limit and the thread's own message list:

```html
<form data-rc-send="Apply these filters">
  <select name="range"><option>7d</option><option>30d</option></select>
  <button>Apply</button>
</form>
```

Submitting the form (its button, or Enter in a field) sends "Apply these filters", an empty line, then `range: 7d`: the
prompt the view wrote, then one `name: value` line per named, non-empty field
(unchecked boxes and unnamed controls are left out). A control with
its own `data-rc-send` inside a form sends its text with that form's fields;
a click anywhere else in the form sends nothing; a `data-rc-send` on a plain control outside a form
sends its text alone, as before. The form never
navigates: the runtime prevents it, and the policy would cancel it.

From a script, the second argument of `send` is the structured form of the same
thing:

```html
<script>
  document.querySelector("#range").onchange = (e) =>
    window.rustcode?.send("Compare these ranges", { range: e.target.value });
</script>
```

That sends the text, then `{ "range": "30d" }` as a fenced `json` block, which
the agent reads as easily as prose and parses as easily as JSON. A context that
cannot be serialised is dropped and the text still goes; the whole message is
capped at 2,000 characters.

Which client has what:

| Client | Runtime (base, variables, classes, themed Chart.js) | `window.rustcode` |
| --- | --- | --- |
| Desktop app | Yes | Yes |
| macOS app | No: a transparent body with the system font and `CanvasText` only | No |

The skill tells the agent to call `window.rustcode?.send(...)`, give each
variable a fallback (`var(--rc-muted, GrayText)`) and keep the view readable
without the classes, so one block works in both.

## Teaching the agent

A client that runs blocks advertises `clientCapabilities._meta.rustcode.smartUi = 1`
at `initialize` while the setting is on. Sessions of that engine then get a
short `# Smart UI` section in the root system prompt: the rule (below), the
fence name, and to call `use_skill` with `smart-ui` directly before
writing one. Agents used to call `list_skills` first, a wasted round. It is 116
estimated tokens (`estimate_tokens`, capped at 150; 96 before the rule, 139
when the format lived there, #2625).

The rule, in both the root prompt and the harness text: "Default to a view for
anything structured: a table, comparison, breakdown, status overview, plan,
numbers, a chart or a dashboard, asked or not. Prose, a single fact, a few
bullets, code and diffs stay Markdown. Put the finding in one short line before
the view and nothing after it. Sources go last inside the view, folded:
details.rc-sources > ol > li > a[href]. A report to another thread
stays plain Markdown." Replies used to wrap a view in a paragraph before and a
recap, caveats and a Markdown source list after; the view now carries the
caveats and the sources, and the text around it is one line.

One tool per client state, so the rules never conflict. With the setting on,
the `# Visualizations` section and the output style's `rustcode-viz` sentence
are not sent at all, and the output style gains "A Markdown table only when the
reader will copy it; otherwise a view". That pays for the rule: the tail of the
root prompt these decide goes from 1,738 bytes / 436 tokens to 1,078 / 243.
With it off or unsupported the text is exactly what it was;
`smart_ui_off_and_unsupported_prompts_keep_their_exact_text` holds it byte for
byte against `rustcode/engine/src/tools/testdata/smart_ui_off_prompts.txt`. The
skill's `description` is listed in every client's prompt, so it stays short: it
gained forms and charts, and lost a longer sentence about when to reach for a
view (250 bytes before, 197 now).

The `smart-ui` skill is built into the engine
(`rustcode/engine/src/skills_builtin/smart-ui.md`, 2,188 estimated tokens
with its front matter; a test keeps it under 2,300) and is
listed with the other skills as `<built-in>/smart-ui`. It holds the rule, the
pick-by-question table (a trend is a line chart, parts of a whole a stacked bar
or donut, a ranking `.rc-bars`, exact values a table, a headline number
`.rc-stats`, and comparisons stay a table), the format, limits, the kit with
one short example per family, a Chart.js section with the four presets and what
a canvas cannot do, and design guidance: one view per question, no outer card,
border, background or repeated title, sized for a 320 px column, accessible
controls. It grew from 1,096 tokens when the pick rule and the chart section
were added; it is read once per view, not on every turn. A thread that built a
weather chart (2026-11-02, 7 rounds, 143 s) spent 3 of its 4 previews on a
blank Chart.js canvas because the skill then said to use system colors; the
themed `Chart.defaults` remove that step, and the presets say what a canvas
cannot do. A skill named `smart-ui` in any skill root overrides it, and
`disabled_skills` can turn it off.

While the setting is off the app sends `smartUi = 0`. The root prompt then
carries a one-line note instead (under 80 tokens): Smart UI exists but is off,
so when the user asks for it the agent names Settings › Chat › Smart
UI rather than inventing a fence the app shows as source. A client that sends
no `smartUi` gets neither. Subagent prompts and compact prompts never carry it.
The engine announces `agentCapabilities._meta.rustcode.smartUi = 1`.

### External harnesses

A thread on Codex, Claude Code or another external harness never sees the root
prompt and has no `use_skill`, so it learns Smart UI from
`harness_instructions` (`rustcode/engine/src/tools/schema.rs`), which Codex
takes as `developerInstructions` and a Claude Code adapter as its appended
system prompt. While the setting is on it carries a `# Smart UI` block with
what the skill would have given: the fence, inline `<style>` and `<script>`,
the rule, the kit's class names and the `data-rc-*` attributes with the
instruction to use them rather than write CSS, `rustcode-lib:chart.js`, the main
`--rc-*` variables, no card chrome, no `vh`. It carries no `rustcode-viz` types.
While the setting is off it carries the same off note as
the root prompt, so the agent points at Settings › Chat instead of inventing a
fence. A client that never advertised `smartUi` hears nothing about it.

The block is 1,982 bytes, and a test caps it under 2,000. It was 1,589; the
pick-by-question rule and the kit's chart, sparkline, sortable-table and hover
pieces cost about 390, paid on every turn of every external thread, so it is
written as tightly as the rule allows and the cap is a guard against drift.
Dropping the `rustcode-viz` types paid for the earlier growth: at
visualization level 2 the whole `harness_instructions` text goes from 3,386
bytes / 849 tokens to 3,529 / 935 with Smart UI on (the pick rule is here
because an external harness has no `use_skill` to read it from), against
2,206 / 547 with no Smart UI and no viz taught. The test in
`rustcode/engine/src/tools/tests.rs` caps the whole text at 4 KB.

## Agent preview (`smart_ui_preview`)

The agent used to write blocks blind. Now, while Smart UI is on, the app
advertises `smartUi = 2`, and the engine offers root sessions a
`smart_ui_preview` tool (`rustcode/engine/src/smart_ui.rs`) plus a short
`# Smart UI preview` prompt hint. The tool forwards the block body as a
`_rustcode/smartUi/preview` request. `SmartUIPreviewRenderer`
(`macos/Sources/RustCode/SmartUIPreviewRenderer.swift`) loads it in the same
sandbox configuration as the live view, at `SmartUI.previewWidth` (560 pt) in
a borderless window placed off every screen. It answers with:

- a screenshot, attached to the conversation after the tool batch like a
  `localhost_preview` one;
- the content height and the element count;
- what the page threw: `error` and `unhandledrejection` events,
  `console.error` and CSP violations, which a document-start script records.
  This script runs only in previews, never in the transcript's live view.

The preview page's base URL is `https://smart-ui-preview.invalid/`. With no
base URL the origin is opaque, and WebKit then reports every error as
"Script error.". The host never resolves, and the content rule list and
CSP still block every load. Renders run one at a time, each in a new web
view that is torn down when it answers. A load over 5 s, a page that doesn't
answer in 3 s (a busy loop) or more than 5,000 elements is a failed render
with its reason. Nothing is shown to the user, so no approval is asked.
Older apps (`smartUi = 1`) and other clients never see the tool.

Built for the macOS app, which has been removed: the desktop app answers
neither `_rustcode/smartUi/preview` nor `_rustcode/preview`, and advertises
`smartUi = 1`, so no client reaches `smartUi = 2` today and the tool is not
offered to any thread. Giving the card a real offscreen renderer in the
desktop app (`apps/desktop/src-tauri`) is the work that brings it back; then
the client advertises `2` and both the native tool and the MCP one below start
working.

## Block check (`smart_ui_check`)

What an external harness can have instead, without a renderer: while Smart UI
is on, the `rustcode` MCP server it already has lists `smart_ui_check`
(`rustcode/engine/src/smart_ui.rs`, `orchestration/mcp_server.rs`). The call
is answered by the MCP server itself, so it needs no app, no socket and no
approval, and it works on a thread whose client never advertised `smartUi` at
all. It reports:

- the block's byte size and about how many elements it builds (counted tags,
  not a DOM);
- a missing `<title>`, an unclosed `<script>` or `<style>`;
- a `rustcode-lib:` name the app does not bundle (only `chart.js` is);
- kit markup that does nothing as written: a `.rc-spark` without a
  `data-values` of at least two numbers, or `data-rc-sort` on something that is
  neither a table nor a heading;
- what the page cannot do, with the line: network loads, `fetch`,
  `XMLHttpRequest`, sockets, storage, `document.write`, `@import`, `vh` units,
  and `CanvasText`/`AccentColor` inside a script (a canvas draws nothing with a
  system colour);
- style the kit already sets, with the line: a colour written out (hex, `rgb()`,
  `hsl()`, or a `color`/`background`/`border-color` value that is not a
  `var(--rc-*)`), a `font-size` or `font-family`, a fixed `height` in px, and a
  `.rc-card` inside another `.rc-card`. A `<style>` block is fine when it holds
  only layout and variables; its theme values are not reported.

It does **not** run the page: no script errors, no fitted height, no
screenshot. It is a lint for the mistakes agents actually make, and the
instructions tell the harness to run it before writing the fence.

## Runtime and lifecycle

The host (`apps/desktop/src/ui/SmartUIView.tsx`, `src/smartui/host.ts`) runs
**at most six** pages at once (`MAX_LIVE`; #2484 ran one, then three):

- A view runs when it is seen. A complete block with Smart UI on and no problem
  starts by itself when it comes within 200 px of the viewport, including in a
  chat you open later, so history stays readable now that views replace tables.
  Until then it shows its title, muted, at the height it will need.
- The **Run interactive view** pill stays only for a block you pressed Stop on,
  or one that failed. Both stick after its row unmounts, for as long as the app
  runs: nothing starts it again until you press Run. A failure sticks so a busy
  loop does not stall the app again each time it scrolls into sight.
- The last measured height of each source is kept (the 200 newest), so a
  re-run or a return does not move the transcript.
- Each run gets a new frame. Starting a seventh page stops another: one out of
  sight if there is one, else the one that started longest ago. This is not a
  failure, so it shows its placeholder, and no source. **An evicted view does
  not start by itself again until it has left the viewport and come back**
  (`AutoRun`), or views in sight past the limit would keep evicting each other
  for ever. Its Run icon, on hover, still works. Reloading pages
  into one web view grows its WebContent process
  ([preview.md](preview.md#measurements) measured 78 → 616 MB), so a new
  frame per run lets the old one go.
- A page out of sight for 30 s is torn down, and runs again when it comes back.
  The Stop button tears it down at once.
- A page whose script throws is stopped with the reason in place of the view,
  like any failure, so a broken view never sits on screen as blank space. The
  runtime posts `error` for a `window.onerror` or an unhandled rejection, and
  the app shows what it said alongside the source. If the bundled library
  itself cannot be loaded, the view says so instead of guessing.
- While the page fits the card, wheel events pass to the transcript, so
  scrolling over a card scrolls the chat.

## Security model

Generated code is treated as untrusted, even when the assistant wrote it.

| Surface | Control |
| --- | --- |
| Ambient state | Non-persistent `WKWebsiteDataStore`, shared with nothing: no preview cookies, sign-ins or storage. The page origin is `about:blank`. |
| Network and files | CSP `default-src 'none'`, with only inline `script-src`/`style-src` and `data:` images and fonts. `connect-src`, `frame-src`, `worker-src`, `object-src`, `form-action` and `base-uri` are `'none'`. A content rule list also blocks every `http(s)`, `ws(s)`, `file`, `ftp` and `blob` load. A page's own CSP can only narrow the policy. |
| Navigation | Only the initial `about:blank` document in the main frame. Links, redirects, forms, frames and downloads are cancelled. |
| Windows and panels | `window.open` returns nothing. `alert`, `confirm` and `prompt` return at once without showing UI. File pickers return nothing, and camera, microphone and screen capture are denied. Fullscreen, AirPlay and media autoplay are off. |
| Native bridge | macOS app: none. No script message handler is installed; the app reads the page's height and element count by evaluating one fixed expression every 0.5 s. Desktop app: `window.rustcode` is a `postMessage` to the host carrying a per-frame token the host checks, so another frame cannot speak for the view. `send` is accepted only during a user gesture (a click or key press) and is rate limited; it can only add a user message to its own thread, which the user sees like any other. `copy` writes text only. `open` opens an `http(s)` or `mailto:` address in the browser, only during a user gesture with focus in the view, at most once a second, like a link in a Markdown reply. |
| Runaway pages | The page is stopped and its source shown when it does not finish loading in 5 s, when the height poll goes 3 s without an answer (a busy loop), when it builds more than 5,000 elements, or when its WebContent process exits (WebKit's memory limit or a crash). |
| Provenance | Blocks run only from assistant replies, with the explicit versioned info string, and only while the setting is on. While it is off, a block shows as a card with its title and a **Turn On Smart UI** button instead of its source (#2536). |

The rules were checked against real WebKit (see Measurements). From a page,
`fetch`, an external `<img>`, a `WebSocket` and `window.open` were all
blocked, `location` changes to `https:` and `file:` left the page on
`about:blank`, cookies stayed empty, and no message handler was exposed
(macOS app).

Residual risks: the page can use CPU until the watchdog's next poll, and up to
WebKit's own memory limit. A frame that shares the app's web process can stall
it while a busy loop runs; now that views run when seen, that can happen on
opening a chat, once per app run for each such block, since a failure sticks. It can draw anything inside its card, including
text that imitates the app. It cannot reach anything outside the card, except
that in the desktop app a click on it can send a message into the thread or
open a web link in the browser.

## Measurements

`macos/Tests/RustCodeTests/SmartUISoakTests.swift` runs the isolation
checks and a resource workload against the real host and WebKit:

    RUSTCODE_SMART_UI_SOAK=1 swift test --package-path macos --filter SmartUISoakTests

macOS 27.0.1 (26A434), Apple M5 Pro. The workload is a 200-row filterable,
sortable table, the size of a typical requested view. RSS comes from `ps`.

| Phase | Test process | WebContent |
| --- | --- | --- |
| Before any block runs | 89.7 MB | none |
| First view loaded (cold, 68 ms to loaded) | 90.1 MB | 30.5 MB |
| +10 s idle | 89.8 MB | 31.1 MB, 0.03 s CPU |
| Hidden +10 s | 89.7 MB | 31.0 MB, 0.01 s CPU |
| After 10 / 20 / 30 different views | 90.8 / 91.2 / 91.3 MB | 29.9 / 29.8 / 29.9 MB |
| Stopped | 91.1 MB | none |

A warm start averaged 61 ms. Over 30 views, 31 WebContent processes were used
and at most one older one was still exiting. After Stop, no process was left.
Memory stays flat because each view gets a new process instead of reloads
piling up in one.

Comparison:

- **Native `rustcode-viz`**: no process, no code. It draws only its six
  types, and its interactions are limited to hover, column sort and tree
  folding.
- **Up to six frames (this design)**: measured at about 30 MB per running block
  on the macOS host and nothing otherwise, about 60 ms to show, any
  interaction. At most about 180 MB by that figure; six was not measured on its
  own. The first design ran one, which made a
  reply or chat with several views stop all but the last.
- **One web view per block, unbounded**: rejected. The memory cost scales
  with the number of blocks in a transcript.
- **Tailwind or another CSS library**: not bundled. A prebuilt Tailwind file
  is tens of KB, added to every page and outside the agent's control. System
  colors and a few lines of CSS were enough for the samples.
- **Chart.js**: bundled (205 KB, inlined only into pages that name it).
  Hand-drawn SVG charts were the weakest part of agent-written pages, and
  agents elsewhere get chart libraries from CDNs. Ours can't reach a CDN, so
  the library ships with the app. d3 isn't bundled: it is larger and harder
  for an agent to use well in one block.

Not measured: hours of real use under Instruments, and scrolling smoothness
with a live card. The rendered card was checked in the debug app, but its
controls were not clicked with a real pointer: the debug driver's synthesized
clicks do not reach WebKit.

## Recommendation

It shipped limited: opt-in, experimental, a single live view, with the
controls above. The cost is bounded and paid only when a block runs, and the
isolation held in every check. Opt-in meant agents almost never used it, so
it is now on by default, and the prompt asks for visuals even unasked.
Another orchestrator saw the same thing: its rule "render a visual only when
asked" took inline charts from 8 of 8 eval cases to 0 of 8.
