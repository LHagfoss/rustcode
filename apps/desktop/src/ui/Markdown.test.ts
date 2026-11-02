import { describe, expect, test } from "bun:test";
import { renderBlock } from "./Markdown";

describe("markdown render cache", () => {
  test("the same block text gives the same element tree, so it is parsed once", () => {
    const a = renderBlock("Some **bold** text.");
    expect(renderBlock("Some **bold** text.")).toBe(a);
    expect(renderBlock("Other text.")).not.toBe(a);
  });
});

describe("pictures in replies", () => {
  test("file paths go to the shell; web and data pictures do not; other schemes are refused", async () => {
    const { picturePath } = await import("./MarkdownMedia");
    const { urlTransform } = await import("./Markdown");
    expect(picturePath("docs/shot.png")).toBe("docs/shot.png");
    expect(picturePath("/Users/me/repo/a%20b.png")).toBe("/Users/me/repo/a b.png");
    expect(picturePath("file:///Users/me/repo/a.png")).toBe("/Users/me/repo/a.png");
    expect(picturePath("https://example.com/a.png")).toBeNull();
    expect(picturePath("data:image/png;base64,AAAA")).toBeNull();
    expect(picturePath("javascript:alert(1)")).toBeNull();
    expect(urlTransform("file:///a.png", "src")).toBe("file:///a.png");
    expect(urlTransform("file:///a.png", "href")).toBe("");
    expect(urlTransform("javascript:alert(1)", "src")).toBe("");
  });
});
