import { describe, expect, test } from "bun:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown, { type Components } from "react-markdown";
import { remarkMathAsCode, remarkMoneySafeMath } from "./markdownMath";

// What the formula branch of `Markdown.tsx`'s `code` component does, without
// KaTeX: a marker that says whether a node was read as math.
const components: Components = {
  code: ({ className, children }) => {
    const lang = /language-([\w+#.-]+)/.exec(className ?? "")?.[1];
    if (lang !== "math" && lang !== "inline-math") return <>{String(children ?? "")}</>;
    return <i data-math={lang}>{String(children ?? "")}</i>;
  },
};

function render(text: string): string {
  return renderToStaticMarkup(
    React.createElement(ReactMarkdown, {
      // biome-ignore lint/correctness/noChildrenProp: react-markdown reads the markdown as children
      children: text,
      remarkPlugins: [remarkMoneySafeMath, remarkMathAsCode],
      components,
    }),
  );
}

describe("markdown math", () => {
  test("a display formula becomes a math code node", () => {
    expect(render("$$\nx^2 + y^2\n$$")).toContain('data-math="math"');
  });

  test("a formula inside a sentence stays inline", () => {
    expect(render("where $$a+b$$ holds")).toContain('data-math="inline-math"');
  });

  test("dollars that are money are not math", () => {
    const html = render("It costs $5 and $10, or $5.50.");
    expect(html).not.toContain("data-math");
    expect(html).toContain("$5 and $10");
  });

  test("a lone dollar pair on one line is not math either", () => {
    expect(render("from $x to $y")).not.toContain("data-math");
  });

  test("code fences are still code, not formulas", () => {
    const html = render("```ts\nconst a = 1;\n```");
    expect(html).not.toContain("data-math");
    expect(html).toContain("const a = 1;");
  });
});
