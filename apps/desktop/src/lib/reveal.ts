// Per-word reveal for the reply that is still being written. Each word of the
// block being written gets a birth time the first time it is drawn; while it is
// younger than the fade it is wrapped in a `.rc-reveal` span whose
// `animation-delay` is `birth - now`, so a word that started fading one chunk
// ago carries on where it was instead of restarting or snapping to full
// opacity. Older words are plain text. Markdown.tsx restarts the spans' CSS
// animations after each render, which is what makes the negative delay land
// at the right point.

import type { Element, ElementContent, Root } from "hast";

/** Words that arrive together fade 18 ms apart, and none waits past 180 ms, so a big chunk still lands. */
export const REVEAL_STEP_MS = 18;
export const REVEAL_MAX_DELAY_MS = 180;
/** How long one word's fade runs (`.rc-reveal` in index.css). */
export const REVEAL_MS = 280;
/** How long a block with no words (code, a table, a picture) fades in once (`.rc-reveal-block`). */
export const REVEAL_BLOCK_MS = 180;

/**
 * What the reveal remembers about one block between renders: its text, and
 * when each of its words (by order on screen) was first drawn.
 */
export type RevealState = {
  text: string;
  births: number[];
  /** When the block was first drawn, for a block with no words to reveal. */
  born?: number;
};

export function newRevealState(): RevealState {
  return { text: "", births: [] };
}

/**
 * Prepares `state` for `text`: a block that only grew keeps its words' birth
 * times; one the model rewrote starts over, since none of it is where it was.
 */
export function advance(state: RevealState, text: string): RevealState {
  if (text.startsWith(state.text)) return { ...state, text, births: [...state.births] };
  return { text, births: [] };
}

/**
 * The birth time of the word at `ordinal`, giving a new word one now: words
 * arriving together are staggered by their order among the new ones.
 */
export function birthOf(births: number[], ordinal: number, now: number, fresh: { count: number }): number {
  const known = births[ordinal];
  if (known !== undefined) return known;
  const birth = now + Math.min(fresh.count * REVEAL_STEP_MS, REVEAL_MAX_DELAY_MS);
  fresh.count += 1;
  births[ordinal] = birth;
  return birth;
}

/** Whether a word born at `birth` is still fading (or waiting to) at `now`. */
export function stillFading(birth: number, now: number, duration = REVEAL_MS): boolean {
  return now - birth < duration;
}

/** Tags whose text is code: a span per word would change how it copies and highlights. */
const CODE = new Set(["pre", "code", "style", "script"]);
/**
 * Tags whose text is counted but never split: a link's label must stay its
 * only child, or a bare URL draws as a long link for one chunk and then as a
 * chip (Markdown.tsx `a`).
 */
const WHOLE = new Set(["a"]);

type Walk = { words: number; fresh: { count: number } };

/**
 * A rehype plugin that wraps the words still fading in `.rc-reveal` spans and
 * records new words' birth times in `state`. A block with no words at all
 * fades in whole, once, through `.rc-reveal-block` on its top elements.
 */
export function rehypeReveal({ state, now }: { state: RevealState; now: number }) {
  return (tree: Root) => {
    const walk: Walk = { words: 0, fresh: { count: 0 } };
    reveal(tree, state, now, walk, "split");
    // Words the last render had beyond this one are gone (a rewrite of the tail).
    state.births.length = walk.words;
    if (walk.words === 0) {
      state.born ??= now;
      if (stillFading(state.born, now, REVEAL_BLOCK_MS))
        for (const child of tree.children)
          if (child.type === "element") addClass(child, "rc-reveal-block", state.born - now);
    }
  };
}

type Mode = "split" | "whole" | "skip";

function reveal(parent: Root | Element, state: RevealState, now: number, walk: Walk, mode: Mode) {
  const out: ElementContent[] = [];
  for (const child of parent.children) {
    if (child.type === "text") {
      if (mode === "skip") out.push(child);
      else if (mode === "whole") {
        // Counted, so the words after a link keep their order, but drawn as they are.
        for (const _ of child.value.matchAll(/\S+/g)) birthOf(state.births, walk.words++, now, walk.fresh);
        out.push(child);
      } else out.push(...splitWords(child.value, state, now, walk));
      continue;
    }
    if (child.type === "element") {
      const next: Mode =
        mode === "skip" || CODE.has(child.tagName)
          ? "skip"
          : mode === "whole" || WHOLE.has(child.tagName)
            ? "whole"
            : "split";
      reveal(child, state, now, walk, next);
    }
    // Anything else (a comment) is left where it is.
    out.push(child as ElementContent);
  }
  (parent as { children: ElementContent[] }).children = out;
}

function splitWords(text: string, state: RevealState, now: number, walk: Walk): ElementContent[] {
  const out: ElementContent[] = [];
  let at = 0;
  for (const match of text.matchAll(/\S+/g)) {
    const birth = birthOf(state.births, walk.words++, now, walk.fresh);
    if (!stillFading(birth, now)) continue;
    const from = match.index;
    if (from > at) out.push({ type: "text", value: text.slice(at, from) });
    out.push({
      type: "element",
      tagName: "span",
      properties: { className: ["rc-reveal"], style: `animation-delay:${Math.round(birth - now)}ms` },
      children: [{ type: "text", value: match[0] }],
    });
    at = from + match[0].length;
  }
  if (at === 0) return [{ type: "text", value: text }];
  if (at < text.length) out.push({ type: "text", value: text.slice(at) });
  return out;
}

function addClass(element: Element, name: string, delay: number) {
  const current = element.properties.className;
  const list = Array.isArray(current) ? current : typeof current === "string" ? [current] : [];
  element.properties.className = [...list.map(String), name];
  const style = typeof element.properties.style === "string" ? `${element.properties.style};` : "";
  element.properties.style = `${style}animation-delay:${Math.round(delay)}ms`;
}
