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
  forward credentials; the webview sees only what the engine prints.
- **Native helpers**, each validated in Rust:
  - `native.rs`: `open_url` (http, https, mailto only), `reveal_path`,
    `git_changes` (the Changes panel: `git diff HEAD` plus untracked files),
    `read_image` (an absolute path to a PNG, JPEG, GIF or WebP of at most
    20 MB, for attachments) and `open_login_terminal`, which runs a
    harness's own login in Terminal and accepts only known harness tools.
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
- **UI** (`src/ui/`): tokens in `src/index.css` are ported from `Theme.swift`
  and `CodeStyle.swift` (light and dark, following the system or the
  Appearance setting). The transcript is virtualized with
  `@tanstack/react-virtual` and stays pinned to the bottom unless you scroll
  up. Markdown is `react-markdown` + `remark-gfm`; code is highlighted lazily
  by Shiki with Xcode's colours.

Keys: ⌘N new thread, ⌘K command palette (threads, and actions with `>`), ⌘, Settings, ⌘B sidebar, ⌘F find
in thread, ⌘J terminal, ⏎ send (or queue while a turn runs), ⌘⏎ steer the
running turn, ⇧⏎ newline, ⌫ at the start of the field takes back the last
link chip or the command token, Esc clears the draft and then stops the
turn (⌘. stops it too), 1–9 answer the visible approval or question.

## Features

- **Composer.** One rounded unit: the queue, attachments, the field and a
  button row (folder, Files, agents working, harness, model pill,
  send/queue/stop). A slash command from `available_commands_update` that
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
- **Thread actions.** A row's ⋯ menu or right-click: Pin, Rename, Archive,
  Move to Trash. Pins and archives are app storage, as the SwiftUI app
  keeps them; archived threads move under "Archived" at the foot of the
  list. ⌘F finds in the thread (CSS highlights, ⏎/⇧⏎ step). A sent
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
- **Plan usage.** The usage capsule adds the plan's shortest window from
  `_rustcode/usage/limits` (`5-hour 58%`), all windows in its tooltip.
- **Terminal.** ⌘J: xterm.js on the shell's PTY in the thread's folder, one
  per folder, kept while hidden, following the app's colours.
- **Smart UI.** See below.
- **Look.** On macOS the window is transparent over the system sidebar
  material (`windowEffects` in `tauri.conf.json`, `macos-private-api`);
  the sidebar paints a light tint over it and the chat stays opaque.
  Floating surfaces (composer, popovers, menus, dialogs, toasts) share one
  blurred material. Reduce transparency (the media query, or the system
  setting read by the shell's `reduce_transparency`) makes them solid.
  Controls answer hover, press and focus; menus, dialogs and rows animate
  in 120–180 ms, and nothing moves under Reduce motion.
- **Approvals** are one slim bar over the composer (Details opens the
  command and diff); when a turn waits on several, *Approve all* allows
  them once. Queued messages are rows above the composer that can be
  edited in place, moved up, sent now as a steer, or removed.
- **Command palette** (⌘K): recent threads, then actions: new thread (on
  any harness, or in a worktree), board, review, mark review/done,
  compare a fan-out, panels, every Settings page, appearance, restart.
- **Sidebar.** Threads grouped Needs You, Working, Pinned, then by project
  (a worktree thread under its repository, with its branch). Subagents nest
  under their thread (open one in the Agents panel), and threads another
  thread started nest under it. The list fades out above the project card.
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
  and in the chat header (a menu marks Review or Done). *Board* (sidebar
  switch, or ⌘K) shows those four columns; drag a card to Review or Done.
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
  `_rustcode/worktrees/list`, `_rustcode/checkpoints/list` and
  `_rustcode/session/status`; a method-not-found hides that feature with
  a tooltip saying why. The mock engine implements all of them.

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

`bun run test` covers the orchestration rules and v2 parsers
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
check, UTF-8 across PTY reads and the Smart UI page store. `.github/workflows/desktop-ci.yml`
runs the same on pull requests that touch `apps/desktop`.

`src-tauri` is its own Cargo workspace (the root workspace excludes it), with
its own `target/`. Engine changes never build it.

## Release

`bun run bundle` builds the engine in release mode and copies it to
`src-tauri/binaries/rustcode-<target triple>` (`scripts/bundle-engine.ts`),
then runs `tauri build` with `src-tauri/tauri.bundle.conf.json`, which adds
`"externalBin": ["binaries/rustcode"]`. Keeping that out of
`tauri.conf.json` means `tauri dev`, clippy and CI never need the engine
built. The shell finds the sidecar next to its binary (a debug build still
prefers the workspace's `target/debug/rustcode`). The icons in
`src-tauri/icons` are the repository's app icon. Signing, notarization and
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
