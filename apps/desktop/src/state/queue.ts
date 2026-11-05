// The messages a thread has waiting, and when the next one starts. A thread's queue is shown at the
// end of its transcript (ui/QueuedMessages.tsx): the user's own messages as dashed bubbles, a link
// message labelled with the thread that sent it, and at most one quiet line for a wake turn. A turn
// that completed runs the queue on; one that was stopped or failed holds it for Resume. No React and
// no I/O here, so tests drive it.

import type { ComposeExtras } from "./compose";
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

/** What the steer rule needs to know about the thread a message would be steered into. */
export interface SteerConditions {
  /** The harness takes a message mid-turn (`canSteer`). */
  live: boolean;
  /** The harness takes image blocks (`acceptsImages`). */
  images: boolean;
  /** The built-in harness reads a steer as text: the engine refuses one with pictures. */
  builtin: boolean;
}

/** Why a message cannot be steered: it waits for the next turn. */
export type SteerRefusal = "files" | "commands" | "harness";

export type Steerability = { ok: true } | { ok: false; reason: SteerRefusal };

/** What a dimmed Send now says: the tooltip and the label are one sentence, so they cannot drift. */
export const STEER_REFUSAL: Record<SteerRefusal, string> = {
  files: "Files go with the next turn",
  commands: "Commands go with the next turn",
  harness: "This harness can't take a message mid-turn: it waits for the next one",
};

/**
 * One rule for steering, shared by ⌘⏎, Send now and the queued row's arrow, so the three cannot
 * disagree: plain text steers; pictures steer when the harness takes images mid-turn and is not the
 * built-in one (which reads a steer as text); a command, a link chip or a file waits for the next
 * turn. A file really could ride along on some harnesses, but the choice here is that only text and
 * pictures do, and the tooltip says what happens rather than claiming it is impossible.
 */
export function steerability(extras: ComposeExtras | undefined, conditions: SteerConditions): Steerability {
  const attachments = extras?.attachments ?? [];
  if (attachments.some((a) => a.kind === "file")) return { ok: false, reason: "files" };
  if (extras?.command || (extras?.links.length ?? 0) > 0) return { ok: false, reason: "commands" };
  if (!conditions.live) return { ok: false, reason: "harness" };
  if (attachments.length > 0 && !(conditions.images && !conditions.builtin))
    return { ok: false, reason: "harness" };
  return { ok: true };
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
