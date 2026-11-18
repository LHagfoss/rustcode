// Loops: prompts the scheduler daemon runs on a schedule, each run in a fresh thread
// (docs/daemon.md › Loops, issue 3892). What the engine's `_rustcode/loops/*` answers mean on the
// page: a schedule in words, a loop's state and last result, and what its runs left for the user.

import { create } from "zustand";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { request } from "./store";

/** Failed runs in a row that pause a loop: the daemon's `LOOP_FAILURE_LIMIT`. */
export const LOOP_FAILURE_LIMIT = 3;
/** How far back the review list looks. */
export const REVIEW_DAYS = 7;
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

export type LoopSchedule =
  | { kind: "cron"; expression: string; timezone?: string }
  | { kind: "daily"; hour: number; minute: number; timezone?: string }
  | { kind: "monthly"; day: number; hour: number; minute: number; timezone?: string }
  | { kind: "once"; at: string };

export type LoopRunState = "claimed" | "running" | "succeeded" | "failed" | "cancelled" | "ambiguous";

export interface LoopRun {
  id: string;
  state: LoopRunState;
  scheduledAt: number;
  startedAt?: number;
  finishedAt?: number;
  error?: string;
  /** The thread the run started, or the loop's own thread it ran in. */
  threadId?: string;
  /** The run found nothing for the user, and its thread was archived. */
  quiet?: boolean;
  /** Why the run started nothing: the app was closed, or the loop's thread was busy. */
  skipped?: string;
  /** The job's script printed nothing, so no agent turn started. */
  silent?: boolean;
}

export interface Loop {
  id: string;
  name: string;
  paused: boolean;
  schedule: LoopSchedule;
  workspace: string;
  prompt: string;
  harness?: string;
  account?: string;
  model?: string;
  effort?: string;
  readOnly: boolean;
  /** The thread every run goes to; absent starts a fresh thread per run. */
  threadId?: string;
  /** Script mode: run first; the agent wakes only when it prints something. */
  script?: string;
  nextDueAt?: number;
  /** Newest first. */
  runs: LoopRun[];
}

export interface LoopsSnapshot {
  daemonRunning: boolean;
  loops: Loop[];
}

const RUN_STATES: readonly LoopRunState[] = [
  "claimed",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "ambiguous",
];

function time(value: unknown): number | undefined {
  const text = str(value);
  if (!text) return undefined;
  const at = Date.parse(text);
  return Number.isFinite(at) ? at : undefined;
}

function scheduleOf(value: Json | undefined): LoopSchedule {
  const raw = obj(value) ?? {};
  const timezone = str(raw.timezone);
  const n = (key: string) => (typeof raw[key] === "number" ? (raw[key] as number) : 0);
  switch (str(raw.kind)) {
    case "daily":
      return { kind: "daily", hour: n("hour"), minute: n("minute"), ...(timezone ? { timezone } : {}) };
    case "monthly":
      return {
        kind: "monthly",
        day: n("day"),
        hour: n("hour"),
        minute: n("minute"),
        ...(timezone ? { timezone } : {}),
      };
    case "once":
    case "at":
      return { kind: "once", at: str(raw.at) ?? "" };
    default:
      return { kind: "cron", expression: str(raw.expression) ?? "", ...(timezone ? { timezone } : {}) };
  }
}

function runOf(value: Json): LoopRun | null {
  const raw = obj(value);
  const id = str(raw?.id);
  if (!raw || !id) return null;
  const state = str(raw.state) as LoopRunState | undefined;
  return {
    id,
    state: state && RUN_STATES.includes(state) ? state : "failed",
    scheduledAt: time(raw.scheduledAt) ?? 0,
    startedAt: time(raw.startedAt),
    finishedAt: time(raw.finishedAt),
    error: str(raw.error),
    threadId: str(raw.threadId),
    quiet: bool(raw.quiet) ?? false,
    skipped: str(raw.skipped),
    silent: bool(raw.silent) ?? false,
  };
}

function loopOf(value: Json): Loop | null {
  const raw = obj(value);
  const id = str(raw?.id);
  if (!raw || !id) return null;
  return {
    id,
    name: str(raw.name) || id,
    paused: bool(raw.paused) ?? false,
    schedule: scheduleOf(raw.schedule),
    workspace: str(raw.workspace) ?? "",
    prompt: str(raw.prompt) ?? "",
    harness: str(raw.harness),
    account: str(raw.account),
    model: str(raw.model),
    effort: str(raw.effort),
    readOnly: bool(raw.readOnly) ?? true,
    threadId: str(raw.threadId),
    script: str(raw.script),
    nextDueAt: time(raw.nextDueAt),
    runs: arr(raw.runs).flatMap((run) => runOf(run) ?? []),
  };
}

/** `_rustcode/loops/list`, read leniently: a field the engine left out reads as its default. */
export function parseLoops(value: Json): LoopsSnapshot {
  const raw = obj(value) ?? {};
  return {
    daemonRunning: bool(raw.daemonRunning) ?? false,
    loops: arr(raw.loops).flatMap((loop) => loopOf(loop) ?? []),
  };
}

// ---------------------------------------------------------------- schedule in words

const pad = (n: number) => String(n).padStart(2, "0");
const clock = (hour: number, minute: number) => `${pad(hour)}:${pad(minute)}`;
const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

function ordinal(n: number): string {
  const tens = n % 100;
  if (tens >= 11 && tens <= 13) return `${n}th`;
  return `${n}${{ 1: "st", 2: "nd", 3: "rd" }[n % 10] ?? "th"}`;
}

const isNumber = (field: string) => /^\d+$/.test(field);

/** "Weekdays", "Every Monday", "Mon, Wed and Fri": the days a schedule runs on, Monday first. */
export function daysInWords(days: readonly number[]): string {
  const set = new Set(days);
  if (set.size === 7) return "Every day";
  if (set.size === 5 && [1, 2, 3, 4, 5].every((d) => set.has(d))) return "Weekdays";
  if (set.size === 2 && set.has(0) && set.has(6)) return "Weekends";
  const ordered = [1, 2, 3, 4, 5, 6, 0].filter((d) => set.has(d));
  if (ordered.length === 1) return `Every ${DAYS[ordered[0] ?? 0]}`;
  const short = ordered.map((d) => (DAYS[d] ?? "").slice(0, 3));
  return `${short.slice(0, -1).join(", ")} and ${short.at(-1)}`;
}

/** A five-field cron in words, or null when it is not one of the shapes people write by hand. */
export function cronInWords(expression: string): string | null {
  const fields = expression.trim().split(/\s+/);
  if (fields.length !== 5) return null;
  const [minute, hour, day, month, weekday] = fields as [string, string, string, string, string];
  if (month !== "*") return null;
  const every = /^\*\/(\d+)$/;
  if (hour === "*" && day === "*" && weekday === "*") {
    if (minute === "*") return "Every minute";
    const step = every.exec(minute);
    if (step) return Number(step[1]) === 1 ? "Every minute" : `Every ${Number(step[1])} minutes`;
    if (isNumber(minute))
      return Number(minute) === 0 ? "Every hour" : `Every hour at :${pad(Number(minute))}`;
    return null;
  }
  if (!isNumber(minute)) return null;
  const hourStep = every.exec(hour);
  if (hourStep && day === "*" && weekday === "*") {
    const n = Number(hourStep[1]);
    const words = n === 1 ? "Every hour" : `Every ${n} hours`;
    return Number(minute) === 0 ? words : `${words} at :${pad(Number(minute))}`;
  }
  if (!isNumber(hour)) return null;
  const at = clock(Number(hour), Number(minute));
  if (day === "*" && weekday === "*") return `Every day at ${at}`;
  if (day === "*") {
    const days = weekdaysOf(weekday);
    return days ? `${daysInWords(days)} at ${at}` : null;
  }
  if (weekday === "*" && isNumber(day)) return `Monthly on the ${ordinal(Number(day))} at ${at}`;
  return null;
}

/** How the page says when a loop runs: "Every 4 hours", "Weekdays at 09:00", else the cron itself. */
export function scheduleInWords(schedule: LoopSchedule): string {
  switch (schedule.kind) {
    case "cron":
      return cronInWords(schedule.expression) ?? `Cron ${schedule.expression}`;
    case "daily":
      return `Every day at ${clock(schedule.hour, schedule.minute)}`;
    case "monthly":
      return `Monthly on the ${ordinal(schedule.day)} at ${clock(schedule.hour, schedule.minute)}`;
    case "once": {
      const at = Date.parse(schedule.at);
      if (!Number.isFinite(at)) return "Once";
      const d = new Date(at);
      return `Once on ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} at ${clock(d.getHours(), d.getMinutes())}`;
    }
  }
}

// ---------------------------------------------------------------- the schedule form

/** How the loop dialog builds a schedule; every kind but "once" is saved as a cron. */
export type Repeat = "hours" | "days" | "monthly" | "once" | "cron";

export interface ScheduleForm {
  repeat: Repeat;
  /** "hours": the hours between runs. */
  everyHours: number;
  /** "days" and "monthly": the clock time, "HH:MM"; "hours" keeps its minute. */
  time: string;
  /** "days": the weekdays it runs on, 0 = Sunday. */
  days: number[];
  /** "monthly": the day of the month. */
  monthDay: number;
  /** "once": a local date and time, as an `<input type="datetime-local">` holds it. */
  onceAt: string;
  /** "cron": the expression as written. */
  cron: string;
}

export const HOUR_STEPS = [1, 2, 3, 4, 6, 8, 12] as const;
export const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];
export const WEEKDAYS = [1, 2, 3, 4, 5];
export const WEEKENDS = [0, 6];
/** Weekday names a cron reads the same in every implementation, unlike numbers. */
const CRON_DAYS = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/** "YYYY-MM-DDTHH:MM" in local time, what a datetime-local input holds. */
export function localDateTime(at: Date): string {
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${clock(at.getHours(), at.getMinutes())}`;
}

/** The form a new loop opens on: every 4 hours. */
export function defaultForm(now = Date.now()): ScheduleForm {
  const soon = new Date(now + HOUR);
  soon.setMinutes(0, 0, 0);
  return {
    repeat: "hours",
    everyHours: 4,
    time: "09:00",
    days: [...EVERY_DAY],
    monthDay: 1,
    onceAt: localDateTime(soon),
    cron: "0 */4 * * *",
  };
}

const sameDays = (a: readonly number[], b: readonly number[]) =>
  a.length === b.length && a.every((day) => b.includes(day));

function clockParts(time: string): [number, number] | null {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
  if (!match) return null;
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  return hour <= 23 && minute <= 59 ? [hour, minute] : null;
}

/**
 * A cron weekday field as days, 0 = Sunday: numbers the standard way (0 and 7 Sunday), names,
 * ranges and lists. Null for what it cannot read, such as a step.
 */
export function weekdaysOf(field: string): number[] | null {
  if (field === "*") return [...EVERY_DAY];
  const day = (token: string): number | null => {
    if (/^\d$/.test(token)) return Number(token) <= 7 ? Number(token) % 7 : null;
    const at = CRON_DAYS.indexOf(token.slice(0, 3).toUpperCase());
    return at >= 0 && /^[a-z]+$/i.test(token) ? at : null;
  };
  const days = new Set<number>();
  for (const part of field.split(",")) {
    const [from, to, extra] = part.split("-");
    if (extra !== undefined || from === undefined) return null;
    const a = day(from);
    const b = to === undefined ? a : to === "7" || to === "0" ? 7 : day(to);
    if (a === null || b === null || a > b) return null;
    for (let d = a; d <= b; d++) days.add(d % 7);
  }
  return [...days].sort((x, y) => x - y);
}

/** The days as a cron weekday field: `*`, `MON-FRI`, or a list of names. */
function weekdayField(days: readonly number[]): string {
  if (days.length === 0 || sameDays(days, EVERY_DAY)) return "*";
  if (sameDays(days, WEEKDAYS)) return "MON-FRI";
  return [...days]
    .sort((a, b) => a - b)
    .map((day) => CRON_DAYS[day])
    .join(",");
}

/** The cron a form saves; empty for "once", which saves a time instead. */
export function cronOfForm(form: ScheduleForm): string {
  const [hour, minute] = clockParts(form.time) ?? [9, 0];
  switch (form.repeat) {
    case "hours":
      return form.everyHours <= 1 ? `${minute} * * * *` : `${minute} */${form.everyHours} * * *`;
    case "days":
      return `${minute} ${hour} * * ${weekdayField(form.days)}`;
    case "monthly":
      return `${minute} ${hour} ${form.monthDay} * *`;
    case "cron":
      return form.cron.trim().split(/\s+/).join(" ");
    case "once":
      return "";
  }
}

/** The form that shows a saved cron, so an edit opens on its own choice; anything else is "cron". */
export function formOfCron(expression: string, base: ScheduleForm = defaultForm()): ScheduleForm {
  const normal = expression.trim().split(/\s+/).join(" ");
  const asCron = { ...base, repeat: "cron" as const, cron: normal };
  const fields = normal.split(" ");
  if (fields.length !== 5) return asCron;
  const [minute, hour, day, month, weekday] = fields as [string, string, string, string, string];
  if (month !== "*" || !isNumber(minute) || Number(minute) > 59) return asCron;
  const time = (h: number) => clock(h, Number(minute));
  if (day === "*" && weekday === "*") {
    if (hour === "*") return { ...base, repeat: "hours", everyHours: 1, time: time(0), cron: normal };
    const step = /^\*\/(\d+)$/.exec(hour);
    const every = Number(step?.[1]);
    if (step && (HOUR_STEPS as readonly number[]).includes(every))
      return { ...base, repeat: "hours", everyHours: every, time: time(0), cron: normal };
  }
  if (!isNumber(hour) || Number(hour) > 23) return asCron;
  if (day === "*") {
    const days = weekdaysOf(weekday);
    return days ? { ...base, repeat: "days", days, time: time(Number(hour)), cron: normal } : asCron;
  }
  if (weekday === "*" && isNumber(day) && Number(day) >= 1 && Number(day) <= 28)
    return { ...base, repeat: "monthly", monthDay: Number(day), time: time(Number(hour)), cron: normal };
  return asCron;
}

/** The time a "once" form runs at, or null while it is not a valid date and time. */
export function onceOfForm(form: ScheduleForm): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(form.onceAt);
  if (!match) return null;
  const [, y, mo, d, h, mi] = match.map(Number) as number[];
  const at = new Date(y ?? 0, (mo ?? 1) - 1, d ?? 1, h ?? 0, mi ?? 0);
  return Number.isFinite(at.getTime()) ? at : null;
}

/** Why a form cannot be saved yet; null when it can. */
export function formProblem(form: ScheduleForm, now = Date.now()): string | null {
  switch (form.repeat) {
    case "once": {
      const at = onceOfForm(form);
      if (!at) return "Pick a date and time";
      return at.getTime() <= now ? "That time has passed" : null;
    }
    case "days":
      if (form.days.length === 0) return "Pick at least one day";
      return clockParts(form.time) ? null : "Pick a time";
    case "monthly":
      return clockParts(form.time) ? null : "Pick a time";
    case "cron":
      return cronProblem(form.cron);
    case "hours":
      return null;
  }
}

/** Why a cron cannot be saved, checked before the engine is asked; null when it reads as five fields. */
export function cronProblem(expression: string): string | null {
  const fields = expression.trim().split(/\s+/).filter(Boolean);
  if (fields.length === 0) return "Write a cron expression";
  if (fields.length !== 5) return "A cron expression has five fields: minute hour day month weekday";
  if (!fields.every((field) => /^[\d*/,\-A-Za-z]+$/.test(field))) return "That is not a cron expression";
  return null;
}

// ---------------------------------------------------------------- state and results

/** "just now", "5 min ago", "3 h ago", "2 d ago". */
export function agoText(at: number, now = Date.now()): string {
  const past = Math.max(0, now - at);
  if (past < MINUTE) return "just now";
  if (past < HOUR) return `${Math.floor(past / MINUTE)} min ago`;
  if (past < DAY) return `${Math.floor(past / HOUR)} h ago`;
  return `${Math.floor(past / DAY)} d ago`;
}

const isActive = (run: LoopRun) => run.state === "claimed" || run.state === "running";
const isFailure = (run: LoopRun) =>
  run.finishedAt !== undefined && (run.state === "failed" || run.state === "ambiguous");

/**
 * Why a paused loop stopped: the daemon pauses one whose last three runs failed, and those runs say
 * why. Null for a loop the user paused, or one that runs.
 */
export function pauseReason(loop: Loop): string | null {
  if (!loop.paused) return null;
  const last = loop.runs.slice(0, LOOP_FAILURE_LIMIT);
  if (last.length < LOOP_FAILURE_LIMIT || !last.every(isFailure)) return null;
  return `Paused after ${LOOP_FAILURE_LIMIT} failed runs: ${last[0]?.error ?? "no reason given"}`;
}

export type LoopState = "running" | "failing" | "paused" | "active";

/** One word for the row's badge: a run going, paused by failures, paused, or waiting for its next run. */
export function loopState(loop: Loop): LoopState {
  if (loop.runs.some(isActive)) return "running";
  if (pauseReason(loop)) return "failing";
  return loop.paused ? "paused" : "active";
}

/** The last run, as the list says it. */
export function lastResult(loop: Loop, now = Date.now()): { text: string; tone: "ok" | "bad" | "muted" } {
  const run = loop.runs[0];
  if (!run) return { text: "No runs yet", tone: "muted" };
  if (isActive(run)) return { text: "Running now", tone: "muted" };
  const when = agoText(run.finishedAt ?? run.startedAt ?? run.scheduledAt, now);
  switch (run.state) {
    case "succeeded":
      if (run.silent) return { text: `Script printed nothing ${when}`, tone: "ok" };
      return run.quiet
        ? { text: `Nothing to report ${when}`, tone: "ok" }
        : { text: `Finished ${when}`, tone: "ok" };
    case "cancelled":
      return { text: run.skipped ? `Skipped ${when}: ${run.skipped}` : `Stopped ${when}`, tone: "muted" };
    default:
      return { text: `Failed ${when}${run.error ? `: ${run.error}` : ""}`, tone: "bad" };
  }
}

/** When the next run is, or why there is none. */
export function nextRunText(loop: Loop, now = Date.now()): string {
  if (loop.paused) return "Paused";
  if (loop.nextDueAt === undefined) return "Not scheduled";
  const left = loop.nextDueAt - now;
  if (left <= 0) return "Due now";
  if (left < HOUR) return `in ${Math.ceil(left / MINUTE)} min`;
  if (left < DAY) return `in ${Math.round(left / HOUR)} h`;
  return `in ${Math.round(left / DAY)} d`;
}

export interface ReviewItem {
  loop: Loop;
  run: LoopRun;
  /**
   * "failed": the run's error; "open": its thread is still in the list, waiting for the user. A quiet
   * run archived its thread, so it is never "open".
   */
  kind: "failed" | "open";
}

/**
 * What loops did on their own in the last week that the user has not looked at: failed runs, and
 * runs whose thread is still open (not archived). Marking one reviewed clears it. Newest first.
 */
export function reviewItems(
  loops: readonly Loop[],
  isOpen: (threadId: string) => boolean,
  reviewed: ReadonlySet<string>,
  now = Date.now(),
): ReviewItem[] {
  const since = now - REVIEW_DAYS * DAY;
  const items: ReviewItem[] = [];
  for (const loop of loops) {
    for (const run of loop.runs) {
      const at = run.finishedAt ?? run.startedAt ?? run.scheduledAt;
      if (at < since || reviewed.has(run.id) || isActive(run)) continue;
      if (isFailure(run)) items.push({ loop, run, kind: "failed" });
      else if (run.state === "succeeded" && run.threadId && isOpen(run.threadId))
        items.push({ loop, run, kind: "open" });
    }
  }
  const at = (item: ReviewItem) => item.run.finishedAt ?? item.run.startedAt ?? item.run.scheduledAt;
  return items.sort((a, b) => at(b) - at(a));
}

// ---------------------------------------------------------------- reviewed runs

const REVIEWED_KEY = "rustcode.loopsReviewed";
/** Reviewed run ids kept: well past a week of runs for a handful of loops. */
const MAX_REVIEWED = 500;

export function loadReviewed(): Set<string> {
  try {
    const raw = JSON.parse(localStorage.getItem(REVIEWED_KEY) ?? "[]");
    return new Set(Array.isArray(raw) ? raw.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}

export function saveReviewed(ids: ReadonlySet<string>): void {
  try {
    localStorage.setItem(REVIEWED_KEY, JSON.stringify([...ids].slice(-MAX_REVIEWED)));
  } catch {
    // Storage full or unavailable: the review list forgets on the next launch, nothing worse.
  }
}

// ---------------------------------------------------------------- engine calls

export interface LoopDraft {
  id?: string;
  name: string;
  workspace: string;
  prompt: string;
  cron: string;
  timezone?: string;
  /** A one-shot run at this time instead of the cron. */
  once?: Date;
  harness?: string;
  account?: string;
  model?: string;
  effort?: string;
  readOnly: boolean;
  /** Send every run to this thread instead of starting a fresh one. */
  threadId?: string;
  /** Script mode: run first; the agent wakes only when it prints something. */
  script?: string;
}

/** The user's IANA zone, which a loop's cron is read in. */
export function userTimeZone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

/** The save call's parameters: a cron in the user's own zone, empty fields left out. */
export function saveParams(draft: LoopDraft, timezone = draft.timezone ?? userTimeZone()): JsonObject {
  const params: JsonObject = {
    name: draft.name.trim(),
    workspace: draft.workspace,
    prompt: draft.prompt.trim(),
    schedule: draft.once
      ? { kind: "at", at: draft.once.toISOString() }
      : {
          kind: "cron",
          expression: draft.cron.trim().split(/\s+/).join(" "),
          ...(timezone ? { timezone } : {}),
        },
    readOnly: draft.readOnly,
  };
  if (draft.id) params.id = draft.id;
  const script = draft.script?.trim();
  if (script) params.script = script;
  for (const key of draft.threadId
    ? (["threadId"] as const)
    : (["harness", "account", "model", "effort"] as const)) {
    const value = draft[key]?.trim();
    if (value) params[key] = value;
  }
  return params;
}

export async function saveLoop(draft: LoopDraft): Promise<void> {
  await request(Method.loopsSave, saveParams(draft), 30_000);
}

export async function setLoopPaused(id: string, paused: boolean): Promise<void> {
  await request(Method.loopsSetPaused, { id, paused }, 20_000);
}

export async function runLoopNow(id: string): Promise<void> {
  await request(Method.scheduledRun, { id }, 20_000);
}

export async function deleteLoop(id: string): Promise<void> {
  await request(Method.scheduledDelete, { id }, 20_000);
}

export async function startScheduler(): Promise<void> {
  await request(Method.loopsStartDaemon, {}, 30_000);
}

// ---------------------------------------------------------------- loop threads in the sidebar

/** Which loop each thread a run started or ran in belongs to, by thread id: the sidebar's Loop badge. */
export const useLoopThreads = create<{ byThread: Record<string, string> }>(() => ({ byThread: {} }));

/** Every thread the loops' recorded runs went to, and the loop's name. */
export function loopThreadNames(loops: readonly Loop[]): Record<string, string> {
  const byThread: Record<string, string> = {};
  for (const loop of loops) {
    if (loop.threadId) byThread[loop.threadId] = loop.name;
    for (const run of loop.runs) if (run.threadId) byThread[run.threadId] = loop.name;
  }
  return byThread;
}

/** Keep the sidebar's Loop badges in step with a list the page or a refresh read. */
export function rememberLoops(snapshot: LoopsSnapshot): void {
  useLoopThreads.setState({ byThread: loopThreadNames(snapshot.loops) });
}

/** Read the loops once for the badges; a scheduler that is not running leaves them empty. */
export async function refreshLoopThreads(): Promise<void> {
  try {
    rememberLoops(parseLoops(await request(Method.loopsList, {}, 20_000)));
  } catch {
    // No engine yet or no scheduler: no badges, and the next refresh tries again.
  }
}
