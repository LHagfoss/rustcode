// Dragging a thread row to another place in its section (ui/Sidebar.tsx).
//
// A press on a row is still a click until the pointer has moved a few pixels
// (state/sidebarOrder.ts `moveDrag`): a shiver of the hand opens the thread as it
// always did, a real drag takes the row with the pointer, draws the rule it would
// drop on, and arranges the section on release. The click a drag leaves behind is
// swallowed once, so a row that was dragged never also opens itself.
//
// The row moves by its own transform, so the list does not reflow while it is
// dragged, and the drop rule is one absolutely placed line in the scrolling list.

import type { PointerEvent as ReactPointerEvent } from "react";
import { useRef, useState } from "react";
import { insertIndex } from "../state/headerButtons";
import { type DragPhase, moveDrag, startDrag, swallowsClick } from "../state/sidebarOrder";

/** What the row being dragged belongs to: the section key it arranges, and the rows drawn there. */
export interface DragHome {
  key: string;
  ids: readonly string[];
}

/** The rule drawn where the row would land, in the list's own scrolling coordinates. */
export interface DropLine {
  top: number;
  left: number;
  width: number;
}

interface Press {
  id: string;
  home: DragHome;
  pointerId: number;
  drag: DragPhase;
  /** The slot the pointer names, among the rows with this one taken out. */
  index: number | null;
  /** The other rows' boxes, read once: a drag only translates the row it moves. */
  boxes?: { top: number; height: number }[];
  el: HTMLElement;
}

/**
 * The row drag: `start` on the list's `onPointerDown`, `line` drawn in the list while
 * a row is over another place, and `draggedId` for the row that is off its place.
 */
export function useThreadDrag({
  list,
  homeOf,
  enabled,
  drop,
}: {
  /** The list's scrolling element: row boxes and the drop rule are read against it. */
  list: React.RefObject<HTMLElement | null>;
  /** The section a row is drawn in, for the row a press landed on. */
  homeOf: (id: string) => DragHome | undefined;
  /** False while a search narrows the list: a drag arranges what the section draws itself. */
  enabled: boolean;
  /** The section's new order: `id` dropped at `index` of the rows drawn there. */
  drop: (home: DragHome, id: string, index: number) => void;
}) {
  const [line, setLine] = useState<DropLine | null>(null);
  const [draggedId, setDraggedId] = useState<string | null>(null);
  const press = useRef<Press | null>(null);

  /** One click only: the one a drag would leave behind, never the next press. */
  const unswallow = () => {
    document.removeEventListener("click", swallow, true);
    document.removeEventListener("pointerdown", unswallow, true);
  };
  const swallow = (e: Event) => {
    e.stopPropagation();
    e.preventDefault();
    unswallow();
  };

  const rowOf = (id: string) =>
    list.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(id)}"]`) ?? null;

  /** The other rows as the pointer sees them, in the order the list draws them. */
  const boxesOf = (p: Press) => {
    const rows: { top: number; height: number }[] = [];
    for (const id of p.home.ids) {
      if (id === p.id) continue;
      const el = rowOf(id);
      if (!el) continue;
      const box = el.getBoundingClientRect();
      rows.push({ top: box.top, height: box.height });
    }
    return rows;
  };

  /** Where the pointer would drop the row, and the rule drawn for it. */
  const place = (p: Press, y: number) => {
    const listEl = list.current;
    if (!listEl) return;
    // Read once: the rows do not move during a drag, the one under the pointer does.
    p.boxes ??= boxesOf(p);
    const rows = p.boxes;
    const index = insertIndex(rows, y);
    p.index = index;
    const above = rows[index - 1];
    const edge = above ? above.top + above.height : rows[0]?.top;
    if (edge === undefined) return;
    const row = p.el.getBoundingClientRect();
    const box = listEl.getBoundingClientRect();
    const next = {
      top: edge - box.top + listEl.scrollTop,
      left: row.left - box.left + listEl.scrollLeft,
      width: row.width,
    };
    setLine((prev) =>
      prev && prev.top === next.top && prev.left === next.left && prev.width === next.width ? prev : next,
    );
  };

  /** The press became a drag: the row lifts, the pointer stops selecting text, the click is eaten. */
  const lift = (p: Press) => {
    const { style } = p.el;
    style.zIndex = "20";
    style.opacity = "0.85";
    style.willChange = "transform";
    style.transition = "none";
    setDraggedId(p.id);
    document.body.style.userSelect = "none";
    window.getSelection()?.removeAllRanges();
    document.addEventListener("click", swallow, true);
    document.addEventListener("pointerdown", unswallow, true);
  };

  /** Puts the row back the way it was drawn, whatever else the press did. */
  const release = (p: Press) => {
    press.current = null;
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", abort);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("blur", abort);
    const { style } = p.el;
    style.transform = "";
    style.zIndex = "";
    style.opacity = "";
    style.willChange = "";
    style.transition = "";
    document.body.style.userSelect = "";
    setDraggedId(null);
    setLine(null);
    // A press that never moved is a click, and its click goes through as it always did.
    if (!swallowsClick(p.drag)) unswallow();
  };

  const onMove = (e: PointerEvent) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    const next = moveDrag(p.drag, e.clientX, e.clientY);
    if (next === p.drag) return;
    const beginning = p.drag.phase === "pending";
    p.drag = next;
    if (beginning) lift(p);
    if (e.cancelable) e.preventDefault();
    p.el.style.transform = `translateY(${e.clientY - p.drag.y}px)`;
    place(p, e.clientY);
  };

  const onUp = (e: PointerEvent) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    const index = p.index;
    const dragged = swallowsClick(p.drag);
    release(p);
    if (dragged && index !== null) drop(p.home, p.id, index);
  };

  const abort = () => {
    const p = press.current;
    if (p) release(p);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") abort();
  };

  /** A press on a row: a drag if the pointer moves, a click if it does not. */
  const start = (e: ReactPointerEvent) => {
    if (!enabled || press.current || e.button !== 0 || !e.isPrimary) return;
    // A modified press is the list's own: ⌘-click picks, ⇧-click takes a range.
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const target = e.target as HTMLElement | null;
    const row = target?.closest<HTMLElement>("[data-row-id]");
    if (!row || !target) return;
    // A control inside the row (its own actions, a linked thread) answers its press itself.
    const control = target.closest("button, a, input, textarea, select");
    if (control && control !== row) return;
    const id = row.dataset.rowId;
    if (!id) return;
    const home = homeOf(id);
    if (!home || home.ids.length < 2) return;

    press.current = {
      id,
      home,
      pointerId: e.pointerId,
      drag: startDrag(e.clientX, e.clientY),
      index: null,
      el: row,
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", abort);
    window.addEventListener("keydown", onKey);
    // A window that goes away mid-drag (⌘-Tab) never sends its pointerup.
    window.addEventListener("blur", abort);
  };

  return { start, line, draggedId };
}
