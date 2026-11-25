/**
 * The new-thread overview's grid and its edit mode: cards are dragged by their grip to a new
 * place (the others make room as the pointer passes over them), widened or narrowed, removed and
 * added back from a gallery of the removed cards. The arrangement lives in `state/widgetLayout.ts`.
 */

import { DropdownMenu } from "radix-ui";
import {
  type CSSProperties,
  createContext,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  addWidget,
  cardHeight,
  DEFAULT_LAYOUT,
  fittedRowHeight,
  getWidgetLayout,
  hiddenWidgets,
  moveWidget,
  packLayout,
  ROW_HEIGHT,
  removeWidget,
  resizeWidget,
  sameLayout,
  setWidgetLayout,
  stackMembers,
  stackWidget,
  useWidgetLayout,
  WIDGET_BLURBS,
  WIDGET_NAMES,
  type WidgetExtent,
  type WidgetId,
  type WidgetSlot,
} from "../state/widgetLayout";
import { GripVertical, Layers, Plus, RotateCcw, SlidersHorizontal, X } from "./icons";
import { cx, MenuContent, menuItem, menuLabel, Pill, Tooltip } from "./primitives";
import { StackContext, type StackState } from "./WidgetStack";

/** The grid's columns for its width: four on a wide window, two on a narrow one, one on a sliver. */
export function columnsFor(width: number): number {
  if (width >= 880) return 4;
  if (width >= 520) return 2;
  return 1;
}

const sizeName = (slot: WidgetSlot) => `${slot.w}×${slot.h}`;

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
  /** The card the place shows now: its own, or one of its stack's. */
  shown: WidgetId;
  columns: number;
  index: number;
  editing: boolean;
  dragging: boolean;
  startDrag: (event: PointerEvent<HTMLButtonElement>) => void;
  nudge: (event: KeyboardEvent<HTMLButtonElement>) => void;
}

const FrameContext = createContext<Frame | undefined>(undefined);
const RowContext = createContext(ROW_HEIGHT);

/** The card's height in pixels: its rows of the grid, which grow when the overview has room to fill. */
export function useCardHeight(): number {
  const row = useContext(RowContext);
  return cardHeight(row, useContext(FrameContext)?.slot.h ?? 1);
}

/**
 * The height left for the grid's scroll box inside the box that holds it, the overview's docked
 * column: that column's height less everything else in it and the gaps between them.
 */
function useRoom(scroller: HTMLElement | null | undefined, on: boolean): number | undefined {
  const [room, setRoom] = useState<number>();
  useLayoutEffect(() => {
    const column = scroller?.parentElement;
    if (!on || !scroller || !column) {
      setRoom(undefined);
      return;
    }
    const measure = () => {
      const others = [...column.children].filter((child) => child !== scroller);
      const gap = Number.parseFloat(getComputedStyle(column).rowGap) || 0;
      const scrollerStyle = getComputedStyle(scroller);
      const padding =
        (Number.parseFloat(scrollerStyle.paddingTop) || 0) +
        (Number.parseFloat(scrollerStyle.paddingBottom) || 0);
      const taken = others.reduce((sum, child) => sum + (child as HTMLElement).offsetHeight, 0);
      setRoom(column.clientHeight - taken - gap * others.length - padding);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(column);
    for (const child of column.children) if (child !== scroller) observer.observe(child);
    return () => observer.disconnect();
  }, [scroller, on]);
  return room;
}

/** Where the card sits in the grid: its columns, never more than the grid has, and its rows. */
export function useWidgetPlacement(): CSSProperties {
  const frame = useContext(FrameContext);
  if (!frame) return {};
  const wide = Math.min(frame.slot.w, frame.columns);
  return {
    gridColumn: `span ${wide} / span ${wide}`,
    gridRow: `span ${frame.slot.h} / span ${frame.slot.h}`,
  };
}

/** How many of the grid's columns the card spans, so a card can lay out for its width. */
export function useWidgetColumns(): number {
  const frame = useContext(FrameContext);
  return frame ? Math.min(frame.slot.w, frame.columns) : 1;
}

/** How many columns a grid of `width` pixels has, followed as the element resizes. */
function useColumns(element: HTMLElement | null): number {
  const [columns, setColumns] = useState(4);
  useLayoutEffect(() => {
    if (!element) return;
    const fit = () => setColumns(columnsFor(element.clientWidth));
    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return columns;
}

/** How many rows of the grid the card spans. */
export function useWidgetRows(): WidgetExtent {
  return useContext(FrameContext)?.slot.h ?? 1;
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
  const name = WIDGET_NAMES[frame.shown];
  const stacked = stackMembers(frame.slot).length > 1;
  return (
    <span className="flex shrink-0 items-center gap-0.5">
      <StackMenu slot={frame.slot} name={name} />
      <Tooltip label="Change the size: wide, tall or both">
        <button
          type="button"
          aria-label={`Size of ${name}: ${frame.slot.w} wide, ${frame.slot.h} tall`}
          onClick={() => setWidgetLayout(resizeWidget(getWidgetLayout(), frame.slot.id))}
          className="h-5 min-w-5 rounded-sm px-1 font-medium text-caption text-fg-muted tabular hover:bg-fill-subtle hover:text-fg"
        >
          {sizeName(frame.slot)}
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
      <Tooltip label={stacked ? "Remove this card from the stack" : "Remove from the overview"}>
        <button
          type="button"
          aria-label={`Remove ${name}`}
          onClick={() => setWidgetLayout(removeWidget(getWidgetLayout(), frame.shown))}
          className="flex size-5 items-center justify-center rounded-sm text-fg-muted hover:bg-fill-subtle hover:text-danger"
        >
          <X size={12} />
        </button>
      </Tooltip>
    </span>
  );
}

/** In edit mode, the control that stacks a removed card into this card's place, to take turns with it. */
function StackMenu({ slot, name }: { slot: WidgetSlot; name: string }) {
  const hidden = hiddenWidgets(getWidgetLayout());
  if (!hidden.length) return null;
  return (
    <DropdownMenu.Root modal={false}>
      <Tooltip label="Stack a card here: they take turns">
        <DropdownMenu.Trigger asChild>
          <button
            type="button"
            aria-label={`Stack a card with ${name}`}
            className="flex size-5 items-center justify-center rounded-sm text-fg-muted hover:bg-fill-subtle hover:text-fg data-[state=open]:bg-fill-subtle"
          >
            <Layers size={12} />
          </button>
        </DropdownMenu.Trigger>
      </Tooltip>
      <MenuContent align="end" className="w-[260px]">
        <div className={menuLabel}>Stack with {name}</div>
        {hidden.map((id) => (
          <DropdownMenu.Item
            key={id}
            className={menuItem}
            onSelect={() => setWidgetLayout(stackWidget(getWidgetLayout(), slot.id, id))}
          >
            <span className="flex min-w-0 flex-col">
              <span className="truncate">{WIDGET_NAMES[id]}</span>
              <span className="truncate text-caption text-fg-muted">{WIDGET_BLURBS[id]}</span>
            </span>
          </DropdownMenu.Item>
        ))}
      </MenuContent>
    </DropdownMenu.Root>
  );
}

/**
 * One place in the grid: a card, or a stack of cards that take turns. A stack turns to its next
 * card when its indicator's countdown ends (`StackDots`), and holds still while the pointer or the
 * focus is on it and in edit mode.
 */
function StackPlace({
  slot,
  editing,
  children,
}: {
  slot: WidgetSlot;
  editing: boolean;
  children: (shown: WidgetId) => ReactNode;
}) {
  const members = stackMembers(slot);
  const [picked, setPicked] = useState<{ at: number; turned: boolean }>({ at: 0, turned: false });
  const [held, setHeld] = useState(false);
  const at = picked.at % members.length;
  const shown = members[at] ?? slot.id;
  if (members.length < 2) return <>{children(shown)}</>;
  const stack: StackState = {
    members,
    at,
    turned: picked.turned,
    paused: held || editing,
    select: (index) => setPicked({ at: index, turned: true }),
    advance: () => setPicked((was) => ({ at: (was.at + 1) % members.length, turned: true })),
  };
  return (
    <div
      className="contents"
      onMouseOver={() => setHeld(true)}
      onMouseLeave={() => setHeld(false)}
      onFocus={() => setHeld(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setHeld(false);
      }}
    >
      <StackContext.Provider value={stack}>{children(shown)}</StackContext.Provider>
    </div>
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
export function WidgetGrid({
  render,
  fill = false,
}: {
  render: (id: WidgetId) => ReactNode;
  /** Grow the cards to fill the column the overview is docked in. */
  fill?: boolean;
}) {
  const layout = useWidgetLayout();
  const isEditing = useWidgetsEditing();
  const [dragId, setDragId] = useState<WidgetId>();
  const [ghost, setGhost] = useState<{ x: number; y: number }>();
  const grid = useSlideOnReorder(layout.map((slot) => `${slot.id}:${slot.w}${slot.h}`).join());
  const [scroller, setScroller] = useState<HTMLElement | null>();
  useLayoutEffect(() => setScroller(grid.current?.parentElement), [grid]);
  const room = useRoom(scroller, fill);
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const columns = useColumns(box);
  const row = room === undefined ? ROW_HEIGHT : fittedRowHeight(room, packLayout(layout, columns).rows);

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
    <RowContext.Provider value={row}>
      <div
        ref={(element) => {
          grid.current = element;
          setBox(element);
        }}
        className="relative grid grid-flow-row-dense gap-2"
        style={{ gridAutoRows: row, gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
        role="group"
        aria-label="Activity, usage and recent work"
      >
        {layout.map((slot, index) => (
          <StackPlace key={slot.id} slot={slot} editing={isEditing}>
            {(shown) => (
              <FrameContext.Provider
                key={shown}
                value={{
                  slot,
                  shown,
                  columns,
                  index,
                  editing: isEditing,
                  dragging: dragId === slot.id,
                  startDrag: startDrag(slot.id),
                  nudge: nudge(slot.id),
                }}
              >
                {render(shown)}
              </FrameContext.Provider>
            )}
          </StackPlace>
        ))}
        {!isEditing && layout.length === 0 && (
          <div className="col-span-full flex items-center justify-center gap-2 py-6 text-caption text-fg-muted">
            The overview is empty.
            <Pill quiet size="sm" onClick={() => setWidgetsEditing(true)}>
              Add cards
            </Pill>
          </div>
        )}
      </div>
      {isEditing && <AddTray hidden={hidden} layout={layout} />}
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
    </RowContext.Provider>
  );
}

function AddTray({ hidden, layout }: { hidden: WidgetId[]; layout: WidgetSlot[] }) {
  const changed = !sameLayout(layout, DEFAULT_LAYOUT);
  const tray = useRef<HTMLElement>(null);
  useEffect(() => {
    tray.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, []);
  return (
    <section
      ref={tray}
      aria-label="Add cards"
      className="mt-2 flex flex-col gap-2 rounded-xl border border-accent/35 border-dashed p-3"
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
        <div className="grid grid-cols-[repeat(auto-fill,minmax(12rem,1fr))] gap-1.5">
          {hidden.map((id) => (
            <button
              key={id}
              type="button"
              aria-label={`Add ${WIDGET_NAMES[id]}`}
              onClick={() => setWidgetLayout(addWidget(layout, id))}
              className="flex items-start gap-2 rounded-lg border border-border px-3 py-2 text-left hover:bg-fill-subtle"
            >
              <Plus size={12} className="mt-0.5 shrink-0 text-accent" aria-hidden />
              <span className="flex min-w-0 flex-col">
                <span className="text-caption font-medium text-fg">{WIDGET_NAMES[id]}</span>
                <span className="text-caption text-fg-muted">{WIDGET_BLURBS[id]}</span>
              </span>
            </button>
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
