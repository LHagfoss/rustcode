import { describe, expect, test } from "bun:test";
import {
  badgeCount,
  bodyFor,
  createNotifyGate,
  finishIsNews,
  firstLine,
  type NotifyContext,
  type NotifyEvent,
  WARMUP_MS,
  wakeEvent,
  warmingUp,
} from "./notify";

const context = (over: Partial<NotifyContext> = {}): NotifyContext => ({
  enabled: true,
  backgroundOnly: true,
  windowFocused: true,
  onScreen: "a",
  ...over,
});

const event = (over: Partial<NotifyEvent> = {}): NotifyEvent => ({
  threadId: "a",
  kind: "approval",
  key: "request-1",
  title: "Fix the sidebar",
  ...over,
});

describe("which moments notify", () => {
  test("an approval, a question, a limit, a failure and a finished turn all do", () => {
    const gate = createNotifyGate();
    const focused = context({ windowFocused: false });
    for (const kind of ["approval", "question", "limit", "failed", "finished"] as const) {
      expect(gate.take(event({ kind, key: kind }), focused)?.kind).toBe(kind);
    }
  });

  test("the thread on screen is never announced", () => {
    const gate = createNotifyGate();
    expect(gate.take(event(), context({ onScreen: "a", backgroundOnly: false }))).toBeNull();
  });

  test("another thread is, while the app is in the background", () => {
    const gate = createNotifyGate();
    expect(gate.take(event({ threadId: "b" }), context({ windowFocused: false }))).not.toBeNull();
  });

  test("with the background-only setting, the window in front says nothing at all", () => {
    const gate = createNotifyGate();
    const front = context({ windowFocused: true, backgroundOnly: true, onScreen: "a" });
    expect(gate.take(event({ threadId: "b" }), front)).toBeNull();
    // The same event behind the app is worth one.
    expect(gate.take(event({ threadId: "b" }), context({ windowFocused: false }))).not.toBeNull();
  });

  test("with the setting off, a thread off screen is announced in front of the app too", () => {
    const gate = createNotifyGate();
    expect(gate.take(event({ threadId: "b" }), context({ backgroundOnly: false }))).not.toBeNull();
  });

  test("notifications turned off says nothing, whatever the window does", () => {
    const gate = createNotifyGate();
    expect(gate.take(event(), context({ enabled: false, windowFocused: false }))).toBeNull();
  });

  test("a snoozed thread says nothing, and keeps what it has to say for later", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    const asleep = context({ windowFocused: false, snoozed: { a: 2_000 } });
    // A turn that ends, a request that arrives: neither is news while it sleeps.
    expect(gate.take(event({ kind: "finished", key: "1000" }), asleep)).toBeNull();
    expect(gate.take(event(), asleep)).toBeNull();
    // Its snooze is over: the same event is said once, then never again.
    expect(gate.take(event(), behind)?.kind).toBe("approval");
    expect(gate.take(event(), behind)).toBeNull();
    // Another thread is unaffected by its sleep.
    expect(gate.take(event({ threadId: "b" }), asleep)).not.toBeNull();
  });
});

describe("dedupe", () => {
  test("the same request twice is one notification", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    expect(gate.take(event(), behind)).not.toBeNull();
    expect(gate.take(event(), behind)).toBeNull();
  });

  test("a second request of the same thread is its own", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    expect(gate.take(event(), behind)).not.toBeNull();
    expect(gate.take(event({ key: "request-2" }), behind)).not.toBeNull();
  });

  test("the same request of another thread is its own", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    expect(gate.take(event(), behind)).not.toBeNull();
    expect(gate.take(event({ threadId: "b" }), behind)).not.toBeNull();
  });

  test("a turn's end is once per turn, and a new turn ends again", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    expect(gate.take(event({ kind: "finished", key: "1000" }), behind)).not.toBeNull();
    expect(gate.take(event({ kind: "finished", key: "1000" }), behind)).toBeNull();
    expect(gate.take(event({ kind: "finished", key: "2000" }), behind)).not.toBeNull();
  });

  test("an event hidden by the window in front still says itself once it is behind", () => {
    const gate = createNotifyGate();
    expect(gate.take(event(), context({ backgroundOnly: false, onScreen: "a" }))).toBeNull();
    expect(gate.take(event(), context({ windowFocused: false }))).not.toBeNull();
  });

  test("a thread the app no longer lists is forgotten", () => {
    const gate = createNotifyGate();
    const behind = context({ windowFocused: false });
    expect(gate.take(event(), behind)).not.toBeNull();
    gate.retain(new Set(["b"]));
    expect(gate.take(event(), behind)).not.toBeNull();
  });
});

describe("a launch", () => {
  test("says nothing about what the threads already told the app", () => {
    // From a fixed launch, not the module's: a slow test run must not outlast the window.
    const launch = 1_000_000;
    expect(warmingUp(launch + 10, launch)).toBe(true);
    expect(warmingUp(launch + WARMUP_MS + 1, launch)).toBe(false);
  });
});

describe("what it says", () => {
  test("the short line of each kind", () => {
    expect(bodyFor(event({ kind: "approval" }))).toBe("Needs your approval");
    expect(bodyFor(event({ kind: "finished" }))).toBe("Finished");
    expect(bodyFor(event({ kind: "limit" }))).toBe("Hit a usage limit");
  });

  test("a question says the question, in one line", () => {
    expect(bodyFor(event({ kind: "question", detail: "Which folder?\nThe one on main." }))).toBe(
      "Which folder?",
    );
    expect(bodyFor(event({ kind: "question" }))).toBe("Asked you a question");
  });

  test("a long first line is cut to a notification's width", () => {
    expect(firstLine("word ".repeat(40), 20).length).toBe(20);
  });
});

describe("the Dock badge", () => {
  test("counts the threads that need the user, once each", () => {
    expect(
      badgeCount({
        threads: {
          a: { permissions: [{}, {}] },
          b: { permissions: [] },
        },
        finished: { a: 10, c: 20 },
        failed: { c: 30 },
      }),
    ).toBe(2);
  });

  test("nothing needing the user clears it", () => {
    expect(badgeCount({ threads: { a: { permissions: [] } }, finished: {}, failed: {} })).toBe(0);
  });

  test("a snoozed thread is not counted until its snooze ends", () => {
    const state = {
      threads: { a: { permissions: [{}] }, b: { permissions: [] } },
      finished: { b: 10 },
      failed: { c: 20 },
    };
    expect(badgeCount({ ...state, snoozed: { a: 5, b: 5 } })).toBe(1);
    expect(badgeCount({ ...state, snoozed: { a: 5, b: 5, c: 5 } })).toBe(0);
  });
});

describe("what a thread that slept has to say", () => {
  const read = { threadId: "a", title: "Fix the sidebar" };

  test("one thing at most: a request first, then a failure, then a finish", () => {
    expect(
      wakeEvent({
        ...read,
        request: { rpcId: 7 },
        finishedAt: 1_000,
        failedAt: 2_000,
      }),
    ).toMatchObject({ kind: "approval", key: "7" });
    expect(wakeEvent({ ...read, request: { rpcId: 8, question: { text: "Which runner?" } } })).toMatchObject({
      kind: "question",
      key: "8",
      detail: "Which runner?",
    });
    expect(wakeEvent({ ...read, failedAt: 2_000, finishedAt: 1_000 })).toMatchObject({
      kind: "failed",
      key: "2000",
    });
    expect(wakeEvent({ ...read, finishedAt: 1_000 })).toMatchObject({ kind: "finished", key: "1000" });
  });

  test("the last turn it ended with is the one said, not an older one", () => {
    // A finish clears the failure before it, but a thread marked unread by hand keeps both.
    expect(wakeEvent({ ...read, finishedAt: 2_000, failedAt: 1_000 })).toMatchObject({
      kind: "finished",
      key: "2000",
    });
    expect(wakeEvent({ ...read, failedAt: 2_000, finishedAt: 2_000 })).toMatchObject({ kind: "failed" });
  });

  test("a delegate's own ending is its parent's news, not a notification", () => {
    expect(wakeEvent({ ...read, delegated: true, finishedAt: 1_000 })).toBeNull();
    expect(wakeEvent({ ...read, delegated: true, failedAt: 2_000 })).toBeNull();
    // What it waits on the user for is theirs, whoever started it.
    expect(wakeEvent({ ...read, delegated: true, request: { rpcId: 9 } })).toMatchObject({
      kind: "approval",
    });
  });

  test("a quiet return says nothing at all", () => {
    expect(wakeEvent(read)).toBeNull();
  });
});

describe("which finishes are news", () => {
  const idle = { waiting: false, delegated: false, queued: false };

  test("a thread that is truly finished is", () => {
    expect(finishIsNews(idle)).toBe(true);
  });

  test("a turn that ends while background work or a delegate is still going is not", () => {
    expect(finishIsNews({ ...idle, waiting: true })).toBe(false);
  });

  test("a delegate finishing is not: its parent hears it", () => {
    expect(finishIsNews({ ...idle, delegated: true })).toBe(false);
  });

  test("a turn that hands over to a queued message is between turns", () => {
    expect(finishIsNews({ ...idle, queued: true })).toBe(false);
  });

  test("a question reaches the gate whatever else the thread is doing", () => {
    const gate = createNotifyGate();
    const away = { enabled: true, backgroundOnly: true, windowFocused: false, onScreen: null };
    expect(
      gate.take({ threadId: "a", kind: "question", key: "7", title: "Fix it", detail: "Which runner?" }, away),
    ).not.toBeNull();
  });
});
