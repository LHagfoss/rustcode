
import type { TurnFold } from "./fold";
import type { TranscriptItem } from "./thread";

export const UPDATE_MAX_MS = 30_000;

export const UPDATE_RUN_MIN = 3;

export interface UpdateRun {
  key: string;
  open: boolean;
  count: number;
  pulls: string[];
}

export type UpdateRow = { key: string; item?: TranscriptItem; inFold?: string; fold?: TurnFold };

const PULL_NEWS = /^Pull request #(\d+)\b/;

function automaticStart(item: TranscriptItem | undefined): boolean {
  if (item?.kind === "user") return !!item.wake || !!item.continued;
  return item?.kind === "notice" && item.tone === "info" && item.id.startsWith("wake-");
}

function quietTurn(rows: readonly UpdateRow[]): boolean {
  if (!automaticStart(rows[0]?.item)) return false;
  const end = rows.at(-1)?.item;
  if (end?.kind !== "turnEnd" || end.outcome !== "completed") return false;
  if (end.elapsedMs === undefined || end.elapsedMs >= UPDATE_MAX_MS) return false;
  return rows.slice(1).every((row) => {
    if (row.fold) return row.fold.files === 0;
    if (row.inFold) return true;
    const kind = row.item?.kind;
    return kind === "assistant" || kind === "turnEnd" || (kind === "notice" && row.item?.tone === "info");
  });
}

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

function pullsOf(turns: readonly UpdateRow[][]): string[] {
  const seen = new Set<string>();
  for (const turn of turns) {
    const item = turn[0]?.item;
    const number = item?.kind === "user" ? PULL_NEWS.exec(item.text)?.[1] : undefined;
    if (number) seen.add(`#${number}`);
  }
  return [...seen];
}

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

export function updateRunLabel(run: UpdateRun): { label: string; detail?: string } {
  return {
    label: `${run.count} earlier updates`,
    detail: run.pulls.length ? run.pulls.join(", ") : undefined,
  };
}
