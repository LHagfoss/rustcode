# Discord Rich Presence

RustCode can publish the active session title and a small set of meaningful
states—idle, queued, thinking, running tools, and action required—to the
Discord desktop application through Discord's local Rich Presence IPC.

Rich Presence is enabled by default when possible. If Discord is closed or
unavailable, RustCode silently retries in the background and the TUI remains
usable. It does not open a Discord login flow and never reads or stores Discord
passwords, account tokens, cookies, or browser profiles.

When a generated session title is unavailable, the activity uses only the
basename of the workspace, repository, or current folder (for example,
"rustcode"). It never publishes an absolute local path. During a request,
provider usage may be shown as compact "out" and "total" token checkpoints.
These values are rounded and deduplicated so streaming deltas do not create a
Discord update for every token.

## Setup and control

With the Discord desktop application installed and running:

```bash
rustcode discord --setup
rustcode discord --status
rustcode discord --disable
rustcode discord --enable
```

`--setup` enables the feature and prints the expectations. `--status` is a
read-only check of RustCode's setting and the local IPC socket; it does not
connect to or modify a Discord account. The setting is stored as
`discord_rpc_enabled = true|false` in RustCode's normal `config.toml`.

## Discord application assets

The built-in RustCode application/client ID is used for the IPC handshake.
The corresponding Discord Developer Portal application should contain a large
image asset named `rustcode_logo`. Without that asset, the activity still
works but Discord may omit the large image. Discord must be the desktop client
(not only the web app), and the user must already be logged in there.

macOS is the first supported target. The implementation uses the
cross-platform IPC crate and compiles on Windows and Unix targets; status
socket discovery is filesystem-based on Unix and is intentionally unavailable
on Windows, where Discord uses named pipes. No live Discord client is needed
for RustCode's tests.
