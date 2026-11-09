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
