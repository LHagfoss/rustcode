# Desktop automation (experimental)

The macOS app can let the agent inspect and operate native applications through
[agent-desktop](https://github.com/vercel-labs/agent-desktop). This is separate
from the localhost preview. It is off by default.

## Setup

1. Install Node.js 18 or later and run `npm install -g agent-desktop`. The CLI
   must be on the PATH RustCode inherits, or in a standard RustCode tool path.
   Verify with `agent-desktop version`.
2. In RustCode, open Settings → General → Desktop automation and enable it.
   Restart the engine when offered.
3. Grant Accessibility access in macOS System Settings to the application
   launching agent-desktop. See the [macOS permission guide](https://agent-browser.dev/desktop/platforms/macos).
   Permission applies to an executable, so a grant to Terminal does not
   necessarily grant the RustCode app access.

Ask the agent to work with a named app. RustCode asks before every snapshot,
click, and text entry, even when general auto-approval is enabled. A denial
runs nothing. Turning the setting off blocks new operations immediately; the
engine must restart to remove the tool from its advertised tools.

## Scope and privacy

The tool supports a snapshot of one app window and semantic clicks and text
entry on refs returned by the latest snapshot for that chat and app. An action
can fail if the app does not expose an accessible element or if the ref is
stale. A successful command does not necessarily prove the UI changed; the
agent should observe again. No arbitrary shell command, screenshot, clipboard,
physical input, or browser automation is exposed by this tool.

Accessibility snapshots can contain private content. They are sent to the
model as tool results and the upstream CLI stores snapshot refmaps in its state
root. Do not approve reading or typing sensitive information you do not want in
the conversation. Stopping the turn invalidates outstanding refs and pending
approval; an operation already sent to another app may still have taken effect.
