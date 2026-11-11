import { describe, expect, test } from "bun:test";
import { answerPreview } from "./answerPreview";

describe("a delegate's one-line preview", () => {
  test("takes the first prose line of the reply, not the table below it", () => {
    expect(answerPreview("Here is the machine:\n\n| Item | Value |\n|---|---|\n| CPU | M3 |")).toBe(
      "Here is the machine:",
    );
  });

  test("reads an answer that is only a table as its first row's cells, with no pipes", () => {
    expect(answerPreview("| Item | Value |\n| --- | --- |\n| CPU | M3 |")).toBe("Item · Value");
  });

  test("drops heading, list and emphasis markup and keeps a link's words", () => {
    expect(answerPreview("## **Done**: see [the docs](https://example.com)")).toBe("Done: see the docs");
    expect(answerPreview("- `cargo test` passes")).toBe("cargo test passes");
  });

  test("is undefined for an empty answer", () => {
    expect(answerPreview("")).toBeUndefined();
    expect(answerPreview("\n  \n")).toBeUndefined();
    expect(answerPreview(undefined)).toBeUndefined();
  });
});
