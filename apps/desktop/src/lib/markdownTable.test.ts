import { describe, expect, test } from "bun:test";
import { isFigure, numericColumns, toCsv, toMarkdown } from "./markdownTable";

const table = {
  header: ["Test", "Time", "Share"],
  rows: [
    ["parse | split", "1,204 ms", "42%"],
    ["render", "88 ms", "—"],
    ['say "hi", twice', "3 s", "7%"],
  ],
};

describe("markdown tables", () => {
  test("figures are numbers with units, not words", () => {
    for (const figure of ["42", "1,204 ms", "-3.5%", "$12", "~2x", "128 tokens"])
      expect(isFigure(figure)).toBe(true);
    for (const word of ["render", "v2 build", "", "12 tests failed"]) expect(isFigure(word)).toBe(false);
  });

  test("a column of figures is numeric, dashes ignored", () => {
    expect(numericColumns(table)).toEqual([false, true, true]);
  });

  test("Markdown escapes pipes and right-aligns figure columns", () => {
    expect(toMarkdown(table)).toBe(
      [
        "| Test | Time | Share |",
        "| --- | ---: | ---: |",
        "| parse \\| split | 1,204 ms | 42% |",
        "| render | 88 ms | — |",
        '| say "hi", twice | 3 s | 7% |',
      ].join("\n"),
    );
  });

  test("CSV quotes commas and quotes", () => {
    expect(toCsv(table).split("\r\n")).toEqual([
      "Test,Time,Share",
      'parse | split,"1,204 ms",42%',
      "render,88 ms,—",
      '"say ""hi"", twice",3 s,7%',
    ]);
  });
});
