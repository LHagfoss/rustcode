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
reports when Chrome/Chromium needs installing. Every action asks for approval.
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
screenshots. Auto-approve does not skip these cards on any harness,
and a headless `rc` run, which has nobody to ask, refuses the tools. Plan mode
refuses `browser_open`, `browser_click` and `browser_type`; reads and
screenshots still ask. Turn `enabled` off to refuse subsequent calls. No browser is
installed or launched until the first approved action.

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