// One rule for the action row under a reply, and for the one a finished turn's
// footer takes over. Both rows hold the same things (copy, fork, the time), so
// a turn shows exactly one of them and they cannot disagree about what is there.
//
// Copy puts the reply's own Markdown on the clipboard (`CopyButton` takes the
// text as it is), and only when it has something past whitespace.

export type ActionRowWhere = "reply" | "turnEnd";

export type TurnOutcome = "completed" | "cancelled" | "failed";

export interface ReplyActions {
  /** Whether the row draws at all. A turn footer always has an outcome and a time. */
  row: boolean;
  /** The reply's raw Markdown, or null when the row has no copy button. */
  copy: string | null;
  fork: boolean;
  time: boolean;
}

const NONE: ReplyActions = { row: false, copy: null, fork: false, time: false };

export function replyActions({
  where,
  text,
  at,
  threadId,
  streaming = false,
  lastInTurn = false,
}: {
  /** The row under a reply, or a finished turn's footer. */
  where: ActionRowWhere;
  /** The reply's raw Markdown. */
  text?: string | null;
  /** When it was written, in ms since the epoch. */
  at?: number;
  /** The thread a fork copies; without it the row cannot fork. */
  threadId?: string;
  /** A reply still being written never forks, and shows no copy until it ends. */
  streaming?: boolean;
  /** The last reply of a finished turn: the turn footer shows copy and fork instead. */
  lastInTurn?: boolean;
}): ReplyActions {
  const copy = text?.trim() ? text : null;
  // A turn footer with no reply under it has nothing to copy, and nothing to fork either.
  if (where === "turnEnd") return { row: true, copy, fork: !!threadId && !!copy, time: !!at };
  if (lastInTurn) return NONE;
  return {
    // A reply with nothing to copy and no time has no row (and no hover target).
    row: !!copy || !!at,
    copy,
    fork: !!threadId && !streaming,
    time: !!at,
  };
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
  outcome: TurnOutcome;
}): boolean {
  return (
    !!folded && !reply?.trim() && !completion && !changes && outcome !== "failed" && outcome !== "cancelled"
  );
}
