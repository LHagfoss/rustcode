// A wake row's divider, read from the wake message's own lines. The engine keeps
// the text a turn was started with, not the app's links (state/thread.ts
// `isWakeMessage`), so a reloaded thread shows the same row: a delegate's title,
// harness and time are parsed back out of the message. Old rows ("1 child
// finished") read too, with the title they carry and no harness or time.

/** One delegate a wake message names, as its own line writes it. */
export interface WakeDelegate {
  linkId?: string;
  /** The link's state word: done, failed, needs_user. */
  state: string;
  title?: string;
  /** The harness's display name, as written (Pi, Codex, Claude Code). */
  harness?: string;
  /** How long it worked, already formatted (`3m 49s`). */
  worked?: string;
}

/** What a wake row's divider says. */
export interface WakeNotice {
  /** "Delegate finished", "2 delegates finished · 1 failed", "Delegate asks", … */
  label: string;
  /** The delegate's title, or the titles of several, joined. */
  titles?: string;
  /** The harness name of one delegate; several show their icons instead. */
  harness?: string;
  /** One delegate's worked time, or the longest of several. */
  worked?: string;
  /** A failed delegate is named as failed, never only tinted. */
  failed: boolean;
  /** The delegates that finished, in the message's order. */
  finished: WakeDelegate[];
  /** The delegates waiting on the user, in the message's order. */
  asking: WakeDelegate[];
}

const SEP = " · ";

/** How many milliseconds a formatted worked time is (`3m 49s`, `1h 2m`). */
function workedMs(worked: string): number {
  let ms = 0;
  for (const part of worked.matchAll(/(\d+)\s*([hms])/g)) {
    const unit = part[2] === "h" ? 3_600_000 : part[2] === "m" ? 60_000 : 1_000;
    ms += Number(part[1]) * unit;
  }
  return ms;
}

/** The longest worked time of several delegates. */
function longest(worked: string[]): string | undefined {
  let best: string | undefined;
  let bestMs = -1;
  for (const time of worked) {
    const ms = workedMs(time);
    if (ms > bestMs) {
      best = time;
      bestMs = ms;
    }
  }
  return best;
}

/** A delegate line's title, when it is quoted the way `linkLine` writes it. */
function titleOf(bits: string[]): string | undefined {
  const quoted = /^“(.+)”$/.exec(bits[2] ?? "");
  return quoted?.[1]?.trim() || undefined;
}

/** The harness and worked time from the rest of a delegate line. */
function detailOf(bits: string[]): { harness?: string; worked?: string } {
  let harness: string | undefined;
  let worked: string | undefined;
  for (const bit of bits.slice(3)) {
    if (bit.startsWith("session ") || bit.startsWith("worked ")) {
      if (bit.startsWith("worked ")) worked = bit.slice("worked ".length).trim() || undefined;
      continue;
    }
    if (/^\d+ files? changed$/.test(bit) || /^\d+s ago$/.test(bit)) continue;
    if (!harness) harness = bit;
  }
  return { harness, worked };
}

/** A line of the message's body as the delegate it names. */
function delegateOf(line: string): WakeDelegate | undefined {
  if (!line.startsWith("- link_id ")) return undefined;
  const bits = line.slice(2).split(SEP);
  const linkId = bits[0]?.replace(/^link_id\s+/, "");
  const state = bits[1] ?? "";
  if (!state) return undefined;
  return { linkId, state, title: titleOf(bits), ...detailOf(bits) };
}

/**
 * The divider a wake message makes: who finished (or failed), what each was
 * called, on which harness, and for how long. It reads only the text, so a
 * thread reloaded from history shows the row it showed live; a wake written by
 * an older build says "1 child finished" and keeps what its lines carry.
 *
 * Null for a message that is not a wake.
 */
export function wakeNotice(text: string): WakeNotice | null {
  const lines = text.split("\n");
  const head = (lines[0] ?? "").trim();
  if (!head) return null;

  const finished: WakeDelegate[] = [];
  const asking: WakeDelegate[] = [];
  for (const line of lines.slice(1)) {
    const delegate = delegateOf(line);
    if (!delegate) continue;
    (delegate.state === "needs_user" ? asking : finished).push(delegate);
  }

  // An asking part is the head's second segment ("… · Delegate asks: Title", "… · 2 delegates need you");
  // old rows append their own ("1 child finished · 1 needs you").
  const asksAt = head.indexOf(`${SEP}Delegate asks: `);
  const manyAsks = /^(.*) · (\d+) delegates need you$/.exec(head);
  const settledHead = asksAt >= 0 ? head.slice(0, asksAt) : (manyAsks?.[1] ?? head);
  const asksTitle = asksAt >= 0 ? head.slice(asksAt + `${SEP}Delegate asks: `.length).trim() : undefined;
  // An older build's first line ("1 child finished · 1 needs you"), whose count is all it says.
  const oldFinished = /^(\d+) child(?:ren)? finished/.exec(settledHead);
  const oldAsking = /(\d+) needs? you/.exec(head);
  // A newer line names one delegate in full; several only by count (their titles are on the lines).
  const one = /^Delegate (finished|failed): (.+)$/.exec(settledHead);
  const oneAsks = /^Delegate asks: (.+)$/.exec(settledHead);
  const counted = /^(\d+) delegates (finished|failed)/.exec(settledHead);
  const manyOnly = /^(\d+) delegates need you$/.exec(settledHead);
  const failedCount = /(\d+) failed/.exec(settledHead);
  if (!finished.length && !asking.length && !oldFinished && !one && !oneAsks && !counted && !manyOnly)
    return null;

  const count = finished.length || Number(oldFinished?.[1] ?? counted?.[1] ?? (one ? 1 : 0));
  const asks =
    asking.length ||
    Number(oldAsking?.[1] ?? manyAsks?.[2] ?? manyOnly?.[1] ?? (oneAsks || asksTitle ? 1 : 0));
  const askingTitle = asksTitle || oneAsks?.[1]?.trim();
  const fails = finished.filter((d) => d.state === "failed").length;
  const failures =
    fails ||
    Number(failedCount?.[1] ?? 0) ||
    (one?.[1] === "failed" || counted?.[2] === "failed" ? count : 0);
  const failed = failures > 0;
  const titlesOf = (list: WakeDelegate[]) =>
    list
      .map((d) => d.title)
      .filter((t): t is string => !!t)
      .join(", ");
  const titles =
    titlesOf(finished) ||
    (count > 0 ? one?.[2]?.trim() : undefined) ||
    titlesOf(asking) ||
    askingTitle ||
    undefined;

  let label: string;
  if (count === 1) label = failed ? "Delegate failed" : "Delegate finished";
  else if (count > 1) {
    label = `${count} delegates ${failures === count ? "failed" : "finished"}`;
    if (failures && failures < count) label += `${SEP}${failures} failed`;
  } else {
    label = asks === 1 ? "Delegate asks" : `${asks} delegates need you`;
  }
  if (count > 0 && asks > 0) {
    label += `${SEP}${asks === 1 ? "1 delegate asks" : `${asks} delegates need you`}`;
  }

  const harness =
    count === 1 ? finished[0]?.harness : count === 0 && asking.length === 1 ? asking[0]?.harness : undefined;
  const worked =
    count === 1 ? finished[0]?.worked : longest(finished.flatMap((d) => (d.worked ? [d.worked] : [])));

  return { label, titles, harness, worked, failed, finished, asking };
}

/** The id `HarnessIcon` needs from a harness name the message wrote: `Pi` → `pi`, `Claude Code` → `claude-code`. */
export function harnessId(name: string): string {
  return name.toLowerCase().replace(/\s+/g, "-");
}
