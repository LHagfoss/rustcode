// What a message is made of before it is sent: the typed text, an optional
// slash-command token, pasted link chips, images and files. `promptBlocks`
// turns it into ACP content blocks; no React and no I/O, so tests drive it.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { SlashCommand } from "../acp/types";
import { pasteLabel } from "../lib/paste";

/**
 * A message that starts with a command (`/review the diff`), as the name and what follows. A path
 * (`/usr/bin/env`) is not a command: the name stops at a space.
 */
export function splitSlashCommand(text: string): { command: string; rest: string } | null {
  const match = /^(\/[A-Za-z][\w:-]*)(?:\s+|$)/.exec(text);
  return match ? { command: match[1]!, rest: text.slice(match[0].length) } : null;
}

/**
 * The `/word` the caret is in, when the `/` opens a word: at the start of the
 * message or just after whitespace. A path is not a command, so a second slash
 * inside the word (`/usr/bin`) closes the token, and so does a space.
 */
export function slashToken(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const start = before.lastIndexOf("/");
  if (start < 0) return null;
  if (start > 0 && !/\s/.test(before[start - 1] ?? "")) return null;
  const query = before.slice(start + 1);
  if (/\s/.test(query)) return null;
  // The caret mid-word: the rest of the word is part of what gets replaced.
  if (/^\S/.test(text.slice(caret))) return null;
  return { start, query };
}

/**
 * The commands the token `query` is looking for: names that start with it first,
 * then the ones with it anywhere, so `/rev` finds `/review` and `/release`.
 */
export function commandSuggestions(query: string, commands: SlashCommand[], limit = 40): SlashCommand[] {
  const typed = `/${query.toLowerCase()}`;
  const prefixed = commands.filter((c) => c.name.toLowerCase().startsWith(typed));
  const others =
    typed.length > 1
      ? commands.filter((c) => !prefixed.includes(c) && c.name.toLowerCase().includes(typed.slice(1)))
      : [];
  return [...prefixed, ...others].slice(0, limit);
}

/** The text with the token replaced by `/name `, and where the caret goes. */
export function insertCommand(
  text: string,
  token: { start: number; query: string },
  command: string,
): { text: string; caret: number } {
  const ref = `${command} `;
  const end = token.start + 1 + token.query.length;
  const rest = text.slice(end).replace(/^ /, "");
  return { text: text.slice(0, token.start) + ref + rest, caret: token.start + ref.length };
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
  /** A long paste the field kept out of the draft (lib/paste.ts). */
  | { id: string; kind: "text"; text: string };

export interface ComposeExtras {
  command?: SlashCommand;
  links: string[];
  attachments: Attachment[];
}

export const emptyExtras: ComposeExtras = { links: [], attachments: [] };

export function hasExtras(extras: ComposeExtras | undefined): boolean {
  return !!extras && (!!extras.command || extras.links.length > 0 || extras.attachments.length > 0);
}

/** The text the prompt carries, in the order the composer shows it: command, links, then what was typed. */
export function composedText(text: string, extras: ComposeExtras): string {
  const head = [extras.command?.name, ...extras.links].filter(Boolean).join(" ");
  const body = text.trim();
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

/** A fenced block the text cannot close: one backtick longer than its longest run of them. */
function fenced(text: string): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

/**
 * ACP prompt blocks for a message. Images become `image` blocks; files with a
 * path become `resource_link` blocks on an external harness and an "Attached
 * files" list in the text on the built-in one (it ignores links, as the
 * SwiftUI app found); a file without a path, and a long paste held as a chip,
 * are inlined as text.
 *
 * A skill is RustCode's own (the harness did not advertise it): the built-in
 * harness reads `/name` as a prompt and routes it to `use_skill`, but another
 * harness may reject a leading slash command it does not know, so it is sent as
 * words naming the skill instead. What the transcript shows is unchanged.
 */
export function promptBlocks(text: string, extras: ComposeExtras, options: BlockOptions): Json[] {
  let body = composedText(text, extras);
  if (!options.builtin && extras.command?.action === "skill") {
    const name = extras.command.name.replace(/^\/+/, "");
    const rest = composedText(text, { ...extras, command: undefined });
    body = rest ? `Use the ${name} skill. ${rest}` : `Use the ${name} skill.`;
  }
  const files = extras.attachments.filter((a) => a.kind === "file");
  const withPath = files.filter((f) => f.path);
  const inline = files.filter((f) => !f.path && f.text !== undefined);
  if (withPath.length && options.builtin) {
    const list = withPath.map((f) => `- \`${relative(f.path!, options.cwd)}\``).join("\n");
    body = `${body ? `${body}\n\n` : ""}Attached files:\n${list}`;
  }
  // A pasted block goes in where the user wrote it, as a fenced block of its own, before the files
  // it came with.
  for (const a of extras.attachments) {
    if (a.kind !== "text") continue;
    body = `${body ? `${body}\n\n` : ""}${fenced(a.text)}`;
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
  /** The pasted text, for the chip a sent message keeps (`ui/PasteChip.tsx`). */
  text?: string;
}

export function sentAttachments(extras: ComposeExtras): SentAttachment[] {
  return extras.attachments.map((a) => {
    if (a.kind === "image")
      return { kind: "image", name: a.name, preview: `data:${a.mimeType};base64,${a.data}` };
    if (a.kind === "text") return { kind: "text", name: pasteLabel(a.text), text: a.text };
    return { kind: "file", name: a.name };
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
