// Where a turn's time and money went: the pure half of the Trace chip under a
// finished turn (docs/design.md › Turn trace).
//
// Tool calls arrive with the timestamps the app saw. A model call has no event
// of its own, so it is the gap between the calls around it: from the turn's
// start to the first call, between one call's end and the next call's start,
// and from the last call to the turn's end. The engine's usage updates arrive
// per round, so a sample minus the one before it prices that round; when the
// counts disagree (a harness that reports only a session gauge) no call gets a
// number, because a guessed one would be worse than none.
//
// Layout, sorting and the slowest mark live here too, so the panel only draws
// and the tests drive this file directly.

export type TraceStatus = "completed" | "failed" | "running";
export type TraceKind = "model" | "tool" | "delegates";
/** How the panel orders its rows. */
export type TraceSort = "timeline" | "duration" | "cost";
/** Which number the rows show, and what `cost` sorts by. */
export type TraceMeasure = "cost" | "tokens";

/** A tool call of the turn, as the app watched it. */
export interface TraceCall {
  id: string;
  label: string;
  /** The line that tells two calls apart: a task, a command, a thread's name. */
  detail?: string;
  status: TraceStatus;
  startedAt: number;
  /** Unset while it runs. */
  endedAt?: number;
  /** A call that reaches another thread or starts a subagent: folded into one row. */
  delegate?: boolean;
}

/** Text a turn streamed, for naming the model call it arrived in. */
export interface TraceText {
  at: number;
  kind: "reply" | "thought";
  text: string;
}

/** What a stretch of the turn used: the four numbers a usage update can price. */
export interface TraceUsage {
  inputTokens?: number;
  outputTokens?: number;
  cachedTokens?: number;
  cost?: number;
}

/** Session totals at one moment, as the engine reported them. */
export interface TraceUsageSample {
  at: number;
  usage: TraceUsage;
}

export interface TraceInput {
  /** When the app saw the turn begin. A replayed turn has no such moment. */
  start: number;
  /** When the turn ended. */
  end: number;
  calls: TraceCall[];
  text?: TraceText[];
  /** The session's totals when the turn began, so a sample minus it is the turn's own. */
  before?: TraceUsage;
  /** One per round the engine reported, oldest first. */
  samples?: TraceUsageSample[];
}

export interface TraceRow {
  id: string;
  kind: TraceKind;
  label: string;
  detail?: string;
  status: TraceStatus;
  startedAt: number;
  endedAt: number;
  /** The tool call's transcript id, for the detail its expander draws. */
  toolId?: string;
  /** What a model call streamed, for the detail its expander draws. */
  text?: string;
  /** The tokens and cost this call reported. Absent when the engine reported none for it. */
  usage?: TraceUsage;
  /** A folded delegate row's own calls. */
  children?: TraceRow[];
}

export interface Trace {
  start: number;
  end: number;
  durationMs: number;
  /** Timeline order, delegates folded into one row. */
  rows: TraceRow[];
  /** The row that took longest, when it is a row of its own. */
  slowest?: string;
  /** The turn's own reported usage, when the engine reported enough to subtract. */
  total?: TraceUsage;
  /** Model rows the engine gave no numbers for. */
  unpriced: number;
}

const clamp = (n: number, low: number, high: number) => Math.min(high, Math.max(low, n));

/**
 * Whether a turn has a timeline worth drawing. A replayed turn has no start of
 * its own, and its calls all share the moment the transcript loaded.
 */
export function hasTimeline(end: { startedAt?: number; at?: number }): boolean {
  return end.startedAt !== undefined && end.at !== undefined && end.at > end.startedAt;
}

/** A round's own numbers: this sample minus the one before it, never a guess. */
export function usageDelta(
  sample: TraceUsage | undefined,
  before: TraceUsage | undefined,
): TraceUsage | undefined {
  if (!sample || !before) return undefined;
  const diff = (now?: number, then?: number) =>
    now !== undefined && then !== undefined && now >= then ? now - then : undefined;
  const delta: TraceUsage = {
    inputTokens: diff(sample.inputTokens, before.inputTokens),
    outputTokens: diff(sample.outputTokens, before.outputTokens),
    cachedTokens: diff(sample.cachedTokens, before.cachedTokens),
    cost: diff(sample.cost, before.cost),
  };
  return Object.values(delta).some((v) => v !== undefined) ? delta : undefined;
}

/** A row's tokens: what it read plus what it wrote. Cached is a part of the input, not a third count. */
export function rowTokens(usage: TraceUsage | undefined): number | undefined {
  if (!usage || (usage.inputTokens === undefined && usage.outputTokens === undefined)) return undefined;
  return (usage.inputTokens ?? 0) + (usage.outputTokens ?? 0);
}

/** The number a row shows, and what the third sort orders by, in the measure the panel picked. */
export function rowValue(row: TraceRow, measure: TraceMeasure): number | undefined {
  return measure === "cost" ? row.usage?.cost : rowTokens(row.usage);
}

function rowOf(call: TraceCall, end: number): TraceRow {
  const endedAt = clamp(call.endedAt ?? end, call.startedAt, end);
  return {
    id: call.id,
    kind: "tool",
    label: call.label,
    detail: call.detail,
    status: call.endedAt === undefined ? "running" : call.status,
    startedAt: call.startedAt,
    endedAt,
    toolId: call.id,
  };
}

/** Build one finished turn's trace: its calls, the model time between them, and what each cost. */
export function buildTrace(input: TraceInput): Trace {
  const start = input.start;
  const end = Math.max(start, input.end);
  // A turn's list can hold a neighbour's rows (a wake turn has no message of its
  // own) or a call that began before the app saw the turn: only what falls in
  // this turn's window is its own.
  const calls = input.calls
    .filter((call) => call.startedAt >= start && call.startedAt <= end)
    .sort((a, b) => a.startedAt - b.startedAt || a.id.localeCompare(b.id))
    .map((call) => ({
      ...call,
      endedAt: call.endedAt === undefined ? undefined : clamp(call.endedAt, start, end),
    }));

  // The model calls: every stretch no tool call was running.
  const windows: { startedAt: number; endedAt: number }[] = [];
  let cursor = start;
  for (const call of calls) {
    if (call.startedAt > cursor) windows.push({ startedAt: cursor, endedAt: call.startedAt });
    cursor = Math.max(cursor, call.endedAt ?? end);
  }
  if (end > cursor) windows.push({ startedAt: cursor, endedAt: end });

  const samples = input.samples ?? [];
  const deltas = samples.map((sample, i) =>
    usageDelta(sample.usage, i === 0 ? input.before : samples[i - 1]?.usage),
  );
  // The counts only line up when the harness reported one sample per round;
  // otherwise a row's number would be another round's.
  const perRound = deltas.length === windows.length;

  const models: TraceRow[] = windows.map((window, i) => {
    const inside = (input.text ?? []).filter(
      (mark) => mark.at >= window.startedAt && mark.at <= window.endedAt,
    );
    const reply = inside
      .filter((mark) => mark.kind === "reply")
      .map((mark) => mark.text)
      .join("")
      .trim();
    const thought = inside
      .filter((mark) => mark.kind === "thought")
      .map((mark) => mark.text)
      .join("")
      .trim();
    return {
      id: `model-${i + 1}`,
      kind: "model",
      label: `Model ${i + 1}`,
      detail: reply ? "Wrote the reply" : thought ? "Thought before its next call" : undefined,
      status: "completed",
      startedAt: window.startedAt,
      endedAt: window.endedAt,
      text: reply || thought || undefined,
      usage: perRound ? deltas[i] : undefined,
    };
  });

  const rows = [...calls.filter((call) => !call.delegate).map((call) => rowOf(call, end)), ...models];
  const delegates = calls.filter((call) => call.delegate).map((call) => rowOf(call, end));
  if (delegates.length) {
    const first = delegates[0]!;
    const last = delegates.reduce((a, b) => (b.endedAt > a.endedAt ? b : a), first);
    rows.push({
      id: "delegates",
      kind: "delegates",
      label: `${delegates.length} delegate${delegates.length === 1 ? "" : "s"}`,
      status: delegates.some((row) => row.status === "failed")
        ? "failed"
        : delegates.some((row) => row.status === "running")
          ? "running"
          : "completed",
      startedAt: first.startedAt,
      endedAt: last.endedAt,
      children: delegates,
    });
  }
  rows.sort((a, b) => a.startedAt - b.startedAt || a.endedAt - b.endedAt || a.id.localeCompare(b.id));

  // The longest call among the rows in view. One row alone has nothing to beat.
  let slowest: string | undefined;
  if (rows.length > 1) {
    const longest = rows.reduce((a, b) => (b.endedAt - b.startedAt > a.endedAt - a.startedAt ? b : a));
    if (longest.endedAt > longest.startedAt) slowest = longest.id;
  }

  const last = samples.at(-1);
  const total = last ? usageDelta(last.usage, input.before) : undefined;
  return {
    start,
    end,
    durationMs: end - start,
    rows,
    slowest,
    total,
    unpriced: models.filter((row) => !row.usage).length,
  };
}

/** The rows in the order the panel draws them: unknown numbers always last. */
export function sortTrace(rows: TraceRow[], sort: TraceSort, measure: TraceMeasure): TraceRow[] {
  if (sort === "timeline") return rows;
  const duration = (row: TraceRow) => row.endedAt - row.startedAt;
  const ordered = [...rows];
  if (sort === "duration") {
    return ordered.sort((a, b) => duration(b) - duration(a) || a.startedAt - b.startedAt);
  }
  const value = (row: TraceRow) => rowValue(row, measure);
  return ordered.sort((a, b) => {
    const one = value(a);
    const two = value(b);
    if (one === undefined && two === undefined) return a.startedAt - b.startedAt;
    if (one === undefined) return 1;
    if (two === undefined) return -1;
    return two - one || a.startedAt - b.startedAt;
  });
}

/** Where a row's bar sits on the turn's timeline, as percentages of the track. */
export function barLayout(
  row: TraceRow,
  trace: Pick<Trace, "start" | "end">,
): { left: number; width: number } {
  const span = trace.end - trace.start;
  if (span <= 0) return { left: 0, width: 100 };
  const left = clamp(((row.startedAt - trace.start) / span) * 100, 0, 100);
  const right = clamp(((row.endedAt - trace.start) / span) * 100, left, 100);
  return { left, width: right - left };
}
