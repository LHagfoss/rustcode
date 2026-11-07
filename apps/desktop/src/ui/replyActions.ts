// The one action row a turn has: copy, fork and the time, at the foot of the
// turn's final reply, always in view.
//
// No other row draws actions. An intermediate message, a thought or a tool row
// only narrates what the turn is doing, and while a turn runs its reply has no
// row yet (the footer arrives with the turn's end). Nothing here holds a
// hover's room either: a row that only fills in under the pointer leaves a gap
// in the transcript that reads as a mistake.
//
// Copy puts the reply's own Markdown on the clipboard (`CopyButton` takes the
// text as it is), and only when it has something past whitespace.

export type TurnOutcome = "completed" | "cancelled" | "failed";

export interface TurnActions {
  /** Whether the row has anything to draw: a copy, a fork, or a time. */
  row: boolean;
  /** The reply's raw Markdown, or null when the row has no copy button. */
  copy: string | null;
  fork: boolean;
  time: boolean;
}

/**
 * What a finished turn's action row holds. The reply is the turn's last one, which
 * is the only reply the row copies or forks; a turn that wrote no answer has
 * nothing to copy, so nothing to fork either, and the row is its time alone.
 */
export function turnActions({
  text,
  at,
  threadId,
}: {
  /** The turn's final reply, as its raw Markdown. */
  text?: string | null;
  /** When the turn ended, in ms since the epoch. */
  at?: number;
  /** The thread a fork copies; without it the row cannot fork. */
  threadId?: string;
}): TurnActions {
  const copy = text?.trim() ? text : null;
  return { row: !!copy || !!at, copy, fork: !!threadId && !!copy, time: !!at };
}

/**
 * Whether a finished turn draws a footer at all. A turn that folded its work and wrote no answer has
 * said everything its header says (how long it worked, when), and has nothing to copy or fork, so
 * the row goes; its Changes chip and its checks still keep one.
 */
export function turnFooterHidden({
  folded,
  reply,
  completion,
  changes,
  trace,
  outcome,
}: {
  /** The turn's work is one summary row above. */
  folded?: boolean;
  /** The reply the footer would copy. */
  reply?: string;
  /** The turn's completion card (files changed, checks run). */
  completion?: boolean;
  /** The turn's checkpoint chip, when it changed files. */
  changes?: boolean;
  /** The turn has a Trace chip, which is content of its own. */
  trace?: boolean;
  outcome: TurnOutcome;
}): boolean {
  return (
    !!folded &&
    !reply?.trim() &&
    !completion &&
    !changes &&
    !trace &&
    outcome !== "failed" &&
    outcome !== "cancelled"
  );
}
