// The order a sidebar section draws its rows in, and how a drag or ⌥↑/↓ changes
// it (ui/Sidebar.tsx, ui/sidebarDrag.tsx). Pure, so the store, the list, the drag
// and the tests share one order.
//
// The stored value is one id list per section key (`pinned`, `threads`,
// `settled`, and `threads:<project>` where the list is split by project): what
// the user arranged, in the order they left it. Ids a section no longer draws
// are kept — a thread that comes back (re-pinned, unfolded, filtered in again)
// returns to the place it was put rather than to the end — and an id the section
// draws that was never arranged comes after the arranged ones, in the order the
// section itself gives it. An id for a thread that no longer exists is not kept:
// `withoutThreads` on a trashed one and `keepThreads` against the engine's list
// (state/store.ts) keep the stored value the size of the work in it.

import type { SessionSummary } from "../acp/types";

/** How far the pointer moves before a press on a row becomes a drag, not a click. */
export const DRAG_THRESHOLD = 4;

/** One section's arrangement: the section keys and the ids each draws, in order. */
export type SidebarOrder = Record<string, string[]>;

/**
 * The key a section keeps its order under. The list split by project arranges each
 * project's rows on its own, so two projects never share one order.
 */
export function sectionOrderKey(section: string, project?: string): string {
  return project === undefined ? section : `${section}:${project}`;
}

/** The items in their arranged order: the ones the order names, then the rest as they came. */
export function applyOrder(
  items: readonly SessionSummary[],
  stored: readonly string[] | undefined,
): SessionSummary[] {
  if (!stored?.length) return [...items];
  const rank = new Map<string, number>();
  for (const id of stored) if (!rank.has(id)) rank.set(id, rank.size);
  const arranged = items
    .filter((item) => rank.has(item.id))
    .sort((a, b) => rank.get(a.id)! - rank.get(b.id)!);
  const rest = items.filter((item) => !rank.has(item.id));
  return [...arranged, ...rest];
}

/** The stored order with the ids it does not name yet, in the order the section draws them. */
function merged(stored: readonly string[], drawn: readonly string[]): string[] {
  const ids = [...stored];
  const seen = new Set(ids);
  for (const id of drawn) {
    if (seen.has(id)) continue;
    seen.add(id);
    ids.push(id);
  }
  return ids;
}

/**
 * The order after `id` moves to `index` of what is drawn: `index` counts the drawn
 * rows with `id` taken out, so 0 puts it first and a number past the last row puts
 * it last. Ids the section is not drawing keep their places around it, so
 * arranging a searched or filtered list never loses another thread's slot.
 */
export function reorder(
  stored: readonly string[],
  drawn: readonly string[],
  id: string,
  index: number,
): string[] {
  const ids = merged(stored, drawn).filter((x) => x !== id);
  const rank = new Map(ids.map((x, i) => [x, i] as const));
  // The drawn rows with the moved one taken out, in the order the list has them.
  const rest = drawn.filter((x) => x !== id).sort((a, b) => rank.get(a)! - rank.get(b)!);
  // The row the moved one lands above; past the last drawn row, after it.
  const above = rest[index];
  const spot =
    above !== undefined
      ? ids.indexOf(above)
      : rest.length
        ? ids.indexOf(rest[rest.length - 1]!) + 1
        : ids.length;
  ids.splice(Math.min(Math.max(spot, 0), ids.length), 0, id);
  return ids;
}

/**
 * Where ⌥↑/↓ takes `id` in what is drawn: the drawn list after the move, or null
 * at either end (there is nowhere further to go).
 */
export function moveBy(
  stored: readonly string[],
  drawn: readonly string[],
  id: string,
  delta: number,
): string[] | null {
  const from = drawn.indexOf(id);
  if (from < 0) return null;
  const to = from + delta;
  if (to < 0 || to >= drawn.length) return null;
  return reorder(stored, drawn, id, to);
}

/** A press on a row, before it is known whether the user is clicking or dragging it. */
export interface DragPhase {
  /** Where the pointer went down. */
  x: number;
  y: number;
  /** `pending` is a click until the pointer moves far enough; a `dragging` press is a drag. */
  phase: "pending" | "dragging";
}

export function startDrag(x: number, y: number): DragPhase {
  return { x, y, phase: "pending" };
}

/**
 * The press after the pointer reaches (`x`, `y`), and whether this move is the one that turned it
 * into a drag. The phase itself stops changing once it is a drag — where the row has got to is the
 * pointer's own business (`ui/sidebarDrag.tsx` `onMove`) — so the crossing is reported on the move
 * it happens, and a caller that only compared phases would stop carrying the row after the first.
 */
export function advanceDrag(
  drag: DragPhase,
  x: number,
  y: number,
  threshold = DRAG_THRESHOLD,
): { next: DragPhase; began: boolean } {
  if (drag.phase === "dragging") return { next: drag, began: false };
  const dx = x - drag.x;
  const dy = y - drag.y;
  if (Math.hypot(dx, dy) <= threshold) return { next: drag, began: false };
  return { next: { ...drag, phase: "dragging" }, began: true };
}

/** A press that became a drag must not also open the row it started on. */
export function swallowsClick(drag: DragPhase | null | undefined): boolean {
  return drag?.phase === "dragging";
}

/**
 * The arrangement limited to the ids `keep` allows, and the same object back when it keeps
 * everything: the caller writes to storage only when something was actually forgotten. A section
 * left with no ids goes with them.
 */
export function keepThreads(order: SidebarOrder, keep: (id: string) => boolean): SidebarOrder {
  let changed = false;
  const next: SidebarOrder = {};
  for (const [key, list] of Object.entries(order)) {
    const kept = list.filter(keep);
    if (kept.length !== list.length) changed = true;
    if (kept.length) next[key] = kept;
  }
  return changed ? next : order;
}

/** The arrangement without `ids`: a thread that is gone takes no place in any section. */
export function withoutThreads(order: SidebarOrder, ids: readonly string[]): SidebarOrder {
  const gone = new Set(ids);
  return keepThreads(order, (id) => !gone.has(id));
}
