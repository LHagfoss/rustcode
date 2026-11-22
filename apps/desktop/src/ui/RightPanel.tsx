// One resizable card with a tab per view: Details (the default), Changes, Pull
// requests and the beside thread. Every tab and every thread share one width,
// so switching tabs or threads never moves the card's edge or the chat beside
// it. Every view draws on one surface, the chat's own dark ground with a
// hairline edge, as tall as its content but never shorter than it is wide
// (about square), so a short tab does not shrink to a strip. On a wide window the card floats over the chat's row: the
// transcript keeps the row's full width (its scrollbar stays at the window's
// edge) and pads its lane by the room the card takes, published on the row as
// `--rc-panel-room`, so the lane stays centred in what is left. A narrow window
// opens it as a sheet over a scrim instead.
//
// Keys: the left edge is a `separator` that takes focus (←/→ move it 24px from
// the width on screen, Shift 96px, Home/End to the ends, ⏎ back to the
// default), Escape inside the panel and the header's toggle close it (the card has no close button), focus moves in when a shortcut
// opens it with nothing focused, and closing hands the focus back to the
// header's toggle. The card is beside the composer rather than over it, so a
// field being typed in always keeps the keyboard.

import {
  type CSSProperties,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { relationshipOf } from "../acp/types";
import { observeSize } from "../lib/resize";
import { readStorage, writeStorage } from "../lib/storage";
import { type AppState, setPanel, showPanel, useApp } from "../state/store";
import { PANEL_SHEET_AT } from "../state/viewActions";
import { HEADER_HEIGHT } from "./MainHeader";
import { cx, Segments } from "./primitives";
import { ThreadPeek } from "./ThreadPeek";

const ChangesPanel = lazy(() => import("./ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const ThreadDetails = lazy(() => import("./ThreadDetails").then((m) => ({ default: m.ThreadDetails })));
const PullsPanel = lazy(() => import("./PullRequests").then((m) => ({ default: m.PullsPanel })));

/** The views the panel shows. */
type Kind = NonNullable<AppState["panel"]>;

/** The card's own bounds, in px, and the width it starts with. */
export const MIN = 320;
export const MAX = 560;
export const DEFAULT_WIDTH = 420;
/** The room the chat column always keeps of the row, however wide the card asks to be. */
export const CHAT_MIN = 360;
/** One press of an arrow key, and with Shift. */
const STEP = 24;
const STEP_LARGE = 96;
/** A window no wider than this has no room for both columns: the panel opens as a sheet. */
export const SHEET_AT = PANEL_SHEET_AT;
/** The width the card is kept under: one for every tab and thread. */
const KEY = "rustcode.rightPanelWidth";
/** The gap the card keeps from the header's foot and from the window's right and bottom edges. */
const CARD_GAP = 12;
/** The strip the resize edge straddles: a `-left-1 w-2` handle, half either side of the card's edge. */
const HANDLE = 8;
/** The surface the right panel's card is drawn on, whichever tab it shows. */
const CARD_SURFACE = "rounded-2xl border border-border bg-bg";
/** The row is told when the card's width moves with it, so the lane's padding follows in step. */
const MOVING = "rc-panel-moving";
/**
 * A sheet is opaque: it opens over a scrim that is already blurred, and one filter never stacks on
 * another. It sits on the chat's own `bg-bg`, as the settings panel does, so a card or a picture
 * inside it (`bg-surface`, Viz, an agent's steps) still steps off the sheet instead of vanishing.
 */
const SHEET_SURFACE = "bg-bg shadow-2";

const PANE_ID = "rc-side-panel";

/**
 * The widest the card may draw in this window: its own cap, 70% of the window,
 * and the row it shares with the chat less the room the chat always keeps. Never
 * under its own minimum, so a window too narrow for even that still gets a card
 * (`MIN`) rather than an inverted clamp.
 */
export function panelMax(viewport: number, row: number): number {
  const fraction = Math.floor(viewport * 0.7);
  const shared = row > 0 ? Math.floor(row) - CHAT_MIN : Number.POSITIVE_INFINITY;
  return Math.max(MIN, Math.min(MAX, fraction, shared));
}

/**
 * The width the card draws: the one asked for, kept within what the window and
 * the row allow. 0 is before the row is measured, when the window's own caps
 * stand until it is.
 */
export function panelWidth(wanted: number, viewport: number, row: number): number {
  return Math.min(Math.min(MAX, Math.max(MIN, Math.round(wanted))), panelMax(viewport, row));
}

/**
 * The width after a resize key moves the edge; null for a key that does not.
 * `current` is the width on screen and `max` the widest it may draw, so a key
 * never promises a width the card cannot show (a narrow window fits it down).
 */
export function resizedWidth(current: number, key: string, large: boolean, max: number = MAX): number | null {
  const step = large ? STEP_LARGE : STEP;
  const fit = (width: number) =>
    Math.min(Math.max(MIN, max), Math.min(MAX, Math.max(MIN, Math.round(width))));
  switch (key) {
    // The edge is on the left, so left grows the card and right shrinks it.
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

/** Whether this window is too narrow to hold the chat and the card side by side. */
export function panelIsSheet(viewport: number): boolean {
  return viewport <= SHEET_AT;
}

/** The width kept for the panel, else the default. */
function storedWidth(): number {
  const kept = Number(readStorage(KEY));
  return kept >= MIN && kept <= MAX ? kept : DEFAULT_WIDTH;
}

/** Kept when it settles: the end of a drag, a key, a reset. Never per frame. */
function persist(width: number) {
  writeStorage(KEY, String(width));
}

/** Focus inside the panel that a key should not take for closing it. */
function inField(target: EventTarget | null): boolean {
  const el = target instanceof HTMLElement ? target : null;
  return !!el?.closest('input, textarea, select, [contenteditable="true"], [role="textbox"]');
}

/** The header's button that opens the panel: where a close hands focus when nothing else had it. */
function headerToggle(): HTMLElement | null {
  return document.querySelector<HTMLElement>("[data-panel-toggle]");
}

export function RightPanel() {
  const panel = useApp((s) => s.panel);
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const peekOn = useApp((s) => !!s.peekId);
  // A side chat opened beside this thread gives its tab that name.
  const peekIsSideChat = useApp((s) => {
    const parent = s.sessions.find((x) => x.id === s.peekId)?.parent;
    return !!parent && relationshipOf(parent) === "side_chat";
  });
  // What the panel shows: the open one, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(storedWidth);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  // The row the card shares with the chat, measured on the column that holds it; 0
  // until the first measurement, when the stored width stands as it is.
  const [row, setRow] = useState(0);
  const dock = useRef<HTMLDivElement>(null);
  // The row the card floats over, which carries `--rc-panel-room` for the chat's lane.
  const rowEl = useRef<HTMLElement | null>(null);
  const stopRow = useRef<(() => void) | null>(null);
  // The surface that is showing, and the element that had the focus before it opened.
  const surface = useRef<HTMLElement>(null);
  const restoreTo = useRef<HTMLElement | null>(null);
  // A drag in progress: the width it has reached, where the pointer started, and whether it moved.
  const drag = useRef<{ live: number; startX: number; moved: boolean } | null>(null);

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  useEffect(() => {
    const onResize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const open = !!panel;
  const kind = panel ?? shown;
  const sheet = panelIsSheet(viewport);
  const card = panelWidth(width, viewport, row);
  const widest = panelMax(viewport, row);

  // Width animates only for an open or a close: any other change (a tab, a
  // window resize, the end of a drag) turns the transition off in the same
  // commit that changes the width, so the browser never starts one. The next
  // open or close turns it back on.
  const [motion, setMotion] = useState({ open, width: card, enabled: true });
  if (motion.open !== open || motion.width !== card) {
    setMotion({ open, width: card, enabled: motion.open !== open });
  }

  // The card's width lives in a custom property, so a drag writes one style per
  // frame and re-renders nothing; a commit republishes it from state. It is also
  // re-applied after any other render, so a streaming thread mid-drag does not
  // snap the card back to the width it started from.
  useLayoutEffect(() => {
    publish(drag.current?.live ?? card);
    rowEl.current?.classList.toggle(MOVING, motion.enabled && !drag.current);
  });

  // The card and the chat share the row: the panel draws no wider than the room
  // the chat leaves it. The observer watches the row, which the card's own width
  // never changes, so measuring and drawing cannot chase each other.
  const attachDock = useCallback((el: HTMLDivElement | null) => {
    dock.current = el;
    stopRow.current?.();
    stopRow.current = null;
    // A sheet takes no room from the row: the lane gets its whole width back.
    if (!el) {
      rowEl.current?.style.removeProperty("--rc-panel-room");
      rowEl.current?.classList.remove(MOVING);
    }
    const parent = el?.parentElement;
    rowEl.current = parent ?? null;
    if (!parent) return;
    const measure = () => setRow(parent.clientWidth);
    measure();
    stopRow.current = observeSize(parent, measure);
  }, []);
  useEffect(() => () => stopRow.current?.(), []);

  // Keyboard focus follows the panel, and never sits where it cannot be seen: a
  // shortcut opens it with nothing focused, a card closing takes the focus with
  // it. Closing hands the focus back to the header's toggle when nothing else
  // was there before.
  // biome-ignore lint/correctness/useExhaustiveDependencies: `kind` swaps the view inside the surface while the panel stays open, and the card's own focusable content comes and goes with it
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
    const back = restoreTo.current?.isConnected ? restoreTo.current : headerToggle();
    if (inside || lost) back?.focus();
    restoreTo.current = null;
  }, [open, kind]);

  // The card's width, and the room it takes from the chat's lane: the card, its gap and the strip
  // its resize edge straddles, or nothing while it is closed.
  function publish(width: number) {
    dock.current?.style.setProperty("--rc-panel-width", `${width}px`);
    rowEl.current?.style.setProperty("--rc-panel-room", open ? `${width + HANDLE + CARD_GAP}px` : "0px");
  }

  const commit = (next: number) => {
    setWidth(next);
    persist(next);
  };

  // A drag writes the width on the frame it is dragged on and keeps it once the
  // pointer is up, never before: a click that does not drag leaves the width that
  // was asked for alone. The pointer is captured, so the edge keeps the pointer
  // until it is released, cancelled or lost.
  const onResize = (e: React.PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { live: card, startX: e.clientX, moved: false };
  };

  const onResizeMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    d.moved = d.moved || e.clientX !== d.startX;
    d.live = Math.min(Math.max(MIN, Math.round(card - (e.clientX - d.startX))), widest);
    // A drag is followed frame by frame, not eased after.
    rowEl.current?.classList.remove(MOVING);
    publish(d.live);
  };

  const onResizeEnd = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    if (d.moved) commit(d.live);
    else publish(card);
  };

  const onResizeKey = (e: React.KeyboardEvent) => {
    const next =
      e.key === "Enter"
        ? panelWidth(DEFAULT_WIDTH, viewport, row)
        : resizedWidth(card, e.key, e.shiftKey, widest);
    if (next === null) return;
    e.preventDefault();
    commit(next);
  };

  // Escape closes the panel, unless a field in it is being edited (then Escape
  // belongs to that field). It stops there: the app's own window handler reads
  // Escape as "back to the chat" off the chat views, and a consumed one must not
  // also leave the board or the review.
  const onSurfaceKey = (e: React.KeyboardEvent) => {
    if (e.key !== "Escape" || e.defaultPrevented || inField(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    setPanel(null);
  };

  // A sheet is modal: Escape closes it wherever the keyboard is, since the scrim
  // already keeps the chat behind it out of reach. A field inside it still owns
  // the key, the way it does in the docked card.
  useEffect(() => {
    if (!open || !sheet) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const target = e.target instanceof Node ? e.target : null;
      const inside = !!target && !!surface.current?.contains(target);
      if (inside && inField(target)) return;
      e.preventDefault();
      setPanel(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, sheet]);

  const tabs: { value: Kind; label: string }[] = [
    { value: "details", label: "Details" },
    { value: "changes", label: "Changes" },
    ...(pullsOn ? [{ value: "pulls" as const, label: "Pull requests" }] : []),
    ...(peekOn ? [{ value: "peek" as const, label: peekIsSideChat ? "Side chat" : "Thread" }] : []),
  ];

  const contents = kind && (
    <>
      <div data-tauri-drag-region className="flex h-12 shrink-0 items-center gap-2 px-3">
        {/* The views keep their labels and scroll sideways when the card is narrow. The header's
            toggle and Escape close the panel, so the card carries no close button of its own. */}
        <div className="flex min-w-0 items-center overflow-x-auto">
          <div className="flex shrink-0 items-center">
            {tabs.length > 1 ? (
              // Soft pills, as T3 Code's panel tabs: the pick sits on a quiet fill, never the white
              // knob, so the panel stays secondary to the chat beside it.
              <Segments
                label="Panel"
                value={kind}
                options={tabs}
                onChange={showPanel}
                tabs={{ controls: PANE_ID }}
              />
            ) : (
              <span className="truncate px-1 font-semibold text-body">{tabs[0]?.label}</span>
            )}
          </div>
        </div>
        <span className="flex-1" data-tauri-drag-region />
      </div>
      <Suspense fallback={<div className="flex-1" />}>
        {kind === "details" && <ThreadDetails />}
        {kind === "changes" && <ChangesPanel />}
        {kind === "pulls" && <PullsPanel />}
        {kind === "peek" && <ThreadPeek />}
      </Suspense>
    </>
  );

  // A narrow window cannot hold both columns: the panel opens over the chat
  // instead, on the same card surface, with a scrim that a press or Escape closes.
  if (sheet) {
    return (
      <div
        className={cx("fixed inset-0 z-40", !open && "pointer-events-none")}
        aria-hidden={!open}
        inert={!open}
      >
        <div
          className={cx(
            "rc-scrim absolute inset-0 transition-opacity duration-200",
            open ? "opacity-100" : "opacity-0",
          )}
          onMouseDown={() => setPanel(null)}
        />
        <aside
          ref={surface}
          id={PANE_ID}
          role="dialog"
          aria-modal="true"
          aria-label="Side panel"
          data-focus-area="panel"
          data-tab-free
          tabIndex={-1}
          onKeyDown={onSurfaceKey}
          onTransitionEnd={(e) => {
            if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
          }}
          className={cx(
            SHEET_SURFACE,
            "absolute inset-y-0 right-0 flex w-[min(88vw,380px)] flex-col overflow-hidden rounded-l-2xl outline-none transition-[opacity,translate] duration-200 ease-snappy",
            open ? "translate-x-0 opacity-100" : "translate-x-4 opacity-0",
          )}
        >
          {contents}
        </aside>
      </div>
    );
  }

  return (
    <div
      ref={attachDock}
      aria-hidden={!open}
      inert={!open}
      // The dock floats over the row's right edge and takes no pointer itself: the transcript under
      // it still scrolls, and it carries no z-index, so the room above the card stays under the
      // header (`z-10`, ui/MainHeader.tsx) and never takes a click meant for the header's
      // right-hand controls. The card inside it is the part that rises and takes the pointer.
      className="pointer-events-none absolute inset-y-0 right-0 flex min-h-0 justify-end overflow-clip"
      style={
        {
          // The card's width and the 12px gap and the strip its resize edge straddles.
          width: open ? `calc(var(--rc-panel-width) + ${HANDLE + CARD_GAP}px)` : 0,
          "--rc-panel-width": `${card}px`,
          ...(motion.enabled ? { transition: "width 200ms var(--ease-snappy)" } : null),
        } as CSSProperties
      }
    >
      <aside
        ref={surface}
        id={PANE_ID}
        aria-label="Side panel"
        data-focus-area="panel"
        tabIndex={-1}
        onKeyDown={onSurfaceKey}
        onTransitionEnd={(e) => {
          if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
        }}
        className={cx(
          CARD_SURFACE,
          // As tall as what it shows, up to the room the row gives it.
          "self-start",
          // Above the header's glass (`z-10`): the header spans the whole view and fades 24px past
          // its foot, which would otherwise wash over the card's top 12px. The card starts below
          // the header's band, so nothing of it covers the header's own controls.
          "relative z-20 flex flex-col overflow-hidden outline-none transition-opacity duration-200",
          open ? "pointer-events-auto opacity-100" : "opacity-0",
        )}
        style={{
          width: "var(--rc-panel-width)",
          marginTop: HEADER_HEIGHT + CARD_GAP,
          marginRight: CARD_GAP,
          // Fit to the content, capped by the window alone: the card may run down to 12px above the
          // window's bottom edge, whatever the composer or anything else in the chat is doing.
          marginBottom: CARD_GAP,
          maxHeight: `calc(100% - ${HEADER_HEIGHT + 2 * CARD_GAP}px)`,
          // Never shorter than it is wide, so the card stays about square whatever the tab shows;
          // the window's own height still wins.
          minHeight: `min(var(--rc-panel-width), calc(100% - ${HEADER_HEIGHT + 2 * CARD_GAP}px))`,
        }}
      >
        {contents}
        {open && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize panel"
            aria-controls={PANE_ID}
            aria-valuemin={MIN}
            aria-valuemax={widest}
            aria-valuenow={Math.round(card)}
            aria-valuetext={`${Math.round(card)} pixels wide`}
            tabIndex={0}
            onPointerDown={onResize}
            onPointerMove={onResizeMove}
            onPointerUp={onResizeEnd}
            onPointerCancel={onResizeEnd}
            onLostPointerCapture={onResizeEnd}
            onDoubleClick={() => commit(panelWidth(DEFAULT_WIDTH, viewport, row))}
            onKeyDown={onResizeKey}
            className="group absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize touch-none select-none outline-none keyboard:focus-visible:bg-focus/20"
          >
            <span
              aria-hidden
              className="pointer-events-none absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-transparent group-hover:bg-fill-strong keyboard:group-focus-visible:bg-focus"
            />
          </div>
        )}
      </aside>
    </div>
  );
}
