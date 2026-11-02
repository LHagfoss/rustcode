// What a message is made of before it is sent: the typed text, an optional
// slash-command token, pasted link chips, images and files. `promptBlocks`
// turns it into ACP content blocks; no React and no I/O, so tests drive it.

import type { Json, JsonObject } from "../acp/jsonrpc";
import type { SlashCommand } from "../acp/types";

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
  | { id: string; kind: "file"; name: string; path?: string; text?: string };

export interface ComposeExtras {
  command?: SlashCommand;
  links: string[];
  attachments: Attachment[];
}

export const emptyExtras: ComposeExtras = { links: [], attachments: [] };

export function hasExtras(extras: ComposeExtras | undefined): boolean {
  return !!extras && (!!extras.command || extras.links.length > 0 || extras.attachments.length > 0);
}

/** A pasted text that is one http(s) URL and nothing else. */
export function pastedUrl(text: string): string | null {
  const t = text.trim();
  if (!/^https?:\/\/\S+$/i.test(t) || t.length > 2048) return null;
  try {
    const url = new URL(t);
    return url.protocol === "http:" || url.protocol === "https:" ? t : null;
  } catch {
    return null;
  }
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
 * SwiftUI app found); a file without a path is inlined as text.
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
  kind: "image" | "file";
  name: string;
  /** A data URL, for an image sent from here. */
  preview?: string;
}

export function sentAttachments(extras: ComposeExtras): SentAttachment[] {
  return extras.attachments.map((a) =>
    a.kind === "image"
      ? { kind: "image", name: a.name, preview: `data:${a.mimeType};base64,${a.data}` }
      : { kind: "file", name: a.name },
  );
}

const IMAGE_MARKER = /!\[image\]\(file:\/\/[^)\s]+\)\n?/g;

/** A replayed user message: its text without the engine's `![image](file://…)` markers, and how many there were. */
export function splitImageMarkers(text: string): { text: string; images: number } {
  const images = text.match(IMAGE_MARKER)?.length ?? 0;
  return { text: images ? text.replace(IMAGE_MARKER, "").trim() : text, images };
}

const URL_IN_TEXT = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]}]/gi;

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
