# MCP Apps in tool results

An MCP tool can return an interactive view next to its text: an embedded
resource with a `ui://` URI and the MIME type `text/html;profile=mcp-app`
([MCP Apps, spec 2026-01-26](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)).
The desktop app draws it in a bounded card under the call's row (issue 3607).
This is v1: the view is display-only.

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

## Isolation

- The frame is `sandbox="allow-scripts"` only: an opaque origin, so no cookies,
  storage or access to the app's DOM, and no top navigation, popups or forms.
- In the app the page is served from Smart UI's `rcui://` scheme with its strict
  CSP header (`apps/desktop/src-tauri/src/smartui.rs`); the same policy is put
  in a meta tag ahead of the app's own scripts: `default-src 'none'`, inline
  script and style, `data:` images, no network (`connect-src 'none'`).
- The card has a max height and scrolls; the frame's height follows
  `ui/notifications/size-changed`, clamped to 48–2000 px.

## Messages

Only messages whose `source` is that frame's own window are read, and only
JSON-RPC 2.0.

| From the app | The host |
| --- | --- |
| `ui/initialize` | answers with `hostInfo`, `hostContext.theme` and `displayMode: "inline"` |
| `ui/notifications/initialized` | sends `ui/notifications/tool-input` and `ui/notifications/tool-result` (the call's arguments and text) |
| `ui/notifications/size-changed` | resizes the frame, clamped |
| `ping` | answers `{}` |
| any other request (`tools/call`, `resources/read`, `ui/open-link`, `ui/message`, …) | answers JSON-RPC error `-32601`, "… is not supported yet" |
| any other notification | ignored |

The host also sends `ui/notifications/host-context-changed` when the app's
theme changes.

## Not yet

- Tool calls from an app, routed through the thread's permissions and shown
  with their origin.
- `resources/read` for apps a tool names only in `_meta.ui.resourceUri`,
  without embedded content.
- Sending the HTML to remote clients, and the built-in harness's own MCP client.
