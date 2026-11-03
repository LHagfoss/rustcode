// Per-word reveal for the reply that is still being written: which words the
// last chunk added, and how long each waits before it fades in. Markdown.tsx
// wraps them in spans; words that were already on screen are left as text, so
// they never fade twice.

import type { Element, ElementContent, Root } from "hast";

/** Words in one chunk fade 18 ms apart, and none waits past 180 ms, so a big chunk still lands. */
export const REVEAL_STEP_MS = 18;
export const REVEAL_MAX_DELAY_MS = 180;

export type RevealRun = {
  /** Character range of the word in the text it came from. */
  from: number;
  to: number;
  /** Animation delay, in ms. */
  delay: number;
};

/**
 * The words of `text` that start at or after `revealed`, with their stagger. A
 * word the last chunk already started is not new: it keeps its place, and only
 * the letters added to it appear, so nothing on screen fades in twice.
 */
export function revealRuns(text: string, revealed: number, first = 0): RevealRun[] {
  const runs: RevealRun[] = [];
  let index = first;
  for (const match of text.matchAll(/\S+/g)) {
    const from = match.index;
    if (from < revealed) continue;
    runs.push({
      from,
      to: from + match[0].length,
      delay: Math.min(index * REVEAL_STEP_MS, REVEAL_MAX_DELAY_MS),
    });
    index += 1;
  }
  return runs;
}

/**
 * How much of the block was on screen last frame. A block that only grew keeps
 * it; one the model rewrote starts over, since none of it is where it was.
 */
export function revealedBefore(prev: string, wasRevealed: number, next: string): number {
  if (!next.startsWith(prev)) return 0;
  return Math.min(wasRevealed, next.length);
}

/** Tags whose text is code: a span per word would change how it copies and highlights. */
const CODE = new Set(["pre", "code", "style", "script"]);

/** Everything drawn on screen, in document order, as the last render left it. */
type Walk = { chars: number; words: number };

/**
 * A rehype plugin that wraps the words arriving after `revealed` in a
 * `.rc-reveal` span, staggered by `animation-delay`, and reports the block's
 * rendered length so the next chunk knows where this one ended.
 */
export function rehypeReveal({
  revealed,
  onRendered,
}: {
  revealed: number;
  onRendered?: (chars: number) => void;
}) {
  return (tree: Root) => {
    const walk: Walk = { chars: 0, words: 0 };
    reveal(tree, revealed, walk, false);
    onRendered?.(walk.chars);
  };
}

function reveal(parent: Root | Element, revealed: number, walk: Walk, code: boolean) {
  const out: ElementContent[] = [];
  for (const child of parent.children) {
    if (child.type === "text") {
      const start = walk.chars;
      walk.chars += child.value.length;
      const runs = code ? [] : revealRuns(child.value, revealed - start, walk.words);
      walk.words += runs.length;
      if (runs.length) out.push(...wrap(child.value, runs));
      else out.push(child);
      continue;
    }
    if (child.type === "element") reveal(child, revealed, walk, code || CODE.has(child.tagName));
    // Anything else (a comment) is left where it is.
    out.push(child as ElementContent);
  }
  (parent as { children: ElementContent[] }).children = out;
}

function wrap(text: string, runs: RevealRun[]): ElementContent[] {
  const out: ElementContent[] = [];
  let at = 0;
  for (const run of runs) {
    if (run.from > at) out.push({ type: "text", value: text.slice(at, run.from) });
    out.push({
      type: "element",
      tagName: "span",
      properties: { className: ["rc-reveal"], style: `animation-delay:${run.delay}ms` },
      children: [{ type: "text", value: text.slice(run.from, run.to) }],
    });
    at = run.to;
  }
  if (at < text.length) out.push({ type: "text", value: text.slice(at) });
  return out;
}
