// Inline code in a reply copies its own text when pressed, but a press is also how a drag across
// the words ends: by then the reply is selected, and copying would take the code the person did
// not point at while throwing that selection away. So a press copies only when it left nothing
// selected behind.

/** The part of a `Selection` the rule reads, so a test can hand it a plain object. */
export type PressSelection = { isCollapsed: boolean; rangeCount: number } | null;

/**
 * True when a press ended with nothing selected, so it was a press on the code and not the end of
 * a drag. `window.getSelection()` is a live selection, never a copy: read it inside the handler.
 */
export function copiesOnClick(selection: PressSelection): boolean {
  // No selection to keep (a webview that keeps none), one that holds no range at all, or a
  // collapsed caret where the press landed: all three are a plain press.
  return selection === null || selection.rangeCount === 0 || selection.isCollapsed;
}
