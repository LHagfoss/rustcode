/**
 * A command line as a person reads it: without the `/bin/zsh -lc '…'` wrapper
 * Codex and others run every command through, and with the home folder as `~`.
 */
export function displayCommand(text: string): string {
  return unwrapShell(text.trim()).replace(/(^|[\s'"=:(])\/Users\/[^/\s'"]+(?=\/|$|[\s'"])/g, "$1~");
}

const WRAPPER = /^(?:\/(?:usr\/)?bin\/)?(?:env\s+)?(?:zsh|bash|sh)\s+-l?c\s+([\s\S]+)$/;

function unwrapShell(text: string): string {
  const match = WRAPPER.exec(text);
  if (!match) return text;
  const arg = match[1]!.trim();
  // One single-quoted argument, where `'\''` is how a quote inside is written.
  if (/^'(?:[^']|'\\'')*'$/.test(arg)) return arg.slice(1, -1).replaceAll("'\\''", "'");
  if (/^"(?:[^"\\]|\\.)*"$/.test(arg)) return arg.slice(1, -1).replace(/\\(["\\$`])/g, "$1");
  return text;
}
