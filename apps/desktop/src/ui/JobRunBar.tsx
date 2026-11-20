// Above the field of a thread a scheduled job's run started: which job it was, and the way back to
// it. A quiet run settles itself, so a generic "Settled" bar there read as if the user had settled
// an ordinary thread; this one says what the thread is instead.

import { isJobRunThread, useLoopThreads } from "../state/loops";
import { useApp } from "../state/store";
import { Repeat } from "./icons";
import { showJob } from "./Loops";
import { Pill } from "./primitives";
import { useThreadStatus } from "./Status";

/** The job a thread is a run of, by id and name, or null for any other thread. */
export function useJobRun(threadId: string): { id?: string; name?: string } | null {
  const session = useApp((s) => s.sessions.find((item) => item.id === threadId));
  const runs = useLoopThreads((s) => s.runs);
  const id = useLoopThreads((s) => s.loopOf[threadId]) ?? session?.scheduledJob;
  const name = useLoopThreads((s) => s.byThread[threadId]);
  if (!isJobRunThread({ id: threadId, scheduledJob: session?.scheduledJob }, runs)) return null;
  return { id, name };
}

export function JobRunBar({ threadId }: { threadId: string }) {
  const run = useJobRun(threadId);
  const status = useThreadStatus(threadId);
  if (!run) return null;
  return (
    <div
      role="status"
      className="rc-glass-overlay relative flex h-[32px] items-center gap-2 rounded-full pr-1.5 pl-3 text-callout rc-in"
    >
      <Repeat size={12} className="shrink-0 text-fg-muted" />
      <span className="min-w-0 flex-1 truncate text-fg-muted">
        <span className="font-medium">Job run</span>
        {run.name && <> of “{run.name}”</>}
        {status === "done" && <span className="text-fg-faint"> · finished</span>}
      </span>
      {run.id && (
        <Pill size="sm" quiet onClick={() => run.id && showJob(run.id)}>
          View job
        </Pill>
      )}
    </div>
  );
}
