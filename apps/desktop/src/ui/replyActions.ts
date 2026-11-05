// One rule for the action row under a reply, and for the one a finished turn's
// footer takes over. Both rows hold the same things (copy, fork, the time), so
// a turn shows exactly one of them and they cannot disagree about what is there.
//
// Copy puts the reply's own Markdown on the clipboard (`CopyButton` takes the
// text as it is), and only when it has something past whitespace.

export type ActionRowWhere = "reply" | "turnEnd";

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
  if (where === "turnEnd") return { row: true, copy, fork: !!threadId, time: !!at };
  if (lastInTurn) return NONE;
  return {
    // A reply with nothing to copy and no time has no row (and no hover target).
    row: !!copy || !!at,
    copy,
    fork: !!threadId && !streaming,
    time: !!at,
  };
}
