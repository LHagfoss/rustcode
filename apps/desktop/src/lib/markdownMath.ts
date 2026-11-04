// remark-math hands formulas over as `math` and `inlineMath` nodes, which have
// no HTML of their own. This plugin turns them into code nodes carrying a
// language, so `Markdown.tsx`'s one `code` component draws every formula (and
// loads KaTeX only when one appears).

import remarkMath from "remark-math";

type MathNode = { type: string; value?: string; lang?: string; children?: MathNode[] };

/**
 * `$$…$$` (and `\(…\)`) is math; a lone `$` is money, so this turns
 * `singleDollarTextMath` off: "$5 and $10" stays a sentence.
 */
export function remarkMoneySafeMath(this: ThisParameterType<typeof remarkMath>) {
  return remarkMath.call(this, { singleDollarTextMath: false });
}

/** Turns the parsed formulas into `code` nodes the renderer already draws. */
export function remarkMathAsCode() {
  const convert = (node: MathNode) => {
    if (!node.children) return;
    node.children = node.children.map((child) => {
      if (child.type === "math" || child.type === "inlineMath")
        return {
          type: "code",
          lang: child.type === "math" ? "math" : "inline-math",
          value: child.value ?? "",
        };
      convert(child);
      return child;
    });
  };
  return (tree: MathNode) => convert(tree);
}
