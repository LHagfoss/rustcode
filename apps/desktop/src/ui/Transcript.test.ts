import { describe, expect, test } from "bun:test";
import { Provider as TooltipProvider } from "@radix-ui/react-tooltip";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  appendSteer,
  emptyThread,
  stripMarks,
  type ThreadState,
  withPullMark,
  withWorkflowMark,
} from "../state/thread";
import { UserRow } from "./Rows";
import { estimatedRowHeight, queueRows, type Row, rowGap } from "./Transcript";

const user = (id: string, text = "hi"): Row => ({ key: id, item: { kind: "user", id, text } });
const tool = (id: string): Row => ({
  key: id,
  item: { kind: "tool", id, title: "bash", status: "completed", diffs: [] },
});
const thought = (id: string, text: string, done: boolean): Row => ({
  key: id,
  item: { kind: "thought", id, text, summary: false, startedAt: 0, done },
});

describe("the air above a transcript row", () => {
  test("a turn starts and ends on the wide gap, work inside it on the narrow one", () => {
    const rows = [user("m1"), tool("c1"), tool("c2"), user("m2")];
    expect(rowGap(rows, 0)).toBe(0);
    // A sent bubble ends on its own 24px time line, so the turn under it takes the narrow gap.
    expect(rowGap(rows, 1)).toBe(14);
    expect(rowGap(rows, 2)).toBe(14);
    // The message that starts the next turn takes the wide gap again.
    expect(rowGap(rows, 3)).toBe(28);
  });

  test("the first line under a divider sits on the narrow gap", () => {
    const rows: Row[] = [
      { key: "n", item: { kind: "notice", id: "n", text: "Compacted", tone: "info" } },
      tool("c1"),
      { key: "w", item: { kind: "user", id: "w", text: "A delegate finished", wake: true } },
      tool("c2"),
    ];
    expect(rowGap(rows, 1)).toBe(14);
    // A wake row starts a turn, so it takes the wide gap itself.
    expect(rowGap(rows, 2)).toBe(28);
    // A wake divider is the turn's own first line: the work under it sits on it.
    expect(rowGap(rows, 3)).toBe(14);
  });
});

describe("the height a row is reserved at", () => {
  test("a sent message is its bubble and the time line under it", () => {
    // 12px of padding each way, one line, then the 22px line the time keeps.
    expect(estimatedRowHeight(user("m1"))).toBe(12 * 2 + 23 + 24);
    // Pictures and files add their chip row above the bubble.
    expect(estimatedRowHeight(user("m1", "hi")) + 32).toBe(
      estimatedRowHeight({
        key: "m2",
        item: { kind: "user", id: "m2", text: "hi", attachments: [{ kind: "file", name: "a.ts" }] },
      }),
    );
  });

  test("the turn's own lines are the heights they are drawn at", () => {
    // `WorkingRow` is `h-[34px]` over a hairline; the live activity row is `min-h-6`.
    expect(estimatedRowHeight({ key: "working", working: {} })).toBe(35);
    expect(estimatedRowHeight({ key: "activity", activity: "Reading files" })).toBe(24);
  });

  test("a live thought shows the tail a finished one drops", () => {
    expect(estimatedRowHeight(thought("t", "one line", true))).toBe(17);
    const live = estimatedRowHeight(thought("t", "x".repeat(110), false));
    // A label line, its air, and the two lines of tail it has so far.
    expect(live).toBe(25 + 2 * 17);
    expect(estimatedRowHeight(thought("t", "x".repeat(2_000), false))).toBe(25 + 5 * 17);
  });

  test("a live thought is reserved for the text it is drawing, chunk by chunk", () => {
    // A harness streams its thinking in pieces, and the row grows with them. The estimate grows the
    // same amount, so the growth is the new text and never a correction on top of it.
    const first = estimatedRowHeight(thought("t", "**Reading the request**\n", false));
    const second = estimatedRowHeight(
      thought("t", "**Reading the request**\nChecking which files are involved ", false),
    );
    expect(first).toBe(17 + 8 + 17);
    expect(second - first).toBe(17);
  });

  test("a row's estimate and its gap are the whole size it mounts at", () => {
    // A message sent into an open thread: the bubble, then the turn's `Working for 0s` and its
    // activity line. Estimates that leave the gap out (or guess 24 for the working line) measured
    // 105px taller a frame after mounting, and the pinned transcript took that back in one step.
    const rows: Row[] = [
      tool("c0"),
      user("m1"),
      { key: "working", working: {} },
      { key: "activity", activity: "Working" },
    ];
    const reserved = rows.map((row, i) => rowGap(rows, i) + estimatedRowHeight(row));
    expect(reserved[1]).toBe(28 + 12 * 2 + 23 + 24);
    expect(reserved[2]).toBe(14 + 35);
    expect(reserved[3]).toBe(14 + 24);
  });

  test("a footer nothing draws takes no room, gap included", () => {
    const rows = [user("m1"), { key: "end" } as Row];
    expect(estimatedRowHeight(rows[1], true)).toBe(0);
  });
});

describe("a notice queued behind a running turn (issue 3709)", () => {
  const thread = (queue: { id: string; text: string }[]) => ({
    ...emptyThread("s", "/p"),
    turn: { kind: "user" as const, startedAt: 0 },
    queue,
  });

  test("a pull request notice is a divider row, never a bubble, and shows no marker", () => {
    const rows = queueRows(
      thread([
        { id: "q1", text: withPullMark("Pull request #3678: Already merged") },
        { id: "q2", text: "and fix the tests" },
        { id: "q3", text: withWorkflowMark("Workflow review: done\nthree agents") },
      ]),
      [],
    );
    expect(rows.map((r) => r.key)).toEqual(["queued-q1", "queued-q2", "queued-q3"]);
    expect(rows[0]?.queueNotice?.line).toBe("Pull request #3678: Already merged");
    expect(rows[0]?.queued).toBeUndefined();
    // The user's own message keeps its bubble, numbered among the messages only.
    expect(rows[1]?.queued?.index).toBe(0);
    expect(rows[1]?.queued?.count).toBe(1);
    expect(rows[2]?.queueNotice?.line).toBe("Workflow review: done");
    for (const row of rows) expect(JSON.stringify(row)).not.toContain("<!--" + " rustcode");
  });

  test("a held queue of notices is resumable but counts no messages", () => {
    const held = {
      ...thread([{ id: "q1", text: withPullMark("Pull request #1: Merged") }]),
      turn: undefined,
      queueHeld: true,
    };
    const rows = queueRows(held, []);
    expect(rows.map((r) => r.key)).toEqual(["queued-q1", "queue-resume"]);
    expect(rows[1]?.resume?.count).toBe(0);
  });

  test("a hidden marker is never drawn as text in a sent message either", () => {
    expect(stripMarks("Pull request #1: Merged\n\n<!-- rustcode:pull -->")).toBe("Pull request #1: Merged");
    const markup = renderToStaticMarkup(
      createElement(
        TooltipProvider,
        null,
        createElement(UserRow, {
          item: { kind: "user", id: "u", text: "hello\n\n<!-- rustcode:resumed -->" },
        }),
      ),
    );
    expect(markup).toContain("hello");
    expect(markup).not.toContain("rustcode:resumed");
  });
});

describe("a steered message (issue 3711)", () => {
  test("is drawn once: its bubble says Steered, and no divider repeats it", () => {
    let state: ThreadState = { ...emptyThread("s", "/p"), turn: { kind: "user", startedAt: 0 } };
    state = appendSteer(state, "dont cut new release", 1000);
    // The note stays on the thread for the plan card, and none of it is a transcript row of its own.
    expect(state.steers).toHaveLength(1);
    const rows = queueRows(state, []);
    expect(rows.some((r) => "steerNote" in r)).toBe(false);
    const item = state.items[0];
    if (item?.kind !== "user") throw new Error("not a message");
    const markup = renderToStaticMarkup(
      createElement(TooltipProvider, null, createElement(UserRow, { item })),
    );
    expect(markup.split("Steered").length - 1).toBe(1);
    expect(markup).not.toContain("↳");
  });
});
