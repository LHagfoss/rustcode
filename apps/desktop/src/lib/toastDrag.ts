// What a drag on a toast does, as pure arithmetic and one DOM guard
// (ui/Toaster.tsx): the card follows sideways fully and upward at a sixth,
// drops away downward, and dismisses past 48px sideways or 24px down. A press
// that barely moves is a click, which dismisses too. A drag never selects text
// anywhere: the guard below turns selection off on the document and blocks
// `selectstart` until it is released, and every way a drag ends releases it.

/** How far a drag must travel to dismiss the toast. */
export const TOAST_DISMISS_PX = 48;
/** Upward drags resist: the stack sits in the window's bottom corner and leaves downward. */
const UP_RESIST = 6;
/** Less travel than this, end to end, is a click on the card rather than a drag. */
export const TOAST_CLICK_PX = 4;
export const TOAST_DRAG_MS = 160;

export type SwipeDirection = "left" | "right" | "down";

export interface DragState {
  /** Sideways movement, unmodified. */
  dx: number;
  /** Vertical movement: full downward, resisted upward. */
  dy: number;
  /** How far it has travelled from its place, for the fade. */
  away: number;
  dismiss: boolean;
  /** Which way it leaves when dismissed; null while it snaps back. */
  direction: SwipeDirection | null;
}

export function dragState(dx: number, dy: number): DragState {
  const followY = dy > 0 ? dy : dy / UP_RESIST;
  const away = Math.max(Math.abs(dx), followY);
  const dismiss = Math.abs(dx) > TOAST_DISMISS_PX || dy >= TOAST_DISMISS_PX / 2;
  const direction: SwipeDirection | null = !dismiss
    ? null
    : Math.abs(dx) > dy
      ? dx < 0
        ? "left"
        : "right"
      : "down";
  return { dx, dy: followY, away, dismiss, direction };
}

/** Opacity while dragging: it fades as it travels, never below 0.2. */
export function dragOpacity(away: number): number {
  return Math.max(0.2, 1 - away / 200);
}

/** Where a dismissed card flies to. */
export function dragExit(direction: SwipeDirection, dx: number): string {
  if (direction === "down") return "translateY(140%)";
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
