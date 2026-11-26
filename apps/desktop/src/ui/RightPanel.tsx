// One resizable card: Details (the home, everything on one card) and the full
// views it opens, Changes, Pull requests and the beside thread, each with a way
// back. Every tab and every thread share one width,
// so switching tabs or threads never moves the card's edge or the chat beside
// it. The card is a second card beside the chat's, on the same ground with the
// same edge and corners, the window's whole height: it renders into the slot
// after the main card (`PANEL_SLOT_ID`, App.tsx), so the chat's card narrows
// as it opens instead of being drawn over, and it carries the panel's toggle
// itself (ui/PanelToggle.tsx). A narrow window opens it as a sheet over a
// scrim instead.
//
// Keys: the left edge is a `separator` that takes focus (←/→ move it 24px from
// the width on screen, Shift 96px, Home/End to the ends, ⏎ back to the
// default), Escape inside the panel and the toggle on the card close it, focus moves in when a shortcut
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
import { createPortal } from "react-dom";
import { relationshipOf } from "../acp/types";
import { observeSize } from "../lib/resize";
import { readStorage, writeStorage } from "../lib/storage";
import { type AppState, setPanel, showPanel, useApp } from "../state/store";
import { PANEL_SHEET_AT } from "../state/viewActions";
import {
  CircleDot,
  GitCompareArrows,
  GitPullRequest,
  Globe,
  Info,
  type LucideIcon,
  MessagesSquare,
} from "./icons";
import { HEADER_HEIGHT } from "./MainHeader";
import { PanelToggle, useCardOverHeader } from "./PanelToggle";
import { cx, Pill, Tooltip } from "./primitives";
import { ThreadPeek } from "./ThreadPeek";

const ChangesPanel = lazy(() => import("./ChangesPanel").then((m) => ({ default: m.ChangesPanel })));
const ThreadDetails = lazy(() => import("./ThreadDetails").then((m) => ({ default: m.ThreadDetails })));
const PullsPanel = lazy(() => import("./PullRequests").then((m) => ({ default: m.PullsPanel })));
const IssuesPanel = lazy(() => import("./IssuesPanel").then((m) => ({ default: m.IssuesPanel })));
const BrowserPreviewPanel = lazy(() =>
  import("./BrowserPreviewPanel").then((m) => ({ default: m.BrowserPreviewPanel })),
);

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
/** The gap the card keeps from the window's edges and from the chat's card: the main card's own margin. */
const CARD_GAP = 8;
/** The surface the right panel's card is drawn on, whichever tab it shows: the chat card's own. */
const CARD_SURFACE = "rc-panel-card";
/** The element after the main card the docked panel renders into (App.tsx). */
export const PANEL_SLOT_ID = "rc-panel-slot";
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

interface PanelTab {
  kind: Kind;
  label: string;
  icon: LucideIcon;
}

/** One view's tab: its icon alone, and its name beside it while it is the one showing. */
function PanelTabButton({ tab, active, onSelect }: { tab: PanelTab; active: boolean; onSelect: () => void }) {
  const Icon = tab.icon;
  if (active) {
    return (
      <Pill role="tab" aria-selected aria-label={tab.label} onClick={onSelect}>
        <Icon size={14} />
        <span className="truncate">{tab.label}</span>
      </Pill>
    );
  }
  return (
    <Tooltip label={tab.label}>
      <button
        type="button"
        role="tab"
        aria-selected={false}
        aria-label={tab.label}
        onClick={onSelect}
        className="inline-flex size-7 shrink-0 items-center justify-center rounded-full text-fg-muted hover:bg-fill hover:text-fg active:bg-fill-strong"
      >
        <Icon size={14} />
      </button>
    </Tooltip>
  );
}

export function RightPanel() {
  const panel = useApp((s) => s.panel);
  // A side chat opened beside this thread gives its tab that name.
  const peekIsSideChat = useApp((s) => {
    const parent = s.sessions.find((x) => x.id === s.peekId)?.parent;
    return !!parent && relationshipOf(parent) === "side_chat";
  });
  // What the panel shows: the open one, or while closing the one that was.
  const [shown, setShown] = useState<Kind | null>(panel);
  const [width, setWidth] = useState(storedWidth);
  const [viewport, setViewport] = useState(() => window.innerWidth);
  // The room the chat's card and this one share, measured on both; 0 until the first
  // measurement, when the stored width stands as it is.
  const [row, setRow] = useState(0);
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const dock = useRef<HTMLDivElement>(null);
  // The surface that is showing, and the element that had the focus before it opened.
  const surface = useRef<HTMLElement>(null);
  const restoreTo = useRef<HTMLElement | null>(null);
  // A drag in progress: the width it has reached, where the pointer started, and whether it moved.
  const drag = useRef<{ live: number; startX: number; moved: boolean } | null>(null);

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  useEffect(() => setSlot(document.getElementById(PANEL_SLOT_ID)), []);

  // The chat's card is the row's flexible part and this one's slot takes what the card asks, so
  // together they are always the room the two share, whatever the card's width does.
  useEffect(() => {
    const main = slot?.previousElementSibling;
    if (!slot || !(main instanceof HTMLElement)) return;
    const measure = () => setRow(main.offsetWidth + slot.offsetWidth);
    measure();
    const stopMain = observeSize(main, measure);
    const stopSlot = observeSize(slot, measure);
    return () => {
      stopMain();
      stopSlot();
    };
  }, [slot]);

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
  const overHeader = useCardOverHeader();
  const top = overHeader ? CARD_GAP : HEADER_HEIGHT + CARD_GAP;

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
  });

  const attachDock = useCallback((el: HTMLDivElement | null) => {
    dock.current = el;
  }, []);

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

  function publish(width: number) {
    dock.current?.style.setProperty("--rc-panel-width", `${width}px`);
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
    if (dock.current) dock.current.style.transition = "none";
    publish(d.live);
  };

  const onResizeEnd = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    if (dock.current) dock.current.style.removeProperty("transition");
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

  // Every view has a tab, Thread info first; a thread opened beside this one gets one while it shows.
  const tabs: PanelTab[] = [
    { kind: "details", label: "Thread info", icon: Info },
    { kind: "changes", label: "Changes", icon: GitCompareArrows },
    { kind: "pulls", label: "Pull requests", icon: GitPullRequest },
    { kind: "issues", label: "Issues", icon: CircleDot },
    { kind: "preview", label: "Preview", icon: Globe },
    ...(kind === "peek"
      ? [{ kind: "peek" as const, label: peekIsSideChat ? "Side chat" : "Thread", icon: MessagesSquare }]
      : []),
  ];

  const contents = kind && (
    <>
      <div
        data-tauri-drag-region
        role="tablist"
        aria-label="Side panel views"
        className="flex h-11 shrink-0 items-center gap-0.5 border-separator border-b px-2"
      >
        {tabs.map((tab) => (
          <PanelTabButton
            key={tab.kind}
            tab={tab}
            active={tab.kind === kind}
            onSelect={() => showPanel(tab.kind)}
          />
        ))}
        <span className="flex-1" data-tauri-drag-region />
        <PanelToggle iconOnly />
      </div>
      <Suspense fallback={<div className="flex-1" />}>
        {kind === "details" && <ThreadDetails />}
        {kind === "changes" && <ChangesPanel />}
        {kind === "pulls" && <PullsPanel />}
        {kind === "issues" && <IssuesPanel />}
        {kind === "preview" && <BrowserPreviewPanel />}
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

  const docked = (
    <div
      ref={attachDock}
      aria-hidden={!open}
      inert={!open}
      className="flex h-full min-h-0 shrink-0 justify-end overflow-clip"
      style={
        {
          width: open ? `calc(var(--rc-panel-width) + ${CARD_GAP}px)` : 0,
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
          "relative flex shrink-0 flex-col self-stretch overflow-hidden outline-none transition-opacity duration-200",
          open ? "opacity-100" : "opacity-0",
        )}
        style={{
          width: "var(--rc-panel-width)",
          marginTop: top,
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
            className="group absolute inset-y-0 left-0 z-10 w-2 cursor-col-resize touch-none select-none outline-none keyboard:focus-visible:bg-focus/20"
          >
            <span
              aria-hidden
              className="pointer-events-none absolute inset-y-0 left-0 w-px bg-transparent group-hover:bg-fill-strong keyboard:group-focus-visible:bg-focus"
            />
          </div>
        )}
      </aside>
    </div>
  );
  return slot ? createPortal(docked, slot) : docked;
}
