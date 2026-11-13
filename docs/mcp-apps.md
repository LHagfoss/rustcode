# MCP Apps in tool results

An MCP tool can return an interactive view next to its text: an embedded
resource with a `ui://` URI and the MIME type `text/html;profile=mcp-app`
([MCP Apps, spec 2026-01-26](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)).
The desktop app draws it in a bounded card under the call's row (issue 3607).
On the built-in harness the view can also call the tools of its own server, after
the user approves each call (see [Tool calls from an app](#tool-calls-from-an-app)).

## Where it is carried

- **Desktop ACP parser** (`apps/desktop/src/acp/types.ts`, `src/lib/mcpApp.ts`):
  a tool call's `content` block `{ type: "resource", resource: { uri, mimeType, text } }`
  with a `ui://` URI and the app MIME type (plain `text/html` under `ui://` is
  accepted too) becomes the call's optional `app`. Its HTML is not added to the
  call's output text.
- **Remote mirror** (`rustcode/engine/src/remote/harness.rs`): the same block is
  kept on the result's `ToolResultRecord.mcp_app` (`rustcode-core::mcp_app`),
  and a remote client sees `RemoteMessageTool.mcp_app` with the URI, MIME type,
  size and whether the HTML was kept. The HTML itself is not sent to remote
  clients yet.
- HTML over 512 KB is not kept: the app is recorded without it and the card says
  the view is unavailable.

Harnesses: any harness whose ACP adapter passes an MCP tool result's embedded
resources through as `resource` content. A harness that flattens tool results
to text shows the text only, as before. Transports are whatever the harness
uses to reach the MCP server; RustCode only sees the ACP tool call.

## Where the HTML comes from

- **Embedded** (any harness): the tool result carries the `ui://` resource, as above.
- **Declared** (built-in harness only): the tool's definition names its view in
  `_meta.ui.resourceUri`. The desktop asks the engine to `read` it
  (`_rustcode/mcp/app`, `rustcode/engine/src/acp/mcp_app.rs`). The engine resolves
  the call's tool name to the exact MCP connection that serves it and reads only
  the `ui://` URI that tool declared (`McpClient::read_tool_app`): an app cannot
  name another resource, and a server of the same name started for another
  purpose is never consulted. Only calls named `mcp__server__tool` are tried.
  A server that is gone shows nothing; any other failure shows
  "Interactive view unavailable · reason".
- Another harness (Claude Code, Codex, ...) owns its MCP servers, so RustCode has no
  connection to read from or call: its apps are display-only, and the engine
  refuses the request for such a thread.

## Security model

An app is untrusted markup from a third party's server. Each boundary is
enforced separately, and each fails closed.

**Resources.** The app's HTML is capped at 512 KB (larger apps are recorded
without it and the card says so). MCP responses are bounded before they are
parsed: 16 MB per stdio line or HTTP body, so a server cannot exhaust memory.
Tool-call arguments from an app are capped at 512 KB.

**Isolation.**

- The frame is `sandbox="allow-scripts"` only: an opaque origin, so no cookies,
  storage or access to the app's DOM, and no top navigation, popups, forms or
  downloads. `referrerPolicy="no-referrer"`.
- In the app the page is served from Smart UI's `rcui://` scheme with its strict
  CSP header (`apps/desktop/src-tauri/src/smartui.rs`); the same policy is put
  in a meta tag ahead of the app's own scripts: `default-src 'none'`, inline
  script and style, `data:` images, `connect-src 'none'`, no frames, no forms.
  An app cannot loosen it: every policy on a page applies.
- **Network.** None. The resource's own `_meta.ui.csp` domain lists are not
  honoured in this version, so an app is never more open than it asked to be, and
  nothing it draws can fetch or send data. Opening links and loading remote assets
  are refused (`ui/open-link` answers "not supported yet").
- The card has a max height and scrolls; the frame's height follows
  `ui/notifications/size-changed`, clamped to 48-2000 px.
- The app only ever receives the call's own arguments and output text, theme and
  its own results. It gets no credentials, no other thread data and no
  resource reads.

**Messages.** Only messages whose `source` is that frame's own window are read, and
only JSON-RPC 2.0.

| From the app | The host |
| --- | --- |
| `ui/initialize` | answers with `hostInfo`, `hostContext.theme`, `displayMode: "inline"`, and `serverTools` when it can call |
| `ui/notifications/initialized` | sends `ui/notifications/tool-input` and `ui/notifications/tool-result` (the call's arguments and text) |
| `ui/notifications/size-changed` | resizes the frame, clamped |
| `ping` | answers `{}` |
| `tools/call` | see below; a display-only host answers `-32601` |
| any other request (`resources/read`, `ui/open-link`, `ui/message`, ...) | answers JSON-RPC error `-32601`, "... is not supported yet" |
| any other notification | ignored |

The host also sends `ui/notifications/host-context-changed` when the app's
theme changes.

## Tool calls from an app

A `tools/call` from the frame (built-in harness only) goes to the engine as
`_rustcode/mcp/app` with `action: "call"`, the originating call's name and the
app's tool and arguments. The engine:

1. Resolves the originating call to its MCP connection, as above. The app cannot
   choose the server.
2. Requires the tool to be marked app-visible in the server's own definition
   (`_meta.ui.visibility` contains `"app"`; absent means both audiences; a
   malformed value means neither). A tool marked `["app"]` only is never offered to
   the model, and a tool marked `["model"]` only cannot be called by an app.
3. Sends the call through the turn policy an agent's tool call goes through
   (`AcpPolicy::should_approve`, `acp/permissions.rs`) as an `mcp__server__tool`
   call: plan mode refuses it, and an MCP tool always asks, unless the session
   runs with auto-approve, exactly as for the agent. The permission prompt
   is titled "MCP App requests mcp__server__tool" and shows the arguments, so the
   origin is the app and not the agent. Only "Allow once" and "Reject" are offered:
   an app's call is never remembered, so no grant outlives the call.
4. Runs it on that connection only if approved. A rejection, a refused policy, an
   unreachable server or a bad argument answers the app with a short JSON-RPC error
   (`-32000` or `-32602`), never the engine's internals.

The frame can have one call waiting at a time; a second is refused, not queued.
Malformed `tools/call` params (no name, a name over 256 characters, non-object
arguments) are answered with `-32602` without reaching the engine.

## Tests

- `apps/desktop/src/lib/mcpApp.test.ts`: detection, size cap, page policy, sender
  check, handshake, clamped resizing, refused methods, tool-call parsing and
  replies.
- `rustcode/engine/src/mcp.rs`: bounded lines, visibility defaults and failing
  closed, reading only the declared resource, app calls needing visibility and
  approval.
- `rustcode/engine/src/acp/mcp_app.rs`: request shape, origin-named approval, refusal
  for another harness and for a server that is not connected.
- `rustcode/engine/src/remote/harness.rs`: the mirror keeps the app next to the text.

## Not yet

- `resources/read`, `ui/open-link` and `ui/message` from an app.
- The resource's `_meta.ui.csp` domains (network is none by design for now), and
  app calls and reads for calls the harness ran (Claude Code, Codex, ...).
- Declared apps for a tool whose raw name is not `mcp__server__tool`.
- Sending the HTML to remote clients.
