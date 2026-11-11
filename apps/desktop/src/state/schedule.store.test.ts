// Send later as the store keeps it (state/store.ts): a message written now, the card that waits for
// its time, a restart that reads it back, sending once due, Send now, and Delete.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { emptyExtras } from "./compose";
import { resetQueues } from "./host";
import { parseScheduleBook, SCHEDULED_KEY, type ScheduleBook } from "./schedule";
import {
  connect,
  continueDueScheduled,
  openThread,
  removeScheduled,
  resetForTests,
  restoreScheduled,
  scheduleMessage,
  sendScheduledNow,
  updateThread,
  useApp,
} from "./store";

const data = new Map<string, string>();
const scope = globalThis as unknown as Record<string, unknown>;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const until = async (check: () => boolean, ms = 4000) => {
  const start = Date.now();
  while (!check()) {
    if (Date.now() - start > ms) throw new Error("timed out waiting");
    await sleep(2);
  }
};

const stored = (): ScheduleBook => parseScheduleBook(data.get(SCHEDULED_KEY) ?? null);

/** A window with a web view's storage, seeded as an earlier run left it. */
const withStorage = async (seed: ScheduleBook | null, run: () => Promise<void>) => {
  data.clear();
  if (seed) data.set(SCHEDULED_KEY, JSON.stringify(seed));
  scope.localStorage = {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  } as unknown as Storage;
  try {
    await run();
  } finally {
    delete scope.localStorage;
  }
};

/** A connected app with one thread open and loaded. */
const openMockThread = async (id: string) => {
  await connect(new MockTransport({ delayMs: 0 }));
  await until(() => useApp.getState().status === "ready");
  await openThread(id);
  await until(() => useApp.getState().threads[id]?.loaded === true);
};

const said = (id: string, text: string) =>
  !!useApp.getState().threads[id]?.items.some((row) => row.kind === "user" && row.text.includes(text));

describe("send later", () => {
  beforeEach(() => {
    data.clear();
    resetForTests();
    resetQueues();
  });

  afterEach(() => {
    resetForTests();
    resetQueues();
    delete scope.localStorage;
    data.clear();
  });

  test("a message is kept under its thread and comes back in a new window", async () => {
    const due = Date.now() + 3_600_000;
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      const item = scheduleMessage("mock-thread-1", "carry on with the diff", emptyExtras, due);
      expect(item).not.toBeNull();
      expect(useApp.getState().scheduled["mock-thread-1"]?.map((row) => row.id)).toEqual([item!.id]);
      // Written at once, not on a timer: a quit after the press keeps it.
      expect(stored()["mock-thread-1"]?.map((row) => row.text)).toEqual(["carry on with the diff"]);

      // The window closes and opens again: only storage survives.
      resetForTests();
      expect(useApp.getState().scheduled).toEqual({});
      restoreScheduled();
      expect(useApp.getState().scheduled["mock-thread-1"]?.[0]).toMatchObject({
        text: "carry on with the diff",
        due,
      });
    });
  });

  test("when its time comes it is sent on the thread, and leaves the book", async () => {
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      const item = scheduleMessage("mock-thread-1", "the window has reset", emptyExtras, Date.now() + 5);
      expect(item).not.toBeNull();
      await sleep(10);
      await continueDueScheduled();
      await until(() => said("mock-thread-1", "the window has reset"));
      expect(useApp.getState().scheduled["mock-thread-1"]).toBeUndefined();
      // And it is not kept for a later window to send again.
      expect(stored()["mock-thread-1"]).toBeUndefined();
    });
  });

  test("a message that is not due yet stays", async () => {
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      scheduleMessage("mock-thread-1", "not yet", emptyExtras, Date.now() + 3_600_000);
      await continueDueScheduled();
      expect(useApp.getState().scheduled["mock-thread-1"]).toHaveLength(1);
      expect(said("mock-thread-1", "not yet")).toBe(false);
    });
  });

  test("a thread that is busy when the time comes queues it like a follow-up", async () => {
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      // A turn runs: the message goes behind it instead of starting one of its own.
      updateThread("mock-thread-1", (t) => ({ ...t, running: true }));
      scheduleMessage("mock-thread-1", "after this turn", emptyExtras, Date.now() + 5);
      await sleep(10);
      await continueDueScheduled();
      await until(() =>
        useApp.getState().threads["mock-thread-1"]!.queue.some((row) => row.text === "after this turn"),
      );
      expect(useApp.getState().scheduled["mock-thread-1"]).toBeUndefined();
      expect(said("mock-thread-1", "after this turn")).toBe(false);
    });
  });

  test("Send now sends it at once, before its time", async () => {
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      const item = scheduleMessage("mock-thread-1", "send me now", emptyExtras, Date.now() + 3_600_000)!;
      expect(await sendScheduledNow("mock-thread-1", item.id)).toBe(true);
      await until(() => said("mock-thread-1", "send me now"));
      expect(useApp.getState().scheduled["mock-thread-1"]).toBeUndefined();
    });
  });

  test("Delete drops it from the book and from storage", async () => {
    await withStorage(null, async () => {
      await openMockThread("mock-thread-1");
      const item = scheduleMessage("mock-thread-1", "never mind", emptyExtras, Date.now() + 3_600_000)!;
      removeScheduled("mock-thread-1", item.id);
      expect(useApp.getState().scheduled["mock-thread-1"]).toBeUndefined();
      expect(stored()["mock-thread-1"]).toBeUndefined();
      expect(said("mock-thread-1", "never mind")).toBe(false);
    });
  });
});
