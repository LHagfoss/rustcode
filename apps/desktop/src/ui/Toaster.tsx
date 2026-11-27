// Up to three toasts, newest in front (state/toasts.ts keeps the queue), on the composer's glass in
// the corner Settings › Appearance › Toast position picks (bottom right by default). A dot says the
// tone; the × shows on hover or focus. At rest the cards behind the front one take its height and
// hide their text, so only their edges peek past it. Info and success leave after 2.5 s, errors
// after 5 s. The pointer or focus on the stack, a drag, or the window in the background stops the
// clock. A click on the card, ×, Escape, or a horizontal drag past 48px (or a quick
// horizontal flick) dismisses; a drag never selects text anywhere.

import { type CSSProperties, type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { observeSize } from "../lib/resize";
import { dragOpacity, dragState, holdSelection, isClick, startsDrag, TOAST_DRAG_MS } from "../lib/toastDrag";
import { dismissToast, expireToasts, setToastsPaused, type Toast, useApp } from "../state/store";
import { type ToastPosition, toastFromTop, useToastPosition } from "../state/toastPosition";
import { nextExpiry, TOAST_EXIT_MS } from "../state/toasts";
import { X } from "./icons";
import { cx, Spinner } from "./primitives";

/** How far the cards behind the front one peek past its edge, and how much smaller each is. */
const TOAST_PEEK = 6;
const TOAST_SHRINK = 0.04;
/** The stack's distance from the window's edges. */
const TOAST_INSET = 16;
/** At the top, the stack clears the window's header. */
const TOAST_TOP = 52;
/** The 8px between fanned-out cards. */
const TOAST_GAP = 8;
/** A quiet settle on the check after a spinner finishes. */
const TOAST_SETTLE_MS = 320;

/** A popover, menu or dialog over the toast takes Escape and the click first. */
function aboveToasts(): boolean {
  return !!document.querySelector(
    '[data-radix-popper-content-wrapper]:not(:has([role="tooltip"])), [role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]',
  );
}

interface DragSession {
  pointerId: number;
  x: number;
  y: number;
  /** When the press began, for a flick. */
  at: number;
  offset: number;
  moved: boolean;
  /** Puts the document back; safe to call twice. */
  release: () => void;
}

function ToastCard({
  toast,
  compactHeight,
  fromTop,
  onDrag,
}: {
  toast: Toast;
  /** A card behind the front one at rest: drawn at the front card's height with its text hidden. */
  compactHeight?: number;
  /** Anchored at the top: it drops in from above. */
  fromTop: boolean;
  onDrag: (dragging: boolean) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<DragSession | null>(null);
  const [dragging, setDragging] = useState(false);
  const [flung, setFlung] = useState(false);

  // Whatever ends the drag, the document gets its selection back: pointerup,
  // pointercancel, a lost capture, the window blurring, or this card leaving.
  const end = useCallback(
    (flyOut: boolean) => {
      const session = drag.current;
      const el = ref.current;
      drag.current = null;
      session?.release();
      setDragging(false);
      onDrag(false);
      if (session && el?.hasPointerCapture(session.pointerId)) {
        el.releasePointerCapture(session.pointerId);
      }
      if (flyOut && session && el) {
        el.style.transition = `translate ${TOAST_EXIT_MS}ms var(--ease-snappy), opacity ${TOAST_EXIT_MS}ms var(--ease-snappy)`;
        el.style.translate = `${(Number.parseFloat(el.style.translate) || 0) - 64}px`;
        el.style.opacity = "0";
        setFlung(true);
        dismissToast(toast.id);
        return;
      }
      if (el) {
        el.style.transition = `translate ${TOAST_DRAG_MS}ms var(--ease-snappy), opacity ${TOAST_DRAG_MS}ms var(--ease-snappy)`;
        el.style.translate = "";
        el.style.opacity = "";
      }
    },
    [onDrag, toast.id],
  );
  useEffect(() => () => end(false), [end]);

  useEffect(() => {
    if (!dragging) return;
    const blur = () => end(false);
    window.addEventListener("blur", blur);
    return () => window.removeEventListener("blur", blur);
  }, [dragging, end]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || drag.current || toast.leaving || !startsDrag(e.target as HTMLElement)) return;
    // No native drag and no text selection, even where the platform would
    // start one despite the document's `user-select: none`.
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    const el = e.currentTarget;
    const style = getComputedStyle(el);
    const offset = Number.parseFloat(style.translate) || 0;
    const opacity = style.opacity;
    el.style.transition = "none";
    el.style.translate = `${offset}px`;
    el.style.opacity = opacity;
    el.setPointerCapture(e.pointerId);
    drag.current = {
      pointerId: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      at: e.timeStamp,
      offset,
      moved: false,
      release: holdSelection(document),
    };
    setDragging(true);
    onDrag(true);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const session = drag.current;
    const el = ref.current;
    if (!session || !el || session.pointerId !== e.pointerId) return;
    const dx = e.clientX - session.x;
    session.moved ||= !isClick(dx, e.clientY - session.y);
    const state = dragState(dx + session.offset);
    el.style.translate = `${state.dx}px`;
    el.style.opacity = String(dragOpacity(state.away));
  };

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    const session = drag.current;
    if (!session || session.pointerId !== e.pointerId) return;
    const dx = e.clientX - session.x;
    const dy = e.clientY - session.y;
    if (!session.moved && isClick(dx, dy)) {
      // A click on the card's body (not its buttons, which never start a drag) dismisses it.
      end(false);
      dismissToast(toast.id);
      return;
    }
    end(dragState(dx, e.timeStamp - session.at).dismiss);
  };

  const onPointerCancel = (e: React.PointerEvent<HTMLDivElement>) => {
    if (drag.current?.pointerId === e.pointerId) end(false);
  };
  const onLostPointerCapture = () => {
    if (drag.current) end(false);
  };

  const error = toast.tone === "error";
  // A promise-style toast (the engine's start) is replaced in place by its success; the spinner
  // it had settles into the green dot instead of swapping between two unrelated marks.
  const spinning = toast.progress !== undefined && !error;
  const wasSpinning = useRef(spinning);
  const [settled, setSettled] = useState(false);
  useEffect(() => {
    const before = wasSpinning.current;
    wasSpinning.current = spinning;
    if (!before || spinning || toast.tone !== "success") return;
    setSettled(true);
    const timer = setTimeout(() => setSettled(false), TOAST_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [spinning, toast.tone]);
  const compact = compactHeight !== undefined;
  const edge = fromTop ? "-top" : "";
  const motion = toast.leaving ? (flung ? "" : `rc-toast-out${edge}`) : `rc-toast-in${edge}`;
  return (
    <div
      ref={ref}
      role={error ? "alert" : "status"}
      data-toast={toast.id}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onLostPointerCapture={onLostPointerCapture}
      style={compact ? { height: compactHeight } : undefined}
      className={cx(
        "rc-toast group pointer-events-auto relative w-[340px] max-w-[calc(100vw-32px)] cursor-pointer touch-none overflow-hidden rounded-2xl text-callout select-none active:cursor-grabbing",
        motion,
      )}
    >
      <div
        data-toast-body
        aria-hidden={compact || undefined}
        className={cx(
          "flex items-center gap-2 py-2 pr-2 pl-4 transition-opacity duration-150 ease-out",
          compact && "opacity-0",
        )}
      >
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <span className="flex size-3 shrink-0 items-center justify-center" data-toast-tone={toast.tone}>
            {spinning ? (
              <Spinner size={12} />
            ) : (
              <span
                className={cx(
                  "size-1.5 rounded-full",
                  settled && "rc-dot-in",
                  error ? "bg-danger" : toast.tone === "success" ? "bg-success" : "bg-fg-muted",
                )}
              />
            )}
          </span>
          <p className="line-clamp-3 min-w-0 flex-1 py-1 text-fg">{toast.text}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {toast.action && <ToastActionButton toast={toast} compact={compact} />}
          <button
            type="button"
            aria-label="Dismiss"
            tabIndex={compact ? -1 : undefined}
            onClick={() => dismissToast(toast.id)}
            className="-ml-1 flex size-6 w-0 items-center justify-center overflow-hidden rounded-full text-fg-faint opacity-0 transition-[width,margin,opacity] duration-150 ease-out group-hover:ml-0 group-hover:w-6 group-hover:opacity-100 hover:bg-fill hover:text-fg focus-visible:ml-0 focus-visible:w-6 focus-visible:opacity-100 active:bg-fill-strong"
          >
            <X size={12} />
          </button>
        </div>
      </div>
      {toast.persist && toast.progress !== undefined && (
        <span className="absolute inset-x-0 bottom-0 h-0.5 bg-fill" aria-hidden>
          <span
            className="block h-full bg-accent transition-[width] duration-300 ease-snappy"
            style={{ width: `${Math.round(Math.min(1, Math.max(0, toast.progress)) * 100)}%` }}
          />
        </span>
      )}
    </div>
  );
}

function ToastActionButton({ toast, compact }: { toast: Toast; compact: boolean }) {
  const action = toast.action;
  if (!action) return null;
  return (
    <button
      type="button"
      tabIndex={compact ? -1 : undefined}
      onClick={() => {
        action.run();
        dismissToast(toast.id);
      }}
      className="flex h-6 items-center rounded-full px-3 text-sub font-medium whitespace-nowrap text-fg ring-1 ring-border-strong ring-inset hover:bg-fill active:bg-fill-strong"
    >
      {action.label}
    </button>
  );
}

/** The stack's place in the window for a position. */
function anchorStyle(position: ToastPosition): CSSProperties {
  switch (position) {
    case "bottom-left":
      return { left: TOAST_INSET, bottom: TOAST_INSET };
    case "top-right":
      return { right: TOAST_INSET, top: TOAST_TOP };
    case "top-center":
      return { left: "50%", top: TOAST_TOP, transform: "translateX(-50%)" };
    default:
      return { right: TOAST_INSET, bottom: TOAST_INSET };
  }
}

export function Toaster() {
  const toasts = useApp((s) => s.toasts);
  const [position] = useToastPosition();
  const fromTop = toastFromTop(position);
  const away = fromTop ? 1 : -1;
  const section = useRef<HTMLElement>(null);
  const [hovering, setHovering] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dragging, setDragging] = useState(false);
  // A window that has never had focus still gets its toasts: only an observed
  // blur (or a hidden document) stops the clock.
  const [active, setActive] = useState(true);
  const paused = hovering || focused || dragging || !active;
  // The stack holds only the front card in flow; the cards behind it are absolute and need the
  // front card's height, and their own, to fan out below it without overlapping.
  const [heights, setHeights] = useState<Record<string, number>>({});
  const measure = useCallback((id: string, height: number) => {
    setHeights((prev) => (prev[id] === height ? prev : { ...prev, [id]: height }));
  }, []);
  const expanded = hovering || focused || dragging;
  let stack = 0;
  const offsets = toasts.map((toast) => {
    const at = stack;
    stack += (heights[toast.id] ?? 0) + TOAST_GAP;
    return at;
  });
  const stackHeight = toasts.length ? stack - TOAST_GAP : 0;

  useEffect(() => {
    const onFocus = () => setActive(true);
    const onBlur = () => setActive(false);
    const onVisibility = () => setActive(document.visibilityState === "visible" && !document.hidden);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  useEffect(() => {
    setToastsPaused(paused);
  }, [paused]);
  // The last card went from under the pointer: no pointerleave has to arrive for the next one to
  // count down (the stack itself takes no pointer events).
  useEffect(() => {
    if (!toasts.length) setHovering(false);
  }, [toasts.length]);
  useEffect(() => () => setToastsPaused(false), []);
  // A focused close button goes with its toast; keep the stack from staying paused for a focus
  // that no longer exists (removing a focused element does not always fire blur).
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-check when a toast goes, which takes the focus with it
  useEffect(() => {
    if (focused && !section.current?.contains(document.activeElement)) setFocused(false);
  }, [toasts, focused]);

  // One timer for the whole queue, set to the soonest expiry; a pause clears
  // every expiry, so this schedules nothing while the stack is held.
  const expiry = nextExpiry(toasts);
  useEffect(() => {
    if (expiry === null) return;
    const timer = setTimeout(expireToasts, Math.max(0, expiry - Date.now()));
    return () => clearTimeout(timer);
  }, [expiry]);

  // Escape dismisses the front toast, after the field's own handler (a draft
  // clears first) and after an open popover or dialog has taken the key.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || aboveToasts()) return;
      const front = useApp.getState().toasts.find((t) => !t.leaving);
      if (!front) return;
      e.preventDefault();
      dismissToast(front.id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <section
      ref={section}
      aria-label="Notifications"
      data-position={position}
      className="pointer-events-none fixed z-50 flex w-[340px] max-w-[calc(100vw-32px)] flex-col items-stretch"
      style={anchorStyle(position)}
      onPointerEnter={() => setHovering(true)}
      onPointerLeave={() => setHovering(false)}
      onFocus={() => setFocused(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setFocused(false);
      }}
    >
      <div className="relative w-full">
        {/* The fan leaves the section's own box; this keeps the pointer inside it on the way
            up to a card behind, so the stack does not fold before it is reached. */}
        {expanded && stackHeight > 0 && (
          <span
            className={cx("pointer-events-auto absolute inset-x-0", fromTop ? "top-0" : "bottom-0")}
            style={{ height: stackHeight }}
            aria-hidden
          />
        )}
        {toasts.map((toast, index) => (
          <ToastSlot
            key={toast.id}
            id={toast.id}
            onHeight={measure}
            className={cx(
              index === 0 ? "relative" : cx("absolute inset-x-0", fromTop ? "top-0" : "bottom-0"),
              fromTop ? "origin-top" : "origin-bottom",
            )}
            style={{
              zIndex: toasts.length - index,
              transform: expanded
                ? `translateY(${away * (offsets[index] ?? 0)}px) scale(1)`
                : `translateY(${away * index * TOAST_PEEK}px) scale(${Math.max(0.9, 1 - index * TOAST_SHRINK)})`,
            }}
          >
            <ToastCard
              toast={toast}
              fromTop={fromTop}
              compactHeight={!expanded && index > 0 ? heights[toasts[0]?.id ?? ""] : undefined}
              onDrag={setDragging}
            />
          </ToastSlot>
        ))}
      </div>
    </section>
  );
}

/** One card's place in the stack: measured, and moved by transform only. */
function ToastSlot({
  id,
  onHeight,
  className,
  style,
  children,
}: {
  id: string;
  onHeight: (id: string, height: number) => void;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const body = el.querySelector<HTMLElement>("[data-toast-body]") ?? el;
    const report = () => onHeight(id, body.offsetHeight);
    report();
    return observeSize(body, report);
  }, [id, onHeight]);
  return (
    <div ref={ref} className={cx("rc-toast-slot", className)} style={style}>
      {children}
    </div>
  );
}
