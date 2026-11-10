// `?notices=repeat`: the first thread's turn ends on six identical quiet notices, the shape an
// access mode that lets calls through leaves (one notice per call). The mock replays them as the
// engine's lines; this reads them back through the app's own update handling, so the test covers
// what the transcript is given, not only the mock's canned data.

import { describe, expect, test } from "bun:test";
import { collapseNotices, foldTurns } from "../state/fold";
import { applyUpdate, emptyThread, type ThreadState } from "../state/thread";
import type { JsonObject } from "./jsonrpc";
import { MockTransport } from "./mock";
import { parseSessionUpdate } from "./types";

const ALLOWED = "Cleaned up a delegate's worktree · allowed by Full access";
const THREAD = "mock-thread-1";
const CWD = "/Users/you/code/rustcode";

/** The thread as the app has it after `session/load`, from the mock's replayed lines. */
async function loaded(notices?: "repeat"): Promise<ThreadState> {
  const transport = new MockTransport({ notices, needsYou: false });
  const lines: JsonObject[] = [];
  await transport.start((event) => {
    if (event.type === "line") lines.push(JSON.parse(event.line) as JsonObject);
  });
  transport.send(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "session/load",
      params: { sessionId: THREAD, cwd: CWD },
    }),
  );
  for (let i = 0; i < 200 && !lines.some((line) => line.id === 1); i++) await Bun.sleep(5);
  let state = emptyThread(THREAD, CWD, "Fix the composer focus ring");
  for (const line of lines) {
    if (line.method !== "session/update") continue;
    state = applyUpdate(state, parseSessionUpdate(line.params as JsonObject));
  }
  return state;
}

describe("?notices=repeat", () => {
  test("the turn ends on six identical notices, drawn as one row with its count", async () => {
    const state = await loaded("repeat");
    const notices = state.items.flatMap((item) => (item.kind === "notice" ? [item] : []));
    expect(notices).toHaveLength(6);
    expect(notices.every((notice) => notice.text === ALLOWED && notice.tone === "info")).toBe(true);

    const rows = collapseNotices(foldTurns(state.items, false, new Set()));
    const drawn = rows.flatMap((row) => ("item" in row && row.item.kind === "notice" ? [row.item] : []));
    expect(drawn).toHaveLength(1);
    expect(drawn[0]).toMatchObject({ text: ALLOWED, count: 6 });
  });

  test("the default mock leaves no notice at all", async () => {
    const state = await loaded();
    expect(state.items.some((item) => item.kind === "notice")).toBe(false);
  });
});
