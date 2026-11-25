/**
 * The new-thread overview's grid and its edit mode: cards are dragged by their grip to a new
 * place (the others make room as the pointer passes over them), widened or narrowed, removed and
 * added back from a tray. The arrangement lives in `state/widgetLayout.ts`.
 */

import {
  createContext,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  addWidget,
  DEFAULT_LAYOUT,
  getWidgetLayout,
  hiddenWidgets,
  moveWidget,
  removeWidget,
  resizeWidget,
  sameLayout,
  setWidgetLayout,
  useWidgetLayout,
  WIDGET_BLURBS,
  WIDGET_NAMES,
  type WidgetId,
  type WidgetSlot,
  type WidgetSpan,
} from "../state/widgetLayout";
import { GripVertical, Plus, RotateCcw, SlidersHorizontal, X } from "./icons";
import { cx, Pill, Tooltip } from "./primitives";

const SPAN_CLASS: Record<WidgetSpan, string> = {
  2: "lg:col-span-2",
  3: "lg:col-span-3",
  4: "lg:col-span-4",
  6: "lg:col-span-6",
};

const SPAN_NAME: Record<WidgetSpan, string> = { 2: "S", 3: "M", 4: "L", 6: "XL" };

let editing = false;
const editListeners = new Set<() => void>();

export function setWidgetsEditing(on: boolean): void {
  editing = on;
  for (const listener of editListeners) listener();
}

export function useWidgetsEditing(): boolean {
  return useSyncExternalStore(
    (listener) => {
      editListeners.add(listener);
      return () => editListeners.delete(listener);
    },
    () => editing,
  );
}

interface Frame {
  slot: WidgetSlot;
  index: number;
  editing: boolean;
  dragging: boolean;
  startDrag: (event: PointerEvent<HTMLButtonElement>) => void;
  nudge: (event: KeyboardEvent<HTMLButtonElement>) => void;
}

const FrameContext = createContext<Frame | undefined>(undefined);

/** The card's width class in the grid, for `Card` to put on its section. */
export function useWidgetSpan(): string {
  const frame = useContext(FrameContext);
  return frame ? SPAN_CLASS[frame.slot.span] : "lg:col-span-2";
}

/** The card's id for the grid's hit test, and the edit mode's outline. */
export function useWidgetId(): WidgetId | undefined {
  return useContext(FrameContext)?.slot.id;
}

/** The edit mode's outline for a card. */
export function useWidgetEditClass(): string | undefined {
  const frame = useContext(FrameContext);
  if (!frame?.editing) return undefined;
  return frame.dragging ? "ring-2 ring-accent" : "ring-1 ring-accent/35";
}

/** In edit mode, what sits at the end of a card's label row: its grip, width and remove controls. */
export function WidgetChrome({ fallback }: { fallback?: ReactNode }) {
  const frame = useContext(FrameContext);
  if (!frame?.editing) return <>{fallback}</>;
  const name = WIDGET_NAMES[frame.slot.id];
  return (
    <span className="flex shrink-0 items-center gap-0.5">
      <Tooltip label="Change the width">
        <button
          type="button"
          aria-label={`Width of ${name}: ${SPAN_NAME[frame.slot.span]}`}
          onClick={() => setWidgetLayout(resizeWidget(getWidgetLayout(), frame.slot.id))}
          className="h-5 min-w-5 rounded-sm px-1 font-medium text-caption text-fg-muted tabular hover:bg-fill-subtle hover:text-fg"
        >
          {SPAN_NAME[frame.slot.span]}
        </button>
      </Tooltip>
      <Tooltip label="Drag to move, or use the arrow keys">
        <button
          type="button"
          aria-label={`Move ${name}`}
          onPointerDown={frame.startDrag}
          onKeyDown={frame.nudge}
          className="flex size-5 cursor-grab touch-none items-center justify-center rounded-sm text-fg-muted hover:bg-fill-subtle hover:text-fg active:cursor-grabbing"
        >
          <GripVertical size={12} />
        </button>
      </Tooltip>
      <Tooltip label="Remove from the overview">
        <button
          type="button"
          aria-label={`Remove ${name}`}
          onClick={() => setWidgetLayout(removeWidget(getWidgetLayout(), frame.slot.id))}
          className="flex size-5 items-center justify-center rounded-sm text-fg-muted hover:bg-fill-subtle hover:text-danger"
        >
          <X size={12} />
        </button>
      </Tooltip>
    </span>
  );
}

/** Moves cards that changed place from where they were, so a reorder slides instead of jumping. */
function useSlideOnReorder(order: string) {
  const grid = useRef<HTMLDivElement>(null);
  const rects = useRef(new Map<string, DOMRect>());
  useLayoutEffect(() => {
    const root = grid.current;
    if (!root || !order) return;
    const next = new Map<string, DOMRect>();
    for (const element of root.querySelectorAll<HTMLElement>("[data-widget]")) {
      const id = element.dataset.widget ?? "";
      const rect = element.getBoundingClientRect();
      next.set(id, rect);
      const before = rects.current.get(id);
      if (!before || matchMedia("(prefers-reduced-motion: reduce)").matches) continue;
      const dx = before.left - rect.left;
      const dy = before.top - rect.top;
      if (Math.abs(dx) < 1 && Math.abs(dy) < 1) continue;
      element.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], {
        duration: 200,
        easing: "cubic-bezier(0.2, 0.9, 0.3, 1)",
      });
    }
    rects.current = next;
  }, [order]);
  return grid;
}

/** The overview's cards in the user's order, with the edit mode's tray at the end. */
export function WidgetGrid({ render }: { render: (id: WidgetId) => ReactNode }) {
  const layout = useWidgetLayout();
  const isEditing = useWidgetsEditing();
  const [dragId, setDragId] = useState<WidgetId>();
  const [ghost, setGhost] = useState<{ x: number; y: number }>();
  const grid = useSlideOnReorder(layout.map((slot) => `${slot.id}:${slot.span}`).join());

  const hoverTarget = (x: number, pointerY: number, id: WidgetId) => {
    const box = grid.current?.parentElement?.getBoundingClientRect();
    const y = box ? Math.min(Math.max(pointerY, box.top + 4), box.bottom - 4) : pointerY;
    const root = grid.current;
    if (!root) return;
    const origin = root.getBoundingClientRect();
    const over = [...root.querySelectorAll<HTMLElement>("[data-widget]")].find((card) => {
      const left = origin.left + card.offsetLeft - root.scrollLeft;
      const top = origin.top + card.offsetTop;
      return (
        card.dataset.widget !== id &&
        x >= left &&
        x <= left + card.offsetWidth &&
        y >= top &&
        y <= top + card.offsetHeight
      );
    });
    const target = over?.dataset.widget;
    if (!target) return;
    const to = getWidgetLayout().findIndex((slot) => slot.id === target);
    if (to < 0) return;
    const next = moveWidget(getWidgetLayout(), id, to);
    if (!sameLayout(next, getWidgetLayout())) setWidgetLayout(next);
  };

  const startDrag = (id: WidgetId) => (event: PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    setDragId(id);
    setGhost({ x: event.clientX, y: event.clientY });
    const start = { x: event.clientX, y: event.clientY };
    let pointer = start;
    let moved = false;
    const scroller = grid.current?.parentElement;
    scroller?.style.setProperty("scroll-snap-type", "none");
    let frame = requestAnimationFrame(function edgeScroll() {
      const box = scroller?.getBoundingClientRect();
      if (scroller && box && moved) {
        const above = box.top - pointer.y;
        const below = pointer.y - box.bottom;
        const step = above > 0 ? -Math.min(above, 60) / 4 : below > 0 ? Math.min(below, 60) / 4 : 0;
        if (step) {
          scroller.scrollTop += step;
          hoverTarget(pointer.x, pointer.y, id);
        }
      }
      frame = requestAnimationFrame(edgeScroll);
    });
    const move = (next: globalThis.PointerEvent) => {
      pointer = { x: next.clientX, y: next.clientY };
      moved ||= Math.hypot(pointer.x - start.x, pointer.y - start.y) > 8;
      setGhost(pointer);
      hoverTarget(next.clientX, next.clientY, id);
    };
    const end = () => {
      cancelAnimationFrame(frame);
      scroller?.style.removeProperty("scroll-snap-type");
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      setDragId(undefined);
      setGhost(undefined);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  };

  const nudge = (id: WidgetId) => (event: KeyboardEvent<HTMLButtonElement>) => {
    const step =
      event.key === "ArrowLeft" || event.key === "ArrowUp"
        ? -1
        : event.key === "ArrowRight" || event.key === "ArrowDown"
          ? 1
          : 0;
    if (!step) return;
    event.preventDefault();
    const handle = event.currentTarget;
    const layout = getWidgetLayout();
    setWidgetLayout(moveWidget(layout, id, layout.findIndex((slot) => slot.id === id) + step));
    requestAnimationFrame(() => handle.focus());
  };

  const hidden = hiddenWidgets(layout);
  return (
    <>
      <div
        ref={grid}
        className="relative flex snap-x gap-2 overflow-x-auto pb-0.5 lg:grid lg:grid-cols-6 lg:overflow-visible lg:pb-0"
        role="group"
        aria-label="Activity, usage and recent work"
      >
        {layout.map((slot, index) => (
          <FrameContext.Provider
            key={slot.id}
            value={{
              slot,
              index,
              editing: isEditing,
              dragging: dragId === slot.id,
              startDrag: startDrag(slot.id),
              nudge: nudge(slot.id),
            }}
          >
            {render(slot.id)}
          </FrameContext.Provider>
        ))}
        {isEditing && <AddTray hidden={hidden} layout={layout} />}
        {!isEditing && layout.length === 0 && (
          <div className="col-span-6 flex items-center justify-center gap-2 py-6 text-caption text-fg-muted">
            The overview is empty.
            <Pill quiet size="sm" onClick={() => setWidgetsEditing(true)}>
              Add cards
            </Pill>
          </div>
        )}
      </div>
      {dragId && ghost && (
        <div
          aria-hidden
          className="rc-glass pointer-events-none fixed z-50 flex items-center gap-2 rounded-full px-3 py-1 text-caption font-medium text-fg"
          style={{ left: ghost.x + 12, top: ghost.y + 12 }}
        >
          <GripVertical size={12} className="text-fg-muted" />
          {WIDGET_NAMES[dragId]}
        </div>
      )}
    </>
  );
}

function AddTray({ hidden, layout }: { hidden: WidgetId[]; layout: WidgetSlot[] }) {
  const changed = !sameLayout(layout, DEFAULT_LAYOUT);
  return (
    <section
      aria-label="Add cards"
      className="flex w-[240px] shrink-0 snap-start flex-col gap-2 rounded-xl border border-accent/35 border-dashed p-3 lg:col-span-6 lg:w-auto"
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-caption font-medium text-fg-muted">
          {hidden.length ? "Add a card" : "Every card is on the overview"}
        </h3>
        <span className="flex items-center gap-1">
          {changed && (
            <Pill quiet size="sm" onClick={() => setWidgetLayout(DEFAULT_LAYOUT)}>
              <RotateCcw size={12} />
              Reset
            </Pill>
          )}
          <Pill size="sm" onClick={() => setWidgetsEditing(false)}>
            Done
          </Pill>
        </span>
      </div>
      {hidden.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {hidden.map((id) => (
            <Tooltip key={id} label={WIDGET_BLURBS[id]}>
              <Pill quiet onClick={() => setWidgetLayout(addWidget(layout, id))}>
                <Plus size={12} />
                {WIDGET_NAMES[id]}
              </Pill>
            </Tooltip>
          ))}
        </div>
      )}
    </section>
  );
}

/** The pill beside "Hide overview" that turns the edit mode on and off. */
export function CustomizeOverview({ glass }: { glass: boolean }) {
  const isEditing = useWidgetsEditing();
  return (
    <button
      type="button"
      aria-pressed={isEditing}
      onClick={() => setWidgetsEditing(!isEditing)}
      className={cx(
        "flex shrink-0 items-center gap-1 rounded-full px-2 py-1 text-caption",
        isEditing ? "bg-accent/15 text-accent" : "text-fg-muted",
        !isEditing && (glass ? "rc-glass hover:text-fg" : "hover:bg-fill-subtle"),
      )}
    >
      <SlidersHorizontal size={12} aria-hidden />
      {isEditing ? "Done" : "Customize"}
    </button>
  );
}
