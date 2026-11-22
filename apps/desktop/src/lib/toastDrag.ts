// What a drag on a toast does, as pure arithmetic and one DOM guard
// (ui/Toaster.tsx): the card slides one way only, right toward the window's
// edge. Any other way it rubber-bands at a sixth and snaps back. It dismisses
// past 48px, or on a quick flick right. A press that barely moves is a click,
// which dismisses too. A drag never selects text anywhere: the guard below
// turns selection off on the document and blocks `selectstart` until it is
// released, and every way a drag ends releases it.

/** How far a drag must travel to dismiss the toast. */
export const TOAST_DISMISS_PX = 48;
/** A flick faster than this, in px/ms over the whole drag, dismisses short of 48px. */
export const TOAST_FLICK_SPEED = 0.5;
/** ...but only once it has gone this far, so a fast twitch is not a dismissal. */
export const TOAST_FLICK_PX = 12;
/** Every way but right resists. */
const RESIST = 6;
/** Less travel than this, end to end, is a click on the card rather than a drag. */
export const TOAST_CLICK_PX = 4;
export const TOAST_DRAG_MS = 160;

export interface DragState {
  /** Sideways offset: full to the right, resisted to the left. */
  dx: number;
  /** Vertical offset, resisted both ways. */
  dy: number;
  /** How far it has travelled toward the edge, for the fade. */
  away: number;
  dismiss: boolean;
}

/** `ms` is how long the drag has lasted, for a flick; omit it while the pointer moves. */
export function dragState(dx: number, dy: number, ms = Number.POSITIVE_INFINITY): DragState {
  const followX = dx > 0 ? dx : dx / RESIST;
  const flick = dx >= TOAST_FLICK_PX && dx / Math.max(ms, 1) > TOAST_FLICK_SPEED;
  return {
    dx: followX,
    dy: dy / RESIST,
    away: Math.max(0, dx),
    dismiss: dx > TOAST_DISMISS_PX || flick,
  };
}

/** Opacity while dragging: it fades as it travels, never below 0.2. */
export function dragOpacity(away: number): number {
  return Math.max(0.2, 1 - away / 200);
}

/** Where a dismissed card flies to: off the window's right edge. */
export const TOAST_EXIT = "translateX(120%)";

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
