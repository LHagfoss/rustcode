// Which edges of a scrolling field fade, from where it is scrolled to. The composer's draft
// scrolls inside its own box once it is taller than the box allows; the edge it is cut at fades so
// a line never ends on a hard edge (`.rc-field-fade` in index.css). Pure: no DOM, no React.

/** How far the fade runs, in px: enough to soften a cut line, not so much that two lines dim. */
export const FIELD_FADE = 20;

/** How close to an edge counts as being at it: a scrolled field parks a fraction of a px away. */
const EDGE = 1;

export type FieldFade = "none" | "top" | "bottom" | "both";

/**
 * The edges to fade: the top once the field is scrolled down, the bottom while there is text
 * below. Neither when the draft fits, so a short message never dims. A field scrolled to its very
 * end is at the bottom however its `scrollTop` rounded, or the last line being typed would dim.
 */
export function fieldFade(scrollTop: number, scrollHeight: number, clientHeight: number): FieldFade {
  const top = scrollTop > EDGE;
  const bottom = scrollHeight - clientHeight - scrollTop > EDGE;
  return top ? (bottom ? "both" : "top") : bottom ? "bottom" : "none";
}
