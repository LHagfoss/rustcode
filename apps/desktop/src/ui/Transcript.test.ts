import { describe, expect, test } from "bun:test";
import { estimatedRowHeight, type Row, rowGap } from "./Transcript";

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
    expect(rowGap(rows, 1)).toBe(28);
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
    expect(estimatedRowHeight({ key: "working", working: true })).toBe(35);
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
      { key: "working", working: true },
      { key: "activity", activity: "Working" },
    ];
    const reserved = rows.map((row, i) => rowGap(rows, i) + estimatedRowHeight(row));
    expect(reserved[1]).toBe(28 + 12 * 2 + 23 + 24);
    expect(reserved[2]).toBe(28 + 35);
    expect(reserved[3]).toBe(14 + 24);
  });

  test("a footer nothing draws takes no room, gap included", () => {
    const rows = [user("m1"), { key: "end" } as Row];
    expect(estimatedRowHeight(rows[1], true)).toBe(0);
  });
});
