# Desktop app (Tauri)

`apps/desktop` is the RustCode app, the orchestrator: Tauri 2 with a React,
TypeScript and Tailwind UI. It replaced the SwiftUI app (removed from the
tree; it lives on in git history). It runs the engine as `rustcode --acp`
and talks ACP to it, so every harness
(the built-in one, Codex, Claude Code, Gemini CLI, OpenCode, Pi, any ACP agent;
see [harnesses.md](harnesses.md)) runs a thread through the same UI.

Status: usable day to day. A chat for a thread on any harness with
approvals, attachments, link chips, Smart UI views, thread actions, find,
rewind, subagents, pull requests, plan usage and a terminal. A few things
the SwiftUI app had are not ported yet; see [Not yet ported](#not-yet-ported).

## Status and roadmap

Where the desktop work stands, so the next session (in any harness) can pick
it up. Direction: this app is the product; external coding agents
run threads as harnesses ([harnesses.md](harnesses.md)); agents can start
and message other threads ([orchestrator.md](orchestrator.md)). The design
rules are in [design.md](design.md), themes in [themes.md](themes.md), and
reply formatting in [output-style.md](output-style.md).

Shipped in **1.0.0** (2026-11-02, #2788 to #2837): the app on every harness,
orchestration, worktree threads, checkpoints and review; a compact
sidebar (live clock, Pin/Settle on hover, threads idle for three days settle,
Settled paged); harness settings with live model lists (Claude Code first,
RustCode as the local harness); approvals with Allow/Decline/More; tool-call
groups, a working line that says what the agent does, a context inspector
and plan usage card, report panels, copy everywhere, ⌘O/⌘⇧O folders;
compaction on any harness (`/compact`, Codex through `thread/compact/start`);
a SQLite thread index with full-text search (`index.sqlite`); background
update checks with an "Update to X" sidebar row and What's New; installers
for macOS, Windows and Linux (the Windows and Linux bundle jobs ran for the
first time in 1.0.0). The SwiftUI app was removed (tag `swiftui-app-final`).

Next, in rough order:

1. Signing and notarization once the Apple secrets exist (#2034).
2. #2799, cross-harness delegation: the live test plan with Claude Code
   and Codex parents, a harness/model discovery tool, one approval per
   delegation, waking a parent when its child ends.
3. Compaction follow-ups: the built-in harness's context bar marks its
   auto-compact point and the card compares the context before and after a
   compaction; a harness that reports no threshold keeps the total-only view.
   RustCode now also decides its own auto-compaction from the app: when a turn
   ends above the size threshold, and, in its second mode, shortly before a
   prompt cache expires.
4. SQLite as the store, not only an index: pins and status first, then an
   event table with projections, the
   sidebar listed from the index, lazy transcript import.
5. The rest of [Not yet ported](#not-yet-ported).

Several machines (#3202): Settings › **Environments** (called Machines before; the app says *environment* for a computer it can run threads on, after T3 Code) pairs the app with another
computer's RustCode server and keeps the connection, so one app reaches its
own threads here and other machines' threads there. The sidebar is one flat
list: every machine's threads sit in the same sections, in the order this
computer's own use, with no machine heading and no folder unless Group by
project is on. A thread that runs on another machine carries that machine's
glyph (a laptop for macOS, a monitor for Windows, a terminal for Linux, a
question mark when it has not said) and its name in muted text on its row
(hover: `Runs on ‹name›`); a thread on this computer carries nothing. A delegate
stays under its parent whichever machine it runs on. A machine that is away
lists the copy this computer last read from it, read-only, and its chat says
“‹Machine› is offline since HH:MM. This is the last copy this computer saw; it
updates when the machine is back.”; the Settings gear shows an amber dot while any paired
machine is not connected. New thread and ⌘N still start here; the draft's Run on selector picks another environment (each with its status and address, and *Add environment…* at the end), and Settings › Environments keeps the
pairing, Reconnect and each machine's own page. A machine that is away while
the app retries it on its own reads **Reconnecting…** rather than Offline (in
Settings › Environments and in the notice of its open thread), because it is coming
back without anyone pressing anything. Every client that lists threads
hears of one another makes there (`_rustcode/threads/created`), a just-made
thread stays listed until that machine's own list names it, and one trashed in
any app leaves the others' lists (`_rustcode/threads/deleted`).
Needs you counts
threads across every machine. A delegate that runs on another machine than its
parent is nested under the parent as usual and also keeps a row in its own
machine's list, saying `from <thread> on <machine>`; the hint opens the parent
(`crossMachineDelegates`, `src/state/sidebarGroups.ts`).
A thread key carries its machine (`local` keeps its bare id, another one reads
`‹host›/‹id›`, `src/state/threadKeys.ts`), and every operation on it goes over
that machine's own connection: `connectionFor` in `src/state/store.ts` returns
the local engine or the machine's `AcpConnection`, open the thread and its
permission answers travel there, and its `session/update` stream keeps it live.
A machine that is away keeps its threads listed and read-only, also after the
app restarted: the app keeps what it last read from each machine in its own
storage (`state/machineCache.ts`: the 200 newest threads per machine, and the
transcript of each thread opened in the last 30 days, capped in size; never a
token), and a live read replaces it when the machine is back. The same reads also go to this
computer's disk, through its own engine (`state/machineMirror.ts`, `docs/data-store.md`
› Thread mirror): every thread a machine listed and the last transcript read
from each, without the web view's caps, so a wiped web view, a reinstall or a
machine that is wiped or removed does not take its threads with it. A machine that is away
draws the mirrored threads its 200 cached rows lack, and a thread whose transcript
the web view no longer holds opens from the mirror. It is read-only, never deleted
and never sent back; Settings › Environments › **Thread copies** switches it off
and lists projects to leave out. The chat says
"‹Machine› is offline since HH:MM. This is the last copy this computer saw; it
updates when the machine is back.", the composer is disabled, and the
header shows `Runs on ‹machine› · ‹harness› · ‹model›`; the controls that read
this computer (terminal, working tree, pull requests, Finder, worktrees,
checkpoints and undo) stay disabled with "Runs on ‹machine›" where the action
would be. Settling a thread there (or Review, Unsettle, pin and unpin) still
works: the row changes at once, the toast says "Syncs when ‹machine› is back",
the row carries a cloud glyph, and the change waits in the app's storage
(`state/machineOutbox.ts`, one per thread and kind, so a settle undone before
then sends nothing). When the machine lists its threads again
(`machines.ts` `readThreads`) the changes go to it in order. A change the
machine already has is not sent again; one the machine overrode is dropped,
the newer one winning: the machine's when its thread was active there after
the change was made (its `updatedAt` is later), the user's otherwise. A change
the machine refuses (an older server) puts the row back and says so. The Rust
shell owns one connection supervisor per machine
(`src-tauri/src/machines.rs`, `rustcode/client`) and the webview gives each a
channel (`MachineTransport` in `src/acp/transport.ts`, `src/state/machines.ts`),
because a web view cannot dial a gateway and the pairing token belongs in the
credential store. `?machines=paired|connect|offline|revoked` in the mock walks
the page without a second computer; `paired` is a Mac, a Windows PC with
threads, and an offline Linux box, so the sidebar shows three headings, the
Linux box folded to a muted row.

A machine's name is shared across the fleet and its glyph is this app's own:
Settings › Environments (and the same row for this computer) gives each one a
**Rename** popover (name and icon). A rename is stamped with the time and spread
to the machine itself and to every other paired computer (`state/fleet.ts`,
[Environment names and the fleet](remote.md#environment-names-and-the-fleet)),
where the newest rename wins; a computer that was offline shows it after its next
sync. The name is stored as `display_name` with `name_updated_at` in
`machines.json`; **Reset** (or a blank field) goes back to the name the machine
reports, which spreads the same way, and pairing the same machine again keeps the
name. The glyph is one of a small fixed set (laptop, desktop, Mac Studio/mini, server,
Windows tower, Linux, cloud, phone), defaulting to the platform's own, with an
optional tint from the chart palette. Everywhere a machine is named uses both:
a thread row's machine tag, a thread's `Runs on ‹name›` header, the composer's
**Run on** control and its menu, and `list_machines`, which keeps the reported
`name` and adds `display_name`, `icon` and `icon_tint`.

A machine's own **Machine settings** page (`src/ui/MachineSettingsPage.tsx`) is
one screen per computer, reached from a Machines row in Settings. Its header
says the local name and glyph, the platform, the RustCode version it runs (with
the skew warning), the state, the route and when it was last seen. The controls are Reconnect,
Disconnect (keeps the machine saved), Forget and **Pause new threads here**:
there is no main machine, every computer is a peer and **Run on** starts on this
one unless it names another. A paused machine keeps its
connection and its existing threads, and **Run on** refuses it until it is
resumed. Below them are its **Routes**: each saved address (the VPN route is
saved with its port like the others) with the last test of it, reachable,
refused, timed out or unreachable, with the latency, and a **Test routes**
button that connects to every address now (a plain connect with no sign-in; the
shell's `machines_probe`), then that machine's harnesses with their install and
sign-in state and one **Check** (re-detect there, read its snapshot again), and
**What syncs**: one row per Settings row, generated from the same registry the
**Applying to** bar reads, in three columns — Shared, This machine only, This
app only — with **Differs** where that machine's value differs from this Mac's.
Choosing a row opens its page with **Applying to** set to the machine, and the
footer says a value set remotely takes effect there on its next launch. An
offline machine shows what was last read, read-only.

Agents across machines: the orchestration tools take a `machine` (v12), so a
thread here can list the paired computers (`list_machines`), start, message,
read, wait on and stop threads there, and be woken when a remote delegate
finishes — the delegate is nested under its caller and its stack mark names the
machine. The app executes those calls over the machine's own connection; see
`docs/orchestrator.md` › Orchestration tools v12 for the tool set, the approval
card and what v1 leaves out (this app must be running, and a remote delegate
keeps the title its first message gives it there).

**Run on** is the new-thread composer's own choice of machine
(`src/ui/Composer.tsx`), in the strip beside the folder: this computer first,
then each paired machine with its connection state, and a machine that is away
disabled with "Offline". It is hidden until a second computer is paired. Moving
it reads that machine's `_rustcode/harnesses` — its harnesses, models, logins
and default — over the machine's own connection and keeps the answer per
machine (`remoteHarnesses` in `src/state/store.ts`), so the model picker, the
`@` agents and the harness defaults are the machine's; this computer's list is
untouched. The composer's Run on pill starts a thread there
(`startNewThread(folder, host)`), and any other new thread starts here. A folder picked on this disk — "Choose Folder…", "New
Thread in Folder…", the palette's Add project folder — always starts here: a
path alone never names a machine, so two Macs that share a checkout path cannot
mistake each other; ⌘O with no thread open starts here in the folder it picks. "New Thread Here" in a thread's folder menu and `/new` in
its composer keep that thread's machine and folder.

**Settings on one machine or all.** Settings is not synced from a main machine
(user decision, #3202). A server-setting page — General's server rows,
Harnesses and Capabilities — starts with **Applying to**: This machine (the
default), one paired machine, or All machines, and the choice persists across
pages (`src/state/settingsScope.ts`). A row whose connected machines disagree
shows **Mixed** with a layers mark; its hover card lists each machine's value.
Picking a value under All machines writes it to every connected machine, and
an offline machine is listed as keeping its value rather than being changed.
Every row's kind is in one registry (`src/state/settingsRegistry.ts`):
appearance, shortcuts and window state are this app's own and show no
selector; credentials, accounts, the default project folder, Remote access and
the machines list are machine-only and shown for one machine at a time —
credentials are never editable over a remote connection.

Skills and MCP servers are listed per machine under Capabilities. **Add server
to all** adds an MCP definition to every connected machine; **Copy to ‹machine›**
on a row copies its definition. Values of MCP environment variables and
headers never travel: the machine you type them on keeps them, the others get
the key names blank with a note to set them there, and a definition whose
target carries a secret-looking part (`--token`, `KEY=value`, a URL with
userinfo, a query or a fragment) is added to this machine only and refused on
the others, naming the part. A command whose target has a masked secret is not
offered for copying. A skill copies its `SKILL.md`, read with
`_rustcode/skills/get` only when a copy is chosen; other files in its folder
stay on the source machine. The default harness/model/effort reads through
`_rustcode/harnesses`, and harness switches write through
`_rustcode/harnesses/set` on each chosen machine. The other server settings
live in each machine's `~/.config/rustcode/settings.json` through
`_rustcode/settings/get|set` (docs/configuration.md); the app applies this
computer's values when it starts, so a value set from another machine is in
force there next launch. Its first-start seed writes only the keys the server
does not already hold (`stored`), so it cannot revert a value another machine
wrote before this app's first start. The design's machine-only line (§6.1)
covers binary paths, credentials and accounts; the enabled harness switches
and MCP definitions are server settings here and fan out, with every copy
leaving secrets behind.

The project follows the machine: `src/state/projects.ts` finds the same project
there by path, then by git remote (`_meta.rustcode.remote`, normalized so
`git@…` and `https://…` compare equal), then by folder name, and otherwise the
composer offers that machine's most recent project and its other folders from
its thread list ("Choose Folder…" stays this computer's, since no code here can
browse a machine's disk). The folder list shows each folder once, without
Windows' `\\?\` prefix, and never RustCode's own data or install folder. With
no folder chosen and none to go on, a new thread starts in that machine's home
folder (the composer shows *Home folder*; `session/new` is sent no folder,
which the engine resolves to its home). An engine that does not report a remote falls back to
the folder name; the mock's Studio reports one. `session/new` is sent over the
machine's connection with its folder and harness, and the thread is keyed with
its host, so it appears in the list at once; a thread created
there is named by the title it is made with (`session/new`'s
`_meta.rustcode.title`), since `thread_rename` is this shell's. Until that
machine's own `session/list` lists the thread, the app keeps listing it rather
than reading a list from before it as a deletion (issue 3523). Its
`read_activity` and its delegates' `last_provider_error` are read from that
machine's store over the same connection (issue 3525).
A machine on an older RustCode that does not answer `_rustcode/harnesses` is
shown as the builtin harness, as this computer's engine is, so a thread can
still start there. A harness whose models that machine has not learned yet
keeps no model from here: the machine's own default decides. Removing a
machine in Settings › Environments sends Run on back to this computer and drops
the harness list kept for it. Reopening a thread uses its own machine's
connection (`openThread`), including a thread listed before its machine went
away.

Item 4 and the rest of the orchestration work move the domain model from
sessions to an execution graph:
[architecture/execution-graph.md](architecture/execution-graph.md) records that
decision, the current → target mapping and the migration order (#2879).

Checking UI: `bun run mock-window open` in `apps/desktop`, one shared, headless
Chrome on the mock at native scale (`open --show` for a visible window) (`eval`, `click`, `dblclick`, `hover`,
`drag`, `scroll`, `move`, `type`, `press`, `screenshot`, `close`), never a
browser pane in the user's terminal; WebKit
(the app) can differ from Chrome, so check alignment-sensitive changes in the
app. Releases:
`scripts/release-notes.sh > notes.md`, edit, then
`scripts/release.sh --version X --notes-file notes.md --yes`.

How to continue: run the app with `bun tauri dev` in `apps/desktop` (it uses
`target/debug/rustcode`; rebuild with `cargo build -p rustcode-tui --bin
rustcode`), check with `bun run check`, and measure with `bun run perf` /
`bun run perf:native`. Screenshots: `?transport=mock` in the browser dev
mode gives a stable thread to capture.

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
  events (Settings › Diagnostics shows them). When the engine exits it restarts
  with backoff (0.5 s doubling to 30 s, reset after a minute up) and the UI
  initializes a fresh connection. Quitting closes the engine's stdin so it
  cleans up its sessions (every running turn is cancelled and each harness and
  delegate ended), then kills it after 10 s; a restart gives it 2 s. The
  shell does not read or
  forward credentials; the webview sees only what the engine prints. The
  engine starts with the app, while the page loads; the first page adopts
  it. Lines already buffered on stdout go out together as one `lines`
  message (up to 1,024 lines or 4 MB).
- **Run state follows the engine** (`state/runState.ts`, issue 3652). The
  engine's thread list is the truth about what runs. While the window is
  visible it is read every 5 s when anything runs or is in doubt and every 30 s
  otherwise (and on focus, on every turn end and on each reconnect). A thread
  the window shows running that the list calls idle on two reads in a row,
  more than 10 s into the turn, is closed locally and read again; a thread
  the list shows running that the window holds no turn for reads as running
  (`engineRunning`) until the engine says otherwise. **Stop** always answers:
  with the engine away it says so, it reaches a turn this window lost track of,
  and a turn still running 10 s after `session/cancel` is forced (the thread is
  closed in the engine and read again). Moving a running thread to the Trash
  stops it first the same way. The link book is read again from the data store
  on every connect.
- **Native helpers**, each validated in Rust:
  - `native.rs`: `open_url` (http, https, mailto only), `reveal_path`,
    `git_changes` (the Changes panel: `git diff HEAD` plus untracked files),
    `read_image` (an absolute path to a PNG, JPEG, GIF or WebP of at most
    20 MB, for attachments), `open_login_terminal`, which runs a
    harness's own login in Terminal and accepts only known harness tools,
    `open_path` (an existing folder, or a Markdown, TOML, JSON, YAML or
    text file, in its editor; nothing that runs) and `skill_create` (a
    valid skill name, user or project root, never overwriting) and
    `open_in_editor` (an existing file, at a line).
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
  - `notify.rs`: `notify_show`, `notify_permission` and `notify_badge`, the
    system notifications and the Dock badge (Features, Notifications); a
    click on one brings the window forward and emits `notify:open` with the
    thread's id. The page never touches the notification plugin, so
    capabilities stay as they are.

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
  built-in harness and says so in Settings. One product choice names an id:
  Claude Code (`PREFERRED_HARNESS`) is listed first and suggested on a
  first run, and the built-in harness is listed last as the *Local*
  harness, kept for the TUI, testing and local models.
- **Settings › Harnesses.** Every harness is detected again (installed, signed in, version, each account's sign-in) when the page opens and whenever the window regains focus (`useAutoDetect`, at most once in three seconds); no start of a harness is involved. *New threads start on* comes first and saves as
  it changes (no Save button); picking an external harness lists its
  models at once (`ensureHarnessModels`). Under it, *Harnesses*: one
  card per harness with its mark, status and version, *Sign in* only until
  the harness is known to be signed in, one *Check* and the on/off switch;
  turning a harness on checks it. *Check* (`state/harnessChecks.ts`) detects
  the install again, reads every account's sign-in, then starts the harness
  to list its models, and always ends in one of *Ready*, *Needs sign-in*, *Not
  installed* or an error with the harness's own text (`state/harnessCheck.ts`);
  it never leaves *Installed* or *Not checked* behind. The group has *Check
  all* and *Add harness*; no Refresh sits beside them. *Add harness* opens the
  *Local ACP command* form (`ui/AddHarness.tsx`, rules in
  `state/harnessForm.ts`): a name, an executable, one literal argument per row
  and environment rows. The command is started directly, never through a
  shell. It is added turned off, checked (the ACP handshake and a model list),
  and turned on only if that worked; a failure shows the error text in the form
  and removes it again. A verified one lists under *Added by you*, appears in
  the model picker like any other harness, and has *Remove* under *Details*.
  RustCode searches no ACP registry. The command and what it supports fold under *Details*. On a card whose
  harness has named logins (Claude Code, Codex), *Accounts* lists each named login (its own `CLAUDE_CONFIG_DIR` or
  `CODEX_HOME`) as *Signed in*, *Not signed in* or *Unknown*, with a remove
  that says the directory stays on disk. *Add account* asks only for a name and
  its *Add and sign in* creates `~/.claude-<id>` (or `~/.codex-<id>`) and runs
  the harness's own login in Terminal under it; the row says *Waiting for
  sign-in…* and checks again every three seconds for up to five minutes, then
  offers *Sign in*. The copyable `mkdir -p … && CLAUDE_CONFIG_DIR=… claude auth
  login` (or `CODEX_HOME=… codex login`) stays as the second way. Both the
  variable and the command come from the engine's snapshot (`account_env`,
  `account_login_command`), so the app holds no per-harness copy of them.
  *Found on this Mac* lists the logins the engine found for that harness
  (`~/.claude`, `~/.claude-personal`; `~/.codex`, `~/.codex-paral`), each added
  by its own path. RustCode never reads or copies those tokens. The model card's
  effort column lists the harness's named accounts, each with what is left of
  its plan, and offers a choice while there is more
  than one: a row starts a new thread on it, moves an idle thread with the
  conversation so far as a summary, and a running thread shows the rows
  disabled with *Stop the turn to switch*. A thread from before accounts shows its own login as
  *Original login*. The composer's model pill names the account whenever the
  harness has any. The composer's plan meter shows one meter per account: a
  thread's meter never shows another login's numbers, for Claude Code or
  Codex (the Codex CLI is asked with that account's `CODEX_HOME`). Settings ›
  Usage lists one row per Claude Code account; its ChatGPT row is the Mac's
  own `~/.codex` login. For Codex the section says each account has its own
  Codex settings, because `CODEX_HOME` replaces `~/.codex`. Separate accounts
  are chosen by hand, limits stay per account, and there is no pooling.
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

Keys: ⌘N new thread, ⌘O the folder of the thread in front (a new thread starts there; an open one moves there through `_rustcode/session/move`, closed and reopened; also a click on the header's path), ⌘⇧O the default folder for new threads (also Settings › General; picking a folder for one new thread no longer changes it), ⌘K command palette (threads, and actions with `>`), ⌘, Settings, ⌘B sidebar, ⌘⌥B right panel, ⌘F find
in thread, ⌘J terminal, ⌘+ / ⌘- / ⌘0 text zoom, ⏎ send (or queue while a turn runs), ⌘⏎ steer the
running turn, ⇧⏎ newline, ⌫ at the start of the field takes back the
command token, Esc clears the draft and then stops the
turn (⌘. stops it too), 1–9 answer the visible approval or question.

Every key above except the composer's is a command in one table
(`src/lib/keys.ts`) with an id, a label and a default binding, and Settings ›
Keyboard rebinds it: click the key, press the new combination, Esc cancels;
a row whose binding differs from its default offers a reset. A binding needs
⌘ (or ⌥) unless it is a function key; ⌘Q, ⌘W, ⌘H, ⌘M and ⌘Tab are refused
because macOS keeps them, and so are the editing keys (⌘C, ⌘V, ⌘X, ⌘A, ⌘Z,
⌘⇧Z) that the window's menu answers before the page sees the keydown. A key
another command already answers is refused with its name, so two commands
never share one. Only the overrides are kept, as `rustcode.shortcuts`
(`{ "command-id": "mod+alt+s" }`) in localStorage: an unknown id, a corrupt
value or a binding equal to the default is dropped, and the rest of the table
stays the default it ships with. `state/shortcuts.ts` runs a matched command;
the transcript matches "find in thread" against the same table. The app sets
no menu of its own (`src-tauri/src/lib.rs`), so a remap has no native
accelerator to fight; ⌘R (reload the view, `ui/Recover.tsx`) and the
composer's keys stay outside the table.

## Features

- **Composer.** One glass card: the field, then a single footer row inside
  it, **+** (attaching and new threads), the project folder, *Worktree*
  (new threads) and the branch, the permissions mode, the plan meter, the
  context ring, the model pill, agents working, and send/queue/stop. A
  narrow window shortens the labels and then drops them (the permission
  label, the branch chip, the agents pill); controls stay. A slash command
  from `available_commands_update` that
  takes arguments becomes a token in front of the field (picked from the
  popup or typed as `/name `). The popup opens for the `/token` at the caret,
  at the start of the message or after whitespace anywhere in it, and a pick
  mid-message writes `/name ` into the message instead. The engine's skills
  (`_meta.rustcode.action: "skill"`, one per enabled skill it discovers) are
  offered beside the harness's own commands, which on Claude Code only cover
  `.claude/skills`. Commands the engine marks for the client
  (`_meta.rustcode.action`) never go out as a prompt: `new_session`
  (`/clear`) opens a new thread on the same project, harness, model and
  effort; `model_picker` (`/model`) opens the model picker;
  `session_picker` the thread switcher; `copy_response` copies the last
  reply (`runCommandAction`). `/fork` leaves you in the thread, as the
  engine does, and a toast offers *Open* on the fork. A report command
  (`_meta.rustcode.report`: `/status`, `/stats`, `/context`…) answers in
  a panel over the composer instead of the transcript (`ReportPanel`): its
  bold first line is the title, `Label: value` lines (with `###` or bold
  headings) become rows, anything else stays Markdown
  (`state/report.ts`). It scrolls past 360px, has Copy and Close, and
  Escape or a click outside closes it after any popover over it; the
  command's message and footer leave the transcript. Images and files can be pasted, dropped or
  picked: images are ACP `image` blocks when the thread's harness takes
  them (`capabilities.images`, or `promptCapabilities.image` on the
  built-in one); files with a path go as an "Attached files" list on the
  built-in harness (it ignores `resource_link`) and as `resource_link`
  blocks on others; a file without a path (the browser) is inlined as text
  up to 200 KB. A long paste becomes a chip above the field while the draft
  is written, and the message puts its text back where it was pasted, as
  plain text, when it is sent (`state/compose.ts` `splicePastes`).
- **Sending.** ⏎ answers at once, whatever the harness or the worktree
  costs: `session/new` starts a harness and may check out a worktree, which
  can take a minute, so the message is shown before it is asked for
  (`state/setup.ts` `PendingSetup`). From the new-thread screen the message
  bubble and the `Starting…` line (with the setup's clock) take the place the
  thread's first row will keep, with its pictures and files, its links and
  its command, and the composer glides down to the foot as that send's own
  motion; the prompt goes out the moment the session exists, and the thread's
  own transcript takes over without moving the bubble (`createThread` and
  `startSetup` in `state/store.ts`). The bubble is drawn at rest and never
  animated in: this screen is unmounted the moment the session exists, so
  anything it started would stop halfway through. The message's title is the
  thread's from the first frame, so the row never
  blinks back to "New thread". A failure keeps the message, its attachments
  and the reason on the bubble with *Retry*, which sends exactly that again,
  and *Dismiss*, which gives up and puts the text and its attachments back in
  the field; a second ⏎ while one is starting starts nothing. Attachments are
  read when they are attached, not when they are sent, and the prompt's blocks
  are built after the bubble is on screen, so a long paste or a 20 MB picture
  does not hold the row back.
- **New thread cards.** Under an empty thread's field, a quiet row of glass cards
  (`ui/NewThreadWidgets.tsx`, `state/widgets.ts`): the last 17 weeks as a
  contribution heatmap of turns with the threads active in them, this project's
  three most recent threads, the last 30 days of reported tokens and cost, this
  project's three most recent pull requests (the same cached `gh` read the
  sidebar uses; a row opens the pull on GitHub), and this month's turns and
  spend against the last. One `_rustcode/data/usage` read grouped by day feeds
  the three figures cards, so the screen asks the engine once. Every card keeps
  one height from the first frame and says what it has in a line when it has
  nothing. A persisted toggle opens the overview below the centred composer,
  without moving it; the remaining space scrolls. Below `lg` the
  cards are one sideways-scrolling row, and a window shorter than 700px leaves
  toggle and cards out.
- **Link chips.** A pasted URL becomes a chip in front of the field; bare
  links in replies and messages render as chips: the site's favicon, or a globe
  when it has none or it will not decode, and the title on a tint of the page's
  `theme-color`, else neutral. The favicon sits on a small white tile so a dark
  mark still reads on the chip in the dark theme. The prompt still carries
  the plain URL. Settings › Chat › Link previews turns fetching off.
- **Thread actions.** A row's ⋯ menu or right-click: Mark for Review,
  Settle, Pin, Rename, Move to Trash. Settle is **Unsettle** once the thread
  is settled, in the row's menu, on hover in the row, in the thread's status
  menu and in one glass capsule above the field in the chat — the same
  `.rc-glass-overlay` as the jump-to-latest pill, a `success` tick, *Settled*
  and a quiet *Unsettle* pill — so the way back out is one click wherever you
  are. Pins go to the engine
  (`_rustcode/session/pin`, read back from `_meta.rustcode.pinned`) when it
  has the method; pins kept by an older build move there once, and
  without the method they stay in app storage. Archive is gone: the Settled
  shelf holds exactly the settled threads, and threads an older build
  archived are settled once. ⌘F finds in the thread (CSS highlights, ⏎/⇧⏎ step). A sent
  message has Edit (back to the composer; sending takes the thread back to
  it) and Retry, both with `_rustcode/rewind`, asking first when later
  messages go too. A finished turn shows a summary card: files changed with
  +/−, and whether the checks it ran (test, lint, build, …) passed on their
  last run, from its tool calls only.
- **Subagents.** Every harness's subagents take one row:
  Claude Code's `Agent`/`Task` calls (their steps fold under them by
  `_meta.claudeCode.parentToolUseId`) and RustCode's `spawn_agent` (status
  from `_rustcode/agents`, polled every 2 s while a turn runs or a child
  works; steps from the child's conversation). A Claude Code thread RustCode
  starts with orchestration on has that tool disallowed
  ([harnesses.md](harnesses.md#starting-a-thread-on-a-harness)),
  so its calls only come from an older session or another harness. While one
  works the row says the step it is on; once done, how many steps and how
  long. A subagent sent to the background keeps its row running until the
  turn ends. While it works it reads in the strip above the composer; once
  it settles its row is the transcript's, so there is one delegation surface,
  never a pill of its own over the composer. The threads this one delegated to
  are not in that strip: they are one collapsed line above it (harness icons,
  a count, what needs you in its own tint) that opens to a bounded, scrolling
  list with Open and Stop per delegate (`ui/DelegateStatus.tsx`).
- **Workflow runs** (`docs/workflows.md`, issue 3394). A run an agent starts with
  `run_workflow` shows in three places that read one record. **The card** sits
  under the `run_workflow` call's row in the chat (`ui/WorkflowRuns.tsx`): its
  name, status, elapsed time, the phases as steps (`review 2/3 · 1 failed`),
  the workers done, running and failed, the last log line or why it ended, and
  Stop while it goes. **The status row** is the first line of the delegate
  panel above the composer while a run goes, always shown (the delegates stay
  folded under it): one line, `review-pipeline · verify · 2 of 5 workers done ·
  2m 19s`, with its own Stop, and it opens to the phases and every worker. The
  panel is one place for what a thread has going; there is no second strip.
  **The run panel** is that opened list, in the card and in the row: each
  worker is a delegate thread of the thread that started the run, so a click
  opens it. A run the engine has not been driving since a restart says it
  resumes when its thread is open. Runs come from `_rustcode/workflows/list`
  when a thread opens and from `_rustcode/workflows/changed` notifications
  (the engine coalesces them to one summary per run every 300 ms), kept in
  `state/workflows.ts`. When a run ends the starting thread is told once, as a
  notice row (`Workflow review: done · 5 workers · 4m 12s`) that wakes it after
  its turn like any delegate finish; the engine hands that claim to one client
  (`_rustcode/workflows/wake`), so a second window or a reload cannot repeat it,
  and a run that ended more than an hour before the app first saw it is not
  announced. Settings has a **Workflows** group under Orchestration: workers at once, agent
  calls per run and the run time limit, per machine like the delegate limits.
- **Background work.** Claude Code's background commands and the turns it
  starts on its own when one finishes reach the app as
  `_rustcode/session/activity` (the engine asks the adapter for its async
  tasks, `harness/background.rs`): the thread shows "Waiting on a background
  command", a command the harness backgrounded says `Started in background`
  on its row once the call returns (the task is the thread's waiting line, not
  a spinner on a call that already answered), and a turn Claude starts
  itself shows as working with a quiet "Background task finished" line saying
  why it ran again. A call whose task state never comes back can then only
  leave the wrong waiting line, never a row spinning forever.
- **Pull requests.** A panel lists the project's pull requests
  (`_rustcode/pulls`), shows one (`/view`: description, checks, files,
  activity) and posts a review you write (`/review`: comment, approve or
  request changes, at the head commit). "Ask the agent to review" starts a
  thread draft naming the commit. Each `gh` read is a process, so the answers
  live in one store (`state/pullQueries.ts`) shared by the panel, the sidebar and
  the review view: a list, a detail or a diff the app already holds paints at
  once and is read again behind, and the lists and details are saved on the
  device (8 projects, 40 details, a week) so a reload paints them too. A row
  the pointer or the keyboard rests on for 150 ms reads its detail — the same
  read the click makes, so nothing is wasted — and the review view's pull
  segment reads that pull's diff the same way.
- **Settings › Usage.** Every subscription this machine is signed in to, with
  no thread open: `_rustcode/usage/plans` (engine `acp/usage.rs`) answers
  `{plans: [{id, name, via, status, plan?, error?, reason?, windows}]}`. ChatGPT
  comes from the Codex CLI's `account/rateLimits/read` for the Mac's own
  `~/.codex` login (a thread's meter asks with its own account's `CODEX_HOME`);
  Claude from Claude Code's OAuth sign-in (the Keychain item for that login's
  config directory — `Claude Code-credentials`, or
  `Claude Code-credentials-` plus the first 8 hex digits of the directory
  path's SHA-256 for a named account — and `<config dir>/.credentials.json`,
  the copy with the freshest token winning) against
  `api.anthropic.com/api/oauth/usage` (5-hour, weekly, weekly per model),
  read-only and never refreshed (`provider_auth/claude_usage.rs`). The first
  read happens when Usage opens or a Claude account is used; macOS may ask to
  allow Keychain access. ChatGPT's numbers are cached a minute; Claude's five
  minutes, with a failed read retried after one. That is one Anthropic request
  per login per interval however often the view is opened, because the endpoint
  rate-limits: when it answers 429 the engine waits out its `Retry-After` (or
  doubles its own wait per refusal, up to about 30 minutes) and keeps the last
  numbers on screen, marked with when they were read, instead of showing an
  error. Showing the page is an explicit look and reads every Claude login; the
  background reads one at a
  time, the default login and the account of the thread on screen
  (`claudeAccount`). While the page is open it reads again every three minutes
  (`PLAN_CACHE_POLL_MS`, the meter poll's interval, `watchUsageRefresh` in
  `state/usagePage.ts`), and once on coming back to the window when what it
  draws is older than that. That tick is a background read: it asks for the same
  logins the app's poll asks for (`backgroundPlansAsk`, through
  `state/plans.ts`), so it never reads a login nothing was read for or one a
  failed read paused, and it keeps the rows it did not ask rather than dropping
  them; the foot keeps the time of the last whole read, since a background read
  does not ask for every named login. It reads even when the app's own poll just
  filled the plan cache — the rows on screen come from the page's read, and the
  engine shares a read that is already under way. A read that fails doubles the
  wait, up to fifteen minutes, and a read that answers puts it back on the
  interval; a read in flight is never doubled, and Refresh still reads at once,
  past the engine's cache. What the page reads fills the same cache
  `list_harnesses` reports to an agent (`state/harnessUsage.ts`,
  `docs/orchestrator.md`), which answers without a Keychain read or a request of
  its own and refreshes the stale named logins in the background.
  A Claude access token lives hours and is renewed only when Claude Code runs,
  so a login nobody ran a thread on since reads as lapsed. The row then draws
  the numbers RustCode last read for that login, from `claude-usage.json` in
  the config directory, marked `asOf` with when they were read and drawn as
  "as of" then; only a login with nothing recorded is `unavailable`.
  A Claude row that has no numbers is `unavailable` and says why: `expired`
  (the stored token passed its expiry and nothing was recorded yet — run a
  Claude Code thread on that account, which renews it), `refused` (Anthropic
  answered 401), `forbidden` (Anthropic answered 403: renewing the same sign-in
  does not grant the usage read), `denied` (macOS would not hand the Keychain
  item over) or `transient` (no answer). The page retries `expired` and `refused`
  on its own
  and sends `force: true` then, which reads again instead of answering from the
  engine's cache (`WINDOWS_TTL`, five minutes for numbers, a minute for a
  failure) — so a sign-in renewed in Claude Code shows on return
  to the page, and the background poll picks it up within its three minutes. A
  429 is not an error for the user to fix: the engine waits it out (its
  `Retry-After`, else a doubling wait up to about 30 minutes) and keeps serving
  the login's last numbers, marked `asOf` so the row says how old they are. A
  `forbidden` login is not retried on its own (renewing does not fix it), nor is
  a `denied` one (that would only prompt again); Settings › Usage, Refresh and
  Check in Settings › Harnesses read them again, and Refresh always forces. Reads
  of one login are serialized in the
  engine (`provider_auth/claude_usage.rs`), and a forced read whose login was
  read after the user asked shares that answer instead of asking macOS again. A
  Keychain refusal is remembered while the engine runs: the background reads
  then use the credentials file only, so a denial cannot prompt again even when
  that file answers and its numbers keep showing; Refresh asks macOS once more.
- **Composer meter poll.** The app's one poll (`startPlanPolling`, every three
  minutes while the window is visible, and when it comes back) also re-reads the
  limits of the thread on screen (`_rustcode/usage/limits`, `pollPlans` in
  `state/store.ts`) when they are a poll old, never read or marked stale, since
  the footer draws a thread's own windows before the harness's cache. Claude
  Code answers none there, so its meter keeps coming from the plans read; the
  limits read never touches the Keychain.
- **Settings › Statistics.** What the local data store recorded of every
  turn, by harness and model and by thread: `_rustcode/data/usage`
  (`engine/src/acp/data.rs`) read three times, once `group_by=model`, once
  `group_by=thread` and once `group_by=day`, the last for the 30 UTC days it
  charts (via `since`, so the store's row cap cannot hide recent days). A
  thread's total includes the threads it started, from the parent recorded in
  its metadata or the link book, so delegated work is counted once; a parent
  known only by title is listed on its own and says so. Parents ride on the
  usage rows from the store, along with the harness and model its turns share
  (`model_harness`/`model`, present only when every turn ran the same pair) so
  its row can be priced; a thread that switched harness or model is not
  estimated, since one rate would be wrong for the other turns. When a row has
  no parent id, the link book that started it is the fallback, and an older
  engine also falls back to the session list, which the page marks as
  incomplete if it was capped or missing threads. Tokens stay blank where a
  harness reported none, and each token column marks the turns that reported
  no value for it rather than treating them as zero. A read truncated at the
  store's 1,000-group cap says so. A row's **Cost** is the USD its harness
  reported or, when it reported none (Codex never reports one), the same
  tokens at public API list prices (`apps/desktop/src/state/pricing.ts`,
  dated in `PRICE_DATE`; no network read at runtime): each harness's counts
  are normalized to `{uncached_input, cache_read, cache_write, output}` first,
  because the built-in harness and Codex count cached input inside
  `input_tokens` while Claude Code's and Pi's logs report it separately. A
  reported number always wins and is never altered; the headline total adds
  each row's reported cost or its estimate, never both, and shows the
  reported and estimated parts beneath it, with `≈` marking anything partly
  estimated and `unknown` (never `$0`) where neither number exists. A reported
  row still counts the turns that reported no cost as unknown. Estimates are
  list prices, not necessarily what a subscription bills. **Cost by model**
  draws the same numbers as bars, and **Last 30 days** charts turns per day.
  The new thread page's *Usage* and *This month* spend use the same rule,
  read by model. A model id a router prefixes (`openrouter/openai/…`) is
  priced as the model it names; a `-free` tier never is.
  The CLI's `rustcode usage` stays reported-only.
- **Settings › Diagnostics.** Built in for debugging the RustCode server
  (the `rustcode --acp` process; "server" is its user-facing name,
  [terminology.md](terminology.md)) on macOS, Linux and Windows
  (`src/ui/DiagnosticsPage.tsx`, `src-tauri/src/diagnostics.rs`). *Server*:
  running, starting, stopped or crashed (an exit nobody asked for), process
  ID, version (`rustcode --version`), binary, uptime, config folder and its
  `debug.log`, restarts after a crash and the last exit's code or signal and
  time (`engine.rs` keeps them), with *Restart server* and *Open logs
  folder*. *Server log*: the last 1000 stderr lines live, a level filter,
  search, Pause/Follow, Copy and Save (redacted). *System*: OS and version
  (`sw_vers`, `/etc/os-release` and the kernel, `ver`), architecture, app
  and Tauri versions, the web view (WKWebView, WebView2, WebKitGTK) and its
  version, the GPU WebGL names, display scale, `data-platform` and the
  environment that changes how WebKitGTK composites. *Harnesses*: what
  Settings › Harnesses last detected, with the binary this app's `PATH`
  resolves. *Open thread log* opens the open thread's `logs/debug.log` (also
  in every thread's menu); the shell finds the folder with `rustcode threads path`.
  *Copy debug report* puts all of it, the last 200 log lines and the last 50
  `operational_events` in one Markdown block for a GitHub issue, with
  tokens, API keys, email addresses and the home folder (`~`) taken out
  (`src/lib/diagnostics.ts`); *Save…* writes it where the save panel says.
- **Settings › Advanced › Record request bodies.** The two payload-capture
  switches (`docs/data-store.md` › Captured payloads), off by default:
  *Provider requests (built-in harness)* for the built-in harness's request
  bodies, *Harness messages* for each ACP or Codex JSON-RPC message. They are
  the engine's own global config keys, read and written with
  `_rustcode/config/telemetry` / `_rustcode/config/telemetry/set`
  (`engine/src/acp/telemetry.rs`), so a terminal and this page change one
  setting; a project file still cannot turn either on. The engine reads them
  from the config it started with, so changing one shows *Restart the server
  to apply* right under the switches, the same row the Smart UI and
  orchestration settings use.
- **Usage.** In the composer row, after the SwiftUI app's `UsageCapsule`
  and `PlanUsageMeter`: a ring filling with the context window in use, and
  the plan's shortest window from `_rustcode/usage/limits` as a short bar
  with the percentage left. Clicking the ring opens the context inspector:
  a stacked bar and each category's tokens from the `usage_update`'s
  `_meta.rustcode.context` (the conversation in accent shades, the fixed
  overhead in greys), then the thread's in/out/cached totals. Clicking the
  bar opens Settings › Usage (Open Usage in ⌘K). Hovering the
  bar opens every window at once with a bar, quarter ticks and the time to
  its reset. The plan numbers refresh on hover, when a thread is selected,
  when a turn in it ends, every 3 minutes while the app is in front and just
  after a window resets; a failed refresh keeps the last numbers and says so.
  Each login has its own numbers: the meter shows the account the thread runs
  on, read when the thread first opens, never another login's. A new thread
  shows the limits the last thread learnt. The chat header no longer repeats
  them.
- **Auto-compact.** Settings › General › Threads owns one mode and a size
  threshold. RustCode sends the harness's own `/compact` after a turn when the
  context is above the threshold. The second mode, *Also before the cache
  expires*, adds a second rule: on Claude Code and Codex
  again about 4.5 minutes after a turn ended, while the prompt cache is warm and
  the context is above 60k. That rule is off by default: it once ran four and a
  half minutes after an answer the reader had not come back to, which read as
  the answer going missing. The threshold rule is the default one. Codex uses
  only the cache rule when it is on (it compacts inside a turn itself); the
  built-in harness keeps its own pre-request compaction; any other ACP agent is
  used only when it advertises `/compact`. The decision is pure
  (`state/autoCompact.ts`): it never fires while a turn runs, a message waits,
  an approval is open, the user is typing, the thread is settled, the last
  answer is still unread (the row says Done or Failed until it is opened), or a
  delegate's orchestrator has stopped talking to it, and it runs at most once
  per idle period. The thread menu turns it off for one thread. The compaction
  turn writes no rows of its own (`state/thread.ts` `beginQuietTurn`): the
  answer above it stays the last answer and one row under it says
  *Compacted automatically*, and the context inspector compares the context
  before and after it. A thread the compaction caught while a delegate was still
  working carries on afterwards by itself, with one turn of its own drawn as a
  *Continued after compaction* divider rather than a message the user typed
  (`state/autoCompact.ts` `shouldContinueAfterCompaction`). It is skipped when
  the compaction or the turn before it was stopped or failed, while the user is
  typing or another turn or message already carries the thread, when the user
  snoozed or settled the thread meanwhile, and when nothing is owed any more
  (every delegate finished while it compacted: its wake carries the thread on).
  That turn stays in the idle period the compaction spent, so it cannot compact
  and continue again in a loop. It is not a message the user sent: the
  composer's ↑ does not recall it, and a reloaded thread replays it as the same
  divider (`state/thread.ts` `CONTINUE_MARK`).
- **Model picker.** One pill names the model and its effort (*Opus 5.5
  Medium*) and opens one card with no step between its parts: a harness icon
  rail, a searchable model list and, docked at the right, the current model's
  effort column. The column holds the model's levels, *Reset to default*,
  *Fast mode* (when the thread's model has the `fast` option) and the
  harness's logins, each with the share of its plan left. Picking a level or a
  login keeps the card open; picking a model that has levels moves the
  keyboard to them. Keys: ↑↓ and ⏎ in the list, → at the end of the search
  (or after a pick) into the column, ← back to the search, a printed character
  in the column goes to the search to filter, Esc closes. Each model keeps the
  effort it was last left on (one record per harness, which its logins share),
  so picking the model again returns to that level; a level the model no longer
  offers falls back to its default. The harness row shows only when more than
  one harness is usable. Fast mode is set on a thread once it exists; a new
  thread does not carry it yet. The new-thread picker keeps the harness, model,
  effort and login picked there for the session, so opening a thread and
  coming back to a new one shows that choice rather than the default again.
- **New thread banner.** Settings › Appearance: a wide picture over new
  threads, drawn in coloured text (ASCII, inverted on a light background)
  or as is, filling the whole pane from the title bar down, behind the field
  and the cards so their glass has a picture to frost, under a frosted title
  bar. The picture is copied into the app's data folder
  (`src-tauri/src/banner.rs`; the setting keeps the copy's name), decoded
  once, and the text drawn once per size on an offscreen canvas
  (`src/lib/banner.ts`), so a resize scales the last drawing until the new
  one is ready. It fades in, and does not move under Reduce motion.
  Under the picture, *Darken* (0–80%, black over it), *Blur* (0–40px) and
  *Show in threads* (off by default): on, open threads draw the same picture,
  darkened and blurred the same, behind the transcript, under a heavier wash of
  the pane colour (80%, against 35% on the new-thread screen): a white photo in the dark theme left
  the body text at 4.4:1 and its muted lines at 2.0:1 under the 60% first tried, where 80% keeps
  9.4:1 and 4.3:1. A thread is read line by line and a new thread is not. They are kept with the
  banner (`lib/banner.ts`).
- **Remote access.** Settings › Remote access ([remote.md](remote.md)) is the
  machine's switch: **Network access** turned on starts the remote gateway
  (or uses the one the RustCode server runs, which keeps this computer
  reachable when the app closes) and
  lets paired devices reach every thread on this computer, across all
  projects, not only the ones open here; open threads are also streamed live —
  on the built-in harness and on Codex, Claude Code, Pi or any other external
  harness — and a prompt, cancel, question or approval from the phone acts on
  the desktop thread. The page shows the gateway's address and its **Routes**
  (the LAN, VPN and host-name addresses a phone can dial, best first), the
  pairing QR code and the eight-digit code with the challenge's countdown and
  the wrong codes left, and the paired devices with when each was last seen. A new device appears as *Paired ‹name›* and the used code is taken
  down, not left until it expires. A phone that asks to connect gets a card
  in the window's corner over any page (`src/ui/ConnectRequests.tsx`):
  *‹name› wants to connect*, Deny or Accept; Accept shows the code with
  Copy and a countdown until the phone pairs or it runs out. The app reads
  the machine's status every 2.5 s while the gateway runs for that. Each
  paired device has *Revoke*. It asks the engine's `_rustcode/remote`,
  `_rustcode/remote/start`, `_rustcode/remote/pair`,
  `_rustcode/remote/answer`, `_rustcode/remote/revoke` and
  `_rustcode/remote/stop` over ACP, with no `sessionId` for the machine's own
  switch and pairing; the engine owns the gateway, the shares and pairing
  (`rustcode/engine/src/acp/remote.rs`). The QR code is the engine's own rows
  (`qrRows`) when it sends them, else it is encoded in the app
  (`src/lib/qr.ts`), so no pairing material leaves it. Its *Routes* group lists
  the addresses the gateway reports, marking the advertised one; the phone
  learns the same list from the handshake and reconnects over whichever
  answers. A thread's header carries a chip while it is shared, with its
  attached count.
- **Terminal.** ⌘J: xterm.js on the shell's PTY in the thread's folder, one
  per folder, kept while hidden, following the app's colours.
- **Window.** The window opens at the size it was last closed with
  (`webview.rs` keeps it as logical points in `window.json` under the app's
  config folder; closing the window or quitting writes it, and a missing or
  nonsense file falls back to the default in `tauri.conf.json`).
- **Smart UI.** See below.
- **Look.** Neutral and opaque, after [design.md](design.md): the
  sidebar and every surface paint solid token colours, so greys no longer
  pick up the wallpaper through the window's material (`windowEffects` is
  still set in `tauri.conf.json` but nothing shows it). What floats (the
  composer, popovers, menus, tooltips, dialogs, toasts, the lightbox)
  shares one glass, `--rc-glass-bg` + the shared `glass-filter` on macOS; Windows and Linux paint
  no blur at all and keep every surface solid (design.md › Platforms). Reduce transparency
  (Settings › Appearance, the media query, or the system setting read by the shell's
  `reduce_transparency`) makes the glass solid too. Hover is
  colour only, in 60 ms and out 120 ms; focus rings show for the keyboard
  only; scrollbars are thin and appear on hover, with a stable gutter on
  long scrollers.
- **Approvals** are one card over the composer: what the agent wants, its
  risk (*Read-only*, *Risky*, *Changes files* or *Destructive*, from the tool
  and its input; anything unknown is risky), then *Decline* and *Approve*
  (prominent, once). *Grant* says how long an approval lasts: *1 hour* or
  *Always allow*, the engine's other answers and, when a turn waits on
  several, *Allow all N waiting*. A *Destructive* call — a tree deletion, a
  forced push, sudo, a write outside the folder — gets neither grant, in the
  card, the bar or the inbox: only Allow once and Decline. The row in the
  transcript adds the key–value preview of what would run (command, folder,
  paths); an edit needs none, since its diff header names the file and its
  `+N −M`. An answered card settles into one line (`Allowed edit src/app.ts ·
  once`) for a moment.
  A timed grant is kept by the engine beside the permanent always-allow
  rules and listed with its minutes left under Details › Allowed, where
  Revoke drops it. Keys 1–9 still pick in the engine's order. A child thread's approval or question
  also appears at the foot of the transcript of the thread above it, under a
  header naming the child (a click opens it); answering there answers the
  child through the same card, and it leaves both chats. The composer is glass
  (`--rc-composer-bg`: 80% of the raised surface, 88% on focus; blur 12px,
  16px in dark), solid under Reduce transparency. Queued messages are rows above the composer that can be
  edited in place, moved up, sent now as a steer, or removed. Whether a message
  steers is the harness's `steering` record and the turn's state read by one
  rule (`state/queue.ts` `sendDecision`), the same one the composer's
  running-state hint shows: a message with links, a command token or a file
  queues whole for the next turn, as does a picture on the built-in harness
  (its steer is text) and anything sent while an approval or question waits
  there. The row shows a paperclip count, its text stays editable (the images
  stay), and its send-now arrow is greyed with the reason on hover. The queue
  outlives the window: each thread's rows are kept in app storage as they change
  (`state/queues.ts`, `state/host.ts`) and read back when the thread is opened
  again, a thread that kept one is opened at start-up and its head starts a turn
  once it is idle, and an id that was already sent is never run twice. That is
  also what makes a busy parent's wake durable: a child that needs the user is
  steered into the parent's running turn where its harness takes a live steer
  that keeps its tools running, and queued — never a stopped turn — everywhere
  else ([orchestrator.md](orchestrator.md) › Waking a parent thread).
- **Command palette** (⌘K): recent threads, then actions: new thread (on
  any harness, or in a worktree), board, review, mark for review, settle the
  open thread (unsettle it while it is settled),
  compare a fan-out, panels, every Settings page, appearance, restart.
- **Sidebar.** A *Needs you* row above the list opens the inbox
  (`ui/Inbox.tsx`, `state/inbox.ts`) and is drawn only while something waits
  on the user: an approval, a question, a failed turn, or a delegate asking.
  A thread merely marked for review does not count, and the row's count is the
  inbox's own list. With nothing waiting there is no row at all, so no empty
  heading stands over the thread rows, and the inbox stays reachable from ⌘⇧I
  and the palette. One list, top to bottom: Pinned, then one **Threads** list,
  then Snoozed and Settled as shelves at the foot. **Threads** leads with the
  threads whose turn runs and the threads still waiting on work they handed
  out, the thread you last sent work to first, then the threads waiting on an
  approval or an answer, the one that has waited longest first, then the rest,
  the one that last came back to you first with the completions and failures
  you have not opened above them. A pinned thread that works or needs you
  stays in Pinned and says so in its chip, so no row is drawn twice; a thread
  that asks you something is not a section of its own, the chip is the whole
  of it. A section sorts itself from marks the app already keeps, so it never
  reorders as a turn streams; a row you dragged keeps the place you put it, in
  any section. Only Pinned names itself when it is there; Threads carries a
  heading only while Pinned is drawn above it, and a lone list needs none.
  Section headers never fold and are always open, and a stale `working` or
  `threads` id in `rustcode.sidebar.folded` is dropped on mount so an older
  release cannot shut a list with no control to open it. With Sidebar: Working
  at the top off (⌘K) the list is what it was: Pinned and Threads lead, and
  Working folds into the shelf at the foot beside Snoozed and Settled.
  Settled holds the threads marked Settled and threads untouched for three
  days unless they wait for review (`idleSettled`), folded by default, 5 at a
  time then *Show 25 more*. A thread that works, waits or needs you stays in
  the list and says so in its row, unless it is Snoozed: a sleeping thread
  keeps its place in the Snoozed shelf while its turn runs, while it waits on an
  approval or a question, and after it finishes, and none of that marks it
  unread, counts in *Needs you*, badges the Dock or notifies the system. It
  comes back when its snooze ends, or when you write in it, with the state it
  reached then and one notification at most (`snoozed` in the store;
  `state/store.ts` `wakeSnoozed`, `state/sidebarRows.ts` `placeThread`). Every
  thread row is the same 68px grid of three lines, each a left and a right part
  (`docs/design.md` › Sidebar rows): the title and its state or age, then the
  project and the computer it runs on with the diff and pull request, then the
  branch (or what the thread asks of you) with the model and harness mark or
  the stack of delegates. Renaming edits the title in place, at the row's height. A turn that ended while you looked elsewhere keeps its row at
  full strength until you open the thread (`finished`/`failed` in the store;
  *Mark Unread* in the menu brings it back). Hovering swaps the state for Pin,
  Settle (or Unsettle) and the menu, in the same place, on a 24px target each.
  Dragging a row slides the others aside to show where it lands, and the list
  scrolls near its edges.
  ↑ ↓ move through the rows without opening them, ⏎ opens the
  one the keyboard is on, and P pins (or unpins) it while D settles (or
  unsettles) it — the picked rows when there are several — with the same
  toast and Undo; Shift+F10 and the ContextMenu key open the row's menu, and
  the keys are rebindable in Settings › Keyboard.
  Double-click or F2 renames. Settling is marking a thread Settled, with an
  Undo toast (the row, the selection bar, the chat's menu, the palette and the
  board all offer it); a new message brings it back. Settling the selected
  thread while the chat is showing closes its panel and leaves it for the
  new-thread screen in the same folder. Settling from the board, review or
  compare stays on that view, settling any other thread changes nothing on
  screen, and a status set by an agent or the engine never moves it. The
  filter button by the search groups Threads by project (off by default; each
  project folds and has New thread on hover; a worktree thread sits under
  its repository), keeps to the current project, and opens the Board. Working
  stays above the folders when it has a shelf of its own, and a folded
  folder's dot says what it hides.
  Search matches titles and folders at once, and from three characters
  also what was said: the engine's SQLite index (`index.sqlite`,
  `_rustcode/threads/search`) adds threads whose messages match, with the
  matching words under the title.
  Threads another thread started have no rows of their own while the parent
  exists, whatever the filter or search shows: their parent's row
  ends with a stack of up to three marks for the ones still working or waiting
  on you, then `+N`, a warning dot on one that needs you, and the tooltip lists
  them (`ui/AgentStack.tsx`, `state/agentStack.ts`). Finished delegates show
  nothing there. The open delegate is drawn as an indented row under its
  parent's, folded away when the user moves on, and a search result that is a
  delegate says `in <parent title>`. The chat's agent and thread group cards
  show the same stack.
  The title row carries Hide sidebar and Settings beside it, clear of the traffic lights
  and in the same place with the sidebar hidden (docs/design.md › Title row); it used to sit over Add project and New
  thread buttons and above a foot row of Settings, Pull requests and Usage, all
  gone so the thread list takes the rest of the column; the header's cluster
  holds Show sidebar and Settings while the sidebar is hidden, where the title row's pair stood. There is no search or
  filter in the sidebar: ⌘K finds threads (by title, project, branch, pull
  request and what was said) and holds those five as New thread, Add project
  folder, Open Settings, Open Git and Open Usage, with their shortcuts, and
  the view options. Snoozed and Settled are the shelves at the foot of the
  list, each folding on its own;
  the project is chosen per thread in the composer's footer.
- **Title bar.** One row, 52 px on every platform, is the window's title bar
  and holds the sidebar's brand and toggle with the main header beside it
  (`src/ui/Titlebar.tsx`; `src-tauri/tauri.conf.json` › `trafficLightPosition`
  centres the traffic lights on it). The sidebar's and header's content starts
  14 px past the zoom button (x 92; the button ends at x 78), the gap the
  SwiftUI app leaves; in full screen, where the lights hide, the rows start at
  the edge. On Windows the window has no native decorations
  (`src-tauri/tauri.windows.conf.json`): the page draws minimize,
  maximize/restore and close at the window's top right
  (`src/ui/WindowControls.tsx`, 46 × 52 px each, the close one red under the
  pointer), the row drags the window, and a double-click on it maximizes.
  macOS keeps its traffic lights and Linux its native title bar.
- **One shell.** The sidebar and the main header (`src/ui/MainHeader.tsx`)
  stay mounted for every view. The header floats in glass over its view
  (the chat, new or open), with a soft fade at its foot, and content
  scrolls under it; views leave `HEADER_HEIGHT` (the title row's 52 px) at
  their top.
- **Thread header buttons.** The chat header's buttons come from one
  registry (`ui/ChatView.tsx`, ids in `state/headerButtons.ts`): status,
  plan, pull requests, terminal, details, changes. Settings › Chat › Thread
  header buttons lists them as Shown and Hidden: drag a row to reorder it or
  move it across (pointer capture, so the webview's own drops never see it),
  ⌥↑/⌥↓ moves a focused row, and each row's Show/Hide button does the same
  without a pointer. A button's own condition still applies (no plan, no
  pull requests), and hiding every button leaves the title alone. The two
  panel toggles (details, changes) always draw after the rest, in their kept
  order, so the panel's control is the header's last, rightmost one. The order
  is kept as `rustcode.headerButtons` (`{shown, known}`), so a button added
  in a later release shows by default. Pull requests is hidden on a fresh
  install and after Reset (Open Git in ⌘K holds it); a saved layout
  that shows it keeps it.
- **Settings** takes the whole window: the shell (sidebar, chat, right
  panel) stays mounted and laid out underneath, hidden and `inert`, so the
  thread comes back exactly where it was (scroll, drafts, open rows). A
  224 px nav on the left holds the search field and then the pages, under
  short headings; one page shows at a time in a centred 760 px column
  under its name and a close button, and Esc, ⌘, and that button return.
  Pages open with the last engine answers and refresh behind them, with
  skeleton rows the first time; a row whose value differs from its default
  offers a reset on hover.
- **Loading.** Skeletons stand where content will appear: the sidebar's
  thread rows, the model pill, an opening thread's transcript and Settings'
  lists (`Skeleton` in `src/ui/primitives.tsx`; the sweep stops under
  Reduce motion). The engine says nothing while it starts quickly; after
  2 s a toast says "Starting engine…" with progress (spawned, then
  initializing). When it stops or crashes, an error toast carries Restart;
  when the shell brings it back, "The engine restarted" (`state/store.ts`,
  `onEngineStatus`). The mock takes `?boot=<ms>` (a slow start) and
  `?crash=<ms>` (a crash after the first start) to show these.
- **Continuing on their own** (`state/limitResume.ts`, `state/internetWait.ts`):
  a turn that ends on a plan's limit waits for the reset. Claude Code says it as
  a reply, not a failure (`You've hit your 5-hour limit · resets 2:50pm
  (Europe/Oslo)`), so the whole reply must be that message; the turn's error is
  read the same way, with its `Internal error:` prefix and `{"errorKind":…}` tail
  taken off (a delegate ended on exactly that), and a message of several
  paragraphs counts when each one says the limit. A harness that names no time
  uses the plan's latest full window, and with neither there is no wait. The wait
  is saved with the links the stop settled as failed: the turn that continues
  reopens them and settles them with its answer, and the check also runs when
  the window gets focus after a sleep. The parent is woken once for a wait, not
  at each retry: a link that fails on the limit again stays heard until the
  thread settles for good or gives up. Stopping or archiving the parent cancels
  those links and lets go of the delegate's wait. Codex's `try again at 3:47 PM`
  / `try again in 3 days 2 hours` and Gemini's `quota will reset after 3h42m18s`
  are read as well. A thread the restart cut short is sent its resume, never
  that and a due wait.
  A provider that was overloaded (HTTP 529/503, `overloaded_error`) is tried
  again after 30 s, 1 m, 2 m and 4 m, each with up to a quarter of it added at
  random so an account's threads do not wake together, and no fifth time. A turn
  that failed because the machine had no network at all (Claude Code "Can't
  reach the API server", Codex's dropped stream, Pi's "fetch failed";
  `state/turnError.ts` `networkFailure`) waits for the connection instead of
  dying: while any thread waits the app asks the webview every 5 s, and at once
  on `online` or a wake, then sends "Please continue." once the connection has
  stayed up for 5 s, 30 s, 2 m and then 5 m on later retries, so flaky Wi-Fi
  cannot burn them; after 6 h it stops and says so. A credit or spend limit is
  never retried. A row over the composer says what is waited for and when, with
  **Send now** and **Cancel**. A wait ends without a turn when the thread moves
  to another harness or account, when the user sends anything or stops the turn,
  or under Settings › General (Continue threads on their own). Queued messages
  hold until then. After three waits a limit stays stopped and after four an
  overload; waits survive a restart, and the timer catches up after sleep.
  **Continue threads after a restart** is the same idea for a turn a quit, an
  update or an engine restart cut off (`state/store.ts` `resumeInterrupted`,
  the decision in `state/restartResume.ts`). The turn continues once, with a
  "Resumed after restart" divider in the transcript in place of a message that
  reads as the user's. A turn the user stopped stays stopped, even when the
  quit came before the harness finished cancelling, and a thread whose resume
  was cut off again within ten minutes is left for the user instead of being
  restarted in a loop. A turn that lost the network resumes by the internet
  wait above.
- **Toasts** (`src/ui/Toaster.tsx`, queue in `state/toasts.ts`, `toast()`
  in `state/store.ts`): up to three, newest on top, centred under the chat
  column's header (the window's top while Settings is open), frosted like
  the composer. Info and success leave after 3 s, errors after 7 s; the
  pointer or focus on the stack, a drag, and a backgrounded window pause
  every clock. Hover and focus resume with what was left. A drag past 48px
  up or sideways, Escape, or the close button dismisses, and a drag never
  selects text. A toast may carry one action. The same text again restarts
  the shown toast, and an `id` replaces one in place (the engine's). Use
  them for copying, errors not tied to a message, and engine events, not
  for what the screen already shows.
- **Notifications** (`state/notify.ts` decides, `src-tauri/src/notify.rs`
  shows). A thread that needs the user (an approval, a question), hits a
  usage limit, fails or finishes its turn gets one system notification: the
  thread's title, then one line (`Needs your approval`, the question's first
  line, `Finished`). One gate decides all of it: nothing for the thread on
  screen while the window is in front, nothing at all while the window is in
  front under Settings › General › *Only when RustCode is in the background*
  (on by default; a toast says it there instead), and each event once — an
  approval by its request id, a turn's end by its time — so a state seen twice
  says nothing twice and a launch says nothing about the past. A click brings
  the window forward and opens that thread. A thread the user snoozed is out of
  all of it until its snooze ends: the gate is told which threads sleep, so a
  request or a finished turn on one says nothing now and says its one thing when
  the snooze ends (`wakeEvent`, `wakeSnoozed`). Settings › General › *System
  notifications* (on by default) turns them off; the system's own permission
  is asked the first time one is due, from the shell, so nothing on startup
  prompts. The Dock badge counts the threads that need the user, the unread
  finished ones and the failed ones, and clears as they are opened; a sleeping
  thread is counted when it wakes, not before. The
  shell shows them: the page calls `notify_show`, `notify_permission` and
  `notify_badge`, and the webview's capabilities gain no notification
  permission.
- **Settings › Capabilities.** Skills and MCP servers in one page, through
  `_rustcode/capabilities*`: servers with their state, tool count and
  names, errors and the engine log lines that name them, on/off (servers
  from a project config are read-only), Retry (reconnects a failed or dropped
  server in place, through `_rustcode/capabilities/mcp/restart`, without
  rewriting the config), add (command or URL, with environment or headers),
  remove, and Edit config; skills by where they
  were found (project, user, shared, configured, and a harness's own
  `Claude Code (project)` / `Claude Code (~/.claude)` groups, whose rows carry
  `readOnly` and so have no Edit action) with their path, on/off,
  Edit (opens SKILL.md; never for a read-only row), Show in Finder, and New
  skill (the shell writes
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
- **Thread peek.** Hover a thread this one started and press *Show beside this
  thread*: the side panel's Thread tab reads it live, and *Open* goes there
  (`src/ui/ThreadPeek.tsx`). It does not write; a full pane layout is not built.
- **Timeline minimap.** A strip per message you sent in the gutter left of
  the chat, evenly spaced and squeezed to fit. Strips of the turns on screen
  read brighter. Hovering widens the strips around the pointer and previews
  the message and its answer; clicking jumps there, chevrons step a turn, and
  arrows, Home and End move it from the keyboard. With under 48px of gutter it
  shows on hover. A turn whose checkpoint a rewind can return to is named in
  its hover card (`Checkpoint · turn N`, plus `current` for the head, the
  newest) with *Rewind here* when the harness can go back; the strips carry no
  mark of their own. It reads the virtualizer's row offsets on scroll and
  re-renders only when the turns on screen change (`src/ui/TimelineRail.tsx`).
- **Harness marks.** `HarnessIcon` draws each harness's real mark, inline
  (no fetch, no dependency), in the model pill, the harness picker and list
  (onboarding, Settings), the composer's footer, the sidebar's rows and the thread's
  header: the OpenAI blossom for Codex (currentColor), the Claude mark in
  its orange, the Gemini spark with its gradient (all three from
  `@lobehub/icons-static-svg` 1.102.0, MIT), OpenCode's own mark from
  `anomalyco/opencode` `packages/identity/mark.svg` (MIT; its two greys as
  currentColor at 100 % and 35 %), and the app icon for RustCode; any other
  harness gets a letter. `mono` draws the coloured marks in currentColor.
  The marks are their owners' trademarks, used only to identify the
  integration (a NOTICE in the file says so). A harness `icon` URL is
  preferred when the engine sends one.
- **Model picker.** An external harness lists its models only when asked
  (`_rustcode/harness/models`): a new thread on one asks once per engine
  run (`ensureHarnessModels`), so the model list, effort and the rest are
  there before the thread exists; a stale default model falls back to the
  harness's first.
- **Turn folding.** A finished turn's work (thoughts, tool calls,
  subagents, the replies between steps) folds into one row above its final
  answer, "Worked for 1m 3s" (or "You stopped after 12s"), which opens to
  every step as it streamed; the running turn shows "Working for 12s"
  under its message and everything live below it, and
  replayed threads fold the same way (`src/state/fold.ts`, tests in
  `fold.test.ts`; recipe in [design.md](design.md)).
- **Tool groups.** Inside what shows (the live turn, or an opened fold),
  two or more finished tool calls in a row fold into one row in the tool
  rows' grammar: "Read 4 files and ran 2 commands", the two most frequent
  actions by tool kind and "performed N other actions" for the rest, with
  "N failed" in danger. A call still running and a subagent keep their own rows; a
  search opens every group (`groupTools`).
- **Pictures.** A pasted picture's thumbnail and a sent message's pictures
  open in a lightbox (fit to the window, ←→ between the message's
  pictures, a click zooms, Esc or a click outside closes). A reply's
  `![](path)` shows the picture when the path (absolute, `~/`, `file://`
  or relative to the thread's folder) resolves inside the thread's folder
  or its worktree: the shell's `thread_image` follows symlinks and refuses
  anything else, and anything that is not a PNG, JPEG, GIF, WebP or SVG of
  at most 20 MB. Web and `data:` pictures show as they are.
- **Diagrams.** A ```` ```mermaid ```` block loads mermaid (12.2.0, its own
  chunk) the first time one appears, themed from the tokens (`base` theme,
  re-rendered on a theme change), rendered with `securityLevel: "strict"`,
  200 ms after a streaming block stops changing. Over 12,000 characters or
  300 lines, or when it does not parse, it stays code with a note. A click
  opens the diagram in the lightbox.
- **File references.** Inline code naming a file (`src/a.rs:12`,
  `src/a.rs:12:4`, `Cargo.toml`) and relative links (`[a](src/a.rs#L12)`)
  open the file in the editor at that line, resolved against the thread's
  folder (`open_in_editor`: VS Code, Cursor or Zed when installed, else
  the default text editor).
- **Right panel.** Changes, Agents and Pull requests share one floating card at
  the window's right edge (`src/ui/RightPanel.tsx`): the thread details card's
  surface, full height, inset from the window's edges by the same gap, with
  tabs between them, a close button, a draggable left edge (kept, and moved
  with ←/→ or Home/End), Escape to close, and ⌘⌥B to toggle it. Each thread
  keeps the view it was last showing, and opening it restores exactly that;
  a thread never seen, or one whose panel was closed by hand, opens without
  one, and nothing opens the panel on its own. The new-thread screen opens
  without a panel (⌘⌥B still opens one), and settling the selected thread
  while the chat is showing closes it and leaves that thread. Its Changes read
  belongs to the thread's
  folder: an answer for the last thread's folder is dropped and never drawn
  with this thread's name, and while the turn runs the panel says the changes
  are read when it ends. It takes no slot, so the chat keeps the pane and the
  lane centres in the room the card leaves, narrowing only when that room is
  too small to read; under 320px of room the card floats over the lane, which
  steps clear as far as it can. The card stops above the composer.
- **Threads working for threads** ([orchestrator.md](orchestrator.md),
  M1 rules from
  [cross-session-orchestration.md](architecture/cross-session-orchestration.md)).
  On by default (Settings › Harnesses turns it off). The app answers `_rustcode/orchestration`
  (`src/state/host.ts`, rules in `src/state/orchestration.ts`):
  `list_sessions`, `read_session`, `message_session`, `wait_session`,
  `start_session` and `fan_out`. Same project only — the repository, so a main
  checkout and all its worktrees are one project — and a thread it started is
  always reachable (another project only
  for `start_session`, one the user has opened, with approval), two levels of
  delegation (a top-level thread, its children and their children — no
  deeper, and no message that would close a loop), 8 delegates per thread and
  16 across this app instance, a 32-message mailbox, 8 KiB messages, 16 KiB reads,
  waits up to 1 h. Each needs the user's *Allow for This Thread*,
  *Allow Once* or *Don't Allow*; a grant is per calling thread and kind of
  change, not per harness, model or named login, so an already-granted start
  may name another login without another card. Grants and links are kept in the
  local data store (`data-store.md`), with local storage as the first paint and
  for an older engine, and after a restart active links are *interrupted*.
  Threads the restart cut off are resumed with a *Continue* message (Settings ›
  General, on by default), and that resume turn is what answers the links the
  quit severed: they work again from the resume, and only that turn settles
  them (a turn the user starts there later does not), so the parent that was
  waiting is woken with the answer instead of waiting for good
  (`docs/orchestrator.md`).
  Started threads go through `session/new` with `_meta.rustcode.harness`,
  `.account` (orchestration v5) and `.worktree` and get the prompt as their
  first message, headed
  `From thread "<title>" (agent):`. A child the orchestrator did not name is
  named from the task's first words (`childTitle` in `src/lib/title.ts`), so
  that header never becomes a thread title in the sidebar. The parent is also
  recorded in the child's own metadata (`_meta.rustcode.parent`), so a pruned
  link cannot turn it into a top-level row: it sits under its parent, or in
  Settled once finished when the parent is gone, and only a child that works or
  waits on you, or that you pinned or messaged, is a row of its own
  (`state/topLevel.ts`, `docs/orchestrator.md`). Forks (`/fork`, the fork button under a
  reply) and side chats (the side-chat button beside it) record their parent the
  same way with a `relationship` and nest under it with a glyph
  (`docs/orchestrator.md`, *Forks and side chats*). Chips above the composer (state, open,
  stop), a *Working for* banner, sidebar badges. Stopping a thread stops the
  work it gave others, and theirs, in one cascade. A thread whose turn ended
  while the threads it handed work to still run says so instead of looking
  finished: *Waiting on 2 threads* at the foot of the transcript (the titles in
  its tooltip) and *Waiting* in its sidebar row's status slot, one state read in
  one place (`src/state/waiting.ts`). A child waiting on an approval or a
  question shows there too, one card per request, oldest first, with the thread
  it came from in the header; a grandchild's request reaches the top-level
  thread once (`src/state/pending.ts`).
- **Worktree threads.** The new-thread composer's *Worktree* toggle starts
  the thread in its own worktree, with a branch name suggested from the
  message (`<type>/<slug>`) that can be edited. A worktree thread's menu
  opens it in Finder or Terminal and removes it, asking first and again with
  the engine's reason (uncommitted changes, unpushed commits).
- **Status and Board.** Running, Needs input, Review and Settled chips on rows
  and in the chat header (a menu marks Review or Settled). *Board* (the
  ⌘K) shows those four columns; drag a card to
  Review or Settled.
  Marks go to `_rustcode/session/status`; on an engine without it the app
  keeps them itself. `_meta.rustcode.status` from the engine (a merged
  branch's auto-done) updates the chip.
- **Fan-out compare.** A `fan_out` adds *Compare* to the chips: one card per
  harness with its status, branch, files changed and diff stat of the
  branch, its answer, Open and Review. *Keep this* marks the others done
  and the kept one for review.
- **Checkpoints and Review.** A finished turn has a *Changes* chip from its
  checkpoint (`_rustcode/checkpoints/list|diff`) with *Rewind to before turn N*:
  the files go back to before that turn and, when the harness rewinds, the
  conversation with them; the turns it leaves stay below as a faded earlier
  attempt whose *Go back to this attempt* restores the state the rewind
  abandoned (`_rustcode/checkpoints/restore` with `keep`). A turn with nothing
  folded keeps its chip in its footer, so every turn that has a checkpoint can
  say what it changed. The minimap names each turn that has a checkpoint in
  that turn's hover card, with *Rewind here*, which is the chip's rewind: one
  click when the engine keeps the state a rewind abandons, behind its confirm
  when it does not. A harness that cannot rewind the conversation gets the
  files only, after a confirm that says so. The Review screen
  (thread menu, header menu or ⌘K) shows the uncommitted changes, the
  worktree branch since its base (the base named beside the file count, as
  `vs main`, with the full ref on hover), one turn, or the branch's open pull
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
icons at its top right on hover or focus. A complete block runs by itself
when it comes into sight, also in a chat opened later, and shows its title
at its reserved height until then; Run stays only for one you stopped or one
that failed. At most six run; starting a seventh stops one out of sight, else
the oldest, which waits until it has left the viewport and come back. 64 KB of source, 5,000 elements,
at least 48 px tall and growing to its content's full height (only `.rc-scroll`
or the view's own CSS scrolls); a page that does not load in 5 s,
goes 3 s without its heartbeat (a busy loop) or builds too many elements is
stopped with its source shown; one out of sight for 30 s is torn down.

Each page gets, before the block (`src/smartui/page.ts`): the CSP
`default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline';
img-src data:; font-src data:` (plus `connect-src`, `frame-src`,
`worker-src`, `object-src`, `form-action`, `base-uri` `'none'`); the
variables `--rc-text … --rc-series-8` resolved from the app's theme and
updated live; base styles; the `.rc-*` layout classes and component kit;
Chart.js 4.5.1 (MIT,
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
Settings › Chat › Smart UI is on (default), else 0; the engine reads it at
`initialize`, so Settings offers Restart Engine after a change.
`rustcode-viz` blocks are drawn by the app itself (Features, Visualizations).

In dev builds `window.__rustcode` exposes the store and its actions, so an
agent can drive and inspect the UI (as `DebugDriver` does for the SwiftUI
app), for example from a headless browser over the Chrome DevTools Protocol.
In `apps/desktop`, `bun run mock-window open` is the shared, headless browser for
that; see AGENTS.md › Checking desktop app behavior.

## Native text fields

The app's fields hold prompts, paths, branch names and search terms, where the system's text
helpers are noise: they rewrite what the user typed and fight the app's own state. All of them are
off, at three layers, because no single one reaches every macOS version:

- **The fields themselves** (`ui/primitives.tsx` › `noTextHelpers`, spread into every `<input>` and
  `<textarea>` the app draws): `spellCheck={false}`, `autoCorrect="off"`, `autoCapitalize="off"`,
  `autoComplete="off"`. Plus `writingsuggestions="false"`, which WebKit reads on macOS 15+ to stop
  Apple Intelligence's inline completion. `lib/quietNative.ts` puts the same attributes on any field
  that arrives in the document without them, which is the net under a field added without the
  spread. It does not watch the attributes: React writes a prop it is given once, at mount, so a
  re-render does not take them away, and watching them would re-enter the observer for every write
  it makes itself. Smart UI's frame is a separate sandboxed document the sweep cannot reach, so
  `src/smartui/page.ts` › `pageAt` puts the same four attributes on the frame's `<body>`, which
  every field the block draws inherits.
- **The web view's configuration** (`src-tauri/src/webview.rs`, macOS 15+):
  `WKWebViewConfiguration.writingToolsBehavior = NSWritingToolsBehaviorNone`, so Writing Tools does
  not offer itself on a selection's context menu. It is applied behind
  `respondsToSelector:setWritingToolsBehavior:`, so a macOS without the property (14 and earlier)
  builds the window exactly as before.
- **The app's own defaults domain** (`src-tauri/src/webview.rs` › `quiet_text_helpers`, run before
  the window is built, since WebKit reads them when it creates an editor):
  `NSAutomaticSpellingCorrectionEnabled` (autocorrect), `NSAutomaticCapitalizationEnabled`,
  `NSAutomaticQuoteSubstitutionEnabled` and `NSAutomaticDashSubstitutionEnabled` (smart quotes and
  dashes), `NSAutomaticPeriodSubstitutionEnabled` (double-space period),
  `NSAutomaticTextReplacementEnabled` (Text Replacement), `NSAutomaticInlinePredictionEnabled`,
  `WebAutomaticSpellingCorrectionEnabled`, `WebAutomaticTextReplacementEnabled`,
  `WebContinuousSpellCheckingEnabled` and `WebGrammarCheckingEnabled`.

This writes only the app's own preferences (`~/Library/Preferences/<bundle id>.plist`), never the
global domain: no `defaults write -g`, and nothing another app sees. It is done on every launch, so
a user cannot turn one of these back on for this app; that is the point of the round, and Settings ›
Diagnostics is where a future toggle would live if one is wanted.

What the platform does not allow: `autoCorrect`, `autoCapitalize` and `autoComplete` are hints
WebKit honours for form fields and nothing else, and there is no public API to stop the system's
Text Replacement dictionary from expanding on a space — the defaults keys above are the documented
way, and the field attributes are what remains if a macOS build ignores them. Nothing in the app's
own chrome is left to the system; a harness's login, which runs in Terminal, is outside it. The
chat composer used to keep spell checking on for prose; it is off there too, like every other
field, which is the round's point: the app does not tell a prompt from a paragraph.

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

After the loading, toast, glass header, full-window Settings, pictures,
diagrams and turn folding round (`perf prod`, same machine, before / after):
startup thread list 261 / 244 ms; first rows of the 2,000-message thread
149–163 / 152–221 ms (runs vary by about 60 ms; its longest task 62–80 ms
in both); reopening it 6.5 / 5–13 ms; streaming 60 fps with 2 dropped
frames, key-to-frame p95 15.3 / 15.7 ms, main thread 22 / 23 % busy; idle
1–2 %. Mermaid is its own lazy chunk; the opening thread's skeleton shows
only after 200 ms, so a fast open never draws it.

Still open: the sidebar's 500 rows are not virtualized, so every store
notification runs their selectors (about 3 % of the main thread while
streaming) and filtering re-renders them. (The vibrancy and
`backdrop-filter` cost this paragraph used to suspect was measured
natively; see below: it is not measurable.)

### Native frame rate

`bun run perf:native [prod|dev] [--near60]` runs the same kind of scenarios
in a real WKWebView (`scripts/perf-native.swift`,
`scripts/perf-native-scenario.js`): a floating 1440×900 window set up like
the app's (transparent web view over sidebar vibrancy), against
`?transport=perf`. Per phase it prints the rAF rate, frame-interval p95,
late frames (over 1.5× the display's interval), the main thread's
rendering work after each rAF, long tasks (gaps in a 10 ms heartbeat:
WebKit has no Long Tasks API) and the CPU of the WebContent, GPU and
WindowServer processes. Scrolling is scripted (`scrollTop` per frame): a
proxy, since WebKit scrolls wheel and trackpad input off the main thread
and that input cannot be synthesized here. `sample` cannot attach to the
system WebContent process (SIP), so causes come from this split, a
CDP profile of the same build in Chrome, and A/B runs. In the dev app,
⌘⌥F toggles an overlay (`src/lib/perfOverlay.ts`) with the rAF rate,
frame p95, late frames, long tasks, the display's refresh rate, Low Power
Mode and WebKit's 60 fps preference, and logs them to the console every
5 s.

What held the app below 120 fps, and what changed:

- **WebKit renders near 60 fps by default.** Its "Prefer Page Rendering
  Updates near 60fps" feature (on in every WKWebView) runs rAF, CSS
  animations and rendering updates at 60 on a 120 Hz display. The main
  window is now built in `src-tauri/src/webview.rs` (`"create": false` in
  `tauri.conf.json`) on a `WKWebViewConfiguration` with the feature off,
  through the private `_WKFeature` list; every selector is checked first,
  so a WebKit without it leaves the default. WebKit reads it when the page
  is created: set on a live web view it does nothing, so it cannot be
  applied in `setup` to the config-built window.
- **Low Power Mode caps WebKit at 30 fps**, whatever the preference says.
  This Mac had it on for AC power only (`pmset -g custom`: `powermode 1`
  under AC Power), so the app ran at 30 fps whenever it was plugged in.
  The overlay says when it is on; it is the user's setting, not the app's.
- **Shiki's JavaScript regex engine stalled scrolling.** The first
  TypeScript block took 976 ms to highlight in JavaScriptCore (68 ms in
  Chrome, which is why `bun run perf` never showed it), in one main-thread
  task, as a code block scrolled into view; each 200-line block then took
  60 ms. Highlighting now runs in a module worker
  (`src/lib/highlight.worker.ts`) on the Oniguruma WebAssembly engine
  (73 ms first block, 13 ms per 200 lines, none of it on the main thread);
  the main thread keeps the HTML cache, so a remounted block is coloured
  in its first frame. The app's CSP needs no change: Tauri sends it only
  with HTML, and the worker compiles the WebAssembly under the app's
  policy too (checked with it as a meta tag).
- **The right panel animated its slot's width**, so the chat column
  reflowed, every visible row re-measured and the transcript re-rendered
  on each frame for 200 ms (about 40 ms of rendering work per frame). The
  slot now takes its width at once and the frame slides and fades in on
  the compositor (`opacity` with `translate`): one reflow
  per toggle, a quarter of the main-thread work. (Its slide had never run:
  Tailwind 4's `translate-x-*` sets `translate`, which
  `transition-[opacity,transform]` did not list.) The sidebar did the same
  with its own width; at the default 1280 px window the chat column is
  narrower than its 1,036 px maximum, so each frame rewrapped the
  transcript. It now slides out on `translate` and its slot collapses
  after 200 ms (and opens before it slides in).

  The panel was later docked beside the chat rather than floating over it, and
  that one open and close animates the dock's width again (200 ms, a product
  decision: the panel is a column now, so it takes a column's room). What keeps
  it cheap is that nothing else moves: its content is pinned to the card's right
  edge and clipped, a drag writes one custom property per frame with React state
  committed on release, and every other width change (a tab, a window resize, the
  end of a drag) is committed with the transition off in the same frame.
- **Glass costs nothing measurable.** With `--rc-glass-filter: none` and
  the header glass off, frames, WebContent, GPU-process and WindowServer
  CPU stayed within run-to-run noise in every phase, so the blur stays.

`perf:native prod`, built-in 120 Hz display, Low Power Mode off, before
(near-60 preference on, as the app shipped) and after the preference fix:

| | near-60 on | near-60 off | blur off |
|---|---|---|---|
| Idle rAF | 60 fps | 121 fps | 121 fps |
| Streaming 100 chunks/s, composer focused | 59 fps, p95 18 ms | 118 fps, p95 9 ms | 119 fps |
| Scripted scroll of `perf-long` (incl. the Shiki stall) | 45 fps, one 1,010 ms frame | 91 fps, one 979 ms frame | 91 fps |
| ⌘K switcher open/close ×3 | 59 fps, max frame 59 ms | 114 fps, max 57 ms | 114 fps |
| Settings open/close ×2 | 60 fps, max 37 ms | 117 fps, max 34 ms | 117 fps |
| Right panel ×4 | 49 fps, p95 45 ms | 80 fps, p95 41 ms, work p95 39 ms | 79 fps |
| Sidebar ×4 (1440 wide) | 60 fps | 121 fps, p95 10 ms | 121 fps |
| CPU while streaming: WebContent / GPU / WindowServer | 34 / 14 / 54 % | 46 / 21 / 52 % | 46 / 19 / 50 % |

Twice the frames cost the WebContent and GPU processes about a third more
CPU while streaming. The Shiki stall and the panel's per-frame reflow are
fixed after these runs: in WebKit the scroll no longer has a long task
when a code block mounts, and four panel toggles take 77 ms of rendering
work instead of 262 ms (one reflow per toggle, about 56 ms, instead of
one per frame).

Dev mode (`bun tauri dev`) adds React's development build on top: in
Chrome, streaming keeps the main thread 31 % busy in dev against 22 % in
a production build (table above). Native dev and bundled runs on the same display are still to be
recorded with `perf:native dev` and `perf:native prod`.

Still not 120: opening the ⌘K switcher (a 57 ms frame) and Settings
(34 ms) each drop a few frames in one long React render, and the one
reflow when the right panel or (at narrow widths) the sidebar opens or
closes, about 56 ms of re-measuring the visible transcript rows, is a
visible hitch.

## Release

`bun run bundle` builds the engine in release mode and copies it to
`src-tauri/binaries/rustcode-<target triple>` (`scripts/bundle-engine.ts`),
then runs `bun run bundle:app`: `tauri build` with
`src-tauri/tauri.bundle.conf.json`, a generated
`src-tauri/gen-version/tauri.conf.json` and a generated
`src-tauri/gen-icon/tauri.conf.json`. `bundle:app` alone bundles whatever
sidecar is already in `binaries/`, which is how CI reuses the engine it
built for the CLI. Extra arguments go to `tauri build`, e.g.
`bun run bundle:app --bundles app`.

- **Identity.** `tauri.bundle.conf.json` gives the bundle the SwiftUI app's
  identifier, `com.lhagfoss.rustcode.desktop`, so the released app replaces
  it in place (Dock pin, Launch Services, notification permissions); why that
  is safe for both apps' data is in
  [install.md](install.md#taking-over-from-the-swiftui-app). `tauri.conf.json`
  keeps `dev.rustcode.desktop`, so `tauri dev` never shares data with the
  installed app. It also adds `"externalBin": ["binaries/rustcode"]`, so
  `tauri dev`, clippy and CI never need the engine built, and the targets
  (`app`, `nsis` per-user, `appimage`, `deb`; each platform builds its own).
- **Version.** `scripts/bundle-version.ts` writes the root `Cargo.toml`
  version (the engine's, the release tag's) into `gen-version`, so the
  bundle, the installer and the engine agree. `tauri.conf.json`'s `0.1.0` is
  only the dev build's.
- **Icon.** The bundle carries the SwiftUI app's layered macOS 26 icon:
  `scripts/app-icon.ts` compiles `src-tauri/icons/AppIcon.icon` with `xcrun
  actool` into `Assets.car` (mapped into `Contents/Resources` by `gen-icon`),
  and `src-tauri/Info.plist` sets `CFBundleIconName = AppIcon`, with
  `icons/icon.icns` as the fallback when actool is missing. `tauri dev` runs
  the bare binary, so the Dock shows the flat icon there.
- **Sidecar.** The shell finds the engine next to its binary
  (`Contents/MacOS/rustcode`; a debug build still prefers the workspace's
  `target/debug/rustcode`).
- **Assets.** `scripts/release-assets.ts <dir> [--sums]` copies the bundles
  under their release names: `RustCode-<ver>-macos-aarch64.app.zip`,
  `RustCode-<ver>-windows-x64-setup.exe`,
  `RustCode-<ver>-linux-x86_64.AppImage` and `.deb`; `--sums` writes a
  `SHA256SUMS` for a local installer test (see
  [install.md](install.md#testing-the-installers-without-a-release)).
- **CI.** `build.yml`'s `desktop-macos` job (also for a beta) and `desktop`
  job (Windows, Linux) build these from the CLI jobs' engine artifacts, and
  the release lists them in `SHA256SUMS`. Releases up to 1.2.0 also published
  the macOS zip as `rustcode-macos-aarch64-swift-app.zip` so SwiftUI installs
  could update into this app; that bridge has ended.
- **Signing.** The macOS bundle is ad-hoc signed (`signingIdentity: "-"`);
  with the `MACOS_*` secrets (docs/macos-release-signing.md) the job passes
  the Developer ID as `APPLE_SIGNING_IDENTITY` and notarizes and staples the
  app. Windows and Linux packages are unsigned.

### Haptics

On macOS the shell's `haptic` command plays an NSHapticFeedbackManager
pattern (`generic`, `alignment`, `level`) on the main thread
(`src-tauri/src/native.rs`), through `src/lib/haptics.ts`. Sending or
queueing taps `generic`, stopping a turn `level`, resting on an enabled Send
`alignment`; repeats within 40 ms drop. Only a Force Touch trackpad with a
finger on it feels them. Settings › General › Trackpad haptics turns them
off (`rustcode.haptics`).

### Updater

Per-OS first-run and post-update steps (PATH, quarantine, launcher entry, the
background server) and their Settings page are in [system-setup.md](system-setup.md).

The app checks for an update 5 s after launch, every 30 seconds while its
window is in front (every 30 minutes behind it), when its window comes back,
and 30 s after a failed check, doubling to 5 minutes (`src/state/updates.ts`).
A check is a HEAD request to the release page on Stable, and the release feed
on Beta: neither is the GitHub API, which a 30-second check would exhaust. A
release counts as out only once the package this copy installs from is
uploaded and its `SHA256SUMS` lists it: the release goes public minutes before
the other platforms and the sums land, and an install verifies against them.
When a newer release is out, an *Update to X* row
sits at the foot of the sidebar: it installs in place where the app can
(below), else opens the release page; while it installs, the row shows a
progress bar (`Downloading · 39% of 58.2 MB`, then Verifying and Installing)
fed by the `update_install` command's progress channel. The first launch of a
new version shows *What's New* with that release's notes. It decides from the
app's own version (`getVersion`, no network) against the version seen last
launch and the `rustcode.updatedFrom` marker an in-app install writes, so it
shows even offline or after a failed check; the notes are that version's
section of the repo's `CHANGELOG.md` at the release's tag — what
`scripts/release.sh` writes from each PR's `## Changelog` line — fetched by the
shell's `release_notes` command, which falls back to the GitHub release body
when the tag has no section. The dialog opens at once with the notes' shape as
a skeleton, and reads either form through `src/lib/releaseNotes.ts`: one badge
per category, GitHub's `by @user in <pull url>` tail as a compact `#N` link,
and the release commit and full-changelog line out of the way. A first install
and a dev build skip
it; Settings › Advanced opens it on demand and shows why the last background check failed. `?updatePreview` in the dev browser
shows both with sample data.

`src-tauri/src/updater.rs` ports the SwiftUI app's `Updater.swift`; Settings ›
About shows the version with **Check for Updates**. **Update channel**
(Stable, the default, or Beta) picks what a check looks for: Stable reads the
tag that `github.com/LHagfoss/rustcode/releases/latest` redirects to, and Beta
reads the release feed (`releases.atom`, which lists the prereleases
`releases/latest` never points at) and offers the newest version of either
kind, so `1.21.0-beta.3` is offered above `1.20.0` but `1.21.0` takes over
when it ships. Both compare versions as semver does, prereleases included
(`1.20.0-beta.2 < 1.20.0`) and with build metadata ignored, and only a release
whose app zip and `SHA256SUMS` are uploaded is offered, so a tagged beta with
nothing to install yet stays hidden. The picker asks the shell for the
channels it offers (`update_channels`, no release check), so it is there
before a check answers and when one fails offline; a change re-checks, and an
answer for the previous channel is dropped rather than shown, which is why
the row only draws an offer whose `channel` is the selected one. A beta ships
the macOS app only, so the channel row is hidden on platforms without one,
and switching back to Stable never offers a downgrade (a beta is already newer
than the release before it). On Beta the row reads **Update to
1.21.0-beta.3**, with **Skip this beta** under it: that version is kept out of
the row across restarts (`rustcode.skippedBeta`) and the next beta is offered
as usual. **Install updates automatically** (Settings › About, on by default,
`rustcode.autoInstallUpdates`) installs an offered release by itself once no
thread is working: no running turn, no background command a turn waits on, no
pending approval or question (`autoInstallDecision` in `state/updates.ts`).
While something works it asks again every 10 seconds and never interrupts a
turn; a skipped beta stays skipped and a dev build never installs. An automatic
install that fails is tried again after 1, 5 and 15 minutes
(`AUTO_INSTALL_BACKOFF_MS`), and then left to the sidebar row; every failure
stays on the row (*Retry update to X*, with the reason under it) until the next
install starts. A click on **Update to X** while a thread is working asks
first (`requestUpdate`): **Update When Idle** waits for the threads and then
installs by itself (the row reads *Waiting for threads to finish*), **Update
Now** quits and ends their turns. The download itself is tried up to three
times when the network drops or GitHub answers 5xx/429 (waits of 2 and 6
seconds, the bar starting over), has no cap on its total time but fails after
30 seconds without a byte, and a 404 says the release has no app for this
platform yet instead of a bare status. Each step (`update.started`,
`download.retry`, `update.verified`, `update.installing`, `update.failed`)
is written as a line to `updates.log` in the config folder, next to
`debug.log`. It shows *Updating to X…*, installs and
reopens like **Update to X** does, so the app starts its bundled engine anew
and, in a release build, its start-up catch-up restarts a background server
onto the new binary with `rustcode server restart --when-idle`. **Update to X** installs in place on macOS, Linux (AppImage and
`.deb`) and Windows; how depends on how this copy was installed
(`Install` in `updater.rs`, decided from `$APPIMAGE`, the binary's path,
`dpkg-query -S` and the files beside it). Every route first downloads the
package, checks it against the release's `SHA256SUMS` in memory and refuses a
mismatch, so nothing unverified is written or run:

- **macOS app bundle**: `RustCode-<ver>-macos-<arch>.app.zip`, unpacked with
  `ditto`; `codesign --verify --deep --strict` and this app's bundle id are
  required, then a detached shell waits for the app to quit, swaps the bundle,
  re-registers it with Launch Services and opens it.
  Run `scripts/macos-stable-signature.sh` once so Keychain stops asking after updates; the updater signs ad-hoc bundles with that local certificate before relaunching. `scripts/install.sh` signs the app it installs the same way, and `scripts/macos-stable-signature.sh /Applications/RustCode.app` re-signs an app that is already installed.
- **Linux AppImage** (`$APPIMAGE` is set): `RustCode-<ver>-linux-x86_64.AppImage`
  is written next to the running file, made executable and renamed over it
  (atomic, no root); a detached shell then starts it once the app has quit.
- **Linux `.deb`** (a dpkg-owned binary under `/usr`, no `$APPIMAGE`, not
  Flatpak or Snap): `RustCode-<ver>-linux-x86_64.deb` is saved in a new
  `0700` folder and installed with `pkexec apt-get install -y <file>` (`pkexec
  dpkg -i` when there is no apt-get); polkit asks for the password. A
  dismissed dialog, a missing `pkexec` or a failing package tool shows the
  reason in a toast with a **Release page** button. The app reopens from the
  path it started from.
- **Windows** (an NSIS install: `uninstall.exe` beside the app):
  `RustCode-<ver>-windows-x64-setup.exe` is saved to a new temp folder and run
  as `setup.exe /S /UPDATE /R` (the flags Tauri's NSIS template reads: silent,
  keep the install and skip its pages, start the app again), the path passed as
  one argument and never through a shell. The app quits so the installer can
  replace its files; it needs no administrator rights (`installMode:
  currentUser`).

Any other copy (a tarball or AUR/manual install, Flatpak, an AppImage or
bundle in a folder RustCode cannot write, Linux or Windows on arm) cannot
replace itself: the button opens the release page, and a read-only folder says
so in the row. A `tauri dev` build always sees an update but never installs it.
An install that fails leaves the running app as it was and shows the reason
with a **Release page** button. The channel is the app's own: Homebrew and the
CLI updater stay on stable (`scripts/release.sh --beta` publishes a GitHub
prerelease both skip).

**After an update, the copies outside the app catch up.** The updater replaces
the app and its engine; the next start of the new app brings the rest to the
same version (`src-tauri/src/service.rs` `catch_up_after_update`, issue #3487):

- The `rustcode` a terminal runs — the install script's copy in `~/.local/bin`
  or `/usr/local/bin`, `~/.rustcode/bin`, or whatever the login shell finds
  first — is replaced by `rustcode server update-cli` when it is older and this
  app may own it. Homebrew's copy and `cargo install`'s are named as theirs and
  left to `brew upgrade` / `cargo install`, a development build is never
  touched, and a link that already points at the app needs nothing.
- A service installed from an AppImage gets the new engine copied to it.
- A background server still running the engine from before the update is
  restarted (`rustcode server restart --when-idle`, so a running turn finishes
  first). When its definition names another `rustcode`, the app rewrites the
  definition to its engine while that is still the program it installed the
  service with, and otherwise leaves it alone and the Machines page says
  *Server runs an older rustcode at `<path>`*, once per version pair.

Paired machines that lose this computer while its server restarts reconnect on
their own, as they do after any drop.

It does not use `tauri-plugin-updater`: that plugin requires its own
signing key pair, and a release without the private key in CI would fail.
`SHA256SUMS` over HTTPS from the release is what the installers and the
SwiftUI updater already trust; Developer ID signing adds to it once set up.

## Not yet ported

Linked pull request watches (`_rustcode/pulls/watch`) deliver each pending event
once into the linked thread through its ordinary turn queue, including after
a reconnect. The live thread status update also applies the review badge.

The mock’s `?pullWatchPreview` path seeds a linked PR check failure.

From the SwiftUI app, still missing or partial:

- Picking diff lines for a message. Files dropped in
  the browser are inlined as text rather than linked.
- Slash commands the app answers itself (`/terminal`, `/remote`,
  `/visualize`).
- The retention sweep, swipe actions; loading
  the thread list page by page. Trash moves the folder into `~/.Trash`
  (macOS); Finder's Put Back does not know it.
- A file reference that is changed in the turn does not open the Changes
  or Review diff yet.
- Sticky turn bar, stream pacing, quote in reply.
- The auto-approve (YOLO) toggle on approval cards.
- Subagents' verification status and background agents of threads off
  screen (subagents of a thread whose agents were listed nest under it).
- Pull request line comments and the diff view in the review.
- Changes panel: branch mode, renames' old paths, syntax colours in diffs.
- Localhost preview, the Smart UI agent preview (`smart_ui_preview`,
  `smartUi = 2`). `rustcode-viz` cards have Copy source but not Copy as
  Text or VoiceOver summaries.
- A link's `cancel_session_turn` and the target's *Pause messages* / *Detach*
  are not offered.
- The vibrancy was not checked by eye in a native window yet (the display
  was off during the change); the CSS and config were.
- GitHub sign-in flow, model account switching (Connections is read-only),
  keybinding editor, Discord presence, revoking a paired device (Settings ›
  Remote access lists them; `rustcode remote revoke` in a terminal revokes).
- Terminal tabs (one terminal per folder).
- An E2E driver for the native window; Developer ID signing and
  notarization run only once the secrets exist (#2034). The
  Smart UI `rcui://` path was checked by unit test and the page itself in
  Chrome; a click in a view inside the native window was not exercised.

### Windows child processes

New non-interactive engine process launches must use `platform::command` or `platform::tokio_command`; shared core code uses `rustcode_command::hidden_command`, and the desktop shell uses `process::command`. These helpers prevent console windows when the engine runs under the desktop app. PTY-backed interactive terminals retain their own console handling.
