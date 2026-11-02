// The one toast stack (docs/design.md, Toast): frosted like the composer,
// stacked at the bottom right of the chat column just above the composer
// stack (ChatView publishes where that is as `--rc-toast-right` and
// `--rc-toast-bottom`), or the window's corner while Settings is open. The
// newest sits lowest; at most three show and the rest wait. Info and success
// leave after 4 s, errors stay until closed; the pointer over the stack
// pauses every timer. A swipe to the right or the close button dismisses.

import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { dismissToast, type Toast, useApp } from "../state/store";
import { cx, Spinner } from "./primitives";

const VISIBLE = 3;
const DURATION_MS = 4000;
/** How far a swipe must travel to dismiss. */
const SWIPE_PX = 56;

function ToastCard({ toast, paused }: { toast: Toast; paused: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const remaining = useRef(DURATION_MS);
  const swipe = useRef<{ x: number; id: number; dx: number } | null>(null);

  // A repeat (same text, or a replace by id) starts the full time again.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset when the toast is shown again
  useEffect(() => {
    remaining.current = DURATION_MS;
  }, [toast.at]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: toast.at restarts the timer
  useEffect(() => {
    if (toast.persist || toast.leaving || paused) return;
    const started = Date.now();
    const timer = setTimeout(() => dismissToast(toast.id), remaining.current);
    return () => {
      clearTimeout(timer);
      remaining.current = Math.max(800, remaining.current - (Date.now() - started));
    };
  }, [toast.persist, toast.leaving, paused, toast.at, toast.id]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || (e.target as HTMLElement).closest("button, .selectable")) return;
    swipe.current = { x: e.clientX, id: e.pointerId, dx: 0 };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const s = swipe.current;
    const el = ref.current;
    if (!s || !el || s.id !== e.pointerId) return;
    // Follows the pointer to the right only; to the left it resists.
    const dx = e.clientX - s.x;
    s.dx = dx > 0 ? dx : dx / 6;
    el.style.transition = "none";
    el.style.transform = `translateX(${s.dx}px)`;
    el.style.opacity = String(Math.max(0.2, 1 - Math.max(0, s.dx) / 240));
  };
  const onPointerUp = () => {
    const s = swipe.current;
    const el = ref.current;
    swipe.current = null;
    if (!s || !el) return;
    el.style.transition = "transform 160ms var(--ease-snappy), opacity 160ms ease-out";
    if (s.dx > SWIPE_PX) {
      el.style.transform = "translateX(120%)";
      el.style.opacity = "0";
      dismissToast(toast.id);
    } else {
      el.style.transform = "";
      el.style.opacity = "";
    }
  };

  const error = toast.tone === "error";
  const Icon = error ? CircleAlert : toast.tone === "success" ? CircleCheck : Info;
  return (
    <div
      ref={ref}
      role={error ? "alert" : "status"}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      className={cx(
        "rc-toast pointer-events-auto relative flex w-[340px] max-w-[calc(100vw-32px)] touch-pan-y items-start gap-2.5 overflow-hidden rounded-lg py-2 pr-1.5 pl-3 text-callout",
        toast.leaving ? "rc-toast-out" : "rc-in",
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
      <p className="selectable line-clamp-4 min-w-0 flex-1 py-1 text-fg">{toast.text}</p>
      {toast.action && (
        <button
          type="button"
          onClick={() => {
            toast.action?.run();
            dismissToast(toast.id);
          }}
          className="h-6 shrink-0 rounded-full bg-fill px-2.5 font-medium text-fg hover:bg-fill-strong active:bg-fill-strong"
        >
          {toast.action.label}
        </button>
      )}
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => dismissToast(toast.id)}
        className="flex size-6 shrink-0 items-center justify-center rounded-md text-fg-faint hover:bg-fill hover:text-fg active:bg-fill-strong"
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
  const [paused, setPaused] = useState(false);
  const shown = toasts.slice(-VISIBLE);
  // Nothing under the pointer any more: the timers must not stay paused.
  useEffect(() => {
    if (!shown.length) setPaused(false);
  }, [shown.length]);
  return (
    <section
      aria-label="Notifications"
      aria-live="polite"
      className="pointer-events-none fixed z-50 flex flex-col items-end gap-2"
      style={
        settings
          ? { right: 16, bottom: 16 }
          : { right: "var(--rc-toast-right, 16px)", bottom: "var(--rc-toast-bottom, 16px)" }
      }
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
    >
      {shown.map((t) => (
        <ToastCard key={t.id} toast={t} paused={paused} />
      ))}
    </section>
  );
}
