// A thread row's third line, left side: where the thread runs, as state/sidebarGroups.ts
// `rowWhere` picks it. A worktree or a branch leads with its glyph, as T3 Code's sidebar does, and
// every one is cut in the middle at its real width, so both its start and its end stay readable.

import type { RowWhere } from "../state/sidebarGroups";
import { FolderGit2, GitBranch } from "./icons";
import { MiddleTruncate } from "./MiddleTruncate";

/** Where the row runs; `title` is the full folder, for the pointer that rests on it. */
export function RowWhereLabel({ where, title }: { where: RowWhere; title?: string }) {
  if (where.kind === "none") return <span className="min-w-0 flex-1" />;
  const Glyph = where.kind === "worktree" ? FolderGit2 : where.kind === "branch" ? GitBranch : undefined;
  return (
    <span className="flex min-w-0 flex-1 items-center gap-1.5" title={title}>
      {Glyph && <Glyph size={13} className="shrink-0 text-fg-faint" />}
      <span className="flex min-w-0 flex-1">
        <MiddleTruncate value={where.text} title={false} />
      </span>
    </span>
  );
}
