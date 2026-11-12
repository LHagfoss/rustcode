// A thread stopped by its subscription's limit carries on when the limit
// resets; one stopped by a provider that was out of capacity is tried again
// after a short backoff. Pure, so the store and its tests share one answer:
// whether a turn ended on a limit or an overload, when that wait ends, and
// whether it still holds. Claude Code says the limit in a reply of its own, not
// as a failed turn: `You've hit your 5-hour limit · resets 2:50pm (Europe/Oslo)`.

import type { UsageWindow } from "../acp/rustcode";
import { failureStatus } from "./turnError";

/** Why a thread waits before it goes on: a plan limit resetting, or a provider that was full. */
export type StopReason = "limit" | "overload";

export interface LimitWait {
  /** When the wait ends: the limit's reset, or the moment a retry is due (ms). */
  until: number;
  /** What the thread ran on when it stopped: another harness or login is not this stop. */
  harness?: string;
  account: string | null;
  /** How many times this stop has waited; a stop that keeps coming back stops being retried. */
  attempts: number;
  /** What stopped the turn; absent is a limit (a wait saved before overloads existed). */
  reason?: StopReason;
  /**
   * The links (a parent's delegations) the stopped turn answered: the stop settled them as failed,
   * and the turn that continues reopens them and answers again.
   */
  links?: string[];
}

/** What the thread is told when it continues. */
export const LIMIT_RESUME_TEXT =
  "The usage limit that stopped your last turn has reset. Continue where you left off.";
/** What it is told after a provider that was out of capacity answers again. */
export const OVERLOAD_RESUME_TEXT =
  "The provider was overloaded and stopped your last turn. Continue where you left off.";

/** The words a waiting thread is sent. */
export function resumeText(wait: LimitWait): string {
  return reasonOf(wait) === "overload" ? OVERLOAD_RESUME_TEXT : LIMIT_RESUME_TEXT;
}

/** A wait with no reason is a limit: overloads are newer than the waits already saved. */
export const reasonOf = (wait: LimitWait): StopReason => wait.reason ?? "limit";
/** The provider's clock and ours differ, so wait a little past the reset. */
export const LIMIT_GRACE_MS = 60_000;
/** A reset time just behind us was a minute that has not rolled over: try again soon. */
const JUST_PASSED_MS = 3 * 3_600_000;
const RETRY_SOON_MS = 5 * 60_000;
/** After this many waits for one stop, the thread stays stopped. */
export const LIMIT_MAX_ATTEMPTS = 3;
/** A provider overload is not the thread's fault, so it gets more tries than a limit. */
export const OVERLOAD_MAX_ATTEMPTS = 4;
/** Doubling backoff for an overloaded provider: 30 s, 1 m, 2 m, 4 m. */
export const OVERLOAD_FIRST_MS = 30_000;
export const OVERLOAD_MAX_MS = 4 * 60_000;
/** Up to this share of the backoff is added at random, so many threads do not wake together. */
export const OVERLOAD_JITTER = 0.25;
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
  const m =
    /^(?:([a-z]{3})[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?(?:,|\s+at)?\s+(?:(\d{4}),?\s+(?:at\s+)?)?)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$/i.exec(
      text.trim(),
    );
  if (!m) return null;
  const hour12 = Number(m[4]);
  const minute = m[5] ? Number(m[5]) : 0;
  const meridiem = m[6]?.toLowerCase();
  if (minute > 59 || hour12 > 23 || (!meridiem && !m[5])) return null;
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
    const dated = { year: m[3] ? Number(m[3]) : today.year, month, day: Number(m[2]), hour, minute };
    const at = wallClock(dated, timeZone);
    if (at > now) return at;
    return now - at < JUST_PASSED_MS
      ? now + RETRY_SOON_MS
      : wallClock({ ...dated, year: dated.year + 1 }, timeZone);
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
  const lines = limitLines(text);
  if (!lines) return null;
  // A message of several paragraphs says one thing when each paragraph says the same limit.
  let reset: number | null = null;
  for (const line of lines) {
    const at = lineReset(line, now, windows);
    if (at === null) return null;
    reset = reset === null ? at : Math.max(reset, at);
  }
  return reset;
}

// What a harness wraps a limit in: `Internal error: <the limit>: {"errorKind":"rate_limit"}`, or its name.
const ERROR_PREFIX =
  /^(?:internal error|error|claude code|codex|gemini(?: cli)?|opencode|pi|rustcode)\s*:\s*/i;
const JSON_TAIL = /\s*:?\s*\{[^{}\n]*\}$/;

/**
 * The lines of a message that is only a usage limit, said once or more: each line with a
 * harness or `Internal error:` prefix and the JSON tail of an error taken off. Null for anything with a
 * line that says something else, so a reply that talks about limits never matches.
 */
function limitLines(text: string | undefined): string[] | null {
  const said = text?.trim();
  if (!said || said.length > 2_000) return null;
  const lines = said
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (!lines.length || lines.length > 6) return null;
  return lines.map((line) => {
    // Only a bare limit survives this: any other words left on the line fail its pattern.
    return line.replace(ERROR_PREFIX, "").replace(ERROR_PREFIX, "").replace(JSON_TAIL, "").trim();
  });
}

// Codex: `You've hit your usage limit. Upgrade to Pro (…) to purchase more credits or try again at
// 3:47 PM.`, or `… Try again in 3 days 2 hours 5 minutes.` The time is the machine's own clock.
const SAYS_CODEX_LIMIT =
  /^you(?:'|’)?ve hit your usage limit\b.{0,240}?\btry again (at|in) (.{1,60}?)\s*\.?$/i;
// Gemini CLI: `You have exhausted your capacity on this model. Your quota will reset after 3h42m18s.`
const SAYS_GEMINI_LIMIT =
  /^(?:you have exhausted your capacity on this model\.?\s*)?your quota will reset after (.{1,60}?)\s*\.?$/i;
const DURATION_UNIT_MS: Record<string, number> = {
  d: 86_400_000,
  h: 3_600_000,
  m: 60_000,
  s: 1_000,
};

/** `3h42m18s`, `54s`, `3 days 2 hours 5 minutes`, as milliseconds; null for anything else. */
function parseDuration(text: string): number | null {
  const unit = /(\d+(?:\.\d+)?)\s*(days?|d|hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)(?![a-z])/gi;
  let total = 0;
  const rest = text.replace(unit, (_, amount: string, name: string) => {
    total += Number(amount) * DURATION_UNIT_MS[name[0]!.toLowerCase()]!;
    return "";
  });
  return total > 0 && /^[\s,]*(?:and[\s,]*)*$/i.test(rest) ? Math.round(total) : null;
}

function lineReset(said: string, now: number, windows: readonly UsageWindow[]): number | null {
  if (said.length > 300) return null;
  const codex = SAYS_CODEX_LIMIT.exec(said);
  if (codex) {
    if (codex[1]!.toLowerCase() === "at") return parseReset(codex[2]!, now);
    const wait = parseDuration(codex[2]!);
    return wait === null ? null : now + wait;
  }
  const gemini = SAYS_GEMINI_LIMIT.exec(said);
  if (gemini) {
    const wait = parseDuration(gemini[1]!);
    return wait === null ? null : now + wait;
  }
  if (said.length > 200) return null;
  const stated = SAYS_RESET.exec(said);
  if (stated) return parseReset(stated[1]!, now, stated[2]);
  if (!SAYS_ONLY_LIMIT.test(said)) return null;
  const full = windows
    .filter((w) => w.usedPercent >= 100 && w.resetsAt !== undefined && w.resetsAt > now)
    .map((w) => w.resetsAt!);
  return full.length ? Math.max(...full) : null;
}

/** How many waits a stop of this kind gets before the thread stays stopped. */
export function waitMaxAttempts(wait: Pick<LimitWait, "reason">): number {
  return (wait.reason ?? "limit") === "overload" ? OVERLOAD_MAX_ATTEMPTS : LIMIT_MAX_ATTEMPTS;
}

/**
 * The next wait for a stop, or null once this stop has waited often enough. A
 * stop that changes kind (a limit, then an overload) starts its own count.
 */
export function nextWait(
  previous: LimitWait | undefined,
  next: Omit<LimitWait, "attempts">,
): LimitWait | null {
  const prior = previous && (previous.reason ?? "limit") === (next.reason ?? "limit") ? previous : undefined;
  const attempts = (prior?.attempts ?? 0) + 1;
  return attempts > waitMaxAttempts(next) ? null : { ...next, attempts };
}

/**
 * How long to wait before attempt number `attempts` (1 is the first): the backoff
 * doubles from {@link OVERLOAD_FIRST_MS} to {@link OVERLOAD_MAX_MS}, and up to
 * {@link OVERLOAD_JITTER} of it is added at random so an account's threads spread out.
 */
export function overloadDelay(attempts: number, random: () => number = Math.random): number {
  const doubling = Math.min(OVERLOAD_FIRST_MS * 2 ** Math.max(attempts - 1, 0), OVERLOAD_MAX_MS);
  const roll = Math.min(Math.max(random(), 0), 1);
  return doubling + Math.round(doubling * OVERLOAD_JITTER * roll);
}

// A provider saying it is full right now: the same turn, a little later, is worth it.
const SAYS_OVERLOADED =
  /\boverloaded(?:_error)?\b|\bover capacity\b|\b(?:servers?|service|provider|model)\b[^.\n]{0,24}\b(?:at capacity|is busy|are busy)\b/i;
// A spend or credit limit is not capacity: retrying only spends more, so it never does.
const SAYS_SPEND_LIMIT =
  /\b(?:credit balance|credits?|billing|payment|spend(?:ing)? limit|insufficient_quota|insufficient quota|exceeded your current quota|balance is too low|out of credits|quota exceeded)\b/i;
// `529`/`503` where a status is written, and never a number inside a count.
const BARE_OVERLOAD_STATUS =
  /\b(?:http|status|code|error)\b\s*:?\s*(?:529|503)\b|\b(?:529|503)\s+(?:service unavailable|overloaded|over capacity)\b|^\s*(?:529|503)\b/i;

/**
 * Whether a failed turn is worth the same prompt again because the provider was
 * overloaded (HTTP 529/503, `overloaded_error`, `over capacity`). A credit or
 * spend limit never is, whatever else its text says.
 */
export function overloadStop(text: string | undefined): boolean {
  const said = text?.trim();
  if (!said || said.length > 2_000) return false;
  if (SAYS_SPEND_LIMIT.test(said)) return false;
  const status = failureStatus(said);
  if (status === 529 || status === 503) return true;
  if (BARE_OVERLOAD_STATUS.test(said)) return true;
  return SAYS_OVERLOADED.test(said);
}

/** Whether the thread still runs on what stopped it; on anything else it is not this stop. */
export function waitHolds(wait: LimitWait, now: { harness?: string; account: string | null }): boolean {
  return wait.harness === now.harness && wait.account === now.account;
}

/** Whether the wait is over: a limit a little past its reset, an overload the moment it is due. */
export function waitDue(wait: LimitWait, now: number): boolean {
  return now >= wait.until + (reasonOf(wait) === "overload" ? 0 : LIMIT_GRACE_MS);
}
