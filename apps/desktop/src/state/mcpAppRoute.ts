// The route from an MCP App's frame to the MCP server that ran its tool call
// (`_rustcode/mcp/app`, engine `acp/mcp_app.rs`). Only a thread on the built-in
// harness has one: another harness owns its MCP servers, and its apps are shown
// from the content they embed, display-only.

import type { Json } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { type McpApp, mayHaveApp, parseAppRead } from "../lib/mcpApp";
import { connectionForThread, threadHarness, useApp } from "./store";
import { splitThreadKey } from "./threadKeys";

/** How long an approval may wait for the user. */
const APPROVAL_TIMEOUT_MS = 10 * 60_000;

/** Whether a thread's MCP servers are RustCode's own, so an app can reach them. */
export function appRouteOpen(threadId: string): boolean {
  const thread = useApp.getState().threads[threadId];
  return (
    thread !== undefined && threadHarness(thread)?.builtin === true && connectionForThread(threadId) !== null
  );
}

/** The app a finished MCP call's tool declares, read from the server that ran it. */
export async function readApp(threadId: string, name: string): Promise<McpApp | undefined> {
  const conn = connectionForThread(threadId);
  if (!conn || !appRouteOpen(threadId) || !mayHaveApp(name)) return undefined;
  const result = await conn.request(
    Method.mcpApp,
    { sessionId: splitThreadKey(threadId).id, action: "read", name },
    30_000,
  );
  return parseAppRead(result);
}

/** A tool the app asks for, run on the same server after the user approves it. */
export async function callAppTool(
  threadId: string,
  name: string,
  tool: string,
  args: Record<string, Json>,
): Promise<Json> {
  const conn = connectionForThread(threadId);
  if (!conn || !appRouteOpen(threadId)) throw new Error("this thread has no connection to the app's server");
  const result = (await conn.request(
    Method.mcpApp,
    { sessionId: splitThreadKey(threadId).id, action: "call", name, tool, arguments: args },
    APPROVAL_TIMEOUT_MS,
  )) as { result?: Json } | null;
  return result?.result ?? null;
}
