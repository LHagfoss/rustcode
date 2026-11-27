# Computer use: browser preview

RustCode can expose five browser tools through its own `rustcode` MCP server:
`browser_open`, `browser_read`, `browser_screenshot`, `browser_click` and
`browser_type`. External harnesses use the same injected server as the thread
and orchestration tools; the built-in harness exposes matching tools through
its normal tool permission gate. Every call is forwarded to the engine, authenticated
with the thread token, and asks through RustCode's approval card. Missing
approval channels, denial and cancellation fail closed.

## Enable

This preview is **off by default**. Open **Settings → Voice & computer use** on
this machine, turn on the switch in the **Computer use** title (Browser tools)
and add allowed domains. The section
reports when Chrome/Chromium needs installing. Every action asks for approval
unless the thread holds a site grant (below).
Only headless mode is currently supported. You can also edit your user
`~/.config/rustcode/config.toml` (or `RUSTCODE_CONFIG_DIR/config.toml`), add:

```toml
[computer_use]
enabled = true
allowed_domains = ["example.com", "*.example.org"]
# Optional, when Chrome/Chromium is not in a standard macOS/Linux location:
# executable = "/path/to/chromium"
```

Start a new thread after changing the setting. Ask it to open
an allowed URL, approve the action in RustCode, then read, click a CSS selector,
or type into a CSS selector. Each step asks for approval, including reads and
screenshots, until you allow the site for the thread. Auto-approve does not skip these cards on any harness,
and a headless `rc` run, which has nobody to ask, refuses the tools. Plan mode
refuses `browser_open`, `browser_click` and `browser_type`; reads and
screenshots still ask. Turn `enabled` off to refuse subsequent calls. No browser is
installed or launched until the first approved action.

### Allowing a site for a thread

When the card knows the page's site, it also offers **Allow staging.example.com
for this thread**. After that, in that thread, `browser_read`,
`browser_screenshot` and `browser_open` on the same origin (scheme, host and
port) run without a card. `browser_click` and `browser_type` still ask unless
**Also allow clicks and typing on granted sites** is on
(`grant_covers_actions = true` under `[computer_use]`). Another origin, another
thread or a restarted engine asks again: grants are kept in memory only and end
when the thread closes.

While a thread holds a grant, its header shows a **Browser: staging.example.com**
chip. Clicking it is the stop control. The grant ends, the thread's browser
closes, and a browser card still waiting is answered as revoked. The same grants
are listed by `_rustcode/permissions` under `browser`, and
`_rustcode/permissions/revoke` with `id: "browser:<thread id>"` revokes one.
The grant covers calls through RustCode's MCP hub (Codex, Claude Code and the
other external harnesses). The built-in harness still asks on every call.

Exact domain entries match only that host; `*.example.org` matches subdomains,
not the apex. HTTP and HTTPS only; credentials in URLs are refused. Empty lists
allow nothing. Every connection the browser makes, including page loads, third-party
resources, WebSockets, workers and iframes, goes through a proxy RustCode runs
on a loopback port for that browser (Chrome's `--proxy-server`, with the
implicit localhost bypass removed). It opens a connection only to a host on the
current list, so turning `enabled` off or removing a host takes effect on the
next connection; a connection already open stays open. WebRTC is limited to
proxied connections. The page target's requests are also checked by URL with
Chrome's [Fetch domain](https://chromedevtools.github.io/devtools-protocol/tot/Fetch/),
which refuses other schemes and URLs with credentials. Add resource hosts
explicitly if a page needs them. This is a browser request policy, not an OS
network sandbox: hosts are matched by name, so a listed name that resolves to a
private address is still reached.

## Isolation and limits

Chrome runs headless with a separate random profile for each thread below
`<config>/computer-use/`; it never attaches to an everyday browser. Browser
state stays within that profile. A thread's browser closes after 10 minutes
without a browser call, and when the thread is closed or deleted; the next call
starts a fresh one with a new profile, so sign-ins do not carry over. Profiles
are removed when the browser exits. A profile left behind by an engine that
crashed or was killed is removed the next time any thread starts a browser. External harnesses receive a screenshot as an MCP image. On the
built-in harness the tool result is a short note, and the image is saved with
the thread's attachments and shown to the model after the tool batch, like a
pasted image (a model without image input gets it described by the configured
`vision_model`). Typed values are redacted from the engine's approval preview;
the harness may retain its own tool arguments in its transcript.

`browser_read` returns the URL, the title, the visible text (capped at 32000
characters) and up to 100 interactive elements from Chrome's accessibility
tree, each with a ref such as `e12`, a role, a name and states like `disabled`
or `checked`. Text fields' contents are never listed, so a password is not
returned. The page's text and elements are wrapped as untrusted content, with a
note telling the model not to follow instructions in them. `browser_click` and
`browser_type` take a `ref` from the latest read (preferred) or a CSS
`selector`. A click is a real mouse press and release at the element's center
after scrolling it into view, so the page sees trusted input. A ref that is no
longer on the page fails with a message to read again. Every script RustCode
runs in the page runs in an isolated world, so the page's own scripts can
neither see nor change it. `browser_open` waits up to 15 s for the page's load
event and says when it had not finished.

This slice supports one page per thread and standard macOS/Linux Chrome
locations. Calls time out, and a failed or cancelled call closes that thread's
browser. Popups are blocked. No arbitrary JavaScript tool, desktop control,
uploads, downloads UI or Windows integration is provided.

## Desktop control

Desktop control lets any harness see and use this computer's own apps with
seven tools from the same `rustcode` MCP server: `computer_windows`,
`computer_snapshot`, `computer_screenshot`, `computer_click`, `computer_type`,
`computer_key` and `computer_scroll`. It runs on **macOS 14 or later** for
now; Windows and Linux follow ([ADR 0011](adr/0011-os-computer-use.md)). The
built-in harness does not offer these tools yet.

### Enable

On by default, per machine; turn it off under **Settings → Voice & computer
use** › **Desktop control** on the Mac itself. Nothing runs until macOS grants
**Accessibility** (to read apps and act on them) and **Screen Recording** (for
window screenshots); the section shows both and **Ask macOS** shows the
prompts, as does the **Allow desktop control** step under **Settings › System
setup**. While the app is open it hosts the worker, so macOS checks RustCode's
own grants and lists them as **RustCode**. With the app closed the engine
starts its own worker, which macOS treats as the bare `rc` binary (that
Settings names) with grants of its own. Or set it in `config.toml`:

```toml
[desktop_control]
enabled = false
```

Start a new thread after changing it.

### How an agent works

1. `computer_windows` lists on-screen windows with their `window_id` and `pid`.
2. `computer_snapshot` reads one window's accessibility tree. Each element gets
   a ref such as `e12`. Password fields are marked and their values never
   listed; everything read is wrapped as untrusted screen content.
3. `computer_click`, `computer_type` and `computer_scroll` act on a ref. A
   click presses the element through Accessibility and typing inserts at its
   caret, so the app works in the background: the user's pointer, keyboard
   focus and front window stay theirs. Keys and pixel clicks are posted to the
   app's own process. Chromium and Electron apps (Discord, Slack, Chrome,
   VS Code) ignore input sent that way; for them the agent passes
   `foreground: true` to `computer_click`, `computer_type`, `computer_key` or
   `computer_scroll`, which brings the app to the front and sends the input
   through the system's own input stream (it moves the user's pointer and
   takes focus). Foreground input is approved separately from background
   control of the same app.
4. `computer_screenshot` captures one window (ScreenCaptureKit, longest side
   1600 px). Pixel coordinates for `computer_click` and `computer_scroll`
   (`x`, `y` with `window_id`) are pixels of the latest screenshot of that
   window, for canvases and custom views without a usable tree.

Before each action the **agent's cursor**, a rounded arrow in RustCode's
colour, glides to where it is about to act and stays there afterwards. It is
drawn by RustCode in a click-through window that screen captures leave out.

### Approval

- The first action on an app in a thread asks: **Allow ‹App› for this thread**,
  **Allow once** or **Deny**. Listing windows does not ask.
- Password managers (1Password, Bitwarden, LastPass, Dashlane, Passwords,
  Keychain Access) and System Settings ask on every action.
- `computer_type` refuses password fields: hand those to the user.
- One thread drives an app at a time; another thread gets `app_busy` for two
  minutes after the last action.
- Typed text is redacted on the card. Each action is logged as the
  `computer_use.action` operational event (thread, tool, app, outcome).

### Stop

The agent's cursor glides to each target on a short eased arc (longer moves
take longer, up to about 0.7 s), starting from the user's pointer the first
time, and shrinks briefly on each click. It then floats gently for half a
minute and rests where it last acted, and the app's worker shows it there
again after a restart (`<config>/run/cursor-position`). Its outline is Cua
Driver's default cursor (MIT). Nothing is added to the menu bar.
Turning the switch off, closing the thread, or revoking `desktop:<thread>`
through `_rustcode/permissions/revoke` also ends a thread's grants. The worker
(`rc computer-worker`) runs as a child of the RustCode app, listening on
`<config>/run/computer-worker.sock` for the engine (the same `rc` binary, or
one with the same stable signature, so an engine started before an update
still connects), and starts again if it exits. When a check finds a
permission missing, the worker asks a fresh copy of itself every three seconds
for 15 minutes and starts over in place once the user has granted it, so a
grant in System Settings applies without restarting the app; its log is
`<config>/computer-use/hosted-worker.log`. With the app closed, the engine
starts a worker of its own on the first desktop call, which exits after two
idle minutes; its log is `<config>/computer-use/desktop-worker.log`.

### Across paired machines

Every `computer_*` tool accepts an optional `machine` argument (the machine id
or name from `list_machines`). A thread running on another paired computer—for
example a headless Linux server—can control your Mac's desktop while the
RustCode desktop app is open on the Mac and Desktop control is switched on
there:

- On machines where desktop control is off or unsupported locally, the seven
  `computer_*` tools remain discoverable through `tool_search` (`computer`)
  whenever an app supporting orchestration version 23 or newer is attached.
- The call routes from the remote thread's engine through the RustCode app on
  the controlled computer to that computer's local engine
  (`_rustcode/desktop/call`).
- `_rustcode/desktop/call` is never exposed over remote ACP (`acp_relay.rs`),
  and remote `_rustcode/settings/set` cannot enable `desktop-control`: no
  paired device can turn desktop control on or drive a desktop without the app
  open on that computer.
- The approval dialog opens on the controlled computer, names the remote
  machine and thread, redacts typed text, and is never auto-approved by Full
  access.
- Remote `computer_screenshot` results that exceed 600 KiB as PNG are
  re-encoded as JPEG at the same pixel dimensions so they stay inside the
  remote ACP frame limit without shifting click coordinates.
