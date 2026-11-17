import { useCallback, useEffect, useState } from "react";

/** One async operation; starting a retry replaces its previous failure. */
export type Op<T = never> =
  | { status: "idle"; data?: T }
  | { status: "running"; data?: T }
  | { status: "failed"; error: string; data?: T };

export function opError(op: Op<unknown>): string | null {
  return op.status === "failed" ? op.error : null;
}

export function opRunning(op: Op<unknown>): boolean {
  return op.status === "running";
}

/** What happens to an operation: it starts, answers, fails, or is put back to idle. */
export type OpEvent<T> =
  | { type: "start" }
  | { type: "succeed"; data: T }
  | { type: "fail"; error: string }
  | { type: "reset" };

/**
 * The one transition for an operation. A start clears the last failure and
 * keeps the last data, so a retry shows the previous answer under its spinner
 * instead of a blank; a reset forgets both.
 */
export function opTransition<T>(op: Op<T>, event: OpEvent<T>): Op<T> {
  switch (event.type) {
    case "start":
      return { status: "running", data: op.data };
    case "succeed":
      return { status: "idle", data: event.data };
    case "fail":
      return { status: "failed", error: event.error, data: op.data };
    case "reset":
      return { status: "idle" };
  }
}

/** How one run ended: with its answer, a failure, or dropped because a newer run (or unmount) replaced it. */
export type OpResult<T> =
  | { status: "done"; value: T }
  | { status: "failed"; error: string }
  | { status: "stale" };

/** What a failure says in words, unless the caller names its own wording. */
export function failureText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export interface RunOptions {
  /** Turns a thrown error into the text the operation keeps. */
  describe?: (error: unknown) => string;
}

export interface OpRunner<T> {
  run: (action: () => Promise<T>, options?: RunOptions) => Promise<OpResult<T>>;
  /** Back to idle with no data; an answer still on its way is dropped. */
  reset: () => void;
  /** The owner is mounted: answers are applied again. */
  attach: () => void;
  /** The owner went away: every answer still on its way is dropped. */
  detach: () => void;
}

/**
 * Runs operations against one `Op`, applying only the newest run's answer.
 * Pure of React so a test can drive it; `useOp` wires it to component state.
 */
export function createOpRunner<T>(apply: (event: OpEvent<T>) => void): OpRunner<T> {
  let generation = 0;
  let attached = true;
  return {
    async run(action, options) {
      const mine = ++generation;
      const current = () => attached && mine === generation;
      apply({ type: "start" });
      try {
        const value = await action();
        if (!current()) return { status: "stale" };
        apply({ type: "succeed", data: value });
        return { status: "done", value };
      } catch (failure) {
        if (!current()) return { status: "stale" };
        const error = (options?.describe ?? failureText)(failure);
        apply({ type: "fail", error });
        return { status: "failed", error };
      }
    },
    reset() {
      generation++;
      apply({ type: "reset" });
    },
    attach() {
      attached = true;
    },
    detach() {
      attached = false;
      generation++;
    },
  };
}

/**
 * Component state for a one-shot request: the `Op`, a `run` that drops an
 * answer arriving after a newer run or after unmount, and a `reset`.
 * `run` and `reset` keep their identity across renders.
 */
export function useOp<T = void>(): [Op<T>, OpRunner<T>["run"], () => void] {
  const [op, setOp] = useState<Op<T>>({ status: "idle" });
  const [runner] = useState(() => createOpRunner<T>((event) => setOp((prev) => opTransition(prev, event))));
  useEffect(() => {
    runner.attach();
    return runner.detach;
  }, [runner]);
  const run = useCallback<OpRunner<T>["run"]>((action, options) => runner.run(action, options), [runner]);
  return [op, run, runner.reset];
}
