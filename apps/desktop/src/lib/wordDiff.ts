// Word-level marks for changed diff lines: which spans of a removed line and
// of the added line beside it actually changed, so a reader sees the edit
// inside the line and not only the line tint. Pure, and cheap by design: a
// line pair that is too long or too different is left unmarked rather than
// cost the diff a long LCS.

/** A changed span of one line, as character offsets into it (`[start, end)`). */
export interface WordRange {
  start: number;
  end: number;
}

export interface WordMarks {
  old: WordRange[];
  new: WordRange[];
}

/** Past this many characters a line is not marked: word-level detail is lost anyway. */
export const MAX_MARKED_LINE = 2000;
/** Past this many tokens per side the LCS is not worth it. */
const MAX_TOKENS = 500;
/** Below this share of kept tokens the two lines are different edits, not one edited line. */
const MIN_KEPT = 0.34;

interface Token {
  text: string;
  start: number;
  end: number;
}

/** Words, single punctuation runs, and whitespace: the pieces an edit usually touches. */
const TOKEN = /[\p{L}\p{N}_$]+|[^\s\p{L}\p{N}_$]+|\s+/gu;

function tokenize(text: string): Token[] {
  const out: Token[] = [];
  TOKEN.lastIndex = 0;
  for (let m = TOKEN.exec(text); m; m = TOKEN.exec(text)) {
    out.push({ text: m[0], start: m.index, end: m.index + m[0].length });
    if (m[0] === "") TOKEN.lastIndex++;
  }
  return out;
}

/**
 * The unkept tokens of one side, merged where they touch and trimmed at the
 * whitespace between them: a tint over spaces alone shows as a stray block.
 */
function rangesOf(text: string, tokens: Token[], kept: Uint8Array): WordRange[] {
  const out: WordRange[] = [];
  for (let i = 0; i < tokens.length; i++) {
    if (kept[i]) continue;
    let start = tokens[i]!.start;
    let end = tokens[i]!.end;
    while (i + 1 < tokens.length && !kept[i + 1]) end = tokens[++i]!.end;
    while (start < end && !text[start]!.trim()) start++;
    while (end > start && !text[end - 1]!.trim()) end--;
    if (start < end) out.push({ start, end });
  }
  return out;
}

const WHITESPACE = /^\s+$/;

/**
 * The changed spans of one line and its replacement, or null when there is
 * nothing to mark: identical lines, a line too long for it, or two lines too
 * different for the marks to mean anything.
 */
export function wordDiff(oldText: string, newText: string): WordMarks | null {
  if (oldText === newText) return null;
  if (oldText.length > MAX_MARKED_LINE || newText.length > MAX_MARKED_LINE) return null;
  const a = tokenize(oldText);
  const b = tokenize(newText);
  if (!a.length || !b.length) return null;
  if (a.length > MAX_TOKENS || b.length > MAX_TOKENS) return null;

  // Longest common subsequence over tokens, walked back into the kept ones.
  const n = a.length;
  const m = b.length;
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i]![j] =
        a[i]!.text === b[j]!.text ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!);
    }
  }
  const keptA = new Uint8Array(n);
  const keptB = new Uint8Array(m);
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i]!.text === b[j]!.text) {
      keptA[i++] = 1;
      keptB[j++] = 1;
    } else if (lcs[i + 1]![j]! >= lcs[i]![j + 1]!) i++;
    else j++;
  }
  // Words, not the spaces between them: two unrelated sentences share their spaces.
  let kept = 0;
  let words = 0;
  for (let k = 0; k < n; k++) {
    if (WHITESPACE.test(a[k]!.text)) continue;
    words++;
    if (keptA[k]) kept++;
  }
  const newWords = b.reduce((sum, token) => sum + (WHITESPACE.test(token.text) ? 0 : 1), 0);
  if (kept / Math.max(words, newWords) < MIN_KEPT) return null;
  const oldRanges = rangesOf(oldText, a, keptA);
  const newRanges = rangesOf(newText, b, keptB);
  if (!oldRanges.length && !newRanges.length) return null;
  return { old: oldRanges, new: newRanges };
}

const REMOVED = new Set(["removed", "del"]);
const ADDED = new Set(["added", "add"]);

/**
 * Marks per line of a diff, aligned with it: each run of removals is paired in
 * order with the additions written after it, so the marks sit on the line each
 * removal became. Context lines, and runs with no counterpart, get none.
 */
export function runWordMarks(lines: readonly { kind: string; text: string }[]): (WordRange[] | undefined)[] {
  const out: (WordRange[] | undefined)[] = Array.from({ length: lines.length }, () => undefined);
  let i = 0;
  while (i < lines.length) {
    if (!REMOVED.has(lines[i]!.kind)) {
      i++;
      continue;
    }
    const removed: number[] = [];
    while (i < lines.length && REMOVED.has(lines[i]!.kind)) removed.push(i++);
    const added: number[] = [];
    while (i < lines.length && ADDED.has(lines[i]!.kind)) added.push(i++);
    for (let k = 0; k < Math.min(removed.length, added.length); k++) {
      const marks = wordDiff(lines[removed[k]!]!.text, lines[added[k]!]!.text);
      if (!marks) continue;
      if (marks.old.length) out[removed[k]!] = marks.old;
      if (marks.new.length) out[added[k]!] = marks.new;
    }
  }
  return out;
}

/** One drawn piece of a line: its text, and whether the mark tint goes under it. */
export interface Piece {
  text: string;
  marked: boolean;
}

/** A line split around its marks, covering all of its text and nothing else. */
export function markedPieces(text: string, marks?: readonly WordRange[]): Piece[] {
  if (!marks?.length) return [{ text, marked: false }];
  const out: Piece[] = [];
  let at = 0;
  for (const range of marks) {
    const start = Math.max(at, Math.min(range.start, text.length));
    const end = Math.max(start, Math.min(range.end, text.length));
    if (start >= end) continue;
    if (start > at) out.push({ text: text.slice(at, start), marked: false });
    out.push({ text: text.slice(start, end), marked: true });
    at = end;
  }
  if (at < text.length) out.push({ text: text.slice(at), marked: false });
  return out.length ? out : [{ text, marked: false }];
}
