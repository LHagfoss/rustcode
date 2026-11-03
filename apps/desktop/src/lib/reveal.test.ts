import { describe, expect, test } from "bun:test";
import type { Element, ElementContent, Root } from "hast";
import { rehypeReveal, revealedBefore, revealRuns } from "./reveal";

describe("which words are new", () => {
  test("words before the revealed offset are left alone, the rest fade in order", () => {
    // "one two three": "one" and "two" were already on screen.
    expect(revealRuns("one two three", 7)).toEqual([{ from: 8, to: 13, delay: 0 }]);
    expect(revealRuns("one two three", 0).map((r) => r.from)).toEqual([0, 4, 8]);
  });

  test("a word the last chunk started is not new: it keeps its place", () => {
    // "wor" was on screen; the chunk finished it, so only "and" and "on" arrive.
    const runs = revealRuns("world and on", 3);
    expect(runs.map((r) => r.from)).toEqual([6, 10]);
  });

  test("delays step 18 ms and stop at 180 ms, so a big chunk does not lag", () => {
    const text = Array.from({ length: 20 }, (_, i) => `w${i}`).join(" ");
    const delays = revealRuns(text, 0).map((r) => r.delay);
    expect(delays.slice(0, 3)).toEqual([0, 18, 36]);
    expect(delays[10]).toBe(180);
    expect(delays.at(-1)).toBe(180);
  });

  test("whitespace is never a word", () => {
    expect(revealRuns("   \n\n ", 0)).toEqual([]);
  });
});

describe("how much of the block was on screen", () => {
  test("a block that only grew keeps its offset, clamped to its length", () => {
    expect(revealedBefore("Hello", 5, "Hello wor")).toBe(5);
    expect(revealedBefore("Hello", 5, "Hello")).toBe(5);
    expect(revealedBefore("Hello", 99, "Hello wor")).toBe(9);
  });

  test("a rewritten block starts over", () => {
    expect(revealedBefore("Hello wor", 9, "Hello world")).toBe(9);
    expect(revealedBefore("Hello wor", 9, "Goodbye")).toBe(0);
  });
});

const text = (value: string): ElementContent => ({ type: "text", value });
const element = (tagName: string, children: ElementContent[]): Element =>
  ({ type: "element", tagName, properties: {}, children }) as Element;

function run(children: ElementContent[], revealed: number) {
  const tree: Root = { type: "root", children };
  let chars = -1;
  rehypeReveal({ revealed, onRendered: (n) => (chars = n) })(tree);
  return { tree, chars };
}

describe("wrapping the new words", () => {
  test("only the new words get a span, each with its own delay", () => {
    const { tree, chars } = run([element("p", [text("one two three")])], 4);
    const p = tree.children[0] as Element;
    expect(p.children).toEqual([
      text("one "),
      {
        type: "element",
        tagName: "span",
        properties: { className: ["rc-reveal"], style: "animation-delay:0ms" },
        children: [text("two")],
      },
      text(" "),
      {
        type: "element",
        tagName: "span",
        properties: { className: ["rc-reveal"], style: "animation-delay:18ms" },
        children: [text("three")],
      },
    ]);
    expect(chars).toBe(13);
  });

  test("a finished block is left exactly as it was", () => {
    const { tree, chars } = run([element("p", [text("one two")])], 7);
    expect(tree.children).toEqual([element("p", [text("one two")])]);
    expect(chars).toBe(7);
  });

  test("code is counted but never split, and the offsets after it still line up", () => {
    const { tree, chars } = run(
      [
        element("p", [text("ab")]),
        element("pre", [element("code", [text("1234")])]),
        element("p", [text("cd")]),
      ],
      6,
    );
    expect((tree.children[1] as Element).children).toEqual([element("code", [text("1234")])]);
    // "cd" starts at 6, where the last chunk ended: it is new, and it fades from now.
    expect((tree.children[2] as Element).children[0]).toMatchObject({
      tagName: "span",
      properties: { style: "animation-delay:0ms" },
    });
    expect(chars).toBe(8);
  });
});
