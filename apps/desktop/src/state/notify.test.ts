import { describe, expect, test } from "bun:test";
import {
  badgeCount,
  bodyFor,
  createNotifyGate,
  firstLine,
  type NotifyContext,
  type NotifyEvent,
  WARMUP_MS,
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
    expect(warmingUp()).toBe(true);
    expect(warmingUp(Date.now() + WARMUP_MS + 1)).toBe(false);
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
});
