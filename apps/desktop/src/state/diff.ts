// Unified diffs for the Review screen: parsed into numbered lines, paired
// for side-by-side, and review comments turned into one message for the
// agent. Pure.

export interface PatchLine {
  kind: "add" | "del" | "ctx" | "hunk";
  text: string;
  old?: number;
  new?: number;
}

/** A file's hunks (`@@ … @@` and the lines after) as numbered lines. Header lines are skipped. */
export function parsePatch(patch: string | string[]): PatchLine[] {
  const lines = Array.isArray(patch) ? patch : patch.split("\n");
  const out: PatchLine[] = [];
  let oldNo = 0;
  let newNo = 0;
  let inHunk = false;
  for (const line of lines) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/.exec(line);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      inHunk = true;
      out.push({ kind: "hunk", text: line });
      continue;
    }
    if (!inHunk || line.startsWith("\\")) continue;
    if (line.startsWith("+")) out.push({ kind: "add", text: line.slice(1), new: newNo++ });
    else if (line.startsWith("-")) out.push({ kind: "del", text: line.slice(1), old: oldNo++ });
    else if (line.startsWith(" ") || line === "") {
      out.push({ kind: "ctx", text: line.slice(1), old: oldNo++, new: newNo++ });
    }
  }
  // A trailing empty line from the split is not context.
  const last = out.at(-1);
  if (last?.kind === "ctx" && last.text === "" && lines.at(-1) === "") out.pop();
  return out;
}

export interface SplitRow {
  left?: PatchLine;
  right?: PatchLine;
  hunk?: string;
}

/** Side by side: context on both sides, a run of removals next to the run of additions after it. */
export function splitRows(lines: PatchLine[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (line.kind === "hunk") {
      rows.push({ hunk: line.text });
      i++;
    } else if (line.kind === "ctx") {
      rows.push({ left: line, right: line });
      i++;
    } else {
      const dels: PatchLine[] = [];
      const adds: PatchLine[] = [];
      while (lines[i]?.kind === "del") dels.push(lines[i++]!);
      while (lines[i]?.kind === "add") adds.push(lines[i++]!);
      for (let k = 0; k < Math.max(dels.length, adds.length); k++)
        rows.push({ left: dels[k], right: adds[k] });
    }
  }
  return rows;
}

export interface ReviewComment {
  path: string;
  /** The last line commented on; with `startLine`, the end of the range. */
  line: number;
  /** The first line of a range, on the same side. */
  startLine?: number;
  side: "LEFT" | "RIGHT";
  body: string;
  /** The lines commented on, quoted in the message. */
  code: string;
}

/** Lines of a range quoted in a message; the rest is elided. */
export const QUOTED_LINES = 6;

/** The comments as one message for the agent working on these changes. */
export function reviewMessage(source: string, comments: ReviewComment[], note = ""): string {
  const items = comments.map((c) => {
    const ranged = c.startLine !== undefined && c.startLine !== c.line;
    const where = ranged ? `${c.path}:${c.startLine}-${c.line}` : `${c.path}:${c.line}`;
    const code = c.code.split("\n");
    const shown = code.slice(0, QUOTED_LINES).map((line) => `\n  > ${line.trimEnd()}`);
    const quote = c.code.trim() ? shown.join("") + (code.length > QUOTED_LINES ? "\n  > …" : "") : "";
    const removed = c.side === "LEFT" ? (ranged ? " (removed lines)" : " (removed line)") : "";
    return `- \`${where}\`${removed}: ${c.body.trim()}${quote}`;
  });
  return [`Review comments on ${source}:`, note.trim(), items.join("\n")].filter(Boolean).join("\n\n");
}

/** Paths as a tree: folders first, each folder's files under it. */
export function fileTree(paths: string[]): { dir: string; files: string[] }[] {
  const byDir = new Map<string, string[]>();
  for (const p of [...paths].sort()) {
    const at = p.lastIndexOf("/");
    const dir = at < 0 ? "" : p.slice(0, at);
    byDir.set(dir, [...(byDir.get(dir) ?? []), p]);
  }
  return [...byDir.entries()].map(([dir, files]) => ({ dir, files }));
}

/**
 * The lines numbered `from` to `to` on one side of a file's patch, in order, or null when the
 * range crosses a hunk header (lines the diff does not show) or a number is missing.
 */
export function rangeLines(
  lines: readonly PatchLine[],
  side: "LEFT" | "RIGHT",
  from: number,
  to: number,
): PatchLine[] | null {
  const [lo, hi] = from <= to ? [from, to] : [to, from];
  const out: PatchLine[] = [];
  let open = false;
  for (const line of lines) {
    if (line.kind === "hunk") {
      if (open) return null;
      continue;
    }
    const n = side === "LEFT" ? line.old : line.new;
    if (n === undefined) continue;
    if (n === lo) open = true;
    if (open) out.push(line);
    if (n === hi) return open ? out : null;
  }
  return null;
}
