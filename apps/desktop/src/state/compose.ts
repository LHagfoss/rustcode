// What a message is made of before it is sent: the typed text, an optional
// slash-command token, pasted link chips, long pastes held above the field,
// images and files. A paste goes back into the text where it was pasted when
// the message is composed, so it is sent as plain text, not as a chip; and
// `promptBlocks` turns the rest into ACP content blocks. No React and no I/O,
// so tests drive it.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { SlashCommand } from "../acp/types";

/**
 * A message that starts with a command (`/review the diff`), as the name and what follows. A path
 * (`/usr/bin/env`) is not a command: the name stops at a space.
 */
export function splitSlashCommand(text: string): { command: string; rest: string } | null {
  const match = /^(\/[A-Za-z][\w:-]*)(?:\s+|$)/.exec(text);
  return match ? { command: match[1]!, rest: text.slice(match[0].length) } : null;
}

/** A message folds under this height (px)... */
export const COLLAPSED_MESSAGE_HEIGHT = 176;
/** ...only when folding hides more than this: three lines. Hiding one line behind a button helps nobody. */
const COLLAPSE_SLACK = 66;

/** Whether a message this tall folds. By how tall it draws, so a fold always opens to something. */
export const messageCollapses = (height: number) => height > COLLAPSED_MESSAGE_HEIGHT + COLLAPSE_SLACK;

/** The engine refuses larger images (`ImageAttachment.maxBytes` in the SwiftUI app). */
export const MAX_IMAGE_BYTES = 20 * 1024 * 1024;
/** A file without a path (the browser) is sent inline only up to this size. */
export const MAX_INLINE_FILE_BYTES = 200 * 1024;

export const IMAGE_TYPES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
};

export function imageMime(name: string): string | undefined {
  return IMAGE_TYPES[name.split(".").pop()?.toLowerCase() ?? ""];
}

export type Attachment =
  | { id: string; kind: "image"; name: string; mimeType: string; data: string }
  | { id: string; kind: "file"; name: string; path?: string; text?: string }
  /**
   * A long paste the field kept out of the draft (lib/paste.ts). `at` is the caret offset it was
   * pasted at, kept right while the draft changes (`trackPastes`), so the sent text can put it back
   * where the user wrote it.
   */
  | { id: string; kind: "text"; text: string; at: number };

export interface ComposeExtras {
  command?: SlashCommand;
  links: string[];
  attachments: Attachment[];
}

export const emptyExtras: ComposeExtras = { links: [], attachments: [] };

export function hasExtras(extras: ComposeExtras | undefined): boolean {
  return !!extras && (!!extras.command || extras.links.length > 0 || extras.attachments.length > 0);
}

/**
 * Where a paste sits after the draft changes: `before` and `after` are the draft around one edit,
 * and `at` the offset the paste took when it was made. The edit starts where the two texts stop
 * agreeing, so one that starts at or before the paste moves it by the edit's length change; one
 * after the paste leaves it alone. Clamped to the new draft, since the text it was pasted under may
 * have gone.
 */
export function shiftPasteOffset(at: number, before: string, after: string): number {
  const max = Math.min(before.length, after.length);
  let common = 0;
  while (common < max && before[common] === after[common]) common++;
  const next = common <= at ? at + (after.length - before.length) : at;
  return Math.max(0, Math.min(after.length, next));
}

/** The attachments with each paste's offset kept where it was in the draft that became `after`. */
export function trackPastes(
  attachments: readonly Attachment[],
  before: string,
  after: string,
): readonly Attachment[] {
  if (before === after) return attachments;
  let changed = false;
  const next = attachments.map((a) => {
    if (a.kind !== "text") return a;
    const at = shiftPasteOffset(a.at, before, after);
    if (at === a.at) return a;
    changed = true;
    return { ...a, at };
  });
  return changed ? next : attachments;
}

/** A paste spliced into the draft at `at`, with a newline where it would run into the text around it. */
function insertPaste(draft: string, at: number, pasted: string): { text: string; added: number } {
  const before = draft.slice(0, at);
  const after = draft.slice(at);
  const lead = before && !/\s$/.test(before) && !/^\s/.test(pasted) ? "\n" : "";
  const trail = after && !/\s$/.test(pasted) && !/^\s/.test(after) ? "\n" : "";
  return {
    text: `${before}${lead}${pasted}${trail}${after}`,
    added: lead.length + pasted.length + trail.length,
  };
}

/**
 * The typed text with every long paste put back where it was pasted, as plain text: the field keeps
 * a paste above it while the draft is written, and the message says it in the place it was pasted.
 * Pastes go in by offset (`at`), those at the same offset in the order they were made, and each one
 * gets a newline where it would otherwise run into the text beside it. The typed text keeps the
 * trim it had before pastes were a part of it.
 */
export function splicePastes(text: string, attachments: readonly Attachment[]): string {
  const start = text.length - text.trimStart().length;
  let body = text.slice(start).trimEnd();
  let added = 0;
  const pastes = attachments.flatMap((a, i) => (a.kind === "text" ? [{ paste: a, i }] : []));
  pastes.sort((x, y) => x.paste.at - y.paste.at || x.i - y.i);
  for (const { paste } of pastes) {
    const at = Math.max(0, Math.min(body.length, paste.at - start + added));
    const next = insertPaste(body, at, paste.text);
    body = next.text;
    added += next.added;
  }
  return body;
}

/** The text the prompt carries, in the order the composer shows it: command, links, then what was typed with its pastes in place. */
export function composedText(text: string, extras: ComposeExtras): string {
  const head = [extras.command?.name, ...extras.links].filter(Boolean).join(" ");
  const body = splicePastes(text, extras.attachments);
  if (!head) return body;
  return body ? `${head} ${body}` : head;
}

function relative(path: string, cwd: string): string {
  return cwd && path.startsWith(`${cwd.replace(/\/$/, "")}/`)
    ? path.slice(cwd.replace(/\/$/, "").length + 1)
    : path;
}

/** A message another thread delivered: the app writes it as `From thread “…” (agent): …`. */
const DELIVERED = /^From thread “/;

/**
 * What ↑ walks in the composer: the user's own messages in this thread, oldest first. A wake is the
 * app's own turn, a message another thread delivered is not the user's to recall, and one with no
 * text (attachments only) has nothing to put back in the field.
 */
export function sentMessages(items: readonly { kind: string; text?: string; wake?: boolean }[]): string[] {
  const out: string[] = [];
  for (const item of items) {
    if (item.kind !== "user" || item.wake) continue;
    const text = item.text;
    if (!text?.trim() || DELIVERED.test(text)) continue;
    out.push(text);
  }
  return out;
}

export interface BlockOptions {
  /** The thread's harness takes image blocks. */
  images: boolean;
  /** The built-in harness reads only text and images; others also take `resource_link` blocks. */
  builtin: boolean;
  cwd: string;
}

/**
 * ACP prompt blocks for a message. Images become `image` blocks; files with a
 * path become `resource_link` blocks on an external harness and an "Attached
 * files" list in the text on the built-in one (it ignores links, as the
 * SwiftUI app found); a file without a path is inlined as text. A long paste is
 * already in the text, at the offset it was pasted (`composedText`), so it
 * needs no block of its own.
 */
export function promptBlocks(text: string, extras: ComposeExtras, options: BlockOptions): Json[] {
  let body = composedText(text, extras);
  const files = extras.attachments.filter((a) => a.kind === "file");
  const withPath = files.filter((f) => f.path);
  const inline = files.filter((f) => !f.path && f.text !== undefined);
  if (withPath.length && options.builtin) {
    const list = withPath.map((f) => `- \`${relative(f.path!, options.cwd)}\``).join("\n");
    body = `${body ? `${body}\n\n` : ""}Attached files:\n${list}`;
  }
  for (const f of inline) {
    body = `${body ? `${body}\n\n` : ""}Attached file \`${f.name}\`:\n\`\`\`\n${f.text}\n\`\`\``;
  }
  const blocks: Json[] = [];
  if (body) blocks.push({ type: "text", text: body });
  if (!options.builtin) {
    for (const f of withPath) {
      const block: JsonObject = { type: "resource_link", uri: `file://${encodeURI(f.path!)}`, name: f.name };
      blocks.push(block);
    }
  }
  if (options.images) {
    for (const a of extras.attachments) {
      if (a.kind === "image") blocks.push({ type: "image", mimeType: a.mimeType, data: a.data });
    }
  }
  return blocks;
}

/** What the transcript shows for a sent message's attachments. */
export interface SentAttachment {
  kind: "image" | "file" | "text";
  name: string;
  /** A data URL, for an image sent from here. */
  preview?: string;
  /**
   * The pasted text, on a message sent before pastes were composed into the text: history keeps its
   * chip (`ui/PasteChip.tsx`), and Retry or Edit carries the text again.
   */
  text?: string;
}

/**
 * What the transcript keeps of a sent message's attachments. A long paste is in the message's text
 * now (`composedText`), so no chip is kept for it; images and files keep their bubble.
 */
export function sentAttachments(extras: ComposeExtras): SentAttachment[] {
  return extras.attachments.flatMap((a): SentAttachment[] => {
    if (a.kind === "image")
      return [{ kind: "image", name: a.name, preview: `data:${a.mimeType};base64,${a.data}` }];
    if (a.kind === "text") return [];
    return [{ kind: "file", name: a.name }];
  });
}

const IMAGE_MARKER = /!\[image\]\(file:\/\/[^)\s]+\)\n?/g;

/** A replayed user message: its text without the engine's `![image](file://…)` markers, and how many there were. */
export function splitImageMarkers(text: string): { text: string; images: number } {
  const images = text.match(IMAGE_MARKER)?.length ?? 0;
  return { text: images ? text.replace(IMAGE_MARKER, "").trim() : text, images };
}

const URL_IN_TEXT = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]}]/gi;
/** A file reference the `@` menu wrote: `@src/main.ts` or, with a space in the path, `@"my file.ts"`. */
const REF_IN_TEXT = /(^|[\s(])@("[^"\n]+"|[\w./@+-]+)/g;

/**
 * What the composer draws as a token inside the draft: a URL, or a `@file` reference.
 *
 * T3 Code's composer keeps references and citations as tokens in the text itself. RustCode has no
 * editor, so the field's mirror does it: the textarea's text is transparent and the mirror under it
 * draws the same string with each token on a quiet pill. The spans take **no** horizontal padding,
 * since the mirror and the field share one line box and any padding would move the caret off its
 * character.
 */
export interface DraftToken {
  text?: string;
  url?: string;
  ref?: string;
}

function refRuns(text: string): DraftToken[] {
  const out: DraftToken[] = [];
  let last = 0;
  for (const m of text.matchAll(REF_IN_TEXT)) {
    const ref = m[2];
    if (!ref) continue;
    const at = (m.index ?? 0) + (m[1]?.length ?? 0);
    if (at > last) out.push({ text: text.slice(last, at) });
    out.push({ ref });
    // `at` is the `@`; the pill draws it back in front of the reference.
    last = at + 1 + ref.length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** The draft split into text and tokens, URLs first and references inside the text between them. */
export function splitTokens(text: string): DraftToken[] {
  return splitUrls(text).flatMap((part) =>
    part.url ? [part] : part.text === undefined ? [] : refRuns(part.text),
  );
}

/** Whether the draft has anything the mirror draws as a token. */
export function hasTokens(text: string): boolean {
  const urls = new RegExp(URL_IN_TEXT.source, "i");
  const refs = new RegExp(REF_IN_TEXT.source);
  return urls.test(text) || refs.test(text);
}

/** Plain text split into text runs and URLs, for link chips in user messages. */
export function splitUrls(text: string): { text?: string; url?: string }[] {
  const out: { text?: string; url?: string }[] = [];
  let last = 0;
  for (const m of text.matchAll(URL_IN_TEXT)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ text: text.slice(last, at) });
    out.push({ url: m[0] });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}
