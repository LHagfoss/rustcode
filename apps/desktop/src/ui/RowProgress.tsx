// A detailed sidebar row's third line, before the branch: how far a working thread is through its
// plan (a short bar and `3/4 · Run the checks`), or why one that is not working has not finished
// (`Limit resets 14:00`, `Waits for ‹thread›`). The wording is pure (state/sidebarRows.ts).

import { useMemo } from "react";
import { rowProgress, rowWaitReason, type StepMark } from "../state/sidebarRows";
import { clockLabel } from "../state/snooze";
import { useApp } from "../state/store";
import type { Waiting } from "../state/waiting";
import { cx } from "./primitives";

/**
 * A row's progress while it works, else its wait reason. `quiet` is a row that says something
 * else, or nothing: one that asks for you, or one settled by your word.
 */
export function useRowProgress(id: string, running: boolean, quiet: boolean, wait: Waiting | undefined) {
  const plan = useApp((s) => s.threads[id]?.plan);
  const limit = useApp((s) => s.limitWaits[id]);
  const offline = useApp((s) => !!s.internetWaits[id]);
  const progress = useMemo(() => (running ? rowProgress(plan) : undefined), [running, plan]);
  const waitReason = useMemo(
    () =>
      running || quiet
        ? undefined
        : rowWaitReason({
            limit,
            offline,
            threads: wait ? [...new Map(wait.links.map((l) => [l.target, l.targetTitle])).values()] : [],
            commands: wait?.commands,
            clock: clockLabel,
          }),
    [running, quiet, limit, offline, wait],
  );
  return { progress, waitReason };
}

/**
 * A working thread's plan as a short bar: one segment per step, the done ones solid, the one it is
 * on in the accent, the rest faint. Nothing past `MAX_STEP_MARKS`; the words beside it still count.
 */
export function StepBar({ marks }: { marks: StepMark[] }) {
  if (!marks.length) return null;
  return (
    <span aria-hidden className="flex shrink-0 items-center gap-[3px]">
      {marks.map((mark, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: a plan's steps have no ids, and keep their order
          key={i}
          className={cx(
            "h-[3px] w-2.5 rounded-full",
            mark === "done" ? "bg-fg-muted" : mark === "active" ? "bg-accent" : "bg-fill",
          )}
        />
      ))}
    </span>
  );
}
