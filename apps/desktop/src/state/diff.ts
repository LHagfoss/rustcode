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
  line: number;
  side: "LEFT" | "RIGHT";
  body: string;
  /** The line commented on, quoted in the message. */
  code: string;
}

/** The comments as one message for the agent working on these changes. */
export function reviewMessage(source: string, comments: ReviewComment[], note = ""): string {
  const items = comments.map((c) => {
    const quote = c.code.trim() ? `\n  > ${c.code.trim()}` : "";
    return `- \`${c.path}:${c.line}\`${c.side === "LEFT" ? " (removed line)" : ""}: ${c.body.trim()}${quote}`;
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
