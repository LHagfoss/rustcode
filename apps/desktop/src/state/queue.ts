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

/** How a harness takes a message sent mid-turn, as the engine reports it (`HarnessSteering`). */
export interface SteeringCapabilities {
  live: boolean;
  attachments: boolean;
  /** A live steer goes in while an approval or a question waits; the built-in turn does not. */
  while_awaiting_user: boolean;
  interrupt_restart: boolean;
  queue: boolean;
}

/** What the running turn is doing, as the one send rule needs it. */
export interface TurnState {
  /** A turn runs now: nothing steers into an idle thread. */
  running: boolean;
  /** An approval card or a question waits on the user: the turn reads no steer until it is answered. */
  awaitingUser: boolean;
}

/** What a message sent now does: it steers, it waits, or it stops the turn and starts the next. */
export type SendAction = "steer" | "queue" | "interruptRestart";

/** Why a message cannot be steered: it goes with the next turn. */
export type QueueReason =
  | "idle"
  | "files"
  | "commands"
  | "links"
  | "attachments"
  | "awaiting"
  | "refused"
  | "harness";

export type SendDecision =
  | { action: "steer" }
  | { action: "queue"; reason: QueueReason }
  | { action: "interruptRestart" };

/** What a dimmed Send now says: the tooltip and the label are one sentence, so they cannot drift. */
export const QUEUE_REASON: Record<QueueReason, string> = {
  idle: "No turn is running: this starts one",
  files: "Files go with the next turn",
  commands: "Commands go with the next turn",
  links: "Links go with the next turn",
  attachments: "This harness takes text mid-turn only: the pictures go with the next turn",
  awaiting: "The turn is waiting on you: this goes with the next one",
  refused: "The turn can't take it now: it goes with the next one",
  harness: "This harness can't take a message mid-turn: it waits for the next one",
};

/**
 * The one rule for a message sent while a turn runs, shared by ⌘⏎, Send now and the queued row's
 * arrow, so the three cannot disagree, and read by the composer's hint so it says what will happen
 * rather than what might. It goes in: a message that carries a command, a link chip or a file never
 * does (those are turn-shaped), and neither does one while an approval or a question waits on the
 * user. It steers when the harness takes it mid-turn, with its pictures when the harness takes
 * those too. Otherwise it waits for the next turn when the harness queues, and only a harness that
 * cannot queue takes the turn down instead (`interruptRestart`).
 */
export function sendDecision(
  extras: ComposeExtras | undefined,
  steering: SteeringCapabilities,
  turn: TurnState,
): SendDecision {
  if (!turn.running) return { action: "queue", reason: "idle" };
  const attachments = extras?.attachments ?? [];
  if (attachments.some((a) => a.kind === "file")) return { action: "queue", reason: "files" };
  if (extras?.command) return { action: "queue", reason: "commands" };
  if ((extras?.links.length ?? 0) > 0) return { action: "queue", reason: "links" };
  // An approval or a question is the user's to answer first, where the turn reads a steer only
  // between steps; a Codex or an ACP turn takes one anyway.
  if (turn.awaitingUser && !steering.while_awaiting_user) return { action: "queue", reason: "awaiting" };
  if (steering.live && (attachments.length === 0 || steering.attachments)) return { action: "steer" };
  if (steering.queue) return { action: "queue", reason: steering.live ? "attachments" : "harness" };
  if (steering.interrupt_restart) return { action: "interruptRestart" };
  return { action: "queue", reason: "harness" };
}

/** What the live region says when the queue changes: what is waiting, and whether it is held. */
export function queuedAnnouncement(view: QueueView, editing: boolean): string {
  const count = view.messages.length;
  const what = count === 1 ? "1 message" : `${count} messages`;
  const parts: string[] = [];
  if (count) parts.push(`${what} ${view.held ? "held" : "queued"}`);
  if (view.wake) parts.push(`a wake-up waiting: ${view.wake}`);
  if (editing) parts.push("Editing a queued message in the composer");
  if (!parts.length) return "The queue is empty";
  return parts.join(" · ");
}
