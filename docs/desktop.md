# Desktop app (Tauri)

`apps/desktop` is the new RustCode desktop app: Tauri 2 with a React,
TypeScript and Tailwind UI. It is meant to replace the SwiftUI app in
`macos/` over time, with the same look and layout. Like the SwiftUI app, it
runs the engine as `rustcode --acp` and talks ACP to it, so every harness
(the built-in one, Codex, Claude Code, Gemini CLI, OpenCode, any ACP agent;
see [harnesses.md](harnesses.md)) runs a thread through the same UI.

Status: usable day to day. A chat for a thread on any harness with
approvals, attachments, link chips, Smart UI views, thread actions, find,
rewind, subagents, pull requests, plan usage and a terminal. Parts of the
SwiftUI app are not ported yet; see [Not yet ported](#not-yet-ported).

## Run it

Everything uses Bun from `apps/desktop`; nothing is installed globally.

```bash
cd apps/desktop
bun install

# The UI in a browser, against the real engine through a local bridge.
bun run dev:web            # http://localhost:1420

# The UI against a canned mock engine: no engine, no model turns.
bun run dev:mock           # or open http://localhost:1420/?transport=mock
                           # first run: ?transport=mock&scenario=onboarding

# The native app (starts Vite itself).
bun tauri dev

# A debug .app bundle with a debug engine sidecar (fast, for a smoke test).
bun run bundle:debug       # src-tauri/target/debug/bundle/macos/RustCode.app

# A release bundle with a release engine sidecar.
bun run bundle
```

The engine is found in this order: `$RUSTCODE_ENGINE`; in a debug build the
workspace's `target/debug/rustcode`; a `rustcode` next to the app binary (the
bundled sidecar); `rustcode` on `PATH`. Build the engine with
`cargo build -p rustcode-tui --bin rustcode` from the repository root.

The mock and `dev:web` never copy credentials; `dev:web` uses your real
`~/.config/rustcode`, exactly as the SwiftUI app does. Completing onboarding
or toggling a harness there writes your real config.

## Architecture

```
React UI (webview)  ──Transport──▶  Tauri shell (src-tauri)  ──stdio──▶  rustcode --acp
   state/store.ts                     engine.rs: spawn, relay,             (harness layer,
   state/thread.ts (reducer)          restart with backoff                  sessions, tools)
   acp/jsonrpc.ts, acp/types.ts       native.rs: links, git, login
```

- **Sidecar** (`src-tauri/src/engine.rs`). The shell starts `rustcode --acp`,
  relays each stdout line to the webview over a Tauri `Channel`, takes
  outgoing lines with the `engine_send` command, and forwards stderr as `log`
  events (Settings › Engine shows them). When the engine exits it restarts
  with backoff (0.5 s doubling to 30 s, reset after a minute up) and the UI
  initializes a fresh connection. Quitting closes the engine's stdin so it
  cleans up its sessions, then kills it after 2 s. The shell does not read or
  forward credentials; the webview sees only what the engine prints. The
  engine starts with the app, while the page loads; the first page adopts
  it. Lines already buffered on stdout go out together as one `lines`
  message (up to 1,024 lines or 4 MB).
- **Native helpers**, each validated in Rust:
  - `native.rs`: `open_url` (http, https, mailto only), `reveal_path`,
    `git_changes` (the Changes panel: `git diff HEAD` plus untracked files),
    `read_image` (an absolute path to a PNG, JPEG, GIF or WebP of at most
    20 MB, for attachments), `open_login_terminal`, which runs a
    harness's own login in Terminal and accepts only known harness tools,
    `open_path` (an existing folder, or a Markdown, TOML, JSON, YAML or
    text file, in its editor; nothing that runs) and `skill_create` (a
    valid skill name, user or project root, never overwriting).
  - `links.rs`: `link_meta`, a page's title, favicon (as a `data:` URL) and
    `theme-color` for link chips. http(s) only; the host must resolve to
    public addresses only (checked after DNS and pinned, so neither a
    redirect nor DNS rebinding reaches localhost or the LAN); redirects
    followed by hand (3, each re-checked); 3 s per request; 512 KB of HTML,
    64 KB of icon; no cookies. Cached in memory and in the app's cache
    folder for a week (failures for an hour).
  - `threads.rs`: `thread_rename` and `thread_trash`, done as the SwiftUI
    app does them: the folder comes from `rustcode threads path <id>` and
    must be that thread's folder in the store. Rename writes `title.txt`;
    Trash moves the folder to `~/.Trash` after `session/close`.
  - `pty.rs`: `pty_open|write|resize|close`, the login shell in a PTY
    (portable-pty) in an existing folder; at most 8; killed on quit.
  - `smartui.rs`: `smartui_page` and the `rcui://` scheme that serves a
    Smart UI page once, with its own CSP header (see Smart UI below).

  Capabilities allow only core APIs, window dragging and the folder picker.
  The app's CSP allows no remote content; `frame-src` allows only `rcui:`.
- **Transport** (`src/acp/transport.ts`): `TauriTransport` in the app,
  `WebSocketTransport` to `scripts/engine-bridge.ts` for `dev:web` (bound to
  127.0.0.1, accepts only the Vite origin, one engine per connection, same
  restart rules), and `MockTransport` (`src/acp/mock.ts`), an in-process
  fake engine with canned threads, a streamed turn with a plan, a running
  command, an edit approval and a question, the full harness contract,
  subagents, pull requests, usage limits, rewind and a Smart UI reply
  (a prompt with "view"; "agents" starts two subagents; "yolo" skips the
  approval). Native helpers go through `src/lib/shell.ts`: Tauri commands
  in the app, `scripts/bridge-native.ts` (the same rules in Bun, dev only)
  under `dev:web`, canned answers in the mock.
- **ACP client** (`src/acp/jsonrpc.ts`, `src/acp/types.ts`): a typed
  JSON-RPC peer and parsers ported from `RustCodeKit` (`ACPTypes.swift`,
  `ACPConnection.swift`, `FileDiff.swift`, `Subagents.swift`,
  `PullRequests.swift`, `PlanUsage.swift`; `src/acp/rustcode.ts`).
  `initialize` advertises no client fs or terminal; of the
  `_meta.rustcode` features, `smartUi` (1 while the setting is on, else 0),
  `visualizations: 2`, and `orchestration: 2` only while Settings ›
  General › *Threads can start and message other threads* is on.
- **Reducer** (`src/state/thread.ts`): pure functions fold `session/update`s
  into a thread: streamed reply and thought chunks, tool calls updated in
  place (output deltas, diffs, exit codes), plans, usage, config options,
  replayed turn timing, steers and the queue.
- **Store** (`src/state/store.ts`, Zustand): the engine controller. It drives
  sessions (`session/list|new|load|prompt|cancel|set_config_option`),
  `_rustcode/steer`, permission requests, the harness methods and the
  Settings requests. Components read slices with `useApp(selector)`.
- **Harness logic** (`src/state/harness.ts`): what can be chosen and why
  something is greyed out, from capability flags only, never a harness id.
  Before an engine implements `_rustcode/harnesses`, the app shows only the
  built-in harness and says so in Settings.
- **UI** (`src/ui/`): every colour, radius and shadow is a token from
  [design.md](design.md), defined once in `src/index.css` (the Tailwind
  `@theme` maps `--rc-*` variables to classes; the palette is switched
  off) and mirrored for themes in `src/lib/themes.ts`. `src/design.test.ts`
  fails on a raw colour, a palette class, an arbitrary radius or a retired
  token name outside those files, and when `index.css` and the built-in
  themes disagree. The transcript is virtualized with
  `@tanstack/react-virtual` and stays pinned to the bottom unless you scroll
  up. Markdown is `react-markdown` + `remark-gfm`; code is highlighted lazily
  by Shiki with Xcode's colours.

Keys: ⌘N new thread, ⌘K command palette (threads, and actions with `>`), ⌘, Settings, ⌘B sidebar, ⌘⌥B right panel, ⌘F find
in thread, ⌘J terminal, ⏎ send (or queue while a turn runs), ⌘⏎ steer the
running turn, ⇧⏎ newline, ⌫ at the start of the field takes back the last
link chip or the command token, Esc clears the draft and then stops the
turn (⌘. stops it too), 1–9 answer the visible approval or question.

## Features

- **Composer.** A compact rounded box (the field, then a row: **+** for
  attaching and new threads, the plan meter, the context ring, the model
  pill and send/queue/stop) with a lighter tray tucked under it: the
  project folder, *Worktree* (new threads), *Files*, agents working, and the
  harness only when it is not the default one. A slash command from `available_commands_update` that
  takes arguments becomes a token in front of the field (picked from the
  popup or typed as `/name `). Images and files can be pasted, dropped or
  picked: images are ACP `image` blocks when the thread's harness takes
  them (`capabilities.images`, or `promptCapabilities.image` on the
  built-in one); files with a path go as an "Attached files" list on the
  built-in harness (it ignores `resource_link`) and as `resource_link`
  blocks on others; a file without a path (the browser) is inlined as text
  up to 200 KB.
- **Link chips.** A pasted URL becomes a chip in front of the field; bare
  links in replies and messages render as chips: favicon and title on a
  tint of the page's `theme-color`, else neutral. The prompt still carries
  the plain URL. Settings › General › Chat › Link previews turns fetching off.
- **Thread actions.** A row's ⋯ menu or right-click: Mark for Review,
  Mark Done, Pin, Rename, Move to Trash. Pins go to the engine
  (`_rustcode/session/pin`, read back from `_meta.rustcode.pinned`) when it
  has the method; pins kept by an older build move there once, and
  without the method they stay in app storage. Archive is gone: Settled is
  exactly the threads marked Done, and threads an older build archived
  are marked Done once. ⌘F finds in the thread (CSS highlights, ⏎/⇧⏎ step). A sent
  message has Edit (back to the composer; sending takes the thread back to
  it) and Retry, both with `_rustcode/rewind`, asking first when later
  messages go too. A finished turn shows a summary card: files changed with
  +/−, and whether the checks it ran (test, lint, build, …) passed on their
  last run, from its tool calls only.
- **Subagents.** A `spawn_agent` call shows as the agent it started (status,
  role, task, time, tokens). The Agents panel lists the thread's agents
  (`_rustcode/agents`, polled every 2 s while a turn runs or a child
  works), reads one's conversation, stops it and sends it more. A pill in
  the composer row says how many work.
- **Pull requests.** A panel lists the project's pull requests
  (`_rustcode/pulls`), shows one (`/view`: description, checks, files,
  activity) and posts a review you write (`/review`: comment, approve or
  request changes, at the head commit). "Ask the agent to review" starts a
  thread draft naming the commit.
- **Usage.** In the composer row, after the SwiftUI app's `UsageCapsule`
  and `PlanUsageMeter`: a ring filling with the context window in use
  (`34k / 128k tokens · 73% left` on hover), and the plan's shortest window
  from `_rustcode/usage/limits` as a short bar with the percentage left
  (all windows and their resets on hover, which also refreshes them). A new
  thread shows the limits the last thread learnt. The chat header no longer
  repeats them.
- **Model picker.** After `ModelEffortPanel`: *Fast mode* (when the thread's
  model has the `fast` option) and the model with its effort on one line;
  the model's name drills into the list. Effort is a slider with a dot per
  level: drag (snaps on release), click, or ←→/Home/End. The harness row
  shows only when more than one harness is usable. Fast mode is set on a
  thread once it exists; a new thread does not carry it yet.
- **New thread banner.** Settings › Appearance: a wide picture over new
  threads, drawn in coloured text (ASCII, inverted on a light background)
  or as is, covering the top half of the chat and fading toward the
  composer, under a frosted title bar. The picture is copied into the app's
  data folder (`src-tauri/src/banner.rs`; the setting keeps the copy's
  name), decoded once, and the text drawn once per size on an offscreen
  canvas (`src/lib/banner.ts`), so a resize scales the last drawing until
  the new one is ready. It fades in, and does not move under Reduce motion.
- **Terminal.** ⌘J: xterm.js on the shell's PTY in the thread's folder, one
  per folder, kept while hidden, following the app's colours.
- **Smart UI.** See below.
- **Look.** Neutral and opaque, after [design.md](design.md): the
  sidebar and every surface paint solid token colours, so greys no longer
  pick up the wallpaper through the window's material (`windowEffects` is
  still set in `tauri.conf.json` but nothing shows it). Only the composer,
  toasts, the jump-to-latest button and the header over a banner are
  frosted; Reduce transparency (the media query, or the system setting
  read by the shell's `reduce_transparency`) makes them solid. Hover is
  colour only, in 60 ms and out 120 ms; focus rings show for the keyboard
  only; scrollbars are thin and appear on hover, with a stable gutter on
  long scrollers.
- **Approvals** are one slim bar over the composer (Details opens the
  command and diff); when a turn waits on several, *Approve all* allows
  them once. Queued messages are rows above the composer that can be
  edited in place, moved up, sent now as a steer, or removed.
- **Command palette** (⌘K): recent threads, then actions: new thread (on
  any harness, or in a worktree), board, review, mark review/done,
  compare a fan-out, panels, every Settings page, appearance, restart.
- **Sidebar.** Threads by where they are in their life (after T3 Code):
  Pinned (always shown), Needs You, Working, Threads (split by project when
  there are several; a worktree thread under its repository, with its
  branch), and Settled: done or archived threads, folded by default. Each
  section folds with its chevron and remembers it; a search opens them all.
  Settling is marking a thread Done; a new message brings it back. The
  filter button by the search groups by project, keeps to the current
  project, and opens the Board. Subagents nest under their thread (open one
  in the Agents panel), and threads another thread started nest under it.
  Settings is the gear at the right of the main header; the project is
  chosen per thread in the composer's tray.
- **Title bar.** The sidebar's and header's buttons sit on the traffic
  lights' line, 14 px past the zoom button (x 92; the button ends at x 78),
  the gap the SwiftUI app leaves (`src/ui/Titlebar.tsx`); in full screen,
  where the lights hide, the rows start at the edge.
- **One shell.** The sidebar and the main header (`src/ui/MainHeader.tsx`)
  stay mounted for every view. Settings covers only the main column and
  the right panel, which stay mounted and laid out underneath (`inert`),
  so ⌘, renders Settings in the first frame with nothing remounted; its
  pages open with the last engine answers and refresh behind them.
- **Settings › Capabilities.** Skills and MCP servers in one page, through
  `_rustcode/capabilities*`: servers with their state, tool count and
  names, errors and the engine log lines that name them, on/off (servers
  from a project config are read-only), add (command or URL, with
  environment or headers), remove, and Edit config; skills by where they
  were found (project, user, shared, configured) with their path, on/off,
  Edit (opens SKILL.md), Show in Finder, and New skill (the shell writes
  `SKILL.md` from a template under `~/.config/rustcode/skills` or the
  project's `.rustcode/skills`, `skill_create`, and opens it).
- **Themes.** Settings › Appearance lists the engine's themes
  (`_rustcode/themes`) with a preview each, picks one
  (`_rustcode/themes/set`), opens the themes folder and creates one
  (`_rustcode/themes/new`; an engine without it gets the
  `rustcode theme new` command to copy). `_rustcode/themes/changed`
  (a file edited, or the CLI picking another) applies at once. A theme
  sets the light/dark mode; System uses the built-in pair. The tokens are
  written onto `:root` (`src/lib/themes.ts`), so nothing re-renders, and
  the last theme is cached so a restart does not flash the default.
  Without the methods the app shows its two built-in themes. Theme files
  are described in [themes.md](themes.md).
- **Right panel.** Changes, Agents and Pull requests share one floating
  panel (`src/ui/RightPanel.tsx`): tabs between them, a close button, a
  draggable left edge (kept), ⌘⌥B to toggle it.
- **Threads working for threads** ([orchestrator.md](orchestrator.md),
  M1 rules from
  [cross-session-orchestration.md](architecture/cross-session-orchestration.md)).
  Off by default. The app answers `_rustcode/orchestration`
  (`src/state/host.ts`, rules in `src/state/orchestration.ts`):
  `list_sessions`, `read_session`, `message_session`, `wait_session`,
  `start_session` and `fan_out`. Same project only (another project only
  for `start_session`, one the user has opened, with approval), no chains,
  4 active links per thread, a 32-message mailbox, 8 KiB messages, 16 KiB
  reads, waits up to 1 h. Each needs the user's *Allow for This Thread*,
  *Allow Once* or *Don't Allow*; grants and links are kept in app storage
  (`rustcode.threadLinks`), and after a restart active links are
  *interrupted*. Started threads go through `session/new` with
  `_meta.rustcode.harness` and `.worktree` and get the prompt as their first
  message, headed `From thread "<title>" (agent):`. Chips above the composer
  (state, open, stop), a *Working for* banner, sidebar badges. Stopping a
  thread stops the work it gave others.
- **Worktree threads.** The new-thread composer's *Worktree* toggle starts
  the thread in its own worktree, with a branch name suggested from the
  message (`<type>/<slug>`) that can be edited. A worktree thread's menu
  opens it in Finder or Terminal and removes it, asking first and again with
  the engine's reason (uncommitted changes, unpushed commits).
- **Status and Board.** Running, Needs input, Review and Done chips on rows
  and in the chat header (a menu marks Review or Done). *Board* (the
  sidebar's filter menu, or ⌘K) shows those four columns; drag a card to Review or Done.
  Marks go to `_rustcode/session/status`; on an engine without it the app
  keeps them itself. `_meta.rustcode.status` from the engine (a merged
  branch's auto-done) updates the chip.
- **Fan-out compare.** A `fan_out` adds *Compare* to the chips: one card per
  harness with its status, branch, files changed and diff stat of the
  branch, its answer, Open and Review. *Keep this* marks the others done
  and the kept one for review.
- **Checkpoints and Review.** A finished turn has a *Changes* chip from its
  checkpoint (`_rustcode/checkpoints/list|diff`) with *Restore…* (files only,
  or files and conversation when the harness rewinds). The Review screen
  (thread menu, header menu or ⌘K) shows the uncommitted changes, the
  worktree branch since its base, one turn, or the branch's open pull
  request as a file tree and a unified or side-by-side diff coloured by
  Shiki. Click a line number to draft a comment; the drafts go to the agent
  as one message, or to the pull request as one review with line comments
  (`_rustcode/pulls/review`).
- **Visualizations.** `rustcode-viz` blocks are drawn natively
  (`src/ui/Viz.tsx`): bar chart, table (sortable), tree, line chart (hover),
  stat list and checklist, with the limits and source fallback of
  [visualizations.md](visualizations.md).
- **Missing engine methods.** At `initialize` the app probes
  `_rustcode/worktrees/list`, `_rustcode/checkpoints/list`,
  `_rustcode/session/status`, `_rustcode/session/pin` and
  `_rustcode/themes`; a method-not-found hides that feature with a
  tooltip saying why (pins stay local, themes are the built-in two). The
  mock engine implements all of them.

## Smart UI

`rustcode-ui v1` blocks in assistant replies run as views
([smart-ui.md](smart-ui.md), "Runtime" holds the contract shared with the
engine's `smart-ui` skill). The view has no chrome: no title row,
background or border. Copy source, Show source and Stop/Run are small
icons at its top right on hover or focus. A block that finishes streaming
in front of you runs on its own; others show Run. At most three run;
starting a fourth stops the oldest. 64 KB of source, 5,000 elements,
48–720 px tall and fitting the content; a page that does not load in 5 s,
goes 3 s without its heartbeat (a busy loop) or builds too many elements is
stopped with its source shown; one out of sight for 30 s is torn down.

Each page gets, before the block (`src/smartui/page.ts`): the CSP
`default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline';
img-src data:; font-src data:` (plus `connect-src`, `frame-src`,
`worker-src`, `object-src`, `form-action`, `base-uri` `'none'`); the
variables `--rc-text … --rc-series-8` resolved from the app's theme and
updated live; base styles; the `.rc-*` layout classes; Chart.js 4.5.1 (MIT,
bundled from npm, loaded only for a page that names
`rustcode-lib:chart.js`) with `Chart.defaults` and series colours set from
the variables; and `window.rustcode`:

- `send(text)`: the user's next message in the thread, shown tagged "From
  view". Only from a real click or key press: the page checks
  `navigator.userActivation.isActive`, and the app checks again that the
  gesture reached it (activation propagates to the app's frame) and that
  focus is in that view. One per 2 s per view, at most 2,000 characters.
  While a turn runs it is queued.
- `copy(text)`, `theme` (`"light"` or `"dark"`), `onThemeChange(callback)`.

The bridge is postMessage with a random token per frame; the app accepts a
message only when `event.source` is that frame. The frame is
`sandbox="allow-scripts"` (opaque origin: no storage, cookies, popups,
forms or top navigation). In the browser (`dev:web`, mock) the page is the
frame's `srcdoc`. In the app it cannot be: a `srcdoc` frame inherits the
app's CSP, which allows no inline script, so the page is handed to the
shell (`smartui_page`) and served once from `rcui://localhost/<id>` with
its own CSP header. The app advertises `smartUi = 1` while Settings ›
General › Chat › Smart UI is on (default), else 0; the engine reads it at
`initialize`, so Settings offers Restart Engine after a change.
`rustcode-viz` blocks are drawn by the app itself (Features, Visualizations).

In dev builds `window.__rustcode` exposes the store and its actions, so an
agent can drive and inspect the UI (as `DebugDriver` does for the SwiftUI
app), for example from a headless browser over the Chrome DevTools Protocol.

## Checks

```bash
cd apps/desktop
bun run typecheck && bun run lint && bun run test && bun run build
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`bun run test` covers the design system's guard rails (`design.test.ts`),
theme parsing and resolution, themes, pins and Settled against the mock,
the orchestration rules and v2 parsers
(`orchestration.test.ts`), `rustcode-viz` parsing, a fan-out end to end
against the mock (grant, two worktree threads, answers), status marks, the
JSON-RPC peer, the ACP and extension parsers, the
reducer, the harness and onboarding rules, composing prompt blocks, link
metadata sanitizing, the Smart UI parser, page and limits (and that the app
and the shell serve pages under the same CSP), and the store end to end
against the mock engine (replay, a turn with an approval, a question and
cancel, first-run onboarding, a thread on another harness, thread actions,
edit and resend with rewind, subagents and usage limits). `cargo test`
covers the link fetcher's parsing and address rules, the thread folder
check, UTF-8 across PTY reads, the Smart UI page store, and what
`open_path` opens and `skill_create` writes. `.github/workflows/desktop-ci.yml`
runs the same on pull requests that touch `apps/desktop`.

`src-tauri` is its own Cargo workspace (the root workspace excludes it), with
its own `target/`. Engine changes never build it.

## Performance

`bun run perf [prod|dev] [--seconds 30] [--rate 100] [--replay file.jsonl]`
builds the UI (`prod`: `vite build` + `vite preview`; `dev`: a Vite dev
server with its own dependency cache, on ports 1434/1433) and drives it in
headless Chrome against `?transport=perf` (`src/acp/perf.ts`): the mock
engine plus 500 threads, `perf-long` (2,000 messages, 5,000 rows),
`perf-stream` (reply chunks at `--rate` per second with a running tool call
every 40 chunks) and, with `--replay`, `perf-real`, a recorded real
`session/load` (record one with `initialize` + `session/load` against a
copy of a thread folder in a scratch `RUSTCODE_CONFIG_DIR`). Every engine
line arrives as its own task, as Tauri channel messages do. It prints
startup, time to the transcript's first rows, React commits, long tasks,
frames, key-to-next-frame latency while typing in the composer during the
stream, ⌘K and sidebar filtering, and idle main-thread time. `--profile`
(with `--inclusive`) prints the hottest functions of an unminified
production build, `--trace` main-thread time by trace event. Chrome stands
in for WKWebView: compare runs with each other.

What was slow, and what changed:

- **The transcript did not virtualize.** The chat column had no
  `min-height: 0`, so it grew to the full transcript height and every row
  (5,000 in `perf-long`) was in the DOM; switching threads re-rendered and
  re-measured all of them. Now the column is bounded, the transcript is
  keyed by thread and starts at its end, measured row heights are kept by
  item id, and a newly mounted row keeps its estimate until its
  ResizeObserver entry (reading `offsetHeight` in the ref forced a layout
  per row).
- **Every engine line was a store update and a render**, notifying every
  subscriber. Lines are now queued and applied once per animation frame
  inside one batch (`state/batch.ts`): state updates at once, subscribers
  hear once. Inside the batch the reducer changes arrays it made in that
  batch in place (`foldingBatch`), so a replay no longer copies the
  transcript per update, and a streamed chunk closes open thoughts by
  looking back to the last reply, not over the whole thread.
- **The sidebar re-rendered all rows per chunk** (it subscribed to every
  thread). Rows are memoized and read only their flags; row selectors look
  sessions up by id instead of scanning the list. The composer ignores
  transcript changes.
- **Markdown was re-parsed whole per chunk and per remount.** A streaming
  reply is split into blocks and only the last one re-parses; finished
  text is parsed once and its element tree is cached by its text; Shiki
  output is cached, so a remounted code block is coloured in its first
  frame.
- **Engine stderr** was appended to the store per line; it is now batched.

Before (HEAD before these changes) and after, `perf-long` at 2,000
messages, streaming 100 chunks/s for 30 s with a key press every 120 ms,
Chrome headless on an Apple silicon Mac:

| | prod before | prod after | dev before | dev after |
|---|---|---|---|---|
| Thread list after load | 270 ms | 400 ms | 1.26 s | 1.08 s |
| Open 2,000-message thread, first rows | > 120 s (200 messages: 25.5 s) | 131 ms | > 60 s at 200 messages | 180 ms |
| Open recorded real thread (1,437 updates, 3.5 MB) | 51.2 s | 110 ms | > 60 s | 251 ms |
| Switch back to an open long thread | 189 ms (500 rows) | 13 ms | 757 ms | 61 ms |
| Streaming: frames per second | 32 | 60 | 4.8 | 60 |
| Streaming: dropped frames in 30 s | 838 | 3 | 1,701 | 15 |
| Streaming: long tasks > 50 ms | 3 (max 72 ms) | 0 | 143 (max 325 ms) | 2 (max 192 ms) |
| Streaming: main thread busy | 100 % | 22 % | 97 % | 31 % |
| Streaming: backlog after the stream ends | 110 s | 0 | > 120 s | 0 |
| Key to next frame while streaming, p50 / p95 | 31 / 40 ms | 9 / 16 ms | 85 / 188 ms | 10 / 16 ms |
| Sidebar filter over 500 threads, p95 | 64 ms | 28 ms | 408 ms | 54 ms |
| Idle main thread | 2 % | 2 % | 96 % (backlog) | 3 % |

The streaming and sidebar "before" rows were measured with 10-message
threads, as the larger ones did not finish loading. Most of the dev-mode
gap is React's development build (StrictMode double renders, dev-only
checks and the Performance-track logging); in dev mode streaming and
opening threads are now smooth too. Natively (`tauri build --debug`, debug
engine, a scratch config) the engine now starts 0.85 s after launch
instead of when the page asks for it (1.4 s); idle, the shell, the
WebContent process and the engine sit at 0 % CPU.

Still open: the sidebar's 500 rows are not virtualized, so every store
notification runs their selectors (about 3 % of the main thread while
streaming) and filtering re-renders them; the window's vibrancy and
`backdrop-filter` surfaces cost compositor time that a headless browser
does not show.

## Release

`bun run bundle` builds the engine in release mode and copies it to
`src-tauri/binaries/rustcode-<target triple>` (`scripts/bundle-engine.ts`),
then runs `tauri build` with `src-tauri/tauri.bundle.conf.json`, which adds
`"externalBin": ["binaries/rustcode"]`. Keeping that out of
`tauri.conf.json` means `tauri dev`, clippy and CI never need the engine
built. The shell finds the sidecar next to its binary (a debug build still
prefers the workspace's `target/debug/rustcode`). The bundle carries the
SwiftUI app's layered macOS 26 icon: `scripts/app-icon.ts` compiles
`macos/Resources/AppIcon.icon` with `xcrun actool` into `Assets.car`
(mapped into `Contents/Resources` by a generated
`src-tauri/gen-icon/tauri.conf.json`), and `src-tauri/Info.plist` sets
`CFBundleIconName = AppIcon`, with `icons/icon.icns` as the fallback when
actool is missing. `tauri dev` runs the bare binary, so the Dock shows the
flat icon there; only the bundled app shows the layered one. Signing, notarization and
an updater are not set up.

## Not yet ported

From the SwiftUI app, still missing or partial:

- `@` file references; picking diff lines for a message. Files dropped in
  the browser are inlined as text rather than linked.
- Slash commands the app answers itself (`/terminal`, `/remote`,
  `/visualize`), report-command sheets, `/context` sheet.
- Snoozed and done threads, moving a thread to another folder
  (`_rustcode/session/move`), the retention sweep, swipe actions; loading
  the thread list page by page. Trash moves the folder into `~/.Trash`
  (macOS); Finder's Put Back does not know it.
- The timeline rail, turn folding ("Worked for … · N tool calls"), sticky
  turn bar, stream pacing, quote in reply.
- The auto-approve (YOLO) toggle on approval cards.
- Subagents' verification status and background agents of threads off
  screen (subagents of a thread whose agents were listed nest under it).
- Pull request line comments, the diff view in the review, and watches
  (`_rustcode/pulls/watch*`).
- Changes panel: branch mode, renames' old paths, syntax colours in diffs.
- Localhost preview, the Smart UI agent preview (`smart_ui_preview`,
  `smartUi = 2`). `rustcode-viz` cards have Copy source but not Copy as
  Text or VoiceOver summaries.
- Orchestrator v2 against the real engine: as of this build the engine does
  not answer `_rustcode/worktrees/*`, `_rustcode/checkpoints/*` or
  `_rustcode/session/status`, so those features are hidden there and were
  checked against the mock only. Pull request watches that auto-settle are
  the engine's. A link's `cancel_session_turn` and the target's *Pause
  messages* / *Detach* are not offered.
- The vibrancy was not checked by eye in a native window yet (the display
  was off during the change); the CSS and config were.
- GitHub sign-in flow, model account switching (Connections is read-only),
  keybinding editor, Discord presence, remote pairing, notifications,
  updater, What's New.
- Terminal tabs (one terminal per folder).
- An E2E driver for the native window; signing and notarization. The
  Smart UI `rcui://` path was checked by unit test and the page itself in
  Chrome; a click in a view inside the native window was not exercised.
