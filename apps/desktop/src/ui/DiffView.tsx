// The diff of one file: the transcript's tool-row diff, and the rows the
// Changes panel and the turn popover draw a patch with. Lines keep their
// syntax colours and carry word-level marks inside them (lib/wordDiff.ts),
// so an edited line shows what changed in it and not only its tint.

import { FilePlus, FileText } from "lucide-react";
import { useMemo, useState } from "react";
import type { FileDiff } from "../acp/types";
import { useHighlightedLines } from "../lib/useHighlightedLines";
import { markedPieces, type Piece, runWordMarks, type WordRange } from "../lib/wordDiff";
import { type PatchLine, splitRows } from "../state/diff";
import { diffCounts, diffLines } from "../state/thread";
import { cx } from "./primitives";
import { DiffCount } from "./Rows";

const LINE_TINT = {
  added: "bg-[var(--rc-diff-add)]",
  removed: "bg-[var(--rc-diff-del)]",
} as const;

/** The word marks read stronger than the line they sit on (index.css › diff tokens). */
const WORD_TINT = {
  add: "rounded-xs bg-[var(--rc-diff-add-word)]",
  del: "rounded-xs bg-[var(--rc-diff-del-word)]",
} as const;

/**
 * Highlighted fragments for a set of lines, split around their word marks:
 * one call to the highlighter for the lot, and the index where each line's
 * fragments start.
 */
export function useMarkedFragments(
  texts: string[],
  marks: readonly (WordRange[] | undefined)[],
  path: string,
): { pieces: Piece[][]; html: (string | null)[]; starts: number[] } {
  const pieces = useMemo(() => texts.map((text, i) => markedPieces(text, marks[i])), [texts, marks]);
  const flat = useMemo(() => pieces.flat().map((piece) => piece.text), [pieces]);
  const html = useHighlightedLines(flat, path);
  const starts = useMemo(() => {
    const out: number[] = [];
    let at = 0;
    for (const line of pieces) {
      out.push(at);
      at += line.length;
    }
    return out;
  }, [pieces]);
  return { pieces, html, starts };
}

/** One line's fragments, marked words tinted in their side's colour. */
export function Fragments({
  pieces,
  html,
  start,
  tint,
}: {
  pieces: Piece[];
  html: (string | null)[];
  start: number;
  tint?: "add" | "del";
}) {
  return (
    <span className="whitespace-pre">
      {pieces.map((piece, i) => {
        const code = html[start + i];
        return (
          <span key={i} className={piece.marked && tint ? WORD_TINT[tint] : undefined}>
            {code ? (
              // Shiki escapes the code; the HTML holds only spans with colour variables.
              // biome-ignore lint/security/noDangerouslySetInnerHtml: highlighted, escaped output
              <span className="shiki whitespace-pre" dangerouslySetInnerHTML={{ __html: code }} />
            ) : (
              piece.text || " "
            )}
          </span>
        );
      })}
    </span>
  );
}

/** A file's diff in a transcript row, folded to its first lines. */
export function DiffView({ diff, header = true }: { diff: FileDiff; header?: boolean }) {
  const lines = useMemo(() => diffLines(diff), [diff]);
  const texts = useMemo(() => lines.map((line) => line.text), [lines]);
  const marks = useMemo(() => runWordMarks(lines), [lines]);
  const { pieces, html, starts } = useMarkedFragments(texts, marks, diff.path);
  const [all, setAll] = useState(false);
  const counts = useMemo(() => diffCounts([diff]), [diff]);
  const shown = all || lines.length <= 15 ? lines.length : 12;
  const width = String(
    Math.max(...lines.map((l) => (l.kind === "removed" ? l.oldNumber : l.newNumber) ?? 0), 1),
  ).length;
  return (
    <div>
      {header && (
        <div className="mb-1 flex items-center gap-1.5 text-caption">
          {diff.oldText === undefined ? (
            <FilePlus size={12} className="text-fg-muted" />
          ) : (
            <FileText size={12} className="text-fg-muted" />
          )}
          <span className="truncate font-mono">{diff.path}</span>
          <DiffCount {...counts} />
        </div>
      )}
      <div className="selectable overflow-x-auto rounded-md bg-fill-subtle py-1 font-mono text-caption leading-[16px]">
        <div className="min-w-fit">
          {lines.slice(0, shown).map((line, i) => (
            <div key={i} className={cx("flex pr-3", line.kind !== "context" && LINE_TINT[line.kind])}>
              <span
                className="shrink-0 pr-1.5 pl-2 text-right text-fg-muted/60 tabular"
                style={{ width: `${width + 2}ch` }}
              >
                {(line.kind === "removed" ? line.oldNumber : line.newNumber) ?? ""}
              </span>
              <span
                className={cx(
                  "w-3 shrink-0",
                  line.kind === "added" && "text-success",
                  line.kind === "removed" && "text-danger",
                )}
              >
                {line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}
              </span>
              <Fragments
                pieces={pieces[i]!}
                html={html}
                start={starts[i]!}
                tint={line.kind === "added" ? "add" : line.kind === "removed" ? "del" : undefined}
              />
            </div>
          ))}
        </div>
      </div>
      {lines.length > 15 && (
        <button
          type="button"
          onClick={() => setAll(!all)}
          className="mt-1 text-caption text-accent hover:underline"
        >
          {all ? "Show less" : `Show all ${lines.length} lines`}
        </button>
      )}
    </div>
  );
}

const PATCH_TINT = {
  add: "bg-[var(--rc-diff-add)]",
  del: "bg-[var(--rc-diff-del)]",
} as const;

const signOf = (kind: PatchLine["kind"]) => (kind === "add" ? "+" : kind === "del" ? "−" : " ");
const patchTint = (kind: PatchLine["kind"]) =>
  kind === "add" ? PATCH_TINT.add : kind === "del" ? PATCH_TINT.del : undefined;
const wordTintOf = (kind: PatchLine["kind"]) =>
  kind === "add" ? ("add" as const) : kind === "del" ? ("del" as const) : undefined;

/**
 * A patch (`@@` hunks and their lines) as one file's body, unified or side by
 * side. Review draws its own to place comments; this is the plain one.
 */
export function Patch({
  lines,
  path,
  mode = "unified",
}: {
  lines: PatchLine[];
  path: string;
  mode?: "unified" | "split";
}) {
  const texts = useMemo(() => lines.map((line) => line.text), [lines]);
  const marks = useMemo(() => runWordMarks(lines), [lines]);
  const { pieces, html, starts } = useMarkedFragments(texts, marks, path);
  const index = useMemo(() => new Map(lines.map((line, i) => [line, i])), [lines]);
  const width = String(Math.max(...lines.map((line) => line.new ?? line.old ?? 0), 1)).length;
  const hunkRow = (key: string, text: string) => (
    <div key={key} className="px-2.5 text-fg-faint">
      {text}
    </div>
  );
  return (
    <div className="selectable overflow-x-auto rounded-md bg-fill-subtle py-1 font-mono text-caption leading-[18px]">
      <div className="min-w-fit">
        {mode === "unified"
          ? lines.map((line, i) =>
              line.kind === "hunk" ? (
                hunkRow(`u${i}`, line.text)
              ) : (
                <div key={i} className={cx("flex pr-3", patchTint(line.kind))}>
                  <span
                    className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                    style={{ width: `${width + 1}ch` }}
                  >
                    {line.old ?? ""}
                  </span>
                  <span
                    className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                    style={{ width: `${width + 1}ch` }}
                  >
                    {line.new ?? ""}
                  </span>
                  <span
                    className={cx(
                      "w-3 shrink-0",
                      line.kind === "add" && "text-success",
                      line.kind === "del" && "text-danger",
                    )}
                  >
                    {signOf(line.kind)}
                  </span>
                  <Fragments
                    pieces={pieces[i]!}
                    html={html}
                    start={starts[i]!}
                    tint={wordTintOf(line.kind)}
                  />
                </div>
              ),
            )
          : splitRows(lines).map((row, i) => {
              if (row.hunk !== undefined) return hunkRow(`s${i}`, row.hunk);
              return (
                <div key={i} className="grid grid-cols-2">
                  {(["left", "right"] as const).map((side) => {
                    const line = side === "left" ? row.left : row.right;
                    const at = line ? index.get(line) : undefined;
                    return (
                      <div
                        key={side}
                        className={cx("flex min-w-0", side === "right" && "border-border border-l")}
                      >
                        {line && at !== undefined ? (
                          <>
                            <span
                              className="shrink-0 pr-1.5 text-right text-fg-muted/60 tabular"
                              style={{ width: `${width + 1}ch` }}
                            >
                              {(side === "left" ? line.old : line.new) ?? ""}
                            </span>
                            <span className={cx("flex-1", patchTint(line.kind))}>
                              <Fragments
                                pieces={pieces[at]!}
                                html={html}
                                start={starts[at]!}
                                tint={wordTintOf(line.kind)}
                              />
                            </span>
                          </>
                        ) : (
                          <>
                            <span className="shrink-0" style={{ width: `${width + 1}ch` }} />
                            <span className="h-[18px] flex-1 bg-fill-subtle/50" />
                          </>
                        )}
                      </div>
                    );
                  })}
                </div>
              );
            })}
      </div>
    </div>
  );
}
