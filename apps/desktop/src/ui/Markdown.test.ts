import { describe, expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { renderBlock } from "./Markdown";

/** One block of a reply, drawn by the components the transcript uses. */
const drawn = (text: string) => renderToStaticMarkup(renderBlock(text));

/** The class list of every `<li>` in a drawn block, in order. */
const items = (markup: string) => [...markup.matchAll(/<li class="([^"]*)"/g)].map((m) => m[1] ?? "");

describe("markdown render cache", () => {
  test("the same block text gives the same element tree, so it is parsed once", () => {
    const a = renderBlock("Some **bold** text.");
    expect(renderBlock("Some **bold** text.")).toBe(a);
    expect(renderBlock("Other text.")).not.toBe(a);
  });
});

describe("blocks in a reply", () => {
  test("a fenced block sits on the code surface, as diffs and diagrams do", () => {
    expect(drawn("```ts\nlet a = 1;\n```")).toContain("bg-code-bg");
  });

  test("a task item's box is its marker, with no bullet beside it", () => {
    const out = drawn("- [x] done\n- plain");
    const [task, plain] = items(out);
    expect(out).toContain('type="checkbox"');
    expect(task).toContain("list-none");
    expect(plain).not.toContain("list-none");
  });

  test("a nested list steps its marker instead of only indenting", () => {
    const out = drawn("- a\n  - b\n\n1. x\n   1. y");
    for (const marker of ["list-[circle]", "list-[square]", "list-[lower-alpha]", "list-[lower-roman]"]) {
      expect(out).toContain(marker);
    }
  });

  test("rows inside a table are parted, the last one is not", () => {
    const out = drawn("| a |\n| - |\n| 1 |\n| 2 |");
    // The header and both body rows, each closing on a hairline except the last.
    expect(out.match(/border-b border-border\/60 last:border-0/g)).toHaveLength(3);
  });

  test("bold is the semibold titles use, not the browser's 700", () => {
    expect(drawn("**bold**")).toContain('<strong class="font-semibold">');
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
