// A thread row's second line, left side: where the thread runs, as state/sidebarGroups.ts
// `rowWhere` picks it. A worktree or a branch carries its glyph; a folder is fitted from both ends.

import { useEffect, useRef, useState } from "react";
import { middleEllipsis } from "../lib/folderList";
import type { RowWhere } from "../state/sidebarGroups";
import { useApp } from "../state/store";
import { FolderGit2, GitBranch } from "./icons";

/** Fit both ends of a folder path to the space the row actually gives it. */
export function MiddlePath({ path }: { path: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [chars, setChars] = useState(32);
  const font = useApp((s) => s.font);
  const textSize = useApp((s) => s.textSize);
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new font or text size changes the em, not the box, so measure again
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    // An average character is just under 0.6em in the app's faces (7px at Small's 12px); reading the
    // size keeps the estimate right at every Text size and font.
    const observer = new ResizeObserver(([entry]) => {
      const em = Number.parseFloat(getComputedStyle(node).fontSize) || 12;
      if (entry) setChars(Math.max(5, Math.floor(entry.contentRect.width / (em * 0.58))));
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [font, textSize]);
  return (
    <span ref={ref} className="block overflow-hidden text-ellipsis whitespace-nowrap">
      {middleEllipsis(path, chars)}
    </span>
  );
}

/** Where the row runs; `title` is the full folder, for the pointer that rests on it. */
export function RowWhereLabel({ where, title }: { where: RowWhere; title?: string }) {
  if (where.kind === "none") return <span className="min-w-0 flex-1" />;
  const Glyph = where.kind === "worktree" ? FolderGit2 : where.kind === "branch" ? GitBranch : undefined;
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1" title={title}>
      {Glyph && <Glyph size={12} className="shrink-0 text-fg-faint" />}
      {/* A branch is cut at its end; a folder keeps both ends. */}
      {Glyph ? <span className="min-w-0 truncate">{where.text}</span> : <MiddlePath path={where.text} />}
    </span>
  );
}
