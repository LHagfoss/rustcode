// Dragging a thread row to another place in its section (ui/Sidebar.tsx).
//
// A press on a row is still a click until the pointer has moved a few pixels
// (state/sidebarOrder.ts `advanceDrag`): a shiver of the hand opens the thread as it
// always did, a real drag takes the row with the pointer, slides the rows it passes
// aside to open the place it would land in, and arranges the section on release. The
// click a drag leaves behind is swallowed once, so a row that was dragged never also
// opens itself.
//
// Every row moves by its own transform, so the list does not reflow while one is
// dragged. Near the list's top or bottom edge the list scrolls under the pointer, so a
// row can be taken to a place that is not on screen.

import type { PointerEvent as ReactPointerEvent } from "react";
import { useRef, useState } from "react";
import { insertIndex } from "../state/headerButtons";
import { advanceDrag, type DragPhase, startDrag, swallowsClick } from "../state/sidebarOrder";

/** What the row being dragged belongs to: the section key it arranges, and the rows drawn there. */
export interface DragHome {
  key: string;
  ids: readonly string[];
}

/** How close to the list's top or bottom edge the pointer scrolls it, and how fast at the edge. */
const EDGE = 40;
const MAX_SCROLL_STEP = 14;
/** How fast the rows slide aside. */
const SLIDE = "transform 150ms cubic-bezier(0.2, 0.8, 0.2, 1)";

interface Press {
  id: string;
  home: DragHome;
  pointerId: number;
  drag: DragPhase;
  /** The slot the pointer names, among the rows with this one taken out. */
  index: number | null;
  /**
   * The other rows, in the order the list draws them, with their boxes in the list's scrolling
   * coordinates: read once, since a drag only translates rows and a scroll moves all of them.
   */
  others?: { el: HTMLElement; top: number; height: number }[];
  /** Where the dragged row sits among the rows drawn in its section. */
  from: number;
  /** How far a row slides to make room: the dragged row's height. */
  room: number;
  el: HTMLElement;
  /** The list's scroll when the press began, so the row stays under the pointer as it scrolls. */
  scroll0: number;
  /** The pointer's last height, for the edge scroll that moves the list without a pointer move. */
  lastY: number;
  /** The edge scroll's frame, while one runs. */
  frame?: number;
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

  /** The other rows of the section as the pointer sees them, in the list's scrolling coordinates. */
  const othersOf = (p: Press, listEl: HTMLElement) => {
    const box = listEl.getBoundingClientRect();
    const rows: { el: HTMLElement; top: number; height: number }[] = [];
    for (const id of p.home.ids) {
      if (id === p.id) continue;
      const el = rowOf(id);
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      rows.push({ el, top: rect.top - box.top + listEl.scrollTop, height: rect.height });
    }
    return rows;
  };

  /**
   * Where the pointer would drop the row: the rows between its place and that one slide aside by
   * its height, so the list shows the gap it would fill.
   */
  const place = (p: Press, clientY: number) => {
    const listEl = list.current;
    if (!listEl) return;
    p.others ??= othersOf(p, listEl);
    const y = clientY - listEl.getBoundingClientRect().top + listEl.scrollTop;
    const index = insertIndex(p.others, y);
    if (index === p.index) return;
    p.index = index;
    p.others.forEach((row, k) => {
      const shift = k < p.from && k >= index ? p.room : k >= p.from && k < index ? -p.room : 0;
      row.el.style.transition = SLIDE;
      row.el.style.transform = shift ? `translateY(${shift}px)` : "";
    });
  };

  /** Carry the row with the pointer, the list's scroll since the press included. */
  const follow = (p: Press) => {
    const scrolled = (list.current?.scrollTop ?? p.scroll0) - p.scroll0;
    p.el.style.transform = `translateY(${p.lastY - p.drag.y + scrolled}px)`;
    place(p, p.lastY);
  };

  /** Near the list's top or bottom edge, scroll it a step a frame, faster the closer it is. */
  const edgeScroll = (p: Press) => {
    p.frame = undefined;
    const listEl = list.current;
    if (!listEl || press.current !== p) return;
    const box = listEl.getBoundingClientRect();
    const above = box.top + EDGE - p.lastY;
    const below = p.lastY - (box.bottom - EDGE);
    const step =
      above > 0
        ? -Math.ceil((Math.min(above, EDGE) / EDGE) * MAX_SCROLL_STEP)
        : below > 0
          ? Math.ceil((Math.min(below, EDGE) / EDGE) * MAX_SCROLL_STEP)
          : 0;
    if (!step) return;
    const before = listEl.scrollTop;
    listEl.scrollTop = before + step;
    if (listEl.scrollTop === before) return;
    follow(p);
    p.frame = requestAnimationFrame(() => edgeScroll(p));
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

  /**
   * Puts the row back the way it was drawn, whatever else the press did. `landed` is a drop: it
   * leaves one click behind that must not open the row. A cancelled press (Escape, a capture the
   * window took back) leaves none, and a press that never moved is the click itself — in both the
   * swallow goes, so it cannot eat the next press.
   */
  const release = (p: Press, landed: boolean) => {
    press.current = null;
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", abort);
    window.removeEventListener("keydown", onKey);
    window.removeEventListener("blur", abort);
    p.el.removeEventListener("lostpointercapture", abort);
    if (p.frame !== undefined) cancelAnimationFrame(p.frame);
    // The rows that slid aside go back without sliding: the drop redraws them in their new order.
    for (const row of p.others ?? []) {
      row.el.style.transition = "";
      row.el.style.transform = "";
    }
    if (p.el.hasPointerCapture(p.pointerId)) p.el.releasePointerCapture(p.pointerId);
    const { style } = p.el;
    style.transform = "";
    style.zIndex = "";
    style.opacity = "";
    style.willChange = "";
    style.transition = "";
    document.body.style.userSelect = "";
    setDraggedId(null);
    if (!landed) unswallow();
  };

  const onMove = (e: PointerEvent) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    // The move that crosses the threshold is its own step: from there on every move carries the
    // row, even though the phase stops changing (state/sidebarOrder.ts `advanceDrag`).
    const step = advanceDrag(p.drag, e.clientX, e.clientY);
    p.drag = step.next;
    if (step.began) lift(p);
    if (!swallowsClick(p.drag)) return;
    if (e.cancelable) e.preventDefault();
    p.lastY = e.clientY;
    follow(p);
    if (p.frame === undefined) p.frame = requestAnimationFrame(() => edgeScroll(p));
  };

  const onUp = (e: PointerEvent) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    const index = p.index;
    const dragged = swallowsClick(p.drag);
    release(p, dragged);
    if (dragged && index !== null) drop(p.home, p.id, index);
  };

  const abort = () => {
    const p = press.current;
    if (p) release(p, false);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") abort();
  };

  /** A press on a row: a drag if the pointer moves, a click if it does not. */
  const start = (e: ReactPointerEvent) => {
    if (!enabled || e.button !== 0 || !e.isPrimary) return;
    // A press the window never ended (its release went somewhere it could not see) must not keep
    // the list from starting another: one press at a time is all a primary pointer can have.
    if (press.current) abort();
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

    const drawn = home.ids.filter((other) => other === id || rowOf(other));
    press.current = {
      id,
      home,
      pointerId: e.pointerId,
      drag: startDrag(e.clientX, e.clientY),
      index: null,
      from: Math.max(0, drawn.indexOf(id)),
      room: row.getBoundingClientRect().height,
      el: row,
      scroll0: list.current?.scrollTop ?? 0,
      lastY: e.clientY,
    };
    // The row keeps the pointer wherever it goes, a release outside the window included: without
    // it the row can be left lifted and the list unable to start another drag.
    try {
      row.setPointerCapture(e.pointerId);
    } catch {
      // A pointer the browser already dropped has no capture to take; the window still sees it.
    }
    row.addEventListener("lostpointercapture", abort);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", abort);
    window.addEventListener("keydown", onKey);
    // A window that goes away mid-drag (⌘-Tab) never sends its pointerup.
    window.addEventListener("blur", abort);
  };

  return { start, draggedId };
}
