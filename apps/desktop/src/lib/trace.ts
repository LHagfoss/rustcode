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
  /** Stretches the call spent waiting on the user's approval, as the app recorded them. */
  waits?: { from: number; to?: number }[];
}

/** A stretch of a call spent waiting on the user, closed at both ends. */
export interface TraceWait {
  from: number;
  to: number;
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
  /** The thread's model, which names its model calls. */
  model?: string;
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
  /** Stretches of the call the user's approval held, drawn apart from its work. */
  waits?: TraceWait[];
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
 * The panel's columns, in px: a row's label column, the track, and the numbers
 * at the right. The axis over the rows and the rows themselves are laid out on
 * these, so a tick sits exactly on the end of the bar under it.
 */
export const TRACE_COLUMNS = { label: 184, value: 124 } as const;

/**
 * How far inside the track's ends the bars are drawn, in px on both sides: a bar
 * at the turn's first instant keeps clear of the track's rounded edge, and the
 * axis ticks sit on the ends of that inner box, not the track's own.
 */
export const TRACE_TRACK_INSET = 4;

/** The narrowest a bar is drawn, in px, so a call that took an instant still shows. */
export const TRACE_MIN_BAR = 3;

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
  const waits = (call.waits ?? [])
    .map((wait) => ({
      from: clamp(wait.from, call.startedAt, endedAt),
      to: clamp(wait.to ?? end, call.startedAt, endedAt),
    }))
    .filter((wait) => wait.to > wait.from);
  return {
    id: call.id,
    kind: "tool",
    label: call.label,
    detail: call.detail,
    status: call.endedAt === undefined ? "running" : call.status,
    startedAt: call.startedAt,
    endedAt,
    waits: waits.length ? waits : undefined,
    toolId: call.id,
  };
}

/** How long a call spent waiting on the user, in all its waits together. */
export function waitedMs(row: TraceRow): number {
  return (row.waits ?? []).reduce((sum, wait) => sum + (wait.to - wait.from), 0);
}

/**
 * How long a call actually worked: its span minus what the user's approval
 * held. This is what `Duration` sorts by and what `Slowest` marks, so an
 * approval someone took their time over does not read as a slow call.
 */
export function workDuration(row: TraceRow): number {
  return Math.max(0, row.endedAt - row.startedAt - waitedMs(row));
}

/** A model name for a row, the round's ordinal a suffix only when there is more than one. */
function modelLabel(model: string | undefined, index: number, total: number): string {
  const name = model?.trim();
  if (!name) return `Model ${index + 1}`;
  return total > 1 ? `${name} · ${index + 1}` : name;
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
      label: modelLabel(input.model, i, windows.length),
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
      waits: delegates.flatMap((row) => row.waits ?? []).sort((a, b) => a.from - b.from),
      children: delegates,
    });
  }
  rows.sort((a, b) => a.startedAt - b.startedAt || a.endedAt - b.endedAt || a.id.localeCompare(b.id));

  // The longest call among the rows in view, by the work it did: one row alone
  // has nothing to beat.
  let slowest: string | undefined;
  if (rows.length > 1) {
    const longest = rows.reduce((a, b) => (workDuration(b) > workDuration(a) ? b : a));
    if (workDuration(longest) > 0) slowest = longest.id;
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
  const duration = (row: TraceRow) => workDuration(row);
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

function onTrack(
  from: number,
  to: number,
  trace: Pick<Trace, "start" | "end">,
): { left: number; width: number } {
  const span = trace.end - trace.start;
  if (span <= 0) return { left: 0, width: 100 };
  const left = clamp(((from - trace.start) / span) * 100, 0, 100);
  const right = clamp(((to - trace.start) / span) * 100, left, 100);
  return { left, width: right - left };
}

/** Where a row's bar sits on the turn's timeline, as percentages of the track. */
export function barLayout(row: TraceRow, trace: Pick<Trace, "start" | "end">) {
  return onTrack(row.startedAt, row.endedAt, trace);
}

/** One drawn piece of a row's bar: the work it did, or the approval it waited for. */
export interface TraceBar {
  kind: "work" | "wait";
  left: number;
  width: number;
}

/**
 * A row's bar in pieces, in time order: the work solid, and each wait on the
 * user hatched apart from it. A call that only waited draws one wait.
 */
export function barSegments(row: TraceRow, trace: Pick<Trace, "start" | "end">): TraceBar[] {
  const bars: TraceBar[] = [];
  let cursor = row.startedAt;
  for (const wait of [...(row.waits ?? [])].sort((a, b) => a.from - b.from)) {
    if (wait.from > cursor) bars.push({ kind: "work", ...onTrack(cursor, wait.from, trace) });
    bars.push({ kind: "wait", ...onTrack(Math.max(wait.from, cursor), wait.to, trace) });
    cursor = Math.max(cursor, wait.to);
  }
  if (row.endedAt > cursor) bars.push({ kind: "work", ...onTrack(cursor, row.endedAt, trace) });
  if (!bars.length) bars.push({ kind: "work", ...onTrack(row.startedAt, row.endedAt, trace) });
  return bars;
}

/**
 * A label that keeps both ends when it has to give: `edit src/…/Composer.tsx`
 * still says what it touched. The row's column is narrow, the bar's is not.
 */
export function middleClip(text: string, max: number): string {
  if (text.length <= max) return text;
  const start = Math.max(1, Math.floor((max - 1) / 2));
  const end = Math.max(1, max - 1 - start);
  // The end that starts at the file name, when the budget holds it: the row
  // says which file it touched, not the last few characters of a folder.
  const slash = text.lastIndexOf("/");
  const tail = slash >= 0 && text.length - slash <= end ? text.slice(slash) : text.slice(text.length - end);
  return `${text.slice(0, max - 1 - tail.length)}…${tail}`;
}
