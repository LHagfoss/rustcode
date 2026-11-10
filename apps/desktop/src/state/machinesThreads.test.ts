// A thread on a paired computer, keyed by (machine, thread) and driven over
// that machine's own connection: the routing `state/store.ts` `connectionFor`
// does, the keys `state/threadKeys.ts` makes, and the read-only state a thread
// keeps while its machine is away. The shell and the machine are stand-ins for
// the app's own (`src-tauri/src/machines.rs`) and the mock's ACP.
//
// Everything this file attaches is process-global (the shell transport, the
// machines module's links, the store's machine map), so `afterAll` resets all
// of it: another test file's engine must never see these machines.

import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import { MockMachine } from "../acp/mock";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import {
  type Machine,
  machineConnection,
  refreshMachines,
  resetMachinesForTests as resetMachineLinksForTests,
  useMachines,
} from "./machines";
import {
  answerPermission,
  connectionFor,
  connectionForThread,
  machineOfThread,
  openThread,
  resetForTests,
  setPinned,
  setThreadsStatus,
  startMachineRouting,
  threadIsRemote,
  threadOffline,
  useApp,
  watchMachineConnection,
} from "./store";
import { LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const STUDIO = "gw-8e1f4b7a5c2d4e6f";
const LINUX_THREAD = `${LINUX}/mock-linux-1`;
const LINUX_CWD = "/home/lars/code/rustcode";

function machine(id: string, name: string, state: Machine["state"] = "connected"): Machine {
  return {
    id,
    name,
    platform: "linux",
    main: false,
    routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
    state,
    detail: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: "2026-11-08T09:00:00+00:00",
  };
}

/** The app's shell, answering the commands a machine's supervisor needs, without one. */
class FakeShell implements Transport {
  readonly name = "mock";
  machines: Machine[] = [machine(LINUX, "Lars's Linux box"), machine(STUDIO, "Studio")];
  calls: { command: string; args: Record<string, unknown> }[] = [];

  async start(): Promise<void> {}

  send(): void {}

  async restart(): Promise<void> {}

  async stop(): Promise<void> {}

  async call(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
    this.calls.push({ command, args });
    if (command === "machines_list") return this.machines;
    if (command === "machine_connect") return { connected: true };
    if (command === "machine_send" || command === "machine_disconnect") return null;
    return null;
  }
}

const sleep = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

let shell: FakeShell | null = null;

beforeAll(async () => {
  // Nothing a previous file left behind: its machines, its shell, its engine.
  resetForTests();
  shell = new FakeShell();
  setShellTransport(shell);
  await refreshMachines();
  await startMachineRouting();
  // The mock machine answers `session/list` with three threads once its channel is open.
  await sleep(50);
});

afterAll(() => {
  // Drops the machines' links, the store's machine map, its sessions and the shell: the next
  // file's `connect()` starts from nothing (state/store.ts `resetForTests`).
  resetForTests();
  resetMachineLinksForTests();
  setShellTransport(null);
  shell = null;
});

/** A thread on a machine, as the list the machine gave the store has it. */
function asListed(): void {
  useApp.setState({
    selectedId: null,
    threads: {},
    sessions: [
      {
        id: LINUX_THREAD,
        host: LINUX,
        cwd: LINUX_CWD,
        title: "Run the tests",
        updatedAt: Date.now(),
        harness: "claude",
        model: "sonnet-5.5",
      },
    ],
  });
}

/**
 * A connection whose writes the test can read, answering every request so the
 * store's calls settle. What a remote thread's operation sends is the test.
 */
function recordingConnection(): { conn: AcpConnection; sent: () => JsonObject[] } {
  const writes: string[] = [];
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    writes.push(line);
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    queueMicrotask(() => {
      if (conn.isClosed) return;
      conn.receive(
        JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { configOptions: [], sessions: [] } }),
      );
    });
  });
  return {
    conn,
    sent: () => writes.map((line) => JSON.parse(line) as JsonObject),
  };
}

function permissionFrame(id: number, sessionId: string): string {
  return JSON.stringify({
    jsonrpc: "2.0",
    id,
    method: "session/request_permission",
    params: {
      sessionId,
      toolCall: { toolCallId: "call-1", title: "read src/ui/Composer.tsx", kind: "read" },
      options: [
        { optionId: "allow", name: "Allow once", kind: "allow_once" },
        { optionId: "deny", name: "Decline", kind: "reject_once" },
      ],
    },
  });
}

describe("thread keys", () => {
  test("a local thread keeps its bare id, and a remote one names its machine", () => {
    expect(threadKey(LOCAL_HOST, "9f1c-2d3e")).toBe("9f1c-2d3e");
    expect(threadKey(LINUX, "9f1c-2d3e")).toBe(`${LINUX}/9f1c-2d3e`);
    expect(splitThreadKey("9f1c-2d3e")).toEqual({ host: LOCAL_HOST, id: "9f1c-2d3e" });
    expect(splitThreadKey(`${LINUX}/9f1c-2d3e`)).toEqual({ host: LINUX, id: "9f1c-2d3e" });
  });

  test("only a key with a machine in front of it is remote", () => {
    expect(threadIsRemote("9f1c-2d3e")).toBe(false);
    expect(threadIsRemote(`${LINUX}/9f1c-2d3e`)).toBe(true);
    // A machine's own session id may hold a dash but never a slash (`state/threadKeys.ts`).
    expect(splitThreadKey("mock-linux-1").id).toBe("mock-linux-1");
  });
});

describe("a machine's threads", () => {
  test("its own connection is the one its threads route to", () => {
    const conn = machineConnection(LINUX);
    expect(conn).not.toBeNull();
    expect(connectionFor(LINUX)).toBe(conn);
    expect(connectionForThread(LINUX_THREAD)).toBe(conn);
    // This computer's threads stay on whatever local engine this process has (another test file
    // may have started one), never on a machine's.
    expect(connectionForThread("mock-linux-1")).toBe(connectionFor(LOCAL_HOST));
    expect(connectionFor(LOCAL_HOST)).not.toBe(conn);
    expect(threadOffline(LINUX_THREAD)).toBe(false);
    expect(machineOfThread(LINUX_THREAD)).toBe("Lars's Linux box");
  });

  test("its list arrives keyed by the machine, next to this computer's threads", () => {
    const sessions = useApp.getState().sessions;
    const linux = sessions.filter((s) => s.host === LINUX);
    expect(linux.map((s) => s.id)).toContain(LINUX_THREAD);
    expect(linux[0]?.host).toBe(LINUX);
    // The machine's own id on the machine, not the key, is what it lists.
    expect(linux.some((s) => s.id === "mock-linux-1")).toBe(false);
  });

  test("a rebuilt list replaces that machine's threads and leaves the others alone", () => {
    const before = useMachines.getState().sessions[LINUX] ?? [];
    expect(before).toHaveLength(3);
    // The machine reports one thread fewer, as it would after a settle.
    useMachines.setState((state) => ({
      sessions: { ...state.sessions, [LINUX]: before.slice(0, 2) },
    }));
    const remote = useApp.getState().sessions.filter((s) => s.host === LINUX);
    expect(remote.map((s) => s.id)).toEqual([LINUX_THREAD, `${LINUX}/mock-linux-2`]);
    expect(remote.some((s) => s.id === `${LINUX}/mock-linux-3`)).toBe(false);
    expect(useApp.getState().sessions.some((s) => s.host === STUDIO)).toBe(true);
    // Put the machine's list back for the tests after this one.
    useMachines.setState((state) => ({ sessions: { ...state.sessions, [LINUX]: before } }));
  });

  test("another machine's request is answered on that machine's connection", () => {
    // The connection the store listens on is the machine's own: what arrives there is routed by
    // `connectionForThread` when the answer goes back (state/store.ts `reply`).
    const conn = machineConnection(LINUX);
    expect(connectionForThread(LINUX_THREAD)).toBe(conn);
    expect(connectionForThread(`${STUDIO}/mock-studio-1`)).toBe(machineConnection(STUDIO));
    expect(connectionFor(LINUX)).not.toBe(connectionFor(STUDIO));
  });

  test("a machine that drops keeps its threads listed, read-only", () => {
    watchMachineConnection(LINUX, null);
    // Nothing to send on, so the thread is read-only; its row is still there.
    expect(connectionForThread(LINUX_THREAD)).toBeNull();
    expect(threadOffline(LINUX_THREAD)).toBe(true);
    expect(useApp.getState().sessions.some((s) => s.id === LINUX_THREAD)).toBe(true);
  });

  test("a machine that comes back is routed again", () => {
    const conn = machineConnection(LINUX);
    expect(conn).not.toBeNull();
    watchMachineConnection(LINUX, conn);
    expect(connectionForThread(LINUX_THREAD)).toBe(conn);
    expect(threadOffline(LINUX_THREAD)).toBe(false);
  });

  test("the mock machine answers a thread open and a turn on it", () => {
    const mock = new MockMachine("Studio");
    const loaded = mock.receive(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "session/load",
        params: { sessionId: "mock-studio-1" },
      }),
    );
    expect(loaded).toHaveLength(3);
    expect(loaded[0]).toContain("mock-studio-1");
    const prompted = mock.receive(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 2,
        method: "session/prompt",
        params: { sessionId: "mock-studio-1", prompt: [] },
      }),
    );
    expect(prompted.at(-1)).toContain("end_turn");
  });

  test("a machine speaking is not the local engine", () => {
    const remote = new AcpConnection(() => undefined);
    const local = connectionFor(LOCAL_HOST);
    watchMachineConnection(LINUX, remote);
    expect(connectionFor(LINUX)).toBe(remote);
    expect(connectionFor(LOCAL_HOST)).toBe(local);
    watchMachineConnection(LINUX, null);
    expect(connectionFor(LINUX)).toBeNull();
    // The local engine is untouched by a machine's channel closing.
    expect(connectionFor(LOCAL_HOST)).toBe(local);
    expect(remote.isClosed).toBe(false);
  });
});

describe("a remote thread's operations", () => {
  beforeEach(() => {
    // These tests drive a connection of their own; the machine's real one is put back by the
    // next test through `watchMachineConnection`, and the file's end resets everything.
    watchMachineConnection(LINUX, null);
    asListed();
    useApp.setState((s) => ({ features: { ...s.features, pins: true, status: true } }));
  });

  test("load, pin and status go over the machine's connection with its raw session id", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);

    await openThread(LINUX_THREAD);
    await setPinned([LINUX_THREAD], true);
    await setThreadsStatus([LINUX_THREAD], "review");

    const calls = sent().filter((frame) => typeof frame.method === "string");
    const params = (method: string) =>
      (calls.find((frame) => frame.method === method)?.params ?? {}) as JsonObject;
    expect(params("session/load").sessionId).toBe("mock-linux-1");
    expect(params("_rustcode/session/pin").session_id).toBe("mock-linux-1");
    expect(params("_rustcode/session/status").session_id).toBe("mock-linux-1");
    // A key is this window's name for a thread; the machine only knows its own id.
    expect(JSON.stringify(calls)).not.toContain(LINUX);
  });

  test("a thread on an away machine still opens, read-only", async () => {
    watchMachineConnection(LINUX, null);
    await openThread(LINUX_THREAD);
    const thread = useApp.getState().threads[LINUX_THREAD];
    expect(thread).toBeDefined();
    expect(thread?.loaded).toBe(false);
    expect(thread?.cwd).toBe(LINUX_CWD);
    expect(thread?.title).toBe("Run the tests");
    expect(threadOffline(LINUX_THREAD)).toBe(true);
    expect(machineOfThread(LINUX_THREAD)).toBe("Lars's Linux box");
    // Opening again asks the machine once it is back, and opens this state meanwhile.
    await openThread(LINUX_THREAD);
    expect(useApp.getState().threads[LINUX_THREAD]?.loaded).toBe(false);
  });

  test("a machine's delegation is refused instead of starting a thread here", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    conn.receive(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 88,
        method: "_rustcode/orchestration",
        params: { call: { action: "start_session", thread_id: "mock-linux-1" } },
      }),
    );
    await sleep(5);
    const answer = sent().find((frame) => frame.id === 88);
    expect(JSON.stringify(answer)).toContain("not_allowed");
    // Nothing of this computer's was started for it.
    expect(useApp.getState().threads[LINUX_THREAD]?.running ?? false).toBe(false);
  });

  test("an approval waits for the machine to come back, then answers on its connection", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    await openThread(LINUX_THREAD);

    conn.receive(permissionFrame(77, "mock-linux-1"));
    await sleep(5);
    const request = useApp.getState().threads[LINUX_THREAD]?.permissions[0];
    expect(request?.toolCallId).toBe("call-1");
    expect(request?.sessionId).toBe(LINUX_THREAD);

    // The machine goes away while the turn waits: the card stays, and an answer is refused.
    watchMachineConnection(LINUX, null);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(1);
    answerPermission(request!, request!.options[0]!);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(1);
    expect(sent().some((frame) => frame.id === 77 && frame.result !== undefined)).toBe(false);

    // Back: the request was not thrown away, and the answer reaches the machine.
    watchMachineConnection(LINUX, conn);
    const again = useApp.getState().threads[LINUX_THREAD]?.permissions[0];
    expect(again).toBeDefined();
    answerPermission(again!, again!.options[0]!);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(0);
    expect(sent().some((frame) => frame.id === 77 && frame.result !== undefined)).toBe(true);
  });
});
