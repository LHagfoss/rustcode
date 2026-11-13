// A suggested next prompt (issue 3606): when a completed reply ends by offering something ("Want
// me to add tests?", "Next step: run the migration."), an empty composer shows the accepting reply
// as ghost text. It is read off the reply here, on this computer: no model call, no quota, and a
// reply that offers nothing suggests nothing. Tab puts it in the field to edit; it is never sent.

import type { ThreadState, TranscriptItem } from "../state/thread";

const KEY = "rustcode.nextPrompt";

/** Off until the user turns it on (Settings › Chat). */
export const NEXT_PROMPT_DEFAULT = false;

export function nextPromptEnabled(): boolean {
  try {
    const raw = localStorage.getItem(KEY);
    return raw === null ? NEXT_PROMPT_DEFAULT : raw === "1";
  } catch {
    return NEXT_PROMPT_DEFAULT;
  }
}

export function setNextPrompt(on: boolean) {
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    // Not persisted in a private window.
  }
}

/** A suggestion longer than this is a paragraph, not a next prompt. */
const MAX_LENGTH = 160;

// Offers that end in a question: what follows the opener is the action, accepted with "Yes, …".
const OFFERS: RegExp[] = [
  /^(?:do you )?want me to (.+)\?$/i,
  /^would you like me to (.+)\?$/i,
  /^shall i (.+)\?$/i,
  /^should i (.+)\?$/i,
  /^can i (.+)\?$/i,
  /^may i (.+)\?$/i,
];

// Offers that end in a statement: "Let me know if you'd like me to …".
const STATEMENT_OFFERS: RegExp[] = [
  /^let me know if you(?:'d| would) like me to (.+?)[.!]?$/i,
  /^let me know if you want me to (.+?)[.!]?$/i,
  /^if you(?:'d| would) like,? i can (.+?)[.!]?$/i,
  /^if you want,? i can (.+?)[.!]?$/i,
  /^i can (?:also )?(.+?) if you(?:'d| would)? like[.!]?$/i,
  /^i can (?:also )?(.+?) if you want[.!]?$/i,
];

const NEXT_STEP = /^(?:next steps?|suggested next steps?)\s*[:\-–—]\s*(.*)$/i;
const LIST_ITEM = /^(?:[-*+]|\d+[.)])\s+(.+)$/;

/** The text as it reads, without the Markdown marks around words. */
function plain(text: string): string {
  return text
    .replace(/\*\*|__|`/g, "")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
}

/** The reply's lines outside code blocks, trimmed, empty lines kept as paragraph breaks. */
function proseLines(reply: string): string[] {
  const lines: string[] = [];
  let fenced = false;
  for (const raw of reply.split("\n")) {
    const line = raw.trim();
    if (/^(```|~~~)/.test(line)) {
      fenced = !fenced;
      lines.push("");
      continue;
    }
    lines.push(fenced ? "" : line);
  }
  return lines;
}

/** The last paragraph's sentences, last first: what the reply ends on. */
function closingSentences(lines: string[]): string[] {
  const end = lines.findLastIndex((l) => l !== "");
  if (end < 0) return [];
  let start = end;
  while (start > 0 && lines[start - 1] !== "") start--;
  const paragraph = plain(
    lines
      .slice(start, end + 1)
      .join(" ")
      .replace(/^#+\s*/, ""),
  );
  const sentences = paragraph.match(/[^.?!]+(?:[.?!]+|$)/g) ?? [];
  return sentences
    .map((s) => s.trim())
    .filter(Boolean)
    .reverse();
}

/** "update your config" → "update my config": the user is the one answering. */
function asTheUser(action: string): string {
  return action
    .replace(/\byours\b/gi, "mine")
    .replace(/\byour\b/gi, "my")
    .replace(/\byourself\b/gi, "myself");
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function finish(text: string): string | null {
  const clean = text.replace(/[\s,;:]+$/, "").trim();
  if (!clean || clean.length > MAX_LENGTH) return null;
  return /[.!?]$/.test(clean) ? clean : `${clean}.`;
}

/** The yes to an offered action, or nothing when the offer is a choice ("… or …"). */
function accept(action: string): string | null {
  const body = action.trim().replace(/^(?:go ahead and|now|also)\s+/i, "");
  if (!body || /\bor\b/i.test(body)) return null;
  return finish(`Yes, ${asTheUser(body)}`);
}

/** The step a "Next step:" line names, inline or as the first item of the list under it. */
function namedStep(lines: string[]): string | null {
  const tail = lines.slice(-12);
  for (let i = tail.length - 1; i >= 0; i--) {
    const match = NEXT_STEP.exec(plain(tail[i] ?? "").replace(/^#+\s*/, ""));
    if (!match) continue;
    const inline = match[1]?.trim();
    if (inline) return finish(capitalize(asTheUser(inline)));
    const item = tail.slice(i + 1).find((l) => l !== "");
    const step = item ? LIST_ITEM.exec(item)?.[1] : undefined;
    return step ? finish(capitalize(asTheUser(plain(step)))) : null;
  }
  return null;
}

/**
 * What the user would most likely answer to a reply, or `null` when the reply offers nothing to
 * accept. Only its closing sentences count: an offer in the middle of a long answer is not where
 * the conversation stands.
 */
export function suggestNextPrompt(reply: string): string | null {
  const lines = proseLines(reply);
  const closing = closingSentences(lines).slice(0, 2);
  for (const sentence of closing) {
    for (const offer of OFFERS) {
      const action = offer.exec(sentence)?.[1];
      if (action) return accept(action);
    }
    for (const offer of STATEMENT_OFFERS) {
      const action = offer.exec(sentence)?.[1];
      if (action) return accept(action);
    }
  }
  return namedStep(lines);
}

/** Rows that come after a reply without being part of the conversation. */
function isAside(item: TranscriptItem): boolean {
  return item.kind !== "user" && item.kind !== "assistant" && item.kind !== "turnEnd";
}

/**
 * The reply the thread's latest turn ended on, when that turn completed and nothing waits on the
 * user or the agent: no running turn, approval, queued message or error. A replayed thread's last
 * turn has no `turnEnd` row until the next message; its timing (`pendingTurn`) stands in for it.
 */
export function completedReply(thread: ThreadState): { id: string; text: string } | null {
  if (thread.running || thread.permissions.length || thread.queue.length || thread.error) return null;
  let ended = false;
  for (let i = thread.items.length - 1; i >= 0; i--) {
    const item = thread.items[i];
    if (!item) continue;
    if (item.kind === "turnEnd") {
      if (ended || item.outcome !== "completed") return null;
      ended = true;
      continue;
    }
    if (item.kind === "user") return null;
    if (item.kind === "assistant") {
      if (!ended) {
        const outcome = thread.pendingTurn?.outcome;
        if (!thread.pendingTurn || outcome === "cancelled" || outcome === "failed") return null;
      }
      return item.text.trim() ? { id: item.id, text: item.text } : null;
    }
    if (!isAside(item)) return null;
  }
  return null;
}

/** The suggestion for a thread, keyed by the reply it answers so a dismissal holds for that turn. */
export function nextPromptFor(thread: ThreadState | undefined): { key: string; text: string } | null {
  if (!thread) return null;
  const reply = completedReply(thread);
  if (!reply) return null;
  const text = suggestNextPrompt(reply.text);
  return text ? { key: `${thread.id}:${reply.id}`, text } : null;
}

/**
 * What a key does to a shown suggestion. An open completion menu keeps Tab and Escape; a modified
 * Tab belongs to shortcuts, and Shift+Tab to moving focus back.
 */
export function suggestionKeyAction(
  e: { key: string; shiftKey: boolean; altKey: boolean; ctrlKey: boolean; metaKey: boolean },
  menuOpen: boolean,
): "accept" | "dismiss" | null {
  if (menuOpen) return null;
  const modified = e.shiftKey || e.altKey || e.ctrlKey || e.metaKey;
  if (e.key === "Tab" && !modified) return "accept";
  if (e.key === "Escape" && !modified) return "dismiss";
  return null;
}

// Dismissed suggestions, by key: one Escape holds for that turn, across thread switches.
const dismissed = new Set<string>();

export function dismissNextPrompt(key: string) {
  dismissed.add(key);
}

export function nextPromptDismissed(key: string): boolean {
  return dismissed.has(key);
}
