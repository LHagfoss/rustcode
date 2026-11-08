// Focus rings follow the keyboard, not `:focus-visible` alone. WebKit and WebView2 match
// `:focus-visible` after a click too: on every text field, and on a control focused from script
// (Radix handing focus back to a trigger when its menu closes, our own `.focus()` after Enter or
// Escape). So <html> carries `data-keyboard-nav` from a Tab until the next pointer press, and every
// ring in index.css is drawn under it (docs/design.md › Focus). Tab itself is never taken over.

export const KEYBOARD_NAV = "data-keyboard-nav";

type Root = Pick<Element, "setAttribute" | "removeAttribute">;

/** Starts tracking on `target` (the window); returns the function that stops it. */
export function installFocusModality(
  root: Root = document.documentElement,
  target: EventTarget = window,
): () => void {
  const keyboard = (event: Event) => {
    if ((event as KeyboardEvent).key === "Tab") root.setAttribute(KEYBOARD_NAV, "");
  };
  const pointer = () => root.removeAttribute(KEYBOARD_NAV);
  // Capture, so a handler that stops propagation still leaves the mode right.
  target.addEventListener("keydown", keyboard, true);
  target.addEventListener("pointerdown", pointer, true);
  target.addEventListener("mousedown", pointer, true);
  return () => {
    target.removeEventListener("keydown", keyboard, true);
    target.removeEventListener("pointerdown", pointer, true);
    target.removeEventListener("mousedown", pointer, true);
  };
}
