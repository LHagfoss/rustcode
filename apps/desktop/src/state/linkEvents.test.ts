import { describe, expect, test } from "bun:test";
import {
  advanceLink,
  emptyBook,
  interruptActive,
  LINK_EVENT_LABEL,
  type Link,
  type LinkEvent,
  linkEventText,
  linkStatusText,
  MAX_LINK_EVENTS,
  noteEvent,
  noteLinkEvent,
  parseBook,
  updateLink,
  waitEnded,
  waitStarted,
} from "./orchestration";

const at = (n: number) => 1_700_000_000_000 + n * 1000;

/** A link mid-turn, with whatever the test needs on top. */
const link = (extra: Partial<Link> = {}): Link => ({
  id: "parent/1",
  orchestrator: "parent",
  target: "child",
  orchestratorTitle: "Parent",
  targetTitle: "Child",
  message: "Do the thing",
  state: "working",
  started: true,
  harness: "pi",
  harnessName: "Pi",
  model: "deepseek/flash",
  createdAt: at(0),
  startedAt: at(0),
  ...extra,
});

describe("a link's events", () => {
  test("one more event, oldest first, bounded to the last few", () => {
    const one = noteEvent(link(), "message", at(1));
    expect(one.events?.map((e) => e.kind)).toEqual(["message"]);

    let many = link();
    for (let i = 0; i < MAX_LINK_EVENTS + 4; i++) many = noteEvent(many, "working", at(i));
    expect(many.events).toHaveLength(MAX_LINK_EVENTS);
    // The newest stay: the chip's status is the list's end.
    expect(many.events?.at(-1)?.at).toBe(at(MAX_LINK_EVENTS + 3));
  });

  test("the same moment written twice is one event", () => {
    const one = noteEvent(link(), "asking", at(2));
    expect(noteEvent(one, "asking", at(2))).toBe(one);
  });

  test("a state it newly reaches is the event that says so", () => {
    const queued = link({ state: "queued", startedAt: undefined });
    const working = advanceLink(queued, { state: "working", startedAt: at(3) }, at(3));
    expect(working.events?.at(-1)).toEqual({ kind: "working", at: at(3) });

    const answered = advanceLink(working, { state: "done", settledAt: at(70), workedMs: 67_000 }, at(70));
    expect(answered.events?.at(-1)).toEqual({
      kind: "answered",
      at: at(70),
      detail: "worked 1m 7s",
    });

    // A change that settles nothing new is not an event (the answer's own text, a mark).
    expect(advanceLink(answered, { answer: "done" }, at(71)).events).toHaveLength(2);
  });

  test("a wait on the user is asked, and its end is work again", () => {
    const asked = waitStarted([link({ events: [{ kind: "working", at: at(1) }] })], "child", at(20));
    expect(asked[0]?.events?.at(-1)).toEqual({ kind: "asking", at: at(20) });
    expect(asked[0]?.waitingSince).toBe(at(20));

    const answered = waitEnded(asked, "child", at(30));
    expect(answered[0]?.events?.at(-1)).toEqual({ kind: "working", at: at(30) });
    expect(answered[0]?.waitedMs).toBe(10_000);
  });

  test("a restart stops the link and says so once", () => {
    const book = { ...emptyBook(), links: [link()] };
    const stopped = interruptActive(book, at(5));
    expect(stopped.links[0]?.state).toBe("interrupted");
    expect(stopped.links[0]?.events?.at(-1)).toEqual({ kind: "interrupted", at: at(5) });
    // A link already settled by the restart is not stopped again.
    expect(interruptActive(stopped, at(6)).links[0]?.events).toHaveLength(1);
  });

  test("a merge or a cleanup lands on the newest link to that thread", () => {
    const older = link({ id: "parent/1", createdAt: at(0) });
    const newer = link({ id: "parent/2", createdAt: at(4) });
    const book = { ...emptyBook(), links: [older, newer] };
    const merged = noteLinkEvent(book, "parent", "child", "merged", at(80));
    expect(merged.links[1]?.events?.at(-1)).toEqual({ kind: "merged", at: at(80) });
    expect(merged.links[0]?.events).toBeUndefined();
    // A thread this parent never linked is left alone.
    expect(noteLinkEvent(book, "parent", "elsewhere", "cleaned", at(80))).toBe(book);
  });

  test("updateLink writes the state's event at the time it is given", () => {
    const book = { ...emptyBook(), links: [link()] };
    const failed = updateLink(book, "parent/1", { state: "failed" }, at(9));
    expect(failed.links[0]?.events?.at(-1)).toEqual({ kind: "failed", at: at(9) });
  });
});

describe("the chip's status text", () => {
  test("it is the last event's word", () => {
    const events: LinkEvent[] = [
      { kind: "started", at: at(0), detail: "Pi · deepseek/flash" },
      { kind: "working", at: at(1) },
      { kind: "asking", at: at(5) },
    ];
    expect(linkStatusText(link({ state: "needs_user", events }))).toBe("Asked you");

    const answered: LinkEvent[] = [...events, { kind: "answered", at: at(9), detail: "worked 1m 7s" }];
    expect(linkStatusText(link({ state: "done", events: answered }))).toBe("Answered");
  });

  test("with no events it keeps the link's state word", () => {
    expect(linkStatusText(link({ state: "done" }))).toBe("Done");
    expect(linkStatusText(link({ state: "cancelled" }))).toBe("Stopped");
  });

  test("a wait reads as the question it asked, and its end as the work", () => {
    const asking = link({ events: [{ kind: "asking" as const, at: at(5) }] });
    // Shown as waiting while its child asks, though the link itself still reads working.
    expect(linkStatusText(asking, "needs_user")).toBe("Asked you");
    // A wait with no event of its own still reads as one, not as its state word.
    expect(linkStatusText(link(), "needs_user")).toBe("Needs you");
    // Answered, and the next turn started without its own event (a book from before the list).
    expect(linkStatusText(asking, "working")).toBe("Working");
  });

  test("an event reads with what it adds", () => {
    expect(linkEventText({ kind: "started", at: at(0), detail: "Pi · deepseek/flash" })).toBe(
      "Started · Pi · deepseek/flash",
    );
    expect(linkEventText({ kind: "merged", at: at(0) })).toBe(LINK_EVENT_LABEL.merged);
  });
});

describe("a stored book", () => {
  test("keeps each link's events, and drops what is not one", () => {
    const parsed = parseBook({
      links: [
        {
          ...link({
            events: [{ kind: "started", at: at(0), detail: "Pi" }, { kind: "done" } as unknown as LinkEvent],
          }),
        },
      ],
      grants: {},
      counters: {},
      fanOuts: [],
    } as never);
    expect(parsed?.links[0]?.events).toEqual([{ kind: "started", at: at(0), detail: "Pi" }]);
  });
});
