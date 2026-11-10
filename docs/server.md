# RustCode server

The RustCode server is one engine per user that several clients share: the
app, another app window, a terminal, later phones and other machines. Threads,
their turns and the harness processes run in the server, so a client that
closes stops nothing; the next one to open the thread sees it where it is.

In code the server is still the engine (`rustcode/engine/src/server/`,
`acp::AcpEngine`); "RustCode server" is the name users see.

## `server run` and `server connect`

```sh
rustcode server run       # the server, in the foreground until stopped
rustcode server connect   # ACP on stdin/stdout, relayed to the server
```

- `server run` serves ACP to every client of this user until Ctrl-C,
  `SIGTERM` or `SIGHUP`. If a server already runs for this user it says
  which one (pid, version, socket) and exits with status 0, doing nothing.
- `server connect` relays newline-delimited ACP JSON-RPC between its
  stdin/stdout and the server, the same protocol `rustcode --acp` speaks over
  stdio. If no server runs it starts `rustcode server run` in the background
  first (its own process group, output in `logs/server.log`) and waits up to
  30 seconds for it. When stdin closes it still relays what the server owes,
  then exits once the server closes the connection.
- `rustcode --acp` is unchanged: a standalone engine with exactly one client,
  which ends with it. While it runs it also publishes where another local
  process can open ACP on that same engine (`run/engine.json`), so the remote
  gateway relays a paired desktop to the threads the app shows without a
  server behind it (`docs/remote.md` › Desktop ACP channel).

## Where it lives

The server's lock, description and socket are in `<config dir>/run/`
(`~/.config/rustcode/run/`; a different `RUSTCODE_CONFIG_DIR` gives a separate
server); its log is `<config dir>/logs/server.log` (see
[Logs](#logs)):

| File | What |
| --- | --- |
| `server.lock` | Held (advisory lock) for the server's life; one server per user and config directory. The OS releases it however the process ends. |
| `server.json` | `pid`, `version`, `endpoint`, `started_at` of the running server, removed when it stops. A crash can leave it behind; clients only trust a connection. |
| `server.sock` | The Unix socket (macOS, Linux). |
| `engine.json` | Same fields for an engine started by a client (`rustcode --acp`, the app's engine) that no server runs behind, with the `engine-<pid>.sock` it listens on. Removed when it stops; a crash can leave it behind, so the reader connects to decide. |

On Windows the endpoint is a named pipe, `\\.\pipe\rustcode-server-<random>`,
published only in `server.json`.

## Who can connect

Only the user the server runs as.

- macOS and Linux: `run/` is `0700` and must belong to the user, the socket
  is `0600`, and every connection's peer uid is checked against the server's.
- Windows: the pipe's DACL grants access to its owner and SYSTEM only, it
  refuses remote clients, and its random name is only in the user's profile.

## Clients and turns

- Each connection keeps its own capabilities from `initialize`
  (`acp/clients.rs`). A session's updates go to every connection that created
  or loaded it; thread status and list notifications go to all.
- A prompt shows on the session's other clients as a user message.
- A turn belongs to the server, not to the connection that sent the prompt:
  closing that connection mid-turn loses only the prompt's answer. Harness
  processes end with the server process, never with a client.
- `session/load` of a thread already open in the server, even mid-turn,
  sends the transcript from before the running turn, then what the turn sent
  so far (its prompt first), then the rest live, with nothing missed or sent
  twice in between.
- `session/close` closes the thread for every client, as before.

## Running in the background

`rustcode server run` in a terminal ends with the terminal. To have the server
start when you log in, and come back after a crash, install it as a service:

```sh
rustcode server install     # write the service, start it, wait for the socket
rustcode server status      # installed? running? clients, turns, problems
rustcode server restart     # now; --when-idle waits until no turn is running
rustcode server logs -f     # the server's log
rustcode server uninstall   # remove the service
```

| OS | Mechanism | Runs when |
| --- | --- | --- |
| macOS | LaunchAgent `~/Library/LaunchAgents/no.rustcode.server.plist`, `RunAtLoad` and `KeepAlive` (restart after a crash, not after a clean stop) | you are logged in |
| Linux | `systemd --user` unit `~/.config/systemd/user/rustcode-server.service`, `Restart=on-failure` | you are logged in; with lingering, from boot |
| Windows | Scheduled Task `RustCode\Server`, "At log on" of your user, limited, hidden, restart on failure; if tasks are not allowed, a `Run` key entry (no restart on failure) | you are logged in |

- The service runs `rustcode server run --service` from the `rustcode` that
  ran `install`: the app's bundled one, Homebrew's (the symlink, so formula
  upgrades are followed) or `cargo install`'s. `status` says which.
- **Development builds never install.** A binary inside a cargo `target/`
  directory (or `CARGO_TARGET_DIR`) is refused, because the next `cargo build`
  or `cargo clean` would replace or delete what the service runs. `--force-dev`
  overrides it.
- A different `RUSTCODE_CONFIG_DIR` gets a service of its own
  (`no.rustcode.server-<hash>`, `rustcode-server-<hash>.service`) that sets the
  variable, so a scratch directory never replaces your real server's service.
  Not on Windows, where a task cannot set it.
- If another server already runs (an app's, or `rustcode server run` in a
  terminal), it holds the server lock and the service's own would exit at
  once, so `install` stops it first and then starts the service. It does not
  end running turns without asking: it stops with a message, and `--yes`
  stops the server anyway. Nothing is stopped when the install would be
  refused (a development build, or a custom `RUSTCODE_CONFIG_DIR` on Windows).
- `install` is idempotent. If the service is already installed and the
  definition is unchanged it does nothing (but starts it if it is not
  running). If the definition changed while the server runs (a new binary
  path) the file is rewritten, the running server is left alone and `install`
  says so: `rustcode server restart --when-idle` applies it.
- `uninstall` stops the server the service runs and removes the service.
  Threads, settings, pairings and logs stay. It asks first when turns are
  running (`--yes` skips the question).
- **The app installs it** when Settings › Remote access › *Network access* is
  turned on and the server is not installed, after one confirmation that says
  what it is for: it stays up when the app closes and starts when you log
  in, so the threads that live in it keep running. Threads the app starts itself are still its own
  engine's and end with it, until the app is switched to the server (a later
  step: the app runs its own `rustcode --acp`, not a client of this one). The
  app runs the same `rustcode server install` with the `rustcode` the app
  itself runs, so a development build is refused there with the reason shown,
  and a server another process runs is never stopped while a turn is running —
  `--yes` stays a decision for a terminal. Settings › Machines shows the
  result on *This computer* as *Server: Installed · Running*, with **Install
  server** while it is not installed; Settings › Remote access shows the same
  and its version.
- **From an AppImage**, the bundled `rustcode` lives in a mount that goes away
  with the app, so the app copies it to `$XDG_DATA_HOME/rustcode/bin/rustcode`
  (`~/.local/share/rustcode/bin/rustcode`) and installs the service from that
  copy, refreshing it when the AppImage is updated. A copy that cannot be made
  is refused with the reason; a `.deb` install needs no copy and is installed
  where it is.

### Linux: keep it running after logout

A user service stops when the user's last session ends unless lingering is
on. Enabling it needs root, so `install` and `status` print the command and
never run it:

```sh
sudo loginctl enable-linger "$(id -un)"
```

Until then `status` reports `linger-disabled`. Over SSH or in a container with
no user session, `systemctl --user` has nothing to talk to; log in on the
machine, or enable lingering and sign in again.

### `status`

`rustcode server status` asks the running server over its socket
(`_rustcode/server/status`, which needs no `initialize` and is not counted as a
client) and reads the service manager. `--json` prints:

```json
{
  "installed": true,
  "running": true,
  "version": "1.23.0",
  "cli_version": "1.23.0",
  "pid": 4242,
  "uptime_secs": 3600,
  "socket": "/Users/ada/.config/rustcode/run/server.sock",
  "binary": "/Applications/RustCode.app/Contents/MacOS/rustcode",
  "service": { "kind": "launchd", "path": "/Users/ada/Library/LaunchAgents/no.rustcode.server.plist" },
  "clients": 2,
  "running_turns": 1,
  "log": "/Users/ada/.config/rustcode/logs/server.log",
  "problems": []
}
```

`version`, `pid`, `uptime_secs`, `socket`, `clients` and `running_turns` are
`null` when no server answers. Each problem is `{ "code", "message" }`:

| Code | Meaning |
| --- | --- |
| `not-installed` | No service; threads stop with the app. |
| `stopped` | Installed, but nothing answers on the socket. |
| `linger-disabled` | Linux: the server stops at logout (see above). |
| `restart-pending` | A `restart --when-idle` is waiting for running turns to finish. |
| `version-mismatch` | The running server's version is not this `rustcode`'s. |
| `unmanaged` | The server that answers was not started by the service (an app started it); `restart` hands it over. |

### `restart`

`restart` stops the server and starts it from the installed definition, then
waits for it to answer. A server the service did not start is asked to stop
first, so the service's new one does not find it and exit. It ends running
turns, so with turns running it asks, or refuses without a terminal unless
`--yes` is given.

`restart --when-idle` waits instead. It polls the running turn count every two
seconds and restarts once it has read zero twice in a row (a queued prompt can
start right after a turn ends). While it waits `status` reports
`restart-pending`. It never gives up on its own; Ctrl-C leaves the server as it
is. On Windows stopping a server is a hard kill (no signal reaches a process
without a window), so harness processes it started can outlive it until they
end themselves.

### Logs

The server writes its log to `<config dir>/logs/server.log` (everything the
engine logs, also in `debug.log`), keeping the file and the previous one
(`server.log.1`) under 20 MiB each. launchd and systemd also send the process's
standard output and error there, which catches a crash's trace but cannot be
rotated by the server. A Windows task's output cannot be redirected, so there
only the server's own log exists.

`rustcode server logs` prints the last 100 lines (`--lines N`); `-f` follows
the file, across a rotation.
