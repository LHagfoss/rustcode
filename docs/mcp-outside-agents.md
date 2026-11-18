# Outside agents

Another agent platform (Claude Desktop, Cursor, Codex, a script) can use
RustCode's orchestration tools as an independent MCP client, with a token and
an access level of its own. This is separate from the tools RustCode injects
into its own harness threads (`docs/orchestrator.md`): an outside agent is not
a thread, has no thread's token, and its token opens nothing else.

Connections on this computer use stdio (`rustcode mcp-connect`) or localhost
HTTP with OAuth. Hosted agents (ChatGPT, Claude.ai, other cloud MCP clients)
reach the same endpoint through a public HTTPS address that a tunnel serves;
see [Hosted agents](#hosted-agents-chatgpt-claude-and-other-cloud-clients).
The engine checks every call either way.

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
| Read-only (default) | `whoami`, `list_threads`, `search_threads`, `list_projects`, `read_thread`, `wait_thread`, `thread_status`, `thread_usage`, `query_data`, `read_activity`, `list_harnesses`, `list_machines`, and the lists and reads of `manage_settings` and `manage_capabilities` |
| Delegate | Read-only, plus `start_thread`, `fan_out`, `message_thread` and `interrupt_thread` |
| Manage | Delegate, plus `merge_when_green`, `cleanup_worktree`, `update_thread` and `configure_thread` |
| Admin | Manage, plus the changes of `manage_settings` (`set`, `reset`) and `manage_capabilities` (skills, MCP servers, the default harness and the theme) |

A tool's level can depend on what the call does: `manage_settings` and
`manage_capabilities` read at Read-only and write only at Admin (`required_scope_for`), and a refusal names
the level the call needs. Harness accounts, connections and outside-agent
tokens are never offered to an outside agent at any level.

The engine checks the level on every call, so lowering an app's access or
revoking it applies to its next call. A tool that is not classified in
`orchestration/outside.rs` (`required_scope`) is refused to every level, so a
new internal tool is never exposed by accident. A tool that is allowed by level
but that the engine cannot run without a thread behind it (`manage_queue`,
`handoff_worktree`, `transfer_thread`, projects and scheduled tasks) is not
listed, and a call says so. `whoami` lists the projects the app can start
threads in.

An outside agent has no thread, so the tools that change another thread run in
the engine (`orchestration/outside_manage.rs`) on the operations the app uses
for its own threads, and each one takes the thread's `thread_id` (there is no
"own thread" to default to):

| Tool | What it does for an outside agent |
| --- | --- |
| `fan_out` | The same task on two to four harnesses, as standalone threads of yours. Nothing is linked or delivered back; use `wait_thread` or `read_thread` |
| `interrupt_thread` | Stops the turn running on a thread, as its stop button does. `{ thread_id, interrupted, status }`; `interrupted` is false when no turn was running |
| `merge_when_green` | Waits for the checks of the pull request on the thread's worktree branch and squash-merges it, as `scripts/pr-merge.sh` does. Takes `thread_id`, not a bare PR `number` |
| `cleanup_worktree` | Removes a merged thread's worktree and branch with the same checks the app makes (clean, merged, nothing unique), and settles the thread unless `settle` is false |
| `update_thread` | `title`, `pinned` and `status` (`active`, `review`, `settled`). `unread` and `snoozed_until` need the app and are refused |
| `configure_thread` | Reports the thread's harness, access, model and effort. Changing `model` or `effort` needs the app and is refused |

Work an outside agent starts is the ceiling of its level: threads are
standalone threads of yours (not delegates of a thread), and run at Auto-edit
under Delegate (files can change; a command that needs approval is declined)
and at Full only under Manage. Reads follow Settings › Orchestration › "Let agents
read threads in other projects": with it off, outside agents cannot read across
projects, `thread_usage` or `query_data`.

## Approval

Above Read-only, each call that changes something (`start_thread`,
`fan_out`, `message_thread`, `interrupt_thread` and the Manage tools) waits for you by default. A card appears anywhere in the app
and in Settings › Outside agents › Waiting for you, with the app, the tool and a
short description (a project, a thread, the first words of a prompt). Allow
runs it; Decline, no answer in two minutes, revoking the app, or lowering its
access while you decide, refuse it. Turn "Ask me first" off per app to let it
act without asking.

When no RustCode app is connected to the machine to show the card (a headless
server, or the app closed), such a call is refused at once instead of waiting
two minutes for nobody, and the agent is told to open the app or run
`rustcode outside update <id> --ask false` there. A paired app that has the
machine online counts as connected.

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

- Reaching another paired computer (`machine`) from an outside agent, `unread`
  and `snoozed_until` in `update_thread`, and changing a thread's model or
  effort with `configure_thread`: they need the app.

`search_threads` matches titles and what was said; across projects it follows
the "Let agents read threads in other projects" setting, as `list_threads`
does. `list_projects` lists the projects that have a thread on this computer;
a folder registered in the app but with no thread yet shows only in the app's
own answer.

## Localhost HTTP and OAuth

The engine listens only on `127.0.0.1`, at `/mcp`, using stateless
Streamable HTTP (JSON responses; no SSE subscription). It prefers port
`43871`, chosen away from common development server defaults. On the first
start, if that is occupied, it tries successive ports and saves the selected
port in `<config>/outside-http-port`. A tunnel points at the saved port, so
it is never replaced: a restart waits up to 15 seconds for the engine before
it to release it, and if it stays occupied that run serves on the next free
port (logged) and the next start tries the saved one again.
Other bind errors stop the endpoint rather than exposing it on another interface.

Open Settings › Outside agents › Connection and press **Copy URL** to paste
the HTTP connection URL into your client. The stdio command is alongside it.
If startup fails, Settings says the HTTP endpoint is not running.
The `_rustcode/outside/list` response supplies `httpUrl`. Keep RustCode
running while connecting. See the client walkthrough below.

Discovery is at `/.well-known/oauth-protected-resource/mcp` (also the root
protected-resource path) and `/.well-known/oauth-authorization-server`.
An unauthenticated MCP request returns a Bearer challenge with the metadata URL.
A client either registers at `/register` (RFC 7591; see below) or uses a
public `client_id` naming itself, with a loopback HTTP callback (`127.0.0.1`,
`localhost` or `[::1]`). It sends `response_type=code`, `resource` equal to the
MCP URL (or none), `state`, an S256 PKCE challenge and `scope`: one or more of
`read_only`, `delegate`, `manage`, `admin`, of which the highest counts; other
words are ignored, and none means Read-only. Only S256 is supported.

Registration stores nothing: the `client_id` it returns carries the app's name
and callbacks, signed with a key kept in `<config>/outside-oauth-key`
(`0600`), so an unauthenticated caller cannot grow the server's state and
nobody can add a callback to an id. Deleting the key makes every app register
again. A registered client may use only the callbacks it registered (a
loopback one on any port, RFC 8252 §7.3; any other exactly). An `https`
callback, as hosted agents use, is accepted only through the public address.

Open `/authorize` with those parameters. In RustCode's Outside agents approval
list, the user sees the requesting app, callback destination and requested
access and approves or declines. The callback receives a single-use code
(valid for 60 seconds) and `iss` (RFC 9207). POST form fields to `/token`:
`grant_type`, `code`, `client_id`, the identical `redirect_uri`, `resource` (or
none) and `code_verifier`. The answer has an access token that expires after
one hour and a refresh token; send the access token only as an Authorization
Bearer header. It cannot authenticate the stdio bridge. Every request checks
expiry and the shared revocable grant store; every tool call uses the existing
scope, per-change approval and audit gate.

`grant_type=refresh_token` with `refresh_token` and the same `client_id`
returns a new access token and a new refresh token for the same connection,
with the same access. The used refresh token never works again, and a refresh
token works only for the client it was issued to. A sign-in lasts 30 days from
approval; after that, or after revocation or a declined or timed-out approval,
start authorization again with fresh PKCE and state. Connections whose sign-in
has ended are dropped from the store when the next app connects.
Revoke connections in Settings › Outside agents. Codes and tokens are never
logged. Remote hosted clients remain out of scope until #3199.


## Hosted agents (ChatGPT, Claude and other cloud clients)

A hosted agent runs in its provider's cloud, so it needs an HTTPS address on
the internet. RustCode does not listen on the internet itself: a tunnel such
as Cloudflare Tunnel (`cloudflared`) forwards one hostname to the loopback
endpoint, and RustCode answers requests for that hostname as well as its
loopback one. Run it on a machine that stays on, such as a VPS running
`rustcode server run` (`docs/server.md`).

1. Tell RustCode its public address, in the user `config.toml` (a project
   config cannot set it):

   ```toml
   [outside]
   public_url = "https://rustcode.example.com"
   ```

   Restart the server. Only a bare `https://` origin is accepted.
2. Point the tunnel at the endpoint's port (`43871` unless
   `<config>/outside-http-port` says otherwise). With `cloudflared`:

   ```sh
   cloudflared tunnel login
   cloudflared tunnel create rustcode
   cloudflared tunnel route dns rustcode rustcode.example.com
   ```

   and in `~/.cloudflared/config.yml`:

   ```yaml
   tunnel: rustcode
   credentials-file: /home/you/.cloudflared/<tunnel-id>.json
   ingress:
     - hostname: rustcode.example.com
       service: http://127.0.0.1:43871
     - service: http_status:404
   ```

   Run it as a service (`cloudflared service install`). The tunnel must keep
   the request's `Host`; a proxy that rewrites it to `127.0.0.1` is refused
   (set `httpHostHeader` back to the public host if it does).
3. In Settings › Outside agents, pick the machine (the page shows every
   connected machine, and starts on one with a public address). Under
   **Connect ChatGPT, Claude and other hosted agents**:
   - **Claude**: *Open in Claude* opens Claude's Add custom connector form
     with RustCode filled in.
   - **ChatGPT**: *Copy URL and open ChatGPT* copies
     `https://rustcode.example.com/mcp`; in ChatGPT's connector settings
     (Developer mode), create a connector with that URL and OAuth.
   - **Another agent**: add a remote MCP server (Streamable HTTP) with the URL
     and OAuth.

   No client ID or secret is needed: the app registers itself.
4. The app opens RustCode's sign-in page. It names the app (the name it gave
   itself), the machine, and where access goes (for example `chatgpt.com`:
   whoever runs that service gets the access). Pick what it may do (each
   level lists what it can and cannot), then either:
   - **Approve in RustCode.** The page waits and shows two words. A card
     with the same words appears in every RustCode app connected to that
     machine, in the window's corner and on Settings › Outside agents. Check
     the words, choose the level (the page's pick is only where it starts)
     and Approve. The page moves on by itself; it gives up after five minutes.
   - **Use a sign-in code.** Settings › Outside agents › *Get a sign-in
     code*, or on the machine

     ```sh
     rustcode outside code --access delegate
     ```

     Type the `XXXX-XXXX` code under *Have a sign-in code?*. It works once,
     for 10 minutes, and sets the most the app may do: the connection gets
     the lower of the page's pick and what the code allows. `--ask false`
     makes the app's changes run without waiting for approval in the app.
5. Ask the agent to call `whoami`.

A sign-in only reaches the app after somebody chose *Approve in RustCode* on
the page, at most two wait at once and ten can start in ten minutes, and the
two words tell your sign-in from one somebody else started at the same time:
decline a card whose words you did not see. After ten wrong codes in ten
minutes the page stops checking codes for a while. Hosted agents show in
Settings › Outside agents as signed in from the web, with the date their
sign-in lasts until, and revoke like any other connection
(`rustcode outside list`).

**What the tunnel sees.** TLS ends at the tunnel's edge, so the tunnel
provider can read the traffic between the hosted agent and RustCode: prompts,
answers and tokens. The hop from the tunnel to RustCode is loopback HTTP on the
same machine. Use a tunnel you trust with that.

**Signing in from another computer.** An agent on your laptop (Claude Code,
Codex) can use the public URL as well; it signs in through the same page and
its loopback callback returns to the laptop.

## Client walkthrough and compatibility

Documentation checked 2026-11-14. These are documented client configuration
formats, **not yet verified with a live client**. The in-process wire tests
exercise RustCode's HTTP protocol, not the clients' OAuth implementations.
Use the copied connection URL if its port differs from these examples.

| Client | stdio configuration | HTTP configuration and current limit | Live verification |
| --- | --- | --- | --- |
| Codex CLI | `~/.codex/config.toml`, `command`, `args`, and `env` below | `url`, explicit OAuth client ID and one scope below; then `codex mcp login rustcode` | not yet verified with a live client |
| Claude Code | `claude mcp add-json rustcode` with the JSON below | `claude mcp add --transport http --client-id rustcode-claude rustcode http://127.0.0.1:43871/mcp`, then `/mcp`; its `localhost` callback is accepted | not yet verified with a live client |
| Claude Desktop | Settings › Developer › Edit Config; merge the `mcpServers` entry below and restart | Settings › Connectors › Add custom connector uses a remote URL. Anthropic connects from its cloud, so use the public URL; see [Hosted agents](#hosted-agents-chatgpt-claude-and-other-cloud-clients) | not yet verified with a live client |
| ChatGPT web (hosted custom MCP) | No stdio configuration in the documented custom MCP flow | Enable developer mode, then create a custom MCP app/plugin with the public URL (`https://<public_url>/mcp`) and OAuth authentication; see [Hosted agents](#hosted-agents-chatgpt-claude-and-other-cloud-clients) | not yet verified with a live client |

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

Run `codex mcp login rustcode`. With an explicit client ID Codex skips
registration; without one it registers itself at `/register`. Requesting one
scope keeps the level you chose (the highest scope requested counts). In RustCode's approval card, check the client, callback and
Read-only access, then press Allow. The browser returns to the client's
loopback callback. Ask Codex to call `whoami`; it should report Read-only.
This sequence still needs live verification, including the client's callback
host, resource parameter and OAuth exchange behavior. Do not treat a saved
configuration as a successful connection.

After expiration or restart, log in again; after revocation, authorize a new
connection. The saved port normally keeps the URL stable. If another process
occupies it, copy the updated URL from RustCode and update the client.
To reach this endpoint from another computer or a hosted agent, set
`[outside] public_url` and serve it through a tunnel, as described next; the
endpoint answers only its loopback Host and that one public host.

### Documentation sources

- [Codex MCP configuration and OAuth](https://developers.openai.com/codex/mcp)
- [Claude Code transports, JSON configuration and OAuth credentials](https://code.claude.com/docs/en/mcp)
- [Claude Desktop local MCP setup](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop)
- [Claude custom connectors and cloud connectivity](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp)
- [ChatGPT custom MCP setup](https://developers.openai.com/api/docs/guides/custom-mcp-server)
