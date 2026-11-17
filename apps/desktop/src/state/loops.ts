// Loops: prompts the scheduler daemon runs on a schedule, each run in a fresh thread
// (docs/daemon.md › Loops, issue 3892). What the engine's `_rustcode/loops/*` answers mean on the
// page: a schedule in words, a loop's state and last result, and what its runs left for the user.

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
  /** The thread the run started. */
  threadId?: string;
  /** Why the run started nothing: the app was closed. */
  skipped?: string;
}

export interface Loop {
  id: string;
  name: string;
  paused: boolean;
  schedule: LoopSchedule;
  workspace: string;
  prompt: string;
  harness?: string;
  model?: string;
  effort?: string;
  readOnly: boolean;
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
    skipped: str(raw.skipped),
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
    model: str(raw.model),
    effort: str(raw.effort),
    readOnly: bool(raw.readOnly) ?? true,
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
    if (weekday === "1-5") return `Weekdays at ${at}`;
    if (weekday === "0,6" || weekday === "6,0") return `Weekends at ${at}`;
    if (isNumber(weekday)) {
      const name = DAYS[Number(weekday) % 7];
      return name ? `Every ${name} at ${at}` : null;
    }
    return null;
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

/** The New loop dialog's schedules; "custom" opens the cron field. */
export const SCHEDULE_PRESETS = [
  { id: "hourly", label: "Every hour", cron: "0 * * * *" },
  { id: "4h", label: "Every 4 hours", cron: "0 */4 * * *" },
  { id: "morning", label: "Every morning", cron: "0 9 * * *" },
  { id: "weekdays", label: "Weekdays", cron: "0 9 * * 1-5" },
  { id: "weekly", label: "Mondays", cron: "0 9 * * 1" },
  { id: "custom", label: "Custom", cron: "" },
] as const;

export type SchedulePreset = (typeof SCHEDULE_PRESETS)[number]["id"];
/** What a new loop runs on unless the user picks another: every 4 hours. */
export const DEFAULT_PRESET: SchedulePreset = "4h";

/** The preset a cron is, so editing a loop opens on its own choice; anything else is custom. */
export function presetOf(expression: string): SchedulePreset {
  const normal = expression.trim().split(/\s+/).join(" ");
  return SCHEDULE_PRESETS.find((preset) => preset.cron && preset.cron === normal)?.id ?? "custom";
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
      return { text: `Finished ${when}`, tone: "ok" };
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
  /** "failed": the run's error; "open": its thread is still in the list, waiting for the user. */
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
  harness?: string;
  model?: string;
  effort?: string;
  readOnly: boolean;
}

/** The user's IANA zone, which a loop's cron is read in. */
function userTimeZone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

/** The save call's parameters: a cron in the user's own zone, empty fields left out. */
export function saveParams(draft: LoopDraft, timezone = userTimeZone()): JsonObject {
  const params: JsonObject = {
    name: draft.name.trim(),
    workspace: draft.workspace,
    prompt: draft.prompt.trim(),
    schedule: {
      kind: "cron",
      expression: draft.cron.trim().split(/\s+/).join(" "),
      ...(timezone ? { timezone } : {}),
    },
    readOnly: draft.readOnly,
  };
  if (draft.id) params.id = draft.id;
  for (const key of ["harness", "model", "effort"] as const) {
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
