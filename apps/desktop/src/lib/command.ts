import { isAbsolute, normalize, shortenHome } from "./paths";

/**
 * A command line as a person reads it: without the `/bin/zsh -lc '…'` wrapper
 * Codex and others run every command through, and with the home folder as `~`.
 */
export function displayCommand(text: string): string {
  return unwrapShell(text.trim()).replace(/(^|[\s'"=:(])\/(?:Users|home)\/[^/\s'"]+(?=\/|$|[\s'"])/g, "$1~");
}

const LEADING_CD = /^cd\s+(?:"([^"]+)"|'([^']+)'|([^\s;&|]+))\s*&&\s*([\s\S]+)$/;

/**
 * Drops a leading `cd <dir> &&` when `<dir>` is already the thread's working
 * folder (in either full or `~`-shortened form), keeping the real command.
 */
export function stripLeadingCd(command: string, cwd?: string): string {
  const trimmed = command.trim();
  if (!cwd) return trimmed;
  const match = LEADING_CD.exec(trimmed);
  if (!match) return trimmed;
  const target = (match[1] ?? match[2] ?? match[3] ?? "").replace(/[\\/]+$/, "");
  const cleanCwd = normalize(cwd).replace(/[\\/]+$/, "");
  const shortCwd = shortenHome(cleanCwd).replace(/[\\/]+$/, "");
  const normalizedTarget = isAbsolute(target) ? normalize(target).replace(/[\\/]+$/, "") : target;
  if (normalizedTarget === cleanCwd || normalizedTarget === shortCwd) {
    return match[4]!.trim();
  }
  return trimmed;
}

const WRAPPER = /^(?:\/(?:usr\/)?bin\/)?(?:env\s+)?(?:zsh|bash|sh)\s+-l?c\s+([\s\S]+)$/;

export function unwrapShell(text: string): string {
  const match = WRAPPER.exec(text);
  if (!match) return text;
  const arg = match[1]!.trim();
  // One single-quoted argument, where `'\''` is how a quote inside is written.
  if (/^'(?:[^']|'\\'')*'$/.test(arg)) return arg.slice(1, -1).replaceAll("'\\''", "'");
  if (/^"(?:[^"\\]|\\.)*"$/.test(arg)) return arg.slice(1, -1).replace(/\\(["\\$`])/g, "$1");
  return text;
}
