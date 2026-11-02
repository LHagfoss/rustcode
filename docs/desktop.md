# Desktop app (Tauri)

`apps/desktop` is the new RustCode desktop app: Tauri 2 with a React,
TypeScript and Tailwind UI. It is meant to replace the SwiftUI app in
`macos/` over time, with the same look and layout. Like the SwiftUI app, it
runs the engine as `rustcode --acp` and talks ACP to it, so every harness
(the built-in one, Codex, Claude Code, Gemini CLI, OpenCode, any ACP agent;
see [harnesses.md](harnesses.md)) runs a thread through the same UI.

Status: early. A daily-usable chat for a thread on any harness, approvals,
the model, effort and harness pickers, onboarding and Settings work. Much of
the SwiftUI app is not ported yet; see [Not yet ported](#not-yet-ported).

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

# A debug .app bundle.
bun tauri build --debug    # src-tauri/target/debug/bundle/macos/RustCode.app
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
- **Native helpers** (`src-tauri/src/native.rs`), each validated in Rust:
  `open_url` (http, https, mailto only), `reveal_path`, `git_changes` (the
  Changes panel: `git diff HEAD` plus untracked files), and
  `open_login_terminal`, which runs a harness's own login in Terminal and
  accepts only known harness tools (`codex`, `claude`, `gemini`, `opencode`,
  …). Capabilities allow only core APIs, window dragging and the folder
  picker; the CSP allows no remote content.
- **Transport** (`src/acp/transport.ts`): `TauriTransport` in the app,
  `WebSocketTransport` to `scripts/engine-bridge.ts` for `dev:web` (bound to
  127.0.0.1, accepts only the Vite origin, one engine per connection, same
  restart rules), and `MockTransport` (`src/acp/mock.ts`), an in-process
  fake engine with canned threads, a streamed turn with a plan, a running
  command, an edit approval and a question, and the full harness contract.
- **ACP client** (`src/acp/jsonrpc.ts`, `src/acp/types.ts`): a typed
  JSON-RPC peer and parsers ported from `RustCodeKit` (`ACPTypes.swift`,
  `ACPConnection.swift`, `FileDiff.swift`). `initialize` advertises no client
  fs or terminal and no `_meta.rustcode` features the app does not implement.
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

Keys: ⌘N new thread, ⌘K switch thread, ⌘, Settings, ⌘B sidebar, ⏎ send (or
queue while a turn runs), ⌘⏎ steer the running turn, ⇧⏎ newline, Esc clears
the draft and then stops the turn (⌘. stops it too), 1–9 answer the visible
approval or question.

In dev builds `window.__rustcode` exposes the store and its actions, so an
agent can drive and inspect the UI (as `DebugDriver` does for the SwiftUI
app), for example from a headless browser over the Chrome DevTools Protocol.

## Checks

```bash
cd apps/desktop
bun run typecheck && bun run lint && bun run test && bun run build
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`bun run test` covers the JSON-RPC peer, the ACP parsers, the reducer, the
harness and onboarding rules, and the store end to end against the mock
engine (replay, a turn with an approval, a question and cancel, first-run
onboarding and a thread on another harness). `.github/workflows/desktop-ci.yml`
runs the same on pull requests that touch `apps/desktop`.

`src-tauri` is its own Cargo workspace (the root workspace excludes it), with
its own `target/`. Engine changes never build it.

## Release (not wired yet)

A release bundle should ship the engine as a sidecar: build `rustcode`, copy
it to `src-tauri/binaries/rustcode-<target triple>`, and add
`"externalBin": ["binaries/rustcode"]` to the bundle config; the shell then
finds it next to the app binary. Signing, notarization and an updater are
not set up.

## Not yet ported

From the SwiftUI app, still missing:

- Image and file attachments, pasting images, `@` file references.
- Slash commands the app answers itself (`/terminal`, `/remote`,
  `/visualize`), report-command sheets, `/context` sheet.
- Pinned, snoozed, done and archived threads, rename, move, trash, the
  retention sweep, swipe actions and row context menus; loading the thread
  list page by page while scrolling.
- Find in thread (⌘F), the timeline rail, turn folding ("Worked for … · N
  tool calls"), the turn completion card, sticky turn bar, stream pacing.
- The auto-approve (YOLO) toggle on approval cards.
- Rewind, edit-and-resend, retry, quote in reply.
- Subagent panel (`_rustcode/agents`), PR list, review and watches
  (`_rustcode/pulls*`), plan usage limits (`_rustcode/usage/limits`).
- Changes panel: branch mode, renames' old paths, picking lines for a
  message, syntax colours in diffs.
- Workspace terminal, localhost preview, Smart UI and native
  visualizations (the app does not advertise them, so the engine does not
  offer them).
- GitHub sign-in flow, model account switching (Connections is read-only),
  keybinding editor, Discord presence, remote pairing, notifications,
  updater, What's New.
- An E2E driver for the native window; a release bundle with the engine
  sidecar, signing and notarization.
