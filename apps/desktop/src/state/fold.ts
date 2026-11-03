// Turn folding (docs/design.md, Turn fold): while a turn runs, every step
// shows as it streams; once it ends, its work (thoughts, tool calls,
// subagents, the replies written between steps) folds into one summary row
// above the final answer. What stays visible: the message you sent, the final
// answer, warnings and errors, and the turn's footer (time, Changes). Pure, so
// the transcript and the tests share it; one pass over the items.

import { subagentOf, type TranscriptItem } from "./thread";

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
  /** The running turn's earlier steps, folded so the latest batch reads on its own. */
  live?: boolean;
}

/** A run of finished tool calls shown as one row: `Read 4 files · Ran 2 commands`. */
export interface ToolGroup {
  key: string;
  open: boolean;
  count: number;
  failed: number;
  label: string;
}

export type FoldRow =
  | { key: string; item: TranscriptItem; inFold?: string }
  | { key: string; fold: TurnFold; inFold?: string }
  | { key: string; group: ToolGroup; inFold?: string };

/** Whether an item stays in view when its turn folds. */
function staysVisible(item: TranscriptItem): boolean {
  return (
    item.kind === "user" ||
    item.kind === "turnEnd" ||
    item.kind === "handoff" ||
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
  if (live) {
    pushLiveTurn(items, from, to, open, rows);
    return;
  }
  if (!work.length) {
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

/** At most this many of the running turn's latest steps show; the rest fold. */
export const LIVE_TAIL = 6;

/**
 * The running turn: what it does now shows, everything before folds into one
 * row. "Now" is the latest reply and the steps after it, at most `LIVE_TAIL`
 * of them, else the last `LIVE_TAIL` steps.
 */
function pushLiveTurn(
  items: TranscriptItem[],
  from: number,
  to: number,
  open: ReadonlySet<string>,
  rows: FoldRow[],
) {
  let tailFrom = Math.max(from, to - LIVE_TAIL);
  for (let i = to - 1; i >= from; i--) {
    if (items[i]!.kind === "assistant") {
      tailFrom = Math.max(i, to - LIVE_TAIL - 1);
      break;
    }
  }
  const earlier: number[] = [];
  for (let i = from; i < tailFrom; i++) if (!staysVisible(items[i]!)) earlier.push(i);
  // One or two earlier steps read fine where they are.
  if (earlier.length < 3) {
    for (let i = from; i < to; i++) rows.push({ key: items[i]!.id, item: items[i]! });
    return;
  }
  const key = `live-${items[from]!.id}`;
  const files = new Set<string>();
  let tools = 0;
  let failed = 0;
  for (const i of earlier) {
    const item = items[i]!;
    if (item.kind !== "tool") continue;
    tools++;
    if (item.status === "failed") failed++;
    for (const diff of item.diffs) files.add(diff.path);
  }
  const fold: TurnFold = {
    key,
    open: open.has(key),
    steps: earlier.length,
    tools,
    failed,
    files: files.size,
    live: true,
  };
  const first = earlier[0]!;
  for (let i = from; i < to; i++) {
    const item = items[i]!;
    if (i === first) rows.push({ key, fold });
    const folded = i < tailFrom && !staysVisible(item);
    if (!folded) rows.push({ key: item.id, item });
    else if (fold.open) rows.push({ key: item.id, item, inFold: key });
  }
}

/** The summary row's words: `Worked for 1m 20s · 14 steps · 3 files changed · 1 failed`. */
export function foldLabel(fold: TurnFold, formatDuration: (ms: number) => string): string {
  const parts = fold.live
    ? [`${fold.steps} earlier ${fold.steps === 1 ? "step" : "steps"}`]
    : [
        fold.elapsedMs !== undefined ? `Worked for ${formatDuration(fold.elapsedMs)}` : "Worked",
        `${fold.steps} ${fold.steps === 1 ? "step" : "steps"}`,
      ];
  if (fold.files) parts.push(`${fold.files} ${fold.files === 1 ? "file" : "files"} changed`);
  if (fold.failed) parts.push(`${fold.failed} failed`);
  return parts.join(" · ");
}

/** Fewer finished calls in a row than this stay as they are. */
export const GROUP_MIN = 3;

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/** A finished tool call that can fold into its neighbours; a subagent keeps its own row. */
function groupable(row: FoldRow): row is { key: string; item: ToolItem; inFold?: string } {
  if (!("item" in row)) return false;
  const item = row.item;
  return (
    item.kind === "tool" &&
    item.title !== "spawn_agent" &&
    !subagentOf(item) &&
    (item.status === "completed" || item.status === "failed")
  );
}

const VERBS: [kinds: string[], one: string, many: (n: number) => string][] = [
  [["read"], "Read 1 file", (n) => `Read ${n} files`],
  [["edit", "delete", "move"], "Edited 1 file", (n) => `Edited ${n} files`],
  [["execute"], "Ran 1 command", (n) => `Ran ${n} commands`],
  [["search"], "Searched once", (n) => `Searched ${n} times`],
  [["fetch"], "Fetched 1 page", (n) => `Fetched ${n} pages`],
];

/** What a run of calls did, most frequent first: three parts, then a count of the rest. */
export function toolGroupLabel(kinds: (string | undefined)[]): string {
  const counts = VERBS.map(([names, one, many]) => {
    const n = kinds.filter((k) => !!k && names.includes(k)).length;
    return { n, text: n === 1 ? one : many(n) };
  });
  const known = counts.reduce((sum, c) => sum + c.n, 0);
  const other = kinds.length - known;
  const parts = counts.filter((c) => c.n > 0).sort((a, b) => b.n - a.n);
  if (other) parts.push({ n: other, text: `${other} other ${other === 1 ? "step" : "steps"}` });
  // Three parts, or four when the fourth would only be replaced by "1 more"-style words.
  const room = parts.length === 4 ? 4 : 3;
  const shown = parts.slice(0, room).map((p) => p.text);
  const rest = parts.slice(room).reduce((sum, p) => sum + p.n, 0);
  if (rest) shown.push(`${rest} more`);
  return shown.join(" · ");
}

/**
 * Folds runs of `GROUP_MIN` or more finished tool calls into one group row
 * each, the way T3 Code's timeline does, so a turn of fifty calls reads as a
 * handful of lines. A call still running stays in view. `open` holds the
 * groups the reader opened; an open group keeps its header above the calls.
 */
export function groupTools(rows: FoldRow[], open: ReadonlySet<string>): FoldRow[] {
  const out: FoldRow[] = [];
  let i = 0;
  while (i < rows.length) {
    const row = rows[i]!;
    if (!groupable(row)) {
      out.push(row);
      i++;
      continue;
    }
    let j = i;
    while (j < rows.length) {
      const next = rows[j]!;
      if (!groupable(next) || next.inFold !== row.inFold) break;
      j++;
    }
    const run = rows.slice(i, j) as { key: string; item: ToolItem; inFold?: string }[];
    if (run.length < GROUP_MIN) {
      out.push(...run);
    } else {
      const key = `tools-${run[0]!.key}`;
      const group: ToolGroup = {
        key,
        open: open.has(key),
        count: run.length,
        failed: run.filter((r) => r.item.status === "failed").length,
        label: toolGroupLabel(run.map((r) => r.item.toolKind)),
      };
      out.push({ key, group, inFold: row.inFold });
      if (group.open) for (const r of run) out.push({ ...r, inFold: key });
    }
    i = j;
  }
  return out;
}
