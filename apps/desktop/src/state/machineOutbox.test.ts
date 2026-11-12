// Settling and pinning a paired machine's thread while the machine is away: the change shows at
// once, waits in app storage, and goes to the machine when it lists its threads again, unless the
// machine changed the same thing after the user did (state/machineOutbox.ts, state/store.ts).

import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import type { SessionSummary } from "../acp/types";
import { V2Method } from "../acp/v2";
import { setShellTransport } from "../lib/shell";
import { type CacheStorage, saveThreadList, useCacheStorage } from "./machineCache";
import {
  changesFor,
  OUTBOX_KEY,
  type OutboxChange,
  queueChange,
  resolveChange,
  useOutbox,
  useOutboxStorage,
} from "./machineOutbox";
import {
  type Machine,
  refreshMachines,
  resetMachinesForTests as resetMachineLinksForTests,
  useMachines,
} from "./machines";
import {
  flushMachineOutbox,
  resetForTests,
  setPinned,
  settleThread,
  startMachineRouting,
  statusOf,
  unsettleThreads,
  useApp,
} from "./store";

const STUDIO = "gw-8e1f4b7a5c2d4e6f";
const ID = "mock-studio-1";
const KEY = `${STUDIO}/${ID}`;

class MemoryStorage implements CacheStorage {
  readonly map = new Map<string, string>();
  getItem = (key: string) => this.map.get(key) ?? null;
  setItem = (key: string, value: string) => void this.map.set(key, value);
  removeItem = (key: string) => void this.map.delete(key);
}

function row(id: string, updatedAt: number, extra: Partial<SessionSummary> = {}): SessionSummary {
  return { id, cwd: "/Users/lars/code/studio", title: `Thread ${id}`, updatedAt, ...extra };
}

type StatusChange = Extract<OutboxChange, { kind: "status" }>;
const settle = (at: number, base: StatusChange["base"] = "idle"): StatusChange => ({
  kind: "status",
  key: KEY,
  value: "done",
  base,
  at,
});

describe("what a waiting change does when its machine is back", () => {
  test("is sent when the machine still has what the user saw", () => {
    expect(resolveChange(settle(1_000), row(ID, 900))).toBe("send");
  });

  test("is sent when the machine changed it before the user did", () => {
    expect(resolveChange(settle(1_000), row(ID, 900, { status: "review" }))).toBe("send");
  });

  test("gives way when the machine changed it after the user did", () => {
    expect(resolveChange(settle(1_000), row(ID, 2_000, { status: "review" }))).toBe("machine");
    // A turn there after the settle is the machine's newer state too.
    expect(resolveChange(settle(1_000), row(ID, 2_000, { status: "running" }))).toBe("machine");
  });

  test("is not sent again when the machine already has it, or no longer lists the thread", () => {
    expect(resolveChange(settle(1_000), row(ID, 2_000, { status: "done" }))).toBe("done");
    expect(resolveChange(settle(1_000), undefined)).toBe("gone");
  });

  test("compares a pin the same way", () => {
    const pin: OutboxChange = { kind: "pin", key: KEY, value: true, base: false, at: 1_000 };
    expect(resolveChange(pin, row(ID, 2_000))).toBe("send");
    expect(resolveChange(pin, row(ID, 2_000, { pinned: true }))).toBe("done");
  });
});

describe("the outbox", () => {
  let disk: MemoryStorage;
  beforeEach(() => {
    disk = new MemoryStorage();
    useOutboxStorage(disk);
  });
  afterEach(() => useOutboxStorage(undefined));

  test("keeps one change per thread and kind, and a change back to the start cancels it", () => {
    queueChange(settle(1_000));
    queueChange({ ...settle(2_000, "done"), value: "review" });
    expect(changesFor(STUDIO)).toEqual([{ ...settle(2_000), value: "review" }]);
    queueChange({ ...settle(3_000, "review"), value: "idle" });
    expect(changesFor(STUDIO)).toEqual([]);
  });

  test("outlives the app: a new start reads what was waiting", () => {
    queueChange(settle(1_000));
    expect(disk.getItem(OUTBOX_KEY)).toContain(KEY);
    useOutbox.setState({ byHost: {} });
    useOutboxStorage(disk);
    expect(changesFor(STUDIO)).toEqual([settle(1_000)]);
  });
});

// ------------------------------------------------------------ the app

function machine(state: Machine["state"]): Machine {
  return {
    id: STUDIO,
    name: "Studio",
    platform: "macos",
    version: "1.31.4",
    routes: [{ kind: "lan", address: "192.168.1.31:17879", encrypted: false }],
    route: { kind: "lan", address: "192.168.1.31:17879", encrypted: false },
    state,
    detail: null,
    hint: null,
    retryInSecs: null,
    addedAt: "2026-11-02T09:00:00+00:00",
    lastConnectedAt: "2026-11-12T15:27:00+00:00",
  };
}

/** A shell whose one machine is away: its channel never opens. */
class AwayShell implements Transport {
  readonly name = "mock";
  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}
  async call(command: string): Promise<unknown> {
    if (command === "machines_list") return [machine("offline")];
    if (command === "machine_connect") return { connected: false };
    return null;
  }
}

/** A machine's live connection that records what it is asked and answers each with `{}`. */
function liveConnection(asked: JsonObject[]): AcpConnection {
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    asked.push(frame);
    queueMicrotask(() => {
      if (!conn.isClosed) conn.receive(JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: {} }));
    });
  });
  return conn;
}

const toasts = () => useApp.getState().toasts.map((t) => t.text);

describe("a thread on a machine that is away", () => {
  let disk: MemoryStorage;

  beforeEach(async () => {
    disk = new MemoryStorage();
    useCacheStorage(disk);
    useOutboxStorage(disk);
    resetForTests();
    resetMachineLinksForTests();
    useApp.setState((s) => ({
      selectedId: null,
      threads: {},
      toasts: [],
      pinned: [],
      features: { ...s.features, pins: true, status: true },
    }));
    saveThreadList(STUDIO, [row(ID, 1_000), row("mock-studio-2", 900, { status: "done" })]);
    setShellTransport(new AwayShell());
    await refreshMachines();
    await startMachineRouting();
  });

  afterEach(() => {
    useOutboxStorage(undefined);
    useCacheStorage(undefined);
  });

  afterAll(() => {
    resetForTests();
    resetMachineLinksForTests();
    setShellTransport(null);
  });

  test("keeps a settled thread settled from the copy, after a restart too", () => {
    expect(statusOf(useApp.getState(), `${STUDIO}/mock-studio-2`)).toBe("done");
  });

  test("settles at once, says it syncs later, and keeps the change for the machine", async () => {
    await settleThread(KEY);
    expect(statusOf(useApp.getState(), KEY)).toBe("done");
    expect(toasts().some((t) => t.includes("Syncs when Studio is back."))).toBe(true);
    expect(toasts().some((t) => t.startsWith("Could not"))).toBe(false);
    expect(changesFor(STUDIO)).toMatchObject([{ kind: "status", key: KEY, value: "done", base: "idle" }]);
    expect(disk.getItem(OUTBOX_KEY)).toContain(KEY);
  });

  test("an unsettle before the machine is back leaves nothing to send", async () => {
    await settleThread(KEY);
    unsettleThreads([KEY]);
    await Bun.sleep(0);
    expect(statusOf(useApp.getState(), KEY)).toBe("idle");
    expect(changesFor(STUDIO)).toEqual([]);
  });

  test("pins at once and keeps the pin for the machine", () => {
    setPinned([KEY], true);
    expect(useApp.getState().pinned).toContain(KEY);
    expect(changesFor(STUDIO)).toMatchObject([{ kind: "pin", key: KEY, value: true, base: false }]);
  });

  test("sends what waits, in order, when the machine lists its threads again", async () => {
    await settleThread(KEY);
    setPinned([KEY], true);
    const asked: JsonObject[] = [];
    await flushMachineOutbox(STUDIO, liveConnection(asked), [row(ID, 1_000)]);
    expect(asked.map((f) => [f.method, f.params])).toEqual([
      [V2Method.sessionStatus, { session_id: ID, status: "done" }],
      [V2Method.sessionPin, { session_id: ID, pinned: true }],
    ]);
    expect(changesFor(STUDIO)).toEqual([]);
    // The machine's list as this computer holds it says so until the machine's next read does.
    expect(useMachines.getState().sessions[STUDIO]?.find((r) => r.id === ID)).toMatchObject({
      status: "done",
      pinned: true,
    });
    expect(statusOf(useApp.getState(), KEY)).toBe("done");
    expect(toasts()).toContain("Synced 2 changes to Studio.");
  });

  test("keeps the machine's state when it changed the thread after the user did", async () => {
    await settleThread(KEY);
    const asked: JsonObject[] = [];
    await flushMachineOutbox(STUDIO, liveConnection(asked), [
      row(ID, Date.now() + 60_000, { status: "review" }),
    ]);
    expect(asked).toEqual([]);
    expect(changesFor(STUDIO)).toEqual([]);
    expect(toasts().some((t) => t.includes("newer state stands"))).toBe(true);
  });

  test("puts the row back and says so when the machine refuses a change", async () => {
    await settleThread(KEY);
    let conn!: AcpConnection;
    conn = new AcpConnection((line) => {
      const frame = JSON.parse(line) as JsonObject;
      if (typeof frame.id !== "number") return;
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
    await flushMachineOutbox(STUDIO, conn, [row(ID, 1_000)]);
    expect(changesFor(STUDIO)).toEqual([]);
    expect(statusOf(useApp.getState(), KEY)).toBe("idle");
    expect(toasts().some((t) => t.startsWith("Could not sync 1 change to Studio"))).toBe(true);
  });

  test("keeps a change for next time when the machine goes away again before it is sent", async () => {
    await settleThread(KEY);
    const conn = liveConnection([]);
    conn.close("away again");
    await flushMachineOutbox(STUDIO, conn, [row(ID, 1_000)]);
    expect(changesFor(STUDIO)).toHaveLength(1);
  });
});
