// Up to three toasts, newest on top (state/toasts.ts keeps the queue), frosted
// like the composer and centred at the top of the chat column under its header
// (ChatView publishes `--rc-toast-center` and `--rc-toast-top`), or the window's
// top while Settings is open. Info and success leave after 3 s, errors after
// 7 s. The pointer or focus on the stack, a drag, or the window in the
// background stops the clock. A click on ×, Escape, or a drag past 48px (up or
// sideways) dismisses; a drag never selects text anywhere.

import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { dragExit, dragOpacity, dragState, holdSelection, startsDrag, TOAST_DRAG_MS } from "../lib/toastDrag";
import { dismissToast, expireToasts, setToastsPaused, type Toast, useApp } from "../state/store";
import { nextExpiry } from "../state/toasts";
import { cx, Pill, Spinner } from "./primitives";

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
  dx: number;
  dy: number;
  /** Puts the document back; safe to call twice. */
  release: () => void;
}

function ToastCard({ toast, onDrag }: { toast: Toast; onDrag: (dragging: boolean) => void }) {
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
        const { dx, direction } = dragState(session.dx, session.dy);
        el.style.transition = `transform ${TOAST_DRAG_MS}ms var(--ease-snappy), opacity ${TOAST_DRAG_MS}ms ease-out`;
        el.style.transform = dragExit(direction ?? "up", dx);
        el.style.opacity = "0";
        setFlung(true);
        dismissToast(toast.id);
        return;
      }
      if (el) {
        el.style.transition = `transform ${TOAST_DRAG_MS}ms var(--ease-snappy), opacity ${TOAST_DRAG_MS}ms ease-out`;
        el.style.transform = "";
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
    if (e.button !== 0 || !startsDrag(e.target as HTMLElement)) return;
    // No native drag and no text selection, even where the platform would
    // start one despite the document's `user-select: none`.
    e.preventDefault();
    window.getSelection()?.removeAllRanges();
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    drag.current = {
      pointerId: e.pointerId,
      x: e.clientX,
      y: e.clientY,
      dx: 0,
      dy: 0,
      release: holdSelection(document),
    };
    setDragging(true);
    onDrag(true);
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const session = drag.current;
    const el = ref.current;
    if (!session || !el || session.pointerId !== e.pointerId) return;
    session.dx = e.clientX - session.x;
    session.dy = e.clientY - session.y;
    const state = dragState(session.dx, session.dy);
    el.style.transition = "none";
    el.style.transform = `translate(${state.dx}px, ${state.dy}px)`;
    el.style.opacity = String(dragOpacity(state.away));
  };

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    const session = drag.current;
    if (!session || session.pointerId !== e.pointerId) return;
    session.dx = e.clientX - session.x;
    session.dy = e.clientY - session.y;
    end(dragState(session.dx, session.dy).dismiss);
  };

  const onPointerCancel = () => end(false);
  const onLostPointerCapture = () => {
    if (drag.current) end(false);
  };

  const error = toast.tone === "error";
  const Icon = error ? CircleAlert : toast.tone === "success" ? CircleCheck : Info;
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
      className={cx(
        "rc-toast pointer-events-auto relative flex w-[380px] max-w-[calc(100vw-32px)] cursor-grab touch-none items-start gap-2.5 overflow-hidden rounded-xl py-1.5 pr-1.5 pl-3.5 text-callout select-none active:cursor-grabbing",
        toast.leaving ? (flung ? "" : "rc-toast-out") : "rc-toast-in",
      )}
    >
      <span className="flex h-6 shrink-0 items-center">
        {toast.progress !== undefined && !error ? (
          <Spinner size={13} />
        ) : (
          <Icon
            size={14}
            className={error ? "text-danger" : toast.tone === "success" ? "text-success" : "text-fg-muted"}
          />
        )}
      </span>
      <p className="line-clamp-3 min-w-0 flex-1 py-1 text-fg">{toast.text}</p>
      {toast.action && (
        <Pill
          size="sm"
          onClick={() => {
            toast.action?.run();
            dismissToast(toast.id);
          }}
        >
          {toast.action.label}
        </Pill>
      )}
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => dismissToast(toast.id)}
        className="flex size-6 shrink-0 items-center justify-center rounded-full text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
      >
        <X size={12} />
      </button>
      {toast.progress !== undefined && (
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

export function Toaster() {
  const toasts = useApp((s) => s.toasts);
  const settings = useApp((s) => s.settings !== null);
  const section = useRef<HTMLElement>(null);
  const [hovering, setHovering] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dragging, setDragging] = useState(false);
  // A window that has never had focus still gets its toasts: only an observed
  // blur (or a hidden document) stops the clock.
  const [active, setActive] = useState(true);
  const paused = hovering || focused || dragging || !active;

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
      className="pointer-events-none fixed z-50 flex -translate-x-1/2 flex-col items-center gap-2"
      style={
        settings
          ? { left: "50%", top: 16 }
          : { left: "var(--rc-toast-center, 50%)", top: "var(--rc-toast-top, 56px)" }
      }
      onPointerEnter={() => setHovering(true)}
      onPointerLeave={() => setHovering(false)}
      onFocus={() => setFocused(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setFocused(false);
      }}
    >
      {toasts.map((toast) => (
        <ToastCard key={toast.id} toast={toast} onDrag={setDragging} />
      ))}
    </section>
  );
}
