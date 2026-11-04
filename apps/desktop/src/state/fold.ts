// Turn folding (docs/design.md, Turn fold):
// while a turn runs, its replies and the call it makes now show, and each run
// of finished calls reads as one line ("Read 4 files and ran 2 commands");
// once it ends, its work (thoughts, tool calls, subagents, the replies written
// between steps) folds under one "Worked for 1m 3s" divider above the final
// answer. What stays visible: the message you sent, the final answer,
// subagents and threads it started, notices, and the turn's footer (time,
// Changes). Pure, so the
// transcript and the tests share it; one pass over the items.

import { subagentOf, type TranscriptItem } from "./thread";
import { orchestrationTool } from "./toolNames";

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
  /** You stopped the turn. */
  stopped?: boolean;
}

/** A run of finished steps shown as one row: `Read 4 files and ran 2 commands`. */
export interface ToolGroup {
  key: string;
  open: boolean;
  count: number;
  failed: number;
  label: string;
  /** The kind most of its calls are, for the row's icon. */
  kind?: string;
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
    item.kind === "switch" ||
    item.kind === "notice" ||
    // Who was sent off, and how it went: the row keeps saying after the turn ends.
    (item.kind === "tool" && (!!subagentOf(item) || orchestrationTool(item.title) === "start_session"))
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
    pushLiveTurn(items, from, to, rows);
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
    stopped: end?.kind === "turnEnd" && end.outcome === "cancelled",
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

/**
 * The running turn shows as it streams; `groupTools` then folds each run of
 * its finished calls into one line, so what it does now reads on its own.
 */
function pushLiveTurn(items: TranscriptItem[], from: number, to: number, rows: FoldRow[]) {
  for (let i = from; i < to; i++) rows.push({ key: items[i]!.id, item: items[i]! });
}

/** The divider's words: `Worked for 1m 3s`, or `You stopped after 12s`. */
export function foldLabel(fold: TurnFold, formatDuration: (ms: number) => string): string {
  const time = fold.elapsedMs !== undefined ? formatDuration(fold.elapsedMs) : undefined;
  if (fold.stopped) return time ? `You stopped after ${time}` : "You stopped this response";
  return time ? `Worked for ${time}` : "Worked";
}

/** Fewer finished calls in a row than this stay as they are. */
export const GROUP_MIN = 2;

type ToolItem = Extract<TranscriptItem, { kind: "tool" }>;

/** The calls that reach other threads, by what they do to them. */
const ORCHESTRATION_KIND: Record<string, string> = {
  start_session: "start",
  fan_out: "start",
  message_session: "message",
  wait_session: "wait",
  read_session: "read_thread",
  list_sessions: "list_thread",
};

/** What a call does, from its ACP kind, else its title. */
export function inferToolKind(item: { toolKind?: string; title: string }): string {
  if (item.toolKind && item.toolKind !== "other") return item.toolKind;
  const t = item.title.toLowerCase();
  // Orchestration calls before the verbs: `read_session` is not a file read, and
  // `start_session` is not a shell. Another server's `start_session` is its own
  // tool, and reads as it always did.
  const called = orchestrationTool(item.title);
  if (called) return ORCHESTRATION_KIND[called]!;
  if (/^(glob|grep|rg|search|find|list|ls)/.test(t)) return "search";
  if (/^(read|cat|view|open)/.test(t)) return "read";
  if (/^(edit|write|apply|patch|create|replace)/.test(t)) return "edit";
  if (/^(bash|shell|exec|run|command)/.test(t)) return "execute";
  if (/^(web|fetch|http|read url|browse)/.test(t)) return "fetch";
  if (/(agent|task|delegate)/.test(t)) return "agent";
  if (/skill/.test(t)) return "skill";
  return "other";
}

type GroupRow = {
  key: string;
  item: ToolItem | Extract<TranscriptItem, { kind: "thought" }>;
  inFold?: string;
};

/** A finished tool call or thought that can fold into its neighbours; a subagent keeps its own row. */
function groupable(row: FoldRow): row is GroupRow {
  if (!("item" in row)) return false;
  const item = row.item;
  if (item.kind === "thought") return item.done;
  return (
    item.kind === "tool" &&
    item.title !== "spawn_agent" &&
    !subagentOf(item) &&
    orchestrationTool(item.title) !== "start_session" &&
    (item.status === "completed" || item.status === "failed")
  );
}

/**
 * The tool kinds a row stands for. A fan-out is one call for as many threads as
 * it was sent to harnesses, so it counts that many starts; anything else is one.
 */
function kindsOf(item: GroupRow["item"]): string[] {
  if (item.kind === "thought") return ["think"];
  if (item.kind === "tool" && orchestrationTool(item.title) === "fan_out") {
    const input = item.input;
    const harnesses =
      input && typeof input === "object" && !Array.isArray(input) ? input.harnesses : undefined;
    const n = Array.isArray(harnesses) ? harnesses.length : 0;
    return Array.from({ length: Math.max(n, 1) }, () => "start");
  }
  return [inferToolKind(item)];
}

const VERBS: [kinds: string[], one: string, many: (n: number) => string][] = [
  [["start"], "started 1 thread", (n) => `started ${n} threads`],
  [["message"], "messaged 1 thread", (n) => `messaged ${n} threads`],
  [["wait"], "waited for threads", () => "waited for threads"],
  [["read_thread"], "read 1 thread", (n) => `read ${n} threads`],
  [["list_thread"], "listed threads", () => "listed threads"],
  [["read"], "read 1 file", (n) => `read ${n} files`],
  [["edit", "delete", "move"], "edited 1 file", (n) => `edited ${n} files`],
  [["execute"], "ran 1 command", (n) => `ran ${n} commands`],
  [["search"], "searched once", (n) => `searched ${n} times`],
  [["fetch"], "fetched 1 page", (n) => `fetched ${n} pages`],
  [["think"], "thought", (n) => `thought ${n} times`],
];

/** The kind most of a run's calls are (the first on a tie), for its icon. */
export function dominantKind(kinds: (string | undefined)[]): string | undefined {
  const counts = new Map<string, number>();
  for (const k of kinds) if (k) counts.set(k, (counts.get(k) ?? 0) + 1);
  let best: string | undefined;
  for (const [k, n] of counts) if (!best || n > counts.get(best)!) best = k;
  return best;
}

/**
 * What a run of calls did: the two most frequent actions
 * in a sentence, then the rest as other actions. `Read 4 files and ran 2
 * commands`, `Ran 2 commands, read 1 file and performed 2 other actions`.
 */
export function toolGroupLabel(kinds: (string | undefined)[]): string {
  const counts = VERBS.map(([names, one, many]) => {
    const n = kinds.filter((k) => !!k && names.includes(k)).length;
    return { n, text: n === 1 ? one : many(n) };
  });
  const parts = counts.filter((c) => c.n > 0).sort((a, b) => b.n - a.n);
  const shown = parts.slice(0, 2).map((p) => p.text);
  const rest = kinds.length - parts.slice(0, 2).reduce((sum, p) => sum + p.n, 0);
  if (rest) shown.push(`performed ${rest} other ${rest === 1 ? "action" : "actions"}`);
  const sentence = shown.length > 1 ? `${shown.slice(0, -1).join(", ")} and ${shown.at(-1)}` : shown[0]!;
  return sentence.charAt(0).toUpperCase() + sentence.slice(1);
}

/**
 * Folds runs of `GROUP_MIN` or more finished tool calls (and the thoughts
 * between them) into one group row each, so
 * a turn of fifty calls reads as a handful of lines. A call still running
 * stays in view. `open` holds the groups the reader opened; an open group
 * keeps its header above the calls.
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
    const run = rows.slice(i, j) as GroupRow[];
    const calls = run.filter((r) => r.item.kind === "tool").length;
    // Thoughts alone, or one call with its thoughts, read fine where they are.
    if (run.length < GROUP_MIN || calls === 0 || (calls === 1 && run.length < 3)) {
      out.push(...run);
    } else {
      const key = `tools-${run[0]!.key}`;
      const kinds = run.flatMap((r) => kindsOf(r.item));
      const group: ToolGroup = {
        key,
        open: open.has(key),
        count: run.length,
        failed: run.filter((r) => r.item.kind === "tool" && r.item.status === "failed").length,
        label: toolGroupLabel(kinds),
        kind: dominantKind(kinds.filter((k) => k !== "think")),
      };
      out.push({ key, group, inFold: row.inFold });
      if (group.open) for (const r of run) out.push({ ...r, inFold: key });
    }
    i = j;
  }
  return out;
}
