// Sending later: messages scheduled for a time, kept across restarts, and the timer that sends them
// when due. Split out of state/store.ts; the state these write still lives in the app store.

import { type ComposeExtras, carriesExtras, emptyExtras, hasExtras } from "./compose";
import {
  addScheduled,
  dueScheduled,
  parseScheduleBook,
  persistableSchedule,
  SCHEDULED_KEY,
  type ScheduleBook,
  type ScheduledMessage,
  type ScheduleRefusal,
  scheduledCount,
  withoutScheduled,
} from "./schedule";
import {
  type AppState,
  connectionForThread,
  openThread,
  resumeQueue,
  runPrompt,
  store,
  stored,
  threadIsGone,
  toast,
  updateThread,
  useApp,
  wakeOnWrite,
} from "./store";
import { isRunning, localId } from "./thread";
import { unsettleThreads } from "./threadStatus";

// Read through `useApp` at call time: store.ts imports this module, so `useApp` is not defined yet
// while this one loads.
const get = () => useApp.getState();
const set = (patch: Partial<AppState> | ((s: AppState) => Partial<AppState>)) => useApp.setState(patch);

/**
 * A message written now and sent at a time the user picked (state/schedule.ts): the card over the
 * composer counts down, Send now sends it at once, Delete drops it, and a restart keeps it because
 * every change is written to app storage. When its time comes it goes out on the thread's normal send
 * path, so its command, links, pictures, model, effort and permissions are the ones the composer
 * had; a thread that is busy then queues it like any follow-up.
 */
function writeScheduled(next: ScheduleBook) {
  set({ scheduled: next });
  store(SCHEDULED_KEY, JSON.stringify(persistableSchedule(next)));
  syncScheduleTimer();
}

/** The book as storage has it: read at start-up, and between tests as a window that just started. */
export function restoreScheduled(): void {
  set({ scheduled: parseScheduleBook(stored(SCHEDULED_KEY)) });
  syncScheduleTimer();
}

/**
 * Schedule the composer's message for `due`: the message, or why it could not be (the composer
 * keeps its text and attachments then, and says so). Not connected is `offline`.
 */
export function scheduleMessage(
  threadId: string,
  text: string,
  extras: ComposeExtras,
  due: number,
): ScheduledMessage | { refused: ScheduleRefusal | "offline" } {
  if (get().status !== "ready") return { refused: "offline" };
  if (!text.trim() && !hasExtras(extras)) return { refused: "empty" };
  if (!(due > Date.now())) return { refused: "time" };
  const item: ScheduledMessage = {
    id: localId("scheduled"),
    text,
    due,
    ...(carriesExtras(extras) ? { extras } : {}),
  };
  const next = addScheduled(get().scheduled, threadId, item);
  if ("refused" in next) return next;
  writeScheduled(next.book);
  return item;
}

/** Delete: the message never goes out. */
export function removeScheduled(threadId: string, id: string): void {
  const next = withoutScheduled(get().scheduled, threadId, id);
  if (next !== get().scheduled) writeScheduled(next);
}

/** Send now: take it out of the book and send it on the thread's normal path. */
export async function sendScheduledNow(threadId: string, id: string): Promise<boolean> {
  const item = get().scheduled[threadId]?.find((row) => row.id === id);
  if (!item) return false;
  if (get().status !== "ready") {
    toast("Not connected: the message stays scheduled.", "error");
    return false;
  }
  const outcome = await deliverScheduled(threadId, item);
  if (outcome === "wait") {
    toast("Not connected: the message stays scheduled.", "error");
    return false;
  }
  removeScheduled(threadId, id);
  return outcome === "sent";
}

/**
 * Open the thread if it is not, then send: a busy turn takes it as its next message (runPrompt).
 * `wait` keeps the message for another try — its computer is away, or the engine is not connected
 * yet — while a thread that no longer exists is `gone` and one the user deleted meanwhile is too.
 */
async function deliverScheduled(threadId: string, item: ScheduledMessage): Promise<"sent" | "gone" | "wait"> {
  if (threadIsGone(threadId)) {
    toast("That thread is gone, so its scheduled message was dropped.", "error");
    return "gone";
  }
  // A thread on a paired computer that is away, or an engine that is not up: the message waits for
  // it rather than being lost, and the timer tries again.
  if (!connectionForThread(threadId)) return "wait";
  if (!get().threads[threadId]?.loaded) {
    await openThread(threadId);
    for (let i = 0; i < 100 && !get().threads[threadId]?.loaded; i++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  if (!get().threads[threadId]?.loaded) return "wait";
  // Deleted while the thread was opening: nothing goes out.
  if (!get().scheduled[threadId]?.some((row) => row.id === item.id)) return "gone";
  const extras = item.extras;
  const thread = get().threads[threadId]!;
  // A turn runs, or the queue is held by a stopped turn, or messages wait in it: this goes behind
  // them and Resume starts the head, exactly as a message typed meanwhile does (`submit`).
  if (isRunning(thread) || thread.queue.length || thread.queueHeld) {
    unsettleThreads([threadId], { quiet: true });
    wakeOnWrite(threadId);
    updateThread(threadId, (t) => ({
      ...t,
      queue: [
        ...t.queue,
        {
          id: localId("queued"),
          text: item.text,
          ...(extras && carriesExtras(extras) ? { extras } : {}),
        },
      ],
    }));
    // Held for Resume: nothing starts until the user presses it. A queue with no turn starts its head.
    if (!isRunning(thread) && !thread.queueHeld) void resumeQueue(threadId);
    return "sent";
  }
  void runPrompt(threadId, item.text, extras ?? emptyExtras);
  return "sent";
}

let scheduleTimer: ReturnType<typeof setInterval> | null = null;
let sendingScheduled = false;

/** Stops the timer and forgets a send in flight, for the store's reset between tests. */
export function resetScheduledSends(): void {
  if (scheduleTimer) clearInterval(scheduleTimer);
  scheduleTimer = null;
  sendingScheduled = false;
}

/**
 * One slow timer while something is scheduled: a card's countdown ticks in the UI, and this only has
 * to notice that a time has come, even after the Mac slept past it.
 */
function syncScheduleTimer() {
  const waiting = scheduledCount(get().scheduled) > 0;
  if (waiting && !scheduleTimer) {
    const timer = setInterval(() => void continueDueScheduled(), 1000) as unknown as ReturnType<
      typeof setInterval
    > & { unref?: () => void };
    // A timer does not keep the process alive (a test, or a window with nothing left to do).
    timer.unref?.();
    scheduleTimer = timer;
  } else if (!waiting && scheduleTimer) {
    clearInterval(scheduleTimer);
    scheduleTimer = null;
  }
}

/** Every message whose time has come goes out, the soonest first. */
export async function continueDueScheduled(now = Date.now()): Promise<void> {
  if (sendingScheduled || get().status !== "ready") return;
  const due = dueScheduled(get().scheduled, now);
  if (!due.length) return;
  sendingScheduled = true;
  try {
    for (const { threadId, item } of due) {
      const outcome = await deliverScheduled(threadId, item);
      // One that waits for its computer stays; anything else has had its one turn out of the book.
      if (outcome !== "wait") removeScheduled(threadId, item.id);
    }
  } finally {
    sendingScheduled = false;
  }
}
