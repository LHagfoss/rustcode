import { expect, test } from "bun:test";
import { parseSessionUpdate } from "../acp/types";
import {
  CALL_FAILED,
  callReply,
  fromApp,
  handleAppMessage,
  INVALID_PARAMS,
  isMcpApp,
  MAX_APP_TOOL_NAME,
  MCP_APP_CSP,
  MCP_APP_MAX_BYTES,
  MCP_APP_MAX_HEIGHT,
  MCP_APP_MIME_TYPE,
  MCP_APP_MIN_HEIGHT,
  MCP_APP_SANDBOX,
  mayHaveApp,
  mcpAppOf,
  mcpAppPage,
  NOT_SUPPORTED,
  parseAppRead,
} from "./mcpApp";

const appBlock = (text: string, mimeType = "text/html;profile=mcp-app", uri = "ui://weather/view") => ({
  type: "content",
  content: { type: "resource", resource: { uri, mimeType, text } },
});

test("an MCP App is a ui:// resource with an HTML type", () => {
  expect(isMcpApp("ui://a", "text/html;profile=mcp-app")).toBe(true);
  expect(isMcpApp("ui://a", "text/html; profile=mcp-app")).toBe(true);
  expect(isMcpApp("ui://a", "text/html")).toBe(true);
  expect(isMcpApp("https://a", "text/html;profile=mcp-app")).toBe(false);
  expect(isMcpApp("ui://a", "text/plain")).toBe(false);
  expect(isMcpApp("ui://a", undefined)).toBe(false);
});

test("a tool result keeps its app next to the text, and the HTML is not the output", () => {
  const update = parseSessionUpdate({
    update: {
      sessionUpdate: "tool_call_update",
      toolCallId: "call-1",
      status: "completed",
      content: [{ type: "content", content: { type: "text", text: "Sunny, 21C" } }, appBlock("<p>Sunny</p>")],
    },
  });
  if (update.kind !== "toolCallUpdate") throw new Error(update.kind);
  expect(update.output).toBe("Sunny, 21C");
  expect(update.app).toEqual({
    uri: "ui://weather/view",
    mimeType: "text/html;profile=mcp-app",
    html: "<p>Sunny</p>",
    totalBytes: 12,
  });
});

test("an ordinary embedded resource stays text", () => {
  const update = parseSessionUpdate({
    update: {
      sessionUpdate: "tool_call",
      toolCallId: "call-2",
      title: "read",
      status: "completed",
      content: [appBlock("file text", "text/plain", "file:///a.txt")],
    },
  });
  if (update.kind !== "toolCall") throw new Error(update.kind);
  expect(update.app).toBeUndefined();
  expect(update.output).toBe("file text");
});

test("an app over the cap is kept without its HTML", () => {
  const big = "x".repeat(MCP_APP_MAX_BYTES + 1);
  const app = mcpAppOf(appBlock(big).content);
  expect(app?.html).toBeUndefined();
  expect(app?.totalBytes).toBe(MCP_APP_MAX_BYTES + 1);
  expect(mcpAppOf(appBlock("x".repeat(MCP_APP_MAX_BYTES)).content)?.html).toBeDefined();
});

test("the frame runs scripts only, with a policy ahead of the app's own scripts", () => {
  expect(MCP_APP_SANDBOX).toBe("allow-scripts");
  for (const forbidden of ["allow-same-origin", "allow-top-navigation", "allow-popups", "allow-forms"]) {
    expect(MCP_APP_SANDBOX).not.toContain(forbidden);
  }
  expect(MCP_APP_CSP).toContain("default-src 'none'");
  expect(MCP_APP_CSP).toContain("connect-src 'none'");
  const page = mcpAppPage(
    '<!doctype html><html><head><script src="x.js"></script></head><body></body></html>',
  );
  expect(page.indexOf("Content-Security-Policy")).toBeLessThan(page.indexOf("<script"));
  expect(mcpAppPage("<p>bare</p>")).toStartWith(
    '<!doctype html><html><head><meta http-equiv="Content-Security-Policy"',
  );
  expect(mcpAppPage("<html lang=en><body>x</body></html>")).toContain("<html lang=en><head><meta");
});

test("only the frame's own window is heard", () => {
  const frame = {};
  expect(fromApp(frame, frame)).toBe(true);
  expect(fromApp({}, frame)).toBe(false);
  expect(fromApp(null, null)).toBe(false);
  expect(fromApp(undefined, undefined)).toBe(false);
});

test("the handshake answers with the host and its theme, then sends the tool's input and result", () => {
  const host = { theme: "dark" as const, input: { city: "Oslo" }, output: "Sunny" };
  const init = handleAppMessage({ jsonrpc: "2.0", id: 1, method: "ui/initialize", params: {} }, host);
  expect(init.replies).toHaveLength(1);
  const result = init.replies[0]?.result as { hostInfo: { name: string }; hostContext: { theme: string } };
  expect(init.replies[0]?.id).toBe(1);
  expect(result.hostInfo.name).toBe("RustCode");
  expect(result.hostContext.theme).toBe("dark");
  const ready = handleAppMessage({ jsonrpc: "2.0", method: "ui/notifications/initialized" }, host);
  expect(ready.replies.map((r) => r.method)).toEqual([
    "ui/notifications/tool-input",
    "ui/notifications/tool-result",
  ]);
});

test("size changes are clamped", () => {
  const host = { theme: "light" as const };
  const size = (height: unknown) =>
    handleAppMessage({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height } }, host)
      .height;
  expect(size(300)).toBe(300);
  expect(size(1e9)).toBe(MCP_APP_MAX_HEIGHT);
  expect(size(-5)).toBe(MCP_APP_MIN_HEIGHT);
  expect(size("tall")).toBeUndefined();
});

test("tool calls without a route to the server and every other request are refused as not supported yet", () => {
  const host = { theme: "light" as const };
  const call = handleAppMessage(
    { jsonrpc: "2.0", id: "a", method: "tools/call", params: { name: "delete_everything" } },
    host,
  );
  expect(call.replies).toEqual([
    { jsonrpc: "2.0", id: "a", error: { code: NOT_SUPPORTED, message: "tools/call is not supported yet" } },
  ]);
  expect(
    handleAppMessage({ jsonrpc: "2.0", id: 2, method: "ui/open-link" }, host).replies[0]?.error,
  ).toBeDefined();
  // Unknown notifications and anything that is not JSON-RPC get no answer.
  expect(handleAppMessage({ jsonrpc: "2.0", method: "ui/notifications/whatever" }, host).replies).toEqual([]);
  expect(handleAppMessage({ id: 3, method: "tools/call" }, host).replies).toEqual([]);
  expect(handleAppMessage("hello", host).replies).toEqual([]);
});

test("an app's tool call is handed to the host only where it can reach the server", () => {
  const request = {
    jsonrpc: "2.0",
    id: 7,
    method: "tools/call",
    params: { name: "refresh", arguments: { a: 1 } },
  };
  const connected = { theme: "light" as const, canCall: true };
  const step = handleAppMessage(request, connected);
  expect(step.replies).toEqual([]);
  expect(step.call).toEqual({ id: 7, tool: "refresh", arguments: { a: 1 } });
  // Without arguments the call has none; the capability is only advertised when it can be used.
  expect(handleAppMessage({ ...request, params: { name: "refresh" } }, connected).call?.arguments).toEqual(
    {},
  );
  const init = (host: { theme: "light"; canCall?: boolean }) => {
    const result = handleAppMessage({ jsonrpc: "2.0", id: 1, method: "ui/initialize" }, host).replies[0]
      ?.result as { hostCapabilities: object } | undefined;
    return result?.hostCapabilities;
  };
  expect(init(connected)).toEqual({ serverTools: {} });
  expect(init({ theme: "light" })).toEqual({});
  // Display-only hosts refuse it, and the host never sees it.
  const refused = handleAppMessage(request, { theme: "light" as const });
  expect(refused.call).toBeUndefined();
  expect(refused.replies[0]?.error).toMatchObject({ code: NOT_SUPPORTED });
});

test("an app's malformed tool call is answered with invalid params", () => {
  const host = { theme: "light" as const, canCall: true };
  const bad = (params: unknown) =>
    handleAppMessage({ jsonrpc: "2.0", id: 1, method: "tools/call", params }, host);
  for (const params of [
    undefined,
    {},
    { name: "" },
    { name: 5 },
    { name: "x".repeat(MAX_APP_TOOL_NAME + 1) },
    { name: "t", arguments: [] },
    { name: "t", arguments: "rm -rf" },
  ]) {
    const step = bad(params);
    expect(step.call).toBeUndefined();
    expect(step.replies[0]?.error).toMatchObject({ code: INVALID_PARAMS });
  }
});

test("a tool call's answer carries the result, or a short error", () => {
  expect(callReply(7, { result: { content: [] } })).toEqual({
    jsonrpc: "2.0",
    id: 7,
    result: { content: [] },
  });
  const failed = callReply("a", { error: "x".repeat(1000) });
  expect((failed.error as { code: number; message: string }).code).toBe(CALL_FAILED);
  expect((failed.error as { message: string }).message).toHaveLength(300);
});

test("an app the engine read is checked like an embedded one", () => {
  const read = (app: unknown) => parseAppRead({ app } as never);
  expect(read({ uri: "ui://w/v", mimeType: MCP_APP_MIME_TYPE, html: "<p>x</p>", totalBytes: 8 })).toEqual({
    uri: "ui://w/v",
    mimeType: MCP_APP_MIME_TYPE,
    html: "<p>x</p>",
    totalBytes: 8,
  });
  // Over the cap, or without HTML: the card falls back, never draws it.
  expect(
    read({ uri: "ui://w/v", mimeType: MCP_APP_MIME_TYPE, totalBytes: MCP_APP_MAX_BYTES + 1 })?.html,
  ).toBeUndefined();
  expect(
    read({
      uri: "ui://w/v",
      mimeType: MCP_APP_MIME_TYPE,
      html: "x".repeat(MCP_APP_MAX_BYTES + 1),
      totalBytes: 1,
    })?.html,
  ).toBeUndefined();
  expect(read({ uri: "https://w/v", mimeType: MCP_APP_MIME_TYPE, html: "x", totalBytes: 1 })).toBeUndefined();
  expect(parseAppRead({ app: null })).toBeUndefined();
  expect(parseAppRead(null)).toBeUndefined();
  expect(mayHaveApp("mcp__weather__show")).toBe(true);
  expect(mayHaveApp("run_command")).toBe(false);
});
