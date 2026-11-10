// A thread on a paired computer, keyed by (machine, thread) and driven over
// that machine's own connection: the routing `state/store.ts` `connectionFor`
// does, the keys `state/threadKeys.ts` makes, and the read-only state a thread
// keeps while its machine is away. The shell and the machine are stand-ins for
// the app's own (`src-tauri/src/machines.rs`) and the mock's ACP.

import { beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection } from "../acp/jsonrpc";
import { MockMachine } from "../acp/mock";
import type { Transport } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import { type Machine, machineConnection, refreshMachines, useMachines } from "./machines";
import {
  connectionFor,
  connectionForThread,
  machineOfThread,
  startMachineRouting,
  threadIsRemote,
  threadOffline,
  useApp,
  watchMachineConnection,
} from "./store";
import { LOCAL_HOST, splitThreadKey, threadKey } from "./threadKeys";

const LINUX = "gw-7c1d9e4f2a8b6d30";
const STUDIO = "gw-8e1f4b7a5c2d4e6f";

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

// The machines module keeps its links for the life of the process, so the shell is set up once:
// each test reads the state that one attach made, and none of them wipes it under another.
let booted: Promise<void> | null = null;
function boot(): Promise<void> {
  booted ??= (async () => {
    setShellTransport(new FakeShell());
    await refreshMachines();
    await startMachineRouting();
    await sleep(50);
  })();
  return booted;
}

beforeEach(async () => {
  await boot();
  // Only the selection is per test: the machines and their threads are the attach's.
  useApp.setState({ selectedId: null });
});

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
  test("its own connection is the one its threads route to", async () => {
    const conn = machineConnection(LINUX);
    expect(conn).not.toBeNull();
    expect(connectionFor(LINUX)).toBe(conn);
    expect(connectionForThread(`${LINUX}/mock-linux-1`)).toBe(conn);
    // This computer's threads stay on whatever local engine this process has (another test file
    // may have started one), never on a machine's.
    expect(connectionForThread("mock-linux-1")).toBe(connectionFor(LOCAL_HOST));
    expect(connectionFor(LOCAL_HOST)).not.toBe(conn);
    expect(threadOffline(`${LINUX}/mock-linux-1`)).toBe(false);
    expect(machineOfThread(`${LINUX}/mock-linux-1`)).toBe("Lars's Linux box");
  });

  test("its list arrives keyed by the machine, next to this computer's threads", async () => {
    // The mock machine answers `session/list` with three threads of its own.
    const sessions = useApp.getState().sessions;
    const linux = sessions.filter((s) => s.host === LINUX);
    expect(linux.map((s) => s.id)).toContain(`${LINUX}/mock-linux-1`);
    expect(linux[0]?.host).toBe(LINUX);
    // The machine's own id on the machine, not the key, is what it lists.
    expect(linux.some((s) => s.id === "mock-linux-1")).toBe(false);
  });

  test("a rebuilt list replaces that machine's threads and leaves the others alone", async () => {
    const before = useApp.getState().sessions.filter((s) => s.host === STUDIO).length;

    // The machine reports one thread fewer, as it would after a settle.
    useMachines.setState((state) => ({
      sessions: {
        ...state.sessions,
        [STUDIO]: (state.sessions[STUDIO] ?? []).slice(0, 2),
      },
    }));
    const after = useApp.getState().sessions.filter((s) => s.host === STUDIO);
    expect(after).toHaveLength(Math.min(2, before));
    expect(useApp.getState().sessions.some((s) => s.host === LINUX)).toBe(true);
  });

  test("another machine's request is answered on that machine's connection", async () => {
    // The connection the store listens on is the machine's own: what arrives there is routed by
    // `connectionForThread` when the answer goes back (state/store.ts `reply`).
    const conn = machineConnection(LINUX);
    expect(connectionForThread(`${LINUX}/mock-linux-1`)).toBe(conn);
    expect(connectionForThread(`${STUDIO}/mock-studio-1`)).toBe(machineConnection(STUDIO));
    expect(connectionFor(LINUX)).not.toBe(connectionFor(STUDIO));
  });

  test("a machine that drops keeps its threads listed, read-only", async () => {
    const key = `${LINUX}/mock-linux-1`;
    expect(connectionForThread(key)).not.toBeNull();

    watchMachineConnection(LINUX, null);
    // Nothing to send on, so the thread is read-only; its row is still there.
    expect(connectionForThread(key)).toBeNull();
    expect(threadOffline(key)).toBe(true);
    expect(useApp.getState().sessions.some((s) => s.id === key)).toBe(true);
  });

  test("a machine that comes back is routed again", async () => {
    const conn = machineConnection(LINUX);
    expect(conn).not.toBeNull();
    watchMachineConnection(LINUX, conn);
    expect(connectionForThread(`${LINUX}/mock-linux-1`)).toBe(conn);
    expect(threadOffline(`${LINUX}/mock-linux-1`)).toBe(false);
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
