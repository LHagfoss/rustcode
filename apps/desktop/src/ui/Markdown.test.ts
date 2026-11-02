import { describe, expect, test } from "bun:test";
import { renderBlock } from "./Markdown";

describe("markdown render cache", () => {
  test("the same block text gives the same element tree, so it is parsed once", () => {
    const a = renderBlock("Some **bold** text.");
    expect(renderBlock("Some **bold** text.")).toBe(a);
    expect(renderBlock("Other text.")).not.toBe(a);
  });
});
