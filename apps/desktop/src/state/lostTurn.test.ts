// A turn on a paired machine whose prompt request died with that machine's connection
// (issue 3440): the turn goes on there, so the delegate's link is settled by what the machine
// says the thread did, not by the half-written rows the app had when the channel went.
//
// The machine is a stand-in for the shell's supervisor and that machine's engine (the frames
// `state/store.ts` `handleInbound` reads), so the three ways it can answer are played here.

import { afterAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { emptyBook, type Link, MACHINE_LOST_TURN } from "./orchestration";
import { lostTurn, resetForTests, setLostMachineMsForTests, useApp, watchMachineConnection } from "./store";
import { emptyThread } from "./thread";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const THREAD = `${LINUX}/mock-linux-1`;
const RAW_THREAD = "mock-linux-1";
const CWD = "/home/lars/code/rustcode";
const LINK = "parent/1";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Waits for a condition the app reaches asynchronously, up to a second. */
async function until(ready: () => boolean): Promise<void> {
  for (let waited = 0; waited < 1_000; waited += 5) {
    if (ready()) return;
    await sleep(5);
  }
  throw new Error("the app never got there");
}

const link = () => useApp.getState().links.links.find((l) => l.id === LINK);

/**
 * A machine's connection: `state.status` is what its `session/list` says its thread does, and a
 * load replays `state.reply` (or `state.notice`) as the thread's last turn.
 */
function machineConnection(state: { status: string; reply?: string; notice?: string }): AcpConnection {
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    queueMicrotask(() => {
      if (conn.isClosed) return;
      if (frame.method === "session/list") {
        const sessions = state.status
          ? [{ sessionId: RAW_THREAD, _meta: { rustcode: { status: state.status } } }]
          : [];
        conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { sessions } }));
        return;
      }
      // A load replays the transcript as it goes: the turn's rows, then the answer to the load.
      if (frame.method === "session/load" && state.reply) {
        conn.receive(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "session/update",
            params: {
              sessionId: RAW_THREAD,
              update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: state.reply },
              },
            },
          }),
        );
      }
      if (frame.method === "session/load" && state.notice) {
        conn.receive(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "session/update",
            params: {
              sessionId: RAW_THREAD,
              update: {
                sessionUpdate: "agent_message_chunk",
                content: { type: "text", text: state.notice },
                _meta: { rustcode: { kind: "notice", resumable: true } },
              },
            },
          }),
        );
      }
      conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { configOptions: [] } }));
    });
  });
  return conn;
}

/** What a machine broadcasts about a thread of its own, as this window reads it. */
function statusFrame(status: string): string {
  return JSON.stringify({
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId: RAW_THREAD,
      update: { sessionUpdate: "session_info_update", _meta: { rustcode: { status } } },
    },
  });
}

beforeEach(() => {
  watchMachineConnection(LINUX, null);
  resetForTests();
  const working: Link = {
    id: LINK,
    orchestrator: "parent",
    target: THREAD,
    orchestratorTitle: "Fix the tests",
    targetTitle: "Run the tests",
    message: "run the tests",
    state: "working",
    started: true,
    machine: "Lars's Linux box",
    createdAt: 1_000,
    startedAt: 2_000,
  };
  useApp.setState((s) => ({
    links: { ...emptyBook(), links: [working] },
    sessions: [{ id: THREAD, host: LINUX, cwd: CWD, title: "Run the tests", updatedAt: 1_000 }],
    threads: { ...s.threads, [THREAD]: { ...emptyThread(THREAD, CWD, "Run the tests"), loaded: true } },
  }));
});

afterAll(() => {
  watchMachineConnection(LINUX, null);
  resetForTests();
});

describe("a delegate whose machine went away mid-turn", () => {
  test("does not take a half-written answer, and follows the turn until it stops", async () => {
    // The rows that had arrived when the channel went are half an answer, not one.
    useApp.setState((s) => ({
      threads: {
        ...s.threads,
        [THREAD]: {
          ...s.threads[THREAD]!,
          items: [
            { kind: "user", id: "u1", text: "run the tests" },
            { kind: "assistant", id: "a1", text: "All 12 tests" },
          ],
        },
      },
    }));
    lostTurn(THREAD, [LINK]);
    await sleep(20);
    expect(link()?.state).toBe("working");

    // Back, with the turn still running there: the link keeps working rather than guessing.
    const machine = { status: "running", reply: "All 12 tests pass." };
    const conn = machineConnection(machine);
    watchMachineConnection(LINUX, conn);
    await until(() => useApp.getState().threads[THREAD]?.items.length === 1);
    expect(link()?.state).toBe("working");

    // The turn ends there, and the machine broadcasts the thread's new status.
    machine.status = "idle";
    conn.receive(statusFrame("idle"));
    await until(() => link()?.state === "done");
    expect(link()?.answer).toBe("All 12 tests pass.");
    expect(link()?.error).toBeUndefined();
  });

  test("reads an idle thread's answer and settles the link with it", async () => {
    // The channel is already back when the request is found dead: the machine is asked at once.
    watchMachineConnection(LINUX, machineConnection({ status: "idle", reply: "Everything is green." }));
    lostTurn(THREAD, [LINK]);
    await until(() => link()?.state === "done");
    expect(link()?.answer).toBe("Everything is green.");
    expect(link()?.error).toBeUndefined();
  });

  test("settles failed with the error the turn ended in, not the connection's", async () => {
    // The machine is back and its thread is idle, but the turn there failed: its own words settle.
    watchMachineConnection(
      LINUX,
      machineConnection({ status: "idle", notice: "Claude Code: the turn ran out of context." }),
    );
    lostTurn(THREAD, [LINK]);
    await until(() => link()?.state === "failed");
    expect(link()?.error).toBe("Claude Code: the turn ran out of context.");
  });

  test("fails with the sentence for a lost machine when it never comes back", async () => {
    setLostMachineMsForTests(20);
    watchMachineConnection(LINUX, null);
    lostTurn(THREAD, [LINK]);
    await until(() => link()?.state === "failed");
    expect(link()?.error).toBe(MACHINE_LOST_TURN);
  });
});
