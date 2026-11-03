import { describe, expect, test } from "bun:test";
import type { Element, ElementContent, Root } from "hast";
import {
  advance,
  birthOf,
  newRevealState,
  REVEAL_MAX_DELAY_MS,
  REVEAL_MS,
  REVEAL_STEP_MS,
  type RevealState,
  rehypeReveal,
  stillFading,
} from "./reveal";

const text = (value: string): ElementContent => ({ type: "text", value });
const element = (tagName: string, children: ElementContent[]): Element =>
  ({ type: "element", tagName, properties: {}, children }) as Element;

/** One render of a block at `now`, the way Markdown.tsx renders the tail. */
function render(state: RevealState, source: string, children: ElementContent[], now: number) {
  const next = advance(state, source);
  const tree: Root = { type: "root", children };
  rehypeReveal({ state: next, now })(tree);
  return { tree, state: next };
}

const spans = (node: Root | Element): { word: string; delay: string }[] =>
  node.children.flatMap((child) => {
    if (child.type !== "element") return [];
    if (child.tagName === "span" && (child.properties.className as string[]).includes("rc-reveal"))
      return [
        { word: (child.children[0] as { value: string }).value, delay: String(child.properties.style) },
      ];
    return spans(child);
  });

describe("birth times", () => {
  test("new words are staggered 18 ms apart and capped at 180 ms", () => {
    const births: number[] = [];
    const fresh = { count: 0 };
    const got = Array.from({ length: 20 }, (_, i) => birthOf(births, i, 1000, fresh));
    expect(got.slice(0, 3)).toEqual([1000, 1000 + REVEAL_STEP_MS, 1000 + 2 * REVEAL_STEP_MS]);
    expect(got.at(-1)).toBe(1000 + REVEAL_MAX_DELAY_MS);
  });

  test("a known word keeps its birth", () => {
    const births = [500];
    expect(birthOf(births, 0, 2000, { count: 0 })).toBe(500);
  });

  test("a word fades for the length of the animation, then is plain", () => {
    expect(stillFading(1000, 1000 + REVEAL_MS - 1)).toBe(true);
    expect(stillFading(1000, 1000 + REVEAL_MS)).toBe(false);
  });

  test("a block that only grew keeps its births; a rewrite starts over", () => {
    const grown = advance({ text: "Hello", births: [1] }, "Hello wor");
    expect(grown.births).toEqual([1]);
    expect(advance({ text: "Hello wor", births: [1, 2] }, "Goodbye").births).toEqual([]);
  });
});

describe("wrapping words across chunks", () => {
  test("a word still fading in the next chunk carries on with a negative delay", () => {
    const first = render(newRevealState(), "one two", [element("p", [text("one two")])], 1000);
    expect(spans(first.tree)).toEqual([
      { word: "one", delay: "animation-delay:0ms" },
      { word: "two", delay: `animation-delay:${REVEAL_STEP_MS}ms` },
    ]);
    // 100 ms later "three" arrives: "one" is 100 ms into its fade, "two" 82 ms, "three" starts.
    const second = render(first.state, "one two three", [element("p", [text("one two three")])], 1100);
    expect(spans(second.tree)).toEqual([
      { word: "one", delay: "animation-delay:-100ms" },
      { word: "two", delay: `animation-delay:${REVEAL_STEP_MS - 100}ms` },
      { word: "three", delay: "animation-delay:0ms" },
    ]);
  });

  test("words that finished fading are plain text", () => {
    const first = render(newRevealState(), "one", [element("p", [text("one")])], 0);
    const later = render(first.state, "one two", [element("p", [text("one two")])], 1000);
    const p = later.tree.children[0] as Element;
    expect(p.children[0]).toEqual(text("one "));
    expect(spans(later.tree).map((s) => s.word)).toEqual(["two"]);
  });

  test("a finished block is left exactly as it was", () => {
    const first = render(newRevealState(), "one two", [element("p", [text("one two")])], 0);
    const done = render(first.state, "one two", [element("p", [text("one two")])], 5000);
    expect(done.tree.children).toEqual([element("p", [text("one two")])]);
  });

  test("a word finished by the next chunk keeps its place and its fade", () => {
    const first = render(newRevealState(), "wor", [element("p", [text("wor")])], 0);
    const second = render(first.state, "world", [element("p", [text("world")])], 50);
    expect(spans(second.tree)).toEqual([{ word: "world", delay: "animation-delay:-50ms" }]);
  });

  test("a link's text is counted but never split, so a bare URL stays one child", () => {
    const tree = render(
      newRevealState(),
      "see https://x.dev now",
      [element("p", [text("see "), element("a", [text("https://x.dev")]), text(" now")])],
      0,
    ).tree;
    const a = (tree.children[0] as Element).children.find(
      (c) => c.type === "element" && c.tagName === "a",
    ) as Element;
    expect(a.children).toEqual([text("https://x.dev")]);
    // "now" is the third word: the link's word still counted, so the stagger lines up.
    expect(spans(tree)).toEqual([
      { word: "see", delay: "animation-delay:0ms" },
      { word: "now", delay: `animation-delay:${2 * REVEAL_STEP_MS}ms` },
    ]);
  });

  test("code is never split", () => {
    const tree = render(
      newRevealState(),
      "x",
      [element("p", [text("ab")]), element("pre", [element("code", [text("12 34")])])],
      0,
    ).tree;
    expect((tree.children[1] as Element).children).toEqual([element("code", [text("12 34")])]);
  });

  test("a block with no words fades in whole once, then stays plain", () => {
    const code = () => [element("pre", [element("code", [text("x = 1")])])];
    const first = render(newRevealState(), "```\nx = 1", code(), 0);
    expect((first.tree.children[0] as Element).properties.className).toEqual(["rc-reveal-block"]);
    const later = render(first.state, "```\nx = 1\n", code(), 1000);
    expect((later.tree.children[0] as Element).properties.className).toBeUndefined();
  });

  test("words a rewrite removed are forgotten", () => {
    const first = render(newRevealState(), "a b c", [element("p", [text("a b c")])], 0);
    expect(first.state.births).toHaveLength(3);
    const shrunk = render(first.state, "a b c", [element("p", [text("a b")])], 10);
    expect(shrunk.state.births).toHaveLength(2);
  });
});
