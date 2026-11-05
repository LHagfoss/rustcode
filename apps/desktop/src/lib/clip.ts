// Cutting display text to a length. `slice` counts UTF-16 units, so a cut can
// land inside an emoji (leaving a lone surrogate, which reads as "�") or
// between a base character and its combining mark. This moves the cut back to
// the character before it, so a title or a label never ends mid-character.

/**
 * How far around the cut the segmenter looks. No character a person sees is
 * this long (a family emoji joined by zero-width joiners is 11 units), and the
 * window keeps a cut cheap: the string is never segmented in full.
 */
const WINDOW = 128;

const segmenter =
  typeof Intl !== "undefined" && typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : undefined;

/**
 * `text` cut to at most `max` UTF-16 units, on a character boundary:
 * `clip("👍👍👍", 5)` is `"👍👍"`, never half of the third one. Text that
 * already fits comes back untouched (and cheaply — this runs per row), and a
 * long string is only looked at around the cut.
 */
export function clip(text: string, max: number): string {
  if (text.length <= max) return text;
  if (max <= 0) return "";
  return text.slice(0, boundary(text, max));
}

/** Where to cut `text` at or before `max`, never inside a character. */
function boundary(text: string, max: number): number {
  if (!segmenter) return outsideSurrogate(text, max);
  const from = Math.max(0, max - WINDOW);
  const to = Math.min(text.length, max + WINDOW);
  for (const { index, segment } of segmenter.segment(text.slice(from, to))) {
    const at = from + index;
    // The character holding the cut starts before it: cut before that character.
    if (at < max && at + segment.length > max) return at;
    // A character starts here, so `max` is already a boundary.
    if (at >= max) break;
  }
  return max;
}

/** Without a segmenter: at least never leave half of an emoji's surrogate pair. */
function outsideSurrogate(text: string, max: number): number {
  const before = text.charCodeAt(max - 1);
  const after = text.charCodeAt(max);
  const pair = before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff;
  return pair ? max - 1 : max;
}
