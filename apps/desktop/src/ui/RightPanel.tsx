// The right panel (docs/design.md, Right panel): one floating card at the
// window's right edge for the side views that take a pane (Changes, Pull
// requests, Thread). It is the details card, in the same surface: inset from
// the window's edges by the same gap, the same radius, glass and elevation,
// with tabs between the views, a close button and a draggable left edge. It
// takes no slot in the layout, so the chat keeps its full width and the
// transcript scrolls under the card; the lane centres in the room the card
// leaves (ui/ChatView.tsx `laneLayout`). The card never draws past the column's
// edges. A list view sizes the card to what it holds (a file or two, a short
// list of pull requests, an empty state), capped at the room above the composer
// (`--rc-composer-top`) so nothing it draws reaches the field and a long list
// scrolls inside it; the Thread view is a transcript, which has no height of
// its own, so that card keeps the whole room. It slides and fades in and out
// on the compositor, and its content stays mounted while it closes
// (docs/desktop.md › Native frame rate).
//
// Keys: the left edge is a `separator` that takes focus (←/→ move it 24px from
// the width on screen, Shift 96px, Home/End to the ends, ⏎ back to the
// default), Escape inside the panel closes it — the pane or the details card —
// focus moves in when a shortcut opens it with nothing focused, and closing
// gives the focus back to whatever had it. The composer is never covered: the
// card ends above it, and the composer's drawer and approvals paint over one
// (ui/ChatView.tsx), so a field being typed in keeps the keyboard.
//
// Thread details is the same card, small: it floats at the top right and lists
// what a thread is, so reading it never moves the lane.

import { X } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { observeSize } from "../lib/resize";
import { type AppState, setPanel, showPanel, useApp, useShortcutHint } from "../state/store";
import { HEADER_HEIGHT } from "./MainHeader";
import { cx, Segments, Tooltip } from "./primitives";
import { ThreadPeek } from "./ThreadPeek";

const ChangesPanel = lazy(() => import("./ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const ThreadDetails = lazy(() => import("./ThreadDetails").then((m) => ({ default: m.ThreadDetails })));
const PullsPanel = lazy(() => import("./PullRequests").then((m) => ({ default: m.PullsPanel })));

type Kind = NonNullable<AppState["panel"]>;
/** The kinds that take a pane of the chat's width. */
type Pane = Exclude<Kind, "details">;

/** The pane's own bounds, in px, and the width it starts with. */
export const MIN = 320;
export const MAX = 760;
export const DEFAULT_WIDTH = 460;
/** One press of an arrow key, and with Shift. */
const STEP = 24;
const STEP_LARGE = 96;
const KEY = "rustcode.rightPanelWidth";
/** The gap every floating card keeps from the window's edges, and from the chat lane. */
const CARD_GAP = 12;
/** The details card's width, and its gap from the window's edges; the chat lane clears it. */
export const DETAILS_CARD = { width: 280, gap: CARD_GAP } as const;
/** The surface a card of the right panel is drawn on: the details card and a pane alike. */
const CARD_SURFACE = "rc-glass rc-glass-strong rounded-2xl shadow-2";

const PANE_ID = "rc-side-panel";

/**
 * How tall a pane's card draws. A list view (`changes`, `pulls`) is as tall as
 * what it holds, and never taller than the room between its own top and the
 * composer: over that, the view scrolls inside it. A transcript (`peek`) has no
 * height to be as tall as, so that card takes the room it has.
 */
export function paneSizing(kind: Pane): { bottom?: string; maxHeight?: string } {
  const foot = `calc(var(--rc-composer-top, 0px) + ${CARD_GAP}px)`;
  if (kind === "peek") return { bottom: foot };
  return { maxHeight: `calc(100% - ${HEADER_HEIGHT + CARD_GAP}px - ${foot})` };
}

function clampWidth(width: number): number {
  return Math.min(MAX, Math.max(MIN, Math.round(width)));
}

function storedWidth(): number {
  try {
    const v = Number(localStorage.getItem(KEY));
    return v >= MIN && v <= MAX ? v : DEFAULT_WIDTH;
  } catch {
    return DEFAULT_WIDTH;
  }
}

/** Kept when it settles: the end of a drag, a key, a reset. Never per frame. */
function persist(width: number) {
  try {
    localStorage.setItem(KEY, String(width));
  } catch {
    // Not kept in a private window.
  }
}

/**
 * The width a pane's card draws: the one asked for (kept, so it comes back when
 * the window widens), no wider than the column leaves it between its two gaps.
 * 0 is before the first measurement, when the stored width stands as it is.
 */
export function panelWidth(wanted: number, available: number): number {
  const room = available > 0 ? Math.max(MIN, available - CARD_GAP * 2) : MAX;
  return Math.min(clampWidth(wanted), room);
}

/**
 * The card over the chat, for the lane to centre in the room beside it
 * (ui/ChatView.tsx): its width, 0 while no card is open.
 */
const cardListeners = new Set<() => void>();
let cardWidth = 0;

function subscribeCard(listener: () => void): () => void {
  cardListeners.add(listener);
  return () => cardListeners.delete(listener);
}

function publishCardWidth(width: number) {
  if (width === cardWidth) return;
  cardWidth = width;
  for (const listener of cardListeners) listener();
}

export function usePanelCardWidth(): number {
  return useSyncExternalStore(subscribeCard, () => cardWidth);
}

/**
 * The width after a resize key moves the edge; null for a key that does not.
 * `current` is the width on screen and `max` the widest it may draw, so a key
 * never promises a width the pane cannot show (a narrow window fits it down).
 */
export function resizedWidth(current: number, key: string, large: boolean, max: number = MAX): number | null {
  const step = large ? STEP_LARGE : STEP;
  const fit = (width: number) => Math.min(Math.max(MIN, max), clampWidth(width));
  switch (key) {
    // The edge is on the left, so left grows the pane and right shrinks it.
    case "ArrowLeft":
      return fit(current + step);
    case "ArrowRight":
      return fit(current - step);
    case "Home":
      return MIN;
    case "End":
      return fit(MAX);
    default:
      return null;
  }
}

/** Focus inside the pane that a key should not take for closing it. */
function inField(target: EventTarget | null): boolean {
  const el = target instanceof HTMLElement ? target : null;
  return !!el?.closest('input, textarea, select, [contenteditable="true"], [role="textbox"]');
}

export function RightPanel() {
  const panel = useApp((s) => s.panel);
  const panelKey = useShortcutHint("toggle-panel");
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const peekOn = useApp((s) => !!s.peekId);
  // What the panel shows: the open one, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(storedWidth);
  // The room the card is drawn in, measured on the column that holds the chat
  // (the card takes no slot, so this is the chat's own width); 0 until the first
  // measurement, when the stored width stands as it is.
  const [available, setAvailable] = useState(0);
  const slot = useRef<HTMLDivElement>(null);
  // The surface that is showing: the pane, or the details card.
  const surface = useRef<HTMLElement>(null);
  const restoreTo = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  useEffect(() => {
    // offsetParent is the column that holds the chat and this slot: the card
    // floats over it, so its width is the whole room a card may draw in.
    const host = slot.current?.offsetParent;
    if (!(host instanceof HTMLElement)) return;
    const measure = () => setAvailable(host.clientWidth);
    measure();
    return observeSize(host, measure);
  }, []);

  const open = !!panel;
  const kind = panel ?? shown;
  const card = kind === "details";
  const pane = kind && !card ? kind : null;
  const fitted = panelWidth(width, available);
  const widest = panelWidth(MAX, available);
  // The lane centres in the room beside the card, so it reads the width on
  // screen here and moves with the edge while it is dragged.
  const shownWidth = !panel ? 0 : card ? DETAILS_CARD.width : fitted;
  useEffect(() => publishCardWidth(shownWidth), [shownWidth]);

  // Keyboard focus follows the panel, and never sits where it cannot be seen:
  // a shortcut opens it with nothing focused, a surface closing takes the focus
  // with it. The composer is never covered, so a field being typed in keeps the
  // keyboard; closing hands the focus back to whatever had it.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `kind` swaps the surface (pane for the details card) while the panel stays open, and the focus must move with it
  useEffect(() => {
    const el = surface.current;
    const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const lost = !active || active === document.body;
    if (open) {
      if (!restoreTo.current) restoreTo.current = lost ? null : active;
      if (lost) el?.focus();
      return;
    }
    const inside = !!el && !!active && el.contains(active);
    if ((inside || lost) && restoreTo.current?.isConnected) restoreTo.current.focus();
    restoreTo.current = null;
  }, [open, kind]);

  const onResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    // From the width on screen, not the one asked for: a drag means what it shows.
    const start = fitted;
    let latest = fitted;
    let moved = false;
    const move = (ev: PointerEvent) => {
      latest = clampWidth(start - (ev.clientX - startX));
      moved = true;
      setWidth(latest);
    };
    const up = () => {
      // Kept once a drag ends, not on every frame of it, and not for a click
      // that never dragged: a click must not overwrite the width that was asked
      // for with the one this window happened to fit.
      if (moved) persist(latest);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const onResizeKey = (e: React.KeyboardEvent) => {
    const next = e.key === "Enter" ? DEFAULT_WIDTH : resizedWidth(fitted, e.key, e.shiftKey, widest);
    if (next === null) return;
    e.preventDefault();
    setWidth(next);
    persist(next);
  };

  // Escape closes whatever the panel shows, unless a field in it is being
  // edited (then Escape belongs to that field). It stops there: the app's own
  // window handler reads Escape as "back to the chat" off the chat views, and a
  // consumed one must not also leave the board or the review.
  const onSurfaceKey = (e: React.KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented || inField(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    setPanel(null);
  };

  const resetWidth = () => {
    setWidth(DEFAULT_WIDTH);
    persist(DEFAULT_WIDTH);
  };

  const tabs: { value: Pane; label: string }[] = [
    { value: "changes", label: "Changes" },
    ...(pullsOn ? [{ value: "pulls" as const, label: "Pull requests" }] : []),
    ...(peekOn ? [{ value: "peek" as const, label: "Thread" }] : []),
  ];

  return (
    <div ref={slot} className="relative shrink-0" style={{ width: 0 }} aria-hidden={!open}>
      {card && (
        <aside
          ref={surface}
          tabIndex={-1}
          aria-label="Thread details"
          onKeyDown={onSurfaceKey}
          onTransitionEnd={(e) => {
            if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
          }}
          className={cx(
            CARD_SURFACE,
            "absolute z-30 flex flex-col overflow-hidden outline-none transition-[opacity,translate] duration-200 ease-snappy",
            open ? "translate-y-0 opacity-100" : "-translate-y-1 opacity-0",
          )}
          style={{
            top: HEADER_HEIGHT + DETAILS_CARD.gap,
            right: DETAILS_CARD.gap,
            width: DETAILS_CARD.width,
            maxHeight: `calc(100% - ${HEADER_HEIGHT + DETAILS_CARD.gap * 2}px)`,
          }}
        >
          <Suspense fallback={<div className="h-24" />}>
            <ThreadDetails />
          </Suspense>
        </aside>
      )}
      {pane && (
        <aside
          ref={surface}
          id={PANE_ID}
          aria-label="Side panel"
          tabIndex={-1}
          onKeyDown={onSurfaceKey}
          className={cx(
            CARD_SURFACE,
            "rc-drill absolute z-20 flex flex-col overflow-hidden outline-none transition-[opacity,translate] duration-200 ease-snappy",
            open ? "translate-x-0 opacity-100" : "translate-x-3 opacity-0",
          )}
          style={{
            top: HEADER_HEIGHT + CARD_GAP,
            right: CARD_GAP,
            // Content high, capped above the composer so the foot of the chat keeps its field.
            ...paneSizing(pane),
            width: fitted,
          }}
          onTransitionEnd={(e) => {
            if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
          }}
        >
          <div
            data-tauri-drag-region
            style={{ height: HEADER_HEIGHT }}
            className="flex shrink-0 items-center gap-2 border-border border-b pr-2 pl-2.5"
          >
            {/* The views keep their labels and scroll before the close button leaves the row. */}
            <div className="flex min-w-0 items-center overflow-x-auto">
              <div className="flex shrink-0 items-center">
                {tabs.length > 1 ? (
                  <Segments label="Panel" value={pane} options={tabs} onChange={(v) => showPanel(v)} />
                ) : (
                  <span className="truncate px-1 font-semibold text-body">{tabs[0]?.label}</span>
                )}
              </div>
            </div>
            <span className="flex-1" data-tauri-drag-region />
            <Tooltip label={`Close (${panelKey})`}>
              <button
                type="button"
                aria-label="Close panel"
                onClick={() => setPanel(null)}
                className="flex size-7 shrink-0 items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
              >
                <X size={14} />
              </button>
            </Tooltip>
          </div>
          <Suspense fallback={<div className="flex-1" />}>
            {pane === "changes" && <ChangesPanel />}
            {pane === "pulls" && <PullsPanel />}
            {pane === "peek" && <ThreadPeek />}
          </Suspense>
          {open && (
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize panel"
              aria-controls={PANE_ID}
              aria-valuemin={MIN}
              aria-valuemax={widest}
              aria-valuenow={Math.round(fitted)}
              aria-valuetext={`${Math.round(fitted)} pixels wide`}
              tabIndex={0}
              onPointerDown={onResize}
              onDoubleClick={resetWidth}
              onKeyDown={onResizeKey}
              className="group absolute inset-y-0 left-0 z-10 w-[7px] cursor-col-resize outline-none focus-visible:bg-focus/20"
            >
              <div className="mx-auto h-full w-0.5 rounded-full bg-transparent group-hover:bg-fill-strong group-focus-visible:bg-focus" />
            </div>
          )}
        </aside>
      )}
    </div>
  );
}
