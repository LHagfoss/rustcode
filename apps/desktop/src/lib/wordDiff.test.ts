import { describe, expect, test } from "bun:test";
import { MAX_MARKED_LINE, markedPieces, runWordMarks, wordDiff } from "./wordDiff";

/** The marked text of one side, so a test reads as the highlight it draws. */
function shown(text: string, ranges: { start: number; end: number }[]): string {
  let out = "";
  let at = 0;
  for (const r of ranges) {
    out += `${text.slice(at, r.start)}[${text.slice(r.start, r.end)}]`;
    at = r.end;
  }
  return out + text.slice(at);
}

describe("wordDiff", () => {
  test("marks the token that changed on both sides", () => {
    const marks = wordDiff("const total = count + 1;", "const total = count + 2;");
    expect(marks).not.toBeNull();
    expect(shown("const total = count + 1;", marks!.old)).toBe("const total = count + [1];");
    expect(shown("const total = count + 2;", marks!.new)).toBe("const total = count + [2];");
  });

  test("marks nothing on the side an edit only added to", () => {
    const marks = wordDiff("let x", "let x = 1");
    expect(marks).not.toBeNull();
    expect(marks!.old).toEqual([]);
    expect(shown("let x = 1", marks!.new)).toBe("let x [= 1]");
  });

  test("has nothing to mark for identical lines", () => {
    expect(wordDiff("same()", "same()")).toBeNull();
  });

  test("leaves a very long line unmarked", () => {
    const long = `const a = "${"x".repeat(MAX_MARKED_LINE)}";`;
    expect(wordDiff(long, `${long} // changed`)).toBeNull();
  });

  test("leaves lines that differ too much unmarked", () => {
    expect(wordDiff("alpha beta gamma delta", "one two three four")).toBeNull();
  });

  test("does not mark a whitespace-only change as a word", () => {
    const marks = wordDiff("call(a, b)", "call(a,b)");
    // The removed space is whitespace: nothing to tint, so there is nothing to draw.
    expect(marks).toBeNull();
  });

  test("marks an identifier that was renamed", () => {
    const marks = wordDiff("  return threadId;", "  return sessionId;");
    expect(shown("  return threadId;", marks!.old)).toBe("  return [threadId];");
    expect(shown("  return sessionId;", marks!.new)).toBe("  return [sessionId];");
  });

  test("keeps the marks inside the line", () => {
    const text = "if (ready) start();";
    const marks = wordDiff(text, "if (ready) begin();");
    for (const r of marks!.old) {
      expect(r.start).toBeGreaterThanOrEqual(0);
      expect(r.end).toBeLessThanOrEqual(text.length);
      expect(r.start).toBeLessThan(r.end);
    }
  });
});

describe("runWordMarks", () => {
  test("pairs each removal with the addition written after it", () => {
    const lines = [
      { kind: "context", text: "keep()" },
      { kind: "removed", text: "old(a)" },
      { kind: "added", text: "next(a)" },
      { kind: "context", text: "keep()" },
      { kind: "removed", text: "gone()" },
      { kind: "added", text: "went()" },
    ];
    const marks = runWordMarks(lines);
    expect(marks[0]).toBeUndefined();
    expect(shown("old(a)", marks[1]!)).toBe("[old](a)");
    expect(shown("next(a)", marks[2]!)).toBe("[next](a)");
    expect(shown("gone()", marks[4]!)).toBe("[gone]()");
    expect(shown("went()", marks[5]!)).toBe("[went]()");
  });

  test("pairs by position across a longer run", () => {
    const marks = runWordMarks([
      { kind: "removed", text: "one = 1" },
      { kind: "removed", text: "two = 2" },
      { kind: "added", text: "one = 10" },
      { kind: "added", text: "two = 20" },
    ]);
    expect(shown("one = 1", marks[0]!)).toBe("one = [1]");
    expect(shown("one = 10", marks[2]!)).toBe("one = [10]");
  });

  test("marks nothing for an addition with no removal beside it", () => {
    expect(runWordMarks([{ kind: "added", text: "fresh()" }])).toEqual([undefined]);
  });

  test("reads the review screen's kind names too", () => {
    const marks = runWordMarks([
      { kind: "del", text: "a = 1" },
      { kind: "add", text: "a = 1 + 1" },
    ]);
    expect(marks[0]).toBeUndefined();
    expect(shown("a = 1 + 1", marks[1]!)).toBe("a = 1 [+ 1]");
  });
});

describe("markedPieces", () => {
  test("covers the whole line in order", () => {
    const text = "const total = 1;";
    const pieces = markedPieces(text, [{ start: 14, end: 15 }]);
    expect(pieces.map((p) => p.text).join("")).toBe(text);
    expect(pieces.filter((p) => p.marked).map((p) => p.text)).toEqual(["1"]);
  });

  test("is one plain piece without marks", () => {
    expect(markedPieces("plain", undefined)).toEqual([{ text: "plain", marked: false }]);
    expect(markedPieces("plain", [])).toEqual([{ text: "plain", marked: false }]);
  });

  test("ignores marks outside the text and empty ones", () => {
    expect(
      markedPieces("ab", [
        { start: 5, end: 9 },
        { start: 1, end: 1 },
      ]),
    ).toEqual([{ text: "ab", marked: false }]);
  });
});
