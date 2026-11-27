/**
 * Review guides: a tour of a pull request in reading order, written by the
 * text generator (`review_guide` in `rustcode/engine/src/review_guide.rs`).
 * The prompt numbers every diff line; the answer is checked against the same
 * diff, so a stop never shows a line the pull request does not change, and
 * every changed file ends up on some stop.
 */

import type { PullRequest } from "../acp/rustcode";
import { readStorage, writeStorage } from "../lib/storage";
import type { PatchLine } from "./diff";
import type { PullDiffFile } from "./pullReview";

export interface GuideExcerpt {
  path: string;
  side: "new" | "old";
  start: number;
  end: number;
  /** The diff lines the range covers, removed lines between them included. */
  lines: PatchLine[];
}

export interface GuideStop {
  title: string;
  summary: string;
  /** A call only the maintainer can make, shown as "Look closely". */
  lookClosely: string | null;
  excerpts: GuideExcerpt[];
  files: string[];
  questions: string[];
}

export interface ReviewGuide {
  headSha: string;
  writtenAt: number;
  stops: GuideStop[];
}

const MAX_EXCERPTS = 3;
const MAX_EXCERPT_LINES = 40;
const MAX_QUESTIONS = 2;
const KEPT_GUIDES = 30;
export const GUIDE_STORAGE = "rustcode.reviewGuides";
export const LEFTOVER_TITLE = "Everything else";

/** One file of the diff as the generator reads it: every line led by the number a stop cites. */
function numberedFile(file: PullDiffFile): string {
  const head = `=== ${file.path}${file.oldPath ? ` (renamed from ${file.oldPath})` : ""} [${file.status}, +${file.added} −${file.removed}]`;
  if (file.binary) return `${head}\n(binary)`;
  const body = file.lines.map((line) => {
    if (line.kind === "hunk") return line.text;
    if (line.kind === "del") return `old ${line.old ?? ""} - ${line.text}`;
    return `${line.new ?? ""} ${line.kind === "add" ? "+" : " "} ${line.text}`;
  });
  return [head, ...body].join("\n");
}

/** What the generator reads: the pull request's title, description and numbered diff. */
export function guidePrompt(
  pull: Pick<PullRequest, "title" | "number">,
  body: string,
  files: PullDiffFile[],
) {
  return [
    `Pull request #${pull.number}: ${pull.title}`,
    "",
    "Description:",
    body.trim() || "(none)",
    "",
    "Diff (lines numbered by their side: new lines by their number, removed ones as `old N`):",
    ...files.map(numberedFile),
  ].join("\n");
}

const text = (value: unknown) => (typeof value === "string" ? value.trim() : "");
const positive = (value: unknown) =>
  typeof value === "number" && Number.isInteger(value) && value > 0 ? value : null;

/** The JSON object in `answer`, with or without a Markdown fence or words around it. */
function jsonOf(answer: string): unknown {
  const start = answer.indexOf("{");
  const end = answer.lastIndexOf("}");
  if (start < 0 || end <= start) throw new Error("The guide was not JSON.");
  try {
    return JSON.parse(answer.slice(start, end + 1));
  } catch {
    throw new Error("The guide was not valid JSON.");
  }
}

/** The lines of `file` from `start` to `end` on `side`, or none when the diff does not show them. */
export function excerptLines(
  file: PullDiffFile,
  side: "new" | "old",
  start: number,
  end: number,
): PatchLine[] {
  const numberOf = (line: PatchLine) =>
    side === "new"
      ? line.kind === "del"
        ? undefined
        : line.new
      : line.kind === "add"
        ? undefined
        : line.old;
  const inRange = (line: PatchLine) => {
    const n = numberOf(line);
    return n !== undefined && n >= start && n <= end;
  };
  const first = file.lines.findIndex(inRange);
  if (first < 0) return [];
  let last = first;
  for (let i = first; i < file.lines.length; i++) if (inRange(file.lines[i]!)) last = i;
  const lines = file.lines.slice(first, last + 1).filter((line) => line.kind !== "hunk");
  return lines.slice(0, MAX_EXCERPT_LINES);
}

function parseExcerpt(raw: unknown, byPath: Map<string, PullDiffFile>): GuideExcerpt | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const file = byPath.get(text(r.path));
  const side = r.side === "old" ? "old" : "new";
  const start = positive(r.start);
  const end = positive(r.end) ?? start;
  if (!file || start === null || end === null) return null;
  const [low, high] = start <= end ? [start, end] : [end, start];
  const lines = excerptLines(file, side, low, high);
  return lines.length ? { path: file.path, side, start: low, end: high, lines } : null;
}

/**
 * The generator's `answer` as a guide for `files`: stops without a title or
 * any content are dropped, excerpts the diff does not show are dropped, and
 * files no stop mentions are gathered into a last "Everything else" stop.
 */
export function parseGuide(
  answer: string,
  files: PullDiffFile[],
  headSha: string,
  now = Date.now(),
): ReviewGuide {
  const parsed = jsonOf(answer) as { stops?: unknown };
  if (!Array.isArray(parsed.stops)) throw new Error("The guide had no stops.");
  const byPath = new Map(files.map((file) => [file.path, file]));
  const stops: GuideStop[] = [];
  for (const raw of parsed.stops) {
    if (!raw || typeof raw !== "object") continue;
    const r = raw as Record<string, unknown>;
    const title = text(r.title);
    const excerpts = (Array.isArray(r.excerpts) ? r.excerpts : [])
      .map((excerpt) => parseExcerpt(excerpt, byPath))
      .filter((excerpt): excerpt is GuideExcerpt => excerpt !== null)
      .slice(0, MAX_EXCERPTS);
    const named = (Array.isArray(r.files) ? r.files : []).map(text).filter((path) => byPath.has(path));
    const stopFiles = [...new Set([...named, ...excerpts.map((excerpt) => excerpt.path)])];
    const summary = text(r.summary);
    if (!title || (!summary && !excerpts.length && !stopFiles.length)) continue;
    stops.push({
      title,
      summary,
      lookClosely: text(r.lookClosely) || null,
      excerpts,
      files: stopFiles,
      questions: (Array.isArray(r.questions) ? r.questions : [])
        .map(text)
        .filter(Boolean)
        .slice(0, MAX_QUESTIONS),
    });
  }
  const covered = new Set(stops.flatMap((stop) => stop.files));
  const leftover = files.map((file) => file.path).filter((path) => !covered.has(path));
  if (leftover.length) {
    stops.push({
      title: LEFTOVER_TITLE,
      summary: "",
      lookClosely: null,
      excerpts: [],
      files: leftover,
      questions: [],
    });
  }
  if (!stops.length) throw new Error("The guide had no stops.");
  return { headSha, writtenAt: now, stops };
}

/** The draft an agent thread starts with to answer `question` about one stop. */
export function stopQuestion(
  pull: Pick<PullRequest, "number" | "title" | "url">,
  stop: GuideStop,
  question: string,
) {
  const where = stop.excerpts.map((e) => `${e.path}:${e.start}${e.end > e.start ? `-${e.end}` : ""}`);
  return [
    `About pull request #${pull.number} (${pull.title})${pull.url ? `: ${pull.url}` : ""}`,
    `Review guide stop "${stop.title}"${where.length ? `, at ${where.join(", ")}` : stop.files.length ? `, in ${stop.files.join(", ")}` : ""}.`,
    "",
    question,
  ].join("\n");
}

const guideKey = (cwd: string, number: number) => `${cwd}\u0000${number}`;

type Stored = Record<string, ReviewGuide>;

function readGuides(): Stored {
  try {
    const parsed = JSON.parse(readStorage(GUIDE_STORAGE) ?? "{}");
    return parsed && typeof parsed === "object" ? (parsed as Stored) : {};
  } catch {
    return {};
  }
}

/** The guide last written for pull request `number` in `cwd`, whichever push it read. */
export function savedGuide(cwd: string, number: number): ReviewGuide | null {
  return readGuides()[guideKey(cwd, number)] ?? null;
}

/** Keeps `guide` for pull request `number`, dropping the oldest past `KEPT_GUIDES`. */
export function saveGuide(cwd: string, number: number, guide: ReviewGuide) {
  const all = { ...readGuides(), [guideKey(cwd, number)]: guide };
  const kept = Object.entries(all)
    .sort(([, a], [, b]) => b.writtenAt - a.writtenAt)
    .slice(0, KEPT_GUIDES);
  writeStorage(GUIDE_STORAGE, JSON.stringify(Object.fromEntries(kept)));
}
