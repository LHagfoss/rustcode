// A long paste is not text the field should hold: a log, a stack trace or a whole file dumped
// between two words makes the draft unreadable and the card tall. Above this size the clipboard's
// text becomes a chip above the field instead, and is still sent with the message.

/** Lines a paste needs to be held as a chip: a dozen is already more than the field shows. */
export const PASTE_LINES = 12;
/** Characters a paste needs when it has few lines: one long line is the same wall of text. */
export const PASTE_CHARS = 1200;

/** How many lines a text has, as `wc -l` would count them with the trailing newline kept. */
function lines(text: string): number {
  return text.split("\n").length;
}

/** Whether a paste goes to a chip instead of the field. */
export function isLongPaste(text: string): boolean {
  return text.length >= PASTE_CHARS || lines(text) >= PASTE_LINES;
}

/**
 * What a paste carries that the field cannot hold: a picture. A web view hands
 * one over in `files`, lists it only as an item (WebKitGTK on Linux does), or
 * keeps it to the native clipboard, which the composer reads itself.
 */
export interface PasteItemLike {
  kind: string;
  type: string;
  getAsFile?: () => File | null;
}

export interface PasteLike {
  files: ArrayLike<File>;
  items?: ArrayLike<PasteItemLike>;
}

/**
 * The images a paste carries, from `files` when the web view fills it and from
 * the items otherwise. Text items stay out of it (they go into the field), and
 * a picture both lists name is attached once.
 */
export function pastedImages(paste: PasteLike): File[] {
  const out: File[] = [];
  const seen = new Set<string>();
  const add = (file: File | null | undefined) => {
    if (!file?.type.startsWith("image/")) return;
    const key = `${file.type}/${file.name}/${file.size}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(file);
  };
  for (const file of Array.from(paste.files ?? [])) add(file);
  for (const item of Array.from(paste.items ?? [])) {
    if (item.kind === "file" && item.type.startsWith("image/")) add(item.getAsFile?.());
  }
  return out;
}

/** `3200` → `3.2k`, `12000` → `12k`: enough to say how big it is, not a digit more. */
function count(n: number): string {
  if (n < 1000) return String(n);
  const k = Math.round(n / 100) / 10;
  return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
}

/**
 * What the chip says: `Pasted text · 84 lines`, or `Pasted text · 3.2k chars` when the paste has
 * only a few very long lines, where the line count says nothing about how big it is.
 */
export function pasteLabel(text: string): string {
  const n = lines(text);
  return n >= PASTE_LINES ? `Pasted text · ${n} lines` : `Pasted text · ${count(text.length)} chars`;
}
