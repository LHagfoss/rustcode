// A thread's status as a small chip: running, needs input, review, done
// (docs/orchestrator.md, "Thread status"). Idle shows nothing.

import { CircleCheck, Eye, Hand } from "lucide-react";
import { STATUS_LABEL, type ThreadStatus } from "../acp/v2";
import { statusOf, useApp } from "../state/store";
import { cx, Spinner } from "./primitives";

const TONE: Record<ThreadStatus, string> = {
  idle: "",
  running: "bg-info/12 text-info",
  needs_input: "bg-attention/14 text-attention",
  review: "bg-brand/16 text-brand",
  done: "bg-success/12 text-success",
};

export function StatusIcon({ status, size = 11 }: { status: ThreadStatus; size?: number }) {
  if (status === "running") return <Spinner size={size} className="text-info" />;
  if (status === "needs_input") return <Hand size={size} />;
  if (status === "review") return <Eye size={size} />;
  if (status === "done") return <CircleCheck size={size} />;
  return null;
}

export function StatusChip({
  status,
  compact,
  className,
}: {
  status: ThreadStatus;
  compact?: boolean;
  className?: string;
}) {
  if (status === "idle") return null;
  return (
    <span
      className={cx(
        "inline-flex h-[18px] shrink-0 items-center gap-1 rounded-full font-medium text-caption transition-colors duration-150",
        compact ? "w-[18px] justify-center" : "px-1.5",
        TONE[status],
        className,
      )}
      title={STATUS_LABEL[status]}
    >
      <StatusIcon status={status} size={10} />
      {!compact && STATUS_LABEL[status]}
    </span>
  );
}

/** The live status of one thread. */
export function useThreadStatus(id: string | undefined): ThreadStatus {
  return useApp((s) => (id ? statusOf(s, id) : "idle"));
}
