// The two windows #3912 names in a paired machine's link, driven through the module's own
// entry points: a machine removed while its channel is still opening, and an `initialize`
// answer from a connection that a reconnect has since replaced. The shell is a stand-in whose
// `machine_connect` the test releases, and the machine answers nothing on its own: the test
// feeds each connection the lines it wants (`receive`).

import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection } from "../acp/jsonrpc";
import { MachineTransport, type Transport, type TransportEvent } from "../acp/transport";
import { setShellTransport } from "../lib/shell";
import {
  type Machine,
  machineConnection,
  reconnectMachine,
  refreshMachines,
  refreshMachineThreads,
  resetMachinesForTests,
  useMachines,
} from "./machines";

const BOX = "gw-3912aaaabbbbcccc";

function machine(id: string): Machine {
  return {
    id,
    name: "Box",
    platform: "linux",
    version: "1.28.0",
    routes: [],
    route: null,
    state: "offline",
    detail: null,
    hint: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: null,
  };
}

/** A shell whose `machine_connect` answers when the test says so. Not "mock": no canned machine answers. */
class HeldShell implements Transport {
  readonly name = "websocket";
  machines: Machine[] = [machine(BOX)];
  private held: (() => void)[] = [];
  hold = false;

  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}

  get waiting(): number {
    return this.held.length;
  }

  release(): void {
    for (const answer of this.held.splice(0)) answer();
  }

  async call(command: string): Promise<unknown> {
    if (command === "machines_list") return this.machines;
    if (command === "machine_connect") {
      if (!this.hold) return { connected: true };
      return new Promise((resolve) => this.held.push(() => resolve({ connected: true })));
    }
    return null;
  }
}

/** Every request a machine connection sends, with the connection it went on. */
const asked: { connection: AcpConnection; method: string }[] = [];
const request = AcpConnection.prototype.request;

const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

let shell: HeldShell;

beforeEach(() => {
  resetMachinesForTests();
  shell = new HeldShell();
  setShellTransport(shell);
  asked.length = 0;
  AcpConnection.prototype.request = function (this: AcpConnection, method, params, timeoutMs) {
    asked.push({ connection: this, method });
    return request.call(this, method, params, timeoutMs);
  } as typeof request;
});

afterEach(() => {
  AcpConnection.prototype.request = request;
  resetMachinesForTests();
});

afterAll(() => setShellTransport(null));

function answer(connection: AcpConnection, method: string): void {
  const index = asked
    .filter((entry) => entry.connection === connection)
    .findIndex((entry) => entry.method === method);
  // Requests are numbered from 1 per connection, in the order they were sent.
  connection.receive(JSON.stringify({ jsonrpc: "2.0", id: index + 1, result: { sessions: [] } }));
}

describe("a machine removed while its channel opens", () => {
  test("keeps no link once the channel answers", async () => {
    shell.hold = true;
    await refreshMachines();
    await tick();
    expect(shell.waiting).toBe(1);
    // Removed while `machine_connect` is still out.
    shell.machines = [];
    await refreshMachines();
    shell.release();
    await tick();
    expect(machineConnection(BOX)).toBeNull();
    // And a later read that lists it again starts it afresh.
    shell.hold = false;
    shell.machines = [machine(BOX)];
    await refreshMachines();
    await tick();
    expect(machineConnection(BOX)).not.toBeNull();
  });
});

describe("an initialize answer from a connection a reconnect replaced", () => {
  test("does not mark the new connection initialized", async () => {
    await refreshMachines();
    await tick();
    const first = machineConnection(BOX);
    expect(first).not.toBeNull();
    if (!first) return;
    expect(asked.filter((entry) => entry.connection === first).map((entry) => entry.method)).toEqual([
      "initialize",
    ]);
    // The answer arrives, and the reconnect closes that connection before the read resumes.
    answer(first, "initialize");
    reconnectMachine({ id: BOX });
    await tick();
    const second = machineConnection(BOX);
    expect(second).not.toBeNull();
    expect(second).not.toBe(first);
    if (!second) return;
    // A re-list (a focus) on the new connection still asks `initialize` until it is answered there.
    refreshMachineThreads();
    await tick();
    const methods = asked.filter((entry) => entry.connection === second).map((entry) => entry.method);
    expect(methods).toEqual(["initialize", "initialize"]);
    answer(second, "initialize");
    await tick();
    refreshMachineThreads();
    await tick();
    expect(asked.filter((entry) => entry.connection === second).map((entry) => entry.method)).toEqual([
      "initialize",
      "initialize",
      "session/list",
      "session/list",
    ]);
  });
});

describe("offlineSince tracks link open and closed events", () => {
  test("link open clears stale offlineSince and a subsequent drop records the new time", async () => {
    const originalStart = MachineTransport.prototype.start;
    const handlers = new Map<string, (event: TransportEvent) => void>();
    MachineTransport.prototype.start = async function (this: MachineTransport, onEvent) {
      handlers.set(this.machineId, onEvent);
    };
    try {
      const stale = Date.parse("2026-11-08T09:00:00Z");
      shell.machines = [{ ...machine(BOX), state: "offline", lastConnectedAt: "2026-11-08T09:00:00Z" }];
      await refreshMachines();
      await tick();
      expect(useMachines.getState().offlineSince[BOX]).toBe(stale);

      const emit = handlers.get(BOX);
      expect(emit).toBeDefined();
      if (!emit) return;

      emit({ type: "open", engine: "Box" });
      expect(useMachines.getState().machines[0]?.state).toBe("connected");
      expect(useMachines.getState().offlineSince[BOX]).toBeUndefined();

      const beforeDrop = Date.now();
      emit({ type: "closed", reason: "connection reset" });
      const firstDrop = useMachines.getState().offlineSince[BOX];
      expect(useMachines.getState().machines[0]?.state).toBe("offline");
      expect(firstDrop).toBeDefined();
      expect(firstDrop ?? 0).toBeGreaterThanOrEqual(beforeDrop);
      expect(firstDrop).not.toBe(stale);

      await tick(15);
      emit({ type: "closed", reason: "still away" });
      expect(useMachines.getState().offlineSince[BOX]).toBe(firstDrop);

      emit({ type: "open", engine: "Box" });
      expect(useMachines.getState().machines[0]?.state).toBe("connected");
      expect(useMachines.getState().offlineSince[BOX]).toBeUndefined();
    } finally {
      MachineTransport.prototype.start = originalStart;
    }
  });

  test("closed event while already offline keeps the existing offlineSince timestamp", async () => {
    const originalStart = MachineTransport.prototype.start;
    const handlers = new Map<string, (event: TransportEvent) => void>();
    MachineTransport.prototype.start = async function (this: MachineTransport, onEvent) {
      handlers.set(this.machineId, onEvent);
    };
    try {
      const stale = Date.parse("2026-11-08T09:00:00Z");
      shell.machines = [{ ...machine(BOX), state: "offline", lastConnectedAt: "2026-11-08T09:00:00Z" }];
      await refreshMachines();
      await tick();
      expect(useMachines.getState().offlineSince[BOX]).toBe(stale);

      const emit = handlers.get(BOX);
      expect(emit).toBeDefined();
      if (!emit) return;

      emit({ type: "closed", reason: "retry failed" });
      expect(useMachines.getState().machines[0]?.state).toBe("offline");
      expect(useMachines.getState().offlineSince[BOX]).toBe(stale);
    } finally {
      MachineTransport.prototype.start = originalStart;
    }
  });
});
