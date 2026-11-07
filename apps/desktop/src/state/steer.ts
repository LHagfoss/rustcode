// What a steering note did: the plan step the turn was on when it read the note, and how long each
// step took. A note is a message the user sent into a running turn (`_rustcode/steer`); the plan is
// the harness's own list of steps (`acp/types.ts` `PlanEntry`). Both are read by the plan card at
// the foot of the transcript (`ui/PlanCard.tsx`), which holds a note on its step and each step's
// clock, and by the transcript, which draws a note under its message when the plan cannot hold it.
// Pure, so the reducer, the card and the tests agree.

import type { PlanEntry } from "../acp/types";

/** A step's clock: when the turn came to it, and when it finished it (a step still on has no end). */
export interface PlanClock {
  startedAt: number;
  endedAt?: number;
}

/** Clocks by step text, which is the plan entry's own identity: the list can be rewritten around it. */
export type PlanClocks = Record<string, PlanClock>;

const has = (clocks: PlanClocks, content: string) => Object.hasOwn(clocks, content);

/** A step's clock, by its text. A plan's own text can be any string, so the lookup is an own key. */
export function planClock(clocks: PlanClocks, content: string): PlanClock | undefined {
  return has(clocks, content) ? clocks[content] : undefined;
}

/** The step a turn is on: the one in progress, and the first if a harness marks more than one. */
export function activeStep(plan: readonly PlanEntry[]): { index: number; content: string } | undefined {
  const index = plan.findIndex((entry) => entry.status === "in_progress");
  const entry = index < 0 ? undefined : plan[index];
  return entry ? { index, content: entry.content } : undefined;
}

/**
 * The clocks after a plan update: a step the turn comes to starts one, a step it finishes stops it.
 * Only a live turn is clocked — a replay of history or a paused thread says nothing about when a
 * step ran, so it gets no made-up duration. A step the plan no longer lists loses its clock.
 */
export function advancePlanClocks(
  clocks: PlanClocks,
  plan: readonly PlanEntry[],
  now: number,
  live: boolean,
): PlanClocks {
  if (!live) return clocks;
  const next: PlanClocks = {};
  for (const entry of plan) {
    const clock = planClock(clocks, entry.content);
    if (entry.status === "completed") {
      // A step done the first time the app saw it is not clocked: its time is unknown, not zero.
      if (clock) next[entry.content] = clock.endedAt === undefined ? { ...clock, endedAt: now } : clock;
    } else if (entry.status === "in_progress") {
      // A step that came up again after it had finished starts a new clock.
      next[entry.content] = clock && clock.endedAt === undefined ? clock : { startedAt: now };
    } else if (clock) {
      // Back to pending: the clock a step already had stays, so a step put back keeps its start.
      next[entry.content] = clock;
    }
  }
  return next;
}

/**
 * How long a step has been running, for the card: `now` past its start while the turn is on it, its
 * whole time once it is done. Nothing when the app did not see it start.
 */
export function stepElapsed(
  entry: PlanEntry,
  clock: PlanClock | undefined,
  now: number,
  running: boolean,
): number | undefined {
  if (!clock) return undefined;
  if (clock.endedAt !== undefined) return Math.max(0, clock.endedAt - clock.startedAt);
  // A paused thread's clock stops where the reader last saw it, not at every later render.
  if (!running || entry.status !== "in_progress") return undefined;
  return Math.max(0, now - clock.startedAt);
}

/**
 * A message the user sent into a running turn, and where the turn read it. The plan step is the one
 * that was in progress when the engine accepted the note; a thread with no plan has none, and the
 * transcript draws the note under its message instead.
 */
export interface SteerNote {
  /** The transcript item the note went in as (that `user` item's id). */
  id: string;
  text: string;
  at: number;
  step?: { index: number; content: string };
}

/** Where a note's marker goes: the plan steps that hold one, and the notes that fall back to the transcript. */
export interface SteerMarks {
  byStep: Map<string, SteerNote[]>;
  inline: SteerNote[];
}

/**
 * The markers each note gets. A note sits on its plan step while the plan still lists that step
 * (matched by its text, so a rewritten list around it keeps it); when the plan moved past it or was
 * replaced, the note draws in the transcript under its message, so it is never lost. Chips from two
 * notes on one step keep the order their turns read them in.
 */
export function steerMarks(notes: readonly SteerNote[], plan: readonly PlanEntry[]): SteerMarks {
  const byStep = new Map<string, SteerNote[]>();
  const inline: SteerNote[] = [];
  for (const note of notes) {
    if (note.step && plan.some((entry) => entry.content === note.step?.content)) {
      const held = byStep.get(note.step.content);
      if (held) held.push(note);
      else byStep.set(note.step.content, [note]);
    } else {
      inline.push(note);
    }
  }
  return { byStep, inline };
}

/** Whether the plan card has anything to say: work left, or a note it keeps on one of its steps. */
export function planCardShown(plan: readonly PlanEntry[], notes: readonly SteerNote[]): boolean {
  if (plan.some((entry) => entry.status !== "completed")) return true;
  return steerMarks(notes, plan).byStep.size > 0;
}

/** A note as one line, for its marker: its first line, squashed and clipped where it is long. */
export function steerLine(text: string, max = 96): string {
  const line = (text.trim().split("\n")[0] ?? "").replace(/\s+/g, " ").trim();
  if (line.length <= max) return line;
  return `${line.slice(0, max - 1).trimEnd()}…`;
}
