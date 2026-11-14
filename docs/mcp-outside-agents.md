# Outside agents

Another agent platform (Claude Desktop, Cursor, Codex, a script) can use
RustCode's orchestration tools as an independent MCP client, with a token and
an access level of its own. This is separate from the tools RustCode injects
into its own harness threads (`docs/orchestrator.md`): an outside agent is not
a thread, has no thread's token, and its token opens nothing else.

Connections on this computer use stdio (`rustcode mcp-connect`) or localhost
HTTP with OAuth. The engine checks every call. Hosted clients need the
reachability work in #3199; a localhost URL is not reachable from their cloud.

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

- Remote hosted clients (#3199).
- Reaching another paired computer (`machine`) from an outside agent, `unread`
  and `snoozed_until` in `update_session`, and changing a thread's model or
  effort with `configure_session`: they need the app.

`search_sessions` matches titles and what was said; across projects it follows
the "Let agents read threads in other projects" setting, as `list_sessions`
does. `list_projects` lists the projects that have a thread on this computer;
a folder registered in the app but with no thread yet shows only in the app's
own answer.

## Localhost HTTP and OAuth

The engine listens only on `127.0.0.1`, at `/mcp`, using stateless
Streamable HTTP (JSON responses; no SSE subscription). It prefers port
`43871`, chosen away from common development server defaults. If occupied,
it tries successive ports and saves the selected port in
`<config>/outside-http-port`; restarts reuse that port unless it is occupied.
Other bind errors stop the endpoint rather than exposing it on another interface.

Open Settings › Outside agents › Connection and press **Copy URL** to paste
the HTTP connection URL into your client. The stdio command is alongside it.
If startup fails, Settings says the HTTP endpoint is not running.
The `_rustcode/outside/list` response supplies `httpUrl`. Keep RustCode
running while connecting. See the client walkthrough below.

Discovery is at `/.well-known/oauth-protected-resource/mcp` (also the root
protected-resource path) and `/.well-known/oauth-authorization-server`.
An unauthenticated MCP request returns a Bearer challenge with the metadata URL.
Use a public `client_id` identifying your app, an exact loopback HTTP callback
on `127.0.0.1`, `response_type=code`, `resource` equal to the MCP URL, one scope
(`read_only`, `delegate`, or `manage`), `state`, and an S256 PKCE challenge.
Only S256 is supported. Client registration and remote callbacks are not
supported in this localhost slice.

Open `/authorize` with those parameters. In RustCode's Outside agents approval
list, the user sees the requesting app, callback destination and requested
access and approves or declines. The callback receives a single-use code
(valid for 60 seconds). POST form fields to `/token`: `grant_type`, `code`,
`client_id`, the identical `redirect_uri` and `resource`, and `code_verifier`.
The access token expires after one hour; send it only as an Authorization Bearer
header. It cannot authenticate the stdio bridge. Every request checks expiry
and the shared revocable grant store; every tool call uses the existing scope,
per-change approval and audit gate.

After expiry, revocation, a declined/timed-out approval, or an engine restart,
start authorization again with fresh PKCE and state. There are no refresh tokens.
Revoke connections in Settings › Outside agents. Codes and tokens are never
logged. Remote hosted clients remain out of scope until #3199.


## Client walkthrough and compatibility

Documentation checked 2026-11-14. These are documented client configuration
formats, **not yet verified with a live client**. The in-process wire tests
exercise RustCode's HTTP protocol, not the clients' OAuth implementations.
Use the copied connection URL if its port differs from these examples.

| Client | stdio configuration | HTTP configuration and current limit | Live verification |
| --- | --- | --- | --- |
| Codex CLI | `~/.codex/config.toml`, `command`, `args`, and `env` below | `url`, explicit OAuth client ID and one scope below; then `codex mcp login rustcode` | not yet verified with a live client |
| Claude Code | `claude mcp add-json rustcode` with the JSON below | `claude mcp add --transport http --client-id rustcode-claude rustcode http://127.0.0.1:43871/mcp`, then `/mcp`; current documented `localhost` callback is refused by RustCode, which requires `127.0.0.1`. Scope selection must also request exactly one supported scope | not yet verified with a live client |
| Claude Desktop | Settings › Developer › Edit Config; merge the `mcpServers` entry below and restart | Settings › Connectors › Add custom connector uses a remote URL. Anthropic connects from its cloud, so this localhost endpoint is unavailable | not yet verified with a live client |
| ChatGPT web (hosted custom MCP) | No stdio configuration in the documented custom MCP flow | Enable developer mode, then create a custom MCP app/plugin with a remote server URL and OAuth authentication. This localhost endpoint is unavailable; remote reachability is tracked in #3199 | not yet verified with a live client |

ChatGPT desktop also documents local MCP servers under Settings › MCP servers:
Add server, choose STDIO or Streamable HTTP, enter the command or URL, save
and restart. For stdio use `rustcode mcp-connect` and supply the generated
`RUSTCODE_MCP_TOKEN` in the server environment. For HTTP use the copied URL
and Authenticate. Its OAuth client ID/scope configuration and callback need
live verification; this is separate from the hosted custom MCP flow above.

### stdio on this computer

In RustCode, open Settings › Outside agents › Connect an app, choose Read-only
and copy the generated token/config. Use an absolute path to the `rustcode`
binary if the client does not inherit your shell PATH. Replace `TOKEN` below
with that connection's token; do not reuse an HTTP access token.

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.rustcode]
command = "rustcode"
args = ["mcp-connect"]
tool_timeout_sec = 180

[mcp_servers.rustcode.env]
RUSTCODE_MCP_TOKEN = "TOKEN"
```

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "rustcode": {
      "command": "rustcode",
      "args": ["mcp-connect"],
      "env": { "RUSTCODE_MCP_TOKEN": "TOKEN" }
    }
  }
}
```

Claude Code accepts the inner server object:

```sh
claude mcp add-json rustcode '{"command":"rustcode","args":["mcp-connect"],"env":{"RUSTCODE_MCP_TOKEN":"TOKEN"}}'
```

In the client, inspect the available MCP tools and ask it to call `whoami`.
The answer identifies the connection and Read-only access. Revoke the connection
in RustCode and repeat: the next call must be refused.

### HTTP with Codex on this computer

Use this entry instead of the stdio entry (do not define the same name twice):

```toml
[mcp_servers.rustcode]
url = "http://127.0.0.1:43871/mcp"
scopes = ["read_only"]
tool_timeout_sec = 180

[mcp_servers.rustcode.oauth]
client_id = "rustcode-codex"
```

Run `codex mcp login rustcode`. Explicit client ID avoids dynamic registration,
which RustCode does not implement; a single scope avoids requesting all three
access levels. In RustCode's approval card, check the client, callback and
Read-only access, then press Allow. The browser returns to the client's
loopback callback. Ask Codex to call `whoami`; it should report Read-only.
This sequence still needs live verification, including the client's callback
host, resource parameter and OAuth exchange behavior. Do not treat a saved
configuration as a successful connection.

After expiration or restart, log in again; after revocation, authorize a new
connection. The saved port normally keeps the URL stable. If another process
occupies it, copy the updated URL from RustCode and update the client.
Do not publish or tunnel this endpoint as a workaround for hosted clients:
it validates the loopback Host, origin and callback.

### Documentation sources

- [Codex MCP configuration and OAuth](https://developers.openai.com/codex/mcp)
- [Claude Code transports, JSON configuration and OAuth credentials](https://code.claude.com/docs/en/mcp)
- [Claude Desktop local MCP setup](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop)
- [Claude custom connectors and cloud connectivity](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp)
- [ChatGPT custom MCP setup](https://developers.openai.com/api/docs/guides/custom-mcp-server)
