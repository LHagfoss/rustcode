// Focus rings follow the keyboard, not `:focus-visible` alone. WebKit and WebView2 match
// `:focus-visible` after a click too: on every text field, and on a control focused from script
// (Radix handing focus back to a trigger when its menu closes, our own `.focus()` after Enter or
// Escape). So <html> carries `data-keyboard-nav` from a Tab until the next pointer press, and every
// ring in index.css is drawn under it (docs/design.md › Focus). Tab is suppressed outside
// editing and modal contexts unless the user enables navigation between controls.

const TAB_FOCUS = "rustcode.tabMovesFocus";
export function tabMovesFocus(): boolean {
  try {
    return localStorage.getItem(TAB_FOCUS) === "1";
  } catch {
    return false;
  }
}
export function setTabMovesFocus(value: boolean): void {
  try {
    localStorage.setItem(TAB_FOCUS, value ? "1" : "0");
  } catch {
    /* Storage unavailable. */
  }
}
/** A bare Tab or Shift+Tab; Ctrl, Cmd and Alt with Tab belong to shortcuts and the system. */
export function shouldBlockTab(
  key: string,
  enabled: boolean,
  protectedContext: boolean,
  modified = false,
): boolean {
  return key === "Tab" && !enabled && !protectedContext && !modified;
}

export const KEYBOARD_NAV = "data-keyboard-nav";

type Root = Pick<Element, "setAttribute" | "removeAttribute">;

/** Starts tracking on `target` (the window); returns the function that stops it. */
export function installFocusModality(
  root: Root = document.documentElement,
  target: EventTarget = window,
  enabled: () => boolean = tabMovesFocus,
): () => void {
  const keyboard = (event: Event) => {
    const { key, ctrlKey, metaKey, altKey } = event as KeyboardEvent;
    const element = event.target;
    const protectedContext =
      typeof Element !== "undefined" &&
      element instanceof Element &&
      !!element.closest(
        'input, textarea, select, [contenteditable]:not([contenteditable="false"]), form, dialog, [role="dialog"], [role="alertdialog"], [aria-modal="true"], [role="menu"], [role="menuitem"], [role="listbox"], [data-radix-popper-content-wrapper]',
      );
    if (shouldBlockTab(key, enabled(), protectedContext, ctrlKey || metaKey || altKey)) {
      event.preventDefault();
      root.removeAttribute(KEYBOARD_NAV);
    } else if (key === "Tab") root.setAttribute(KEYBOARD_NAV, "");
  };
  const pointer = () => root.removeAttribute(KEYBOARD_NAV);
  // Capture, so a handler that stops propagation still leaves the mode right.
  target.addEventListener("keydown", keyboard, true);
  target.addEventListener("pointerdown", pointer, true);
  target.addEventListener("mousedown", pointer, true);
  // Listen on the window itself. Capturing blur also sees ordinary focus moves between elements
  // (including into and out of an iframe), which would turn keyboard mode off during Tab navigation.
  target.addEventListener("blur", pointer);
  return () => {
    target.removeEventListener("keydown", keyboard, true);
    target.removeEventListener("pointerdown", pointer, true);
    target.removeEventListener("mousedown", pointer, true);
    target.removeEventListener("blur", pointer);
  };
}
