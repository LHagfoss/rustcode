import { expect, test } from "bun:test";
import { hasLongLine } from "./codeWrap";

test("only a block with a line past the column offers wrapping", () => {
  expect(hasLongLine("")).toBe(false);
  expect(hasLongLine("short\nlines\nonly")).toBe(false);
  expect(hasLongLine(`${"a".repeat(90)}\n${"b".repeat(90)}`)).toBe(false);
  expect(hasLongLine(`first\n${"x".repeat(91)}\nlast`)).toBe(true);
  expect(hasLongLine(`first\n${"x".repeat(91)}`)).toBe(true);
});
