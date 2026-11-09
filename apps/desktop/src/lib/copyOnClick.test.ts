import { describe, expect, test } from "bun:test";
import { copiesOnClick } from "./copyOnClick";

describe("a press on inline code", () => {
  test("copies when it left the caret alone", () => {
    // A collapsed caret where the press landed is what a webview gives a plain press.
    expect(copiesOnClick({ isCollapsed: true, rangeCount: 1 })).toBe(true);
    // A webview that keeps no selection at all is the same press.
    expect(copiesOnClick(null)).toBe(true);
    expect(copiesOnClick({ isCollapsed: true, rangeCount: 0 })).toBe(true);
  });

  test("does not copy when it ended a drag that selected the words", () => {
    expect(copiesOnClick({ isCollapsed: false, rangeCount: 1 })).toBe(false);
  });
});
