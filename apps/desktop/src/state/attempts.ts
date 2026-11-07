// Rewind as an attempt (docs/design.md, "Rewind an attempt"): a rewind puts the files back to a
// checkpoint and keeps the turns it abandoned as an earlier attempt, drawn faded under a divider
// below the live transcript. Pure, so the store and the transcript share it and the tests drive it
// without a store.
//
// The turns are the app's own record: `_rustcode/rewind` takes them out of the thread's history,
// so the engine no longer has them and a reload of the thread reads only what is left. The state
// they ended in is kept by the engine (a checkpoint of the attempt), so the files can go back to
// it even though the messages cannot.

import type { Checkpoint } from "../acp/v2";
import type { TranscriptItem } from "./thread";

export interface Attempt {
  /** Stable id for the divider and its rows. */
  id: string;
  /** When the rewind ran. */
  at: number;
  /** The checkpoint it restored: the files (and the conversation) go back to before this turn. */
  turn: number;
  /** The checkpoint kept of the state it abandoned; absent when the engine could not keep it. */
  head?: number;
  /** The turns it abandoned, in the order they ran. */
  items: TranscriptItem[];
}

/**
 * Where a rewind at the turn ending `endId` splits the transcript: what stays live, and the turns
 * it abandons (from the message that started that turn). `undefined` when the turn is not in the
 * transcript, so nothing can be said about what the rewind takes.
 */
export function splitAtTurn(
  items: readonly TranscriptItem[],
  endId: string,
): { keep: TranscriptItem[]; tail: TranscriptItem[] } | undefined {
  const end = items.findIndex((item) => item.kind === "turnEnd" && item.id === endId);
  if (end < 0) return undefined;
  // The turn starts at its own message: the first one sent after the turn above it ended. A steer
  // or a note inside the turn is later on, so the walk keeps going past those.
  let start = -1;
  for (let i = end - 1; i >= 0; i--) {
    const item = items[i]!;
    if (item.kind === "turnEnd") break;
    if (item.kind === "user") start = i;
  }
  // A turn with no message of its own (one the engine resumed) starts the transcript as far as
  // this reader knows; abandoning from 0 would take turns that are not this one.
  if (start < 0) return undefined;
  return { keep: items.slice(0, start), tail: items.slice(start) };
}

/** How many turns an attempt holds, for its divider ("3 turns"). */
export function attemptTurns(items: readonly TranscriptItem[]): number {
  const ends = items.filter((item) => item.kind === "turnEnd").length;
  return Math.max(1, ends);
}

/** A turn of the transcript as the checkpoint list sees it, for pairing. */
export interface TranscriptTurn {
  id: string;
  prompt: string;
  at?: number;
}

/** The finished turns of a transcript and the message that started each, in order. */
export function transcriptTurns(items: readonly TranscriptItem[]): TranscriptTurn[] {
  const turns: TranscriptTurn[] = [];
  let prompt = "";
  for (const item of items) {
    if (item.kind === "user") prompt = item.text;
    else if (item.kind === "turnEnd") turns.push({ id: item.id, prompt, at: item.at });
  }
  return turns;
}

/**
 * The checkpoint behind each message you sent, by that message's row id: the turns that have one.
 * The thread's minimap marks each of them with a bead on that turn's strip, so it holds the turns
 * a rewind can go back to and nothing else.
 */
export function checkpointsByMessage(
  items: readonly TranscriptItem[],
  byEnd: Record<string, Checkpoint>,
): Record<string, Checkpoint> {
  const out: Record<string, Checkpoint> = {};
  let startedBy: string | undefined;
  for (const item of items) {
    if (item.kind === "user") {
      startedBy = item.id;
      continue;
    }
    if (item.kind !== "turnEnd") continue;
    const checkpoint = byEnd[item.id];
    // A turn the app resumed has no message of its own, so no row to hang its node on.
    if (checkpoint && startedBy) out[startedBy] = checkpoint;
    startedBy = undefined;
  }
  return out;
}
