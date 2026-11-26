import { describe, expect, test } from "bun:test";
import { parsePatch, QUOTED_LINES, rangeLines, reviewMessage } from "./diff";

const patch = parsePatch([
  "@@ -1,3 +1,4 @@",
  " one",
  "-two",
  "+TWO",
  "+extra",
  " three",
  "@@ -10,2 +11,2 @@",
  " ten",
  " eleven",
]);

describe("review ranges", () => {
  test("a range takes the lines of one side, in order, either way round", () => {
    expect(rangeLines(patch, "RIGHT", 1, 3)?.map((l) => l.text)).toEqual(["one", "TWO", "extra"]);
    expect(rangeLines(patch, "RIGHT", 3, 1)?.map((l) => l.text)).toEqual(["one", "TWO", "extra"]);
    expect(rangeLines(patch, "LEFT", 1, 3)?.map((l) => l.text)).toEqual(["one", "two", "three"]);
  });

  test("a range does not cross a hunk or reach a line the diff lacks", () => {
    expect(rangeLines(patch, "RIGHT", 3, 11)).toBeNull();
    expect(rangeLines(patch, "RIGHT", 1, 7)).toBeNull();
  });

  test("the message names a range and quotes its lines, eliding the rest", () => {
    const code = Array.from({ length: QUOTED_LINES + 2 }, (_, i) => `line ${i}`).join("\n");
    const message = reviewMessage("branch b", [
      { path: "a.ts", line: 18, startLine: 11, side: "RIGHT", body: "Split this.", code },
      { path: "b.ts", line: 4, side: "LEFT", body: "Keep it.", code: "old();" },
    ]);
    expect(message).toContain("- `a.ts:11-18`: Split this.\n  > line 0\n");
    expect(message).toContain(`  > line ${QUOTED_LINES - 1}\n  > …`);
    expect(message).not.toContain(`line ${QUOTED_LINES}\n`);
    expect(message).toContain("- `b.ts:4` (removed line): Keep it.\n  > old();");
  });
});
