// The right panel (docs/design.md, Right panel): one docked, full-height pane
// at the window's right edge for the side views that take a pane (Changes,
// Pull requests, Thread) — the sidebar's twin on the other side: no inset, no
// floating card, a 1px seam and the same rail surface. It has tabs between the
// views, a close button and a draggable left edge, and it is responsive: it
// never grows past the room the chat column keeps (CHAT_FLOOR px, less in a
// column narrower than NARROW), so opening it narrows the conversation without
// ever squeezing it out — and when the column cannot hold both (OVERLAY_BELOW)
// it floats over the transcript instead, stopping above the composer so that
// stays usable, and leaves the chat its full width. Its slot takes its width at
// once (one reflow of the chat column, not one per frame), and the frame slides
// and fades in and out on the compositor; its content stays mounted while it
// closes (docs/desktop.md › Native frame rate).
//
// Keys: the left edge is a `separator` that takes focus (←/→ move it 24px from
// the width on screen, Shift 96px, Home/End to the ends, ⏎ back to the
// default), Escape inside the panel closes it — the pane or the details card —
// focus moves in when a shortcut opens it with nothing focused, and closing
// gives the focus back to whatever had it. The composer is never covered: a
// floating pane ends above it, and the composer's drawer and approvals paint
// over one (ui/ChatView.tsx), so a field being typed in keeps the keyboard.
//
// Thread details is not a pane: it floats over the chat as a small card at the
// top right, so reading it never narrows the transcript.

import { X } from "lucide-react";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
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
/** What the chat column keeps: this much, or NARROW_CHAT_FLOOR below NARROW. */
export const CHAT_FLOOR = 420;
export const NARROW = 900;
export const NARROW_CHAT_FLOOR = 360;
/** One press of an arrow key, and with Shift. */
const STEP = 24;
const STEP_LARGE = 96;
const KEY = "rustcode.rightPanelWidth";
/** The details card's width, and its gap from the window's edges; the chat lane clears it. */
export const DETAILS_CARD = { width: 280, gap: 12 } as const;

const PANE_ID = "rc-side-panel";

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

/** What the chat column keeps beside the pane: less in a column too narrow for both. */
export function chatFloor(available: number): number {
  return available >= NARROW ? CHAT_FLOOR : NARROW_CHAT_FLOOR;
}

/** The widest the pane may draw: its own cap, and the room past the chat's floor. */
export function paneMax(available: number): number {
  if (available <= 0) return MAX;
  return Math.max(MIN, Math.min(MAX, available - chatFloor(available)));
}

/** Below this the pane cannot dock beside a chat that still reads: it floats over it. */
export const OVERLAY_BELOW = MIN + CHAT_FLOOR;

/** Whether the pane floats over the chat instead of taking a slot beside it. */
export function overlays(available: number): boolean {
  return available > 0 && available < OVERLAY_BELOW;
}

/** The width of the floating pane: the one asked for, no wider than the column. */
export function overlayWidth(wanted: number, available: number): number {
  return Math.max(MIN, Math.min(clampWidth(wanted), Math.round(available)));
}

/**
 * The width the pane takes: the one asked for (kept, so it comes back when the
 * window widens), fitted to the room the chat leaves it.
 */
export function fittedWidth(wanted: number, available: number): number {
  return Math.min(clampWidth(wanted), paneMax(available));
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
  // The room the pane shares with the chat, measured on their common parent
  // (the chat column absorbs its own width, so growing the pane never changes
  // it); 0 until the first measurement, when the stored width stands as it is.
  const [available, setAvailable] = useState(0);
  const slot = useRef<HTMLDivElement>(null);
  // The surface that is showing: the pane, or the details card.
  const surface = useRef<HTMLElement>(null);
  const restoreTo = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (panel) setShown(panel);
  }, [panel]);

  useEffect(() => {
    // offsetParent is the column that holds the chat and this slot: the chat is
    // `flex-1`, so this width is the same whatever the pane takes.
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
  // No room for both: the pane floats over the chat rather than squeeze it out.
  const floating = overlays(available);
  const fitted = floating ? overlayWidth(width, available) : fittedWidth(width, available);
  const widest = floating ? Math.max(MIN, Math.round(available)) : paneMax(available);

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
    <div
      ref={slot}
      // A card floats past its edges; a docked pane clips its own, a floating
      // one reaches left over the chat.
      className={cx("relative shrink-0", pane && !floating ? "overflow-hidden" : "overflow-visible")}
      style={{ width: pane && !floating ? fitted : 0 }}
      aria-hidden={!open}
    >
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
            "rc-glass rc-glass-strong absolute z-30 flex flex-col overflow-hidden rounded-2xl shadow-2 outline-none transition-[opacity,translate] duration-200 ease-snappy",
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
            "rc-drill absolute flex flex-col overflow-hidden border-border border-l bg-sidebar outline-none transition-[opacity,translate] duration-200 ease-snappy",
            floating
              ? "top-0 right-0 bottom-[calc(var(--rc-composer-top,0px)+6px)] z-20 border-b"
              : "inset-y-0 left-0",
            open ? "translate-x-0 opacity-100" : "translate-x-3 opacity-0",
          )}
          style={{ width: fitted }}
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
