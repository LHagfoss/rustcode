// The copy this computer keeps of a paired machine's threads: its round trip through storage, the
// bounds that keep it small, and what the sidebar and the chat do with it while the machine is
// away (state/machineCache.ts, state/machines.ts, state/store.ts).

import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";
import { AcpConnection, type JsonObject } from "../acp/jsonrpc";
import type { Transport } from "../acp/transport";
import type { SessionSummary } from "../acp/types";
import { setShellTransport } from "../lib/shell";
import {
  type CacheStorage,
  forgetMachineCache,
  forgetTranscript,
  LIST_KEY_PREFIX,
  loadThreadList,
  loadTranscript,
  MAX_CACHED_THREADS,
  MAX_TRANSCRIPT_CHARS,
  MAX_TRANSCRIPTS_CHARS,
  offlineNotice,
  saveThreadList,
  saveTranscript,
  TRANSCRIPT_MAX_AGE_MS,
  threadFromTranscript,
  useCacheStorage,
} from "./machineCache";
import {
  type Machine,
  refreshMachines,
  resetMachinesForTests as resetMachineLinksForTests,
  useMachines,
} from "./machines";
import {
  openThread,
  resetForTests,
  startMachineRouting,
  threadOffline,
  useApp,
  watchMachineConnection,
} from "./store";
import { emptyThread, type ThreadState, type TranscriptItem } from "./thread";

const STUDIO = "gw-8e1f4b7a5c2d4e6f";
const LINUX = "gw-7c1d9e4f2a8b6d30";
const KEY = `${STUDIO}/mock-studio-1`;

class MemoryStorage implements CacheStorage {
  readonly map = new Map<string, string>();
  getItem = (key: string) => this.map.get(key) ?? null;
  setItem = (key: string, value: string) => void this.map.set(key, value);
  removeItem = (key: string) => void this.map.delete(key);
}

let disk: MemoryStorage;

function row(id: string, updatedAt: number, extra: Partial<SessionSummary> = {}): SessionSummary {
  return { id, cwd: "/Users/lars/code/studio", title: `Thread ${id}`, updatedAt, ...extra };
}

function thread(key: string, items: TranscriptItem[]): ThreadState {
  return { ...emptyThread(key, "/Users/lars/code/studio", "Fix the build"), items, loaded: true };
}

const said = (id: string, text: string): TranscriptItem => ({ kind: "assistant", id, text });
const asked = (id: string, text: string): TranscriptItem => ({ kind: "user", id, text });

beforeEach(() => {
  disk = new MemoryStorage();
  useCacheStorage(disk);
});

afterEach(() => {
  useCacheStorage(undefined);
});

describe("the thread list kept for a machine", () => {
  test("round-trips, newest first, without what is only true while the machine answers", () => {
    saveThreadList(STUDIO, [
      row("old", 100),
      row("new", 300, { status: "running", host: STUDIO }),
      row("mid", 200),
    ]);
    const back = loadThreadList(STUDIO);
    expect(back.map((r) => r.id)).toEqual(["new", "mid", "old"]);
    expect(back[0]?.status).toBeUndefined();
    expect(back[0]?.host).toBeUndefined();
    expect(back[0]?.title).toBe("Thread new");
  });

  test("keeps the 200 newest threads, and a newer list replaces the older one", () => {
    saveThreadList(
      STUDIO,
      Array.from({ length: MAX_CACHED_THREADS + 30 }, (_, i) => row(`t${i}`, i)),
    );
    const back = loadThreadList(STUDIO);
    expect(back).toHaveLength(MAX_CACHED_THREADS);
    expect(back[0]?.id).toBe(`t${MAX_CACHED_THREADS + 29}`);
    saveThreadList(STUDIO, [row("only", 1)]);
    expect(loadThreadList(STUDIO).map((r) => r.id)).toEqual(["only"]);
  });

  test("one machine's list is never another's, and bad storage reads as nothing", () => {
    saveThreadList(STUDIO, [row("a", 1)]);
    expect(loadThreadList(LINUX)).toEqual([]);
    disk.setItem(LIST_KEY_PREFIX + LINUX, "not json");
    expect(loadThreadList(LINUX)).toEqual([]);
  });
});

describe("the transcripts kept", () => {
  test("round-trips the rows, the title and the folder of a thread", () => {
    const items = [asked("u1", "why does it fail?"), said("a1", "A missing import.")];
    expect(saveTranscript(thread(KEY, items), KEY, 1_000)).toBe(true);
    const back = loadTranscript(KEY, 2_000);
    expect(back?.items).toEqual(items);
    expect(back?.title).toBe("Fix the build");
    expect(back?.cwd).toBe("/Users/lars/code/studio");
    const restored = threadFromTranscript(back!);
    expect(restored.loaded).toBe(true);
    expect(restored.running).toBe(false);
    expect(restored.id).toBe(KEY);
  });

  test("a thread with nothing in it is not kept, and attachments and long logs are cut", () => {
    expect(saveTranscript(thread(KEY, []), KEY)).toBe(false);
    const items: TranscriptItem[] = [
      {
        kind: "user",
        id: "u1",
        text: "look at this",
        attachments: [{ name: "shot.png", mime: "image/png", data: "AAAA" } as never],
      },
      { kind: "tool", id: "t1", title: "run", status: "completed", diffs: [], output: "x".repeat(20_000) },
    ];
    saveTranscript(thread(KEY, items), KEY);
    const back = loadTranscript(KEY)!;
    expect((back.items[0] as { attachments?: unknown }).attachments).toBeUndefined();
    expect((back.items[1] as { output?: string }).output!.length).toBeLessThan(5_000);
  });

  test("a transcript over the cap keeps its newest rows", () => {
    const items = Array.from({ length: 400 }, (_, i) => said(`a${i}`, `row ${i} ${"y".repeat(2_000)}`));
    expect(saveTranscript(thread(KEY, items), KEY)).toBe(true);
    const back = loadTranscript(KEY)!;
    expect(JSON.stringify(back).length).toBeLessThanOrEqual(MAX_TRANSCRIPT_CHARS);
    expect(back.items.length).toBeLessThan(400);
    expect(back.items.at(-1)?.id).toBe("a399");
  });

  test("a thread not opened for 30 days is dropped the next time something is kept", () => {
    const day = 24 * 60 * 60 * 1000;
    saveTranscript(thread(`${STUDIO}/old`, [said("a", "old")]), `${STUDIO}/old`, 0);
    expect(loadTranscript(`${STUDIO}/old`, 29 * day)).not.toBeNull();
    // Too old to show, even before anything prunes it.
    expect(loadTranscript(`${STUDIO}/old`, TRANSCRIPT_MAX_AGE_MS + day)).toBeNull();
    saveTranscript(thread(`${STUDIO}/new`, [said("a", "new")]), `${STUDIO}/new`, TRANSCRIPT_MAX_AGE_MS + day);
    expect([...disk.map.keys()].some((key) => key.endsWith("/old"))).toBe(false);
    expect(loadTranscript(`${STUDIO}/new`, TRANSCRIPT_MAX_AGE_MS + day)).not.toBeNull();
  });

  test("all of them together stay under the cap, the longest unopened going first", () => {
    const body = "z".repeat(MAX_TRANSCRIPT_CHARS - 2_000);
    const count = Math.ceil(MAX_TRANSCRIPTS_CHARS / MAX_TRANSCRIPT_CHARS) + 2;
    for (let i = 0; i < count; i++) {
      const key = `${STUDIO}/t${i}`;
      saveTranscript(thread(key, [said("a", body)]), key, 1_000 + i);
    }
    expect(loadTranscript(`${STUDIO}/t0`, 2_000)).toBeNull();
    expect(loadTranscript(`${STUDIO}/t${count - 1}`, 2_000)).not.toBeNull();
    let total = 0;
    for (const [key, value] of disk.map) if (key.includes("machineTranscript.")) total += value.length;
    expect(total).toBeLessThanOrEqual(MAX_TRANSCRIPTS_CHARS);
  });

  test("a deleted thread and a removed machine leave nothing behind", () => {
    saveTranscript(thread(KEY, [said("a", "one")]), KEY);
    saveTranscript(thread(`${LINUX}/x`, [said("a", "two")]), `${LINUX}/x`);
    saveThreadList(STUDIO, [row("a", 1)]);
    forgetTranscript(KEY);
    expect(loadTranscript(KEY)).toBeNull();
    saveTranscript(thread(KEY, [said("a", "one")]), KEY);
    forgetMachineCache(STUDIO);
    expect(loadTranscript(KEY)).toBeNull();
    expect(loadThreadList(STUDIO)).toEqual([]);
    expect(loadTranscript(`${LINUX}/x`)).not.toBeNull();
  });
});

describe("what the chat says over an offline thread", () => {
  test("names the machine and when it went away, and says the copy is the last one seen", () => {
    expect(offlineNotice({ machine: "Studio", since: "15:27", reconnecting: false, hasCopy: true })).toBe(
      "Studio is offline since 15:27. This is the last copy this computer saw; it updates when the machine is back.",
    );
  });

  test("says so when it is already reconnecting, or when no copy was kept", () => {
    expect(offlineNotice({ machine: "Studio", reconnecting: true, hasCopy: true })).toContain(
      "Reconnecting to Studio…",
    );
    expect(
      offlineNotice({ machine: "Studio", since: "15:27", reconnecting: false, hasCopy: false }),
    ).toContain("no copy of this thread");
  });
});

// ------------------------------------------------------------ the app

function machine(id: string, name: string, state: Machine["state"]): Machine {
  return {
    id,
    name,
    platform: "macos",
    version: "1.31.3",
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
  machines: Machine[] = [machine(STUDIO, "Studio", "offline")];
  async start(): Promise<void> {}
  send(): void {}
  async restart(): Promise<void> {}
  async stop(): Promise<void> {}
  async call(command: string): Promise<unknown> {
    if (command === "machines_list") return this.machines;
    if (command === "machine_connect") return { connected: false };
    return null;
  }
}

/** A live connection that answers every request with an empty replay. */
function liveConnection(): AcpConnection {
  let conn!: AcpConnection;
  conn = new AcpConnection((line) => {
    const frame = JSON.parse(line) as JsonObject;
    if (typeof frame.id !== "number" || typeof frame.method !== "string") return;
    queueMicrotask(() => {
      if (!conn.isClosed) {
        conn.receive(
          JSON.stringify({ jsonrpc: "2.0", id: frame.id, result: { configOptions: [], sessions: [] } }),
        );
      }
    });
  });
  return conn;
}

describe("a machine that is away", () => {
  beforeEach(async () => {
    resetForTests();
    resetMachineLinksForTests();
    useApp.setState({ selectedId: null, threads: {} });
    // This computer last read two threads from the machine, and the user had opened one.
    saveThreadList(STUDIO, [row("mock-studio-1", 200, { status: "running" }), row("mock-studio-2", 100)]);
    saveTranscript(thread(KEY, [asked("u1", "why does it fail?"), said("a1", "A missing import.")]), KEY);
    setShellTransport(new AwayShell());
    await refreshMachines();
    await startMachineRouting();
  });

  afterAll(() => {
    resetForTests();
    resetMachineLinksForTests();
    setShellTransport(null);
  });

  test("keeps its threads in the sidebar from the cache, with no stale status", () => {
    expect(useMachines.getState().sessions[STUDIO]?.map((s) => s.id)).toEqual([
      "mock-studio-1",
      "mock-studio-2",
    ]);
    const listed = useApp.getState().sessions.filter((s) => s.host === STUDIO);
    expect(listed.map((s) => s.id).sort()).toEqual([KEY, `${STUDIO}/mock-studio-2`]);
    expect(listed.every((s) => s.status === undefined)).toBe(true);
  });

  test("opens a thread read-only from the cached transcript, and one never opened empty", async () => {
    expect(await openThread(KEY)).toBeNull();
    const opened = useApp.getState().threads[KEY]!;
    expect(opened.items.map((i) => i.id)).toEqual(["u1", "a1"]);
    expect(opened.running).toBe(false);
    expect(opened.loading).toBe(false);
    expect(threadOffline(KEY)).toBe(true);

    const other = `${STUDIO}/mock-studio-2`;
    await openThread(other);
    expect(useApp.getState().threads[other]?.items).toEqual([]);
    expect(threadOffline(other)).toBe(true);
  });

  test("shows the live thread, not the copy, when the machine is back", async () => {
    await openThread(KEY);
    expect(useApp.getState().threads[KEY]?.items).toHaveLength(2);
    useApp.setState({ selectedId: KEY });

    watchMachineConnection(STUDIO, liveConnection());
    expect(threadOffline(KEY)).toBe(false);
    // The copy on screen is from before, so the thread is read again: the machine's replay (empty
    // here) replaces it, and the thread takes turns again.
    await openThread(KEY);
    await Bun.sleep(5);
    const live = useApp.getState().threads[KEY]!;
    expect(live.items).toEqual([]);
    expect(live.loaded).toBe(true);
    expect(live.openFailed).toBeUndefined();
  });

  test("keeps a thread's transcript while its machine answers, for the next time it is away", async () => {
    watchMachineConnection(STUDIO, liveConnection());
    useApp.setState((s) => ({
      threads: { ...s.threads, [KEY]: thread(KEY, [asked("u9", "and now?"), said("a9", "Fixed.")]) },
    }));
    await Bun.sleep(2_300);
    expect(loadTranscript(KEY)?.items.map((i) => i.id)).toEqual(["u9", "a9"]);
  });
});
