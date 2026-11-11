// Messages written now and sent later: the presets the composer's + menu offers, the card that
// counts down over the composer, and the book that keeps them across a restart (state/store.ts).
// A scheduled message is not a queued one: it is not the thread's next turn, it is the user's
// message with a time on it, so it is kept apart from `state/queues.ts` until it is due.
//
// No React and no I/O here, so the store wiring and the tests share it.

import type { ComposeExtras } from "./compose";

/** Where the book lives in app storage. */
export const SCHEDULED_KEY = "rustcode.scheduled";
/** A message over this is not kept: a paste that big is not one anyone schedules to read. */
export const MAX_SCHEDULED_TEXT = 100 * 1024;
/** A message's command, links and attachments are kept up to this size in all: an image rides inline. */
export const MAX_SCHEDULED_EXTRAS = 64 * 1024;
/** At most this many scheduled messages per thread; the ones due last go first. */
export const MAX_SCHEDULED_ROWS = 20;
/** At most this many threads keep a scheduled message; the oldest go first. */
export const MAX_SCHEDULED_THREADS = 100;

/** The one-tap times the popover offers, after the issue's own list. */
export type SendLaterPreset = "30m" | "1h" | "2h" | "evening" | "morning";

export interface SendLaterPresetRow {
  id: SendLaterPreset;
  label: string;
}

export const SEND_LATER_PRESETS: readonly SendLaterPresetRow[] = [
  { id: "30m", label: "In 30 minutes" },
  { id: "1h", label: "In 1 hour" },
  { id: "2h", label: "In 2 hours" },
  { id: "evening", label: "This evening" },
  { id: "morning", label: "Tomorrow morning" },
];

/** A local wall-clock time, `days` from the day `now` is in: the presets are the user's clock. */
function atDay(now: number, days: number, hour: number, minute = 0): number {
  const d = new Date(now);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + days, hour, minute, 0, 0).getTime();
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/**
 * When a preset sends. The first three are offsets from now; "this evening" is 18:00 today, or
 * tomorrow once that has passed; "tomorrow morning" is 09:00 the next day.
 */
export function presetTime(preset: SendLaterPreset, now = Date.now()): number {
  switch (preset) {
    case "30m":
      return now + 30 * MINUTE;
    case "1h":
      return now + HOUR;
    case "2h":
      return now + 2 * HOUR;
    case "evening": {
      const evening = atDay(now, 0, 18);
      return evening > now ? evening : atDay(now, 1, 18);
    }
    case "morning":
      return atDay(now, 1, 9);
  }
}

/** "in 29 min", "in 1 h 5 min", "in 3 d": the card's live countdown, rounded up to what is left. */
export function countdown(due: number, now = Date.now()): string {
  const left = due - now;
  if (left <= 0) return "now";
  const minutes = Math.ceil(left / MINUTE);
  if (minutes < 60) return `in ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const restMinutes = minutes % 60;
  if (hours < 24) return restMinutes ? `in ${hours} h ${restMinutes} min` : `in ${hours} h`;
  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours ? `in ${days} d ${restHours} h` : `in ${days} d`;
}

const startOfDay = (at: number) => {
  const d = new Date(at);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
};

/** The time a card says in full: "today at 6:00 PM", "tomorrow at 9:00 AM", else a dated one. */
export function whenText(due: number, now = Date.now()): string {
  const at = new Date(due);
  const time = at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const days = Math.round((startOfDay(due) - startOfDay(now)) / 86_400_000);
  if (days === 0) return `today at ${time}`;
  if (days === 1) return `tomorrow at ${time}`;
  return `${at.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" })}, ${time}`;
}

/** What the card shows of the message: its first line, or what rides above an empty one. */
export function scheduledSummary(text: string, extras?: ComposeExtras): string {
  const line = text.trim().split("\n")[0] ?? "";
  if (line) return line.length > 140 ? `${line.slice(0, 140)}…` : line;
  const files = extras?.attachments.length ?? 0;
  if (files === 1) return "1 attachment";
  if (files > 1) return `${files} attachments`;
  return "A message";
}

/** One message waiting for its time, as it is kept. */
export interface ScheduledMessage {
  id: string;
  text: string;
  /** When it goes out, epoch ms. */
  due: number;
  /** What went with the text (a command, link chips, pictures, mentions). */
  extras?: ComposeExtras;
}

/** Scheduled messages per thread id, each list in the order it will send. */
export type ScheduleBook = Record<string, ScheduledMessage[]>;

export const emptySchedule = (): ScheduleBook => ({});

const isMessage = (value: unknown): value is ScheduledMessage => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const row = value as Partial<ScheduledMessage>;
  return (
    typeof row.id === "string" &&
    !!row.id &&
    typeof row.text === "string" &&
    row.text.length > 0 &&
    row.text.length <= MAX_SCHEDULED_TEXT &&
    typeof row.due === "number" &&
    Number.isFinite(row.due) &&
    row.due > 0
  );
};

const extrasOf = (value: unknown): ComposeExtras | undefined => {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const extras = value as ComposeExtras;
  if (!Array.isArray(extras.links) || !Array.isArray(extras.attachments)) return undefined;
  try {
    return JSON.stringify(extras).length <= MAX_SCHEDULED_EXTRAS ? extras : undefined;
  } catch {
    return undefined;
  }
};

/** A parsed book: readable messages only, per thread, each list by the time it sends, bounded. */
export function parseScheduleBook(raw: string | null): ScheduleBook {
  if (!raw) return emptySchedule();
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    // A book that is not JSON (an older build, a hand edit) starts empty.
    return emptySchedule();
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return emptySchedule();
  const book: ScheduleBook = {};
  for (const [id, list] of Object.entries(value as Record<string, unknown>)) {
    if (!id || !Array.isArray(list)) continue;
    const rows = list.filter(isMessage).map((row) => {
      const extras = extrasOf(row.extras);
      return extras ? { ...row, extras } : { id: row.id, text: row.text, due: row.due };
    });
    if (rows.length) book[id] = rows.sort((a, b) => a.due - b.due);
  }
  return persistableSchedule(book);
}

/** The book as it goes to storage: bounded rows and threads, and nothing unreadable. */
export function persistableSchedule(book: ScheduleBook): ScheduleBook {
  const entries = Object.entries(book)
    .map(([id, rows]) => [id, rows.slice(-MAX_SCHEDULED_ROWS)] as const)
    .filter(([, rows]) => rows.length)
    .slice(-MAX_SCHEDULED_THREADS);
  return Object.fromEntries(entries);
}

/**
 * The book with one message added for a thread. Null when the message itself cannot be kept (empty
 * or over the size a web view should hold), so nothing is scheduled that a restart would lose.
 */
export function addScheduled(
  book: ScheduleBook,
  threadId: string,
  item: ScheduledMessage,
): ScheduleBook | null {
  if (!threadId || !item.text.length || item.text.length > MAX_SCHEDULED_TEXT) return null;
  if (!Number.isFinite(item.due)) return null;
  const extras = extrasOf(item.extras);
  const row: ScheduledMessage = extras
    ? { id: item.id, text: item.text, due: item.due, extras }
    : { id: item.id, text: item.text, due: item.due };
  const rows = [...(book[threadId] ?? []), row].sort((a, b) => a.due - b.due);
  const { [threadId]: _was, ...rest } = book;
  return { ...rest, [threadId]: rows.slice(-MAX_SCHEDULED_ROWS) };
}

/** The book without one message. The same book comes back when the id was not there. */
export function withoutScheduled(book: ScheduleBook, threadId: string, id: string): ScheduleBook {
  const rows = book[threadId];
  const kept = rows?.filter((row) => row.id !== id);
  if (!rows || kept?.length === rows.length) return book;
  const { [threadId]: _was, ...rest } = book;
  return kept?.length ? { ...rest, [threadId]: kept } : rest;
}

/** Every message whose time has come, the soonest first, with the thread it belongs to. */
export function dueScheduled(
  book: ScheduleBook,
  now = Date.now(),
): { threadId: string; item: ScheduledMessage }[] {
  return Object.entries(book)
    .flatMap(([threadId, rows]) => rows.filter((row) => row.due <= now).map((item) => ({ threadId, item })))
    .sort((a, b) => a.item.due - b.item.due);
}

/** How many messages a book holds: what the store's timer watches to know when to run again. */
export function scheduledCount(book: ScheduleBook): number {
  return Object.values(book).reduce((count, rows) => count + rows.length, 0);
}
