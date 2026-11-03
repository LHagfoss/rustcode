// The one toast slot (docs/design.md, Toast): frosted like the composer,
// centred at the top of the chat column under its header (ChatView publishes
// `--rc-toast-center` and `--rc-toast-top`), or the window's top while
// Settings is open. One toast shows at a time, the newest; older ones wait
// behind it as "+N", which clears them all. Info and success leave after 3 s,
// errors after 7 s; the pointer over it pauses the timer. A swipe up or to
// either side, a click on ×, or Escape dismisses.

import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { clearToasts, dismissToast, type Toast, useApp } from "../state/store";
import { cx, Spinner } from "./primitives";

const DURATION_MS = 3000;
const ERROR_MS = 7000;
/** How far a swipe must travel to dismiss. */
const SWIPE_PX = 48;

function ToastCard({ toast, paused, waiting }: { toast: Toast; paused: boolean; waiting: number }) {
  const ref = useRef<HTMLDivElement>(null);
  const duration = toast.tone === "error" ? ERROR_MS : DURATION_MS;
  const remaining = useRef(duration);
  const swipe = useRef<{ x: number; y: number; id: number; dx: number; dy: number } | null>(null);

  // A repeat (same text, or a replace by id) starts the full time again.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset when the toast is shown again
  useEffect(() => {
    remaining.current = duration;
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
    if (e.button !== 0 || (e.target as HTMLElement).closest("button")) return;
    swipe.current = { x: e.clientX, y: e.clientY, id: e.pointerId, dx: 0, dy: 0 };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const s = swipe.current;
    const el = ref.current;
    if (!s || !el || s.id !== e.pointerId) return;
    // Follows the pointer sideways and up; down it resists.
    s.dx = e.clientX - s.x;
    const dy = e.clientY - s.y;
    s.dy = dy < 0 ? dy : dy / 6;
    const away = Math.max(Math.abs(s.dx), -s.dy);
    el.style.transition = "none";
    el.style.transform = `translate(${s.dx}px, ${s.dy}px)`;
    el.style.opacity = String(Math.max(0.2, 1 - away / 200));
  };
  const onPointerUp = () => {
    const s = swipe.current;
    const el = ref.current;
    swipe.current = null;
    if (!s || !el) return;
    el.style.transition = "transform 160ms var(--ease-snappy), opacity 160ms ease-out";
    if (Math.abs(s.dx) > SWIPE_PX || s.dy < -SWIPE_PX / 2) {
      el.style.transform =
        Math.abs(s.dx) > -s.dy ? `translateX(${Math.sign(s.dx) * 120}%)` : "translateY(-140%)";
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
        "rc-toast pointer-events-auto relative flex w-[380px] max-w-[calc(100vw-32px)] cursor-grab touch-none items-start gap-2.5 overflow-hidden rounded-2xl py-1.5 pr-1.5 pl-3.5 text-callout active:cursor-grabbing",
        toast.leaving ? "rc-toast-out" : "rc-toast-in",
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
      {waiting > 0 && (
        <button
          type="button"
          aria-label={`Clear all ${waiting + 1} notifications`}
          onClick={clearToasts}
          className="h-6 shrink-0 rounded-full px-2 font-medium text-caption text-fg-muted tabular hover:bg-fill hover:text-fg"
        >
          +{waiting}
        </button>
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
  const [paused, setPaused] = useState(false);
  const live = toasts.filter((t) => !t.leaving);
  // The newest shows; one leaving keeps its place until it has faded.
  const front = toasts.at(-1);
  // Nothing under the pointer any more: the timer must not stay paused.
  useEffect(() => {
    if (!front) setPaused(false);
  }, [front]);
  // Escape dismisses the toast in front, unless something else (a dialog, a field) takes it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || !front || front.leaving) return;
      if ((e.target as HTMLElement | null)?.closest("input, textarea, [role=dialog]")) return;
      dismissToast(front.id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [front]);
  return (
    <section
      aria-label="Notifications"
      aria-live="polite"
      className="pointer-events-none fixed z-50 flex -translate-x-1/2 flex-col items-center"
      style={
        settings
          ? { left: "50%", top: 16 }
          : { left: "var(--rc-toast-center, 50%)", top: "var(--rc-toast-top, 56px)" }
      }
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
    >
      {front && (
        <ToastCard key={front.id} toast={front} paused={paused} waiting={Math.max(0, live.length - 1)} />
      )}
    </section>
  );
}
