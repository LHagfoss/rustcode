// System notifications and the Dock badge: a thread that needs the user (an
// approval, a question, a usage limit, a failed turn) or that finished while
// they looked elsewhere gets one notification, and the Dock shows how many
// threads want them. A thread asleep after a snooze is neither: it says its one
// thing when its snooze ends (state/store.ts `wakeSnoozed`). The shell
// (`src-tauri/src/notify.rs`) shows them, so the permission the OS asks for
// belongs to the app, not to the page.

import { isTauri } from "../acp/transport";
import { shellCall } from "../lib/shell";

/** What a thread has to tell the user about. */
export type NotifyKind = "approval" | "question" | "limit" | "failed" | "finished";

/** One thing worth a notification, from one thread. */
export interface NotifyEvent {
  threadId: string;
  kind: NotifyKind;
  /**
   * The turn or the request it belongs to. The same event seen twice is one
   * notification: a turn's end keeps its time, a request its id.
   */
  key: string;
  /** The thread's title, the notification's title. */
  title: string;
  /** A question's own words, cut to the notification's one line. */
  detail?: string;
}

/** What the app knows when an event arrives: whether telling the user is wanted now. */
export interface NotifyContext {
  /** Settings › General › System notifications. */
  enabled: boolean;
  /** Settings › General › Only when RustCode is in the background. */
  backgroundOnly: boolean;
  /** The window is in front. */
  windowFocused: boolean;
  /** The thread on screen. */
  onScreen: string | null;
  /**
   * Threads asleep until a time (state/store.ts `snoozed`): the user put them away, so nothing they
   * do is news before their snooze ends. Absent or empty when nothing sleeps.
   */
  snoozed?: Readonly<Record<string, number>>;
}

/** The line under the title: short, and never the whole question. */
export const NOTIFY_BODY: Record<NotifyKind, string> = {
  approval: "Needs your approval",
  question: "Asked you a question",
  limit: "Hit a usage limit",
  failed: "Its turn failed",
  finished: "Finished",
};

/** A thread's title is the notification's title; the app's name is already beside it. */
export function bodyFor(event: NotifyEvent): string {
  if (event.kind !== "question" || !event.detail) return NOTIFY_BODY[event.kind];
  const line = firstLine(event.detail);
  return line || NOTIFY_BODY.question;
}

/** The first line of a text, at most a notification's width. */
export function firstLine(text: string, max = 120): string {
  const line = text
    .split("\n")
    .map((part) => part.trim())
    .find((part) => part.length > 0);
  if (!line) return "";
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

/**
 * The events already sent, so nothing is said twice. Forgets a thread's keys
 * when its state goes away (`retain`), which keeps it bounded by the threads on
 * screen rather than by everything the app has ever shown.
 */
export function createNotifyGate() {
  const sent = new Map<string, Set<string>>();
  return {
    /** The event to show now, or null when it is a repeat or the moment is wrong. */
    take(event: NotifyEvent, context: NotifyContext): NotifyEvent | null {
      if (!context.enabled) return null;
      // Asleep: the user asked not to hear from it. Nothing is spent here, so the one thing it has
      // to say is said when its snooze ends (state/store.ts `wakeSnoozed`).
      if (context.snoozed && event.threadId in context.snoozed) return null;
      // The window in front, on that thread: the screen already says it.
      if (context.windowFocused && context.onScreen === event.threadId) return null;
      // ...and with this setting, the window in front at all: a toast is enough there.
      if (context.windowFocused && context.backgroundOnly) return null;
      const key = `${event.kind}:${event.key}`;
      const taken = sent.get(event.threadId);
      if (taken?.has(key)) return null;
      if (taken) taken.add(key);
      else sent.set(event.threadId, new Set([key]));
      return event;
    },
    /** Forgets what threads that are no longer known have said. */
    retain(threadIds: ReadonlySet<string>): void {
      for (const id of sent.keys()) if (!threadIds.has(id)) sent.delete(id);
    },
    clear(): void {
      sent.clear();
    },
  };
}

/** The events a thread's store slice can be read for, without the store: it keeps the gate honest. */
export interface NoticeState {
  systemNotifications: boolean;
  notificationsBackgroundOnly: boolean;
  selectedId: string | null;
  /** Threads asleep until a time; a caller reading only part of the state may leave it out. */
  snoozed?: Readonly<Record<string, number>>;
}

/** The context one event is judged in, from the store's slice and the window. */
export function contextOf(state: NoticeState, focused = windowFocused()): NotifyContext {
  return {
    enabled: state.systemNotifications,
    backgroundOnly: state.notificationsBackgroundOnly,
    windowFocused: focused,
    onScreen: state.selectedId,
    snoozed: state.snoozed,
  };
}

/**
 * The one thing a thread that slept has to say now its snooze is over, or null when it has
 * nothing: what it waits on the user for first, else a turn that ended unseen. Any event is at
 * most one, so a thread coming back cannot stack a request and a finish into two notifications.
 *
 * A thread that slept through nothing that concerns the user simply returns, which is the common
 * case: its turn went on or ended fine, and the row says so in the list.
 */
export function wakeEvent(read: {
  threadId: string;
  title: string;
  /** The oldest request it waits on, if one does. */
  request?: { rpcId: number | string; question?: { text: string } };
  /** When a turn of its own finished unseen (`finished`), if one did. */
  finishedAt?: number;
  /** When a turn of its own failed unseen (`failed`), if one did. */
  failedAt?: number;
}): NotifyEvent | null {
  if (read.request) {
    const question = read.request.question;
    return {
      threadId: read.threadId,
      kind: question ? "question" : "approval",
      key: String(read.request.rpcId),
      title: read.title,
      detail: question?.text,
    };
  }
  if (read.failedAt !== undefined) {
    return { threadId: read.threadId, kind: "failed", key: String(read.failedAt), title: read.title };
  }
  if (read.finishedAt !== undefined) {
    return { threadId: read.threadId, kind: "finished", key: String(read.finishedAt), title: read.title };
  }
  return null;
}

/** Whether the app's window has the keyboard: a notification would only repeat the screen. */
export function windowFocused(): boolean {
  if (typeof document === "undefined") return false;
  return document.hasFocus() && document.visibilityState !== "hidden";
}

const gate = createNotifyGate();

/** The app's first moments replay what was already true: nothing there is news. */
export const WARMUP_MS = 3_000;

const startedAt = Date.now();

/** Whether the app has just started, and is still receiving what its threads already told it. */
export function warmingUp(now = Date.now(), since = startedAt): boolean {
  return now - since < WARMUP_MS;
}

/**
 * Tells the user about one thread, if this is a new event and the moment calls
 * for it. Called where a thread starts waiting or a turn ends in the store.
 */
export function notifyThread(event: NotifyEvent, state: NoticeState): void {
  if (warmingUp()) return;
  const allowed = gate.take(event, contextOf(state));
  if (allowed) void show(allowed);
}

/** Drops what threads the app no longer lists have already had said about them. */
export function forgetThreads(threadIds: ReadonlySet<string>): void {
  gate.retain(threadIds);
}

/** Settings › Notifications asks whether the system takes these; asked once, on the first one. */
let permission: Promise<boolean> | null = null;

function permitted(): Promise<boolean> {
  permission ??= shellCall<boolean>("notify_permission").catch(() => false);
  return permission;
}

async function show(event: NotifyEvent): Promise<void> {
  if (!isTauri() || !(await permitted())) return;
  try {
    await shellCall("notify_show", {
      title: event.title,
      body: bodyFor(event),
      thread: event.threadId,
    });
  } catch {
    // The system refused it: the row and the badge still say the thread needs the user.
  }
}

/** What to show on the Dock: threads that need the user, and unread finished ones. */
export interface BadgeState {
  threads: Record<string, { permissions: readonly unknown[] }>;
  finished: Record<string, number>;
  failed: Record<string, number>;
  /** Threads asleep until a time: they are not counted until their snooze ends. */
  snoozed?: Readonly<Record<string, number>>;
}

/** The threads the Dock counts, once each: what asks you and what ended unseen, never a sleeping one. */
export function badgeCount(state: BadgeState): number {
  const asleep = state.snoozed ?? {};
  const ids = new Set<string>(
    [...Object.keys(state.failed), ...Object.keys(state.finished)].filter((id) => !(id in asleep)),
  );
  for (const [id, thread] of Object.entries(state.threads)) {
    if (thread.permissions.length > 0 && !(id in asleep)) ids.add(id);
  }
  return ids.size;
}

/** The store as the notifier reads it: the badge follows every change of attention. */
export interface NoticeStore<State extends NoticeState & BadgeState> {
  get: () => State;
  subscribe: (listener: (state: State) => void) => () => void;
  /** Opens a thread, for a click on one of its notifications. */
  open: (threadId: string) => void;
}

let badge = -1;

async function setBadge(count: number): Promise<void> {
  if (!isTauri()) return;
  try {
    await shellCall("notify_badge", { count });
  } catch {
    // Linux without a launcher to badge: the in-app rows still say it.
  }
}

/** The event the shell emits when one of those notifications is clicked. */
export const OPEN_EVENT = "notify:open";

/**
 * Starts the small parts of this that follow the app rather than a call site:
 * the Dock badge, and a click on a notification opening its thread. Called once
 * when the store exists (state/store.ts).
 */
export function startNotifications<State extends NoticeState & BadgeState>(store: NoticeStore<State>): void {
  const sync = (state: NoticeState & BadgeState) => {
    forgetThreads(
      new Set([...Object.keys(state.threads), ...Object.keys(state.finished), ...Object.keys(state.failed)]),
    );
    const count = badgeCount(state);
    if (count === badge) return;
    badge = count;
    void setBadge(count);
  };
  sync(store.get());
  store.subscribe(sync);
  if (typeof window !== "undefined") {
    // Coming back to the window is not a store change, and it settles what it shows.
    window.addEventListener("focus", () => sync(store.get()));
    document.addEventListener("visibilitychange", () => sync(store.get()));
  }
  void listenForOpen(store.open);
}

async function listenForOpen(open: (threadId: string) => void): Promise<void> {
  if (!isTauri()) return;
  try {
    const { listen } = await import("@tauri-apps/api/event");
    await listen<string>(OPEN_EVENT, (event) => {
      if (typeof event.payload === "string" && event.payload) open(event.payload);
    });
  } catch {
    // Without the event bridge the notification still brings the window forward.
  }
}
