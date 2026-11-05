// The toast queue, as pure functions over an immutable list (ui/Toaster.tsx
// draws it). Newest first, at most three: a burst of orchestration notices
// stacks a few readable cards instead of hiding everything behind one and a
// "+N". The same message again refreshes its card and timer rather than
// stacking a copy. Info and success leave after 3 s, errors after 7 s, and
// nothing expires while the pointer or focus is on the stack, while the window
// is in the background, or while one is being dragged.

export type ToastTone = "info" | "success" | "error";

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: string;
  text: string;
  tone: ToastTone;
  /** One button at the right (Restart, Undo, Open…). */
  action?: ToastAction;
  /** Stays until dismissed (a slow engine start, a progress bar). */
  persist: boolean;
  /** 0–1 draws a progress bar along the foot. */
  progress?: number;
  /** Fading out; removed once the exit animation has run. */
  leaving?: boolean;
  /** When it was shown or last repeated. */
  at: number;
  /** Visible time left, in ms, kept while paused. */
  remainingMs: number;
  /** When it leaves on its own; null while paused, persistent, or leaving. */
  expiresAt: number | null;
}

export interface ToastInput {
  id: string;
  text: string;
  tone: ToastTone;
  action?: ToastAction;
  persist?: boolean;
  progress?: number;
}

/** The most cards shown at once. */
export const TOAST_LIMIT = 3;
export const TOAST_INFO_MS = 3000;
export const TOAST_ERROR_MS = 7000;
/** Exit animation of a toast (docs/design.md, Motion). */
export const TOAST_EXIT_MS = 120;

export function toastTtl(tone: ToastTone): number {
  return tone === "error" ? TOAST_ERROR_MS : TOAST_INFO_MS;
}

/** The card an input replaces, or undefined for a new one: its id, or the same message. */
function match(toasts: readonly Toast[], input: ToastInput): Toast | undefined {
  return toasts.find(
    (t) => !t.leaving && (t.id === input.id || (t.text === input.text && t.tone === input.tone)),
  );
}

function timed(toast: Toast, now: number, paused: boolean): Toast {
  if (toast.persist) return { ...toast, remainingMs: 0, expiresAt: null };
  const ttl = toastTtl(toast.tone);
  return { ...toast, remainingMs: ttl, expiresAt: paused ? null : now + ttl };
}

/**
 * Adds a toast: the same id (or, without one, the same text and tone) replaces
 * the shown card in place and starts its time again. A new one goes on top;
 * past three, the oldest are dropped.
 */
export function enqueue(
  toasts: readonly Toast[],
  input: ToastInput,
  options: { now: number; paused: boolean },
): Toast[] {
  const { now, paused } = options;
  const existing = match(toasts, input);
  const nextBase: Toast = {
    id: existing?.id ?? input.id,
    text: input.text,
    tone: input.tone,
    action: input.action,
    persist: input.persist ?? false,
    progress: input.progress,
    at: now,
    remainingMs: 0,
    expiresAt: null,
  };
  const next = timed(nextBase, now, paused);
  if (existing) return toasts.map((t) => (t === existing ? next : t));
  // A card still fading with the same id goes now, so the new one keeps its React key.
  const leaving = toasts.filter((t) => t.leaving && t.id !== next.id);
  const live = [next, ...toasts.filter((t) => !t.leaving)].slice(0, TOAST_LIMIT);
  return [...live, ...leaving];
}

/** Marks a toast as leaving; `remove` drops it after the exit animation. */
export function dismiss(toasts: readonly Toast[], id: string): Toast[] {
  if (!toasts.some((t) => t.id === id && !t.leaving)) return toasts as Toast[];
  return toasts.map((t) => (t.id === id && !t.leaving ? { ...t, leaving: true, expiresAt: null } : t));
}

export function remove(toasts: readonly Toast[], id: string): Toast[] {
  const next = toasts.filter((t) => !(t.id === id && t.leaving));
  return next.length === toasts.length ? (toasts as Toast[]) : next;
}

/** Every toast at once, except the ones that stay until dismissed. */
export function clear(toasts: readonly Toast[]): Toast[] {
  const next = toasts.filter((t) => t.persist);
  return next.length === toasts.length ? (toasts as Toast[]) : next;
}

/** Stops every running clock, keeping what is left of each. */
export function pause(toasts: readonly Toast[], now: number): Toast[] {
  let changed = false;
  const next = toasts.map((t) => {
    if (t.expiresAt === null) return t;
    changed = true;
    return { ...t, remainingMs: Math.max(0, t.expiresAt - now), expiresAt: null };
  });
  return changed ? next : (toasts as Toast[]);
}

/** Starts the clocks that pause stopped, from what was left. */
export function resume(toasts: readonly Toast[], now: number): Toast[] {
  let changed = false;
  const next = toasts.map((t) => {
    if (t.expiresAt !== null || t.persist || t.leaving) return t;
    changed = true;
    return { ...t, expiresAt: now + t.remainingMs };
  });
  return changed ? next : (toasts as Toast[]);
}

/** The toasts whose time is up. */
export function due(toasts: readonly Toast[], now: number): string[] {
  return toasts
    .filter((t) => !t.persist && !t.leaving && t.expiresAt !== null && t.expiresAt <= now)
    .map((t) => t.id);
}

/** When the next toast leaves, for one timer; null when nothing is counting. */
export function nextExpiry(toasts: readonly Toast[]): number | null {
  let soonest: number | null = null;
  for (const t of toasts) {
    if (t.persist || t.leaving || t.expiresAt === null) continue;
    if (soonest === null || t.expiresAt < soonest) soonest = t.expiresAt;
  }
  return soonest;
}
