# 0002: What `rc` does before it runs a command

Status: accepted, 2026-11-21. Code: `rustcode/cli/src/run.rs` (`run`).

## stdio modes come first

Three commands return before any startup work, because something else reads
their standard output:

- `mcp-orchestrator`: started by an external harness on the engine's behalf.
  It speaks MCP on stdio, so it runs no migrations and prints nothing else.
- `mcp-connect`: started by another app (Claude Desktop, Cursor, Codex) from
  the snippet Settings › Outside agents gives. MCP on stdio, nothing else on
  stdout.
- `guard-command`: Claude Code's `PreToolUse` hook. The hook call comes on
  stdin and the refusal, if any, goes to stdout. It never fails the call
  itself.

## Then, in order

1. User-data migrations a newer release introduced run once, before anything
   reads the config directory (#2599). A failure is reported and retried on
   the next start; it never stops startup.
2. A server service an older release installed runs `rustcode`, which has
   been the standalone harness since 1.45.0, and fails with "unrecognized
   subcommand 'server'". It is pointed at this `rc`.
3. A debug log left by a previous process is rotated before new diagnostics
   are written. The logger also checks the cap before each append, so a
   long-lived process stays bounded.
4. A panic hook writes the message and location to `debug.log`: silent
   mid-stream hangs once left no evidence (#1226).
5. Provider keys exported in `~/.zshrc` are invisible to GUI, systemd and IDE
   launches. Missing keys are read from the login shell once, so profiles,
   MCP servers and tool shells see what the user's terminal sees.

`rc server run` and `rc --acp` hydrate the whole login-shell environment, not
just keys: launchd, systemd and `server connect` start them with a bare
environment, and MCP servers and tool shells need everything the shell
exports.

## Relays exit at once

`rc server connect` only relays: the server it reaches or starts does the
work. Standard input is read on a blocking thread that would keep the runtime
alive, so the process exits as soon as the relay ends.

## Remote session owners run in the harness binary

Detached mobile sessions on the built-in harness run in the standalone
`rustcode` harness binary (`remote session-owner`), so `rc` carries no TUI or
interactive runtime dependencies.
