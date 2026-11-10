// What a tool call printed, as the transcript shows it. Some harnesses wrap a
// tool result in a Markdown fence of its own (Claude Code's adapter sends a
// background command's answer as a ```console block), which is a code block in
// the harness's chat, not text of the output: drawn raw, the backticks and the
// language name would read as the command's own words.

/** An opening fence: indentation, the run of backticks or tildes, its info string, and the break. */
const OPENING = /^[ \t]*(`{3,}|~{3,})[^\n]*(?:\r?\n|$)/;
/** The first line that can close it: the same character, at least as long, alone but for spaces. */
const closing = (marker: string) => new RegExp(`^[ \\t]*\\${marker[0]}{${marker.length},}[ \\t]*\\r?$`, "m");

/**
 * The output a fenced tool result carries, without the fence around it. Text
 * that only holds a fence inside it, or none at all, comes back unchanged:
 * a file's Markdown printed by `cat` is the output, fences and all.
 */
export function stripOutputFence(text: string): string {
  const open = OPENING.exec(text);
  if (!open) return text;
  const marker = open[1]!;
  const body = text.slice(open[0].length);
  // Markdown closes a fence at the first line holding a run of its own character that is at least
  // as long as the one that opened it, so a four-backtick fence wraps a three-backtick block instead
  // of ending at it. Anything but whitespace after that line means the fence is part of what the
  // call printed rather than a wrapper around all of it, and the text stays as it is.
  const close = closing(marker).exec(body);
  if (!close || body.slice(close.index + close[0].length).trim() !== "") return text;
  return body.slice(0, close.index).replace(/\r?\n$/, "");
}
