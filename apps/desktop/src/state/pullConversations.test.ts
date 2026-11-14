import { describe, expect, test } from "bun:test";
import type { PatchLine } from "./diff";
import { lineTarget, targetLabel } from "./pullConversations";

const lines: PatchLine[] = [
  { kind: "hunk", text: "@@ -1,4 +1,5 @@" },
  { kind: "ctx", text: "a", old: 1, new: 1 },
  { kind: "add", text: "b", new: 2 },
  { kind: "ctx", text: "c", old: 2, new: 3 },
  { kind: "del", text: "d", old: 3 },
  { kind: "del", text: "e", old: 4 },
  { kind: "hunk", text: "@@ -40,2 +41,2 @@" },
  { kind: "ctx", text: "f", old: 40, new: 41 },
];

describe("lineTarget", () => {
  test("a context or added line is on the pull request's side", () => {
    expect(lineTarget("a.ts", lines, 2)).toEqual({ path: "a.ts", line: 2, side: "RIGHT" });
    expect(lineTarget("a.ts", lines, 1)).toEqual({ path: "a.ts", line: 1, side: "RIGHT" });
  });

  test("a removed line is on the base's side with its old number", () => {
    expect(lineTarget("a.ts", lines, 4)).toEqual({ path: "a.ts", line: 3, side: "LEFT" });
  });

  test("a range on one side spans from the first line, in either click order", () => {
    expect(lineTarget("a.ts", lines, 3, 1)).toEqual({ path: "a.ts", line: 3, side: "RIGHT", startLine: 1 });
    expect(lineTarget("a.ts", lines, 1, 3)).toEqual({ path: "a.ts", line: 3, side: "RIGHT", startLine: 1 });
    expect(lineTarget("a.ts", lines, 5, 4)).toEqual({ path: "a.ts", line: 4, side: "LEFT", startLine: 3 });
  });

  test("a range across sides or hunks keeps only its last line", () => {
    expect(lineTarget("a.ts", lines, 4, 2)).toEqual({ path: "a.ts", line: 3, side: "LEFT" });
    expect(lineTarget("a.ts", lines, 7, 3)).toEqual({ path: "a.ts", line: 41, side: "RIGHT" });
  });

  test("a hunk header is not a line", () => {
    expect(lineTarget("a.ts", lines, 0)).toBeNull();
    expect(lineTarget("a.ts", lines, 99)).toBeNull();
  });
});

describe("targetLabel", () => {
  test("names the line or range and the base side", () => {
    expect(targetLabel({ path: "a", line: 3, side: "RIGHT" })).toBe("line 3");
    expect(targetLabel({ path: "a", line: 4, side: "LEFT", startLine: 3 })).toBe("base lines 3–4");
  });
});
