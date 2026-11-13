// MCP Apps (the `ui://` extension of MCP, spec 2026-01-26): a tool result can
// embed an HTML view. The app draws it in a sandboxed frame next to the
// result's text (`ui/McpAppView.tsx`). This file holds what can be checked
// without a DOM: which content is an app, the size cap, the page the frame
// gets and what the host answers to the app's messages.
//
// v1 is display-only. The frame runs with `allow-scripts` alone (an opaque
// origin: no cookies, storage, top navigation, popups or forms), a CSP that
// allows no network, and a message channel that accepts only that frame's
// window and answers anything past the handshake and resizing with "not
// supported yet". Tool calls from an app, routed through the thread's
// permissions, come later (issue 3607).

import type { Json, JsonObject } from "../acp/jsonrpc";

/** The MIME type the MCP Apps extension gives an app's HTML. */
export const MCP_APP_MIME_TYPE = "text/html;profile=mcp-app";

/** The largest app HTML the app keeps and draws; larger ones fall back to the text result. */
export const MCP_APP_MAX_BYTES = 512 * 1024;

/** The frame's sandbox: scripts only, never `allow-same-origin`, navigation, popups or forms. */
export const MCP_APP_SANDBOX = "allow-scripts";

/** The page's policy, repeated in a meta tag so it binds even without the app's page server. */
export const MCP_APP_CSP =
  "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; " +
  "connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";

/** The heights a frame may ask for; the card scrolls past its own max height. */
export const MCP_APP_MIN_HEIGHT = 48;
export const MCP_APP_MAX_HEIGHT = 2000;
/** The height the card shows the frame at before the app reports its own. */
export const MCP_APP_DEFAULT_HEIGHT = 240;

/** The MCP Apps protocol version this host speaks. */
export const MCP_APP_PROTOCOL_VERSION = "2026-01-26";

/** An interactive view a tool result offered. */
export interface McpApp {
  /** The resource's `ui://` URI. */
  uri: string;
  mimeType: string;
  /** The app's HTML; absent when it was over {@link MCP_APP_MAX_BYTES}. */
  html?: string;
  /** The HTML's size in bytes, whether or not it was kept. */
  totalBytes: number;
}

function obj(value: Json | undefined): JsonObject | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value : undefined;
}

function str(value: Json | undefined): string | undefined {
  return typeof value === "string" ? value : undefined;
}

/** Whether a resource names an MCP App: a `ui://` URI with an HTML type (the app profile or plain). */
export function isMcpApp(uri: string | undefined, mimeType: string | undefined): boolean {
  if (!uri?.startsWith("ui://") || mimeType === undefined) return false;
  const mime = mimeType.replace(/\s+/g, "").toLowerCase();
  return mime === MCP_APP_MIME_TYPE || mime === "text/html";
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** The app an embedded resource (`{ type: "resource", resource }`) is, if it is one. */
export function mcpAppOf(block: Json | undefined): McpApp | undefined {
  const b = obj(block);
  if (str(b?.type) !== "resource") return undefined;
  const resource = obj(b?.resource);
  const uri = str(resource?.uri);
  const mimeType = str(resource?.mimeType);
  const text = str(resource?.text);
  if (!isMcpApp(uri, mimeType) || text === undefined) return undefined;
  const totalBytes = byteLength(text);
  return {
    uri: uri as string,
    mimeType: mimeType as string,
    html: totalBytes <= MCP_APP_MAX_BYTES ? text : undefined,
    totalBytes,
  };
}

/** The first app a tool call's content (`[{ type: "content", content }]`) embeds. */
export function toolCallApp(content: Json | undefined): McpApp | undefined {
  if (!Array.isArray(content)) return undefined;
  for (const entry of content) {
    const app = mcpAppOf(obj(entry)?.content);
    if (app) return app;
  }
  return undefined;
}

/**
 * The page the frame loads: the app's HTML with the policy as the first thing
 * in its head, before any script it carries. A policy the app adds itself can
 * only narrow this one: every policy on a page applies.
 */
export function mcpAppPage(html: string): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${MCP_APP_CSP}">`;
  const head = /<head(\s[^>]*)?>/i.exec(html);
  if (head) {
    const at = head.index + head[0].length;
    return html.slice(0, at) + meta + html.slice(at);
  }
  const root = /<html(\s[^>]*)?>/i.exec(html) ?? /<!doctype[^>]*>/i.exec(html);
  if (root) {
    const at = root.index + root[0].length;
    return `${html.slice(0, at)}<head>${meta}</head>${html.slice(at)}`;
  }
  return `<!doctype html><html><head>${meta}</head><body>${html}</body></html>`;
}

/** Whether a message is the app's: sent by this frame's window, never another frame or the app. */
export function fromApp(source: unknown, frame: unknown): boolean {
  return source !== null && source !== undefined && source === frame;
}

export interface McpAppHost {
  theme: "light" | "dark";
  /** The tool's arguments and result text, sent once the app says it is ready. */
  input?: Json;
  output?: string;
}

export interface McpAppStep {
  /** What to post back to the frame, in order. */
  replies: JsonObject[];
  /** A new frame height, already clamped. */
  height?: number;
}

/** A JSON-RPC error for a method this host does not offer yet. */
export const NOT_SUPPORTED = -32601;

export function clampAppHeight(height: number): number {
  if (!Number.isFinite(height)) return MCP_APP_DEFAULT_HEIGHT;
  return Math.round(Math.min(MCP_APP_MAX_HEIGHT, Math.max(MCP_APP_MIN_HEIGHT, height)));
}

/** What the host answers an app's message with. Anything that is not JSON-RPC 2.0 is ignored. */
export function handleAppMessage(data: unknown, host: McpAppHost): McpAppStep {
  const message = obj(data as Json);
  if (message?.jsonrpc !== "2.0") return { replies: [] };
  const method = str(message.method);
  const id = message.id;
  const isRequest = (typeof id === "string" || typeof id === "number") && method !== undefined;

  if (method === "ui/initialize" && isRequest) {
    return {
      replies: [
        {
          jsonrpc: "2.0",
          id,
          result: {
            protocolVersion: MCP_APP_PROTOCOL_VERSION,
            hostInfo: { name: "RustCode", version: "1" },
            hostCapabilities: {},
            hostContext: {
              theme: host.theme,
              displayMode: "inline",
              availableDisplayModes: ["inline"],
              platform: "desktop",
            },
          },
        },
      ],
    };
  }
  if (method === "ui/notifications/initialized" && !isRequest) {
    const replies: JsonObject[] = [];
    if (host.input !== undefined) {
      const args = obj(host.input) ?? {};
      replies.push({ jsonrpc: "2.0", method: "ui/notifications/tool-input", params: { arguments: args } });
    }
    if (host.output !== undefined) {
      replies.push({
        jsonrpc: "2.0",
        method: "ui/notifications/tool-result",
        params: { content: [{ type: "text", text: host.output }] },
      });
    }
    return { replies };
  }
  if (method === "ui/notifications/size-changed" && !isRequest) {
    const height = Number(obj(message.params)?.height);
    return Number.isFinite(height) ? { replies: [], height: clampAppHeight(height) } : { replies: [] };
  }
  if (method === "ping" && isRequest) return { replies: [{ jsonrpc: "2.0", id, result: {} }] };
  if (isRequest) {
    // Tool calls, resource reads, links and messages: v1 offers none of them.
    return {
      replies: [
        { jsonrpc: "2.0", id, error: { code: NOT_SUPPORTED, message: `${method} is not supported yet` } },
      ],
    };
  }
  return { replies: [] };
}

/** The notification that tells a running app the theme changed. */
export function themeChanged(theme: "light" | "dark"): JsonObject {
  return { jsonrpc: "2.0", method: "ui/notifications/host-context-changed", params: { theme } };
}
