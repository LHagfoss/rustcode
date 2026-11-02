import { describe, expect, test } from "bun:test";
import { markdownBlocks } from "./markdownBlocks";

describe("markdown blocks", () => {
  test("splits paragraphs and headings at blank lines", () => {
    expect(markdownBlocks("# Title\n\nOne.\n\nTwo\nlines.")).toEqual(["# Title", "One.", "Two\nlines."]);
  });

  test("keeps a fenced block whole, blank lines and all, even unclosed while it streams", () => {
    const text = "Before.\n\n```ts\nconst a = 1;\n\nconst b = 2;\n```\n\nAfter.";
    expect(markdownBlocks(text)).toEqual(["Before.", "```ts\nconst a = 1;\n\nconst b = 2;\n```", "After."]);
    expect(markdownBlocks("Before.\n\n```ts\nconst a = 1;\n\nconst")).toEqual([
      "Before.",
      "```ts\nconst a = 1;\n\nconst",
    ]);
  });

  test("keeps loose lists, indented continuations and tables in one block", () => {
    expect(markdownBlocks("- a\n\n- b\n\n  more b\n\n1. c")).toEqual(["- a\n\n- b\n\n  more b\n\n1. c"]);
    expect(markdownBlocks("| a |\n|---|\n\n| b |")).toEqual(["| a |\n|---|\n\n| b |"]);
  });

  test("a reference definition anywhere keeps the text whole", () => {
    const text = "See [x].\n\nMore.\n\n[x]: https://example.com";
    expect(markdownBlocks(text)).toEqual([text]);
  });

  test("blocks rejoined are the text without the blank lines between them", () => {
    const text = "A\n\n\n\nB\n\n```\nc\n```";
    expect(markdownBlocks(text).join("\n\n")).toBe("A\n\nB\n\n```\nc\n```");
  });
});
