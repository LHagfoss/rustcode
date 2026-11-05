// The messages a thread has waiting, and when the next one starts. A thread's queue is shown at the
// end of its transcript (ui/QueuedMessages.tsx): the user's own messages as dashed bubbles, a link
// message labelled with the thread that sent it, and at most one quiet line for a wake turn. A turn
// that completed runs the queue on; one that was stopped or failed holds it for Resume. No React and
// no I/O here, so tests drive it.

import { type QueuedPrompt, wakeLine } from "./thread";

/** A queued item that reads as a message at the end of the thread; a wake is one quiet line, not a row. */
export const isQueuedMessage = (q: QueuedPrompt) => !q.wake;

/** The user's own queued messages: the ones an edit loads into the composer. */
export const isOwnQueued = (q: QueuedPrompt) => !q.wake && !q.link;

export interface QueueView {
  /** The queued messages, in the order they will run. */
  messages: QueuedPrompt[];
  /** The summary line a wake waits under. At most one is shown, however many wake the thread. */
  wake?: string;
  /** A stopped or failed turn left the queue here: Resume starts it, and nothing runs until then. */
  held: boolean;
  /** There is something to resume. */
  resumable: boolean;
}

/** The end of a thread's queue, as the transcript draws it. */
export function queueView(thread: {
  queue: readonly QueuedPrompt[];
  running: boolean;
  queueHeld?: boolean;
}): QueueView {
  const wake = thread.queue.find((q) => q.wake);
  return {
    messages: thread.queue.filter(isQueuedMessage),
    wake: wake ? wakeLine(wake.text) : undefined,
    held: !!thread.queueHeld && !thread.running,
    resumable: !!thread.queueHeld && !thread.running && thread.queue.length > 0,
  };
}

/**
 * The item that may start a turn now: the head of the queue, unless a turn runs or the queue is
 * held. This is the one decision the store makes after a turn and on Resume.
 */
export function nextQueued(
  queue: readonly QueuedPrompt[],
  state: { running: boolean; queueHeld?: boolean },
): QueuedPrompt | undefined {
  if (state.running || state.queueHeld) return undefined;
  return queue[0];
}

/** What the live region says when the queue changes: what is waiting, and whether it is held. */
export function queuedAnnouncement(view: QueueView, editing: boolean): string {
  const count = view.messages.length;
  const what = count === 1 ? "1 message" : `${count} messages`;
  const parts: string[] = [];
  if (count) parts.push(`${what} ${view.held ? "held" : "queued"}`);
  if (view.wake) parts.push("a wake-up waiting");
  if (editing) parts.push("Editing a queued message in the composer");
  if (!parts.length) return "The queue is empty";
  return parts.join(" · ");
}
