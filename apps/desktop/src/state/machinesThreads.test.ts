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
import { AcpConnection, type JsonObject, obj, str } from "../acp/jsonrpc";
import { MockMachine } from "../acp/mock";
import type { Transport } from "../acp/transport";
import type { PermissionRequest } from "../acp/types";
import { setShellTransport } from "../lib/shell";
import {
  forgetMachineThread,
  type Machine,
  machineConnection,
  refreshMachines,
  refreshMachineThreads,
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
  setSnoozed,
  setThreadsStatus,
  startMachineRouting,
  statusOf,
  threadIsRemote,
  threadOffline,
  useApp,
  watchMachineConnection,
} from "./store";
import { emptyThread } from "./thread";
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
    version: "1.28.0",
    main: false,
    routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
    state,
    detail: null,
    hint: null,
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

  test("a thread another client starts on a machine is listed here at once", async () => {
    const conn = machineConnection(LINUX);
    expect(conn).not.toBeNull();
    const before = useApp.getState().sessions.length;
    // The link listed threads when it initialized, so the machine's server names what any client
    // makes there (`_rustcode/threads/created`); this request is not `createSession`, so only the
    // announcement can list the row.
    const answer = (await conn?.request("session/new", { cwd: "/home/lars/code/site" })) as JsonObject;
    const id = str(obj(answer)?.sessionId);
    if (!id) throw new Error("the machine named no thread");
    await sleep(5);
    const row = useApp.getState().sessions.find((s) => s.id === `${LINUX}/${id}`);
    expect(row?.host).toBe(LINUX);
    expect(row?.cwd).toBe("/home/lars/code/site");
    expect(useApp.getState().sessions.length).toBe(before + 1);
    // The engine lists a session once it saved history: a read before its first turn keeps the
    // announced row.
    refreshMachineThreads();
    await sleep(20);
    expect(useMachines.getState().sessions[LINUX]?.some((s) => s.id === id)).toBe(true);
    // The app's own list survives the same read's mirror of the machine's list.
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(true);
    // Its first turn saves that history: the next read names it once, without the announcement.
    await conn?.request("session/prompt", {
      sessionId: id,
      prompt: [{ type: "text", text: "Keep it" }],
    });
    refreshMachineThreads();
    await sleep(20);
    const listed = (useMachines.getState().sessions[LINUX] ?? []).filter((s) => s.id === id);
    expect(listed).toHaveLength(1);
    expect(listed[0]?.title).toBe("Keep it");
    // Put the machine's list back for the tests after this one.
    const rest = (useMachines.getState().sessions[LINUX] ?? []).filter((s) => s.id !== id);
    useMachines.setState((state) => ({ sessions: { ...state.sessions, [LINUX]: rest } }));
  });

  test("a thread trashed on a machine leaves both lists", async () => {
    const conn = machineConnection(LINUX);
    const answer = (await conn?.request("session/new", { cwd: "/home/lars/code/site" })) as JsonObject;
    const id = str(obj(answer)?.sessionId);
    if (!id) throw new Error("the machine named no thread");
    await sleep(5);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(true);
    // The machine's app trashes it: `_rustcode/threads/deleted` names it to every client.
    await conn?.request("_rustcode/threads/delete", { sessionId: id });
    await sleep(5);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(false);
    expect(useMachines.getState().sessions[LINUX]?.some((s) => s.id === id)).toBe(false);
    // A read after it names nothing, and the announcement does not bring the row back.
    refreshMachineThreads();
    await sleep(20);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(false);
  });

  test("a machine thread trashed while its link was down is gone from both lists, and stays gone", async () => {
    const conn = machineConnection(LINUX);
    const answer = (await conn?.request("session/new", { cwd: "/home/lars/code/site" })) as JsonObject;
    const id = str(obj(answer)?.sessionId);
    if (!id) throw new Error("the machine named no thread");
    await sleep(5);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(true);
    // The window trashed it with no link, so the machine never said `_rustcode/threads/deleted`.
    forgetMachineThread(LINUX, id);
    expect(useMachines.getState().sessions[LINUX]?.some((s) => s.id === id)).toBe(false);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(false);
    // A read that was in flight, or a late announcement, does not bring it back.
    refreshMachineThreads();
    await sleep(20);
    expect(useMachines.getState().sessions[LINUX]?.some((s) => s.id === id)).toBe(false);
    expect(useApp.getState().sessions.some((s) => s.id === `${LINUX}/${id}`)).toBe(false);
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

  test("an unsupported remote method cannot leave a local status mark", async () => {
    let conn!: AcpConnection;
    conn = new AcpConnection((line) => {
      const frame = JSON.parse(line) as JsonObject;
      queueMicrotask(() =>
        conn.receive(
          JSON.stringify({
            jsonrpc: "2.0",
            id: frame.id,
            error: { code: -32601, message: "Method not found" },
          }),
        ),
      );
    });
    watchMachineConnection(LINUX, conn);
    const before = statusOf(useApp.getState(), LINUX_THREAD);
    expect(await setThreadsStatus([LINUX_THREAD], "done")).toBe(1);
    expect(statusOf(useApp.getState(), LINUX_THREAD)).toBe(before);
    expect(useApp.getState().statusMarks[LINUX_THREAD]).toBeUndefined();
  });

  test("owner broadcasts update unopened remote threads", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    for (const status of ["done", "idle", "review"] as const) {
      conn.receive(
        JSON.stringify({
          jsonrpc: "2.0",
          method: "session/update",
          params: {
            sessionId: "mock-linux-1",
            update: {
              sessionUpdate: "session_info_update",
              _meta: { rustcode: { status, snoozedUntil: null } },
            },
          },
        }),
      );
      await sleep(5);
      expect(statusOf(useApp.getState(), LINUX_THREAD)).toBe(status);
    }
  });

  test("offline status and snooze refuse without hiding the thread", async () => {
    watchMachineConnection(LINUX, null);
    const before = statusOf(useApp.getState(), LINUX_THREAD);
    expect(await setThreadsStatus([LINUX_THREAD], "done")).toBe(1);
    expect(statusOf(useApp.getState(), LINUX_THREAD)).toBe(before);
    expect(await setSnoozed(LINUX_THREAD, Date.now() + 60_000)).toBe(false);
    expect(useApp.getState().snoozed[LINUX_THREAD]).toBeUndefined();
  });

  test("snooze reaches the owner with the raw id", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    expect(await setSnoozed(LINUX_THREAD, 123456)).toBe(true);
    const call = sent().find((frame) => frame.method === "_rustcode/session/snooze");
    expect(call?.params).toEqual({ session_id: "mock-linux-1", until: 123456 });
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

  test("an answer on a connected machine goes back on its connection", async () => {
    const { conn, sent } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    await openThread(LINUX_THREAD);

    conn.receive(permissionFrame(77, "mock-linux-1"));
    await sleep(5);
    const request = useApp.getState().threads[LINUX_THREAD]?.permissions[0];
    expect(request?.toolCallId).toBe("call-1");
    expect(request?.sessionId).toBe(LINUX_THREAD);

    answerPermission(request!, request!.options[0]!);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(0);
    expect(sent().some((frame) => frame.id === 77 && frame.result !== undefined)).toBe(true);
  });

  test("a drop ends the turn waiting on an approval, and its card with it", async () => {
    const { conn } = recordingConnection();
    watchMachineConnection(LINUX, conn);
    await openThread(LINUX_THREAD);
    // The turn the engine asked for approval on.
    useApp.setState((s) => ({
      threads: {
        ...s.threads,
        [LINUX_THREAD]: {
          ...s.threads[LINUX_THREAD]!,
          running: true,
          turnStartedAt: Date.now() - 1000,
        },
      },
    }));
    conn.receive(permissionFrame(78, "mock-linux-1"));
    await sleep(5);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(1);

    watchMachineConnection(LINUX, null);
    const thread = useApp.getState().threads[LINUX_THREAD];
    // The connection the request arrived on is gone, so the turn ends and the card goes with it,
    // saying why, rather than holding an answer nothing can carry.
    expect(thread?.running).toBe(false);
    expect(thread?.permissions).toHaveLength(0);
    expect(thread?.error).toContain("machine disconnected");
    expect(threadOffline(LINUX_THREAD)).toBe(true);
  });

  test("an answer with no connection is refused and keeps the card", () => {
    // The guard's own state: a request on a thread whose machine is away (a frame that arrived in
    // the moment before the drop clears it).
    watchMachineConnection(LINUX, null);
    asListed();
    const request: PermissionRequest = {
      rpcId: 79,
      sessionId: LINUX_THREAD,
      toolCallId: "call-2",
      title: "read src/ui/Composer.tsx",
      options: [{ id: "allow", name: "Allow once", kind: "allow_once" }],
      diffs: [],
    };
    useApp.setState((s) => ({
      threads: {
        ...s.threads,
        [LINUX_THREAD]: {
          ...emptyThread(LINUX_THREAD, LINUX_CWD, "Run the tests"),
          loaded: true,
          permissions: [request],
        },
      },
    }));
    answerPermission(request, request.options[0]!);
    expect(useApp.getState().threads[LINUX_THREAD]?.permissions).toHaveLength(1);
  });
});
