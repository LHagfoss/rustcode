// `?tools=background`: the mock's thread of a tool row's hard cases, replayed
// the way the app reads it — a command the harness backgrounded, its answer
// fenced in Markdown, and the call repeated with its description in `content`.

import { describe, expect, test } from "bun:test";
import { applyUpdate, beginTurn, emptyThread, type ThreadState } from "../state/thread";
import type { Json, JsonObject } from "./jsonrpc";
import { MockTransport } from "./mock";
import { parseSessionUpdate, type SessionUpdate } from "./types";

/** One mock engine's updates for the thread the `?tools=` case seeds, as the app replays them. */
async function replayed(): Promise<ThreadState> {
  const updates: SessionUpdate[] = [];
  const waiting = new Map<number, (value: Json) => void>();
  let nextId = 1;
  let opened: (() => void) | null = null;
  const open = new Promise<void>((resolve) => {
    opened = resolve;
  });
  const receive = (line: string) => {
    const message = JSON.parse(line) as {
      id?: number;
      method?: string;
      params?: JsonObject;
      result?: Json;
    };
    if (message.method === "session/update" && message.params) {
      updates.push(parseSessionUpdate(message.params));
      return;
    }
    if (typeof message.id !== "number") return;
    waiting.get(message.id)?.(message.result ?? null);
    waiting.delete(message.id);
  };
  const transport = new MockTransport({ tools: "background", delayMs: 0 });
  const started = transport.start((event) => {
    if (event.type === "open") opened?.();
    else if (event.type === "line") receive(event.line);
    else if (event.type === "lines") event.lines.forEach(receive);
  });
  await Promise.all([started, open]);
  const request = async (method: string, params: JsonObject = {}): Promise<Json> => {
    const id = nextId++;
    return await new Promise<Json>((resolve) => {
      waiting.set(id, resolve);
      transport.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  };
  const listed = (await request("session/list")) as { sessions: { sessionId: string }[] };
  const id = listed.sessions.find((s) => s.sessionId === "mock-tool-rows")?.sessionId;
  if (!id) throw new Error("?tools=background seeded no thread");
  await request("session/load", { sessionId: id });
  let state = beginTurn(emptyThread(id, "/p"), "Build the app");
  for (const update of updates) state = applyUpdate(state, update);
  return state;
}

describe("the mock's background tool rows", () => {
  test("its command is finished, says so, and prints the task's id without the fence", async () => {
    const state = await replayed();
    const call = state.items.find((i) => i.id === "toolu_bg");
    expect(call).toMatchObject({ kind: "tool", status: "completed", background: "command" });
    if (call?.kind !== "tool") throw new Error("no background call");
    expect(call.output).toStartWith("Command running in background with ID: bc82qcgu4.");
    expect(call.output).not.toContain("```");
  });

  test("the call it repeated with the description has no output of its own", async () => {
    const state = await replayed();
    const call = state.items.find((i) => i.id === "toolu_test");
    if (call?.kind !== "tool") throw new Error("no second call");
    expect(call.output).toStartWith("$ bun run check");
    expect(call.output).not.toContain("```");
    expect(state.items.some((i) => i.kind === "tool" && i.output?.includes("Build the Release app"))).toBe(
      false,
    );
  });
});
