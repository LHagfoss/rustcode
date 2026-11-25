import { expect, test } from "bun:test";
import { QUOTE_MAX_CHARS, quoteMarkdown } from "./quoteSelection";

test("each marked line becomes a quoted line", () => {
  expect(quoteMarkdown("one line")).toBe("> one line");
  expect(quoteMarkdown("  first\nsecond  \n")).toBe("> first\n> second");
});

test("blank lines inside stay as one quoted gap, outer ones go", () => {
  expect(quoteMarkdown("\n\nfirst\n\n\n\nsecond\n\n")).toBe("> first\n>\n> second");
  expect(quoteMarkdown("a\r\n\r\nb")).toBe("> a\n>\n> b");
});

test("nothing marked but whitespace quotes nothing", () => {
  expect(quoteMarkdown("  \n \n ")).toBe("");
});

test("a long selection is cut with an ellipsis", () => {
  const quoted = quoteMarkdown("x".repeat(QUOTE_MAX_CHARS + 50));
  expect(quoted).toBe(`> ${"x".repeat(QUOTE_MAX_CHARS)}…`);
});
