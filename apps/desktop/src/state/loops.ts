// Loops: prompts the scheduler daemon runs on a schedule, each run in a fresh thread
// (docs/daemon.md › Loops, issue 3892). What the engine's `_rustcode/loops/*` answers mean on the
// page: a schedule in words, a loop's state and last result, and what its runs left for the user.

import { create } from "zustand";
import { arr, bool, type Json, type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { connectionFor, request, useApp } from "./store";
import { LOCAL_HOST, threadKey } from "./threadKeys";
import { workflowParams } from "./workflowJobs";

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
  | { kind: "once"; at: string }
  /** Never due on its own: runs from Run now or the loop's webhook. */
  | { kind: "manual" };

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
  /** The one-line verdict a non-quiet run opened with. */
  verdict?: string;
  /** The run found nothing for the user, and its thread was archived. */
  quiet?: boolean;
  /** Why the run started nothing: the app was closed, or the loop's thread was busy. */
  skipped?: string;
  /** The job's script printed nothing, so no agent turn started. */
  silent?: boolean;
  /** The harness's login was signed out; `error` names the login and how to sign in. */
  signedOut?: boolean;
  /** A hub Loop: the machine the run was sent to; `threadId` is a thread there. */
  machine?: string;
  /** That machine was offline, so the hub holds the run until it connects again. */
  queued?: boolean;
  /** A workflow job's run: the workflow run it started in `threadId`, and how that run ended. */
  runId?: string;
  workflowStatus?: string;
}

export interface Loop {
  id: string;
  /** The machine whose scheduler owns this job (`""` for this computer, else a paired machine id). */
  host: string;
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
  /** Calling the loop's webhook URL starts a run (its URL is shown once, when made). */
  webhook: boolean;
  /** The webhook events it takes (`check_suite`, `pull_request.opened`); none takes every delivery. */
  webhookEvents?: string[];
  /** A hub Loop: each run goes to this member of the hub, and `workspace` is a folder there. */
  machine?: string;
  /** A workflow job: the `.rustcode/workflows/<name>.js` each run starts, in place of the prompt. */
  workflow?: string;
  /** The workflow's `args`. */
  args?: JsonObject;
  nextDueAt?: number;
  /** Newest first. */
  runs: LoopRun[];
}

/** A member joined to a hub, as `_rustcode/loops/list` reports it when the host is a hub. */
export interface LoopHubMember {
  hostId: string;
  name: string;
  online: boolean;
}

export interface LoopsSnapshot {
  daemonRunning: boolean;
  loops: Loop[];
  /** Present when this host is a hub and its gateway answered. */
  hubMembers?: LoopHubMember[];
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
    case "manual":
      return { kind: "manual" };
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
    verdict: str(raw.verdict),
    quiet: bool(raw.quiet) ?? false,
    skipped: str(raw.skipped),
    silent: bool(raw.silent) ?? false,
    signedOut: bool(raw.signedOut) ?? false,
    machine: str(raw.machine),
    queued: bool(raw.queued) ?? false,
    runId: str(raw.runId),
    workflowStatus: str(raw.workflowStatus),
  };
}

function loopOf(value: Json, host = LOCAL_HOST): Loop | null {
  const raw = obj(value);
  const id = str(raw?.id);
  if (!raw || !id) return null;
  return {
    id,
    host,
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
    webhook: bool(raw.webhook) ?? false,
    webhookEvents: arr(raw.webhookEvents).flatMap((event) => str(event) ?? []),
    machine: str(raw.machine),
    workflow: str(raw.workflow),
    args: obj(raw.args),
    nextDueAt: time(raw.nextDueAt),
    runs: arr(raw.runs).flatMap((run) => runOf(run) ?? []),
  };
}

function hubMembersOf(value: Json | undefined): LoopHubMember[] | undefined {
  if (!Array.isArray(value)) return undefined;
  return value.flatMap((entry) => {
    const raw = obj(entry);
    const hostId = str(raw?.hostId);
    const name = str(raw?.name);
    if (!raw || !hostId || !name) return [];
    return [{ hostId, name, online: bool(raw.online) ?? false }];
  });
}

/** `_rustcode/loops/list`, read leniently: a field the engine left out reads as its default. */
export function parseLoops(value: Json, host = LOCAL_HOST): LoopsSnapshot {
  const raw = obj(value) ?? {};
  const hubMembers = hubMembersOf(raw.hubMembers);
  return {
    daemonRunning: bool(raw.daemonRunning) ?? false,
    loops: arr(raw.loops).flatMap((loop) => loopOf(loop, host) ?? []),
    ...(hubMembers !== undefined ? { hubMembers } : {}),
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
    case "manual":
      return "No schedule";
  }
}

// ---------------------------------------------------------------- the schedule form

/**
 * How the loop dialog builds a schedule; every kind but "once" and "manual" is saved as a cron.
 * "manual" has no schedule: the loop runs from Run now or its webhook.
 */
export type Repeat = "hours" | "days" | "monthly" | "once" | "cron" | "manual";

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
    case "manual":
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
    case "manual":
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

/** Compact run duration ("<1s", "42s", "2m 15s", "1h 4m"), or null when unavailable. */
export function runDurationText(startedAt?: number, finishedAt?: number): string | null {
  if (startedAt === undefined || finishedAt === undefined || finishedAt < startedAt) return null;
  const totalSec = Math.round((finishedAt - startedAt) / 1000);
  if (totalSec < 1) return "<1s";
  if (totalSec < 60) return `${totalSec}s`;
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  if (mins < 60) return secs ? `${mins}m ${secs}s` : `${mins}m`;
  const hours = Math.floor(mins / 60);
  const remMins = mins % 60;
  return remMins ? `${hours}h ${remMins}m` : `${hours}h`;
}

const RUN_DAY_WEEKDAY = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const RUN_DAY_DATE = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

/** "Today 08:00", "Yesterday 08:00", "Thu 08:00", or "Nov 16 08:00" in local 24h time. */
export function runTimestampText(at: number, now = Date.now()): string {
  const d = new Date(at);
  const ref = new Date(now);
  const time = clock(d.getHours(), d.getMinutes());
  if (d.toDateString() === ref.toDateString()) return `Today ${time}`;
  const yesterday = new Date(ref);
  yesterday.setDate(ref.getDate() - 1);
  if (d.toDateString() === yesterday.toDateString()) return `Yesterday ${time}`;
  if (now - at >= 0 && now - at < 6 * DAY) return `${RUN_DAY_WEEKDAY.format(d)} ${time}`;
  return `${RUN_DAY_DATE.format(d)} ${time}`;
}

const FALLBACK_TIMEZONES = [
  "UTC",
  "America/New_York",
  "America/Chicago",
  "America/Denver",
  "America/Los_Angeles",
  "Europe/London",
  "Europe/Oslo",
  "Europe/Berlin",
  "Europe/Paris",
  "Asia/Tokyo",
  "Asia/Singapore",
  "Australia/Sydney",
];

/** Available IANA timezones, with the user's own zone first. */
export function timeZoneChoices(current?: string): string[] {
  const local = userTimeZone();
  const intlWithValues = Intl as unknown as { supportedValuesOf?: (key: string) => string[] };
  const supported =
    typeof intlWithValues.supportedValuesOf === "function"
      ? intlWithValues.supportedValuesOf("timeZone")
      : FALLBACK_TIMEZONES;
  const seen = new Set<string>();
  const list: string[] = [];
  for (const zone of [local, current, "UTC", ...supported]) {
    if (zone && !seen.has(zone)) {
      seen.add(zone);
      list.push(zone);
    }
  }
  return list;
}

const isActive = (run: LoopRun) => run.state === "claimed" || run.state === "running";
const isFailure = (run: LoopRun) =>
  run.finishedAt !== undefined && (run.state === "failed" || run.state === "ambiguous");

/**
 * Why a paused loop stopped: the daemon pauses one whose last three runs failed, or whose last run
 * found its login signed out, and those runs say why. Null for a loop the user paused, or one that runs.
 */
export function pauseReason(loop: Loop): string | null {
  if (!loop.paused) return null;
  // A signed-out login pauses the job on its first run, and the run says which login and how to sign in.
  const latest = loop.runs[0];
  if (latest && isFailure(latest) && latest.signedOut)
    return `Paused: ${latest.error ?? "the harness is not signed in"}`;
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
      // A hub Loop's run ends once its machine has the thread (or the hub holds it for that machine).
      if (run.queued)
        return { text: `Waiting for ${run.machine ?? "its machine"} (queued ${when})`, tone: "muted" };
      if (run.machine) return { text: `Started on ${run.machine} ${when}`, tone: "ok" };
      return run.quiet
        ? { text: `Nothing to report ${when}`, tone: "ok" }
        : run.verdict
          ? { text: `${run.verdict} · ${when}`, tone: "ok" }
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
  if (loop.schedule.kind === "manual") return loop.webhook ? "On webhook" : "Run by hand";
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
      else if (run.state === "succeeded" && run.threadId && !run.machine) {
        const key = threadKey(loop.host ?? LOCAL_HOST, run.threadId);
        if (isOpen(key)) items.push({ loop, run, kind: "open" });
      }
    }
  }
  const at = (item: ReviewItem) => item.run.finishedAt ?? item.run.startedAt ?? item.run.scheduledAt;
  return items.sort((a, b) => at(b) - at(a));
}

/** Review items that say the same thing: one job's failures with one error, or its open threads. */
export interface ReviewGroup {
  /** The newest of them, which the row shows and opens. */
  item: ReviewItem;
  /** Every run in the group: marking the row reviewed marks them all. */
  runIds: string[];
}

const reviewKey = ({ loop, run, kind }: ReviewItem) =>
  kind === "failed"
    ? `${loop.host ?? LOCAL_HOST}\u0000${loop.id}\u0000failed\u0000${run.error ?? ""}`
    : `${loop.host ?? LOCAL_HOST}\u0000${loop.id}\u0000open\u0000${run.id}`;

/**
 * Fold the review list: a job that failed the same way on every run is one row with a count, not one
 * row per run. An open thread stays a row of its own, since each needs reading. Newest first, as given.
 */
export function groupReview(items: readonly ReviewItem[]): ReviewGroup[] {
  const groups = new Map<string, ReviewGroup>();
  for (const item of items) {
    const key = reviewKey(item);
    const group = groups.get(key);
    if (group) group.runIds.push(item.run.id);
    else groups.set(key, { item, runIds: [item.run.id] });
  }
  return [...groups.values()];
}

/** When a review item happened. */
export const reviewAt = ({ run }: ReviewItem) => run.finishedAt ?? run.startedAt ?? run.scheduledAt;

const DAY_LABEL = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "short", day: "numeric" });

/** "Today", "Yesterday", or the date: the review timeline's day headings. */
export function reviewDayLabel(at: number, now = Date.now()): string {
  const day = new Date(at);
  day.setHours(0, 0, 0, 0);
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const days = Math.round((today.getTime() - day.getTime()) / DAY);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  return DAY_LABEL.format(at);
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
  /** No schedule: the loop runs from Run now or its webhook. */
  manual?: boolean;
  /** Start a run when the loop's webhook URL is called. */
  webhook?: boolean;
  /** Replace the webhook's secret, so the old URL stops working. */
  rotateWebhook?: boolean;
  /** The webhook events to take, as typed: separated by commas or spaces. */
  webhookEvents?: string;
  /** On the hub: run each turn on this member instead of this computer. */
  machine?: string;
  /** Set (even empty) for a workflow job: the saved workflow each run starts. */
  workflow?: string;
  /** The workflow's args as typed: a JSON object, or blank for none. */
  args?: string;
}

/** A loop's webhook as saving it answers, once, when its secret is new. */
export interface LoopWebhook {
  /** The URL to give the sender: the public address when one is set, else this computer's. */
  url: string;
  /** False when only this computer can reach the URL. */
  public: boolean;
  /** The secret alone, for a sender that takes `Authorization: Bearer <secret>`. */
  secret: string;
}

/** The user's IANA zone, which a loop's cron is read in. */
export function userTimeZone(): string | undefined {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || undefined;
  } catch {
    return undefined;
  }
}

/**
 * The draft as it is saved: a job that starts fresh threads and names no account runs on `account`,
 * the one a new thread on `harness` would get, with the harness it belongs to. Without it every run
 * started on the harness's bare default login, which may be signed out while the user's threads run
 * on a named account. A job that sends runs to one thread keeps that thread's own account.
 */
export function withRunAccount(
  draft: LoopDraft,
  harness: string | undefined,
  account: string | null,
): LoopDraft {
  // A hub Loop's run starts on its member, whose logins this computer does not know.
  if (draft.machine || draft.threadId || draft.account?.trim() || !account || !harness) return draft;
  return { ...draft, account, harness: draft.harness?.trim() || harness };
}

/** The save call's parameters: a cron in the user's own zone, empty fields left out. */
export function saveParams(draft: LoopDraft, timezone = draft.timezone ?? userTimeZone()): JsonObject {
  const params: JsonObject = {
    name: draft.name.trim(),
    workspace: draft.workspace,
    prompt: draft.prompt.trim(),
    schedule: draft.manual
      ? { kind: "manual" }
      : draft.once
        ? { kind: "at", at: draft.once.toISOString() }
        : {
            kind: "cron",
            expression: draft.cron.trim().split(/\s+/).join(" "),
            ...(timezone ? { timezone } : {}),
          },
    readOnly: draft.readOnly,
  };
  if (draft.id) params.id = draft.id;
  if (draft.webhook) params.webhook = true;
  if (draft.webhook && draft.rotateWebhook) params.rotateWebhook = true;
  if (draft.webhook) params.webhookEvents = webhookEventList(draft.webhookEvents);
  const machine = draft.machine?.trim();
  if (machine) params.machine = machine;
  // A script runs on this computer, so a Loop for another machine takes none.
  const script = machine ? undefined : draft.script?.trim();
  if (script) params.script = script;
  for (const key of draft.threadId
    ? (["threadId"] as const)
    : (["harness", "account", "model", "effort"] as const)) {
    const value = draft[key]?.trim();
    if (value) params[key] = value;
  }
  return draft.workflow === undefined ? params : workflowParams(params, draft.workflow, draft.args);
}

/** The events a typed filter names, without blanks or repeats (issue 4386). */
export function webhookEventList(text: string | undefined): string[] {
  const events = (text ?? "").split(/[\s,]+/).filter(Boolean);
  return [...new Set(events)];
}

/** The webhook a save answered with, when it made a new secret. */
export function webhookOf(value: Json): LoopWebhook | null {
  const raw = obj(obj(value)?.webhook);
  const url = str(raw?.url);
  const secret = str(raw?.secret);
  if (!raw || !url || !secret) return null;
  return { url, public: bool(raw.public) ?? false, secret };
}

function loopRequest<T = Json>(
  host: string,
  method: string,
  params: JsonObject,
  timeoutMs: number,
): Promise<T> {
  if (!host) return request<T>(method, params, timeoutMs);
  const conn = connectionFor(host);
  if (!conn) throw new Error("That machine is not connected.");
  return conn.request<T>(method, params, timeoutMs);
}

/** Read one machine's jobs and scheduler state. */
export async function fetchLoopsOn(host = LOCAL_HOST): Promise<LoopsSnapshot> {
  return parseLoops(await loopRequest(host, Method.loopsList, {}, 20_000), host);
}

/** Save a loop on `host`; answers its saved id and any newly created webhook URL. */
export async function saveLoopResult(
  draft: LoopDraft,
  host = LOCAL_HOST,
): Promise<{ id?: string; webhook: LoopWebhook | null }> {
  const answer = await loopRequest(host, Method.loopsSave, saveParams(draft), 30_000);
  const id = str(obj(obj(answer)?.loop)?.id);
  return { id, webhook: webhookOf(answer) };
}

/** Save a loop; answers its webhook when the save made a new URL, which is shown only then. */
export async function saveLoop(draft: LoopDraft, host = LOCAL_HOST): Promise<LoopWebhook | null> {
  return (await saveLoopResult(draft, host)).webhook;
}

export async function setLoopPaused(id: string, paused: boolean, host = LOCAL_HOST): Promise<void> {
  await loopRequest(host, Method.loopsSetPaused, { id, paused }, 20_000);
}

export async function runLoopNow(id: string, host = LOCAL_HOST): Promise<void> {
  await loopRequest(host, Method.loopsRun, { id }, 20_000);
}

export async function cancelLoopRun(id: string, host = LOCAL_HOST): Promise<void> {
  await loopRequest(host, Method.loopsCancel, { id }, 20_000);
}

export async function deleteLoop(id: string, host = LOCAL_HOST): Promise<void> {
  await loopRequest(host, Method.loopsDelete, { id }, 20_000);
}

export async function startScheduler(host = LOCAL_HOST): Promise<void> {
  await loopRequest(host, Method.loopsStartDaemon, {}, 30_000);
}

// ---------------------------------------------------------------- loop threads in the sidebar

/**
 * Which job each thread a run started or ran in belongs to, by thread id: the sidebar's Job badge.
 * `runs`: the threads a run started fresh, which live in the Jobs view and not the sidebar.
 */
export const useLoopThreads = create<{
  byThread: Record<string, string>;
  runs: Record<string, true>;
  /** The job id (or `host/id` for a remote job) each run's thread belongs to, so a run thread can lead back to its job. */
  loopOf: Record<string, string>;
  /** The job the Jobs view opens on: shown expanded, once. */
  reveal: string | null;
}>(() => ({
  byThread: {},
  runs: {},
  loopOf: {},
  reveal: null,
}));

const loopThreadKey = (loop: Pick<Loop, "host">, id: string) => threadKey(loop.host ?? LOCAL_HOST, id);

/** Resolves a run's thread key across local, paired, or hub-dispatched targets. */
export type RunThreadResolver = (
  loop: Pick<Loop, "host">,
  run: Pick<LoopRun, "machine" | "threadId">,
) => string | undefined;

const defaultRunThread: RunThreadResolver = (loop, run) =>
  run.threadId && !run.machine ? loopThreadKey(loop, run.threadId) : undefined;

/**
 * The threads fresh runs started, by id. A job that sends every run to one thread leaves that
 * thread out: the user picked it, and it stays in the list.
 */
export function loopRunThreads(
  loops: readonly Loop[],
  resolveRunThread: RunThreadResolver = defaultRunThread,
): Record<string, true> {
  const runs: Record<string, true> = {};
  for (const loop of loops) {
    for (const run of loop.runs) {
      if (!run.threadId || run.threadId === loop.threadId) continue;
      const key = resolveRunThread(loop, run);
      if (key) runs[key] = true;
    }
  }
  return runs;
}

/**
 * Whether a thread is a job's run, which the thread list leaves to the Jobs view:
 * the engine marks every run thread it starts, and the jobs' run history names older ones.
 */
export function isJobRunThread(
  session: { id: string; scheduledJob?: string },
  runs: Record<string, true>,
): boolean {
  return !!session.scheduledJob || !!runs[session.id];
}

/** Every thread the loops' recorded runs went to, and the loop's name. */
export function loopThreadNames(
  loops: readonly Loop[],
  resolveRunThread: RunThreadResolver = defaultRunThread,
): Record<string, string> {
  const byThread: Record<string, string> = {};
  for (const loop of loops) {
    if (loop.threadId && !loop.machine) byThread[loopThreadKey(loop, loop.threadId)] = loop.name;
    for (const run of loop.runs) {
      const key = resolveRunThread(loop, run);
      if (key) byThread[key] = loop.name;
    }
  }
  return byThread;
}

/** Every thread the loops' recorded runs went to, and the loop's id (prefixed with host when remote). */
export function loopIdsByThread(
  loops: readonly Loop[],
  resolveRunThread: RunThreadResolver = defaultRunThread,
): Record<string, string> {
  const byThread: Record<string, string> = {};
  for (const loop of loops) {
    const id = loop.host ? `${loop.host}/${loop.id}` : loop.id;
    if (loop.threadId && !loop.machine) byThread[loopThreadKey(loop, loop.threadId)] = id;
    for (const run of loop.runs) {
      const key = resolveRunThread(loop, run);
      if (key) byThread[key] = id;
    }
  }
  return byThread;
}

/** Keep the sidebar's Loop badges in step with a list the page or a refresh read. */
export function rememberLoops(
  snapshot: LoopsSnapshot | readonly Loop[],
  resolveRunThread: RunThreadResolver = defaultRunThread,
): void {
  const loops = "loops" in snapshot ? snapshot.loops : snapshot;
  useLoopThreads.setState({
    byThread: loopThreadNames(loops, resolveRunThread),
    runs: loopRunThreads(loops, resolveRunThread),
    loopOf: loopIdsByThread(loops, resolveRunThread),
  });
}

/** Read the loops on this computer and every connected paired machine for the sidebar badges. */
export async function refreshLoopThreads(
  resolveRunThread?: RunThreadResolver,
  remoteHosts?: readonly string[],
): Promise<void> {
  const hosts = [LOCAL_HOST, ...(remoteHosts ?? useApp.getState().connectedHosts).filter(Boolean)];
  const settled = await Promise.allSettled(hosts.map((host) => fetchLoopsOn(host)));
  const loops: Loop[] = [];
  let anySucceeded = false;
  for (const result of settled) {
    if (result.status === "fulfilled") {
      anySucceeded = true;
      loops.push(...result.value.loops);
    }
  }
  if (anySucceeded) rememberLoops(loops, resolveRunThread);
}
