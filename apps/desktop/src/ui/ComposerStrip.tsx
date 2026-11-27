import type { Checkpoint } from "../acp/v2";
import { useNow } from "../lib/useNow";
import { setView, useApp } from "../state/store";
import { isRunning } from "../state/thread";
import { formatDuration, workingLabel } from "../state/threadFormat";
import { FileDiff } from "./icons";
import { Spinner, Tooltip } from "./primitives";
import { DiffCount } from "./Rows";

/** What the thread's turns changed, added up: the turns' own checkpoints, not the rewinds' leftovers. */
export function threadChanges(checkpoints: readonly Checkpoint[] | undefined) {
  let turns = 0;
  let added = 0;
  let removed = 0;
  for (const c of checkpoints ?? []) {
    if (c.attempt || !c.filesChanged) continue;
    turns++;
    added += c.insertions;
    removed += c.deletions;
  }
  return { turns, added, removed };
}

/** The strip's note while a turn runs: the step it is on and for how long, readable with the chat scrolled away. */
export function RunningNote({ threadId }: { threadId: string }) {
  const running = useApp((s) => isRunning(s.threads[threadId]));
  const label = useApp((s) => {
    const thread = s.threads[threadId];
    return thread && isRunning(thread) ? workingLabel(thread.items, true) : "";
  });
  const startedAt = useApp((s) => {
    const thread = s.threads[threadId];
    return thread?.turn?.startedAt ?? thread?.enginePhase?.startedAt;
  });
  const now = useNow(running);
  if (!running) return null;
  const elapsed = startedAt ? formatDuration(Math.floor(Math.max(0, now - startedAt) / 1000) * 1000) : null;
  return (
    <span role="status" className="flex h-7 min-w-0 items-center gap-1.5 px-1 text-fg-muted rc-in">
      <Spinner size={12} />
      <span className="rc-shimmer min-w-0 truncate">{label || "Working"}</span>
      {elapsed && <span className="shrink-0 text-fg-faint tabular">{elapsed}</span>}
    </span>
  );
}

/** The thread's changes so far as one chip; a press opens Review. */
export function ChangesChip({ threadId }: { threadId: string }) {
  const checkpoints = useApp((s) => s.checkpoints[threadId]);
  const { turns, added, removed } = threadChanges(checkpoints);
  if (!turns || (!added && !removed)) return null;
  return (
    <Tooltip side="bottom" label={`Changed by ${turns} turn${turns === 1 ? "" : "s"} · Review changes`}>
      <button
        type="button"
        aria-label={`Review changes: ${added} added, ${removed} removed`}
        onClick={() => setView({ kind: "review", threadId })}
        className="flex h-7 shrink-0 items-center gap-1.5 rounded-full px-2 text-fg-muted hover:bg-fill hover:text-fg"
      >
        <FileDiff size={14} />
        <DiffCount added={added} removed={removed} />
      </button>
    </Tooltip>
  );
}
