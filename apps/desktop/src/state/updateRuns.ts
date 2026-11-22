// Runs of short automatic turns, folded. A thread that waits on CI is woken for
// every pull request's news and every background task that ends, and each wake
// writes a turn of a few seconds that mostly repeats what is still pending
// ("Still waiting on #23 and #25"). Three or more of them in a row read as
// one line, `4 earlier updates · #21, #17, #18, #22`, with the newest
// still in view under it: that is the one saying where things stand now.
// Pure, so the transcript and the tests share it.

import type { TurnFold } from "./fold";
import type { TranscriptItem } from "./thread";

/** A turn this long or longer did real work, and its report keeps its own place. */
export const UPDATE_MAX_MS = 30_000;

/** Fewer automatic turns in a row than this stay as they are. */
export const UPDATE_RUN_MIN = 3;

/** The folded turns of one run, as its row says them. */
export interface UpdateRun {
  key: string;
  open: boolean;
  /** How many turns the row stands for. */
  count: number;
  /** The pull requests whose news woke them, in order and once each: `#21`. */
  pulls: string[];
}

export type UpdateRow = { key: string; item?: TranscriptItem; inFold?: string; fold?: TurnFold };

const PULL_NEWS = /^Pull request #(\d+)\b/;

/** Whether a turn starts on the app's own message (a wake, a pull request's news) and not the user's. */
function automaticStart(item: TranscriptItem | undefined): boolean {
  if (item?.kind === "user") return !!item.wake || !!item.continued;
  // The engine resuming a turn on its own leaves a `Background task finished` notice (state/thread.ts `wakeTurn`).
  return item?.kind === "notice" && item.tone === "info" && item.id.startsWith("wake-");
}

/** Whether a turn's rows are an automatic start, a short reply and its end, and nothing the reader needs in view. */
function quietTurn(rows: readonly UpdateRow[]): boolean {
  if (!automaticStart(rows[0]?.item)) return false;
  const end = rows.at(-1)?.item;
  if (end?.kind !== "turnEnd" || end.outcome !== "completed") return false;
  if (end.elapsedMs === undefined || end.elapsedMs >= UPDATE_MAX_MS) return false;
  return rows.slice(1).every((row) => {
    if (row.fold) return row.fold.files === 0;
    // An opened `Worked for …` shows its steps; they fold away with the turn.
    if (row.inFold) return true;
    const kind = row.item?.kind;
    return kind === "assistant" || kind === "turnEnd" || (kind === "notice" && row.item?.tone === "info");
  });
}

/** Where each turn starts: at a message, or after a turn's end (a woken turn has no message). */
function turnsOf<T extends UpdateRow>(rows: readonly T[]): T[][] {
  const turns: T[][] = [];
  rows.forEach((row, i) => {
    const prev = rows[i - 1];
    const starts =
      i === 0 ||
      (!row.inFold && row.item?.kind === "user") ||
      (!prev?.inFold && prev?.item?.kind === "turnEnd");
    if (starts) turns.push([row]);
    else turns.at(-1)!.push(row);
  });
  return turns;
}

/** The pull requests a run's turns were woken for. */
function pullsOf(turns: readonly UpdateRow[][]): string[] {
  const seen = new Set<string>();
  for (const turn of turns) {
    const item = turn[0]?.item;
    const number = item?.kind === "user" ? PULL_NEWS.exec(item.text)?.[1] : undefined;
    if (number) seen.add(`#${number}`);
  }
  return [...seen];
}

/**
 * The rows with each run of `UPDATE_RUN_MIN` or more quiet automatic turns folded under one row,
 * except its newest turn, which stays in view. `open` holds the runs the reader opened: an open
 * run keeps its row above the turns it folds.
 */
export function foldUpdateRuns<T extends UpdateRow>(
  rows: readonly T[],
  open: ReadonlySet<string>,
): (T | { key: string; updates: UpdateRun })[] {
  const out: (T | { key: string; updates: UpdateRun })[] = [];
  const turns = turnsOf(rows);
  for (let i = 0; i < turns.length; ) {
    let j = i;
    while (j < turns.length && quietTurn(turns[j]!)) j++;
    if (j - i < UPDATE_RUN_MIN) {
      out.push(...turns[i]!);
      i++;
      continue;
    }
    const folded = turns.slice(i, j - 1);
    const key = `updates-${folded[0]![0]!.key}`;
    const updates: UpdateRun = { key, open: open.has(key), count: folded.length, pulls: pullsOf(folded) };
    out.push({ key, updates });
    if (updates.open) for (const turn of folded) out.push(...turn);
    out.push(...turns[j - 1]!);
    i = j;
  }
  return out;
}

/** The row's words: `4 earlier updates · #21, #17`. */
export function updateRunLabel(run: UpdateRun): { label: string; detail?: string } {
  return {
    label: `${run.count} earlier updates`,
    detail: run.pulls.length ? run.pulls.join(", ") : undefined,
  };
}
