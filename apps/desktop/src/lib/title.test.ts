import { expect, test } from "bun:test";
import { childTitle, titleFrom } from "./title";

test("a long first line is cut at a word, with an ellipsis", () => {
  const prompt = "Add a subtract(a, b) function to calc.py and a tests/test_calc.py with pytest tests";
  expect(titleFrom(prompt, 60)).toBe("Add a subtract(a, b) function to calc.py and a…");
  expect(titleFrom("Short one\nsecond line", 60)).toBe("Short one");
  expect(titleFrom("x".repeat(70), 60)).toBe(`${"x".repeat(59)}…`);
});

test("a message that opens with a blank line still names its thread", () => {
  // A child sent "From thread …" never becomes the thread's title.
  expect(titleFrom("\n\nFix the sidebar inset", 60)).toBe("Fix the sidebar inset");
  expect(titleFrom("\n   \n", 60)).toBe("");
});

test("a cut inside an emoji drops it whole, so a title never ends in half a character", () => {
  const family = "👨‍👩‍👧‍👦";
  // 58 units, then an 11-unit family: the cut at 59 lands inside it.
  expect(titleFrom(`${"x".repeat(58)}${family} and more`, 60)).toBe(`${"x".repeat(58)}…`);
  // Easy to miss: an emoji just before the cut is kept whole.
  expect(titleFrom(`${"x".repeat(50)}🚀${"y".repeat(20)}`, 60)).toBe(`${"x".repeat(50)}🚀${"y".repeat(7)}…`);
});

test("a child is named by its parent, or by the task when the parent gave no title", () => {
  expect(childTitle("Fix the inset", "do whatever")).toBe("Fix the inset");
  expect(childTitle(undefined, "Add a subtract(a, b) function to calc.py")).toBe(
    "Add a subtract(a, b) function to calc.py",
  );
  // Never empty: an empty title would leave the delivered message ("From thread …") as the name.
  expect(childTitle("   ", "\n\nFix the sidebar inset")).toBe("Fix the sidebar inset");
  expect(childTitle("", "\n\nFix the sidebar inset")).toBe("Fix the sidebar inset");
});
