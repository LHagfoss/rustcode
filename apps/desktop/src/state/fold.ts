// Turn folding (docs/design.md, Turn fold): while a turn runs, every step
// shows as it streams; once it ends, its work (thoughts, tool calls,
// subagents, the replies written between steps) folds into one summary row
// above the final answer. What stays visible: the message you sent, the final
// answer, warnings and errors, and the turn's footer (time, Changes). Pure, so
// the transcript and the tests share it; one pass over the items.

import type { TranscriptItem } from "./thread";

export interface TurnFold {
  /** Stable per turn: its footer's id, else its first step's. */
  key: string;
  open: boolean;
  elapsedMs?: number;
  /** Thoughts, tool calls and in-between replies folded away. */
  steps: number;
  tools: number;
  failed: number;
  /** Distinct files the turn's tool calls changed. */
  files: number;
}

export type FoldRow =
  | { key: string; item: TranscriptItem; inFold?: string }
  | { key: string; fold: TurnFold };

/** Whether an item stays in view when its turn folds. */
function staysVisible(item: TranscriptItem): boolean {
  return (
    item.kind === "user" ||
    item.kind === "turnEnd" ||
    (item.kind === "notice" && (item.tone === "error" || item.tone === "warning"))
  );
}

/**
 * The transcript's rows with finished turns folded. `open` holds the keys of
 * the turns the reader opened. The last turn while `running` never folds.
 */
export function foldTurns(items: TranscriptItem[], running: boolean, open: ReadonlySet<string>): FoldRow[] {
  const rows: FoldRow[] = [];
  let start = 0;
  for (let i = 0; i <= items.length; i++) {
    // A turn runs from a message you sent up to the next one (or the end).
    if (i < items.length && (items[i]!.kind !== "user" || i === start)) continue;
    pushTurn(items, start, i, running && i === items.length, open, rows);
    start = i;
  }
  return rows;
}

function pushTurn(
  items: TranscriptItem[],
  from: number,
  to: number,
  live: boolean,
  open: ReadonlySet<string>,
  rows: FoldRow[],
) {
  // The final answer: the replies at the end of the turn, after its last step.
  let answerFrom = to;
  for (let i = to - 1; i >= from; i--) {
    const item = items[i]!;
    if (item.kind === "assistant") answerFrom = i;
    else if (staysVisible(item) && item.kind !== "user") continue;
    else break;
  }
  const work: number[] = [];
  for (let i = from; i < to; i++) {
    const item = items[i]!;
    if (!staysVisible(item) && i < answerFrom) work.push(i);
  }
  if (live || !work.length) {
    for (let i = from; i < to; i++) rows.push({ key: items[i]!.id, item: items[i]! });
    return;
  }
  const end = items.slice(from, to).find((x) => x.kind === "turnEnd");
  const key = `fold-${end?.id ?? items[work[0]!]!.id}`;
  const files = new Set<string>();
  let tools = 0;
  let failed = 0;
  for (const i of work) {
    const item = items[i]!;
    if (item.kind !== "tool") continue;
    tools++;
    if (item.status === "failed") failed++;
    for (const diff of item.diffs) files.add(diff.path);
  }
  const fold: TurnFold = {
    key,
    open: open.has(key),
    elapsedMs: end?.kind === "turnEnd" ? end.elapsedMs : undefined,
    steps: work.length,
    tools,
    failed,
    files: files.size,
  };
  const firstWork = work[0]!;
  for (let i = from; i < to; i++) {
    const item = items[i]!;
    if (i === firstWork) rows.push({ key, fold });
    const folded = i < answerFrom && !staysVisible(item);
    if (!folded) rows.push({ key: item.id, item });
    else if (fold.open) rows.push({ key: item.id, item, inFold: key });
  }
}

/** The summary row's words: `Worked for 1m 20s · 14 steps · 3 files changed · 1 failed`. */
export function foldLabel(fold: TurnFold, formatDuration: (ms: number) => string): string {
  const parts = [
    fold.elapsedMs !== undefined ? `Worked for ${formatDuration(fold.elapsedMs)}` : "Worked",
    `${fold.steps} ${fold.steps === 1 ? "step" : "steps"}`,
  ];
  if (fold.files) parts.push(`${fold.files} ${fold.files === 1 ? "file" : "files"} changed`);
  if (fold.failed) parts.push(`${fold.failed} failed`);
  return parts.join(" · ");
}
