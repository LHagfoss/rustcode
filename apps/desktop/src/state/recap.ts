/**
 * The chat recap: when the user comes back to a chat they have not looked at for a
 * while (Settings › General, an hour by default), a card after its last turn
 * says what they asked last, where the reply left off and whether the thread is
 * still working, waits on them or failed. It is per chat, like Claude Code's
 * recap, and built from the transcript, so it costs no model turn. Pure, so the
 * card, its setting and its tests agree on when a recap is due and what it says.
 */

import type { TranscriptItem } from "./thread";

/** The default time a chat goes unseen before a return earns a recap. */
export const RECAP_MINUTES_DEFAULT = 60;

/** The times the setting offers, in minutes. */
export const RECAP_MINUTE_OPTIONS = [15, 60, 240, 480] as const;

const MINUTE = 60_000;

/** How many chats keep a last-seen time; the oldest go first. */
export const SEEN_LIMIT = 300;

/** A stored recap threshold, kept to one of {@link RECAP_MINUTE_OPTIONS}. */
export function parseRecapMinutes(value: string | null): number {
  const n = value === null ? Number.NaN : Number.parseInt(value, 10);
  return (RECAP_MINUTE_OPTIONS as readonly number[]).includes(n) ? n : RECAP_MINUTES_DEFAULT;
}

/** The setting's label for a threshold: `15m`, `1h`, `4h`. */
export function recapMinutesLabel(minutes: number): string {
  return minutes < 60 ? `${minutes}m` : `${minutes / 60}h`;
}

/**
 * Whether the chat went unseen long enough to earn a recap. A chat never seen
 * before has no last time and never earns one: there is nothing to come back to.
 */
export function recapDue(seenAt: number | undefined, now: number, minutes: number): boolean {
  if (!seenAt) return false;
  return now - seenAt >= minutes * MINUTE;
}

/**
 * When a chat went quiet, for a return to it: when the user last had it on screen,
 * or, for one they never opened here, its last activity. Either earns a recap
 * through {@link recapDue}, so every chat left long enough gets one.
 */
export function quietSince(seenAt: number | undefined, lastActive: number | undefined): number | undefined {
  return seenAt ?? lastActive;
}

/** An open recap: when its chat went quiet, and how many messages it had once loaded. */
export interface OpenRecap {
  since: number;
  messages?: number;
}

/** The messages a chat holds: what the user sent and the replies, not tool calls or notices. */
export function messageCount(items: readonly TranscriptItem[]): number {
  let n = 0;
  for (const item of items) if (item.kind === "assistant" || typed(item)) n++;
  return n;
}

/**
 * The recap once its chat has `count` messages (undefined while it loads): it takes
 * the first count it sees, and is gone (null) when a message arrives after it.
 */
export function recapAfterMessages(recap: OpenRecap, count: number | undefined): OpenRecap | null {
  if (count === undefined) return recap;
  if (recap.messages === undefined) return { ...recap, messages: count };
  return count > recap.messages ? null : recap;
}

/** The last-seen map with only its {@link SEEN_LIMIT} most recent chats. */
export function pruneSeen(seen: Readonly<Record<string, number>>): Record<string, number> {
  const entries = Object.entries(seen).filter(([, at]) => Number.isFinite(at) && at > 0);
  if (entries.length <= SEEN_LIMIT) return Object.fromEntries(entries);
  return Object.fromEntries(entries.sort((a, b) => b[1] - a[1]).slice(0, SEEN_LIMIT));
}

/** Where the chat stands. */
export type RecapState = "running" | "needs" | "failed" | "done";

/** What the card says. */
export interface ChatRecap {
  /** The user's last message, shortened. */
  asked?: string;
  /** The opening of the last reply to it, shortened. */
  reply?: string;
  state: RecapState;
}

/** What {@link chatRecapOf} reads from a thread. */
export interface ChatRecapRead {
  items: readonly TranscriptItem[];
  /** Requests waiting on the user. */
  pending: number;
  running: boolean;
}

/** Markdown and whitespace folded to one plain line. */
export function plainLine(text: string): string {
  return text
    .replace(/```[\s\S]*?(```|$)/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+[.)])\s+/gm, "")
    .replace(/(\*\*|__|~~)/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** A line cut to about `max` characters, at a sentence end or a word where it can. */
export function shorten(text: string, max: number): string {
  if (text.length <= max) return text;
  const head = text.slice(0, max);
  const sentence = Math.max(head.lastIndexOf(". "), head.lastIndexOf("! "), head.lastIndexOf("? "));
  if (sentence >= max * 0.5) return head.slice(0, sentence + 1);
  const word = head.lastIndexOf(" ");
  return `${(word >= max * 0.5 ? head.slice(0, word) : head).replace(/[\s,;:.]+$/, "")}…`;
}

function typed(item: TranscriptItem): item is Extract<TranscriptItem, { kind: "user" }> {
  return item.kind === "user" && !item.wake && !item.continued && !item.steering && item.text.trim() !== "";
}

/**
 * The card's content, or null for a chat with nothing to recap yet. The reply is
 * the last one after the user's last message, so an older answer is never shown
 * as the answer to a newer question.
 */
export function chatRecapOf(read: ChatRecapRead): ChatRecap | null {
  const { items } = read;
  let askedAt = -1;
  for (let i = items.length - 1; i >= 0; i--) {
    if (typed(items[i]!)) {
      askedAt = i;
      break;
    }
  }
  let reply: string | undefined;
  let failed = false;
  for (let i = items.length - 1; i > askedAt && !reply; i--) {
    const item = items[i]!;
    if (item.kind === "assistant" && item.text.trim()) reply = plainLine(item.text);
    else if (item.kind === "notice" && item.tone === "error") failed = true;
  }
  const askedItem = askedAt >= 0 ? items[askedAt] : undefined;
  const asked = askedItem?.kind === "user" ? plainLine(askedItem.text) : undefined;
  if (!asked && !reply) return null;
  const state: RecapState =
    read.pending > 0 ? "needs" : read.running ? "running" : failed ? "failed" : "done";
  return {
    asked: asked ? shorten(asked, 160) : undefined,
    reply: reply ? shorten(reply, 240) : undefined,
    state,
  };
}
