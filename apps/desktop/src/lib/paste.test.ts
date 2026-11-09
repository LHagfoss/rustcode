import { describe, expect, test } from "bun:test";
import { isLongPaste, PASTE_CHARS, PASTE_LINES, pasteLabel } from "./paste";

const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i + 1}`).join("\n");

describe("a long paste", () => {
  test("is a chip past a dozen lines, text before that", () => {
    expect(isLongPaste(lines(PASTE_LINES - 1))).toBe(false);
    expect(isLongPaste(lines(PASTE_LINES))).toBe(true);
    expect(isLongPaste("a log line\nanother")).toBe(false);
  });

  test("is a chip past 1200 characters even with one line", () => {
    expect(isLongPaste("x".repeat(PASTE_CHARS - 1))).toBe(false);
    expect(isLongPaste("x".repeat(PASTE_CHARS))).toBe(true);
    expect(isLongPaste(`a\n${"x".repeat(PASTE_CHARS)}`)).toBe(true);
  });

  test("says how many lines it holds", () => {
    expect(pasteLabel(lines(84))).toBe("Pasted text · 84 lines");
    expect(pasteLabel(lines(PASTE_LINES))).toBe("Pasted text · 12 lines");
  });

  test("says how many characters a few long lines hold", () => {
    expect(pasteLabel("x".repeat(3200))).toBe("Pasted text · 3.2k chars");
    expect(pasteLabel("x".repeat(PASTE_CHARS))).toBe("Pasted text · 1.2k chars");
    expect(pasteLabel(`a\n${"x".repeat(12000)}`)).toBe("Pasted text · 12k chars");
  });
});
