// A thread stopped by its subscription's limit carries on when the limit
// resets. Pure, so the store and its tests share one answer: whether a turn
// ended on a limit, when that limit resets, and whether the wait still holds.
// Claude Code says it in a reply of its own, not as a failed turn:
// `You've hit your 5-hour limit · resets 2:50pm (Europe/Oslo)`.

import type { UsageWindow } from "../acp/rustcode";

export interface LimitWait {
  /** When the limit resets (ms). */
  until: number;
  /** What the thread ran on when it stopped: another harness or login is not this limit. */
  harness?: string;
  account: string | null;
  /** How many times this stop has waited; a limit that keeps coming back stops being retried. */
  attempts: number;
}

/** What the thread is told when it continues. */
export const LIMIT_RESUME_TEXT =
  "The usage limit that stopped your last turn has reset. Continue where you left off.";
/** The provider's clock and ours differ, so wait a little past the reset. */
export const LIMIT_GRACE_MS = 60_000;
/** A reset time just behind us was a minute that has not rolled over: try again soon. */
const JUST_PASSED_MS = 3 * 3_600_000;
const RETRY_SOON_MS = 5 * 60_000;
/** After this many waits for one stop, the thread stays stopped. */
export const LIMIT_MAX_ATTEMPTS = 3;
const DAY_MS = 86_400_000;

const MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

// The whole message and nothing else, so a reply that talks about limits never matches.
const SAYS_RESET =
  /^(?:you(?:'|’)?ve hit your [^·\n]{0,40}limit\s*[·:–-]|(?:claude )?usage limit reached[.·:]?\s*(?:your limit will)?)\s*resets?\s+(?:at\s+|on\s+)?([^()\n]{1,40}?)\s*(?:\(([^()\n]{1,60})\))?\s*\.?$/i;
// A limit with no time in the message: the plan's windows have to say.
const SAYS_ONLY_LIMIT =
  /^(?:[\w ]{1,24}:\s*)?(?:you(?:'|’)?ve hit your [^·\n]{0,40}limit|usage limit reached)[.!]?$/i;

function zoneParts(at: number, timeZone?: string) {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hourCycle: "h23",
    year: "numeric",
    month: "numeric",
    day: "numeric",
    hour: "numeric",
    minute: "numeric",
    second: "numeric",
  }).formatToParts(at);
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value ?? 0);
  return {
    year: get("year"),
    month: get("month"),
    day: get("day"),
    hour: get("hour"),
    minute: get("minute"),
    second: get("second"),
  };
}

/** The moment a wall clock in `timeZone` reads this. */
function wallClock(
  wall: { year: number; month: number; day: number; hour: number; minute: number },
  timeZone?: string,
): number {
  const guess = Date.UTC(wall.year, wall.month - 1, wall.day, wall.hour, wall.minute);
  const seen = zoneParts(guess, timeZone);
  const shown = Date.UTC(seen.year, seen.month - 1, seen.day, seen.hour, seen.minute);
  return guess - (shown - guess);
}

/** `2:50pm`, `3pm`, `14:50`, with `Nov 8, ` or `Nov 8 at ` before it. Null for anything else. */
function parseReset(text: string, now: number, zone?: string): number | null {
  const m = /^(?:([a-z]{3})[a-z]*\.?\s+(\d{1,2})(?:,|\s+at)?\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/i.exec(
    text.trim(),
  );
  if (!m) return null;
  const hour12 = Number(m[3]);
  const minute = m[4] ? Number(m[4]) : 0;
  const meridiem = m[5]?.toLowerCase();
  if (minute > 59 || hour12 > 23 || (!meridiem && !m[4])) return null;
  if (meridiem && (hour12 < 1 || hour12 > 12)) return null;
  const hour = meridiem ? (hour12 % 12) + (meridiem === "pm" ? 12 : 0) : hour12;
  let timeZone = zone;
  let today: ReturnType<typeof zoneParts>;
  try {
    today = zoneParts(now, timeZone);
  } catch {
    // A zone this machine does not know: read it as local time.
    timeZone = undefined;
    today = zoneParts(now);
  }
  if (m[1]) {
    const month = MONTHS.indexOf(m[1].toLowerCase()) + 1;
    if (!month) return null;
    const dated = { year: today.year, month, day: Number(m[2]), hour, minute };
    const at = wallClock(dated, timeZone);
    if (at > now) return at;
    return now - at < JUST_PASSED_MS
      ? now + RETRY_SOON_MS
      : wallClock({ ...dated, year: today.year + 1 }, timeZone);
  }
  const at = wallClock({ year: today.year, month: today.month, day: today.day, hour, minute }, timeZone);
  if (at > now) return at;
  return now - at < JUST_PASSED_MS ? now + RETRY_SOON_MS : at + DAY_MS;
}

/**
 * When a limit a turn ended on resets; null when the text is not a limit, or
 * says nothing and no window of the plan is full. The latest full window wins:
 * a 5-hour reset is no use while the week's limit is full too.
 */
export function limitReset(
  text: string | undefined,
  now: number,
  windows: readonly UsageWindow[] = [],
): number | null {
  const said = text?.trim();
  if (!said || said.length > 200) return null;
  const stated = SAYS_RESET.exec(said);
  if (stated) return parseReset(stated[1]!, now, stated[2]);
  if (!SAYS_ONLY_LIMIT.test(said)) return null;
  const full = windows
    .filter((w) => w.usedPercent >= 100 && w.resetsAt !== undefined && w.resetsAt > now)
    .map((w) => w.resetsAt!);
  return full.length ? Math.max(...full) : null;
}

/** The next wait for a stop, or null once this stop has waited often enough. */
export function nextWait(
  previous: LimitWait | undefined,
  next: Omit<LimitWait, "attempts">,
): LimitWait | null {
  const attempts = (previous?.attempts ?? 0) + 1;
  return attempts > LIMIT_MAX_ATTEMPTS ? null : { ...next, attempts };
}

/** Whether the thread still runs on what hit the limit; on anything else it is not this limit. */
export function waitHolds(wait: LimitWait, now: { harness?: string; account: string | null }): boolean {
  return wait.harness === now.harness && wait.account === now.account;
}

/** Whether the wait is over. */
export function waitDue(wait: LimitWait, now: number): boolean {
  return now >= wait.until + LIMIT_GRACE_MS;
}
