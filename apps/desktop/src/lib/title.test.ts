import { expect, test } from "bun:test";
import { titleFrom } from "./title";

test("a long first line is cut at a word, with an ellipsis", () => {
  const prompt = "Add a subtract(a, b) function to calc.py and a tests/test_calc.py with pytest tests";
  expect(titleFrom(prompt, 60)).toBe("Add a subtract(a, b) function to calc.py and a…");
  expect(titleFrom("Short one\nsecond line", 60)).toBe("Short one");
  expect(titleFrom("x".repeat(70), 60)).toBe(`${"x".repeat(59)}…`);
});
