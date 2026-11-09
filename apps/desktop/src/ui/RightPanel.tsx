// The right panel (docs/design.md, Right panel): one card docked at the window's
// right edge for the thread's side views (Overview, Changes, Pull requests,
// Thread). It is a sibling of the chat column inside the chat view's row, so the
// chat keeps its place on the left and narrows by the card's width; the lane
// re-centres in the room that is left (ui/ChatView.tsx). The card runs from the
// header's foot to the window's bottom edge, inset by the same 12px gap on the
// edges it meets, on the surface it always had. Its left edge drags and answers
// the keyboard, its width is kept per thread, and it grows from and shrinks to
// nothing over 200ms when it opens or closes. While it is closed its content
// stays mounted, inert and unclickable, so nothing it draws can be reached. A
// window narrower than 980px has no room for both columns: there the panel opens
// as a sheet over a scrim instead, which Escape or a press on the scrim closes.
//
// Keys: the left edge is a `separator` that takes focus (←/→ move it 24px from
// the width on screen, Shift 96px, Home/End to the ends, ⏎ back to the
// default), Escape inside the panel closes it, focus moves in when a shortcut
// opens it with nothing focused, and closing hands the focus back to the
// header's toggle. The card is beside the composer rather than over it, so a
// field being typed in always keeps the keyboard.

import { X } from "lucide-react";
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
import { type AppState, setPanel, showPanel, useApp, useShortcutHint } from "../state/store";
import { HEADER_HEIGHT } from "./MainHeader";
import { cx, Segments, Tooltip } from "./primitives";
import { ThreadPeek } from "./ThreadPeek";

const ChangesPanel = lazy(() => import("./ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const ThreadDetails = lazy(() => import("./ThreadDetails").then((m) => ({ default: m.ThreadDetails })));
const PullsPanel = lazy(() => import("./PullRequests").then((m) => ({ default: m.PullsPanel })));

/** The views the panel shows. */
type Kind = NonNullable<AppState["panel"]>;

/** The card's own bounds, in px, and the width it starts with. */
export const MIN = 360;
export const MAX = 760;
export const DEFAULT_WIDTH = 520;
/** The room the chat column always keeps of the row, however wide the card asks to be. */
export const CHAT_MIN = 360;
/** One press of an arrow key, and with Shift. */
const STEP = 24;
const STEP_LARGE = 96;
/** A window no wider than this has no room for both columns: the panel opens as a sheet. */
export const SHEET_AT = 980;
/** The width the card is kept under, per thread; the shared key is what a thread with none starts at. */
const KEY = "rustcode.rightPanelWidth";
/** The gap the card keeps from the header's foot and from the window's right and bottom edges. */
const CARD_GAP = 12;
/** The strip the resize edge straddles: a `-left-1 w-2` handle, half either side of the card's edge. */
const HANDLE = 8;
/** The surface a card of the right panel is drawn on: the details card and a pane alike. */
const CARD_SURFACE = "rc-glass rc-glass-strong rounded-2xl shadow-2";
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

/** The width kept for a thread, else the one last kept for any thread. */
function storedWidth(threadId: string | null): number {
  try {
    const own = Number(threadId ? localStorage.getItem(`${KEY}:${threadId}`) : null);
    if (own >= MIN && own <= MAX) return own;
    const shared = Number(localStorage.getItem(KEY));
    if (shared >= MIN && shared <= MAX) return shared;
  } catch {
    // Not kept in a private window.
  }
  return DEFAULT_WIDTH;
}

/**
 * Kept when it settles: the end of a drag, a key, a reset. Never per frame. It
 * is kept for this thread, and as the width a thread that has none starts at.
 */
function persist(threadId: string | null, width: number) {
  try {
    if (threadId) localStorage.setItem(`${KEY}:${threadId}`, String(width));
    localStorage.setItem(KEY, String(width));
  } catch {
    // Not kept in a private window.
  }
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
  const threadId = useApp((s) => s.selectedId);
  const panelKey = useShortcutHint("toggle-panel");
  const pullsOn = useApp((s) => !!s.agent?.pullRequests);
  const peekOn = useApp((s) => !!s.peekId);
  // A side chat opened beside this thread gives its tab that name.
  const peekIsSideChat = useApp((s) => {
    const parent = s.sessions.find((x) => x.id === s.peekId)?.parent;
    return !!parent && relationshipOf(parent) === "side_chat";
  });
  // What the panel shows: the open one, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(() => storedWidth(threadId));
  const [viewport, setViewport] = useState(() => window.innerWidth);
  // The row the card shares with the chat, measured on the column that holds it; 0
  // until the first measurement, when the stored width stands as it is.
  const [row, setRow] = useState(0);
  const dock = useRef<HTMLDivElement>(null);
  const stopRow = useRef<(() => void) | null>(null);
  // The surface that is showing, and the element that had the focus before it opened.
  const surface = useRef<HTMLElement>(null);
  const restoreTo = useRef<HTMLElement | null>(null);
  // A drag in progress: the width it has reached, where the pointer started, and whether it moved.
  const drag = useRef<{ live: number; startX: number; moved: boolean } | null>(null);

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  // A thread keeps its own width; one that has none starts where the last one left off.
  useEffect(() => {
    setWidth(storedWidth(threadId));
  }, [threadId]);

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
    dock.current?.style.setProperty("--rc-panel-width", `${drag.current?.live ?? card}px`);
  });

  // The card and the chat share the row: the panel draws no wider than the room
  // the chat leaves it. The observer watches the row, which the card's own width
  // never changes, so measuring and drawing cannot chase each other.
  const attachDock = useCallback((el: HTMLDivElement | null) => {
    dock.current = el;
    stopRow.current?.();
    stopRow.current = null;
    const parent = el?.parentElement;
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

  const commit = (next: number) => {
    setWidth(next);
    persist(threadId, next);
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
    dock.current?.style.setProperty("--rc-panel-width", `${d.live}px`);
  };

  const onResizeEnd = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    if (d.moved) commit(d.live);
    else dock.current?.style.setProperty("--rc-panel-width", `${card}px`);
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
    // The panel's first view is the overview: Properties, pulls, deliverables,
    // the execution log and the rest of the details in one column (ui/ThreadDetails.tsx).
    { value: "details", label: "Overview" },
    { value: "changes", label: "Changes" },
    ...(pullsOn ? [{ value: "pulls" as const, label: "Pull requests" }] : []),
    ...(peekOn ? [{ value: "peek" as const, label: peekIsSideChat ? "Side chat" : "Thread" }] : []),
  ];

  const contents = kind && (
    <>
      <div
        data-tauri-drag-region
        style={{ height: HEADER_HEIGHT }}
        className="flex shrink-0 items-center gap-2 border-border border-b pr-2 pl-2"
      >
        {/* The views keep their labels and scroll before the close button leaves the row. */}
        <div className="flex min-w-0 items-center overflow-x-auto">
          <div className="flex shrink-0 items-center">
            {tabs.length > 1 ? (
              <Segments label="Panel" value={kind} options={tabs} onChange={(v) => showPanel(v)} />
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
      // The dock is a spacer, not a surface: it carries no z-index, so the transparent room above
      // the card stays under the header (`z-10`, ui/MainHeader.tsx) and never takes a click meant
      // for the header's right-hand controls. The card inside it is the part that rises.
      className={cx(
        "relative flex min-h-0 shrink-0 justify-end overflow-clip",
        !open && "pointer-events-none",
      )}
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
        tabIndex={-1}
        onKeyDown={onSurfaceKey}
        onTransitionEnd={(e) => {
          if (e.target === e.currentTarget && e.propertyName === "opacity" && !open) setShown(null);
        }}
        className={cx(
          CARD_SURFACE,
          // Above the header's glass (`z-10`): the header spans the whole view and fades 24px past
          // its foot, which would otherwise wash over the card's top 12px. The card starts below
          // the header's band, so nothing of it covers the header's own controls.
          "relative z-20 flex flex-col overflow-hidden outline-none transition-opacity duration-200",
          open ? "opacity-100" : "opacity-0",
        )}
        style={{
          width: "var(--rc-panel-width)",
          marginTop: HEADER_HEIGHT + CARD_GAP,
          marginRight: CARD_GAP,
          marginBottom: CARD_GAP,
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
