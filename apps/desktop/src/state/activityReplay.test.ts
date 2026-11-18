// A thread opened while its agent works outside a prompt (a turn it woke for once a background
// command finished, or background commands still running): the engine tells the load what the
// thread does now, and the app shows it working instead of finished.

import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { resetForTests, useApp, watchMachineConnection } from "./store";
import { isLoaded, isRunning, isWoken } from "./thread";
import { openThread } from "./threadOpen";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const RAW_THREAD = "mock-linux-1";
const THREAD = `${LINUX}/${RAW_THREAD}`;
const CWD = "/home/lars/code/rustcode";

/** A machine whose `session/load` replays one row, then says the thread's activity, then answers. */
function machineConnection(activity: { state: string; background: number }): AcpConnection {
  let conn!: AcpConnection;
  const notify = (method: string, params: JsonObject) =>
    conn.receive(JSON.stringify({ jsonrpc: "2.0", method, params }));
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    queueMicrotask(() => {
      if (conn.isClosed) return;
      if (frame.method === "session/load") {
        notify("session/update", {
          sessionId: RAW_THREAD,
          update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Merging #3967." } },
        });
        notify("_rustcode/session/activity", { sessionId: RAW_THREAD, ...activity });
        conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { configOptions: [] } }));
        return;
      }
      conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { sessions: [] } }));
    });
  });
  return conn;
}

beforeEach(() => {
  watchMachineConnection(LINUX, null);
  resetForTests();
  useApp.setState({ sessions: [{ id: THREAD, host: LINUX, cwd: CWD, title: "Merge it", updatedAt: 1_000 }] });
});

afterAll(() => {
  watchMachineConnection(LINUX, null);
  resetForTests();
});

describe("opening a thread that works outside a prompt", () => {
  test("shows the turn the agent woke for as running, with no finished footer", async () => {
    watchMachineConnection(LINUX, machineConnection({ state: "running", background: 1 }));
    expect(await openThread(THREAD)).toBeNull();
    const thread = useApp.getState().threads[THREAD];
    expect(isLoaded(thread)).toBe(true);
    expect(isRunning(thread)).toBe(true);
    expect(isWoken(thread)).toBe(true);
    expect(thread?.waiting).toBe(1);
    expect(thread?.items.some((item) => item.kind === "turnEnd")).toBe(false);
  });

  test("keeps a thread parked on background commands waiting", async () => {
    watchMachineConnection(LINUX, machineConnection({ state: "waiting", background: 2 }));
    expect(await openThread(THREAD)).toBeNull();
    const thread = useApp.getState().threads[THREAD];
    expect(isRunning(thread)).toBe(false);
    expect(thread?.waiting).toBe(2);
  });
});
