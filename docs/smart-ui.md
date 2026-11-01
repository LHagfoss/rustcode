# Smart UI

Smart UI ([#2484](https://github.com/LHagfoss/rustcode/issues/2484)) lets
the agent answer with a small interactive component, such as a filterable
table or a view driven by a few controls, that the macOS app runs in the
transcript. It is **experimental and off by default**: Settings › General ›
Chat › Smart UI. The engine learns the setting when it starts, so after
switching it Settings offers **Restart Engine** (shown once no chat is
running).

Native `rustcode-viz` widgets ([visualizations.md](visualizations.md)) stay the
default for charts, tables and trees. They run no code, and they keep working
whether Smart UI is on or off.

## Format

A fenced block whose info string is exactly `rustcode-ui v1`, holding one
self-contained HTML fragment:

````markdown
```rustcode-ui v1
<title>Slowest tests</title>
<meta name="description" content="Filter by name, click a heading to sort">
<style> table { width: 100%; } th { color: AccentColor; cursor: pointer; } </style>
<input id="q" placeholder="Filter"><table>…</table>
<script> /* plain JavaScript, data embedded */ </script>
```
````

- `<title>` is the card's heading (default "Interactive view"), and
  `<meta name="description">` is the line under it.
- Styles and scripts are inline. No framework, package or build step. System
  colors (`Canvas`, `CanvasText`, `AccentColor`) follow light and dark mode.
- Bundled libraries: `<script src="rustcode-lib:chart.js"></script>` is
  replaced with Chart.js 4.4.7 inline before the page loads
  (`SmartUI.expandingLibraries`). The code ships in the app
  (`macos/Sources/RustCodeKit/Vendor/ChartJS.swift`, MIT, with its license
  and checksum). Nothing is fetched and the policy is unchanged. It loads once
  per page, even with several tags. It doesn't count against the 64 KB limit,
  which applies to the block as written. An unknown name stays as written, and
  the policy blocks it.
- Limits: 64 KB of source and 5,000 elements. The view is 48–720 pt tall and
  fits its content; taller content scrolls inside the card.

Only assistant replies run blocks, and only while Smart UI is on. User
messages, tool output, `html` fences, a fence without `v1`, and a fence
with another version all show as text. A malformed, oversized, empty or
unfinished block shows its source with the reason. Copy and Show Source are
on every card.

## Teaching the agent

A client that runs blocks advertises `clientCapabilities._meta.rustcode.smartUi = 1`
at `initialize` while the setting is on. Sessions of that engine then get a
short `# Smart UI` section in the root system prompt: when to use Smart UI,
the fence name, and to load the `smart-ui` skill before writing one (83
estimated tokens, down from 139 when the format lived there; #2625).

The `smart-ui` skill is built into the engine
(`rustcode/engine/src/skills_builtin/smart-ui.md`, about 890 tokens) and is
listed with the other skills as `<built-in>/smart-ui`. It holds the format,
limits and design guidance: pick Markdown or `rustcode-viz` first, one compact
component per reply (the app runs one view at a time), sized for a narrow
column, system colors and fonts, accessible controls. A skill named
`smart-ui` in any skill root overrides it, and `disabled_skills` can turn it
off.

While the setting is off the app sends `smartUi = 0`. The root prompt then
carries a one-line note instead (under 80 tokens): Smart UI exists but is off,
so when the user asks for it the agent names Settings › General › Chat › Smart
UI rather than inventing a fence the app shows as source. A client that sends
no `smartUi` gets neither. It tells the agent to use Smart UI only when the user asks
for an interactive view or controls clearly help, and to use Markdown or
`rustcode-viz` otherwise. Subagent prompts and compact prompts never carry it.
The engine announces `agentCapabilities._meta.rustcode.smartUi = 1`.

## Runtime and lifecycle

`SmartUIHost` (`macos/Sources/RustCode/SmartUIView.swift`) owns **at most
one** web view:

- No web view or WebContent process exists until a block runs. A block runs
  automatically when it finishes streaming in front of you. Blocks in a chat
  you open later show a **Run interactive view** button.
- Running another block tears the current page down and starts a new web
  view. Reloading pages into one web view grows its WebContent process
  ([preview.md](preview.md#measurements) measured 78 → 616 MB). A new view
  lets the old process exit.
- A page out of sight for 30 s is torn down. The Stop button tears it down
  at once.
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
| Native bridge | None. No script message handler is installed. The app reads the page's height and element count by evaluating one fixed expression every 0.5 s. |
| Runaway pages | The page is stopped and its source shown when it does not finish loading in 5 s, when the height poll goes 3 s without an answer (a busy loop), when it builds more than 5,000 elements, or when its WebContent process exits (WebKit's memory limit or a crash). |
| Provenance | Blocks run only from assistant replies, with the explicit versioned info string, and only while the setting is on. While it is off, a block shows as a card with its title and a **Turn On Smart UI** button instead of its source (#2536). |

The rules were checked against real WebKit (see Measurements). From a page,
`fetch`, an external `<img>`, a `WebSocket` and `window.open` were all
blocked, `location` changes to `https:` and `file:` left the page on
`about:blank`, cookies stayed empty, and no message handler was exposed.

Residual risks: the page can use CPU until the watchdog's next poll, and up to
WebKit's own memory limit. It can draw anything inside its card, including
text that imitates the app. It cannot reach anything outside the card.

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
- **One reusable web view (this design)**: about 30 MB while a block runs and
  nothing otherwise, about 60 ms to show, any interaction. It costs one
  WebContent process at a time.
- **One web view per block**: rejected. The memory cost scales with the
  number of blocks in a transcript.
- **Tailwind or another CSS library**: not bundled. A prebuilt Tailwind file
  is tens of KB, added to every page and outside the agent's control. System
  colors and a few lines of CSS were enough for the samples.
- **Chart.js**: bundled (205 KB, inlined only into pages that name it).
  Hand-drawn SVG charts were the weakest part of agent-written pages, and
  T3 Code's agents get chart libraries from CDNs. Ours can't reach a CDN, so
  the library ships with the app. d3 isn't bundled: it is larger and harder
  for an agent to use well in one block.

Not measured: hours of real use under Instruments, and scrolling smoothness
with a live card. The rendered card was checked in the debug app, but its
controls were not clicked with a real pointer: the debug driver's synthesized
clicks do not reach WebKit.

## Recommendation

**Ship it limited**: opt-in, experimental, a single live view, with the
controls above. The cost is bounded and paid only when a block runs, and the
isolation held in every check. Keep it off by default until it has been used
for a while with real pointer input and longer sessions.
