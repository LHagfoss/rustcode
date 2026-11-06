// The queue a thread keeps across a restart (state/queues.ts, wired in state/host.ts): the round trip
// through storage, the bounds, the ids that were sent so nothing is delivered twice, and a restart
// that puts a thread's queue back and runs it.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { MockTransport } from "../acp/mock";
import { resetQueues } from "./host";
import {
  MAX_ACKED,
  MAX_ROW_EXTRAS,
  MAX_ROW_TEXT,
  MAX_ROWS,
  MAX_THREADS,
  parseQueueBook,
  pendingRows,
  persistable,
  QUEUES_KEY,
  type QueueBook,
  queueOf,
  withQueue,
} from "./queues";
import { connect, openThread, resetForTests, updateThread, useApp } from "./store";
import type { QueuedPrompt } from "./thread";

const item = (id: string, extra: Partial<QueuedPrompt> = {}): QueuedPrompt => ({
  id,
  text: `message ${id}`,
  ...extra,
});

const now = 1_760_000_000_000;

/** The book as a restart reads it: what was written, through JSON and back. */
const roundTrip = (book: QueueBook): QueueBook => parseQueueBook(JSON.stringify(book));

const ids = (queue: readonly QueuedPrompt[]) => queue.map((q) => q.id);

describe("the queue as it is kept", () => {
  test("a thread's queue comes back as it went in, in order, with what went with each row", () => {
    const queue = [
      item("a", { extras: { links: ["src/state/store.ts"], attachments: [] } }),
      item("w", {
        text: "1 child needs you",
        wake: true,
        wakeLinks: ["link-1", "link-2"],
        wakeAsking: ["link-2"],
      }),
      item("l", { link: "link-3", fromView: true }),
      item("r", { restart: true }),
    ];
    const book = roundTrip(withQueue({ threads: {}, acked: [] }, "t1", queue, now));
    expect(book.threads.t1).toHaveLength(4);
    expect(book.threads.t1![0]).toMatchObject({
      id: "a",
      kind: "user",
      created_at: now,
      extras: { links: ["src/state/store.ts"], attachments: [] },
    });
    expect(book.threads.t1![1]).toMatchObject({
      kind: "wake",
      wakeLinks: ["link-1", "link-2"],
      wakeAsking: ["link-2"],
    });
    expect(queueOf(book.threads.t1!)).toEqual(queue);
  });

  test("a row keeps the moment it was queued, however often the queue is written again", () => {
    const first = withQueue({ threads: {}, acked: [] }, "t1", [item("a")], now);
    const second = withQueue(first, "t1", [item("a"), item("b")], now + 60_000);
    expect(second.threads.t1!.map((row) => [row.id, row.created_at])).toEqual([
      ["a", now],
      ["b", now + 60_000],
    ]);
  });

  test("a row nobody could read back is not kept; an unreadably big attachment goes, its text stays", () => {
    const huge = "x".repeat(MAX_ROW_TEXT + 1);
    const big = {
      links: [],
      attachments: [{ id: "f", kind: "file" as const, name: "a.txt", text: "y".repeat(MAX_ROW_EXTRAS) }],
    };
    const book = roundTrip(
      withQueue(
        { threads: {}, acked: [] },
        "t1",
        [item("huge", { text: huge }), item("big", { extras: big })],
        now,
      ),
    );
    expect(book.threads.t1!.map((row) => row.id)).toEqual(["big"]);
    // The message is still a message: only what cannot be kept is dropped.
    expect(book.threads.t1![0]!.extras).toBeUndefined();
    expect(book.threads.t1![0]!.text).toBe("message big");
  });

  test("a book is bounded: rows per thread, threads kept, and ids remembered", () => {
    const many = Array.from({ length: MAX_ROWS + 3 }, (_, i) => item(`q${i}`));
    const kept = persistable(withQueue({ threads: {}, acked: [] }, "t1", many, now));
    expect(kept.threads.t1).toHaveLength(MAX_ROWS);
    // The newest rows are the ones kept: a queue is read from its head, and the head ran first.
    expect(kept.threads.t1![0]!.id).toBe("q3");

    let book: QueueBook = { threads: {}, acked: [] };
    for (let i = 0; i < MAX_THREADS + 2; i++) book = withQueue(book, `t${i}`, [item(`m${i}`)], now);
    expect(Object.keys(persistable(book).threads)).toHaveLength(MAX_THREADS);
    expect(Object.keys(persistable(book).threads)[0]).toBe("t2");

    const acked = persistable({
      threads: {},
      acked: Array.from({ length: MAX_ACKED + 5 }, (_, i) => `a${i}`),
    });
    expect(acked.acked).toHaveLength(MAX_ACKED);
    expect(acked.acked[0]).toBe("a5");
  });

  test("a book that is not one starts empty, and unreadable rows are dropped", () => {
    expect(parseQueueBook(null)).toEqual({ threads: {}, acked: [] });
    expect(parseQueueBook("not json")).toEqual({ threads: {}, acked: [] });
    expect(parseQueueBook("[1,2,3]")).toEqual({ threads: {}, acked: [] });
    const book = parseQueueBook(
      JSON.stringify({
        threads: {
          t1: [{ id: "a", text: "hi", kind: "user" }, { id: "b" }, { id: "", text: "x", kind: "user" }],
          junk: 5,
        },
        acked: ["z", 7, "y"],
      }),
    );
    expect(book.threads.t1!.map((row) => [row.id, row.created_at])).toEqual([["a", 0]]);
    expect(book.acked).toEqual(["z", "y"]);
  });
});

describe("a row that was sent is never sent again", () => {
  test("what left the queue is recorded, and a restart does not put it back", () => {
    const first = withQueue({ threads: {}, acked: [] }, "t1", [item("a"), item("b")], now);
    // The app sent `a` off the head of the queue: only `b` waits now.
    const second = withQueue(first, "t1", [item("b")], now);
    expect(second.acked).toContain("a");
    // Even a queue written back with `a` in it cannot bring it back: the id is spent.
    const third = withQueue(second, "t1", [item("a"), item("b")], now);
    expect(ids(queueOf(pendingRows(third, "t1"), third.acked))).toEqual(["b"]);
  });

  test("a thread with nothing left keeps no rows, and its saved queue is gone", () => {
    const book = withQueue({ threads: {}, acked: [] }, "t1", [item("a")], now);
    const emptied = withQueue(book, "t1", [], now);
    expect(emptied.threads.t1).toBeUndefined();
    expect(pendingRows(emptied, "t1")).toEqual([]);
    expect(roundTrip(emptied).acked).toEqual(["a"]);
  });
});

// ---------------------------------------------------------------- across a restart

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

/** A window that has a web view's storage: queued rows written there, as the last run left them. */
const withStorage = async (book: unknown, run: () => Promise<void>) => {
  data.clear();
  if (book) data.set(QUEUES_KEY, JSON.stringify(book));
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

describe("a restart puts a thread's queue back", () => {
  beforeEach(() => {
    resetForTests();
    resetQueues();
  });

  afterEach(() => {
    resetForTests();
    resetQueues();
  });

  test("a message queued before the restart runs, and is not kept for a later one", async () => {
    const kept = withQueue(
      { threads: {}, acked: [] },
      "mock-thread-1",
      [item("q1", { text: "carry on with the diff" })],
      now,
    );
    await withStorage(kept, async () => {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().threads["mock-thread-1"]?.loaded === true);
      const thread = useApp.getState().threads["mock-thread-1"]!;
      // It started a turn of its own: the queue is empty and the message is in the transcript.
      await until(() =>
        thread.items.some((row) => row.kind === "user" && row.text.includes("carry on with the diff")),
      );
      expect(useApp.getState().threads["mock-thread-1"]!.queue).toEqual([]);
      await until(() => !(JSON.parse(data.get(QUEUES_KEY) ?? "{}") as QueueBook).threads["mock-thread-1"]);
      const written = JSON.parse(data.get(QUEUES_KEY) ?? "{}") as QueueBook;
      // Sent, so recorded: a second restart cannot deliver it again.
      expect(written.acked).toContain("q1");
    });
  });

  test("a row whose id was already sent is not put back by a second window", async () => {
    const kept = withQueue({ threads: {}, acked: ["q1"] }, "mock-thread-1", [item("q1")], now);
    await withStorage(kept, async () => {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready");
      // Nothing to run in storage, so the thread is not opened for it, and no turn starts.
      await sleep(50);
      expect(useApp.getState().threads["mock-thread-1"]).toBeUndefined();
      await openThread("mock-thread-1");
      await sleep(50);
      // Opened by hand, it is still empty: the id was spent, and the row is never run twice.
      expect(useApp.getState().threads["mock-thread-1"]!.queue).toEqual([]);
      expect(
        useApp
          .getState()
          .threads["mock-thread-1"]!.items.some((row) => row.kind === "user" && row.text.includes("q1")),
      ).toBe(false);
    });
  });

  test("a queue waiting behind a reloaded thread is written back as it was", async () => {
    await withStorage(undefined, async () => {
      await connect(new MockTransport({ delayMs: 0 }));
      await until(() => useApp.getState().status === "ready");
      const id = "mock-thread-1";
      await openThread(id);
      await until(() => useApp.getState().threads[id]?.loaded === true);
      // The user queues two messages: they are written down without anything being sent.
      updateThread(id, (t) => ({ ...t, queue: [item("q1"), item("q2")] }));
      await until(() => (JSON.parse(data.get(QUEUES_KEY) ?? "{}") as QueueBook).threads[id]?.length === 2);
      const written = parseQueueBook(data.get(QUEUES_KEY) ?? null);
      expect(written.threads[id]!.map((row) => row.id)).toEqual(["q1", "q2"]);
      expect(written.acked).toEqual([]);
    });
  });
});
