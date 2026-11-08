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
  first (its own process group, output in `run/server.log`) and waits up to
  30 seconds for it. When stdin closes it still relays what the server owes,
  then exits once the server closes the connection.
- `rustcode --acp` is unchanged: a standalone engine with exactly one client,
  which ends with it.

## Where it lives

Everything is in `<config dir>/run/` (`~/.config/rustcode/run/`; a
different `RUSTCODE_CONFIG_DIR` gives a separate server):

| File | What |
| --- | --- |
| `server.lock` | Held (advisory lock) for the server's life; one server per user and config directory. The OS releases it however the process ends. |
| `server.json` | `pid`, `version`, `endpoint`, `started_at` of the running server, removed when it stops. A crash can leave it behind; clients only trust a connection. |
| `server.sock` | The Unix socket (macOS, Linux). |
| `server.log` | What a server started by `server connect` prints. |

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
