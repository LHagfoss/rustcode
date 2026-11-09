// Where a fork or a side chat came from, and what came of a thread: the header's "Forked from
// ‹title›" link on the child, and the lifecycle row ("Forked to ‹title›", "Side chat ‹title›") in
// the parent's transcript. Both are read from the sessions' recorded parents (state/branches.ts),
// so they are there after a reload and cost the parent's history nothing.

import { CornerUpLeft, GitFork, MessageSquareQuote } from "lucide-react";
import { useMemo } from "react";
import { type Branch, type BranchKind, branchLine, originLine, originOf } from "../state/branches";
import { peekThread, selectThread, useApp } from "../state/store";

const Glyph = ({ kind, size }: { kind: BranchKind; size: number }) =>
  kind === "fork" ? <GitFork size={size} /> : <MessageSquareQuote size={size} />;

/** The header's link from a fork or side chat back to the thread it came from. */
export function OriginChip({ threadId }: { threadId: string }) {
  const sessions = useApp((s) => s.sessions);
  const origin = useMemo(() => originOf(sessions, threadId), [sessions, threadId]);
  if (!origin) return null;
  const line = originLine(origin);
  const open = origin.listed && origin.id ? origin.id : undefined;
  return (
    <button
      type="button"
      disabled={!open}
      title={open ? `Open ${origin.title}` : `${origin.title} is no longer in the list`}
      onClick={() => open && selectThread(open)}
      className="flex min-w-0 max-w-[260px] shrink items-center gap-1 rounded-sm px-1 text-caption text-fg-muted enabled:hover:bg-fill-subtle enabled:hover:text-fg disabled:cursor-default"
    >
      <Glyph kind={origin.kind} size={11} />
      <span className="shrink-0">{line.label}</span>
      <span className="min-w-0 truncate font-medium">{line.title}</span>
      {open && <CornerUpLeft size={10} className="shrink-0 text-fg-faint" />}
    </button>
  );
}

/** One line in a parent's transcript: a fork or a side chat it has, opening it when pressed. */
export function BranchRow({ branch }: { branch: Branch }) {
  const line = branchLine(branch);
  return (
    <div className="flex h-6 items-center justify-center gap-1.5 text-caption text-fg-faint">
      <Glyph kind={branch.kind} size={11} />
      <span>{line.label}</span>
      <button
        type="button"
        title={branch.kind === "fork" ? "Open the fork" : "Open the side chat beside this thread"}
        onClick={() => (branch.kind === "fork" ? selectThread(branch.id) : peekThread(branch.id))}
        className="min-w-0 max-w-[60%] truncate rounded-sm font-medium text-fg-muted hover:text-fg hover:underline"
      >
        {line.title}
      </button>
    </div>
  );
}
