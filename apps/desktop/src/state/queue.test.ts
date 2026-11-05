import { describe, expect, test } from "bun:test";
import { isOwnQueued, isQueuedMessage, nextQueued, queuedAnnouncement, queueView } from "./queue";
import type { QueuedPrompt } from "./thread";

const item = (id: string, extra: Partial<QueuedPrompt> = {}): QueuedPrompt => ({
  id,
  text: `message ${id}`,
  ...extra,
});

const thread = (queue: QueuedPrompt[], over: { running?: boolean; queueHeld?: boolean } = {}) => ({
  queue,
  running: over.running ?? false,
  queueHeld: over.queueHeld,
});

describe("the end of a thread's queue", () => {
  test("the user's messages are rows, in the order they run", () => {
    const view = queueView(thread([item("a"), item("b")]));
    expect(view.messages.map((q) => q.id)).toEqual(["a", "b"]);
    expect(view.wake).toBeUndefined();
    expect(view.held).toBe(false);
  });

  test("a wake is one quiet line, never a row, and a link message is a row of its own", () => {
    const view = queueView(
      thread([
        item("a"),
        item("w", { text: "2 children finished · 1 needs you\n\nFinished:", wake: true }),
        item("l", { link: "a/b" }),
      ]),
    );
    expect(view.messages.map((q) => q.id)).toEqual(["a", "l"]);
    // Its first line is its summary, which is what the transcript shows.
    expect(view.wake).toBe("2 children finished · 1 needs you");
    expect(isQueuedMessage(item("w", { wake: true }))).toBe(false);
    // Only the user's own message is editable in the composer.
    expect(isOwnQueued(item("a"))).toBe(true);
    expect(isOwnQueued(item("l", { link: "a/b" }))).toBe(false);
    expect(isOwnQueued(item("w", { wake: true }))).toBe(false);
  });

  test("a held queue says so, and offers Resume, only while no turn runs", () => {
    const held = queueView(thread([item("a")], { queueHeld: true }));
    expect(held.held).toBe(true);
    expect(held.resumable).toBe(true);
    // The queue that a held turn left does not stay held once its turn started.
    expect(queueView(thread([item("a")], { queueHeld: true, running: true })).held).toBe(false);
    // Nothing to resume with nothing queued.
    expect(queueView(thread([], { queueHeld: true })).resumable).toBe(false);
  });
});

describe("when the next queued message starts", () => {
  const queue = [item("a"), item("b")];

  test("the head of the queue starts when the thread is idle", () => {
    expect(nextQueued(queue, { running: false })?.id).toBe("a");
    expect(nextQueued([], { running: false })).toBeUndefined();
  });

  test("nothing starts while a turn runs", () => {
    expect(nextQueued(queue, { running: true })).toBeUndefined();
  });

  test("nothing starts while the queue is held, however idle the thread is", () => {
    expect(nextQueued(queue, { running: false, queueHeld: true })).toBeUndefined();
    // Resume lets it go: the same queue starts at its head.
    expect(nextQueued(queue, { running: false, queueHeld: undefined })?.id).toBe("a");
  });
});

describe("what the live region says", () => {
  test("counts what waits, and whether it is held", () => {
    expect(queuedAnnouncement(queueView(thread([item("a")])), false)).toBe("1 message queued");
    expect(queuedAnnouncement(queueView(thread([item("a"), item("b")])), false)).toBe("2 messages queued");
    expect(queuedAnnouncement(queueView(thread([item("a")], { queueHeld: true })), false)).toBe(
      "1 message held",
    );
    expect(queuedAnnouncement(queueView(thread([], { queueHeld: true })), false)).toBe("The queue is empty");
  });

  test("a wake is announced as waiting, and an edit says where it is", () => {
    const view = queueView(thread([item("w", { text: "1 child finished", wake: true })]));
    expect(queuedAnnouncement(view, false)).toBe("a wake-up waiting");
    expect(queuedAnnouncement(queueView(thread([item("a")])), true)).toBe(
      "1 message queued · Editing a queued message in the composer",
    );
  });
});
