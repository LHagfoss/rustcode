/** How far a drag must travel to dismiss the toast. */
export const TOAST_DISMISS_PX = 48;
/** A flick faster than this, in px/ms over the whole drag, dismisses short of 48px. */
export const TOAST_FLICK_SPEED = 0.5;
/** ...but only once it has gone this far, so a fast twitch is not a dismissal. */
export const TOAST_FLICK_PX = 12;
/** Less travel than this, end to end, is a click on the card rather than a drag. */
export const TOAST_CLICK_PX = 4;
export const TOAST_DRAG_MS = 160;

export function dragState(dx: number, ms = Number.POSITIVE_INFINITY) {
  const away = Math.abs(dx);
  const flick = away >= TOAST_FLICK_PX && away / Math.max(ms, 1) > TOAST_FLICK_SPEED;
  return { dx, away, dismiss: away > TOAST_DISMISS_PX || flick };
}

/** Opacity while dragging: it fades as it travels, never below 0.2. */
export function dragOpacity(away: number): number {
  return Math.max(0.2, 1 - away / 200);
}

/** Where a dismissed card flies to: in the direction of the swipe. */
export function toastExit(dx: number): string {
  return `translateX(${Math.sign(dx) * 120}%)`;
}

/** A press and release this close together is a click on the card: it dismisses. */
export function isClick(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) < TOAST_CLICK_PX;
}

/** A press that starts on a control is a click, never a drag. */
export function startsDrag(target: { closest?: (selector: string) => unknown } | null): boolean {
  return !target?.closest?.("button, a, input, textarea, [role='button'], [role='menuitem']");
}

export interface SelectionDoc {
  documentElement: { style: { userSelect: string; webkitUserSelect?: string } };
  body: { style: { userSelect: string; webkitUserSelect?: string } };
  addEventListener(type: "selectstart", listener: (event: { preventDefault(): void }) => void): void;
  removeEventListener(type: "selectstart", listener: (event: { preventDefault(): void }) => void): void;
}

/**
 * Turns text selection off for the length of a drag and returns the release.
 * The release is idempotent: a pointerup, a pointercancel, a blur and an
 * unmount can all call it, and only the first one does anything.
 */
export function holdSelection(doc: SelectionDoc): () => void {
  const html = doc.documentElement;
  const body = doc.body;
  const before = {
    html: html.style.userSelect,
    htmlWebkit: html.style.webkitUserSelect,
    body: body.style.userSelect,
    bodyWebkit: body.style.webkitUserSelect,
  };
  const block = (event: { preventDefault(): void }) => event.preventDefault();
  html.style.userSelect = "none";
  body.style.userSelect = "none";
  if ("webkitUserSelect" in html.style) html.style.webkitUserSelect = "none";
  if ("webkitUserSelect" in body.style) body.style.webkitUserSelect = "none";
  doc.addEventListener("selectstart", block);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    html.style.userSelect = before.html;
    body.style.userSelect = before.body;
    if ("webkitUserSelect" in html.style) html.style.webkitUserSelect = before.htmlWebkit ?? "";
    if ("webkitUserSelect" in body.style) body.style.webkitUserSelect = before.bodyWebkit ?? "";
    doc.removeEventListener("selectstart", block);
  };
}
