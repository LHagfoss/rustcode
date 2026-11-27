import { describe, expect, test } from "bun:test";
import { changedShots, commentFor, countDiffering, MARKER, MIN_PIXELS } from "./uiShots";

const png = (text: string) => Buffer.from(text);

describe("countDiffering", () => {
  test("ignores small channel moves and counts each moved pixel once", () => {
    const a = [10, 10, 10, 255, 10, 10, 10, 255, 10, 10, 10, 255];
    const b = [20, 10, 10, 255, 90, 90, 10, 255, 10, 10, 10, 0];
    expect(countDiffering(a, b, 16)).toBe(1);
  });
});

describe("changedShots", () => {
  test("lists views that moved past the threshold, appeared or disappeared", async () => {
    const before = new Map([
      ["home-dark.png", png("a")],
      ["thread-dark.png", png("b")],
      ["noise-dark.png", png("n")],
      ["gone-dark.png", png("c")],
    ]);
    const after = new Map([
      ["home-dark.png", png("a")],
      ["thread-dark.png", png("B")],
      ["noise-dark.png", png("N")],
      ["new-dark.png", png("d")],
    ]);
    const moved = async (a: Buffer) => (a.toString() === "n" ? MIN_PIXELS : MIN_PIXELS + 1);
    expect(await changedShots(before, after, moved)).toEqual([
      "gone-dark.png",
      "new-dark.png",
      "thread-dark.png",
    ]);
  });
});

describe("commentFor", () => {
  test("says nothing changed in one line", () => {
    expect(commentFor([], 8, "https://x")).toBe(
      `${MARKER}\n**UI screenshots**: none of the 8 captured views changed.\n`,
    );
  });

  test("puts each changed view's before and after side by side", () => {
    const body = commentFor(["thread-light.png"], 8, "https://raw/pr-1/abc");
    expect(body.startsWith(MARKER)).toBe(true);
    expect(body).toContain("1 of 8 views changed");
    expect(body).toContain(
      '| **thread-light** | <img src="https://raw/pr-1/abc/before/thread-light.png" width="420"> | <img src="https://raw/pr-1/abc/after/thread-light.png" width="420"> |',
    );
  });
});
