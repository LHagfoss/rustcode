# Outside agents

Another agent platform (Claude Desktop, Cursor, Codex, a script) can use
RustCode's orchestration tools as an independent MCP client, with a token and
an access level of its own. This is separate from the tools RustCode injects
into its own harness threads (`docs/orchestrator.md`): an outside agent is not
a thread, has no thread's token, and its token opens nothing else.

This first slice is for the same computer. The app starts `rustcode
mcp-connect` over stdio and the engine checks every call. An HTTP endpoint
with OAuth for hosted and remote clients (#3368) needs the reachability work in
#3199 and is not part of it.

## Connect an app

Settings › Outside agents › Connect an app:

1. Pick the app (Claude Desktop, Cursor, Codex or another), give it a name and
   choose its access.
2. RustCode shows the config to paste, once: JSON for Claude Desktop and
   Cursor, TOML for Codex. It contains the token.
3. Paste it where the app says (Claude Desktop's `claude_desktop_config.json`,
   `~/.cursor/mcp.json`, `~/.codex/config.toml`) and restart that app.

The snippet runs `rustcode mcp-connect` with the token in
`RUSTCODE_MCP_TOKEN` (not in the arguments, so it is not in `ps`). RustCode
keeps only the token's SHA-256, in `<config>/outside-agents.json` (mode 0600).

## Access levels

| Level | May |
| --- | --- |
| Read-only (default) | `whoami`, `list_sessions`, `search_sessions`, `list_projects`, `read_session`, `wait_session`, `thread_status`, `thread_usage`, `query_data`, `read_activity`, `list_harnesses`, `list_machines` |
| Delegate | Read-only, plus `start_session`, `fan_out`, `message_session` and `interrupt_session` |
| Manage | Delegate, plus `merge_when_green`, `cleanup_worktree`, `update_session` and `configure_session` |

The engine checks the level on every call, so lowering an app's access or
revoking it applies to its next call. A tool that is not classified in
`orchestration/outside.rs` (`required_scope`) is refused to every level, so a
new internal tool is never exposed by accident. A tool that is allowed by level
but that the engine cannot run without a thread behind it (`manage_queue`,
`handoff_worktree`, `transfer_session`, projects and scheduled tasks) is not
listed, and a call says so. `whoami` lists the projects the app can start
threads in.

An outside agent has no thread, so the tools that change another thread run in
the engine (`orchestration/outside_manage.rs`) on the operations the app uses
for its own threads, and each one takes the thread's `session_id` (there is no
"own thread" to default to):

| Tool | What it does for an outside agent |
| --- | --- |
| `fan_out` | The same task on two to four harnesses, as standalone threads of yours. Nothing is linked or delivered back; use `wait_session` or `read_session` |
| `interrupt_session` | Stops the turn running on a thread, as its stop button does. `{ session_id, interrupted, status }`; `interrupted` is false when no turn was running |
| `merge_when_green` | Waits for the checks of the pull request on the thread's worktree branch and squash-merges it, as `scripts/pr-merge.sh` does. Takes `session_id`, not a bare PR `number` |
| `cleanup_worktree` | Removes a merged thread's worktree and branch with the same checks the app makes (clean, merged, nothing unique), and settles the thread unless `settle` is false |
| `update_session` | `title`, `pinned` and `status` (`active`, `review`, `settled`). `unread` and `snoozed_until` need the app and are refused |
| `configure_session` | Reports the thread's harness, access, model and effort. Changing `model` or `effort` needs the app and is refused |

Work an outside agent starts is the ceiling of its level: threads are
standalone threads of yours (not delegates of a thread), and run at Auto-edit
under Delegate (files can change; a command that needs approval is declined)
and at Full only under Manage. Reads follow Settings › Harnesses › "Let agents
read threads in other projects": with it off, outside agents cannot read across
projects, `thread_usage` or `query_data`.

## Approval

Above Read-only, each call that changes something (`start_session`,
`fan_out`, `message_session`, `interrupt_session` and the Manage tools) waits for you by default. A card appears anywhere in the app
and in Settings › Outside agents › Waiting for you, with the app, the tool and a
short description (a project, a thread, the first words of a prompt). Allow
runs it; Decline, no answer in two minutes, revoking the app, or lowering its
access while you decide, refuse it. Turn "Ask me first" off per app to let it
act without asking.

## What the agent is told

The server instructions say what RustCode is, the app's name and level, and
what asking involves. `tools/list` shows only the tools the level allows, with
the level each needs in its description. `whoami` returns who is calling, the
tools it can and cannot call (and what each needs), the projects, the machines
and the harnesses.

## Audit

Every outside call is recorded in the data store (`outside_agent_calls`): time,
app, level, tool, outcome (`ok`, `error`, `denied`, `declined`,
`unavailable`), duration and a short reason. Arguments and answers are never
stored. Rows are kept 90 days. The newest show in Settings › Outside agents ›
Recent activity, and `rustcode data query "select * from outside_agent_calls"`
reads them.

## Failure recovery

| You see | Do |
| --- | --- |
| "RustCode is not running" | Open the RustCode app (or start `rustcode server run`). The app publishes its socket in `<config>/run/orchestrator.json`. |
| "did not accept this token" | The app was revoked or the token was mistyped. Connect it again for a new one. |
| "needs Delegate access" | Raise its access in Settings › Outside agents. |
| "the user did not answer" | Open RustCode, then ask the agent to try again. |

## Not in this release

- HTTP transport, OAuth discovery, PKCE and a connection URL (#3368), and
  remote hosted clients (#3199).
- Reaching another paired computer (`machine`) from an outside agent, `unread`
  and `snoozed_until` in `update_session`, and changing a thread's model or
  effort with `configure_session`: they need the app.

`search_sessions` matches titles and what was said; across projects it follows
the "Let agents read threads in other projects" setting, as `list_sessions`
does. `list_projects` lists the projects that have a thread on this computer;
a folder registered in the app but with no thread yet shows only in the app's
own answer.
